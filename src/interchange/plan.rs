use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::worknotes::WorkNoteError;

/// A typed normalization applied when comparing the export output
/// against the source tree under the round-trip proof. The list is
/// enumerated explicitly so the proof output names every category
/// it elided; the proof never silently ignores divergence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoundTripNormalization {
    /// H1 normalization: filesystem sources may carry arbitrary H1
    /// text while the notebook model insists on selector-stable
    /// titles (per the schema's `generates` convention).
    H1Rewrite,
    /// Provenance-header normalization: durable-document writes
    /// carry provenance headers that the interchange artifact omits.
    ProvenanceHeader,
    /// Meta-note synthesis: the notebook adds a `meta` JSON
    /// control-plane note that has no filesystem analog.
    MetaSynthesis,
    /// Decision-slot reconstruction: filenames are preserved
    /// through import/export, so the proof compares the round-tripped
    /// content rather than treating the slot as opaque.
    DecisionSlotReconstruction,
}

/// Result of the round-trip proof: whether the export output is
/// equivalent to the source tree modulo the typed normalizations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundTripProof {
    /// Whether the proof is clean (the export matches the source
    /// modulo the enumerated normalizations).
    pub clean: bool,
    /// Files where the proof diverged beyond the typed
    /// normalizations (only populated when `clean` is `false`).
    pub divergences: Vec<String>,
}

/// One ingestion entry in the unified plan output. The CLI and MCP
/// surfaces consume this so the same structured payload describes
/// both ingestions and refusals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanEntry {
    /// Active change tree detected under `<root>/<change-id>/`. In
    /// v0.3.0 the execute arm is a no-op: the source filesystem tree
    /// is left untouched and no notebook mutation occurs. Active
    /// filesystem-tree ingest is paused in v0.3.0 pending an NbApi
    /// 0.3 notebook transaction/checkpoint primitive with Git/index
    /// locking; the pre-pause `import_active_tree` body is preserved
    /// in source under `#[cfg(any())]` and in git history, restored
    /// by the 0.3.0-resume cycle once NbApi 0.3 ships.
    ActiveWrite {
        change_id: String,
        source_path: PathBuf,
    },
    /// Legacy archive tree → deterministic tar.zst under
    /// `documentation/archives/`.
    ArchiveWrite {
        change_id: String,
        source_path: PathBuf,
        target_path: PathBuf,
    },
    /// Refusal: the tree is detected but cannot be ingested as-is
    /// (parse failure, change-id collision, layout anomaly). All
    /// refusals are collected before any write.
    Refusal {
        source_path: PathBuf,
        message: String,
    },
    /// Skip: the tree was eligible but the operator narrowed the
    /// scope with `--no-active` or `--no-archives`.
    Skip {
        source_path: PathBuf,
        reason: String,
    },
}

/// The unified plan output for both `import` and `export`. The plan
/// is emitted in full by `--dry-run` before any write or delete;
/// without `--dry-run`, the plan is built first, refusals abort, and
/// only the non-refused entries execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterchangePlan {
    /// Resolved target notebook for the plan.
    pub notebook: String,
    /// Per-tree plan entries in detection order.
    pub entries: Vec<PlanEntry>,
    /// Whether `--delete-original` authorizes deletion of the source
    /// filesystem tree after a clean round-trip proof.
    pub delete_original: bool,
    /// Paths that would be deleted under `--delete-original` (only
    /// populated when the flag is set).
    pub pending_deletions: Vec<PathBuf>,
    /// Whether this plan is dry-run only (no writes, no deletions).
    pub dry_run: bool,
}

/// Structured payload for the interchange plan. The CLI and MCP
/// surfaces both consume this so agents can branch on typed data.
#[derive(Clone, Debug)]
pub struct InterchangePlanStructured {
    pub plan: InterchangePlan,
    pub structured: Value,
}

impl InterchangePlanStructured {
    /// Builds the structured JSON payload from a plan.
    pub fn from_plan(plan: InterchangePlan) -> Self {
        let mut entries: Vec<Value> = Vec::with_capacity(plan.entries.len());
        for entry in &plan.entries {
            let mut object = Map::new();
            match entry {
                PlanEntry::ActiveWrite {
                    change_id,
                    source_path,
                } => {
                    // v0.3.0-pause: the active write is detected but
                    // its notebook mutation is paused pending the
                    // NbApi 0.3 notebook transaction/checkpoint
                    // primitive. Surface the pause state and the
                    // prerequisite in structured output so agents
                    // can branch on the typed payload.
                    let prerequisite = "NbApi 0.3 notebook transaction/checkpoint primitive with \
                                         Git/index locking";
                    object.insert("kind".to_string(), Value::String("active-write".into()));
                    object.insert("status".to_string(), Value::String("paused".into()));
                    object.insert("change_id".to_string(), Value::String(change_id.clone()));
                    object.insert(
                        "source_path".to_string(),
                        Value::String(source_path.display().to_string()),
                    );
                    object.insert(
                        "prerequisite".to_string(),
                        Value::String(prerequisite.into()),
                    );
                }
                PlanEntry::ArchiveWrite {
                    change_id,
                    source_path,
                    target_path,
                } => {
                    object.insert("kind".to_string(), Value::String("archive-write".into()));
                    object.insert("change_id".to_string(), Value::String(change_id.clone()));
                    object.insert(
                        "source_path".to_string(),
                        Value::String(source_path.display().to_string()),
                    );
                    object.insert(
                        "target_path".to_string(),
                        Value::String(target_path.display().to_string()),
                    );
                }
                PlanEntry::Refusal {
                    source_path,
                    message,
                } => {
                    object.insert("kind".to_string(), Value::String("refusal".into()));
                    object.insert(
                        "source_path".to_string(),
                        Value::String(source_path.display().to_string()),
                    );
                    object.insert("message".to_string(), Value::String(message.clone()));
                }
                PlanEntry::Skip {
                    source_path,
                    reason,
                } => {
                    object.insert("kind".to_string(), Value::String("skip".into()));
                    object.insert(
                        "source_path".to_string(),
                        Value::String(source_path.display().to_string()),
                    );
                    object.insert("reason".to_string(), Value::String(reason.clone()));
                }
            }
            entries.push(Value::Object(object));
        }
        let pending_deletions: Vec<Value> = plan
            .pending_deletions
            .iter()
            .map(|path| Value::String(path.display().to_string()))
            .collect();
        let structured = json!({
            "notebook": plan.notebook,
            "entries": entries,
            "delete_original": plan.delete_original,
            "pending_deletions": pending_deletions,
            "dry_run": plan.dry_run,
        });
        Self { plan, structured }
    }
}

/// Renders the plan as a human-readable text block for the CLI's
/// `text` field. The MCP structured payload is produced by
/// `InterchangePlanStructured::from_plan`.
pub fn render_plan_text(plan: &InterchangePlan, root: &Path) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "Plan for notebook {} from root {}:\n",
        plan.notebook,
        root.display()
    ));
    for entry in &plan.entries {
        match entry {
            PlanEntry::ActiveWrite {
                change_id,
                source_path,
            } => {
                // v0.3.0-pause: render the pause state in the human-
                // readable plan output. The execute arm is a no-op;
                // the source filesystem tree is left untouched.
                output.push_str(&format!(
                    "  paused: active {change_id} ({source}) — \
                     v0.3.0 active ingest is no-op pending the NbApi 0.3 notebook \
                     transaction/checkpoint primitive; source is not modified\n",
                    change_id = change_id,
                    source = source_path.display()
                ));
            }
            PlanEntry::ArchiveWrite {
                change_id,
                source_path,
                target_path,
            } => {
                output.push_str(&format!(
                    "  archive-write: {change_id} ({source} -> {target})\n",
                    change_id = change_id,
                    source = source_path.display(),
                    target = target_path.display()
                ));
            }
            PlanEntry::Refusal {
                source_path,
                message,
            } => {
                output.push_str(&format!(
                    "  refusal: {source}: {message}\n",
                    source = source_path.display(),
                    message = message
                ));
            }
            PlanEntry::Skip {
                source_path,
                reason,
            } => {
                output.push_str(&format!(
                    "  skip: {source}: {reason}\n",
                    source = source_path.display(),
                    reason = reason
                ));
            }
        }
    }
    if !plan.pending_deletions.is_empty() {
        output.push_str("pending deletions:\n");
        for path in &plan.pending_deletions {
            output.push_str(&format!("  - {}\n", path.display()));
        }
    }
    output
}

/// One file write in an export plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportEntry {
    /// Logical path under `<target>/<change-id>/` (forward slashes).
    pub tree_path: String,
    /// On-disk path the write will land at.
    pub absolute_path: PathBuf,
    /// Note content to write.
    pub content: String,
}

#[allow(dead_code)] // used by future variants (e.g. Refusal / Skip for export)
impl ExportEntry {
    /// Convenience constructor for a write entry.
    pub fn write(
        tree_path: impl Into<String>,
        absolute_path: PathBuf,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tree_path: tree_path.into(),
            absolute_path,
            content: content.into(),
        }
    }
}

/// The export plan: per-file writes plus whether the target
/// already exists on disk (refused without `--overwrite`). The
/// plan is built once and the same `tree_path` set is used for
/// both the dry-run output and the actual writes, so the two
/// outputs never disagree.
///
/// `overwrite_authorized` records the operator's `--overwrite`
/// authorization so the execute path can re-validate the
/// destination state without re-reading CLI flags. Without this
/// field, the execute path would have no way to distinguish
/// "operator authorized an overwrite" from "no overwrite flag,
/// and the directory happened to not exist at plan time but
/// appeared at execute time" (the R7 / F7 TOCTOU window).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPlan {
    pub change_id: String,
    pub notebook: String,
    pub target: PathBuf,
    pub entries: Vec<ExportEntry>,
    pub would_overwrite: bool,
    pub overwrite_authorized: bool,
    pub dry_run: bool,
}

/// Options for `nbspec import`. Carries every flag the verb accepts
/// so the eventual implementation has a single, typed entry point.
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// Emit the plan only; do not write notes or archives and do
    /// not delete the source filesystem tree.
    pub dry_run: bool,
    /// Authorize deletion of the source filesystem tree after a
    /// clean round-trip proof.
    pub delete_original: bool,
    /// Skip active change tree detection (default: detect both).
    /// In v0.3.0 the active execute arm is a no-op; this flag
    /// emits a `Skip` entry instead of a paused `ActiveWrite`
    /// entry for each detected active tree.
    pub no_active: bool,
    /// Skip legacy archive tree ingestion (default: ingest both).
    pub no_archives: bool,
}

/// Options for `nbspec export`. Carries every flag the verb accepts.
#[derive(Clone, Debug, Default)]
pub struct ExportOptions {
    /// Emit the plan only; do not write the filesystem tree.
    pub dry_run: bool,
    /// Overwrite an existing `<target>/<change-id>/` filesystem
    /// tree without refusing.
    pub overwrite: bool,
}

/// Errors from interchange operations.
#[derive(Debug, Error)]
pub enum InterchangeError {
    #[error("cannot read source file {path}: {source}")]
    SourceRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("source tasks.md parse failure: {0}")]
    TasksParse(#[from] WorkNoteError),

    #[error(
        "change-id collision: notebook namespace proposals/{0}/ already exists; \
         refusing to overwrite without explicit operator intervention"
    )]
    Collision(String),

    #[error(
        "refusal: target filesystem tree {target}/{change_id}/ already exists; \
         pass --overwrite to replace"
    )]
    OverwriteRefused { change_id: String, target: String },

    #[error("refusal: import aborted by {count} refusal(s); see plan output for details")]
    ImportRefused { count: usize },

    #[error("invalid change id {0:?}: expected kebab-case (lowercase alphanumerics and hyphens)")]
    InvalidChangeId(String),

    #[error("notebook read failure for {selector} in notebook {notebook}: {message}")]
    NotebookRead {
        notebook: String,
        selector: String,
        message: String,
    },

    #[error(
        "required notebook note absent: {selector} (in notebook {notebook}); \
         refusing to export a partial change"
    )]
    RequiredNoteAbsent { notebook: String, selector: String },

    #[error("filesystem layout anomaly at {path}: {message}")]
    LayoutAnomaly { path: PathBuf, message: String },

    #[error("notebook listing failure for {folder} in notebook {notebook}: {message}")]
    ListingFailed {
        notebook: String,
        folder: String,
        message: String,
    },

    #[error("archive fidelity proof failed for {change_id}: {} divergence(s)", divergences.len())]
    ArchiveFidelity {
        change_id: String,
        divergences: Vec<String>,
    },
}

impl From<InterchangeError> for crate::operations::OperationError {
    fn from(error: InterchangeError) -> Self {
        match error {
            InterchangeError::SourceRead { path, source } => {
                crate::operations::OperationError::NoteRead { path, source }
            }
            InterchangeError::TasksParse(error) => {
                crate::operations::OperationError::WorkNote(error)
            }
            InterchangeError::Collision(change_id) => {
                crate::operations::OperationError::AlreadyExists(change_id)
            }
            InterchangeError::OverwriteRefused { .. } => {
                crate::operations::OperationError::NoteRead {
                    path: PathBuf::from("<export-refusal>"),
                    source: std::io::Error::other(error.to_string()),
                }
            }
            InterchangeError::ImportRefused { .. } => crate::operations::OperationError::NoteRead {
                path: PathBuf::from("<import-refusal>"),
                source: std::io::Error::other(error.to_string()),
            },
            InterchangeError::InvalidChangeId(_)
            | InterchangeError::NotebookRead { .. }
            | InterchangeError::RequiredNoteAbsent { .. }
            | InterchangeError::LayoutAnomaly { .. }
            | InterchangeError::ListingFailed { .. }
            | InterchangeError::ArchiveFidelity { .. } => {
                crate::operations::OperationError::NoteRead {
                    path: PathBuf::from("<interchange-refusal>"),
                    source: std::io::Error::other(error.to_string()),
                }
            }
        }
    }
}

/// One note write generated by an active-tree import. Imports
/// materialise these via the `nb-api` client during the execute
/// phase; they are also the unit tested by the active-tree
/// mapping tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteWrite {
    /// Notebook-relative folder under `proposals/<change-id>/`,
    /// mirroring the source layout (`specifications/`, `designs/`,
    /// `decisions/`, or empty for root-level notes).
    pub folder: String,
    /// Note title (also the on-disk filename stem, since `nb`
    /// derives filenames from titles).
    pub title: String,
    /// Note content.
    pub content: String,
    /// Whether to write as a `nb` todo note rather than a plain
    /// note (the `work` checklist reconstruction).
    pub is_todo: bool,
}

/// Resolves the effective notebook for an interchange verb.
pub fn resolve_notebook(
    notebook: Option<&str>,
) -> Result<String, crate::operations::OperationError> {
    notebook
        .map(String::from)
        .or_else(nb_api::derive_git_notebook_name)
        .ok_or(crate::operations::OperationError::NotebookUnresolved)
}

/// Warning surfaced by the import plan: a delta-spec section that
/// the merge path will refuse to materialise (slice 1 only adds
/// new documents). Surfaced at import time so the operator can
/// intervene before merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeltaWarning {
    /// Notebook-relative note path carrying the offending delta.
    pub note_path: String,
    /// Whether the offending section is `MODIFIED`, `REMOVED`, or
    /// `RENAMED`. All three bind at merge time, not at import time.
    pub kind: DeltaWarningKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeltaWarningKind {
    Modified,
    Removed,
    Renamed,
}
