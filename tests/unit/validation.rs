use std::path::PathBuf;

use nbspec::rendering::RenderedDocument;
use nbspec::schemata::default_schema;
use nbspec::validation::{Diagnostic, ValidationFailure, validate_change};

const CHANGE_FOLDER: &str = "proposals/add-demo";

const VALID_SPECIFICATION: &str = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users before granting access.

#### Scenario: Valid login
- **WHEN** a user submits correct credentials
- **THEN** a session begins
";

fn proposal_document() -> RenderedDocument {
    RenderedDocument {
        artifact_id: "proposal".to_string(),
        tree_path: PathBuf::from("proposal.md"),
        target_path: None,
        source_note: format!("{CHANGE_FOLDER}/proposal.md"),
        content: "# proposal\n\nWhy: reasons.\n".to_string(),
    }
}

fn specification_document(name: &str, content: &str) -> RenderedDocument {
    RenderedDocument {
        artifact_id: "specifications".to_string(),
        tree_path: PathBuf::from(format!("specifications/{name}.md")),
        target_path: Some(PathBuf::from(format!(
            "documentation/specifications/{name}.md"
        ))),
        source_note: format!("{CHANGE_FOLDER}/specifications/{name}.md"),
        content: content.to_string(),
    }
}

fn design_document(content: &str) -> RenderedDocument {
    RenderedDocument {
        artifact_id: "designs".to_string(),
        tree_path: PathBuf::from("designs/notes.md"),
        target_path: Some(PathBuf::from("documentation/designs/notes.md")),
        source_note: format!("{CHANGE_FOLDER}/designs/notes.md"),
        content: content.to_string(),
    }
}

fn validate(documents: &[RenderedDocument]) -> Vec<Diagnostic> {
    validate_change(documents, &default_schema(), CHANGE_FOLDER)
}

#[test]
fn well_formed_change_yields_no_diagnostics() {
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", VALID_SPECIFICATION),
    ];
    assert_eq!(validate(&documents), Vec::new());
}

#[test]
fn missing_required_artifacts_are_reported_in_schema_order() {
    let diagnostics = validate(&[]);
    let summary: Vec<(&str, &str)> = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.artifact_id.as_str(), diagnostic.note.as_str()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("proposal", "proposals/add-demo/proposal.md"),
            ("specifications", "proposals/add-demo/specifications/"),
        ]
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.line.is_none())
    );
}

#[test]
fn scaffolded_placeholder_proposal_counts_as_unauthored() {
    let mut proposal = proposal_document();
    proposal.content = "# proposal\n\n<!-- Draft the proposal here. -->\n".to_string();
    let documents = vec![
        proposal,
        specification_document("user-auth", VALID_SPECIFICATION),
    ];
    let diagnostics = validate(&documents);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].artifact_id, "proposal");
    assert_eq!(
        diagnostics[0].message,
        "required artifact has no authored content"
    );
}

#[test]
fn requirement_without_scenario_is_reported_with_line() {
    let content = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users.
";
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", content),
    ];
    let diagnostics = validate(&documents);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].note,
        "proposals/add-demo/specifications/user-auth.md"
    );
    assert_eq!(diagnostics[0].line, Some(5));
    assert!(diagnostics[0].message.contains("no #### Scenario: block"));
}

#[test]
fn requirement_without_normative_text_is_reported() {
    let content = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication

#### Scenario: Valid login
- **WHEN** a user submits correct credentials
- **THEN** a session begins
";
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", content),
    ];
    let diagnostics = validate(&documents);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].line, Some(5));
    assert!(diagnostics[0].message.contains("no normative text"));
}

#[test]
fn duplicate_requirement_names_are_reported_at_the_duplicate() {
    let content = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users.

#### Scenario: Valid login
- **WHEN** correct credentials
- **THEN** session begins

### Requirement: User authentication
The system SHALL authenticate users again.

#### Scenario: Repeat login
- **WHEN** correct credentials
- **THEN** session begins
";
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", content),
    ];
    let diagnostics = validate(&documents);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].line, Some(12));
    assert!(
        diagnostics[0]
            .message
            .contains("duplicate requirement name")
    );
}

#[test]
fn empty_delta_sections_are_reported() {
    let content = "\
# user-auth

## ADDED Requirements

## REMOVED Requirements

## RENAMED Requirements
";
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", content),
    ];
    let messages: Vec<String> = validate(&documents)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    assert_eq!(
        messages,
        vec![
            "ADDED Requirements section declares no requirements",
            "REMOVED Requirements section names no requirements",
            "RENAMED Requirements section contains no FROM:/TO: pairs",
        ]
    );
}

#[test]
fn specification_without_delta_sections_is_reported() {
    let content = "\
# user-auth

Some prose that forgot the delta structure entirely.
";
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", content),
    ];
    let diagnostics = validate(&documents);
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("no delta sections"));
}

#[test]
fn scenario_without_content_is_reported() {
    let content = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users.

#### Scenario: Valid login
";
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", content),
    ];
    let diagnostics = validate(&documents);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].line, Some(8));
    assert!(diagnostics[0].message.contains("no WHEN/THEN content"));
}

#[test]
fn free_form_artifacts_face_no_grammar() {
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", VALID_SPECIFICATION),
        design_document("# notes\n\nProse with no delta sections at all.\n"),
    ];
    assert_eq!(validate(&documents), Vec::new());
}

#[test]
fn removed_and_renamed_sections_validate_without_scenarios() {
    let content = "\
# user-auth

## REMOVED Requirements

### Requirement: Legacy login

## RENAMED Requirements

- FROM: `### Requirement: Sign in`
- TO: `### Requirement: User authentication`
";
    let documents = vec![
        proposal_document(),
        specification_document("user-auth", content),
    ];
    assert_eq!(validate(&documents), Vec::new());
}

#[test]
fn conformance_delta_fixtures_pass_native_validation() {
    let entries = std::fs::read_dir("tests/fixtures/conformance/deltas").unwrap();
    let mut checked = 0;
    for entry in entries {
        let path = entry.unwrap().path();
        let content = std::fs::read_to_string(&path).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let documents = vec![proposal_document(), specification_document(&name, &content)];
        assert_eq!(validate(&documents), Vec::new(), "fixture {name}");
        checked += 1;
    }
    assert!(checked >= 3);
}

#[test]
fn diagnostic_display_includes_note_line_and_artifact() {
    let with_line = Diagnostic {
        note: "proposals/add-demo/specifications/user-auth.md".to_string(),
        artifact_id: "specifications".to_string(),
        line: Some(5),
        message: "requirement User authentication has no #### Scenario: block".to_string(),
    };
    assert_eq!(
        with_line.to_string(),
        "proposals/add-demo/specifications/user-auth.md:5: [specifications] \
         requirement User authentication has no #### Scenario: block"
    );
    let without_line = Diagnostic {
        note: "proposals/add-demo/proposal.md".to_string(),
        artifact_id: "proposal".to_string(),
        line: None,
        message: "required artifact has no authored content".to_string(),
    };
    assert_eq!(
        without_line.to_string(),
        "proposals/add-demo/proposal.md: [proposal] required artifact has no authored content"
    );
}

#[test]
fn failure_display_lists_one_diagnostic_per_line() {
    let failure = ValidationFailure {
        change_id: "add-demo".to_string(),
        diagnostics: validate(&[]),
    };
    let rendered = failure.to_string();
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines[0], "change add-demo is invalid: 2 violations");
    assert_eq!(lines.len(), 3);
    assert!(lines[1].starts_with("proposals/add-demo/proposal.md: "));
}
