//! End-to-end lifecycle test: create → author → validate → render →
//! merge, driving the compiled binary against a scratch notebook and
//! a scratch project repository.
//!
//! The binary runs with its working directory inside a scratch git
//! repository (the fixture's `working_dir`), so the resolved project
//! root — and therefore every merge write — stays inside the test
//! sandbox. The scratch notebook lives in nb-api's isolated
//! [`NbTestEnv`] `NB_DIR` (see `super::harness`); the fixture root is
//! removed on drop, so the operator's real notebook list and repo are
//! never touched.

use std::process::{Command, Output};

use super::harness::Fixture;

const CHANGE_ID: &str = "add-demo";

const SPECIFICATION: &str = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users before granting access.

#### Scenario: Valid login
- **WHEN** a user submits correct credentials
- **THEN** a session begins
";

/// Runs the nbspec binary inside the scratch project against the
/// scratch notebook, with the fixture's environment applied (scrubs
/// `GIT_*`, sets `NB_DIR`/`HOME`/`PATH`/git identity, and `cwd` to the
/// project root).
fn nbspec(fixture: &Fixture, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nbspec"));
    fixture.configure_std(&mut command);
    command
        .env("NBSPEC_CONFIG_DIR", fixture.configuration_directory())
        .args(["--notebook", fixture.notebook()])
        .args(arguments)
        .output()
        .unwrap()
}

/// Like [`nbspec`], but pipes `stdin_content` to the subprocess's
/// standard input — the transport for `--comment-file -`. Same
/// hygiene as [`nbspec`]: fixture environment applied, `NB_DIR`
/// pinned to the isolated root.
fn nbspec_with_stdin(fixture: &Fixture, arguments: &[&str], stdin_content: &str) -> Output {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut command = Command::new(env!("CARGO_BIN_EXE_nbspec"));
    fixture.configure_std(&mut command);
    let mut child = command
        .env("NBSPEC_CONFIG_DIR", fixture.configuration_directory())
        .args(["--notebook", fixture.notebook()])
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_content.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn change_lifecycle_end_to_end() {
    let fixture = Fixture::new();

    // Create scaffolds the namespace without touching the repository.
    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    assert!(stdout_of(&created).contains("Created change add-demo"));

    // A fresh change is invalid: both required artifacts unauthored.
    // Contract: exit 1, empty stdout, banner-free report on stderr.
    let invalid = nbspec(&fixture, &["validate", CHANGE_ID]);
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(stdout_of(&invalid), "");
    let report = stderr_of(&invalid);
    assert!(!report.contains("Error:"), "unexpected banner: {report}");
    let lines: Vec<&str> = report.lines().collect();
    assert_eq!(lines[0], "change add-demo is invalid: 2 violations");
    assert!(lines[1].starts_with("proposals/add-demo/proposal.md: [proposal]"));
    assert!(lines[2].starts_with("proposals/add-demo/specifications/: [specifications]"));

    // Author the proposal and one delta specification directly on the
    // notebook filesystem, as an agent's editor would.
    let change_directory = fixture.notebook_path().join("proposals").join(CHANGE_ID);
    let mut proposal = std::fs::read_to_string(change_directory.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nProve the lifecycle.\n");
    std::fs::write(change_directory.join("proposal.md"), proposal).unwrap();
    let specification_note = change_directory.join("specifications/user-auth.md");
    std::fs::write(&specification_note, SPECIFICATION).unwrap();

    // The authored change validates: exit 0 and a one-line summary.
    let valid = nbspec(&fixture, &["validate", CHANGE_ID]);
    assert!(valid.status.success(), "{}", stderr_of(&valid));
    assert!(
        stdout_of(&valid).contains(
            "Change add-demo is valid: 2 documents checked against schema nbspec-default"
        )
    );

    // Render writes the scratch tree byte-for-byte and leaves the
    // repository untouched.
    let rendered = nbspec(&fixture, &["render", CHANGE_ID]);
    assert!(rendered.status.success(), "{}", stderr_of(&rendered));
    let scratch_document = fixture
        .project_root()
        .join(".auxiliary/temporary/renders")
        .join(fixture.notebook())
        .join(CHANGE_ID)
        .join("specifications/user-auth.md");
    assert_eq!(
        std::fs::read_to_string(&scratch_document).unwrap(),
        SPECIFICATION
    );
    assert!(!fixture.project_root().join("documentation").exists());

    // The review diff is pure git-format output for piping.
    let diffed = nbspec(&fixture, &["render", CHANGE_ID, "--diff"]);
    assert!(diffed.status.success());
    let diff = stdout_of(&diffed);
    assert!(diff.starts_with(
        "diff --git a/documentation/specifications/user-auth.md \
         b/documentation/specifications/user-auth.md"
    ));
    assert!(diff.contains("+### Requirement: User authentication"));

    // An unreviewed merge refuses at the review gate: an approving
    // verdict is the merge license, and none exists yet.
    let refused = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(!refused.status.success(), "unreviewed merge must refuse");
    assert!(
        stderr_of(&refused).contains("review gate unsatisfied: no verdict"),
        "{}",
        stderr_of(&refused)
    );
    assert!(!fixture.project_root().join("documentation").exists());

    // A revise verdict without findings refuses; so does a comment
    // file that cannot be read. Findings supplied via --comment-file
    // record — and then block the merge as revise-outstanding.
    let moodless = nbspec(
        &fixture,
        &[
            "review",
            CHANGE_ID,
            "--verdict",
            "revise",
            "--reviewer",
            "itest",
        ],
    );
    assert!(
        !moodless.status.success(),
        "comment-less revise must refuse"
    );
    assert!(stderr_of(&moodless).contains("requires a comment"));
    let unreadable = nbspec(
        &fixture,
        &[
            "review",
            CHANGE_ID,
            "--verdict",
            "revise",
            "--reviewer",
            "itest",
            "--comment-file",
            "absent-findings.md",
        ],
    );
    assert!(
        !unreadable.status.success(),
        "unreadable comment file must refuse"
    );
    assert!(stderr_of(&unreadable).contains("cannot read the review comment file"));
    let findings_file = fixture.project_root().join("itest-findings.md");
    std::fs::write(&findings_file, "tighten the scenario wording").unwrap();
    let revised = nbspec(
        &fixture,
        &[
            "review",
            CHANGE_ID,
            "--verdict",
            "revise",
            "--reviewer",
            "itest",
            "--comment-file",
            "itest-findings.md",
        ],
    );
    assert!(revised.status.success(), "{}", stderr_of(&revised));
    std::fs::remove_file(&findings_file).unwrap();
    let blocked = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(!blocked.status.success(), "revise-outstanding must refuse");
    assert!(
        stderr_of(&blocked).contains("latest verdict is revise by itest"),
        "{}",
        stderr_of(&blocked)
    );

    // A newer approving verdict supersedes the revise and satisfies
    // the gate; its optional comment arrives inline and literally.
    let approved = nbspec(
        &fixture,
        &[
            "review",
            CHANGE_ID,
            "--verdict",
            "approve",
            "--reviewer",
            "itest",
            "--comment",
            "supersedes after rework",
        ],
    );
    assert!(approved.status.success(), "{}", stderr_of(&approved));
    let approved_output = stdout_of(&approved);
    assert!(approved_output.contains("Recorded approve verdict by itest"));

    // The recorded verdict's `note={verdicts_folder}/{name}.md`
    // structured output MUST point at a file that actually exists
    // and whose basename equals the verdict id — the title is the
    // single source of truth for the verdict's identity, and the
    // on-disk filename is `nb`'s derivation of that title. This
    // regression-guards the `add_note(None, ...)` → `add_note(
    // Some(&name), ...)` switch in the verdict writer.
    let verdict_note_qualified = approved_output
        .lines()
        .find_map(|line| line.strip_prefix("note=").map(str::to_string))
        .expect("approved verdict output must contain `note=...` line");
    // The `note=` line is `<notebook>:<relative-path>` (per `nb`'s
    // qualified-path output). The test resolves relative to the
    // notebook root, so strip the `<notebook>:` prefix.
    let verdict_note_path = verdict_note_qualified
        .split_once(':')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or(verdict_note_qualified);
    let verdict_note_filename = std::path::Path::new(&verdict_note_path)
        .file_name()
        .and_then(|name| name.to_str())
        .expect("verdict note path must have a UTF-8 basename");
    let absolute_verdict_note = fixture.notebook_path().join(&verdict_note_path);
    assert!(
        absolute_verdict_note.is_file(),
        "recorded verdict note path must exist on disk: {verdict_note_path} \
         (resolved: {})",
        absolute_verdict_note.display()
    );
    assert!(
        !verdict_note_filename.is_empty() && verdict_note_filename.ends_with(".md"),
        "verdict note filename must be a non-empty `.md` basename: {verdict_note_filename:?}"
    );
    // The basename (without `.md`) is the verdict id the gate binds to.
    let verdict_id = verdict_note_filename
        .strip_suffix(".md")
        .expect(".md suffix");
    assert!(
        !verdict_id.is_empty(),
        "verdict id (basename without `.md`) must be non-empty"
    );
    // Verdict ids: `<YYYYMMDDHHMMSS>-<pid-hex>-<6-hex>-<seq-hex>`.
    assert!(
        nbspec::reviews::is_verdict_note_id(verdict_id),
        "verdict id must match collision-resistant note-id shape, got: {verdict_id:?}"
    );
    // Body assertion per MCP Owner's [P1] regression spec:
    // - starts with `# {verdict_id}\n\n```json\n` (nb materializes
    //   the title as the leading H1, so the H1 is materializing
    //   exactly once — NOT duplicated)
    // - contains exactly one fenced JSON payload
    // - does NOT contain a second duplicate `# {verdict_id}` heading
    //   (which would be the `DuplicateTitleHeading` failure mode
    //   that the verdict writer's body change guards against)
    let body = std::fs::read_to_string(&absolute_verdict_note).expect("read verdict note body");
    let expected_prefix = format!("# {verdict_id}\n\n```json\n");
    let expected_prefix_crlf = format!("# {verdict_id}\r\n\r\n```json\r\n");
    assert!(
        body.starts_with(&expected_prefix) || body.starts_with(&expected_prefix_crlf),
        "verdict body must start with `# {{verdict_id}}\\n\\n```json\\n` \
         (or the line-ending-normalized CRLF equivalent); body starts with: {:?}",
        body.chars().take(80).collect::<String>()
    );
    // Strip line endings for the count / containment assertions so
    // the regression is robust to either LF or CRLF terminators.
    let body_normalized = body.replace("\r\n", "\n");
    let fence_count = body_normalized.matches("```json").count();
    assert_eq!(
        fence_count, 1,
        "verdict body must contain exactly one fenced JSON payload (got {fence_count}):\n{body}"
    );
    let duplicate_h1_count = body_normalized
        .matches(&format!("# {verdict_id}\n"))
        .count();
    assert_eq!(
        duplicate_h1_count, 1,
        "verdict body must contain the title-derived H1 exactly once \
         (got {duplicate_h1_count}); body:\n{body}"
    );

    // A second reviewer's outstanding revise coexists without blocking:
    // slice-1 policy is satisfied by any single current approval, and
    // display lists every reviewer's standing position. Its findings
    // arrive on standard input via `--comment-file -`; the display
    // assertion below proves the piped content landed in the verdict.
    let dissent = nbspec_with_stdin(
        &fixture,
        &[
            "review",
            CHANGE_ID,
            "--verdict",
            "revise",
            "--reviewer",
            "qa",
            "--comment-file",
            "-",
        ],
        "prefer stronger scenario names",
    );
    assert!(dissent.status.success(), "{}", stderr_of(&dissent));
    let displayed = nbspec(&fixture, &["display", CHANGE_ID]);
    assert!(displayed.status.success(), "{}", stderr_of(&displayed));
    let display_output = stdout_of(&displayed);
    assert!(display_output.contains("## review"), "{display_output}");
    assert!(
        display_output.contains("merge: approve by itest (current,"),
        "{display_output}"
    );
    assert!(
        display_output.contains("merge: revise by qa (outstanding,"),
        "{display_output}"
    );
    assert!(display_output.contains("prefer stronger scenario names"));

    // Merge transfers the durable document with provenance and writes
    // the change archive; the missing LFS rule draws a warning.
    let merged = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(merged.status.success(), "{}", stderr_of(&merged));
    let merge_output = stdout_of(&merged);
    assert!(merge_output.contains("wrote documentation/specifications/user-auth.md"));
    assert!(merge_output.contains("archived documentation/archives/add-demo.tar.zst"));
    assert!(merge_output.contains("warning: no .gitattributes rule"));
    let target = fixture
        .project_root()
        .join("documentation/specifications/user-auth.md");
    let merged_content = std::fs::read_to_string(&target).unwrap();
    assert!(merged_content.starts_with("<!-- nbspec: change=add-demo notebook="));
    assert!(merged_content.ends_with(SPECIFICATION));
    assert!(
        fixture
            .project_root()
            .join("documentation/archives/add-demo.tar.zst")
            .is_file()
    );

    // The archive preserves the review trail: every verdict note
    // rides alongside meta and work (three verdicts stand: itest's
    // superseded revise, itest's approve, qa's outstanding revise).
    let archive_bytes = std::fs::read(
        fixture
            .project_root()
            .join("documentation/archives/add-demo.tar.zst"),
    )
    .unwrap();
    let decompressed = zstd::decode_all(archive_bytes.as_slice()).unwrap();
    let mut archive = tar::Archive::new(decompressed.as_slice());
    let entry_paths: Vec<String> = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().path().unwrap().display().to_string())
        .collect();
    assert!(entry_paths.iter().any(|path| path == "add-demo/meta.md"));
    assert!(entry_paths.iter().any(|path| path == "add-demo/work.md"));
    assert_eq!(
        entry_paths
            .iter()
            .filter(|path| path.starts_with("add-demo/verdicts/"))
            .count(),
        3,
        "all three verdict notes must be archived: {entry_paths:?}"
    );
    assert!(
        !fixture
            .project_root()
            .join("documentation/verdicts")
            .exists()
            && !fixture
                .project_root()
                .join("documentation/specifications/verdicts")
                .exists(),
        "verdicts never materialize to the repository tree"
    );

    // Re-merge is idempotent.
    let remerged = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(remerged.status.success());
    assert!(stdout_of(&remerged).contains("unchanged documentation/specifications/user-auth.md"));

    // A hand-edited target refuses without force and nothing changes.
    let drifted_content = format!("{merged_content}\nEdited by hand.\n");
    std::fs::write(&target, &drifted_content).unwrap();
    let refused = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert_eq!(refused.status.code(), Some(1));
    let refusal = stderr_of(&refused);
    assert!(refusal.contains("merge refused; no files were written"));
    assert!(refusal.contains("documentation/specifications/user-auth.md"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), drifted_content);

    // Force restores the notebook's version.
    let forced = nbspec(&fixture, &["merge", CHANGE_ID, "--force"]);
    assert!(forced.status.success(), "{}", stderr_of(&forced));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), merged_content);

    // Breaking the specification surfaces a line-anchored diagnostic.
    let broken = SPECIFICATION
        .split("#### Scenario:")
        .next()
        .unwrap()
        .to_string();
    std::fs::write(&specification_note, broken).unwrap();
    let rebroken = nbspec(&fixture, &["validate", CHANGE_ID]);
    assert_eq!(rebroken.status.code(), Some(1));
    assert!(stderr_of(&rebroken).contains(
        "proposals/add-demo/specifications/user-auth.md:5: [specifications] \
             requirement User authentication has no #### Scenario: block"
    ));
}

/// Regression for issues/1 + issues/8: `display` constructed nb
/// selectors using filename stems (e.g. `proposal`) instead of full
/// filenames (`proposal.md`). When a proposal note's H1 diverged from
/// the literal stem, `nb` could not resolve the selector, and
/// `artifact_has_content` swallowed the error as "no content" —
/// reporting an authored proposal as "ready to author".
#[test]
fn display_reports_authored_when_h1_diverges_from_selector_stem() {
    let fixture = Fixture::new();

    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));

    // Author the proposal with an H1 that does NOT match the note
    // filename stem ("proposal"). Before the fix, this caused nb to
    // fail selector resolution, and display reported "ready to author".
    let change_directory = fixture.notebook_path().join("proposals").join(CHANGE_ID);
    std::fs::write(
        change_directory.join("proposal.md"),
        "# A Human-Readable Title\n\n## Why\n\nBody text that counts as authored.\n",
    )
    .unwrap();

    let displayed = nbspec(&fixture, &["display", CHANGE_ID]);
    assert!(displayed.status.success(), "{}", stderr_of(&displayed));
    let output = stdout_of(&displayed);
    assert!(
        output.contains("- proposal: authored"),
        "expected authored state, got: {output}"
    );

    // display --full must also succeed (reads the note via show_note).
    let full_displayed = nbspec(&fixture, &["display", "--full", CHANGE_ID]);
    assert!(
        full_displayed.status.success(),
        "{}",
        stderr_of(&full_displayed)
    );
    let full_output = stdout_of(&full_displayed);
    assert!(
        full_output.contains("A Human-Readable Title"),
        "expected proposal content in --full output: {full_output}"
    );
}
/// Missing note/folder must classify as absence (ready/empty), not
/// unreadable, on both short and `--full` display.
///
/// Non-not-found → unreadable classification is locked by unit tests
/// of `classify_note_content` / `classify_folder_listing` with
/// synthetic `CommandFailed` payloads. Real-nb injection is not
/// reliable on nb 7.24.0: `nb show` can serve deleted paths from git,
/// directory-as-note can still resolve authored content, chmod is
/// ignored for show via git, and `nb ls` exits 0 with `0 items` on
/// unreadable directories.
#[test]
fn display_classifies_missing_note_and_folder_as_absence() {
    let fixture = Fixture::new();

    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));

    let change_directory = fixture.notebook_path().join("proposals").join(CHANGE_ID);

    // --- missing note: delete proposal.md ---
    std::fs::remove_file(change_directory.join("proposal.md")).unwrap();
    let displayed = nbspec(&fixture, &["display", CHANGE_ID]);
    assert!(displayed.status.success(), "{}", stderr_of(&displayed));
    let output = stdout_of(&displayed);
    assert!(
        output.contains("- proposal: ready to author"),
        "missing proposal must be ready, not unreadable: {output}"
    );
    assert!(
        !output.contains("unreadable"),
        "missing must not report unreadable: {output}"
    );

    // --- missing folder: remove specifications/ ---
    let specs = change_directory.join("specifications");
    if specs.exists() {
        std::fs::remove_dir_all(&specs).unwrap();
    }
    let full_missing = nbspec(&fixture, &["display", "--full", CHANGE_ID]);
    assert!(
        full_missing.status.success(),
        "{}",
        stderr_of(&full_missing)
    );
    let full_missing_out = stdout_of(&full_missing);
    assert!(
        full_missing_out.contains("## specifications/") && full_missing_out.contains("(empty)"),
        "missing folder must render (empty): {full_missing_out}"
    );
    assert!(
        !full_missing_out.contains("(unreadable:"),
        "missing folder must not be unreadable: {full_missing_out}"
    );
    assert!(
        full_missing_out.contains("(missing)") || full_missing_out.contains("## proposal"),
        "full display must tolerate missing proposal note: {full_missing_out}"
    );
}

/// Regression for reviews/8: the FIRST review on a change must report
/// the authoritative QUALIFIED note path (`<notebook>:<folder>/<file>`)
/// in both text and structured output. Before the fix, `review` found
/// the note op via `outcome.ops.first()`; when the `verdicts/` folder
/// was created in the same transaction (first review), op zero was the
/// folder op with no selector, so the reported `note=` fell back to an
/// unqualified `verdicts/<name>.md`.
#[test]
fn first_review_reports_qualified_note_path() {
    let fixture = Fixture::new();

    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));

    // First review on the change: the `verdicts/` folder does not
    // exist yet, so it is created inside the same transaction.
    let reviewed = nbspec(
        &fixture,
        &[
            "review",
            CHANGE_ID,
            "--verdict",
            "approve",
            "--reviewer",
            "itest",
            "--comment",
            "first review",
        ],
    );
    assert!(reviewed.status.success(), "{}", stderr_of(&reviewed));
    let output = stdout_of(&reviewed);

    // The text `note=` MUST be the qualified `<notebook>:<path>`
    // selector, never a bare `verdicts/<name>.md`.
    let note_line = output
        .lines()
        .find_map(|line| line.strip_prefix("note=").map(str::to_string))
        .expect("review output must contain `note=...` line");
    let qualified_prefix = format!("{}:proposals/{CHANGE_ID}/verdicts/", fixture.notebook());
    assert!(
        note_line.starts_with(&qualified_prefix),
        "first-review note= must be qualified as \
         `<notebook>:proposals/<change>/verdicts/<name>.md`, got: {note_line:?}"
    );

    // The resolved on-disk file must exist under the notebook.
    let relative_path = note_line
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(&note_line);
    assert!(
        fixture.notebook_path().join(relative_path).is_file(),
        "recorded verdict note must exist on disk: {relative_path}"
    );
}
