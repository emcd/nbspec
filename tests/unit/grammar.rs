use nbspec::grammar::{Rename, parse_delta_specification};

const ADDED_DELTA: &str = "\
## ADDED Requirements

### Requirement: User authentication
Users SHALL authenticate before accessing protected resources.

#### Scenario: Valid credentials accepted
- **WHEN** a user submits valid credentials
- **THEN** a session is established

#### Scenario: Invalid credentials rejected
- **WHEN** a user submits invalid credentials
- **THEN** access is denied

### Requirement: Session expiry
Sessions SHALL expire after thirty minutes of inactivity.

#### Scenario: Idle session closed
- **WHEN** a session is idle past the expiry window
- **THEN** the session is invalidated
";

#[test]
fn added_requirements_parse_with_scenarios() {
    let delta = parse_delta_specification(ADDED_DELTA);
    assert_eq!(delta.added.len(), 2);
    assert!(delta.presence.added);
    assert!(!delta.presence.modified);

    let first = &delta.added[0];
    assert_eq!(first.name, "User authentication");
    assert_eq!(
        first.text.as_deref(),
        Some("Users SHALL authenticate before accessing protected resources.")
    );
    assert_eq!(first.scenarios.len(), 2);
    assert_eq!(first.scenarios[0].name, "Valid credentials accepted");
    assert!(first.scenarios[0].body.contains("**WHEN**"));

    let second = &delta.added[1];
    assert_eq!(second.name, "Session expiry");
    assert_eq!(second.scenarios.len(), 1);
}

#[test]
fn line_numbers_are_one_indexed_source_positions() {
    let delta = parse_delta_specification(ADDED_DELTA);
    assert_eq!(delta.added[0].line, 3);
    assert_eq!(delta.added[0].scenarios[0].line, 6);
    assert_eq!(delta.added[1].line, 14);
}

#[test]
fn section_and_requirement_headers_match_case_insensitively() {
    let content = "\
## added requirements

###Requirement: Compact header
The system SHALL tolerate a missing space after the header marker.

####scenario: compact scenario header
- **WHEN** headers omit whitespace
- **THEN** parsing still succeeds
";
    let delta = parse_delta_specification(content);
    assert!(delta.presence.added);
    assert_eq!(delta.added.len(), 1);
    assert_eq!(delta.added[0].name, "Compact header");
    assert_eq!(delta.added[0].scenarios.len(), 1);
    assert_eq!(delta.added[0].scenarios[0].name, "compact scenario header");
}

#[test]
fn modified_section_parses_independently_of_added() {
    let content = "\
## MODIFIED Requirements

### Requirement: Session expiry
Sessions SHALL expire after sixty minutes of inactivity.

#### Scenario: Idle session closed
- **WHEN** a session is idle past the expiry window
- **THEN** the session is invalidated
";
    let delta = parse_delta_specification(content);
    assert!(delta.presence.modified);
    assert!(!delta.presence.added);
    assert_eq!(delta.modified.len(), 1);
    assert!(delta.added.is_empty());
}

#[test]
fn removed_section_accepts_headers_and_bullets() {
    let content = "\
## REMOVED Requirements

### Requirement: Legacy login

- `### Requirement: Password hints`
- ### Requirement: Security questions
";
    let delta = parse_delta_specification(content);
    assert!(delta.presence.removed);
    assert_eq!(
        delta.removed,
        vec!["Legacy login", "Password hints", "Security questions"]
    );
}

#[test]
fn renamed_section_pairs_from_and_to_lines() {
    let content = "\
## RENAMED Requirements

- FROM: `### Requirement: Login`
- TO: `### Requirement: User authentication`
FROM: ### Requirement: Logout
TO: ### Requirement: Session termination
";
    let delta = parse_delta_specification(content);
    assert!(delta.presence.renamed);
    assert_eq!(
        delta.renamed,
        vec![
            Rename {
                from: "Login".to_string(),
                to: "User authentication".to_string(),
            },
            Rename {
                from: "Logout".to_string(),
                to: "Session termination".to_string(),
            },
        ]
    );
}

#[test]
fn rename_labels_are_case_sensitive() {
    let content = "\
## RENAMED Requirements

- from: `### Requirement: Login`
- to: `### Requirement: User authentication`
";
    let delta = parse_delta_specification(content);
    assert!(delta.presence.renamed);
    assert!(delta.renamed.is_empty());
}

#[test]
fn empty_sections_register_presence_without_content() {
    let content = "## ADDED Requirements\n\n## REMOVED Requirements\n";
    let delta = parse_delta_specification(content);
    assert!(delta.presence.added);
    assert!(delta.presence.removed);
    assert!(delta.added.is_empty());
    assert!(delta.removed.is_empty());
}

#[test]
fn requirement_without_text_reports_none() {
    let content = "\
## ADDED Requirements

### Requirement: Undocumented behavior

#### Scenario: Something happens
- **WHEN** an event occurs
- **THEN** an outcome follows
";
    let delta = parse_delta_specification(content);
    assert_eq!(delta.added.len(), 1);
    assert_eq!(delta.added[0].text, None);
    assert_eq!(delta.added[0].scenarios.len(), 1);
}

#[test]
fn requirement_block_ends_at_next_section_header() {
    let content = "\
## ADDED Requirements

### Requirement: Bounded block
The block SHALL end at the next level-2 header.

#### Scenario: Boundary respected
- **WHEN** a level-2 header follows
- **THEN** the block excludes it

## Notes

This trailing section is not part of any requirement.
";
    let delta = parse_delta_specification(content);
    assert_eq!(delta.added.len(), 1);
    assert!(!delta.added[0].raw.contains("trailing section"));
    assert!(delta.added[0].raw.ends_with("the block excludes it"));
}

#[test]
fn scenario_body_keeps_deep_subheadings() {
    let content = "\
## ADDED Requirements

### Requirement: Annotated behavior
The system SHALL support annotated scenarios.

#### Scenario: Documented outcome
- **WHEN** an event occurs
- **THEN** an outcome follows

##### Notes
Deep subheadings belong to the scenario body.

#### Scenario: Second scenario
- **WHEN** something else occurs
- **THEN** another outcome follows
";
    let delta = parse_delta_specification(content);
    assert_eq!(delta.added.len(), 1);
    let scenarios = &delta.added[0].scenarios;
    assert_eq!(scenarios.len(), 2);
    assert!(scenarios[0].body.contains("##### Notes"));
    assert!(
        scenarios[0]
            .body
            .ends_with("Deep subheadings belong to the scenario body.")
    );
    assert_eq!(scenarios[1].name, "Second scenario");
}

#[test]
fn scenario_body_keeps_hashtag_lines_without_space() {
    let content = "\
## ADDED Requirements

### Requirement: Tagged behavior
The system SHALL tolerate hashtag-like lines.

#### Scenario: Tagged outcome
- **WHEN** an event occurs
- **THEN** an outcome follows
#hashtag
";
    let delta = parse_delta_specification(content);
    let scenarios = &delta.added[0].scenarios;
    assert_eq!(scenarios.len(), 1);
    assert!(scenarios[0].body.ends_with("#hashtag"));
}

#[test]
fn crlf_line_endings_are_normalized() {
    let content = ADDED_DELTA.replace('\n', "\r\n");
    let delta = parse_delta_specification(&content);
    assert_eq!(delta.added.len(), 2);
    assert_eq!(delta.added[0].scenarios.len(), 2);
    assert_eq!(delta.added[0].line, 3);
}

#[test]
fn raw_block_preserves_header_and_body() {
    let delta = parse_delta_specification(ADDED_DELTA);
    let raw = &delta.added[1].raw;
    assert!(raw.starts_with("### Requirement: Session expiry"));
    assert!(raw.contains("#### Scenario: Idle session closed"));
    assert!(raw.ends_with("the session is invalidated"));
}
