use nbspec::changes::{
    ArtifactLayout, ChangeError, ChangeMetadata, ChangeStatus, artifact_layout, change_folder,
    first_h1_title, has_h2_section, namespace_folders, namespace_notes, parse_meta_note,
    render_meta_note, validate_change_id,
};
use nbspec::schemata::default_schema;

#[test]
fn change_ids_must_be_kebab_case() {
    for valid in ["add-foo", "fix-bar-baz", "v2-migration", "cleanup"] {
        assert!(validate_change_id(valid).is_ok(), "{valid} should be valid");
    }
    for invalid in [
        "", "Add-Foo", "add_foo", "add foo", "-add-foo", "add-foo-", "add--foo",
    ] {
        assert!(
            matches!(
                validate_change_id(invalid),
                Err(ChangeError::InvalidChangeId(_))
            ),
            "{invalid} should be invalid"
        );
    }
}

#[test]
fn change_folder_lives_under_proposals() {
    assert_eq!(change_folder("add-foo"), "proposals/add-foo");
}

#[test]
fn artifact_layouts_derive_from_generates_paths() {
    let schema = default_schema();
    assert_eq!(
        artifact_layout(schema.artifact("proposal").unwrap()),
        ArtifactLayout::Note("proposal".to_string())
    );
    assert_eq!(
        artifact_layout(schema.artifact("specifications").unwrap()),
        ArtifactLayout::Folder("specifications".to_string())
    );
}

#[test]
fn default_schema_namespace_has_expected_shape() {
    let schema = default_schema();
    assert_eq!(namespace_notes(&schema), vec!["proposal"]);
    assert_eq!(
        namespace_folders(&schema),
        vec!["specifications", "designs", "decisions"]
    );
}

#[test]
fn lifecycle_permits_main_progression_and_side_states() {
    use ChangeStatus::*;
    assert!(Draft.permits_transition(Approved));
    assert!(Approved.permits_transition(Implemented));
    assert!(Implemented.permits_transition(Archived));
    for state in [Draft, Approved, Implemented] {
        assert!(state.permits_transition(Blocked));
        assert!(state.permits_transition(Superseded));
        assert!(state.permits_transition(Abandoned));
    }
    assert!(Blocked.permits_transition(Draft));
    assert!(Blocked.permits_transition(Implemented));
}

#[test]
fn lifecycle_refuses_skips_and_terminal_exits() {
    use ChangeStatus::*;
    assert!(!Draft.permits_transition(Implemented));
    assert!(!Draft.permits_transition(Archived));
    assert!(!Approved.permits_transition(Draft));
    for terminal in [Archived, Superseded, Abandoned] {
        for next in [Draft, Approved, Implemented, Archived, Blocked] {
            assert!(!terminal.permits_transition(next));
        }
    }
}

#[test]
fn transition_updates_status_and_timestamp() {
    let mut metadata =
        ChangeMetadata::new("add-foo", Some("Add foo"), "nbspec-default", "nbspec").unwrap();
    let created = metadata.created_at;
    metadata.transition(ChangeStatus::Approved).unwrap();
    assert_eq!(metadata.status, ChangeStatus::Approved);
    assert!(metadata.updated_at >= created);
    assert_eq!(metadata.created_at, created);

    let error = metadata.transition(ChangeStatus::Draft).unwrap_err();
    assert!(matches!(error, ChangeError::InvalidTransition(_, _)));
    assert_eq!(metadata.status, ChangeStatus::Approved);
}

#[test]
fn record_commit_captures_status_and_sha() {
    let mut metadata = ChangeMetadata::new("add-foo", None, "nbspec-default", "nbspec").unwrap();
    metadata.transition(ChangeStatus::Approved).unwrap();
    metadata.record_commit("abc1234");
    assert_eq!(metadata.repository_commits.len(), 1);
    assert_eq!(metadata.repository_commits[0].commit, "abc1234");
    assert_eq!(
        metadata.repository_commits[0].status,
        ChangeStatus::Approved
    );
}

#[test]
fn meta_note_round_trips_through_fenced_json() {
    let metadata =
        ChangeMetadata::new("add-foo", Some("Add foo"), "nbspec-default", "nbspec").unwrap();
    let rendered = render_meta_note(&metadata).unwrap();
    assert!(rendered.starts_with("```json\n"));
    assert!(rendered.ends_with("```\n"));
    let parsed = parse_meta_note(&rendered).unwrap();
    assert_eq!(parsed, metadata);
}

#[test]
fn meta_note_parses_with_leading_title_heading() {
    let metadata = ChangeMetadata::new("add-foo", None, "nbspec-default", "nbspec").unwrap();
    let rendered = render_meta_note(&metadata).unwrap();
    let with_heading = format!("# meta\n\n{rendered}");
    let parsed = parse_meta_note(&with_heading).unwrap();
    assert_eq!(parsed, metadata);
}

#[test]
fn meta_note_parses_bare_json() {
    let metadata = ChangeMetadata::new("add-foo", None, "nbspec-default", "nbspec").unwrap();
    let json = serde_json::to_string_pretty(&metadata).unwrap();
    let parsed = parse_meta_note(&json).unwrap();
    assert_eq!(parsed, metadata);
}

#[test]
fn meta_note_without_json_reports_parse_failure() {
    let error = parse_meta_note("# meta\n\nno json here\n").unwrap_err();
    assert!(matches!(error, ChangeError::MetaParse(_)));
}

#[test]
fn status_serializes_lowercase() {
    let metadata = ChangeMetadata::new("add-foo", None, "nbspec-default", "nbspec").unwrap();
    let json = serde_json::to_string(&metadata).unwrap();
    assert!(json.contains("\"status\":\"draft\""));
    assert!(json.contains("\"meta_version\":1"));
}

#[test]
fn first_h1_title_skips_fenced_code_blocks() {
    let content = "\
```
# Not A Heading
```
# Real Heading
";
    assert_eq!(first_h1_title(content).as_deref(), Some("Real Heading"));
}

#[test]
fn first_h1_title_ignores_indented_heading() {
    let content = "    # Indented (code block)\n# Real\n";
    assert_eq!(first_h1_title(content).as_deref(), Some("Real"));
}

#[test]
fn first_h1_title_returns_none_for_fence_only() {
    let content = "\
```
# Only In Code Block
```
";
    assert!(first_h1_title(content).is_none());
}

#[test]
fn has_h2_section_skips_fenced_code_blocks() {
    let content = "\
```
## Why
```
## Why
";
    assert!(has_h2_section(content, "Why"));
    let fenced_only = "\
```
## Why
```
";
    assert!(!has_h2_section(fenced_only, "Why"));
}

#[test]
fn has_h2_section_ignores_indented_section() {
    let content = "    ## Why\n## Why\n";
    assert!(has_h2_section(content, "Why"));
    let indented_only = "    ## Why\n";
    assert!(!has_h2_section(indented_only, "Why"));
}

#[test]
fn first_h1_title_skips_tilde_fences() {
    let content = "~~~\n# Not A Heading\n~~~\n# Real\n";
    assert_eq!(first_h1_title(content).as_deref(), Some("Real"));
    let fenced_only = "~~~\n# Only In Tilde\n~~~\n";
    assert!(first_h1_title(fenced_only).is_none());
}

#[test]
fn first_h1_title_skips_html_comments() {
    let content = "<!--\n# Commented Out\n-->\n# Real\n";
    assert_eq!(first_h1_title(content).as_deref(), Some("Real"));
    let inline = "<!-- # inline -->\n# Real\n";
    assert_eq!(first_h1_title(inline).as_deref(), Some("Real"));
}

#[test]
fn first_h1_title_respects_fence_length() {
    let content = "````\n``` still inside\n````\n# Real\n";
    assert_eq!(first_h1_title(content).as_deref(), Some("Real"));
}

#[test]
fn first_h1_title_accepts_tab_after_marker() {
    let content = "#\tTabbed Title\n";
    assert_eq!(first_h1_title(content).as_deref(), Some("Tabbed Title"));
}

#[test]
fn first_h1_title_rejects_no_space_after_marker() {
    assert!(first_h1_title("#NotAHeading\n").is_none());
}

#[test]
fn has_h2_section_finds_non_first_heading() {
    let content = "\
# proposal

## Context

Background.

## Why

Reasons.
";
    assert!(has_h2_section(content, "Why"));
    assert!(has_h2_section(content, "Context"));
    assert!(!has_h2_section(content, "Impact"));
}

#[test]
fn has_h2_section_matches_each_of_multiple_required_names() {
    let content = "\
# design

## Context

Background.

## Goals

Ship it.

## Decisions

Use the shared scanner.
";
    assert!(has_h2_section(content, "Context"));
    assert!(has_h2_section(content, "Goals"));
    assert!(has_h2_section(content, "Decisions"));
    assert!(has_h2_section(content, "goals"));
}
