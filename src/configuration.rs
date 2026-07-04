//! Project configuration for nbspec.
//!
//! Configuration lives under `.auxiliary/configuration/nbspec/` in the
//! project repository: `config.yaml` for settings and `schemata/<name>/`
//! for project-local workflow schemas. A missing configuration file
//! yields the default configuration; unknown fields are ignored for
//! forward compatibility.

use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

/// Repository-relative directory holding nbspec configuration.
pub const CONFIGURATION_DIR: &str = ".auxiliary/configuration/nbspec";

/// Errors from configuration loading.
#[derive(Debug, Error)]
pub enum ConfigurationError {
    #[error("configuration parse failure: {0}")]
    Parse(#[from] serde_norway::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Project-level nbspec settings.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProjectConfiguration {
    /// Workflow schema for changes that do not name one in their meta
    /// note. `None` selects the embedded default schema.
    #[serde(default)]
    pub schema: Option<String>,
}

/// Loads project configuration from `config.yaml` under the project's
/// configuration directory, returning defaults when the file is absent.
///
/// # Errors
///
/// Returns [`ConfigurationError::Parse`] for malformed YAML and
/// [`ConfigurationError::Io`] for unreadable files.
pub fn load_configuration(project_root: &Path) -> Result<ProjectConfiguration, ConfigurationError> {
    let path = project_root.join(CONFIGURATION_DIR).join("config.yaml");
    if !path.is_file() {
        return Ok(ProjectConfiguration::default());
    }
    Ok(serde_norway::from_str(&std::fs::read_to_string(&path)?)?)
}
