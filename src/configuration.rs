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
//!
//! Directory resolution is [`approots`]-backed on absolute project
//! roots: the global file comes from the platform tier, the
//! `NBSPEC_CONFIG_DIR` override from a declared environment variable
//! (exact directory, validated like the explicit tier), the file
//! setting from the explicit tier, and the default from the local
//! root composition. Per-value `project.or(global)` merge and archive
//! confinement stay nbspec-owned. Outside a repository the project
//! root can be relative (`.` fallback), where approots refuses
//! relative starts by contract — that edge keeps the legacy lexical
//! behavior verbatim.

use std::path::{Path, PathBuf};

use approots::{
    DirectoryClass, LocalStructure, Provenance, ResolvedRoot, Resolver,
    local_root, platform_root,
};
use serde::Deserialize;
use thiserror::Error;

/// Default repository-relative directory holding nbspec configuration.
pub const PROJECT_CONFIGURATION_DIR_DEFAULT: &str =
    ".auxiliary/configuration/nbspec";

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

    #[error("configuration location failure: {0}")]
    Location(#[from] approots::ResolutionError),
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
            std::path::Component::Normal(_) => {},
            std::path::Component::ParentDir => {
                return Some("contains a parent-directory component");
            },
            std::path::Component::CurDir => {
                return Some("contains a current-directory component");
            },
            std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Some("is absolute; paths must stay inside their root");
            },
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

/// Legacy `directories::ProjectDirs::from("", "", "nbspec")` layout
/// segments beneath the approots platform root. Only Windows nests an
/// extra class word (`config` for settings, `cache` for renders);
/// Linux and macOS address the application directory directly.
/// Preserved verbatim: existing user files must not move silently.
#[cfg(windows)]
const PLATFORM_CONFIG_SUFFIX: &[&str] = &["config", SETTINGS_FILE];
#[cfg(not(windows))]
const PLATFORM_CONFIG_SUFFIX: &[&str] = &[SETTINGS_FILE];

/// Loads layered configuration for a project: the user-global settings
/// file, the `NBSPEC_CONFIG_DIR` environment override, and the
/// per-project settings file.
///
/// The environment override is read through a declared approots
/// variable on absolute project roots: a zero-length value is a hard
/// `EmptyPath` refusal, not silent absence. Relative roots keep the
/// legacy ambient read and silent join. Production always passes the
/// read result into [`resolve_configuration`], which itself never
/// touches ambient environment — injected inputs stay deterministic.
///
/// # Errors
///
/// Returns [`ConfigurationError::Parse`] for malformed TOML,
/// [`ConfigurationError::Location`] for unresolvable locations, and
/// [`ConfigurationError::Io`] for unreadable files.
pub fn load_configuration(
    project_root: &Path,
) -> Result<Configuration, ConfigurationError> {
    let global = match global_settings_path() {
        Some(path) => load_settings_document(&path)?,
        None => SettingsDocument::default(),
    };
    // Production environment lookup lives here, never inside
    // resolve_configuration: the resolver itself stays deterministic
    // given explicit inputs. Relative roots keep the legacy ambient
    // read; absolute roots use the declared variable.
    let environment_directory = if project_root.is_absolute() {
        declared_environment_directory(project_root)?
    } else {
        std::env::var_os(CONFIGURATION_DIR_ENV).map(PathBuf::from)
    };
    resolve_configuration(project_root, global, environment_directory)
}

/// Reads the declared `NBSPEC_CONFIG_DIR` variable through approots.
///
/// An unset variable — or an unusable platform base beneath it, which
/// can only matter when no variable is set — resolves to `None` so
/// the file setting and local default still apply. A set but
/// zero-length value is a hard refusal.
///
/// Only called with absolute project roots; the approots probe needs
/// an absolute start. Relative roots keep the legacy silent join, so
/// the `EmptyPath` hardening applies to absolute starts only — see
/// [`resolve_configuration`].
fn declared_environment_directory(
    project_root: &Path,
) -> Result<Option<PathBuf>, ConfigurationError> {
    let resolver = Resolver::new("nbspec")?
        .env_directory(DirectoryClass::Configuration, CONFIGURATION_DIR_ENV);
    match resolver.resolve(
        DirectoryClass::Configuration,
        None,
        Some(project_root),
    ) {
        Ok(root) if root.provenance == Provenance::AppvarDirectory => {
            Ok(Some(root.path))
        },
        // No declared value: the variable is unset, so the platform
        // tier the probe falls through to is irrelevant here.
        Ok(_) => Ok(None),
        // Unusable platform base with no declared value: same as
        // unset — the local default below needs no platform root.
        Err(approots::ResolutionError::Unavailable(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Layers configuration sources into a resolved [`Configuration`].
/// Exposed so tests can inject the user-global document and
/// environment override explicitly.
///
/// This function never reads ambient environment: production passes
/// the declared-variable read from [`load_configuration`], tests pass
/// explicit values. Precedence is explicit override, then file
/// setting, then the conventional local default.
///
/// # Errors
///
/// Returns [`ConfigurationError::Parse`] for malformed TOML,
/// [`ConfigurationError::Invalid`] for an `archive_directory` that
/// would escape the repository, [`ConfigurationError::Location`] for
/// unresolvable locations, and [`ConfigurationError::Io`] for
/// unreadable files.
pub fn resolve_configuration(
    project_root: &Path,
    global: SettingsDocument,
    environment_directory: Option<PathBuf>,
) -> Result<Configuration, ConfigurationError> {
    // approots refuses relative starts by contract; outside a
    // repository the root can be relative (`.` fallback), where the
    // legacy lexical behavior is preserved verbatim.
    let directory = if project_root.is_absolute() {
        resolve_absolute(project_root, &global, environment_directory)?
    } else {
        resolve_relative_legacy(project_root, &global, environment_directory)?
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
    if let Some(detail) =
        confinement_violation(&archive_directory.to_string_lossy())
    {
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

/// approots-backed project directory resolution for absolute roots.
///
/// Precedence is the explicit override, then the file setting, then
/// the conventional local default. Declared-environment reads happen
/// in [`load_configuration`]; by the time values arrive here they are
/// ordinary explicit inputs, so this function never falls through to
/// the platform tier.
fn resolve_absolute(
    project_root: &Path,
    global: &SettingsDocument,
    environment_directory: Option<PathBuf>,
) -> Result<PathBuf, ConfigurationError> {
    let resolver = Resolver::new("nbspec")?;
    if let Some(directory) = environment_directory {
        return Ok(resolver
            .resolve(
                DirectoryClass::Configuration,
                Some(&directory),
                Some(project_root),
            )?
            .path);
    }
    match &global.project_configuration_directory {
        Some(setting) => Ok(resolver
            .resolve(
                DirectoryClass::Configuration,
                Some(setting),
                Some(project_root),
            )?
            .path),
        None => Ok(local_root(
            &ResolvedRoot {
                path: project_root.to_path_buf(),
                provenance: Provenance::ExplicitBase,
            },
            DirectoryClass::Configuration,
            "nbspec",
            &LocalStructure::default(),
        )?
        .path),
    }
}

/// Legacy lexical project directory resolution for relative roots,
/// preserved verbatim: approots refuses relative starts by contract.
///
/// Takes the environment override as an ordinary explicit input —
/// ambient reads happen in [`load_configuration`], never here.
fn resolve_relative_legacy(
    project_root: &Path,
    global: &SettingsDocument,
    environment_directory: Option<PathBuf>,
) -> Result<PathBuf, ConfigurationError> {
    let directory = environment_directory
        .or_else(|| global.project_configuration_directory.clone())
        .unwrap_or_else(|| PathBuf::from(PROJECT_CONFIGURATION_DIR_DEFAULT));
    Ok(if directory.is_absolute() {
        directory
    } else {
        project_root.join(directory)
    })
}

fn global_settings_path() -> Option<PathBuf> {
    platform_root("nbspec", DirectoryClass::Configuration)
        .ok()?
        .join(PLATFORM_CONFIG_SUFFIX)
        .ok()
        .map(|root| root.path)
}

fn load_settings_document(
    path: &Path,
) -> Result<SettingsDocument, ConfigurationError> {
    if !path.is_file() {
        return Ok(SettingsDocument::default());
    }
    Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
}
