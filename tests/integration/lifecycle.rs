//! End-to-end lifecycle test: create → author → validate → render →
//! merge, driving the compiled binary against a scratch notebook and
//! a scratch project repository.
//!
//! The binary runs with its working directory inside a scratch git
//! repository, so the resolved project root — and therefore every
//! merge write — stays inside the test sandbox. The scratch notebook
//! lives in a per-test isolated `NB_DIR` (see `super::harness`); the
//! directory is removed on drop, so the operator's real notebook
//! list never sees scratch notebooks from this test.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::harness::{IsolatedNbDir, scrub_git_env};

const TEMP_TEST_ROOT: &str = ".auxiliary/temporary/tests";
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

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// A scratch nb notebook, isolated to a per-test `NB_DIR` and
/// removed on drop along with the directory.
struct ScratchNotebook {
    name: String,
    nb_dir: IsolatedNbDir,
}

impl ScratchNotebook {
    fn create() -> Self {
        let nb_dir = IsolatedNbDir::new();
        let name = format!("nbspec-itest-{}", unique_suffix());
        let mut command = Command::new("nb");
        scrub_git_env(&mut command);
        let output = command
            .env("NB_DIR", nb_dir.path())
            .args(["notebooks", "add", &name])
            .output()
            .expect("nb must be installed for integration tests");
        assert!(output.status.success(), "cannot create scratch notebook");
        ScratchNotebook { name, nb_dir }
    }

    /// Filesystem path of the notebook directory inside the
    /// isolated `NB_DIR`.
    fn path(&self) -> PathBuf {
        let mut command = Command::new("nb");
        scrub_git_env(&mut command);
        let output = command
            .env("NB_DIR", self.nb_dir.path())
            .args(["notebooks", "--paths", "--no-color"])
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&output.stdout);
        listing
            .lines()
            .map(str::trim)
            .find(|line| line.ends_with(&self.name))
            .map(PathBuf::from)
            .expect("scratch notebook path must be listed")
    }

    /// The isolated `NB_DIR`; passed to the subprocess harness so
    /// the `nbspec serve mcp` (and any other spawned nbspec
    /// process) targets the same isolated root.
    fn nb_dir_path(&self) -> &Path {
        self.nb_dir.path()
    }
}

impl Drop for ScratchNotebook {
    fn drop(&mut self) {
        // Best-effort: retry the delete because transient
        // contention can fail an `nb notebooks delete`.
        for _ in 0..3 {
            let mut command = Command::new("nb");
            scrub_git_env(&mut command);
            let deleted = command
                .env("NB_DIR", self.nb_dir.path())
                .args(["notebooks", "delete", &self.name, "--force"])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);
            if deleted {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        // IsolatedNbDir::drop removes the directory itself.
    }
}

/// A scratch project repository: an initialized git repository with a
/// project configuration keeping render scratch inside the sandbox.
struct ScratchProject {
    root: PathBuf,
}

impl ScratchProject {
    fn create() -> Self {
        let root = PathBuf::from(TEMP_TEST_ROOT)
            .join(format!("lifecycle-{}", unique_suffix()))
            .canonicalize_base();
        std::fs::create_dir_all(&root).unwrap();
        let mut command = Command::new("git");
        scrub_git_env(&mut command);
        let output = command
            .args(["init", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(output.status.success(), "cannot initialize scratch repo");
        let configuration_directory = root.join(".auxiliary/configuration/nbspec");
        std::fs::create_dir_all(&configuration_directory).unwrap();
        // Pins every setting the test depends on at the
        // highest-precedence layer, so an operator's user-global
        // configuration cannot change test behavior.
        std::fs::write(
            configuration_directory.join("general.toml"),
            "schema = \"nbspec-default\"\n\
             scratch_directory = \".auxiliary/temporary/renders\"\n\
             archives = true\n\
             archive_directory = \"documentation/archives\"\n",
        )
        .unwrap();
        ScratchProject { root }
    }

    /// The sandbox configuration directory, pinned through
    /// `NBSPEC_CONFIG_DIR` so a user-global
    /// `project_configuration_directory` cannot redirect it.
    fn configuration_directory(&self) -> PathBuf {
        self.root.join(".auxiliary/configuration/nbspec")
    }
}

impl Drop for ScratchProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Anchors a relative path under the crate directory so that scratch
/// state survives the binary's differing working directory.
trait CanonicalizeBase {
    fn canonicalize_base(self) -> PathBuf;
}

impl CanonicalizeBase for PathBuf {
    fn canonicalize_base(self) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(self)
    }
}

/// Runs the nbspec binary inside the scratch project against the
/// scratch notebook. Scrubs `GIT_*` so the subprocess targets the
/// expected repository even when the test runner leaked ambient git
/// state into the parent environment.
fn nbspec(project: &ScratchProject, notebook: &ScratchNotebook, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nbspec"));
    scrub_git_env(&mut command);
    command
        .current_dir(&project.root)
        .env("NBSPEC_CONFIG_DIR", project.configuration_directory())
        .env("NB_DIR", notebook.nb_dir_path())
        .args(["--notebook", &notebook.name])
        .args(arguments)
        .output()
        .unwrap()
}

/// Like [`nbspec`], but pipes `stdin_content` to the subprocess's
/// standard input — the transport for `--comment-file -`. Same
/// hygiene as [`nbspec`]: `GIT_*` scrubbed, `NB_DIR` pinned to the
/// isolated root.
fn nbspec_with_stdin(
    project: &ScratchProject,
    notebook: &ScratchNotebook,
    arguments: &[&str],
    stdin_content: &str,
) -> Output {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut command = Command::new(env!("CARGO_BIN_EXE_nbspec"));
    scrub_git_env(&mut command);
    let mut child = command
        .current_dir(&project.root)
        .env("NBSPEC_CONFIG_DIR", project.configuration_directory())
        .env("NB_DIR", notebook.nb_dir_path())
        .args(["--notebook", &notebook.name])
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
    let notebook = ScratchNotebook::create();
    let project = ScratchProject::create();

    // Create scaffolds the namespace without touching the repository.
    let created = nbspec(
        &project,
        &notebook,
        &["create", CHANGE_ID, "--title", "Demo"],
    );
    assert!(created.status.success(), "{}", stderr_of(&created));
    assert!(stdout_of(&created).contains("Created change add-demo"));

    // A fresh change is invalid: both required artifacts unauthored.
    // Contract: exit 1, empty stdout, banner-free report on stderr.
    let invalid = nbspec(&project, &notebook, &["validate", CHANGE_ID]);
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
    let change_directory = notebook.path().join("proposals").join(CHANGE_ID);
    let mut proposal = std::fs::read_to_string(change_directory.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nProve the lifecycle.\n");
    std::fs::write(change_directory.join("proposal.md"), proposal).unwrap();
    let specification_note = change_directory.join("specifications/user-auth.md");
    std::fs::write(&specification_note, SPECIFICATION).unwrap();

    // The authored change validates: exit 0 and a one-line summary.
    let valid = nbspec(&project, &notebook, &["validate", CHANGE_ID]);
    assert!(valid.status.success(), "{}", stderr_of(&valid));
    assert!(
        stdout_of(&valid).contains(
            "Change add-demo is valid: 2 documents checked against schema nbspec-default"
        )
    );

    // Render writes the scratch tree byte-for-byte and leaves the
    // repository untouched.
    let rendered = nbspec(&project, &notebook, &["render", CHANGE_ID]);
    assert!(rendered.status.success(), "{}", stderr_of(&rendered));
    let scratch_document = project
        .root
        .join(".auxiliary/temporary/renders")
        .join(&notebook.name)
        .join(CHANGE_ID)
        .join("specifications/user-auth.md");
    assert_eq!(
        std::fs::read_to_string(&scratch_document).unwrap(),
        SPECIFICATION
    );
    assert!(!project.root.join("documentation").exists());

    // The review diff is pure git-format output for piping.
    let diffed = nbspec(&project, &notebook, &["render", CHANGE_ID, "--diff"]);
    assert!(diffed.status.success());
    let diff = stdout_of(&diffed);
    assert!(diff.starts_with(
        "diff --git a/documentation/specifications/user-auth.md \
         b/documentation/specifications/user-auth.md"
    ));
    assert!(diff.contains("+### Requirement: User authentication"));

    // An unreviewed merge refuses at the review gate: an approving
    // verdict is the merge license, and none exists yet.
    let refused = nbspec(&project, &notebook, &["merge", CHANGE_ID]);
    assert!(!refused.status.success(), "unreviewed merge must refuse");
    assert!(
        stderr_of(&refused).contains("review gate unsatisfied: no verdict"),
        "{}",
        stderr_of(&refused)
    );
    assert!(!project.root.join("documentation").exists());

    // A revise verdict without findings refuses; so does a comment
    // file that cannot be read. Findings supplied via --comment-file
    // record — and then block the merge as revise-outstanding.
    let moodless = nbspec(
        &project,
        &notebook,
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
        &project,
        &notebook,
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
    let findings_file = project.root.join("itest-findings.md");
    std::fs::write(&findings_file, "tighten the scenario wording").unwrap();
    let revised = nbspec(
        &project,
        &notebook,
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
    let blocked = nbspec(&project, &notebook, &["merge", CHANGE_ID]);
    assert!(!blocked.status.success(), "revise-outstanding must refuse");
    assert!(
        stderr_of(&blocked).contains("latest verdict is revise by itest"),
        "{}",
        stderr_of(&blocked)
    );

    // A newer approving verdict supersedes the revise and satisfies
    // the gate; its optional comment arrives inline and literally.
    let approved = nbspec(
        &project,
        &notebook,
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
    assert!(stdout_of(&approved).contains("Recorded approve verdict by itest"));

    // A second reviewer's outstanding revise coexists without blocking:
    // slice-1 policy is satisfied by any single current approval, and
    // display lists every reviewer's standing position. Its findings
    // arrive on standard input via `--comment-file -`; the display
    // assertion below proves the piped content landed in the verdict.
    let dissent = nbspec_with_stdin(
        &project,
        &notebook,
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
    let displayed = nbspec(&project, &notebook, &["display", CHANGE_ID]);
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
    let merged = nbspec(&project, &notebook, &["merge", CHANGE_ID]);
    assert!(merged.status.success(), "{}", stderr_of(&merged));
    let merge_output = stdout_of(&merged);
    assert!(merge_output.contains("wrote documentation/specifications/user-auth.md"));
    assert!(merge_output.contains("archived documentation/archives/add-demo.tar.zst"));
    assert!(merge_output.contains("warning: no .gitattributes rule"));
    let target = project
        .root
        .join("documentation/specifications/user-auth.md");
    let merged_content = std::fs::read_to_string(&target).unwrap();
    assert!(merged_content.starts_with("<!-- nbspec: change=add-demo notebook="));
    assert!(merged_content.ends_with(SPECIFICATION));
    assert!(
        project
            .root
            .join("documentation/archives/add-demo.tar.zst")
            .is_file()
    );

    // The archive preserves the review trail: every verdict note
    // rides alongside meta and work (three verdicts stand: itest's
    // superseded revise, itest's approve, qa's outstanding revise).
    let archive_bytes =
        std::fs::read(project.root.join("documentation/archives/add-demo.tar.zst")).unwrap();
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
        !project.root.join("documentation/verdicts").exists()
            && !project
                .root
                .join("documentation/specifications/verdicts")
                .exists(),
        "verdicts never materialize to the repository tree"
    );

    // Re-merge is idempotent.
    let remerged = nbspec(&project, &notebook, &["merge", CHANGE_ID]);
    assert!(remerged.status.success());
    assert!(stdout_of(&remerged).contains("unchanged documentation/specifications/user-auth.md"));

    // A hand-edited target refuses without force and nothing changes.
    let drifted_content = format!("{merged_content}\nEdited by hand.\n");
    std::fs::write(&target, &drifted_content).unwrap();
    let refused = nbspec(&project, &notebook, &["merge", CHANGE_ID]);
    assert_eq!(refused.status.code(), Some(1));
    let refusal = stderr_of(&refused);
    assert!(refusal.contains("merge refused; no files were written"));
    assert!(refusal.contains("documentation/specifications/user-auth.md"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), drifted_content);

    // Force restores the notebook's version.
    let forced = nbspec(&project, &notebook, &["merge", CHANGE_ID, "--force"]);
    assert!(forced.status.success(), "{}", stderr_of(&forced));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), merged_content);

    // Breaking the specification surfaces a line-anchored diagnostic.
    let broken = SPECIFICATION
        .split("#### Scenario:")
        .next()
        .unwrap()
        .to_string();
    std::fs::write(&specification_note, broken).unwrap();
    let rebroken = nbspec(&project, &notebook, &["validate", CHANGE_ID]);
    assert_eq!(rebroken.status.code(), Some(1));
    assert!(stderr_of(&rebroken).contains(
        "proposals/add-demo/specifications/user-auth.md:5: [specifications] \
             requirement User authentication has no #### Scenario: block"
    ));
}
