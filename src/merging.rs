//! Durable artifact merge with provenance and drift protection.
//!
//! Merge is the only nbspec operation that writes to the repository
//! working tree, and it never creates git commits. It runs in two
//! phases: planning inspects every merge target and collects every
//! refusal — unsupported delta operations, hand-edited targets,
//! unmanaged files, foreign-owned documents that drifted — and only a
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
    /// Repository-relative merge target, forward-slash logical path.
    pub target: String,
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
    /// The target belongs to a different change AND has drifted from
    /// its recorded provenance. Clean-provenance foreign targets do
    /// not refuse: they succeed by clean succession (see
    /// [`MergeReport::successions`]).
    ForeignDrifted(String),
    /// A directory (or other non-file) occupies the target; nbspec
    /// never removes such occupants, so `--force` cannot override.
    NonFileTarget,
    /// The review gate is unsatisfied. The payload describes the gate
    /// state: no verdict, stale approval naming the bound hash,
    /// outstanding revise, or unparseable verdict naming the note.
    /// Policy, not integrity: `--force` overrides it, loudly.
    ReviewGate(String),
    /// A file in the repository has provenance naming a source note
    /// in the current change, but its path no longer matches the
    /// rendered target — typically because an H1 edit on a
    /// timestamp-named note changed the materialization slug.
    /// `--force` overrides (the old file remains and must be removed
    /// manually).
    StaleTarget {
        source_note: String,
        new_target: String,
    },
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
            RefusalReason::ForeignDrifted(change_id) => write!(
                formatter,
                "owned by change {change_id} and drifted from its recorded \
                 provenance; rerun with --force to take over"
            ),
            RefusalReason::NonFileTarget => write!(
                formatter,
                "a directory or other non-file occupies the target; \
                 remove it manually (--force does not override)"
            ),
            RefusalReason::ReviewGate(state) => write!(
                formatter,
                "review gate unsatisfied: {state}; record an approving \
                 verdict with nbspec review, or rerun with --force to \
                 override the gate"
            ),
            RefusalReason::StaleTarget {
                source_note,
                new_target,
            } => write!(
                formatter,
                "provenance names source note {source_note} but target moved \
                 to {new_target} (H1-derived slug changed); remove this stale \
                 file or rerun with --force to proceed"
            ),
        }
    }
}

fn format_refusals(refusals: &[Refusal]) -> String {
    refusals
        .iter()
        .map(|refusal| format!("- {}: {}", refusal.target, refusal.reason))
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
    /// The target's provenance names a different change and its body
    /// still matches that provenance: clean succession is available
    /// (a merge inherits the target without `--force`).
    OwnedByOtherChange(String),
    /// The target's provenance names a different change AND its body
    /// no longer matches that provenance: takeover requires `--force`.
    ForeignDrifted(String),
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
                write!(
                    formatter,
                    "owned by change {change_id}; clean succession available"
                )
            }
            TargetStatus::ForeignDrifted(change_id) => {
                write!(
                    formatter,
                    "owned by change {change_id}, drifted since its materialization"
                )
            }
            TargetStatus::NonFile => {
                write!(formatter, "blocked: a non-file occupies the target")
            }
        }
    }
}

/// One ownership transfer of a target from another change: recorded
/// as a clean succession when the previous materialization was
/// intact (body matched its own provenance header), or as a forced
/// drift override when `--force` overwrote a drifted foreign target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Succession {
    /// Repository-relative target path, forward-slash logical path.
    pub target: String,
    /// Change that owned the target before this merge.
    pub previous_owner: String,
}

/// Outcome of a successful merge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    /// Repository-relative paths written, forward-slash logical paths.
    pub written: Vec<String>,
    /// Repository-relative paths already byte-identical and left
    /// untouched, forward-slash logical paths.
    pub unchanged: Vec<String>,
    /// Set when `--force` overrode an unsatisfied review gate; carries
    /// the gate-state description so merge output reports the
    /// override loudly.
    pub review_gate_overridden: Option<String>,
    /// Clean successions performed by this merge. Never silent:
    /// merge output announces each, naming both changes.
    pub successions: Vec<Succession>,
    /// Drifted foreign targets overwritten under `--force`. Never
    /// silent either: merge output states that a drifted target was
    /// overridden, naming its previous owner.
    pub drift_overrides: Vec<Succession>,
    /// Stale targets left behind when `--force` overrode an H1-slug
    /// rename on a timestamp-named note. The old file remains in the
    /// repository and must be removed manually. Never silent: merge
    /// output announces each stale path.
    pub stale_target_overrides: Vec<String>,
}

/// Classifies the merge target of one rendered document.
///
/// # Errors
///
/// Returns [`MergeError::Io`] when the target path escapes confinement
/// (symlink ancestor or target-file symlink), when an intermediate
/// component exists but is not a directory, or when an existing real
/// target file cannot be read.
pub fn target_status(
    document: &RenderedDocument,
    project_root: &Path,
    change_id: &str,
) -> Result<TargetStatus, MergeError> {
    let Some(target_path) = &document.target_path else {
        return Ok(TargetStatus::NotMerged);
    };
    let absolute = match inspect_confined_target(project_root, target_path)? {
        ConfinedTarget::Absent { .. } => return Ok(TargetStatus::NotMerged),
        ConfinedTarget::NonFile { .. } => return Ok(TargetStatus::NonFile),
        ConfinedTarget::RealFile { absolute } => absolute,
    };
    let content =
        std::fs::read_to_string(&absolute).map_err(|error| MergeError::io(&absolute, error))?;
    let (header, body) = provenance::split_document(&content);
    let Some(header) = header else {
        return Ok(TargetStatus::Unmanaged);
    };
    if header.change_id != change_id {
        // The succession test compares the target's CURRENT content
        // against the hash in its OWN provenance header — never the
        // proposed new content against the old header.
        if provenance::body_matches(&header, body) {
            return Ok(TargetStatus::OwnedByOtherChange(header.change_id));
        }
        return Ok(TargetStatus::ForeignDrifted(header.change_id));
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
    review_gate_state: Option<&str>,
    force: bool,
) -> Result<MergeReport, MergeError> {
    let mut refusals = Vec::new();
    let mut writes: Vec<(String, String)> = Vec::new();
    let mut report = MergeReport::default();
    if let Some(state) = review_gate_state {
        if force {
            report.review_gate_overridden = Some(state.to_string());
        } else {
            refusals.push(Refusal {
                target: change_id.to_string(),
                reason: RefusalReason::ReviewGate(state.to_string()),
            });
        }
    }
    let stale = detect_stale_targets(documents, project_root, change_id)?;
    if force {
        for r in &stale {
            report.stale_target_overrides.push(r.target.clone());
        }
    } else {
        refusals.extend(stale);
    }
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
            TargetStatus::ForeignDrifted(other) => {
                Some(RefusalReason::ForeignDrifted(other.clone()))
            }
            TargetStatus::NotMerged
            | TargetStatus::Current
            | TargetStatus::UpdatePending
            | TargetStatus::OwnedByOtherChange(_)
            | TargetStatus::NonFile => None,
        };
        if let Some(reason) = refusal {
            if !force {
                refusals.push(Refusal {
                    target: target_path.clone(),
                    reason,
                });
                continue;
            }
            if let RefusalReason::ForeignDrifted(previous_owner) = &reason {
                report.drift_overrides.push(Succession {
                    target: target_path.clone(),
                    previous_owner: previous_owner.clone(),
                });
            }
        }
        if let TargetStatus::OwnedByOtherChange(previous_owner) = &status {
            report.successions.push(Succession {
                target: target_path.clone(),
                previous_owner: previous_owner.clone(),
            });
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
        // Re-validate confinement immediately before effects so a
        // race that introduces a symlink cannot open an escape.
        let absolute = match inspect_confined_target(project_root, &target_path)? {
            ConfinedTarget::Absent { absolute } | ConfinedTarget::RealFile { absolute } => absolute,
            ConfinedTarget::NonFile { absolute } => {
                return Err(MergeError::io(
                    &absolute,
                    std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "non-file occupies target at write time",
                    ),
                ));
            }
        };
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

/// Scans merge-target directories for files whose provenance names a
/// source note in the current change but whose path no longer matches
/// the rendered target. This detects H1-slug renames on
/// timestamp-named notes: the old target remains in the repository
/// after the slug changes.
///
/// Fails closed: any filesystem error (metadata, read_dir, entry,
/// read) returns `MergeError::Io` before any merge effects. Every
/// existing path component under the project root is validated with
/// non-following metadata; symlink ancestors and non-directory
/// intermediate components refuse. Symlink entries inside a real
/// directory are skipped (not followed).
fn detect_stale_targets(
    documents: &[RenderedDocument],
    project_root: &Path,
    change_id: &str,
) -> Result<Vec<Refusal>, MergeError> {
    use std::collections::HashMap;
    let mut source_to_target: HashMap<&str, &str> = HashMap::new();
    let mut target_dirs: HashMap<String, ()> = HashMap::new();
    for doc in documents {
        let Some(target) = &doc.target_path else {
            continue;
        };
        source_to_target.insert(doc.source_note.as_str(), target.as_str());
        let parent = Path::new(target)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        target_dirs.entry(parent.to_string()).or_default();
    }
    let mut refusals = Vec::new();
    for dir in target_dirs.keys() {
        let Some(abs_dir) = probe_confined_directory(project_root, dir)? else {
            continue;
        };
        let entries = std::fs::read_dir(&abs_dir).map_err(|e| MergeError::io(&abs_dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| MergeError::io(&abs_dir, e))?;
            let file_type = entry
                .file_type()
                .map_err(|e| MergeError::io(&entry.path(), e))?;
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            let filename = entry.file_name().to_string_lossy().to_string();
            if !filename.ends_with(".md") {
                continue;
            }
            let rel_path = if dir.is_empty() {
                filename
            } else {
                format!("{dir}/{filename}")
            };
            let content = std::fs::read_to_string(&path).map_err(|e| MergeError::io(&path, e))?;
            let (header, _) = provenance::split_document(&content);
            let Some(header) = header else {
                continue;
            };
            if header.change_id != change_id {
                continue;
            }
            if let Some(&expected_target) = source_to_target.get(header.note.as_str())
                && rel_path != expected_target
            {
                refusals.push(Refusal {
                    target: rel_path,
                    reason: RefusalReason::StaleTarget {
                        source_note: header.note.clone(),
                        new_target: expected_target.to_string(),
                    },
                });
            }
        }
    }
    Ok(refusals)
}

/// Outcome of a confined inspection of one durable target path.
enum ConfinedTarget {
    /// No final component; every existing ancestor is a real directory.
    Absent { absolute: PathBuf },
    /// Final component is a real regular file under confined ancestors.
    RealFile { absolute: PathBuf },
    /// Final component is a real non-file (directory or special).
    NonFile { absolute: PathBuf },
}

/// Walks each component of `relative` under `project_root` with
/// non-following metadata. Rejects symlink components (ancestors or
/// final) and intermediate non-directory occupants so merge never
/// follows a link out of the repository or treats ENOTDIR as absence.
fn inspect_confined_target(
    project_root: &Path,
    relative: &str,
) -> Result<ConfinedTarget, MergeError> {
    let relative_path = Path::new(relative);
    let mut absolute = project_root.to_path_buf();
    let components: Vec<_> = relative_path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    if components.is_empty() {
        return Err(MergeError::io(
            project_root,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty durable target path",
            ),
        ));
    }
    let last = components.len() - 1;
    for (index, name) in components.iter().enumerate() {
        absolute.push(name);
        let metadata = match std::fs::symlink_metadata(&absolute) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Remainder is absent. Ancestors so far were real dirs.
                return Ok(ConfinedTarget::Absent {
                    absolute: project_root.join(relative_path),
                });
            }
            Err(error) => return Err(MergeError::io(&absolute, error)),
            Ok(metadata) => metadata,
        };
        if metadata.file_type().is_symlink() {
            return Err(symlink_refusal(&absolute));
        }
        if index < last {
            if !metadata.is_dir() {
                return Err(MergeError::io(
                    &absolute,
                    std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        "intermediate path component is not a directory",
                    ),
                ));
            }
            continue;
        }
        // Final component.
        if metadata.is_file() {
            return Ok(ConfinedTarget::RealFile { absolute });
        }
        return Ok(ConfinedTarget::NonFile { absolute });
    }
    unreachable!("components non-empty")
}

/// Probes a target-directory relative path with full ancestor
/// confinement. Returns `Ok(Some(absolute))` for a real directory,
/// `Ok(None)` when absent, and `Err` for symlinks, non-directory
/// intermediates/final, or metadata failures.
fn probe_confined_directory(
    project_root: &Path,
    relative: &str,
) -> Result<Option<PathBuf>, MergeError> {
    if relative.is_empty() {
        return Ok(Some(project_root.to_path_buf()));
    }
    match inspect_confined_target(project_root, relative)? {
        ConfinedTarget::Absent { .. } => Ok(None),
        ConfinedTarget::RealFile { absolute } | ConfinedTarget::NonFile { absolute } => {
            // For a directory probe the final component must be a real
            // directory. RealFile and non-dir NonFile are failures;
            // a real directory arrives as NonFile (not is_file).
            let metadata = std::fs::symlink_metadata(&absolute)
                .map_err(|error| MergeError::io(&absolute, error))?;
            if metadata.file_type().is_symlink() {
                return Err(symlink_refusal(&absolute));
            }
            if metadata.is_dir() {
                Ok(Some(absolute))
            } else {
                Err(MergeError::io(
                    &absolute,
                    std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        "target parent exists and is not a directory",
                    ),
                ))
            }
        }
    }
}

fn symlink_refusal(path: &Path) -> MergeError {
    MergeError::io(
        path,
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "symlink on durable target path; refusing to follow",
        ),
    )
}
