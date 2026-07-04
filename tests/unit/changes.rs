use nbspec::changes::{
    ArtifactLayout, ChangeError, ChangeMetadata, ChangeStatus, artifact_layout, change_folder,
    namespace_folders, namespace_notes, parse_meta_note, render_meta_note, validate_change_id,
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
