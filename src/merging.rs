//! Durable artifact merge with provenance and drift protection.
//!
//! Merge is the only nbspec operation that writes to the repository
//! working tree, and it never creates git commits. It runs in two
//! phases: planning inspects every merge target and collects every
//! refusal — unsupported delta operations, hand-edited targets,
//! unmanaged files, documents owned by other changes — and only a
//! violation-free plan executes, so a refused merge writes nothing.
//! `--force` overrides target-state refusals (drift, unmanaged,
//! foreign ownership) but never unsupported delta operations, which
//! no overwrite can make correct, and never non-file occupants,
//! which nbspec will not remove.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::grammar::parse_delta_specification;
use crate::provenance;
use crate::rendering::RenderedDocument;

/// Errors from merging.
#[derive(Debug, Error)]
pub enum MergeError {
    #[error("IO failure at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("merge refused; no files were written:\n{}", format_refusals(refusals))]
    Refused { refusals: Vec<Refusal> },
}

impl MergeError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        MergeError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// One reason a merge target cannot be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// Repository-relative merge target.
    pub target: PathBuf,
    /// Why the target cannot be written.
    pub reason: RefusalReason,
}

/// Classification of a merge refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefusalReason {
    /// The document carries delta operations merge does not support
    /// yet; merging into existing documents is a deferred capability.
    UnsupportedDelta(Vec<String>),
    /// The target body no longer matches its provenance hash: hand
    /// edits since the last merge.
    Drifted,
    /// A file exists at the target without a provenance header, so
    /// nbspec did not write it.
    Unmanaged,
    /// The target belongs to a different change.
    ForeignChange(String),
    /// A directory (or other non-file) occupies the target; nbspec
    /// never removes such occupants, so `--force` cannot override.
    NonFileTarget,
}

impl std::fmt::Display for RefusalReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefusalReason::UnsupportedDelta(operations) => write!(
                formatter,
                "{} delta operations are not supported yet \
                 (merging into existing documents is deferred)",
                operations.join(", ")
            ),
            RefusalReason::Drifted => write!(
                formatter,
                "drifted since last merge (hand edits present); \
                 rerun with --force to overwrite"
            ),
            RefusalReason::Unmanaged => write!(
                formatter,
                "an unmanaged file occupies the target (no nbspec \
                 provenance); rerun with --force to overwrite"
            ),
            RefusalReason::ForeignChange(change_id) => write!(
                formatter,
                "owned by change {change_id}; rerun with --force to take over"
            ),
            RefusalReason::NonFileTarget => write!(
                formatter,
                "a directory or other non-file occupies the target; \
                 remove it manually (--force does not override)"
            ),
        }
    }
}

fn format_refusals(refusals: &[Refusal]) -> String {
    refusals
        .iter()
        .map(|refusal| format!("- {}: {}", refusal.target.display(), refusal.reason))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Merge target status relative to a change, as reported by
/// `display` and consulted during merge planning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetStatus {
    /// No file at the target yet.
    NotMerged,
    /// Target matches what this change's notes render to.
    Current,
    /// Target is clean but the notebook has newer content.
    UpdatePending,
    /// Target body no longer matches its provenance hash.
    Drifted,
    /// A file without provenance occupies the target.
    Unmanaged,
    /// The target's provenance names a different change.
    OwnedByOtherChange(String),
    /// A directory (or other non-file) occupies the target.
    NonFile,
}

impl std::fmt::Display for TargetStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetStatus::NotMerged => write!(formatter, "not merged"),
            TargetStatus::Current => write!(formatter, "merged, current"),
            TargetStatus::UpdatePending => {
                write!(formatter, "merged, notebook update pending")
            }
            TargetStatus::Drifted => {
                write!(formatter, "drifted (hand edits since last merge)")
            }
            TargetStatus::Unmanaged => {
                write!(
                    formatter,
                    "unmanaged file at target (not written by nbspec)"
                )
            }
            TargetStatus::OwnedByOtherChange(change_id) => {
                write!(formatter, "owned by change {change_id}")
            }
            TargetStatus::NonFile => {
                write!(formatter, "blocked: a non-file occupies the target")
            }
        }
    }
}

/// Outcome of a successful merge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    /// Repository-relative paths written.
    pub written: Vec<PathBuf>,
    /// Repository-relative paths already byte-identical and left
    /// untouched.
    pub unchanged: Vec<PathBuf>,
}

/// Classifies the merge target of one rendered document.
///
/// # Errors
///
/// Returns [`MergeError::Io`] when an existing target cannot be read.
pub fn target_status(
    document: &RenderedDocument,
    project_root: &Path,
    change_id: &str,
) -> Result<TargetStatus, MergeError> {
    let Some(target_path) = &document.target_path else {
        return Ok(TargetStatus::NotMerged);
    };
    let absolute = project_root.join(target_path);
    if !absolute.exists() {
        return Ok(TargetStatus::NotMerged);
    }
    if !absolute.is_file() {
        return Ok(TargetStatus::NonFile);
    }
    let content =
        std::fs::read_to_string(&absolute).map_err(|error| MergeError::io(&absolute, error))?;
    let (header, body) = provenance::split_document(&content);
    let Some(header) = header else {
        return Ok(TargetStatus::Unmanaged);
    };
    if header.change_id != change_id {
        return Ok(TargetStatus::OwnedByOtherChange(header.change_id));
    }
    if !provenance::body_matches(&header, body) {
        return Ok(TargetStatus::Drifted);
    }
    if body == document.content {
        Ok(TargetStatus::Current)
    } else {
        Ok(TargetStatus::UpdatePending)
    }
}

/// Transfers a change's durable documents to their merge targets,
/// stamped with provenance. Plans first — collecting every refusal
/// across every target — and writes only when the whole plan is
/// clean, so a refused merge modifies nothing.
///
/// # Errors
///
/// Returns [`MergeError::Refused`] listing every violating target,
/// and [`MergeError::Io`] on read or write failures.
pub fn merge_documents(
    documents: &[RenderedDocument],
    project_root: &Path,
    change_id: &str,
    notebook: &str,
    force: bool,
) -> Result<MergeReport, MergeError> {
    let mut refusals = Vec::new();
    let mut writes: Vec<(PathBuf, String)> = Vec::new();
    let mut report = MergeReport::default();
    for document in documents {
        let Some(target_path) = &document.target_path else {
            continue;
        };
        if let Some(operations) = unsupported_operations(&document.content) {
            refusals.push(Refusal {
                target: target_path.clone(),
                reason: RefusalReason::UnsupportedDelta(operations),
            });
            continue;
        }
        let status = target_status(document, project_root, change_id)?;
        if status == TargetStatus::NonFile {
            refusals.push(Refusal {
                target: target_path.clone(),
                reason: RefusalReason::NonFileTarget,
            });
            continue;
        }
        let refusal = match &status {
            TargetStatus::Drifted => Some(RefusalReason::Drifted),
            TargetStatus::Unmanaged => Some(RefusalReason::Unmanaged),
            TargetStatus::OwnedByOtherChange(other) => {
                Some(RefusalReason::ForeignChange(other.clone()))
            }
            TargetStatus::NotMerged
            | TargetStatus::Current
            | TargetStatus::UpdatePending
            | TargetStatus::NonFile => None,
        };
        if let Some(reason) = refusal
            && !force
        {
            refusals.push(Refusal {
                target: target_path.clone(),
                reason,
            });
            continue;
        }
        if status == TargetStatus::Current {
            report.unchanged.push(target_path.clone());
            continue;
        }
        let stamped = provenance::stamp(
            &document.content,
            change_id,
            notebook,
            &document.source_note,
        );
        writes.push((target_path.clone(), stamped));
    }
    if !refusals.is_empty() {
        return Err(MergeError::Refused { refusals });
    }
    for (target_path, content) in writes {
        let absolute = project_root.join(&target_path);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).map_err(|error| MergeError::io(parent, error))?;
        }
        std::fs::write(&absolute, content).map_err(|error| MergeError::io(&absolute, error))?;
        report.written.push(target_path);
    }
    Ok(report)
}

/// Names the delta operations a document uses that merge does not
/// support yet, or `None` when the document is mergeable.
fn unsupported_operations(content: &str) -> Option<Vec<String>> {
    let presence = parse_delta_specification(content).presence;
    let mut operations = Vec::new();
    if presence.modified {
        operations.push("MODIFIED".to_string());
    }
    if presence.removed {
        operations.push("REMOVED".to_string());
    }
    if presence.renamed {
        operations.push("RENAMED".to_string());
    }
    if operations.is_empty() {
        None
    } else {
        Some(operations)
    }
}
