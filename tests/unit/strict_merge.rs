//! Strict-mode and typed-diagnostic merge coverage: scenario-drop
//! warn/apply with identity, strict preflight refusals (including
//! force precedence and multi-document atomicity), and
//! document-level diagnostics.

use std::fs;
use std::path::{Path, PathBuf};

use nbspec::merging::{MergeError, RefusalReason, merge_documents};
use nbspec::provenance;
use nbspec::rendering::RenderedDocument;

fn unique_temp_root(label: &str) -> PathBuf {
    let unique = format!(
        "{label}-{}_{}",
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
        provenance::stamp(
            body,
            change,
            "home",
            &format!("proposals/{change}/x.md"),
        ),
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
fn scenario_drop_warns_applies_and_carries_typed_identity() {
    let root = unique_temp_root("delta-scenario-drop");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
The system SHALL alpha loudly.
";
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.written.len(), 1, "drop applies outside strict");
    let body = fs::read_to_string(target_of(&root, "alpha")).unwrap();
    assert!(
        !body.contains("#### Scenario: Alphas"),
        "dropped scenario gone: {body}"
    );
    assert!(
        body.contains("alpha loudly"),
        "modified text applied: {body}"
    );
    assert_eq!(report.diagnostics.len(), 1);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "ScenarioDropped");
    assert_eq!(diagnostic.requirement.as_deref(), Some("Alpha"));
    assert_eq!(diagnostic.scenario.as_deref(), Some("Alphas"));
    assert_eq!(
        diagnostic.target.as_deref(),
        Some("documentation/specifications/alpha.md")
    );
    // Compatibility projection stays in sync with the typed entry.
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("Alphas"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn strict_refuses_scenario_drop_before_any_write() {
    let root = unique_temp_root("delta-strict-drop");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
The system SHALL alpha loudly.
";
    let error = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
        true,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected strict refusal");
    };
    assert_eq!(refusals.len(), 1);
    let RefusalReason::StrictDiagnostics(diagnostics) = &refusals[0].reason
    else {
        panic!("expected StrictDiagnostics, got {:?}", refusals[0].reason);
    };
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "ScenarioDropped");
    assert_eq!(diagnostics[0].requirement.as_deref(), Some("Alpha"));
    assert_eq!(diagnostics[0].scenario.as_deref(), Some("Alphas"));
    let display = refusals[0].reason.to_string();
    assert!(
        display.contains("ScenarioDropped") && display.contains("Alphas"),
        "refusal text names code and identity: {display}"
    );
    // Seeded target is untouched: planning refused before effects.
    let body = fs::read_to_string(target_of(&root, "alpha")).unwrap();
    assert!(body.contains("#### Scenario: Alphas"), "nothing written");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn strict_promotes_non_scenario_warning_and_force_does_not_bypass() {
    let root = unique_temp_root("delta-strict-force");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
The system SHALL alpha loudly.

## REMOVED Requirements

### Requirement: Ghost
";
    // Strict refuses even with force: the preflight gate prevails.
    let error = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        true,
        true,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected strict refusal under force");
    };
    assert!(
        matches!(refusals[0].reason, RefusalReason::StrictDiagnostics(_)),
        "strict prevails over force"
    );
    // Without strict, the same delta warns and applies under force.
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        true,
        false,
    )
    .unwrap();
    assert_eq!(report.written.len(), 1);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "ScenarioDropped")
            && report
                .diagnostics
                .iter()
                .any(|d| d.code == "AlreadyRemoved"),
        "both preflight diagnostics reported: {:?}",
        report.diagnostics
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn strict_refusal_is_atomic_across_documents() {
    let root = unique_temp_root("delta-strict-atomic");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    seed_target(&root, "beta", "add-prior", BASE_SPEC);
    let clean = "\
# beta

## ADDED Requirements

### Requirement: Gamma
The system SHALL gamma.
";
    let dropping = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
The system SHALL alpha loudly.
";
    let error = merge_documents(
        &[document("beta", clean), document("alpha", dropping)],
        &root,
        "add-demo",
        "home",
        None,
        false,
        true,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected strict refusal");
    };
    assert_eq!(refusals.len(), 1, "only the dropping target refuses");
    // The clean document must not have been written either.
    assert!(
        !target_of(&root, "beta").exists()
            || fs::read_to_string(target_of(&root, "beta"))
                .unwrap()
                .contains("Alphas"),
        "atomic: clean document unwritten"
    );
    // Seeded alpha untouched.
    let body = fs::read_to_string(target_of(&root, "alpha")).unwrap();
    assert!(body.contains("#### Scenario: Alphas"), "nothing written");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn no_warning_strict_merge_succeeds() {
    let root = unique_temp_root("delta-strict-clean");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
The system SHALL alpha loudly.

#### Scenario: Alphas
- **WHEN** alpha
- **THEN** alpha loudly
";
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
        true,
    )
    .unwrap();
    assert_eq!(report.written.len(), 1, "clean strict merge applies");
    assert!(report.diagnostics.is_empty());
    assert!(report.warnings.is_empty());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn document_level_diagnostic_carries_target_path() {
    let root = unique_temp_root("delta-doc-diagnostic");
    fs::create_dir_all(&root).unwrap();
    let target =
        "# alpha\n\n## Purpose\n\nOld why.\n\n## ADDED Requirements\n";
    seed_target(&root, "alpha", "add-prior", target);
    let delta = "\
# alpha

## Purpose

New why.

## ADDED Requirements

### Requirement: First
The system SHALL first.
";
    let report = merge_documents(
        &[document("alpha", delta)],
        &root,
        "add-demo",
        "home",
        None,
        false,
        false,
    )
    .unwrap();
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.code == "IgnoredPurpose")
        .expect("IgnoredPurpose diagnostic");
    assert_eq!(diagnostic.requirement, None);
    assert_eq!(diagnostic.scenario, None);
    assert_eq!(
        diagnostic.target.as_deref(),
        Some("documentation/specifications/alpha.md")
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn strict_diagnostics_survive_alongside_legacy_refusals() {
    use nbspec::merging::MergeError;
    let root = unique_temp_root("delta-strict-mixed");
    fs::create_dir_all(&root).unwrap();
    seed_target(&root, "alpha", "add-prior", BASE_SPEC);
    // Block the second target with a directory (non-file occupant).
    let blocked = target_of(&root, "blocked");
    fs::create_dir_all(&blocked).unwrap();
    let dropping = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
The system SHALL alpha loudly.
";
    let clean = "\
# blocked

## ADDED Requirements

### Requirement: Fresh
The system SHALL be fresh.
";
    let error = merge_documents(
        &[document("alpha", dropping), document("blocked", clean)],
        &root,
        "add-demo",
        "home",
        None,
        false,
        true,
    )
    .unwrap_err();
    let MergeError::Refused { refusals } = error else {
        panic!("expected refusal");
    };
    assert_eq!(refusals.len(), 2, "both targets refuse: {refusals:?}");
    let strict_count = refusals
        .iter()
        .filter(|r| {
            matches!(
                r.reason,
                nbspec::merging::RefusalReason::StrictDiagnostics(_)
            )
        })
        .count();
    assert_eq!(strict_count, 1, "strict entry preserved");
    assert!(
        refusals.iter().any(|r| matches!(
            r.reason,
            nbspec::merging::RefusalReason::NonFileTarget
        )),
        "legacy non-file entry preserved"
    );
    // Seeded target untouched; blocked path still a directory.
    let body = fs::read_to_string(target_of(&root, "alpha")).unwrap();
    assert!(body.contains("#### Scenario: Alphas"), "nothing written");
    assert!(target_of(&root, "blocked").is_dir());
    fs::remove_dir_all(&root).unwrap();
}
