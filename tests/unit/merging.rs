use std::fs;
use std::path::PathBuf;

use nbspec::merging::{MergeError, RefusalReason, TargetStatus, merge_documents, target_status};
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

fn render_only_document(content: &str) -> RenderedDocument {
    RenderedDocument {
        artifact_id: "proposal".to_string(),
        tree_path: "proposal.md".to_string(),
        target_path: None,
        source_note: "proposals/add-demo/proposal.md".to_string(),
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
fn provenance_round_trips_through_stamp_and_split() {
    let stamped = provenance::stamp(ADDED_SPEC, "add-demo", "home", "proposals/add-demo/x.md");
    let (header, body) = provenance::split_document(&stamped);
    let header = header.unwrap();
    assert_eq!(header.change_id, "add-demo");
    assert_eq!(header.notebook, "home");
    assert_eq!(header.note, "proposals/add-demo/x.md");
    assert_eq!(body, ADDED_SPEC);
    assert!(provenance::body_matches(&header, body));
}

#[test]
fn split_document_passes_unmanaged_content_through() {
    let (header, body) = provenance::split_document("# plain\n\ncontent\n");
    assert!(header.is_none());
    assert_eq!(body, "# plain\n\ncontent\n");
}

#[test]
fn merge_writes_new_documents_with_provenance() {
    let root = unique_temp_root("merging-new");
    fs::create_dir_all(&root).unwrap();
    let documents = vec![
        render_only_document("# proposal\n"),
        document("alpha", ADDED_SPEC),
    ];
    let report = merge_documents(&documents, &root, "add-demo", "home", false).unwrap();
    assert_eq!(
        report.written,
        vec!["documentation/specifications/alpha.md".to_string()]
    );
    let written = fs::read_to_string(root.join("documentation/specifications/alpha.md")).unwrap();
    let (header, body) = provenance::split_document(&written);
    assert_eq!(header.unwrap().change_id, "add-demo");
    assert_eq!(body, ADDED_SPEC);
    assert!(!root.join("proposal.md").exists());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn remerge_of_identical_content_is_unchanged() {
    let root = unique_temp_root("merging-idempotent");
    fs::create_dir_all(&root).unwrap();
    let documents = vec![document("alpha", ADDED_SPEC)];
    merge_documents(&documents, &root, "add-demo", "home", false).unwrap();
    let first = fs::read_to_string(root.join("documentation/specifications/alpha.md")).unwrap();
    let report = merge_documents(&documents, &root, "add-demo", "home", false).unwrap();
    assert!(report.written.is_empty());
    assert_eq!(
        report.unchanged,
        vec!["documentation/specifications/alpha.md".to_string()]
    );
    let second = fs::read_to_string(root.join("documentation/specifications/alpha.md")).unwrap();
    assert_eq!(first, second);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn hand_edited_target_refuses_and_writes_nothing() {
    let root = unique_temp_root("merging-drift");
    fs::create_dir_all(&root).unwrap();
    let alpha = document("alpha", ADDED_SPEC);
    merge_documents(
        std::slice::from_ref(&alpha),
        &root,
        "add-demo",
        "home",
        false,
    )
    .unwrap();
    let target = root.join("documentation/specifications/alpha.md");
    let edited = fs::read_to_string(&target)
        .unwrap()
        .replace("alpha", "omega");
    fs::write(&target, &edited).unwrap();

    let beta = document("beta", ADDED_SPEC);
    let updated_alpha = document("alpha", "# alpha\n\n## ADDED Requirements\n\nrevised\n");
    let error =
        merge_documents(&[updated_alpha, beta], &root, "add-demo", "home", false).unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].reason, RefusalReason::Drifted);
    assert_eq!(fs::read_to_string(&target).unwrap(), edited);
    assert!(!root.join("documentation/specifications/beta.md").exists());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn force_overwrites_drift_with_fresh_provenance() {
    let root = unique_temp_root("merging-force");
    fs::create_dir_all(&root).unwrap();
    let alpha = document("alpha", ADDED_SPEC);
    merge_documents(
        std::slice::from_ref(&alpha),
        &root,
        "add-demo",
        "home",
        false,
    )
    .unwrap();
    let target = root.join("documentation/specifications/alpha.md");
    fs::write(&target, "hand edits\n").unwrap();

    let report = merge_documents(
        std::slice::from_ref(&alpha),
        &root,
        "add-demo",
        "home",
        true,
    )
    .unwrap();
    assert_eq!(report.written.len(), 1);
    let restored = fs::read_to_string(&target).unwrap();
    let (header, body) = provenance::split_document(&restored);
    assert_eq!(body, ADDED_SPEC);
    assert!(provenance::body_matches(&header.unwrap(), body));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unmanaged_target_refuses_without_force() {
    let root = unique_temp_root("merging-unmanaged");
    let target_dir = root.join("documentation/specifications");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("alpha.md"), "# hand-authored document\n").unwrap();
    let error = merge_documents(
        &[document("alpha", ADDED_SPEC)],
        &root,
        "add-demo",
        "home",
        false,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert_eq!(refusals[0].reason, RefusalReason::Unmanaged);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn directory_at_target_refuses_and_writes_nothing() {
    let root = unique_temp_root("merging-non-file");
    fs::create_dir_all(root.join("documentation/specifications/beta.md")).unwrap();
    let alpha = document("alpha", ADDED_SPEC);
    let beta = document("beta", ADDED_SPEC);
    let error = merge_documents(&[alpha, beta], &root, "add-demo", "home", false).unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].reason, RefusalReason::NonFileTarget);
    assert!(!root.join("documentation/specifications/alpha.md").exists());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn directory_at_target_refuses_even_with_force() {
    let root = unique_temp_root("merging-non-file-force");
    fs::create_dir_all(root.join("documentation/specifications/alpha.md")).unwrap();
    let error = merge_documents(
        &[document("alpha", ADDED_SPEC)],
        &root,
        "add-demo",
        "home",
        true,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert_eq!(refusals[0].reason, RefusalReason::NonFileTarget);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn directory_at_target_reports_non_file_status() {
    let root = unique_temp_root("merging-non-file-status");
    fs::create_dir_all(root.join("documentation/specifications/alpha.md")).unwrap();
    let alpha = document("alpha", ADDED_SPEC);
    assert_eq!(
        target_status(&alpha, &root, "add-demo").unwrap(),
        TargetStatus::NonFile
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn target_owned_by_other_change_refuses() {
    let root = unique_temp_root("merging-foreign");
    fs::create_dir_all(&root).unwrap();
    merge_documents(
        &[document("alpha", ADDED_SPEC)],
        &root,
        "add-first",
        "home",
        false,
    )
    .unwrap();
    let error = merge_documents(
        &[document("alpha", ADDED_SPEC)],
        &root,
        "add-second",
        "home",
        false,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert_eq!(
        refusals[0].reason,
        RefusalReason::ForeignChange("add-first".to_string())
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn modified_delta_refuses_even_with_force() {
    let root = unique_temp_root("merging-modified");
    fs::create_dir_all(&root).unwrap();
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
Changed text.

#### Scenario: Alphas
- **WHEN** alpha
- **THEN** alpha
";
    let error =
        merge_documents(&[document("alpha", delta)], &root, "add-demo", "home", true).unwrap_err();
    let message = error.to_string();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert_eq!(
        refusals[0].reason,
        RefusalReason::UnsupportedDelta(vec!["MODIFIED".to_string()])
    );
    assert!(!root.join("documentation/specifications/alpha.md").exists());
    assert!(message.contains("documentation/specifications/alpha.md"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn target_status_reflects_lifecycle() {
    let root = unique_temp_root("merging-status");
    fs::create_dir_all(&root).unwrap();
    let alpha = document("alpha", ADDED_SPEC);
    assert_eq!(
        target_status(&alpha, &root, "add-demo").unwrap(),
        TargetStatus::NotMerged
    );
    merge_documents(
        std::slice::from_ref(&alpha),
        &root,
        "add-demo",
        "home",
        false,
    )
    .unwrap();
    assert_eq!(
        target_status(&alpha, &root, "add-demo").unwrap(),
        TargetStatus::Current
    );
    let revised = document("alpha", "# alpha\n\n## ADDED Requirements\n\nrevised\n");
    assert_eq!(
        target_status(&revised, &root, "add-demo").unwrap(),
        TargetStatus::UpdatePending
    );
    let target = root.join("documentation/specifications/alpha.md");
    let edited = fs::read_to_string(&target).unwrap() + "\nhand edit\n";
    fs::write(&target, edited).unwrap();
    assert_eq!(
        target_status(&alpha, &root, "add-demo").unwrap(),
        TargetStatus::Drifted
    );
    assert_eq!(
        target_status(&alpha, &root, "add-other").unwrap(),
        TargetStatus::OwnedByOtherChange("add-demo".to_string())
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn render_only_documents_report_not_merged() {
    let root = unique_temp_root("merging-render-only");
    fs::create_dir_all(&root).unwrap();
    let proposal = render_only_document("# proposal\n");
    assert_eq!(
        target_status(&proposal, &root, "add-demo").unwrap(),
        TargetStatus::NotMerged
    );
    fs::remove_dir_all(&root).unwrap();
}
