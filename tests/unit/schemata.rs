use std::fs;
use std::path::PathBuf;

use nbspec::configuration::{CONFIGURATION_DIR, ProjectConfiguration, load_configuration};
use nbspec::schemata::{
    DEFAULT_SCHEMA_NAME, SchemaError, default_schema, parse_schema, resolve_schema,
};

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

#[test]
fn default_schema_declares_expected_artifacts() {
    let schema = default_schema();
    assert_eq!(schema.name, DEFAULT_SCHEMA_NAME);
    let ids: Vec<&str> = schema
        .artifacts
        .iter()
        .map(|artifact| artifact.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["proposal", "specifications", "designs", "decisions"]
    );
    assert!(schema.artifact("tasks").is_none());
}

#[test]
fn default_schema_targets_documentation_directories() {
    let schema = default_schema();
    assert_eq!(schema.artifact("proposal").unwrap().target, None);
    assert_eq!(
        schema.artifact("specifications").unwrap().target.as_deref(),
        Some("documentation/specifications")
    );
    assert_eq!(
        schema.artifact("designs").unwrap().target.as_deref(),
        Some("documentation/designs")
    );
    assert_eq!(
        schema.artifact("decisions").unwrap().target.as_deref(),
        Some("documentation/decisions")
    );
}

#[test]
fn authoring_order_respects_dependencies() {
    let schema = default_schema();
    let order: Vec<&str> = schema
        .authoring_order()
        .iter()
        .map(|artifact| artifact.id.as_str())
        .collect();
    let proposal_position = order.iter().position(|id| *id == "proposal").unwrap();
    for id in ["specifications", "designs", "decisions"] {
        let position = order.iter().position(|other| other == &id).unwrap();
        assert!(proposal_position < position);
    }
}

#[test]
fn upstream_style_schema_with_unknown_fields_parses() {
    let yaml = "\
name: spec-driven
version: 1
description: upstream-style schema
artifacts:
  - id: proposal
    generates: proposal.md
    template: proposal.md
    requires: []
  - id: tasks
    generates: tasks.md
    requires: [proposal]
apply:
  requires: [tasks]
  tracks: tasks.md
";
    let schema = parse_schema(yaml).unwrap();
    assert_eq!(schema.name, "spec-driven");
    assert_eq!(schema.artifacts.len(), 2);
    assert_eq!(
        schema.artifact("proposal").unwrap().template.as_deref(),
        Some("proposal.md")
    );
}

#[test]
fn duplicate_artifact_ids_are_invalid() {
    let yaml = "\
name: broken
version: 1
artifacts:
  - id: proposal
    generates: proposal.md
  - id: proposal
    generates: proposal2.md
";
    assert!(matches!(parse_schema(yaml), Err(SchemaError::Invalid(_))));
}

#[test]
fn unknown_dependency_is_invalid() {
    let yaml = "\
name: broken
version: 1
artifacts:
  - id: proposal
    generates: proposal.md
    requires: [phantom]
";
    assert!(matches!(parse_schema(yaml), Err(SchemaError::Invalid(_))));
}

#[test]
fn dependency_cycle_is_invalid() {
    let yaml = "\
name: broken
version: 1
artifacts:
  - id: alpha
    generates: alpha.md
    requires: [beta]
  - id: beta
    generates: beta.md
    requires: [alpha]
";
    let error = parse_schema(yaml).unwrap_err();
    assert!(matches!(error, SchemaError::Invalid(_)));
    assert!(error.to_string().contains("cycle"));
}

#[test]
fn resolution_falls_back_to_embedded_default() {
    let root = unique_temp_root("schemata-default");
    fs::create_dir_all(&root).unwrap();
    let configuration = ProjectConfiguration::default();
    let schema = resolve_schema(None, &configuration, &root).unwrap();
    assert_eq!(schema.name, DEFAULT_SCHEMA_NAME);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn resolution_prefers_explicit_name_over_configuration() {
    let root = unique_temp_root("schemata-explicit");
    fs::create_dir_all(&root).unwrap();
    let configuration = ProjectConfiguration {
        schema: Some("missing-config-schema".to_string()),
    };
    let schema = resolve_schema(Some(DEFAULT_SCHEMA_NAME), &configuration, &root).unwrap();
    assert_eq!(schema.name, DEFAULT_SCHEMA_NAME);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn resolution_loads_project_schema_from_configuration_directory() {
    let root = unique_temp_root("schemata-project");
    let schema_dir = root.join(CONFIGURATION_DIR).join("schemata").join("custom");
    fs::create_dir_all(&schema_dir).unwrap();
    fs::write(
        schema_dir.join("schema.yaml"),
        "\
name: custom
version: 1
artifacts:
  - id: proposal
    generates: proposal.md
",
    )
    .unwrap();
    let configuration = ProjectConfiguration::default();
    let schema = resolve_schema(Some("custom"), &configuration, &root).unwrap();
    assert_eq!(schema.name, "custom");
    assert_eq!(schema.artifacts.len(), 1);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn resolution_reports_missing_named_schema() {
    let root = unique_temp_root("schemata-missing");
    fs::create_dir_all(&root).unwrap();
    let configuration = ProjectConfiguration::default();
    let error = resolve_schema(Some("phantom"), &configuration, &root).unwrap_err();
    assert!(matches!(error, SchemaError::NotFound(_)));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn configuration_loads_from_project_and_defaults_when_absent() {
    let root = unique_temp_root("configuration");
    let config_dir = root.join(CONFIGURATION_DIR);
    fs::create_dir_all(&config_dir).unwrap();

    let absent = load_configuration(&root).unwrap();
    assert_eq!(absent.schema, None);

    fs::write(config_dir.join("config.yaml"), "schema: custom\n").unwrap();
    let present = load_configuration(&root).unwrap();
    assert_eq!(present.schema.as_deref(), Some("custom"));

    fs::remove_dir_all(&root).unwrap();
}
