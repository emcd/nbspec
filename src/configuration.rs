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
pub const PROJECT_CONFIGURATION_DIR_DEFAULT: &str = ".auxiliary/configuration/nbspec";

/// Environment variable relocating the per-project configuration
/// directory.
pub const CONFIGURATION_DIR_ENV: &str = "NBSPEC_CONFIG_DIR";

/// Settings file name at every layer.
pub const SETTINGS_FILE: &str = "general.toml";

/// Default repository-relative directory receiving change archives.
pub const ARCHIVE_DIR_DEFAULT: &str = "documentation/archives";

/// Errors from configuration loading.
#[derive(Debug, Error)]
pub enum ConfigurationError {
    #[error("configuration parse failure: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("configuration invalid: {0}")]
    Invalid(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Reports why a path would escape its confining root, or `None`
/// for confined relative paths. Shared by configuration and schema
/// validation for every path that anchors a repository or scratch
/// write: rejects empty, absolute, parent-directory,
/// current-directory, backslash, and drive-prefixed values.
pub(crate) fn confinement_violation(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return Some("is empty");
    }
    if path.contains('\\') {
        return Some("contains a backslash");
    }
    let parsed = Path::new(path);
    if parsed.is_absolute() || path.starts_with('/') {
        return Some("is absolute; paths must stay inside their root");
    }
    for component in parsed.components() {
        match component {
            std::path::Component::Normal(_) => {}
            std::path::Component::ParentDir => {
                return Some("contains a parent-directory component");
            }
            std::path::Component::CurDir => {
                return Some("contains a current-directory component");
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Some("is absolute; paths must stay inside their root");
            }
        }
    }
    // Reject Windows drive prefixes even when parsing on Unix, where
    // "c:" is a normal component.
    if path.split('/').any(|segment| segment.contains(':')) {
        return Some("contains a drive or scheme prefix");
    }
    None
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

    /// Scratch directory for rendered change trees. Relative paths
    /// resolve against the project root; unset selects the platform
    /// cache directory.
    #[serde(default)]
    pub scratch_directory: Option<PathBuf>,

    /// Whether merge writes a change archive (default: enabled).
    #[serde(default)]
    pub archives: Option<bool>,

    /// Directory receiving merge-time change archives. Relative
    /// paths are repository-relative.
    #[serde(default)]
    pub archive_directory: Option<PathBuf>,
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

    /// Scratch directory for rendered change trees. `None` selects
    /// the platform cache directory.
    pub scratch_directory: Option<PathBuf>,

    /// Whether merge writes a change archive.
    pub archives: bool,

    /// Directory receiving merge-time change archives. Relative
    /// paths are repository-relative.
    pub archive_directory: PathBuf,
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
/// Returns [`ConfigurationError::Parse`] for malformed TOML,
/// [`ConfigurationError::Invalid`] for an `archive_directory` that
/// would escape the repository, and [`ConfigurationError::Io`] for
/// unreadable files.
pub fn resolve_configuration(
    project_root: &Path,
    global: SettingsDocument,
    environment_directory: Option<PathBuf>,
) -> Result<Configuration, ConfigurationError> {
    let directory = environment_directory
        .or_else(|| global.project_configuration_directory.clone())
        .unwrap_or_else(|| PathBuf::from(PROJECT_CONFIGURATION_DIR_DEFAULT));
    let directory = if directory.is_absolute() {
        directory
    } else {
        project_root.join(directory)
    };
    let project = load_settings_document(&directory.join(SETTINGS_FILE))?;
    let scratch_directory = project
        .scratch_directory
        .or(global.scratch_directory)
        .map(|scratch| {
            if scratch.is_absolute() {
                scratch
            } else {
                project_root.join(scratch)
            }
        });
    let archive_directory = project
        .archive_directory
        .or(global.archive_directory)
        .unwrap_or_else(|| PathBuf::from(ARCHIVE_DIR_DEFAULT));
    // Archives are documented as repository-resident (and LFS
    // candidates), so the directory must stay inside the repository —
    // the same confinement schema paths receive. The scratch
    // directory is exempt by design: its default already lives
    // outside the repository, in the platform cache.
    if let Some(detail) = confinement_violation(&archive_directory.to_string_lossy()) {
        return Err(ConfigurationError::Invalid(format!(
            "archive_directory {archive_directory:?} {detail}"
        )));
    }
    Ok(Configuration {
        schema: project.schema.or(global.schema),
        project_directory: directory,
        scratch_directory,
        archives: project.archives.or(global.archives).unwrap_or(true),
        archive_directory,
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
