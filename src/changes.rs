//! Change namespace conventions and the meta note control plane.
//!
//! Each change lives in its project notebook under a deterministic
//! folder namespace `proposals/<change-id>/` containing a `proposal`
//! note, a `meta` control-plane note, a `work` todo note, and one
//! subfolder per schema artifact whose `generates` path is nested
//! (for the default schema: `specifications/`, `designs/`, and
//! `decisions/`). The meta note holds a JSON object recording the
//! change's identity, status lifecycle, schema selection, and the
//! repository commits produced by repository-writing transitions.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::schemata::{Artifact, WorkflowSchema};

/// Top-level notebook folder holding all changes.
pub const PROPOSALS_FOLDER: &str = "proposals";

/// Note name of the per-change proposal document.
pub const PROPOSAL_NOTE: &str = "proposal";

/// Note name of the per-change JSON control plane.
pub const META_NOTE: &str = "meta";

/// Note name of the per-change todo checklist.
pub const WORK_NOTE: &str = "work";

/// Version of the meta note format written by this crate.
pub const META_FORMAT_VERSION: u32 = 1;

/// Errors from namespace and meta note handling.
#[derive(Debug, Error)]
pub enum ChangeError {
    #[error("invalid change id: {0} (expected kebab-case: lowercase alphanumerics and hyphens)")]
    InvalidChangeId(String),

    #[error("invalid status transition: {0} -> {1}")]
    InvalidTransition(ChangeStatus, ChangeStatus),

    #[error("meta note parse failure: {0}")]
    MetaParse(String),

    #[error("meta note serialization failure: {0}")]
    Json(#[from] serde_json::Error),
}

/// Validates a change id: kebab-case lowercase alphanumerics.
///
/// # Errors
///
/// Returns [`ChangeError::InvalidChangeId`] for empty ids, uppercase or
/// non-alphanumeric characters, and leading, trailing, or doubled
/// hyphens.
pub fn validate_change_id(change_id: &str) -> Result<(), ChangeError> {
    let valid = !change_id.is_empty()
        && !change_id.starts_with('-')
        && !change_id.ends_with('-')
        && !change_id.contains("--")
        && change_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(ChangeError::InvalidChangeId(change_id.to_string()))
    }
}

/// Returns the notebook folder holding a change: `proposals/<change-id>`.
pub fn change_folder(change_id: &str) -> String {
    format!("{PROPOSALS_FOLDER}/{change_id}")
}

/// Reports whether note content goes beyond its title heading and
/// scaffold placeholder comment — i.e. whether an artifact note has
/// been authored since `nbspec create` scaffolded it.
pub fn note_has_authored_content(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        let scaffold = trimmed.is_empty()
            || trimmed.starts_with('#')
            || (trimmed.starts_with("<!--") && trimmed.ends_with("-->"));
        !scaffold
    })
}

/// Notebook layout of one schema artifact within a change namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactLayout {
    /// A single note at the change root, named by the `generates` file
    /// stem (for example `proposal.md` -> note `proposal`).
    Note(String),
    /// A subfolder of the change namespace holding one note per
    /// document, named by the first `generates` path segment (for
    /// example `specifications/**/*.md` -> folder `specifications`).
    Folder(String),
}

/// Derives an artifact's notebook layout from its `generates` path.
pub fn artifact_layout(artifact: &Artifact) -> ArtifactLayout {
    match artifact.generates.split_once('/') {
        Some((folder, _)) => ArtifactLayout::Folder(folder.to_string()),
        None => {
            let stem = artifact
                .generates
                .strip_suffix(".md")
                .unwrap_or(&artifact.generates);
            ArtifactLayout::Note(stem.to_string())
        }
    }
}

/// Returns the subfolder names a schema requires within a change
/// namespace, in artifact declaration order.
pub fn namespace_folders(schema: &WorkflowSchema) -> Vec<String> {
    schema
        .artifacts
        .iter()
        .filter_map(|artifact| match artifact_layout(artifact) {
            ArtifactLayout::Folder(name) => Some(name),
            ArtifactLayout::Note(_) => None,
        })
        .collect()
}

/// Returns the note names a schema requires at the change root, in
/// artifact declaration order.
pub fn namespace_notes(schema: &WorkflowSchema) -> Vec<String> {
    schema
        .artifacts
        .iter()
        .filter_map(|artifact| match artifact_layout(artifact) {
            ArtifactLayout::Note(name) => Some(name),
            ArtifactLayout::Folder(_) => None,
        })
        .collect()
}

/// Change lifecycle status.
///
/// The main progression is `draft -> approved -> implemented ->
/// archived`, one step at a time. Any main state may move to the side
/// states `blocked`, `superseded`, or `abandoned`; `blocked` may return
/// to any main state. `archived`, `superseded`, and `abandoned` are
/// terminal.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeStatus {
    Draft,
    Approved,
    Implemented,
    Archived,
    Blocked,
    Superseded,
    Abandoned,
}

impl ChangeStatus {
    /// Reports whether the lifecycle permits moving to `next`.
    pub fn permits_transition(self, next: ChangeStatus) -> bool {
        use ChangeStatus::*;
        if self == next {
            return false;
        }
        match self {
            Draft => matches!(next, Approved | Blocked | Superseded | Abandoned),
            Approved => matches!(next, Implemented | Blocked | Superseded | Abandoned),
            Implemented => matches!(next, Archived | Blocked | Superseded | Abandoned),
            Blocked => matches!(next, Draft | Approved | Implemented | Archived),
            Archived | Superseded | Abandoned => false,
        }
    }
}

impl std::fmt::Display for ChangeStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ChangeStatus::Draft => "draft",
            ChangeStatus::Approved => "approved",
            ChangeStatus::Implemented => "implemented",
            ChangeStatus::Archived => "archived",
            ChangeStatus::Blocked => "blocked",
            ChangeStatus::Superseded => "superseded",
            ChangeStatus::Abandoned => "abandoned",
        };
        formatter.write_str(name)
    }
}

/// A repository commit recorded at a repository-writing transition.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RepositoryCommit {
    /// Status current when the commit was recorded.
    pub status: ChangeStatus,
    /// Commit SHA in the project repository.
    pub commit: String,
    /// Recording time.
    pub recorded_at: Timestamp,
}

/// The JSON control plane stored in a change's `meta` note.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ChangeMetadata {
    /// Meta note format version.
    pub meta_version: u32,
    /// Change identifier (the namespace folder name).
    pub change_id: String,
    /// Human-readable title.
    pub title: Option<String>,
    /// Lifecycle status.
    pub status: ChangeStatus,
    /// Workflow schema name resolved for this change.
    pub schema: String,
    /// Source notebook holding the change.
    pub notebook: String,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last update time.
    pub updated_at: Timestamp,
    /// Commits recorded at repository-writing transitions.
    #[serde(default)]
    pub repository_commits: Vec<RepositoryCommit>,
}

impl ChangeMetadata {
    /// Creates metadata for a new draft change stamped with the
    /// current time.
    ///
    /// # Errors
    ///
    /// Returns [`ChangeError::InvalidChangeId`] for non-kebab-case ids.
    pub fn new(
        change_id: &str,
        title: Option<&str>,
        schema: &str,
        notebook: &str,
    ) -> Result<Self, ChangeError> {
        validate_change_id(change_id)?;
        let now = Timestamp::now();
        Ok(Self {
            meta_version: META_FORMAT_VERSION,
            change_id: change_id.to_string(),
            title: title.map(String::from),
            status: ChangeStatus::Draft,
            schema: schema.to_string(),
            notebook: notebook.to_string(),
            created_at: now,
            updated_at: now,
            repository_commits: Vec::new(),
        })
    }

    /// Moves the change to `next` and refreshes `updated_at`.
    ///
    /// # Errors
    ///
    /// Returns [`ChangeError::InvalidTransition`] when the lifecycle
    /// does not permit the move.
    pub fn transition(&mut self, next: ChangeStatus) -> Result<(), ChangeError> {
        if !self.status.permits_transition(next) {
            return Err(ChangeError::InvalidTransition(self.status, next));
        }
        self.status = next;
        self.updated_at = Timestamp::now();
        Ok(())
    }

    /// Records a repository commit produced by a repository-writing
    /// operation and refreshes `updated_at`.
    pub fn record_commit(&mut self, commit: &str) {
        let now = Timestamp::now();
        self.repository_commits.push(RepositoryCommit {
            status: self.status,
            commit: commit.to_string(),
            recorded_at: now,
        });
        self.updated_at = now;
    }
}

/// Renders metadata as meta note content: a fenced JSON block, which
/// survives note titling and renders legibly in notebook tooling.
///
/// # Errors
///
/// Returns [`ChangeError::Json`] when serialization fails.
pub fn render_meta_note(metadata: &ChangeMetadata) -> Result<String, ChangeError> {
    let json = serde_json::to_string_pretty(metadata)?;
    Ok(format!("```json\n{json}\n```\n"))
}

/// Parses metadata from meta note content: either a bare JSON object
/// or the first fenced JSON block.
///
/// # Errors
///
/// Returns [`ChangeError::MetaParse`] when no JSON object is found and
/// [`ChangeError::Json`] when the JSON does not match the meta format.
pub fn parse_meta_note(content: &str) -> Result<ChangeMetadata, ChangeError> {
    let trimmed = content.trim();
    if trimmed.starts_with('{') {
        return Ok(serde_json::from_str(trimmed)?);
    }
    let json = extract_fenced_json(content)
        .ok_or_else(|| ChangeError::MetaParse("no JSON object found in meta note".to_string()))?;
    Ok(serde_json::from_str(json)?)
}

fn extract_fenced_json(content: &str) -> Option<&str> {
    let mut fence_body_start: Option<usize> = None;
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        match fence_body_start {
            None => {
                if let Some(language) = trimmed.strip_prefix("```")
                    && (language.trim().is_empty() || language.trim() == "json")
                {
                    fence_body_start = Some(offset + line.len());
                }
            }
            Some(start) => {
                if trimmed == "```" {
                    return Some(&content[start..offset]);
                }
            }
        }
        offset += line.len();
    }
    None
}
