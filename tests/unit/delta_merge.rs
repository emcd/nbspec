//! Surgical merge through `merge_documents`: end-to-end planning,
//! stamping, drift/force interplay, and report warnings.

use std::fs;
use std::path::{Path, PathBuf};

use nbspec::merging::{MergeError, RefusalReason, TargetStatus, merge_documents, target_status};
use nbspec::provenance;
use nbspec::rendering::RenderedDocument;

fn unique_temp_root(label: &str) -> PathBuf {
    let unique = format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    PathBuf::from(".auxiliary/temporary/tests").join(unique)
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

fn target_of(root: &Path, name: &str) -> PathBuf {
    root.join(format!("documentation/specifications/{name}.md"))
}

/// Seeds a stamped durable target as a previous merge would have left it.
fn seed_target(root: &Path, name: &str, change: &str, body: &str) {
    let path = target_of(root, name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        provenance::stamp(body, change, "home", &format!("proposals/{change}/x.md")),
    )
    .unwrap();
}

const BASE_SPEC: &str = "\
# alpha

## ADDED Requirements

### Requirement: Alpha
The system SHALL alpha.

#### Scenario: Alphas
- **WHEN** alpha
- **THEN** alpha

### Requirement: Beta
The system SHALL beta.
";

#[test]
fn surgical_removed_end_to_end() {
    let root = unique_temp_root("delta-removed");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Gamma
The system SHALL gamma.

## REMOVED Requirements

### Requirement: Beta
";
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .unwrap();
    assert_eq!(
        report.written,
        vec!["documentation/specifications/alpha.md".to_string()]
    );
    let written = fs::read_to_string(target_of(&root, "alpha")).unwrap();
    let (header, body) = provenance::split_document(&written);
    assert_eq!(header.unwrap().change_id, "add-demo");
    assert!(
        !body.contains("### Requirement: Beta"),
        "removed block gone"
    );
    assert!(body.contains("### Requirement: Alpha"), "kept block stays");
    assert!(
        body.contains("### Requirement: Gamma"),
        "added block appended"
    );
    assert!(
        report.warnings.is_empty(),
        "clean application warns nothing"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn surgical_remerge_is_unchanged_not_rewritten() {
    let root = unique_temp_root("delta-remerge");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Gamma
The system SHALL gamma.

## REMOVED Requirements

### Requirement: Beta
";
    let documents = vec![document("alpha", delta)];
    let first = merge_documents(&documents, &root, "add-demo", "home", None, false).unwrap();
    assert_eq!(first.written.len(), 1);
    let stamped = fs::read_to_string(target_of(&root, "alpha")).unwrap();
    let second = merge_documents(&documents, &root, "add-demo", "home", None, false).unwrap();
    assert!(second.written.is_empty(), "no rewrite on remerge");
    assert_eq!(
        second.unchanged,
        vec!["documentation/specifications/alpha.md".to_string()]
    );
    assert_eq!(
        fs::read_to_string(target_of(&root, "alpha")).unwrap(),
        stamped,
        "remerge is byte-identical"
    );
    assert_eq!(
        target_status(&documents[0], &root, "add-demo").unwrap(),
        TargetStatus::Current,
        "rebuilt comparison reports Current, not perpetual UpdatePending"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn added_only_notes_apply_surgically() {
    // No whole-write exemption: an ADDED-only note appends new
    // blocks, no-ops identical ones, and refuses differing ones.
    // Full-content restatement is a loud error, never silent
    // duplication or overwrite.
    let root = unique_temp_root("delta-append");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Beta
The system SHALL beta, restated differently.

### Requirement: Gamma
The system SHALL gamma.
";
    let error = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert!(
        matches!(refusals[0].reason, RefusalReason::DanglingDelta(_)),
        "restated content collides loudly"
    );
    assert!(
        fs::read_to_string(target_of(&root, "alpha"))
            .unwrap()
            .contains("### Requirement: Beta"),
        "refused merge leaves the target untouched"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn added_only_note_appends_fresh_block() {
    let root = unique_temp_root("delta-append-ok");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Beta
The system SHALL beta.

### Requirement: Gamma
The system SHALL gamma.
";
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .unwrap();
    assert_eq!(report.written.len(), 1);
    let stamped = fs::read_to_string(target_of(&root, "alpha")).unwrap();
    let (_, body) = provenance::split_document(&stamped);
    assert!(
        body.contains("### Requirement: Gamma"),
        "fresh block appended"
    );
    assert_eq!(
        body.matches("### Requirement: Beta").count(),
        1,
        "no duplication"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn modified_against_absent_target_refuses() {
    let root = unique_temp_root("delta-absent");
    fs::create_dir_all(&root).unwrap();
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
Changed text.
";
    let error = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        true,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert!(
        matches!(refusals[0].reason, RefusalReason::DanglingDelta(_)),
        "MODIFIED on a new document dangles"
    );
    assert!(
        !target_of(&root, "alpha").exists(),
        "refused merge writes nothing"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn removed_only_against_absent_target_warns_and_skips() {
    let root = unique_temp_root("delta-skip");
    fs::create_dir_all(&root).unwrap();
    let delta = "\
# alpha

## REMOVED Requirements

### Requirement: Ghost
";
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .unwrap();
    assert!(report.written.is_empty(), "skipped documents write nothing");
    assert!(report.unchanged.is_empty());
    assert_eq!(report.warnings.len(), 1, "skip warns");
    assert!(
        !target_of(&root, "alpha").exists(),
        "no skeleton materializes"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn drifted_surgical_target_refuses_then_force_applies() {
    let root = unique_temp_root("delta-drift");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-demo", BASE_SPEC);
    let path = target_of(&root, "alpha");
    // Drift outside any requirement block (own section): block-grain
    // application must preserve it, while drift inside a removed
    // block would go with the block.
    let mut drifted = fs::read_to_string(&path).unwrap();
    drifted.push_str("\n## Operator notes\n\nHand edit.\n");
    fs::write(&path, drifted).unwrap();
    let delta = "\
# alpha

## REMOVED Requirements

### Requirement: Beta
";
    let error = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert!(
        matches!(refusals[0].reason, RefusalReason::Drifted),
        "drift refuses before application"
    );
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        true,
    )
    .unwrap();
    assert_eq!(report.written.len(), 1, "force applies onto drifted body");
    let body = fs::read_to_string(&path).unwrap();
    assert!(!body.contains("### Requirement: Beta"), "removal applied");
    assert!(body.contains("Hand edit."), "drifted prose survives force");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unmanaged_parseable_target_adopts_under_force() {
    let root = unique_temp_root("delta-adopt");
    fs::create_dir_all(&root).unwrap();
    let path = target_of(&root, "alpha");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, BASE_SPEC).unwrap();
    let delta = "\
# alpha

## REMOVED Requirements

### Requirement: Beta
";
    let refused = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = refused else {
        panic!("expected refusal");
    };
    assert!(matches!(refusals[0].reason, RefusalReason::Unmanaged));
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        true,
    )
    .unwrap();
    assert_eq!(
        report.written.len(),
        1,
        "parseable base adopted under force"
    );
    let (header, _) = provenance::split_document(&fs::read_to_string(&path).unwrap());
    assert_eq!(
        header.unwrap().change_id,
        "add-demo",
        "adopted target stamped"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unmanaged_unparseable_target_refuses_even_with_force() {
    let root = unique_temp_root("delta-unparseable");
    fs::create_dir_all(&root).unwrap();
    let path = target_of(&root, "alpha");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "Just some prose, no requirement sections.\n").unwrap();
    let delta = "\
# alpha

## REMOVED Requirements

### Requirement: Beta
";
    let error = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        true,
    )
    .unwrap_err();
    let message = error.to_string();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert!(
        matches!(refusals[0].reason, RefusalReason::DanglingDelta(_)),
        "unparseable base refuses under force: {message}"
    );
    assert!(
        message.contains("--force does not override"),
        "force-proof refusal says so: {message}"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "Just some prose, no requirement sections.\n",
        "refused merge writes nothing"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn incoherent_document_reports_unmergeable_status() {
    let root = unique_temp_root("delta-status");
    fs::create_dir_all(&root).unwrap();
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Alpha
Text.

## REMOVED Requirements

### Requirement: Alpha
";
    let status = target_status(&document("alpha", delta), &root, "add-demo").unwrap();
    assert!(
        matches!(status, TargetStatus::Unmergeable(_)),
        "planning failure surfaces as status, keeping display total"
    );
    assert!(status.to_string().contains("unmergeable"), "{status}");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn already_removed_warning_reaches_the_report() {
    let root = unique_temp_root("delta-warn");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Gamma
The system SHALL gamma.

## REMOVED Requirements

### Requirement: Ghost
";
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .unwrap();
    assert_eq!(report.written.len(), 1);
    assert_eq!(report.warnings.len(), 1, "already-removed warns");
    assert!(report.warnings[0].contains("Ghost"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn incoherence_preempts_drift() {
    // Coherence runs before state: a broken note on a drifted
    // target reports the note, not the drift.
    let root = unique_temp_root("delta-order");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-demo", BASE_SPEC);
    let path = target_of(&root, "alpha");
    let drifted = fs::read_to_string(&path)
        .unwrap()
        .replace("SHALL alpha.", "SHALL omega.");
    fs::write(&path, drifted).unwrap();
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Alpha
Text.

## REMOVED Requirements

### Requirement: Alpha
";
    let error = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert!(
        matches!(refusals[0].reason, RefusalReason::IncoherentDelta(_)),
        "note fault beats target fault"
    );
    fs::remove_dir_all(&root).unwrap();
}
