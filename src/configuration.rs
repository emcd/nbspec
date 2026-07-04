//! Layered nbspec configuration.
//!
//! Settings are TOML and layer, lowest to highest precedence: embedded
//! defaults, the user-global settings file (`general.toml` under the
//! platform configuration directory, e.g. `~/.config/nbspec/` on XDG
//! systems), and the per-project settings file
//! (`.auxiliary/configuration/nbspec/general.toml` by default). The
//! per-project directory is relocatable via the `NBSPEC_CONFIG_DIR`
//! environment variable or the user-global
//! `project_configuration_directory` setting; relative paths resolve
//! against the project root. Workflow schemata live beside the project
//! settings under `schemata/<name>/schema.toml`. Missing files yield
//! defaults; unknown keys are ignored for forward compatibility.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Default repository-relative directory holding nbspec configuration.
pub const DEFAULT_PROJECT_CONFIGURATION_DIR: &str = ".auxiliary/configuration/nbspec";

/// Environment variable relocating the per-project configuration
/// directory.
pub const CONFIGURATION_DIR_ENV: &str = "NBSPEC_CONFIG_DIR";

/// Settings file name at every layer.
pub const SETTINGS_FILE: &str = "general.toml";

/// Errors from configuration loading.
#[derive(Debug, Error)]
pub enum ConfigurationError {
    #[error("configuration parse failure: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Raw contents of one `general.toml` settings file.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SettingsDocument {
    /// Workflow schema for changes that do not name one in their meta
    /// note.
    #[serde(default)]
    pub schema: Option<String>,

    /// Per-project configuration directory. Meaningful in the
    /// user-global file; relative paths resolve against the project
    /// root.
    #[serde(default)]
    pub project_configuration_directory: Option<PathBuf>,
}

/// Resolved configuration after layering.
#[derive(Clone, Debug)]
pub struct Configuration {
    /// Workflow schema for changes that do not name one. `None`
    /// selects the embedded default schema.
    pub schema: Option<String>,

    /// Project configuration directory holding the settings file and
    /// the `schemata/` subdirectory.
    pub project_directory: PathBuf,
}

/// Loads layered configuration for a project: the user-global settings
/// file, the `NBSPEC_CONFIG_DIR` environment override, and the
/// per-project settings file.
///
/// # Errors
///
/// Returns [`ConfigurationError::Parse`] for malformed TOML and
/// [`ConfigurationError::Io`] for unreadable files.
pub fn load_configuration(project_root: &Path) -> Result<Configuration, ConfigurationError> {
    let global = match global_settings_path() {
        Some(path) => load_settings_document(&path)?,
        None => SettingsDocument::default(),
    };
    let environment_directory = std::env::var_os(CONFIGURATION_DIR_ENV).map(PathBuf::from);
    resolve_configuration(project_root, global, environment_directory)
}

/// Layers configuration sources into a resolved [`Configuration`].
/// Exposed so tests can inject the user-global document and
/// environment override explicitly.
///
/// # Errors
///
/// Returns [`ConfigurationError::Parse`] for malformed TOML and
/// [`ConfigurationError::Io`] for unreadable files.
pub fn resolve_configuration(
    project_root: &Path,
    global: SettingsDocument,
    environment_directory: Option<PathBuf>,
) -> Result<Configuration, ConfigurationError> {
    let directory = environment_directory
        .or_else(|| global.project_configuration_directory.clone())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PROJECT_CONFIGURATION_DIR));
    let directory = if directory.is_absolute() {
        directory
    } else {
        project_root.join(directory)
    };
    let project = load_settings_document(&directory.join(SETTINGS_FILE))?;
    Ok(Configuration {
        schema: project.schema.or(global.schema),
        project_directory: directory,
    })
}

fn global_settings_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "nbspec")
        .map(|dirs| dirs.config_dir().join(SETTINGS_FILE))
}

fn load_settings_document(path: &Path) -> Result<SettingsDocument, ConfigurationError> {
    if !path.is_file() {
        return Ok(SettingsDocument::default());
    }
    Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
}
