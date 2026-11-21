use std::fs;
use std::path::PathBuf;

use nbspec::merging::{MergeError, RefusalReason, merge_documents};
use nbspec::provenance;
use nbspec::rendering::RenderedDocument;

const TEMP_TEST_ROOT: &str = ".auxiliary/temporary/tests";

fn unique_temp_root(label: &str) -> PathBuf {
    let unique = format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    PathBuf::from(TEMP_TEST_ROOT).join(unique)
}

fn document(name: &str, content: &str) -> RenderedDocument {
    RenderedDocument {
        artifact_id: "specifications".to_string(),
        tree_path: format!("specifications/{name}.md"),
        target_path: Some(format!("documentation/specifications/{name}.md")),
        source_note: format!("proposals/add-demo/specifications/{name}.md"),
        content: content.to_string(),
    }
}

const ADDED_SPEC: &str = "\
# alpha

## ADDED Requirements

### Requirement: Alpha
The system SHALL alpha.

#### Scenario: Alphas
- **WHEN** alpha
- **THEN** alpha
";

#[test]
fn h1_slug_rename_leaves_stale_target_refusal() {
    let root = unique_temp_root("merging-stale-target");
    let target_dir = root.join("documentation/specifications");
    fs::create_dir_all(&target_dir).unwrap();

    // Simulate a previous merge: timestamp note materialized as
    // user-auth.md with provenance naming the timestamp source.
    let source_note = "proposals/add-demo/specifications/20260710175830.md";
    let body = "# user-auth\n\n## ADDED Requirements\n\n### Requirement: Auth\nOld text.\n";
    let stamped = provenance::stamp(body, "add-demo", "home", source_note);
    fs::write(target_dir.join("user-auth.md"), stamped).unwrap();

    // Now render the same source note with a different H1, producing
    // a different target path.
    let doc = RenderedDocument {
        artifact_id: "specifications".to_string(),
        tree_path: "specifications/authentication.md".to_string(),
        target_path: Some("documentation/specifications/authentication.md".to_string()),
        source_note: source_note.to_string(),
        content: "# authentication\n\n## ADDED Requirements\n\n### Requirement: Auth\nOld text.\n"
            .to_string(),
    };

    let result = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        false,
    );
    let Err(MergeError::Refused { refusals }) = result else {
        panic!("expected refusal, got {result:?}");
    };
    assert_eq!(refusals.len(), 1);
    assert_eq!(
        refusals[0].target,
        "documentation/specifications/user-auth.md"
    );
    assert!(matches!(
        refusals[0].reason,
        RefusalReason::StaleTarget { .. }
    ));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn h1_slug_rename_force_override_announces_stale_target() {
    let root = unique_temp_root("merging-stale-target-force");
    let target_dir = root.join("documentation/specifications");
    fs::create_dir_all(&target_dir).unwrap();

    let source_note = "proposals/add-demo/specifications/20260710175830.md";
    let body = "# user-auth\n\n## ADDED Requirements\n\n### Requirement: Auth\nOld text.\n";
    let stamped = provenance::stamp(body, "add-demo", "home", source_note);
    fs::write(target_dir.join("user-auth.md"), stamped).unwrap();

    let doc = RenderedDocument {
        artifact_id: "specifications".to_string(),
        tree_path: "specifications/authentication.md".to_string(),
        target_path: Some("documentation/specifications/authentication.md".to_string()),
        source_note: source_note.to_string(),
        content: "# authentication\n\n## ADDED Requirements\n\n### Requirement: Auth\nOld text.\n"
            .to_string(),
    };

    let report = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        true,
    )
    .unwrap();
    assert_eq!(
        report.stale_target_overrides,
        vec!["documentation/specifications/user-auth.md"]
    );
    // Old target still exists (force does not remove it)
    assert!(target_dir.join("user-auth.md").is_file());
    // New target was written
    assert!(target_dir.join("authentication.md").is_file());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[cfg(unix)]
fn stale_scan_refuses_symlinked_target_directory() {
    let root = unique_temp_root("merging-stale-symlink-dir");
    let real = root.join("real-specifications");
    fs::create_dir_all(&real).unwrap();
    fs::create_dir_all(root.join("documentation")).unwrap();
    std::os::unix::fs::symlink(
        &real,
        root.join("documentation/specifications"),
    )
    .unwrap();

    let doc = document("alpha", ADDED_SPEC);
    let result = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        false,
    );
    let Err(MergeError::Io { path, source }) = result else {
        panic!("expected Io error for symlinked target dir, got {result:?}");
    };
    assert!(
        path.ends_with("documentation/specifications"),
        "path was {}",
        path.display()
    );
    assert_eq!(source.kind(), std::io::ErrorKind::InvalidInput);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[cfg(unix)]
fn stale_scan_fails_closed_on_unreadable_target_directory() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_temp_root("merging-stale-unreadable-dir");
    let target_dir = root.join("documentation/specifications");
    fs::create_dir_all(&target_dir).unwrap();
    fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o000))
        .unwrap();

    let doc = document("alpha", ADDED_SPEC);
    let result = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        false,
    );

    fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o755))
        .unwrap();
    assert!(
        matches!(result, Err(MergeError::Io { .. })),
        "expected Io on unreadable target dir, got {result:?}"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[cfg(unix)]
fn stale_scan_fails_closed_on_unreadable_candidate_file() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_temp_root("merging-stale-unreadable-file");
    let target_dir = root.join("documentation/specifications");
    fs::create_dir_all(&target_dir).unwrap();
    let candidate = target_dir.join("stale.md");
    fs::write(&candidate, "not even readable").unwrap();
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o000))
        .unwrap();

    let doc = document("alpha", ADDED_SPEC);
    let result = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        false,
    );

    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o644))
        .unwrap();
    assert!(
        matches!(result, Err(MergeError::Io { .. })),
        "expected Io on unreadable candidate, got {result:?}"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[cfg(unix)]
fn stale_scan_skips_symlink_entries_inside_real_directory() {
    let root = unique_temp_root("merging-stale-symlink-entry");
    let target_dir = root.join("documentation/specifications");
    fs::create_dir_all(&target_dir).unwrap();
    let outside = root.join("outside.md");
    let source_note = "proposals/add-demo/specifications/20260710175830.md";
    let stamped = provenance::stamp(
        "# user-auth\n\n## ADDED Requirements\n",
        "add-demo",
        "home",
        source_note,
    );
    fs::write(&outside, &stamped).unwrap();
    std::os::unix::fs::symlink(&outside, target_dir.join("user-auth.md"))
        .unwrap();

    let doc = RenderedDocument {
        artifact_id: "specifications".to_string(),
        tree_path: "specifications/authentication.md".to_string(),
        target_path: Some("documentation/specifications/authentication.md".to_string()),
        source_note: source_note.to_string(),
        content: "# authentication\n\n## ADDED Requirements\n\n### Requirement: Auth\nOld text.\n"
            .to_string(),
    };
    // Symlink entry is skipped; no stale refusal; new target writes.
    let report = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .expect("symlink entries must be skipped, not followed");
    assert!(report.stale_target_overrides.is_empty());
    assert!(target_dir.join("authentication.md").is_file());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[cfg(unix)]
fn merge_refuses_symlinked_ancestor_without_external_effects() {
    let root = unique_temp_root("merging-symlink-ancestor");
    let outside = unique_temp_root("merging-symlink-ancestor-outside");
    fs::create_dir_all(&root).unwrap();
    let outside_specs = outside.join("specifications");
    fs::create_dir_all(&outside_specs).unwrap();
    let marker = outside_specs.join("marker-preexisting.txt");
    fs::write(&marker, "untouched\n").unwrap();

    // documentation -> outside, so documentation/specifications resolves externally.
    std::os::unix::fs::symlink(&outside, root.join("documentation")).unwrap();

    let doc = document("alpha", ADDED_SPEC);
    let result = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        false,
    );
    assert!(
        matches!(result, Err(MergeError::Io { .. })),
        "symlinked ancestor must refuse, got {result:?}"
    );
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "untouched\n",
        "external tree must not be modified"
    );
    assert!(
        !outside_specs.join("alpha.md").exists(),
        "must not create files outside the repository"
    );
    // Unlink before remove_dir_all so cleanup does not follow into
    // the external tree.
    fs::remove_file(root.join("documentation")).unwrap();
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&outside).unwrap();
}

#[test]
#[cfg(unix)]
fn merge_refuses_live_target_symlink_without_external_effects() {
    let root = unique_temp_root("merging-live-target-symlink");
    let outside = unique_temp_root("merging-live-target-symlink-outside");
    fs::create_dir_all(&outside).unwrap();
    let external = outside.join("alpha.md");
    fs::write(&external, "external original\n").unwrap();

    let target_dir = root.join("documentation/specifications");
    fs::create_dir_all(&target_dir).unwrap();
    std::os::unix::fs::symlink(&external, target_dir.join("alpha.md"))
        .unwrap();

    let doc = document("alpha", ADDED_SPEC);
    let result = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        false,
    );
    assert!(
        matches!(result, Err(MergeError::Io { .. })),
        "live target symlink must refuse, got {result:?}"
    );
    assert_eq!(
        fs::read_to_string(&external).unwrap(),
        "external original\n",
        "must not overwrite through the symlink"
    );
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&outside).unwrap();
}

#[test]
#[cfg(unix)]
fn merge_refuses_dangling_target_symlink_without_creating_external() {
    let root = unique_temp_root("merging-dangling-target-symlink");
    let outside = unique_temp_root("merging-dangling-target-symlink-outside");
    fs::create_dir_all(&outside).unwrap();
    let dangling_dest = outside.join("never-created.md");

    let target_dir = root.join("documentation/specifications");
    fs::create_dir_all(&target_dir).unwrap();
    std::os::unix::fs::symlink(&dangling_dest, target_dir.join("alpha.md"))
        .unwrap();

    let doc = document("alpha", ADDED_SPEC);
    let result = merge_documents(
        std::slice::from_ref(&doc),
        &root,
        "add-demo",
        "home",
        None,
        false,
    );
    assert!(
        matches!(result, Err(MergeError::Io { .. })),
        "dangling target symlink must refuse, got {result:?}"
    );
    assert!(
        !dangling_dest.exists(),
        "must not materialize the dangling symlink target"
    );
    // The symlink itself must remain a symlink, not be replaced.
    let meta = fs::symlink_metadata(target_dir.join("alpha.md")).unwrap();
    assert!(meta.file_type().is_symlink());
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&outside).unwrap();
}

#[test]
fn non_directory_target_parent_blocks_all_writes() {
    let root = unique_temp_root("merging-nondir-parent");
    fs::create_dir_all(root.join("documentation")).unwrap();
    // specifications is a file, not a directory.
    fs::write(root.join("documentation/specifications"), "not a dir\n")
        .unwrap();

    let alpha = document("alpha", ADDED_SPEC);
    let beta = document("beta", ADDED_SPEC);
    // beta would write under designs — give it a clean parent so only
    // the shared/other path isn't the issue. Both use specifications.
    let docs = vec![alpha, beta];
    let result =
        merge_documents(&docs, &root, "add-demo", "home", None, false);
    assert!(
        matches!(result, Err(MergeError::Io { .. })),
        "non-directory parent must fail during planning, got {result:?}"
    );
    assert!(
        !root.join("documentation/specifications/alpha.md").exists(),
        "no partial write of alpha"
    );
    assert!(
        !root.join("documentation/specifications/beta.md").exists(),
        "no partial write of beta"
    );
    // The file occupant must be unchanged.
    assert_eq!(
        fs::read_to_string(root.join("documentation/specifications")).unwrap(),
        "not a dir\n"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn non_directory_parent_blocks_even_when_earlier_target_is_clean() {
    let root = unique_temp_root("merging-nondir-parent-multi");
    // First document writes to designs/ (clean). Second needs
    // specifications/ which is a file.
    fs::create_dir_all(root.join("documentation")).unwrap();
    fs::write(root.join("documentation/specifications"), "not a dir\n")
        .unwrap();

    let design = RenderedDocument {
        artifact_id: "designs".to_string(),
        tree_path: "designs/notes.md".to_string(),
        target_path: Some("documentation/designs/notes.md".to_string()),
        source_note: "proposals/add-demo/designs/notes.md".to_string(),
        content: "# notes\n\nDesign body.\n".to_string(),
    };
    let spec = document("alpha", ADDED_SPEC);
    let result = merge_documents(
        &[design, spec],
        &root,
        "add-demo",
        "home",
        None,
        false,
    );
    assert!(
        matches!(result, Err(MergeError::Io { .. })),
        "planning must fail closed, got {result:?}"
    );
    assert!(
        !root.join("documentation/designs/notes.md").exists(),
        "earlier clean target must not be written when a later target fails planning"
    );
    assert!(!root.join("documentation/specifications/alpha.md").exists());
    fs::remove_dir_all(&root).unwrap();
}
