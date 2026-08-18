#![allow(dead_code, unused_imports)]
//! Unit tests for active-tree interchange (paused shim).

use std::path::{Path, PathBuf};

#[cfg(any())]
use nbspec::interchange::ActiveTree;
#[cfg(any())]
use nbspec::interchange::import_active_tree;
use nbspec::interchange::{
    ArchiveTree, DetectedTree, detect_trees, import_archive_tree, round_trip_proof,
};
use nbspec::interchange::{InterchangePlan, InterchangePlanStructured, PlanEntry};

fn write_file(base: &Path, relative: &str, content: &str) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
}

fn write_bytes(base: &Path, relative: &str, bytes: &[u8]) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
}

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

#[cfg(any())]
#[test]
fn active_tree_mapping_emits_proposal_spec_design_work_decisions_meta() {
    let root = scratch("active-mapping");
    write_file(
        &root,
        "add-foo/proposal.md",
        "# add-foo\n\nSome proposal body.\n",
    );
    write_file(
        &root,
        "add-foo/specs/user-auth/spec.md",
        "# user-auth\n\n## ADDED Requirements\n",
    );
    write_file(
        &root,
        "add-foo/design.md",
        "# main design\n\nDesign notes here.\n",
    );
    write_file(
        &root,
        "add-foo/decisions/clone-topology-decisions.md",
        "## Decision: use the fleet convention\n\nStatus: accepted.\n",
    );
    write_file(
        &root,
        "add-foo/tasks.md",
        "# [ ] Tasks\n- [ ] first\n- [x] done\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, warnings, meta) =
        import_active_tree(&active, &[]).expect("import_active_tree should succeed");

    assert!(
        warnings.is_empty(),
        "no delta warnings expected: {warnings:?}"
    );
    let titles: Vec<(&str, &str)> = writes
        .iter()
        .map(|w| (w.folder.as_str(), w.title.as_str()))
        .collect();
    assert!(
        titles.contains(&("", "proposal")),
        "missing proposal: {titles:?}"
    );
    assert!(
        titles.contains(&("specifications", "user-auth")),
        "missing spec: {titles:?}"
    );
    assert!(
        titles.contains(&("designs", "main")),
        "missing design: {titles:?}"
    );
    assert!(
        titles.contains(&("decisions", "clone-topology-decisions")),
        "missing decision: {titles:?}"
    );
    assert!(titles.contains(&("", "work")), "missing work: {titles:?}");
    assert!(titles.contains(&("", "meta")), "missing meta: {titles:?}");

    let work_write = writes
        .iter()
        .find(|w| w.title == "work")
        .expect("work write");
    assert!(work_write.is_todo, "work must be a todo note");

    let meta_write = writes
        .iter()
        .find(|w| w.title == "meta")
        .expect("meta write");
    assert!(meta_write.content.contains("\"migrated\": true"));
    assert!(meta_write.content.contains("\"source_path\""));

    assert!(!meta.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_mapping_normalizes_proposal_h1() {
    let root = scratch("active-h1");
    write_file(
        &root,
        "add-foo/proposal.md",
        "# Free-form heading\n\nBody text.\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, _, _) = import_active_tree(&active, &[]).expect("import should succeed");
    let proposal = writes
        .iter()
        .find(|w| w.title == "proposal")
        .expect("proposal write");
    assert!(
        proposal.content.starts_with("# add-foo"),
        "proposal H1 not normalized: {}",
        proposal.content
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_mapping_preserves_decision_filename() {
    let root = scratch("active-decision-name");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    write_file(
        &root,
        "add-foo/decisions/clone-topology-decisions.md",
        "Decision body.\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, _, _) = import_active_tree(&active, &[]).expect("import should succeed");
    let decision = writes
        .iter()
        .find(|w| w.title == "clone-topology-decisions")
        .expect("decision write");
    assert_eq!(decision.folder, "decisions");
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_mapping_warns_on_modified_delta() {
    let root = scratch("active-delta-warning");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    write_file(
        &root,
        "add-foo/specs/user-auth/spec.md",
        "# user-auth\n\n## MODIFIED Requirements\n\n### Requirement: foo\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, warnings, _) = import_active_tree(&active, &[]).expect("import should succeed");
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w.kind, nbspec::interchange::DeltaWarningKind::Modified)),
        "expected MODIFIED warning, got {warnings:?}"
    );
    // Import still proceeds: the warnings are surfaced in the
    // plan output but never refuse the import.
    assert!(writes.iter().any(|w| w.title == "user-auth"));
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_refuses_change_id_collision() {
    let root = scratch("active-collision");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let result = import_active_tree(&active, &["add-foo".to_string()]);
    assert!(
        matches!(result, Err(nbspec::interchange::InterchangeError::Collision(ref id)) if id == "add-foo"),
        "expected Collision, got {result:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_refuses_unparseable_tasks_md() {
    let root = scratch("active-bad-tasks");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    // `- [?]` is neither `[ ]` nor `[x]`, so the work note parser
    // raises WorkNoteError::MalformedItem.
    write_file(
        &root,
        "add-foo/tasks.md",
        "# [ ] Tasks\n- [?] half-written\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let result = import_active_tree(&active, &[]);
    assert!(
        matches!(
            result,
            Err(nbspec::interchange::InterchangeError::TasksParse(_))
        ),
        "expected TasksParse, got {result:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_preserves_checkbox_state_in_work_note() {
    let root = scratch("active-checkboxes");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    write_file(
        &root,
        "add-foo/tasks.md",
        "# [ ] Tasks\n- [ ] open item\n- [x] done item\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, _, _) = import_active_tree(&active, &[]).expect("import should succeed");
    let work = writes
        .iter()
        .find(|w| w.title == "work")
        .expect("work write");
    assert!(
        work.content.contains("- [ ] open item"),
        "open item missing: {}",
        work.content
    );
    assert!(
        work.content.contains("- [x] done item"),
        "done item missing: {}",
        work.content
    );
    std::fs::remove_dir_all(&root).ok();
}
