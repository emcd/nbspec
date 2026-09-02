//! Preflight no-partial-writes tests — split from interchange.rs for linecheck.

use super::harness::Fixture;
use super::interchange::{nbspec, stderr_of, stdout_of};

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

/// P2: leading blank lines before H1 must be normalized so the H1 is
/// still stripped and does not trigger DuplicateTitleHeading. Prior
/// code checked trim_start() but removed only lines().skip(1), leaving
/// the H1 and causing a duplicate-title error.
#[test]
fn import_strips_leading_blank_lines_before_h1() {
    let fixture = Fixture::new();

    let source_root = fixture.project_root().join("scratch-leading-blank-h1");
    std::fs::create_dir_all(source_root.join("add-foo/specs/cap")).unwrap();
    std::fs::write(
        source_root.join("add-foo/proposal.md"),
        "# add-foo\n\nbody\n",
    )
    .unwrap();
    // Spec with leading blank lines before H1 — should be stripped, not treated as duplicate.
    std::fs::write(
        source_root.join("add-foo/specs/cap/spec.md"),
        "\n\n# cap\n\nbody content\n",
    )
    .unwrap();

    let imported = nbspec(&fixture, &["import", &source_root.display().to_string()]);
    assert!(
        imported.status.success(),
        "spec with leading blank H1 should succeed; stderr: {} stdout: {}",
        stderr_of(&imported),
        stdout_of(&imported)
    );
    let notebook_path = fixture.notebook_path();
    let spec_path = notebook_path.join("proposals/add-foo/specifications/cap.md");
    assert!(spec_path.is_file(), "spec should be imported");
    let content = std::fs::read_to_string(&spec_path).unwrap();
    // The H1 should be stripped from the body, leaving only one H1 (the title).
    let h1_count = content.matches("# cap").count();
    assert_eq!(
        h1_count, 1,
        "should have exactly one H1 (the title), not duplicate: {content}"
    );
    assert!(content.contains("body content"));
}

/// P2: indented `#` (e.g. `    #!/bin/sh`) must be preserved — only an
/// unindented H1 (`# ` at column 0) is stripped. This matches
/// `first_h1_title_ignores_indented_heading` and prevents silent data loss.
#[test]
fn import_preserves_indented_hash_content() {
    let fixture = Fixture::new();

    let source_root = fixture.project_root().join("scratch-indented-hash");
    std::fs::create_dir_all(source_root.join("add-foo/specs/cap")).unwrap();
    std::fs::write(
        source_root.join("add-foo/proposal.md"),
        "# add-foo\n\nbody\n",
    )
    .unwrap();
    // Spec body starts with an indented shebang — not an H1, must be preserved verbatim.
    std::fs::write(
        source_root.join("add-foo/specs/cap/spec.md"),
        "    #!/bin/sh\n\necho hi\n",
    )
    .unwrap();

    let imported = nbspec(&fixture, &["import", &source_root.display().to_string()]);
    assert!(
        imported.status.success(),
        "indented hash content should succeed; stderr: {} stdout: {}",
        stderr_of(&imported),
        stdout_of(&imported)
    );
    let spec_path = fixture
        .notebook_path()
        .join("proposals/add-foo/specifications/cap.md");
    assert!(spec_path.is_file(), "spec should be imported");
    let content = std::fs::read_to_string(&spec_path).unwrap();
    assert!(
        content.contains("    #!/bin/sh"),
        "indented #!/bin/sh must be preserved, not stripped as H1: {content}"
    );
    assert!(content.contains("echo hi"));
    // The file should still have its title H1 plus the preserved indented line
    assert!(
        content.contains("# cap"),
        "title H1 must still be present: {content}"
    );
}
