use std::fs;
use std::path::PathBuf;

use nbspec::configuration::{
    Configuration, PROJECT_CONFIGURATION_DIR_DEFAULT, SETTINGS_FILE, SettingsDocument,
    resolve_configuration,
};
use nbspec::schemata::{
    SCHEMA_FILE, SCHEMA_NAME_DEFAULT, SchemaError, default_schema, parse_schema, resolve_schema,
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

fn default_configuration(root: &std::path::Path) -> Configuration {
    Configuration {
        schema: None,
        project_directory: root.join(PROJECT_CONFIGURATION_DIR_DEFAULT),
        scratch_directory: None,
        archives: true,
        archive_directory: PathBuf::from("documentation/archives"),
    }
}

#[test]
fn default_schema_declares_expected_artifacts() {
    let schema = default_schema();
    assert_eq!(schema.name, SCHEMA_NAME_DEFAULT);
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
fn schema_with_unknown_fields_parses() {
    let content = "\
name = \"custom\"
version = 1
description = \"schema with unknown keys\"
future_key = \"ignored\"

[[artifacts]]
id = \"proposal\"
generates = \"proposal.md\"
template = \"proposal.md\"
requires = []
";
    let schema = parse_schema(content).unwrap();
    assert_eq!(schema.name, "custom");
    assert_eq!(
        schema.artifact("proposal").unwrap().template.as_deref(),
        Some("proposal.md")
    );
}

#[test]
fn duplicate_artifact_ids_are_invalid() {
    let content = "\
name = \"broken\"
version = 1

[[artifacts]]
id = \"proposal\"
generates = \"proposal.md\"

[[artifacts]]
id = \"proposal\"
generates = \"proposal2.md\"
";
    assert!(matches!(
        parse_schema(content),
        Err(SchemaError::Invalid(_))
    ));
}

#[test]
fn unknown_dependency_is_invalid() {
    let content = "\
name = \"broken\"
version = 1

[[artifacts]]
id = \"proposal\"
generates = \"proposal.md\"
requires = [\"phantom\"]
";
    assert!(matches!(
        parse_schema(content),
        Err(SchemaError::Invalid(_))
    ));
}

fn single_artifact_schema(generates: &str, target: Option<&str>) -> String {
    let target_line = target
        .map(|value| format!("target = \"{value}\"\n"))
        .unwrap_or_default();
    format!(
        "\
name = \"paths\"
version = 1

[[artifacts]]
id = \"artifact\"
generates = \"{generates}\"
{target_line}"
    )
}

#[test]
fn absolute_generates_path_is_invalid() {
    let error = parse_schema(&single_artifact_schema("/tmp/escape.md", None)).unwrap_err();
    assert!(matches!(error, SchemaError::Invalid(_)));
    assert!(error.to_string().contains("absolute"));
}

#[test]
fn parent_directory_target_is_invalid() {
    let error =
        parse_schema(&single_artifact_schema("artifact.md", Some("../outside"))).unwrap_err();
    assert!(matches!(error, SchemaError::Invalid(_)));
    assert!(error.to_string().contains("parent-directory"));
}

#[test]
fn absolute_target_is_invalid() {
    let error =
        parse_schema(&single_artifact_schema("artifact.md", Some("/etc/nbspec"))).unwrap_err();
    assert!(matches!(error, SchemaError::Invalid(_)));
}

#[test]
fn nested_parent_directory_generates_is_invalid() {
    let error = parse_schema(&single_artifact_schema("docs/../../escape.md", None)).unwrap_err();
    assert!(matches!(error, SchemaError::Invalid(_)));
}

#[test]
fn backslash_and_drive_prefix_paths_are_invalid() {
    // TOML escaping: "docs\\\\escape.md" in the document parses to a
    // value containing one literal backslash.
    for path in ["docs\\\\escape.md", "c:/escape.md"] {
        let error = parse_schema(&single_artifact_schema(path, None)).unwrap_err();
        assert!(matches!(error, SchemaError::Invalid(_)), "path: {path}");
    }
}

#[test]
fn glob_generates_paths_remain_valid() {
    let schema = parse_schema(&single_artifact_schema(
        "specifications/**/*.md",
        Some("documentation/specifications"),
    ))
    .unwrap();
    assert_eq!(schema.artifacts.len(), 1);
}

#[test]
fn dependency_cycle_is_invalid() {
    let content = "\
name = \"broken\"
version = 1

[[artifacts]]
id = \"alpha\"
generates = \"alpha.md\"
requires = [\"beta\"]

[[artifacts]]
id = \"beta\"
generates = \"beta.md\"
requires = [\"alpha\"]
";
    let error = parse_schema(content).unwrap_err();
    assert!(matches!(error, SchemaError::Invalid(_)));
    assert!(error.to_string().contains("cycle"));
}

#[test]
fn resolution_falls_back_to_embedded_default() {
    let root = unique_temp_root("schemata-default");
    fs::create_dir_all(&root).unwrap();
    let schema = resolve_schema(None, &default_configuration(&root)).unwrap();
    assert_eq!(schema.name, SCHEMA_NAME_DEFAULT);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn resolution_prefers_explicit_name_over_configuration() {
    let root = unique_temp_root("schemata-explicit");
    fs::create_dir_all(&root).unwrap();
    let configuration = Configuration {
        schema: Some("missing-config-schema".to_string()),
        project_directory: root.join(PROJECT_CONFIGURATION_DIR_DEFAULT),
        scratch_directory: None,
        archives: true,
        archive_directory: PathBuf::from("documentation/archives"),
    };
    let schema = resolve_schema(Some(SCHEMA_NAME_DEFAULT), &configuration).unwrap();
    assert_eq!(schema.name, SCHEMA_NAME_DEFAULT);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn resolution_loads_project_schema_from_configuration_directory() {
    let root = unique_temp_root("schemata-project");
    let configuration = default_configuration(&root);
    let schema_dir = configuration.project_directory.join("schemata/custom");
    fs::create_dir_all(&schema_dir).unwrap();
    fs::write(
        schema_dir.join(SCHEMA_FILE),
        "\
name = \"custom\"
version = 1

[[artifacts]]
id = \"proposal\"
generates = \"proposal.md\"
",
    )
    .unwrap();
    let schema = resolve_schema(Some("custom"), &configuration).unwrap();
    assert_eq!(schema.name, "custom");
    assert_eq!(schema.artifacts.len(), 1);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn resolution_reports_missing_named_schema() {
    let root = unique_temp_root("schemata-missing");
    fs::create_dir_all(&root).unwrap();
    let error = resolve_schema(Some("phantom"), &default_configuration(&root)).unwrap_err();
    assert!(matches!(error, SchemaError::NotFound(_)));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn configuration_defaults_when_no_files_present() {
    let root = unique_temp_root("configuration-defaults");
    fs::create_dir_all(&root).unwrap();
    let configuration = resolve_configuration(&root, SettingsDocument::default(), None).unwrap();
    assert_eq!(configuration.schema, None);
    assert_eq!(
        configuration.project_directory,
        root.join(PROJECT_CONFIGURATION_DIR_DEFAULT)
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn project_settings_override_global_settings() {
    let root = unique_temp_root("configuration-layering");
    let project_dir = root.join(PROJECT_CONFIGURATION_DIR_DEFAULT);
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(
        project_dir.join(SETTINGS_FILE),
        "schema = \"from-project\"\n",
    )
    .unwrap();
    let global = SettingsDocument {
        schema: Some("from-global".to_string()),
        ..SettingsDocument::default()
    };
    let configuration = resolve_configuration(&root, global, None).unwrap();
    assert_eq!(configuration.schema.as_deref(), Some("from-project"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn global_settings_apply_when_project_is_silent() {
    let root = unique_temp_root("configuration-global");
    fs::create_dir_all(&root).unwrap();
    let global = SettingsDocument {
        schema: Some("from-global".to_string()),
        ..SettingsDocument::default()
    };
    let configuration = resolve_configuration(&root, global, None).unwrap();
    assert_eq!(configuration.schema.as_deref(), Some("from-global"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn archives_default_enabled_at_default_directory() {
    let root = unique_temp_root("configuration-archives-default");
    fs::create_dir_all(&root).unwrap();
    let configuration = resolve_configuration(&root, SettingsDocument::default(), None).unwrap();
    assert!(configuration.archives);
    assert_eq!(
        configuration.archive_directory,
        PathBuf::from("documentation/archives")
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn project_settings_disable_and_relocate_archives() {
    let root = unique_temp_root("configuration-archives-project");
    let project_dir = root.join(PROJECT_CONFIGURATION_DIR_DEFAULT);
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(
        project_dir.join(SETTINGS_FILE),
        "archives = false\narchive_directory = \"records/archives\"\n",
    )
    .unwrap();
    let configuration = resolve_configuration(&root, SettingsDocument::default(), None).unwrap();
    assert!(!configuration.archives);
    assert_eq!(
        configuration.archive_directory,
        PathBuf::from("records/archives")
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn environment_override_relocates_project_directory() {
    let root = unique_temp_root("configuration-env");
    let relocated = root.join("elsewhere/nbspec");
    fs::create_dir_all(&relocated).unwrap();
    fs::write(relocated.join(SETTINGS_FILE), "schema = \"relocated\"\n").unwrap();
    let configuration = resolve_configuration(
        &root,
        SettingsDocument::default(),
        Some(PathBuf::from("elsewhere/nbspec")),
    )
    .unwrap();
    assert_eq!(configuration.schema.as_deref(), Some("relocated"));
    assert_eq!(
        configuration.project_directory,
        root.join("elsewhere/nbspec")
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn global_setting_relocates_project_directory_below_environment() {
    let root = unique_temp_root("configuration-precedence");
    let from_global = root.join("global-choice");
    let from_env = root.join("env-choice");
    fs::create_dir_all(&from_global).unwrap();
    fs::create_dir_all(&from_env).unwrap();
    fs::write(from_global.join(SETTINGS_FILE), "schema = \"global-dir\"\n").unwrap();
    fs::write(from_env.join(SETTINGS_FILE), "schema = \"env-dir\"\n").unwrap();
    let global = SettingsDocument {
        project_configuration_directory: Some(PathBuf::from("global-choice")),
        ..SettingsDocument::default()
    };

    let without_env = resolve_configuration(&root, global.clone(), None).unwrap();
    assert_eq!(without_env.schema.as_deref(), Some("global-dir"));

    let with_env = resolve_configuration(&root, global, Some(PathBuf::from("env-choice"))).unwrap();
    assert_eq!(with_env.schema.as_deref(), Some("env-dir"));

    fs::remove_dir_all(&root).unwrap();
}
