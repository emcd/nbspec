//! End-to-end interchange tests: `nbspec import` and `nbspec
//! export` driving a fresh scratch notebook against the canonical
//! fixture under `tests/fixtures/import-classic/`.
//!
//! The fixture contains one active change (`add-foo/`) with the
//! full classic opsx layout and one legacy archive tree
//! (`openspec/changes/archive/legacy/`). Each test pulls from the
//! fixture via a symlink so the integration surface exercises the
//! documented tree shape rather than a per-test fabrication.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::harness::Fixture;

const FIXTURE_ROOT: &str = "tests/fixtures/import-classic";
const ACTIVE_CHANGE: &str = "add-foo";
const ARCHIVE_CHANGE: &str = "legacy";

fn nbspec(fixture: &Fixture, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nbspec"));
    fixture.configure_std(&mut command);
    command
        .args(["--notebook", fixture.notebook()])
        .args(arguments)
        .output()
        .unwrap()
}

/// Symlinks the canonical fixture tree into a scratch fixture's
/// `import-classic/` directory. The fixture is read-only in git;
/// the symlink lets each test exercise it without copying bytes.
fn link_fixture(fixture: &Fixture) -> PathBuf {
    let target = fixture.project_root().join("import-classic");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_ROOT),
        &target,
    )
    .expect("symlinking fixture should succeed");
    target
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Seeds a notebook change (`add-foo` shape) directly onto the
/// filesystem, bypassing `nbspec import`. The v0.3.0 `import` verb
/// pauses active filesystem-tree ingest (pending the NbApi 0.3
/// transaction/checkpoint primitive); tests that need an existing
/// notebook change to exercise `export` use this helper to set up
/// the change directly. The seed writes proposal.md, meta.md,
/// spec.md, design.md, and decision files in the same layout the
/// resumed `import` would produce; tests that need a different
/// shape can write additional files after calling this.
fn seed_notebook_change(fixture: &Fixture, change_id: &str) {
    let change_path = fixture.notebook_path().join("proposals").join(change_id);
    std::fs::create_dir_all(&change_path).unwrap();
    std::fs::write(
        change_path.join("proposal.md"),
        format!("# {change_id}\n\nbody\n"),
    )
    .unwrap();
    std::fs::write(
        change_path.join("meta.md"),
        format!("# {change_id}\n\n```json\n{{\"change_id\":\"{change_id}\"}}\n```\n"),
    )
    .unwrap();
    std::fs::create_dir_all(change_path.join("specifications")).unwrap();
    std::fs::write(
        change_path.join("specifications/user-auth.md"),
        "spec body\n",
    )
    .unwrap();
    std::fs::create_dir_all(change_path.join("designs")).unwrap();
    std::fs::write(change_path.join("designs/main.md"), "design body\n").unwrap();
    // The export reads `work.todo.md` and renders it as
    // `tasks.md` ONLY when the file carries the canonical
    // `# [ ] work` or `# [x] work` title line. Match that shape
    // so the work-note-to-tasks round-trip reconstructs
    // `tasks.md` in the export.
    std::fs::write(change_path.join("work.todo.md"), "# [ ] work\n- [ ] item\n").unwrap();
    std::fs::create_dir_all(change_path.join("decisions")).unwrap();
    std::fs::write(
        change_path
            .join("decisions")
            .join("clone-topology-decisions.md"),
        "decision body.\n",
    )
    .unwrap();
    std::fs::write(
        change_path.join("decisions").join("20260712-foo.md"),
        "decision body.\n",
    )
    .unwrap();
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[allow(dead_code)]
fn walk_dir(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    names.sort();
    for path in names {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            files.extend(walk_dir(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn import_defers_active_tree_and_writes_archive() {
    let fixture = Fixture::new();
    let fixture_link = link_fixture(&fixture);

    let imported = nbspec(&fixture, &["import", &fixture_link.display().to_string()]);
    assert!(
        imported.status.success(),
        "stderr: {}",
        stderr_of(&imported)
    );

    // Active tree is now written via Transaction (one checkpoint per change).
    let notebook_path = fixture.notebook_path();
    let change_path = notebook_path.join("proposals").join(ACTIVE_CHANGE);
    assert!(
        change_path.exists(),
        "active tree must be written to the notebook via Transaction"
    );
    let stdout = stdout_of(&imported);
    assert!(
        stdout.contains("active") && stdout.contains(ACTIVE_CHANGE),
        "stdout must surface the active entry: {stdout}"
    );
    assert!(
        stdout.contains("wrote active") || stdout.contains("active-write"),
        "stdout must mention the active write: {stdout}"
    );

    // The archive is converted to deterministic tar.zst (v0.3.0
    // ships archive ingestion).
    let archive = fixture
        .project_root()
        .join("documentation/archives")
        .join(format!("{ARCHIVE_CHANGE}.tar.zst"));
    assert!(
        archive.is_file(),
        "archive not written: {}",
        archive.display()
    );
    assert!(
        stdout.contains("wrote archive"),
        "stdout must mention the archive write: {stdout}"
    );
}

#[test]
fn import_preserves_checked_tasks() {
    let fixture = Fixture::new();
    let source = fixture.project_root().join("scratch-checked");
    std::fs::create_dir_all(source.join("add-foo")).unwrap();
    std::fs::write(source.join("add-foo/proposal.md"), "# add-foo\n\nbody\n").unwrap();
    std::fs::write(
        source.join("add-foo/tasks.md"),
        "# [ ] Tasks\n- [x] done task\n- [ ] todo task\n",
    )
    .unwrap();

    let imported = nbspec(&fixture, &["import", &source.display().to_string()]);
    assert!(
        imported.status.success(),
        "import with checked tasks must succeed; stderr: {} stdout: {}",
        stderr_of(&imported),
        stdout_of(&imported)
    );

    let work_path = fixture
        .notebook_path()
        .join("proposals")
        .join("add-foo")
        .join("work.todo.md");
    assert!(work_path.is_file(), "work.todo.md must exist after import");
    let content = std::fs::read_to_string(&work_path).unwrap();
    assert!(
        content.contains("- [x] done task"),
        "checked task must be preserved via mark_task_done: {content}"
    );
    assert!(
        content.contains("- [ ] todo task"),
        "unchecked task must remain unchecked: {content}"
    );
}

#[test]
fn dry_run_writes_nothing() {
    let fixture = Fixture::new();
    let fixture_link = link_fixture(&fixture);

    let plan = nbspec(
        &fixture,
        &["import", &fixture_link.display().to_string(), "--dry-run"],
    );
    assert!(plan.status.success(), "stderr: {}", stderr_of(&plan));

    // No notebook writes happened.
    let notebook_path = fixture.notebook_path();
    assert!(
        !notebook_path.join("proposals").join(ACTIVE_CHANGE).exists(),
        "dry-run must not create the notebook namespace"
    );
    let archive = fixture
        .project_root()
        .join("documentation/archives")
        .join(format!("{ARCHIVE_CHANGE}.tar.zst"));
    assert!(!archive.exists(), "dry-run must not write the archive");

    let stdout = stdout_of(&plan);
    assert!(stdout.contains("dry-run: no writes performed"));
}

#[test]
fn refusal_aborts_partial_writes() {
    let fixture = Fixture::new();

    // Malformed `tasks.md` is a write failure via Transaction (tasks
    // parse error → WorkNoteError → InterchangeError::TasksParse →
    // write failure). No notebook mutation occurs and the import
    // surfaces the failure.
    let source = fixture.project_root().join("scratch-source");
    std::fs::create_dir_all(source.join("add-foo")).unwrap();
    std::fs::write(source.join("add-foo/proposal.md"), "# add-foo\n").unwrap();
    std::fs::write(
        source.join("add-foo/tasks.md"),
        "# [ ] Tasks\n- [?] half-written\n",
    )
    .unwrap();

    let imported = nbspec(&fixture, &["import", &source.display().to_string()]);
    assert!(
        !imported.status.success(),
        "import must fail on malformed tasks.md (write failure); stderr: {} stdout: {}",
        stderr_of(&imported),
        stdout_of(&imported)
    );
    let stderr = stderr_of(&imported);
    let stdout = stdout_of(&imported);
    assert!(
        stderr.contains("failure")
            || stdout.contains("failure")
            || stderr.contains("TasksParse")
            || stdout.contains("TasksParse"),
        "failure marker missing: stderr={stderr} stdout={stdout}"
    );

    let notebook_path = fixture.notebook_path();
    let change_path = notebook_path.join("proposals").join(ACTIVE_CHANGE);
    assert!(
        !change_path.exists(),
        "malformed tasks must not mutate the notebook: change_path = {change_path:?}"
    );
}

#[test]
fn export_round_trips_back_to_a_filesystem_tree() {
    let fixture = Fixture::new();

    // v0.3.0: `nbspec import` defers active filesystem-tree ingest
    // (pending the NbApi 0.3 transaction primitive), so we seed
    // the notebook change directly via the filesystem and let
    // `export` round-trip from a real notebook namespace.
    seed_notebook_change(&fixture, ACTIVE_CHANGE);

    let export_target = fixture.project_root().join("exported");
    std::fs::create_dir_all(&export_target).unwrap();
    let exported = nbspec(
        &fixture,
        &[
            "export",
            ACTIVE_CHANGE,
            &export_target.display().to_string(),
        ],
    );
    assert!(
        exported.status.success(),
        "stderr: {}",
        stderr_of(&exported)
    );

    let change_root = export_target.join(ACTIVE_CHANGE);
    assert!(change_root.join("proposal.md").is_file());
    assert!(change_root.join("specs/user-auth/spec.md").is_file());
    assert!(change_root.join("design.md").is_file());
    assert!(change_root.join("tasks.md").is_file());
    assert!(
        change_root
            .join("decisions/clone-topology-decisions.md")
            .is_file()
    );
    assert!(change_root.join("decisions/20260712-foo.md").is_file());

    // Verdicts remain notebook-resident and must not appear in
    // the filesystem tree.
    assert!(
        !change_root.join("verdicts").exists(),
        "verdicts/ must not enter the export tree"
    );

    // The export plan output names the omission explicitly.
    let stdout = stdout_of(&exported);
    assert!(
        stdout.contains("verdicts") && stdout.contains("NOT exported"),
        "export plan must surface the verdict omission: {stdout}"
    );
}

#[test]
fn export_refuses_overwrite_without_flag() {
    let fixture = Fixture::new();
    seed_notebook_change(&fixture, ACTIVE_CHANGE);

    let export_target = fixture.project_root().join("exported");
    std::fs::create_dir_all(&export_target).unwrap();
    // First export: success.
    let first = nbspec(
        &fixture,
        &[
            "export",
            ACTIVE_CHANGE,
            &export_target.display().to_string(),
        ],
    );
    assert!(first.status.success(), "first export must succeed");
    // Second export without --overwrite: refusal.
    let second = nbspec(
        &fixture,
        &[
            "export",
            ACTIVE_CHANGE,
            &export_target.display().to_string(),
        ],
    );
    assert!(!second.status.success(), "second export must refuse");
    let stderr = stderr_of(&second);
    assert!(
        stderr.contains("refusal") || stderr.contains("Refused"),
        "refusal marker missing: stderr={stderr} stdout={}",
        stdout_of(&second)
    );

    // Third export with --overwrite: success.
    let third = nbspec(
        &fixture,
        &[
            "export",
            ACTIVE_CHANGE,
            &export_target.display().to_string(),
            "--overwrite",
        ],
    );
    assert!(third.status.success(), "overwrite must succeed");
}

/// R1 / F1: change-id traversal refusal. A `change_id` that
/// contains path components or escape sequences must be refused
/// before any filesystem path is constructed, so `export
/// ../victim <target>` cannot escape `<target>` and remove an
/// unrelated directory.
#[test]
fn export_refuses_change_id_traversal() {
    let fixture = Fixture::new();
    // Lay a "victim" directory beside the export target so we
    // can detect accidental traversal if the refusal fails.
    let victim = fixture.project_root().join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    let sentinel = victim.join("sentinel.txt");
    std::fs::write(&sentinel, "untouched\n").unwrap();
    let target = fixture.project_root().join("target");

    let bad = nbspec(
        &fixture,
        &["export", "../victim", &target.display().to_string()],
    );
    assert!(
        !bad.status.success(),
        "traversal change-id must be refused: stderr={}",
        stderr_of(&bad)
    );
    assert!(
        sentinel.exists() && std::fs::read_to_string(&sentinel).unwrap() == "untouched\n",
        "refusal must not have deleted the sentinel"
    );
    let stderr = stderr_of(&bad);
    assert!(
        stderr.contains("invalid change id")
            || stderr.contains("InvalidChangeId")
            || stderr.contains("kebab-case"),
        "stderr should name the validation failure: {stderr}"
    );
}

/// R1' / F1 regression: the change-id traversal check must also
/// hold when `--overwrite` is supplied. The re-review noted that
/// the F1 production ordering is correct but the integration
/// test omitted `--overwrite`, leaving the formerly destructive
/// branch uncovered. A traversal change-id with `--overwrite`
/// must still refuse before any path is constructed.
#[test]
fn export_refuses_change_id_traversal_with_overwrite() {
    let fixture = Fixture::new();
    let victim = fixture.project_root().join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    let sentinel = victim.join("sentinel.txt");
    std::fs::write(&sentinel, "untouched\n").unwrap();
    let target = fixture.project_root().join("target");

    let bad = nbspec(
        &fixture,
        &[
            "export",
            "../victim",
            &target.display().to_string(),
            "--overwrite",
        ],
    );
    assert!(
        !bad.status.success(),
        "traversal change-id with --overwrite must still be refused: stderr={}",
        stderr_of(&bad)
    );
    assert!(
        sentinel.exists() && std::fs::read_to_string(&sentinel).unwrap() == "untouched\n",
        "refusal must not have deleted the sentinel even with --overwrite"
    );
    let stderr = stderr_of(&bad);
    assert!(
        stderr.contains("invalid change id")
            || stderr.contains("InvalidChangeId")
            || stderr.contains("kebab-case"),
        "stderr should name the validation failure: {stderr}"
    );
}

/// R7' / F7 re-review 2026-07-25: export with `--overwrite`
/// stages the new tree to a sibling, renames the old tree to a
/// backup sibling, and renames the staging into place. The
/// backup is preserved after success so the operator can recover
/// if the new tree is broken. This test verifies that the
/// publish stage preserves both the new tree and the backup.
#[test]
fn export_overwrite_preserves_backup_of_old_tree() {
    let fixture = Fixture::new();
    seed_notebook_change(&fixture, ACTIVE_CHANGE);

    let export_target = fixture.project_root().join("exported-backup");
    std::fs::create_dir_all(&export_target).unwrap();

    // First export: clean target.
    let first = nbspec(
        &fixture,
        &[
            "export",
            ACTIVE_CHANGE,
            &export_target.display().to_string(),
        ],
    );
    assert!(first.status.success(), "first export must succeed");
    let change_root = export_target.join(ACTIVE_CHANGE);
    assert!(
        change_root.is_dir(),
        "first export must produce change root"
    );

    // Mutate the exported tree so the overwrite is observable.
    let marker = change_root.join("OLD_TREE_MARKER.txt");
    std::fs::write(&marker, "old\n").unwrap();

    // Second export with --overwrite: staging publishes new tree,
    // old tree becomes a backup sibling.
    let second = nbspec(
        &fixture,
        &[
            "export",
            ACTIVE_CHANGE,
            &export_target.display().to_string(),
            "--overwrite",
        ],
    );
    assert!(second.status.success(), "overwrite must succeed");
    let stdout = stdout_of(&second);
    assert!(
        stdout.contains("backup"),
        "stdout should mention the backup path: {stdout}"
    );
    // The new tree is at change_root; the marker file is gone
    // because the new tree was staged and published via rename.
    assert!(
        !marker.exists(),
        "old marker must be gone from the published tree: {marker:?}"
    );
    // A backup sibling is preserved.
    let backup_entries: Vec<_> = std::fs::read_dir(&export_target)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains(".backup-"))
        .collect();
    assert!(
        !backup_entries.is_empty(),
        "expected a backup sibling after overwrite: {}",
        std::fs::read_dir(&export_target)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let backup_root = backup_entries[0].path();
    assert!(
        backup_root.join("OLD_TREE_MARKER.txt").exists(),
        "backup must preserve the old tree's marker at {}",
        backup_root.display()
    );
}

/// R3 / F3: all-or-nothing `--delete-original`. When a batch of
/// changes has one dirty round-trip proof, no source tree is
/// deleted. The clean change's source must remain on disk even
/// though its own proof was clean — partial deletion violates
/// the all-or-nothing gate.
///
/// Active filesystem-tree import via Transaction; exercises the post-import
/// source deletion path (0.3.0-resume).
#[test]
fn delete_original_all_or_nothing_preserves_clean_change() {
    let fixture = Fixture::new();

    // Two source changes: `add-foo` (clean) and `add-bar` (dirty:
    // the proposal diverges from what the export will produce
    // after the canonicalization that matters).
    let source_root = fixture.project_root().join("scratch-batch");
    std::fs::create_dir_all(source_root.join("add-foo")).unwrap();
    std::fs::write(
        source_root.join("add-foo/proposal.md"),
        "# add-foo\n\nbody\n",
    )
    .unwrap();

    std::fs::create_dir_all(source_root.join("add-bar")).unwrap();
    // Tasks with extra prose the import will not preserve: the
    // proof walks the source verbatim and finds a divergence.
    std::fs::write(
        source_root.join("add-bar/proposal.md"),
        "# add-bar\n\nbody\n",
    )
    .unwrap();
    std::fs::write(
        source_root.join("add-bar/tasks.md"),
        "# [ ] Tasks\n- [ ] only\n\nExtra paragraph the round trip cannot reconstruct verbatim.\n",
    )
    .unwrap();

    let imported = nbspec(
        &fixture,
        &[
            "import",
            &source_root.display().to_string(),
            "--delete-original",
        ],
    );
    let stdout = stdout_of(&imported);
    assert!(
        stdout.contains("round-trip refusal"),
        "expected round-trip refusal marker in stdout: {stdout}"
    );

    // Neither source tree may be deleted: add-foo's proof is
    // clean, add-bar's is dirty, and the all-or-nothing gate
    // keeps both.
    assert!(
        source_root.join("add-foo").exists(),
        "all-or-nothing gate must preserve add-foo source"
    );
    assert!(
        source_root.join("add-bar").exists(),
        "all-or-nothing gate must preserve add-bar source"
    );
}

#[test]
fn delete_original_refuses_when_proof_is_dirty() {
    let fixture = Fixture::new();

    // Build a source tree whose export will diverge beyond the
    // typed normalizations: the tasks body adds an item that
    // the work-note grammar silently drops, so the round-trip
    // diverges.
    let source = fixture.project_root().join("scratch-dirty");
    std::fs::create_dir_all(source.join("add-foo")).unwrap();
    std::fs::write(source.join("add-foo/proposal.md"), "# add-foo\n\nbody\n").unwrap();
    std::fs::write(
        source.join("add-foo/tasks.md"),
        "# [ ] Tasks\n- [ ] only\n\nSecond paragraph the round trip cannot reconstruct verbatim.\n",
    )
    .unwrap();

    let imported = nbspec(
        &fixture,
        &["import", &source.display().to_string(), "--delete-original"],
    );
    // Even on dirty round-trip, the import itself succeeds
    // (the import is additive); only the source deletion is
    // refused. The source tree must remain on disk.
    assert!(
        imported.status.success(),
        "stderr: {}",
        stderr_of(&imported)
    );
    let stdout = stdout_of(&imported);
    assert!(
        stdout.contains("round-trip refusal") || stdout.contains("diverg"),
        "expected round-trip refusal marker: {stdout}"
    );
    assert!(
        source.join("add-foo").exists(),
        "dirty round-trip must not delete the source tree"
    );
}

#[test]
fn delete_original_authorizes_clean_deletion() {
    let fixture = Fixture::new();

    // Build a clean source tree: a single spec, no tasks, no
    // decisions. The export round-trip is exact so the proof
    // is clean and --delete-original proceeds.
    let source = fixture.project_root().join("scratch-clean");
    std::fs::create_dir_all(source.join("add-foo")).unwrap();
    std::fs::write(source.join("add-foo/proposal.md"), "# add-foo\n\nbody\n").unwrap();

    let imported = nbspec(
        &fixture,
        &["import", &source.display().to_string(), "--delete-original"],
    );
    let stdout = stdout_of(&imported);
    assert!(
        imported.status.success(),
        "import must succeed; stderr: {}",
        stderr_of(&imported)
    );
    assert!(
        !source.join("add-foo").exists(),
        "clean round-trip must rename the source out of the original path; stdout: {stdout}"
    );
    // R7''' / F7 second re-review 2026-07-25: the source is
    // moved to a quarantine sibling rather than auto-deleted;
    // the operator decides when to remove it. Verify the
    // quarantine is in place and the original files survive.
    let quarantine_entries: Vec<_> = std::fs::read_dir(&source)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains(".quarantine-"))
        .collect();
    assert_eq!(
        quarantine_entries.len(),
        1,
        "expected one quarantine sibling; stdout: {stdout}"
    );
    let quarantine = quarantine_entries[0].path();
    assert!(
        quarantine.join("proposal.md").exists(),
        "quarantine must preserve the source contents"
    );
}

/// R7''' / F7 second re-review 2026-07-25: dirty proof must
/// restore every quarantine to its original path, not delete it.
/// The proof runs against the quarantine; if the proof is dirty,
/// the source is moved back to its original location. This makes
/// quarantine the recovery contract rather than auto-deletion.
#[test]
fn delete_original_restores_quarantine_on_dirty_proof() {
    let fixture = Fixture::new();

    // Source whose proposal body diverges from the export in a
    // way the canonicalizer cannot absorb (canonical form is
    // exact, so any drift surfaces). The export will not match
    // the source verbatim.
    let source = fixture.project_root().join("scratch-dirty-quarantine");
    std::fs::create_dir_all(source.join("add-foo")).unwrap();
    std::fs::write(source.join("add-foo/proposal.md"), "# add-foo\n\nbody\n").unwrap();
    std::fs::write(
        source.join("add-foo/tasks.md"),
        "# [ ] Tasks\n- [ ] only\n\nThis second paragraph drifts from the export.\n",
    )
    .unwrap();

    let imported = nbspec(
        &fixture,
        &["import", &source.display().to_string(), "--delete-original"],
    );
    assert!(
        imported.status.success(),
        "import itself must succeed even when proof is dirty; stderr: {}",
        stderr_of(&imported)
    );
    let stdout = stdout_of(&imported);
    assert!(
        stdout.contains("restored") && stdout.contains("dirty"),
        "stdout must mention the dirty-proof restore: {stdout}"
    );
    // Source is restored at the original path.
    assert!(
        source.join("add-foo/proposal.md").exists(),
        "dirty proof must restore the source at its original path"
    );
    // No quarantine siblings should remain (they were all
    // restored).
    let quarantine_entries: Vec<_> = std::fs::read_dir(&source)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains(".quarantine-"))
        .collect();
    assert!(
        quarantine_entries.is_empty(),
        "no quarantines should remain after a dirty proof; stdout: {stdout}"
    );
}

/// R2''''' / F7 third re-review 2026-07-25: archive
/// `--delete-original` end-to-end test. A minimal archive tree
/// (one proposal) is imported; the archive fidelity proof runs
/// against the quarantine path directly (not the reconstructed
/// `<parent>/<change_id>/`); on clean proof, the source is
/// quarantined and the archive is written to
/// `documentation/archives/<change-id>.tar.zst`. This is the
/// integration counterpart to the unit tests of
/// `archive_fidelity_proof` and catches the bug where the proof
/// was reconstructing the wrong path.
#[test]
fn delete_original_archive_renames_source_to_quarantine_with_passing_proof() {
    let fixture = Fixture::new();

    // Build a minimal legacy archive tree under
    // openspec/changes/archive/<change-id>/. The proposal body is
    // what the archive build will encode; the fidelity proof
    // compares the extracted archive against this exact content.
    let change_id = "archive-roundtrip";
    let source_root = fixture.project_root().join("scratch-archive-tree");
    let archive_tree = source_root.join("openspec/changes/archive").join(change_id);
    std::fs::create_dir_all(&archive_tree).unwrap();
    std::fs::write(
        archive_tree.join("proposal.md"),
        "# archive-roundtrip\n\nbody\n",
    )
    .unwrap();

    let imported = nbspec(
        &fixture,
        &[
            "import",
            &source_root.display().to_string(),
            "--delete-original",
        ],
    );
    let stdout = stdout_of(&imported);
    let stderr = stderr_of(&imported);
    assert!(
        imported.status.success(),
        "archive --delete-original must succeed; stderr: {stderr}; stdout: {stdout}"
    );
    // The archive must exist on disk.
    let archive = fixture
        .project_root()
        .join("documentation/archives")
        .join(format!("{change_id}.tar.zst"));
    assert!(
        archive.is_file(),
        "archive must be written: {}",
        archive.display()
    );
    // The original archive source tree is renamed out of the path.
    assert!(
        !archive_tree.exists(),
        "original archive tree must be renamed out of the path; stdout: {stdout}"
    );
    // A quarantine sibling exists under
    // openspec/changes/archive/<id>.quarantine-<nanos>-<pid>/.
    let archive_parent = source_root.join("openspec/changes/archive");
    let quarantine_entries: Vec<_> = std::fs::read_dir(&archive_parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{change_id}.quarantine-"))
        })
        .collect();
    assert_eq!(
        quarantine_entries.len(),
        1,
        "expected one archive quarantine sibling; stdout: {stdout}"
    );
    let quarantine = quarantine_entries[0].path();
    assert!(
        quarantine.join("proposal.md").exists(),
        "archive quarantine must preserve the source contents"
    );
}

/// P1: preflight must prevent partial writes when a batch contains a
/// valid active tree, a malformed active tree, and an archive tree.
/// No actives should be committed and no archive should be written.
#[test]
fn preflight_prevents_partial_writes_with_mixed_valid_malformed_and_archive() {
    let fixture = Fixture::new();

    // Pre-create an archive fixture for later verification that it was NOT written.
    let source_root = fixture.project_root().join("scratch-mixed-preflight");
    // Valid active: add-foo with correct proposal
    std::fs::create_dir_all(source_root.join("add-foo")).unwrap();
    std::fs::write(
        source_root.join("add-foo/proposal.md"),
        "# add-foo\n\nbody\n",
    )
    .unwrap();
    // Malformed active: add-bar with bad tasks.md (will fail TasksParse preflight)
    std::fs::create_dir_all(source_root.join("add-bar")).unwrap();
    std::fs::write(
        source_root.join("add-bar/proposal.md"),
        "# add-bar\n\nbody\n",
    )
    .unwrap();
    std::fs::write(
        source_root.join("add-bar/tasks.md"),
        "# [ ] Tasks\n- [?] malformed\n",
    )
    .unwrap();
    // Archive: legacy
    let archive_tree = source_root.join("openspec/changes/archive").join("legacy");
    std::fs::create_dir_all(&archive_tree).unwrap();
    std::fs::write(archive_tree.join("proposal.md"), "# legacy\n\nbody\n").unwrap();

    let imported = nbspec(&fixture, &["import", &source_root.display().to_string()]);
    assert!(
        !imported.status.success(),
        "mixed valid+malformed+archive must fail preflight; stderr: {} stdout: {}",
        stderr_of(&imported),
        stdout_of(&imported)
    );
    // No actives should be committed.
    let notebook_path = fixture.notebook_path();
    assert!(
        !notebook_path.join("proposals/add-foo").exists(),
        "valid active must NOT be committed when batch contains malformed"
    );
    assert!(
        !notebook_path.join("proposals/add-bar").exists(),
        "malformed active must not be committed"
    );
    // Archive must NOT be written (no partial writes).
    let archive = fixture
        .project_root()
        .join("documentation/archives/legacy.tar.zst");
    assert!(
        !archive.exists(),
        "archive must NOT be written when preflight fails"
    );
    // Output should contain the preflight failure and still show the plan.
    let combined = format!("{}{}", stdout_of(&imported), stderr_of(&imported));
    assert!(
        combined.contains("preflight failure") || combined.contains("failure"),
        "output must mention preflight failure: {combined}"
    );
}

/// P1: colliding active (change_id already exists) plus archive must not
/// leave partial writes.
#[test]
fn preflight_prevents_partial_writes_with_colliding_active_and_archive() {
    let fixture = Fixture::new();

    // Seed the notebook with an existing change `existing`.
    let existing_path = fixture.notebook_path().join("proposals/existing");
    std::fs::create_dir_all(&existing_path).unwrap();
    std::fs::write(existing_path.join("proposal.md"), "# existing\n").unwrap();

    let source_root = fixture.project_root().join("scratch-colliding-preflight");
    // Colliding active: same change_id as existing
    std::fs::create_dir_all(source_root.join("existing")).unwrap();
    std::fs::write(
        source_root.join("existing/proposal.md"),
        "# existing\n\nnew body\n",
    )
    .unwrap();
    // Archive: legacy2
    let archive_tree = source_root.join("openspec/changes/archive").join("legacy2");
    std::fs::create_dir_all(&archive_tree).unwrap();
    std::fs::write(archive_tree.join("proposal.md"), "# legacy2\n\nbody\n").unwrap();

    let imported = nbspec(&fixture, &["import", &source_root.display().to_string()]);
    assert!(
        !imported.status.success(),
        "colliding active + archive must fail preflight; stderr: {} stdout: {}",
        stderr_of(&imported),
        stdout_of(&imported)
    );
    // Existing should remain untouched (no new commit for colliding)
    let existing_content = std::fs::read_to_string(existing_path.join("proposal.md")).unwrap();
    assert_eq!(existing_content, "# existing\n");
    // Archive must NOT be written.
    let archive = fixture
        .project_root()
        .join("documentation/archives/legacy2.tar.zst");
    assert!(
        !archive.exists(),
        "archive must NOT be written when colliding active fails preflight"
    );
}

/// P1: later construction-time failure (spec duplicate title heading)
/// must be caught by preflight before any commit. A valid first active
/// plus a second active with a spec that triggers DuplicateTitleHeading
/// plus an archive must leave no partial writes.
#[test]
fn preflight_prevents_partial_writes_with_later_spec_duplicate_title() {
    let fixture = Fixture::new();

    let source_root = fixture.project_root().join("scratch-later-spec-preflight");
    // Valid active: add-foo
    std::fs::create_dir_all(source_root.join("add-foo")).unwrap();
    std::fs::write(
        source_root.join("add-foo/proposal.md"),
        "# add-foo\n\nbody\n",
    )
    .unwrap();
    // Second active: add-bar with a spec that will trigger DuplicateTitleHeading
    // The spec file's title is "cap" and its content starts with "# cap" — the
    // Transaction's add_note will detect the duplicate and fail at preflight.
    std::fs::create_dir_all(source_root.join("add-bar/specs/cap")).unwrap();
    std::fs::write(
        source_root.join("add-bar/proposal.md"),
        "# add-bar\n\nbody\n",
    )
    .unwrap();
    std::fs::write(
        source_root.join("add-bar/specs/cap/spec.md"),
        "# cap\n\n# cap\n\nbody\n",
    )
    .unwrap();
    // Archive: legacy3
    let archive_tree = source_root.join("openspec/changes/archive").join("legacy3");
    std::fs::create_dir_all(&archive_tree).unwrap();
    std::fs::write(archive_tree.join("proposal.md"), "# legacy3\n\nbody\n").unwrap();

    let imported = nbspec(&fixture, &["import", &source_root.display().to_string()]);
    assert!(
        !imported.status.success(),
        "later spec duplicate must fail preflight; stderr: {} stdout: {}",
        stderr_of(&imported),
        stdout_of(&imported)
    );
    // No actives should be committed.
    let notebook_path = fixture.notebook_path();
    assert!(
        !notebook_path.join("proposals/add-foo").exists(),
        "valid first active must NOT be committed when later spec fails preflight"
    );
    assert!(
        !notebook_path.join("proposals/add-bar").exists(),
        "failing second active must not be committed"
    );
    // Archive must NOT be written.
    let archive = fixture
        .project_root()
        .join("documentation/archives/legacy3.tar.zst");
    assert!(
        !archive.exists(),
        "archive must NOT be written when later spec fails preflight"
    );
    let combined = format!("{}{}", stdout_of(&imported), stderr_of(&imported));
    assert!(
        combined.contains("preflight failure")
            || combined.contains("DuplicateTitleHeading")
            || combined.contains("failure"),
        "output must mention preflight failure: {combined}"
    );
}
