#![allow(dead_code, unused_imports)]
//! Unit tests for filesystem-tree detection.

use std::path::{Path, PathBuf};

#[cfg(any())]
use nbspec::interchange::ActiveTree;
#[cfg(any())]
use nbspec::interchange::import_active_tree;
use nbspec::interchange::{
    ArchiveTree, DetectedTree, detect_trees, import_archive_tree, round_trip_proof,
};
use nbspec::interchange::{InterchangePlan, InterchangePlanStructured, PlanEntry};

/// Writes `content` at `<base>/<relative>`, creating intermediate
/// directories. Panics on IO failure so the test's setup failure is
/// obvious (rather than producing a misleading detection outcome).
fn write_file(base: &Path, relative: &str, content: &str) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
}

/// Writes raw `bytes` at `<base>/<relative>`, creating intermediate
/// directories. Used for the round-trip proof's binary-file
/// regression tests where the content must survive verbatim.
fn write_bytes(base: &Path, relative: &str, bytes: &[u8]) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
}

/// Builds a fresh scratch directory under the crate's test
/// temporary root. Returns the path; the caller is responsible for
/// cleanup (or wrap with `tempfile::TempDir` when needed).
fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nbspec-detect-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn detects_active_tree_with_proposal_md() {
    let root = scratch("active-basic");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    let detected = detect_trees(&root);
    assert_eq!(detected.len(), 1);
    match &detected[0] {
        DetectedTree::Active(tree) => {
            assert_eq!(tree.change_id, "add-foo");
            assert!(tree.root.ends_with("add-foo"));
        }
        other => panic!("expected Active, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn skips_directory_without_proposal_md() {
    let root = scratch("no-proposal");
    std::fs::create_dir_all(root.join("not-a-change")).unwrap();
    let detected = detect_trees(&root);
    assert!(detected.is_empty(), "got {detected:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn skips_directory_with_invalid_change_id() {
    let root = scratch("invalid-id");
    write_file(&root, "Not_A_Change/proposal.md", "# proposal\n");
    let detected = detect_trees(&root);
    assert!(detected.is_empty(), "got {detected:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn detects_archive_tree_under_openspec_changes_archive() {
    let root = scratch("archive-basic");
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/proposal.md",
        "# legacy\n",
    );
    let detected = detect_trees(&root);
    assert_eq!(detected.len(), 1);
    match &detected[0] {
        DetectedTree::Archive(tree) => {
            assert_eq!(tree.change_id, "legacy-feature");
            assert!(tree.root.ends_with("legacy-feature"));
        }
        other => panic!("expected Archive, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn detects_active_and_archive_trees_in_one_pass() {
    let root = scratch("mixed");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    write_file(&root, "add-bar/proposal.md", "# add-bar\n");
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/proposal.md",
        "# legacy\n",
    );
    let detected = detect_trees(&root);
    assert_eq!(detected.len(), 3);
    let ids: Vec<&str> = detected
        .iter()
        .map(|tree| match tree {
            DetectedTree::Active(t) => t.change_id.as_str(),
            DetectedTree::Archive(t) => t.change_id.as_str(),
        })
        .collect();
    assert_eq!(ids, vec!["add-bar", "add-foo", "legacy-feature"]);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn skips_archive_subdir_with_invalid_change_id() {
    let root = scratch("archive-bad-id");
    write_file(
        &root,
        "openspec/changes/archive/BAD_ID/proposal.md",
        "# anything\n",
    );
    let detected = detect_trees(&root);
    assert!(detected.is_empty(), "got {detected:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_root_returns_no_trees() {
    let root = scratch("empty");
    let detected = detect_trees(&root);
    assert!(detected.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn missing_root_returns_no_trees() {
    let detected = detect_trees(Path::new("/no/such/path/should/exist"));
    assert!(detected.is_empty());
}
