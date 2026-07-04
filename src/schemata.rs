//! Workflow schema model: artifact sets, dependency graphs, merge targets.
//!
//! Follows the OpenSpec 1.x workflow schema mechanism (artifact list
//! with `generates` paths and a `requires` dependency graph),
//! serialized as TOML, and extends it with a per-artifact `target`
//! field naming the repository directory that receives the artifact's
//! documents at merge. Artifacts without a `target` render for review
//! but never merge. Unknown fields are ignored for forward
//! compatibility; upstream YAML schemas share the data model and are a
//! one-time conversion away.
//!
//! Schema resolution order: an explicit name (from a change's meta
//! note), then the project configuration, then the embedded nbspec
//! default schema. Named schemas load from
//! `schemata/<name>/schema.toml` under the resolved project
//! configuration directory.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use serde::Deserialize;
use thiserror::Error;

use crate::configuration::Configuration;

/// Name of the embedded default schema.
pub const SCHEMA_NAME_DEFAULT: &str = "nbspec-default";

/// Schema file name within a named schema's directory.
pub const SCHEMA_FILE: &str = "schema.toml";

const SCHEMA_TOML_DEFAULT: &str = include_str!("schemata/default.toml");

static SCHEMA_DEFAULT: LazyLock<WorkflowSchema> =
    LazyLock::new(|| parse_schema(SCHEMA_TOML_DEFAULT).expect("embedded default schema is valid"));

/// Errors from schema parsing and resolution.
#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("schema not found: {0}")]
    NotFound(String),

    #[error("schema parse failure: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("schema invalid: {0}")]
    Invalid(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A workflow schema: the artifact set and authoring order for changes.
#[derive(Clone, Debug, Deserialize)]
pub struct WorkflowSchema {
    /// Schema name, referenced by meta notes and project configuration.
    pub name: String,
    /// Schema format version.
    pub version: u32,
    /// Human-readable summary.
    #[serde(default)]
    pub description: String,
    /// Artifacts in declaration order.
    pub artifacts: Vec<Artifact>,
}

/// One artifact declared by a workflow schema.
#[derive(Clone, Debug, Deserialize)]
pub struct Artifact {
    /// Artifact identifier, referenced by `requires` lists.
    pub id: String,
    /// Rendered path (or glob) within a rendered change tree.
    pub generates: String,
    /// Human-readable summary.
    #[serde(default)]
    pub description: String,
    /// Authoring guidance surfaced to agents.
    #[serde(default)]
    pub instruction: Option<String>,
    /// Template file name, when the schema ships one.
    #[serde(default)]
    pub template: Option<String>,
    /// Artifact ids that must be authored first.
    #[serde(default)]
    pub requires: Vec<String>,
    /// nbspec extension: repository directory receiving this artifact's
    /// documents at merge. `None` means the artifact never merges.
    #[serde(default)]
    pub target: Option<String>,
}

impl WorkflowSchema {
    /// Returns the artifact with the given id.
    pub fn artifact(&self, id: &str) -> Option<&Artifact> {
        self.artifacts.iter().find(|artifact| artifact.id == id)
    }

    /// Returns artifacts in a dependency-respecting authoring order:
    /// every artifact appears after all artifacts it requires, with
    /// declaration order preserved among peers.
    pub fn authoring_order(&self) -> Vec<&Artifact> {
        let mut ordered = Vec::with_capacity(self.artifacts.len());
        let mut placed: HashSet<&str> = HashSet::new();
        while ordered.len() < self.artifacts.len() {
            let mut progressed = false;
            for artifact in &self.artifacts {
                if placed.contains(artifact.id.as_str()) {
                    continue;
                }
                if artifact
                    .requires
                    .iter()
                    .all(|dependency| placed.contains(dependency.as_str()))
                {
                    placed.insert(artifact.id.as_str());
                    ordered.push(artifact);
                    progressed = true;
                }
            }
            debug_assert!(progressed, "validated schemas have no cycles");
            if !progressed {
                break;
            }
        }
        ordered
    }
}

/// Parses and validates a workflow schema from TOML content.
///
/// # Errors
///
/// Returns [`SchemaError::Parse`] for malformed TOML and
/// [`SchemaError::Invalid`] for duplicate artifact ids, references to
/// unknown artifacts, or dependency cycles.
pub fn parse_schema(content: &str) -> Result<WorkflowSchema, SchemaError> {
    let schema: WorkflowSchema = toml::from_str(content)?;
    validate_schema(&schema)?;
    Ok(schema)
}

/// Returns the embedded nbspec default schema.
pub fn default_schema() -> WorkflowSchema {
    SCHEMA_DEFAULT.clone()
}

/// Resolves the workflow schema for a change.
///
/// Resolution order: `explicit` (from the change's meta note), then the
/// configuration's `schema` setting, then the embedded default.
///
/// # Errors
///
/// Returns [`SchemaError::NotFound`] when a named schema has no
/// `schema.toml` under the configuration's schemata directory, and
/// parse or validation errors for schemas that load but do not
/// conform.
pub fn resolve_schema(
    explicit: Option<&str>,
    configuration: &Configuration,
) -> Result<WorkflowSchema, SchemaError> {
    let name = explicit
        .or(configuration.schema.as_deref())
        .unwrap_or(SCHEMA_NAME_DEFAULT);
    if name == SCHEMA_NAME_DEFAULT {
        return Ok(default_schema());
    }
    let path = configuration
        .project_directory
        .join("schemata")
        .join(name)
        .join(SCHEMA_FILE);
    if !path.is_file() {
        return Err(SchemaError::NotFound(name.to_string()));
    }
    parse_schema(&std::fs::read_to_string(&path)?)
}

fn validate_schema(schema: &WorkflowSchema) -> Result<(), SchemaError> {
    let mut indices: HashMap<&str, usize> = HashMap::new();
    for (index, artifact) in schema.artifacts.iter().enumerate() {
        if indices.insert(artifact.id.as_str(), index).is_some() {
            return Err(SchemaError::Invalid(format!(
                "duplicate artifact id: {}",
                artifact.id
            )));
        }
    }
    for artifact in &schema.artifacts {
        validate_artifact_path(&artifact.id, "generates", &artifact.generates)?;
        if let Some(target) = &artifact.target {
            validate_artifact_path(&artifact.id, "target", target)?;
        }
        for dependency in &artifact.requires {
            if !indices.contains_key(dependency.as_str()) {
                return Err(SchemaError::Invalid(format!(
                    "artifact {} requires unknown artifact {}",
                    artifact.id, dependency
                )));
            }
        }
    }
    detect_cycles(schema, &indices)
}

/// Validates a schema-declared path. `generates` paths anchor the
/// rendered scratch tree and `target` paths anchor repository writes,
/// so both must stay inside their roots: relative, no parent-directory
/// components, no backslashes or drive prefixes. Rejecting these at
/// schema validation time keeps render confined to scratch
/// destinations and merge confined to the repository.
fn validate_artifact_path(artifact_id: &str, field: &str, path: &str) -> Result<(), SchemaError> {
    let invalid = |detail: &str| {
        Err(SchemaError::Invalid(format!(
            "artifact {artifact_id} {field} path {path:?} {detail}"
        )))
    };
    if path.is_empty() {
        return invalid("is empty");
    }
    if path.contains('\\') {
        return invalid("contains a backslash");
    }
    let parsed = std::path::Path::new(path);
    if parsed.is_absolute() || path.starts_with('/') {
        return invalid("is absolute; paths must stay inside their root");
    }
    for component in parsed.components() {
        match component {
            std::path::Component::Normal(_) => {}
            std::path::Component::ParentDir => {
                return invalid("contains a parent-directory component");
            }
            std::path::Component::CurDir => {
                return invalid("contains a current-directory component");
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return invalid("is absolute; paths must stay inside their root");
            }
        }
    }
    // Reject Windows drive prefixes even when parsing on Unix, where
    // "c:" is a normal component.
    if path.split('/').any(|segment| segment.contains(':')) {
        return invalid("contains a drive or scheme prefix");
    }
    Ok(())
}

fn detect_cycles(
    schema: &WorkflowSchema,
    indices: &HashMap<&str, usize>,
) -> Result<(), SchemaError> {
    // Kahn's algorithm: any node left unprocessed participates in a cycle.
    let count = schema.artifacts.len();
    let mut in_degree = vec![0usize; count];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (index, artifact) in schema.artifacts.iter().enumerate() {
        for dependency in &artifact.requires {
            let dependency_index = indices[dependency.as_str()];
            in_degree[index] += 1;
            dependents[dependency_index].push(index);
        }
    }
    let mut queue: Vec<usize> = (0..count).filter(|&index| in_degree[index] == 0).collect();
    let mut processed = 0;
    while let Some(index) = queue.pop() {
        processed += 1;
        for &dependent in &dependents[index] {
            in_degree[dependent] -= 1;
            if in_degree[dependent] == 0 {
                queue.push(dependent);
            }
        }
    }
    if processed != count {
        let cyclic: Vec<&str> = schema
            .artifacts
            .iter()
            .enumerate()
            .filter(|(index, _)| in_degree[*index] > 0)
            .map(|(_, artifact)| artifact.id.as_str())
            .collect();
        return Err(SchemaError::Invalid(format!(
            "dependency cycle among artifacts: {}",
            cyclic.join(", ")
        )));
    }
    Ok(())
}
