//! Namespace retirement: snapshot-bound deletion of a merged change.
//!
//! After `merge` materializes durable documents and publishes the change
//! tarball, the source namespace under `proposals/<change-id>/` is deleted
//! as one folder-level transaction. The tarball is the sole surviving
//! record, so every step here is fail-closed: the archive must hold a
//! byte-faithful copy of the whole namespace, verified against a captured
//! inventory, and the deletion must bind to that same inventory via the
//! notebook revision captured before the review gate ran.
//!
//! The snapshot discipline (one source snapshot binding render, gate,
//! materialization, archive, and delete) lives in the proposal
//! `add-namespace-retirement`; this module is its executable half.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::archives::{ArchiveEntry, extract_archive};
use crate::changes::parse_meta_note;
use crate::operations::OperationError;

/// Notebook-managed bookkeeping files, excluded from the namespace
/// inventory and the tarball by design. They describe the notebook,
/// not the change.
const BOOKKEEPING_FILES: [&str; 2] = [".gitkeep", ".index"];

/// Archive-relative prefix carrying the byte-faithful namespace copy.
const NAMESPACE_PREFIX: &str = "namespace";

/// One inventoried namespace file: its notebook-relative path with
/// forward slashes, its SHA-256, and its raw bytes. Bytes ride along
/// so archive construction reads files in the same pass that fixes
/// the inventory hashes — the tarball can never mix pre- and
/// post-change content.
#[derive(Clone, Debug)]
pub struct SnapshotFile {
    /// Path relative to the namespace root, `/`-separated.
    pub relative: String,
    /// SHA-256 of the raw bytes.
    pub hash: [u8; 32],
    /// Raw file bytes.
    pub bytes: Vec<u8>,
}

/// A captured namespace snapshot: live files with hashes and bytes.
#[derive(Clone, Debug, Default)]
pub struct NamespaceSnapshot {
    /// Inventory in walk order (callers sort for comparison).
    pub files: Vec<SnapshotFile>,
}

/// Walks `change_directory` recursively, capturing every user file
/// with its hash and bytes. Bookkeeping files are skipped. Any
/// unreadable file or directory fails closed: a namespace that
/// cannot be fully inventoried must never be deleted.
pub fn capture_snapshot(change_directory: &Path) -> Result<NamespaceSnapshot, OperationError> {
    let mut files = Vec::new();
    walk_snapshot(change_directory, change_directory, &mut files)?;
    Ok(NamespaceSnapshot { files })
}

/// Re-verifies the live namespace against a captured snapshot: same
/// file set, same hashes. New, removed, or edited entries fail the
/// check.
pub fn verify_snapshot(
    change_directory: &Path,
    snapshot: &NamespaceSnapshot,
) -> Result<(), OperationError> {
    let live = capture_snapshot(change_directory)?;
    let mut expected: HashMap<&str, &[u8; 32]> = HashMap::new();
    for file in &snapshot.files {
        expected.insert(file.relative.as_str(), &file.hash);
    }
    let mut seen = 0;
    for file in &live.files {
        match expected.get(file.relative.as_str()) {
            Some(hash) if *hash == &file.hash => seen += 1,
            _ => {
                return Err(snapshot_drift(&file.relative));
            }
        }
    }
    if seen != snapshot.files.len() {
        return Err(snapshot_drift("<removed entry>"));
    }
    Ok(())
}

fn snapshot_drift(relative: &str) -> OperationError {
    OperationError::RetirementRefused {
        reason: format!("namespace changed after snapshot: {relative}"),
        guidance: "Resolve the drift (or re-archive) and re-run merge; \
            nothing was published, moved, or deleted."
            .to_string(),
    }
}

fn walk_snapshot(
    root: &Path,
    current: &Path,
    files: &mut Vec<SnapshotFile>,
) -> Result<(), OperationError> {
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(current)
        .map_err(|source| OperationError::NoteRead {
            path: current.to_path_buf(),
            source,
        })?
        .collect::<Result<_, _>>()
        .map_err(|source| OperationError::NoteRead {
            path: current.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        // Bookkeeping files are skipped before anything else. All
        // other names flow into relative_key, which fails closed on
        // non-UTF-8 and backslash names before publication.
        let file_name = entry.file_name();
        if file_name
            .to_str()
            .is_some_and(|name| BOOKKEEPING_FILES.contains(&name))
        {
            continue;
        }
        let path = current.join(&file_name);
        let file_type =
            std::fs::symlink_metadata(&path).map_err(|source| OperationError::NoteRead {
                path: path.clone(),
                source,
            })?;
        if file_type.is_dir() {
            walk_snapshot(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(OperationError::NoteRead {
                path,
                source: std::io::Error::other("not a regular file"),
            });
        }
        let bytes = std::fs::read(&path).map_err(|source| OperationError::NoteRead {
            path: path.clone(),
            source,
        })?;
        // One canonical key helper shared with persisted
        // verification: forward slashes on every platform, with
        // unrepresentable names refused rather than remapped.
        let relative = relative_key(&path, root)?;
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        files.push(SnapshotFile {
            relative,
            hash,
            bytes,
        });
    }
    Ok(())
}

/// Builds the `namespace/` tarball entries from a snapshot: original
/// relative paths, raw bytes, no normalization.
pub fn namespace_entries(change_id: &str, snapshot: &NamespaceSnapshot) -> Vec<ArchiveEntry> {
    snapshot
        .files
        .iter()
        .map(|file| ArchiveEntry {
            path: PathBuf::from(change_id)
                .join(NAMESPACE_PREFIX)
                .join(&file.relative),
            content: file.bytes.clone(),
        })
        .collect()
}

/// Verifies a persisted tarball against the snapshot: extracts the
/// `namespace/` subtree and compares paths and bytes. Any mismatch
/// refuses before any deletion.
pub fn verify_persisted_archive(
    archive_path: &Path,
    change_id: &str,
    snapshot: &NamespaceSnapshot,
) -> Result<(), OperationError> {
    let bytes = std::fs::read(archive_path).map_err(|source| OperationError::NoteRead {
        path: archive_path.to_path_buf(),
        source,
    })?;
    let scratch = scratch_dir(change_id)?;
    let outcome = verify_extracted(&bytes, &scratch, change_id, snapshot);
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}

fn scratch_dir(change_id: &str) -> Result<PathBuf, OperationError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let scratch = std::env::temp_dir().join(format!(
        "nbspec-retirement-verify-{change_id}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).map_err(|source| OperationError::NoteRead {
        path: scratch.clone(),
        source,
    })?;
    Ok(scratch)
}

fn verify_extracted(
    bytes: &[u8],
    scratch: &Path,
    change_id: &str,
    snapshot: &NamespaceSnapshot,
) -> Result<(), OperationError> {
    let extract_root = scratch.join("extracted");
    if let Err(error) = extract_archive(
        bytes,
        &extract_root,
        Some(&format!("{change_id}/{NAMESPACE_PREFIX}/")),
    ) {
        return Err(archive_problem(format!(
            "persisted tarball cannot be verified: {error}"
        )));
    }
    let mut expected: HashMap<&str, &[u8]> = HashMap::new();
    for file in &snapshot.files {
        expected.insert(file.relative.as_str(), file.bytes.as_slice());
    }
    let mut seen = 0;
    let mut stack = vec![extract_root.clone()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(|source| OperationError::NoteRead {
            path: current.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| OperationError::NoteRead {
                path: current.clone(),
                source,
            })?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            // Canonical key shared with snapshot capture: forward
            // slashes, validated components. A tarball entry with a
            // literal backslash cannot masquerade as a slash path.
            let relative = relative_key(&path, &extract_root).map_err(|_| {
                archive_problem("tarball holds an unrepresentable path".to_string())
            })?;
            let bytes = std::fs::read(&path).map_err(|source| OperationError::NoteRead {
                path: path.clone(),
                source,
            })?;
            match expected.get(relative.as_str()) {
                Some(want) if *want == bytes.as_slice() => seen += 1,
                _ => {
                    return Err(archive_problem(format!(
                        "persisted tarball diverges at {relative}"
                    )));
                }
            }
        }
    }
    if seen != snapshot.files.len() {
        return Err(archive_problem(
            "persisted tarball file count diverges from the inventory".to_string(),
        ));
    }
    Ok(())
}

fn archive_problem(reason: String) -> OperationError {
    OperationError::RetirementRefused {
        reason,
        guidance: "inspection required: resolve the archive state before retrying; nothing \
            was deleted."
            .to_string(),
    }
}

/// Validates a tarball already on disk as proof of a retired change:
/// a regular file, a readable tar.zst, every entry beneath the
/// `<change-id>/` prefix, and an embedded meta snapshot naming this
/// change id. Anything else fails closed — presence alone never
/// proves already-merged.
pub fn validate_archive_identity(bytes: &[u8], change_id: &str) -> Result<(), OperationError> {
    let decoded = zstd::decode_all(bytes).map_err(|_| {
        archive_problem("existing tarball is not a readable zstd archive".to_string())
    })?;
    let mut archive = tar::Archive::new(decoded.as_slice());
    let entries = archive.entries().map_err(|_| {
        archive_problem("existing tarball is not a readable tar archive".to_string())
    })?;
    let mut meta_count = 0;
    for entry in entries {
        let mut entry = entry.map_err(|_| {
            archive_problem("existing tarball holds an unreadable entry".to_string())
        })?;
        let path = entry.path().map_err(|_| {
            archive_problem("existing tarball holds an unmappable path".to_string())
        })?;
        if path.is_absolute() {
            return Err(archive_problem(
                "existing tarball holds an absolute path".to_string(),
            ));
        }
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(archive_problem(
                "existing tarball escapes its prefix".to_string(),
            ));
        }
        if !entry.header().entry_type().is_file() {
            return Err(archive_problem(
                "existing tarball holds a non-file entry".to_string(),
            ));
        }
        // Component-boundary match: the first path component must be
        // exactly the change id. A string prefix is not enough —
        // `add-demometa.md` or `add-demo-other/x` must not pass for
        // `add-demo`.
        let text = path.to_str().ok_or_else(|| {
            archive_problem("existing tarball holds a non-UTF-8 path".to_string())
        })?;
        let mut parts = text.split('/');
        if parts.next() != Some(change_id) {
            return Err(archive_problem(
                "existing tarball holds an entry outside this change".to_string(),
            ));
        }
        if parts.collect::<Vec<&str>>() != ["meta.md"] {
            continue;
        }
        let mut content = Vec::new();
        use std::io::Read;
        entry
            .read_to_end(&mut content)
            .map_err(|_| archive_problem("existing tarball meta is unreadable".to_string()))?;
        let text = String::from_utf8(content)
            .map_err(|_| archive_problem("existing tarball meta is not UTF-8".to_string()))?;
        let metadata = parse_meta_note(&text)
            .map_err(|_| archive_problem("existing tarball meta does not parse".to_string()))?;
        if metadata.change_id != change_id {
            return Err(archive_problem(
                "existing tarball meta names a different change".to_string(),
            ));
        }
        meta_count += 1;
    }
    if meta_count != 1 {
        return Err(archive_problem(
            "existing tarball must carry exactly one meta snapshot for this change".to_string(),
        ));
    }
    Ok(())
}

/// Builds the canonical forward-slash inventory key for a path
/// relative to `root`, shared by snapshot capture and persisted
/// verification so the two walks cannot disagree. Every component
/// must be representable: non-UTF-8 names fail closed, and
/// backslashes fail closed even on Unix — a literal backslash is a
/// legal Unix filename character, but normalizing it to a separator
/// would let a tarball entry `a\b` masquerade as inventory path
/// `a/b` and lose the original name on deletion. Anything that is
/// not a plain relative component (prefixes, parent or current dir)
/// fails closed too.
fn relative_key(path: &Path, root: &Path) -> Result<String, OperationError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|source| OperationError::NoteRead {
            path: path.to_path_buf(),
            source: std::io::Error::other(source),
        })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(name) => {
                let text = name.to_str().ok_or_else(|| OperationError::NoteRead {
                    path: path.to_path_buf(),
                    source: std::io::Error::other(
                        "filename is not representable; refusing retirement",
                    ),
                })?;
                if text.contains('\\') {
                    return Err(OperationError::NoteRead {
                        path: path.to_path_buf(),
                        source: std::io::Error::other(
                            "backslash filename is not representable; refusing retirement",
                        ),
                    });
                }
                parts.push(text);
            }
            _ => {
                return Err(OperationError::NoteRead {
                    path: path.to_path_buf(),
                    source: std::io::Error::other(
                        "non-relative inventory path; refusing retirement",
                    ),
                });
            }
        }
    }
    if parts.is_empty() {
        return Err(OperationError::NoteRead {
            path: path.to_path_buf(),
            source: std::io::Error::other("empty inventory path; refusing retirement"),
        });
    }
    Ok(parts.join("/"))
}
/// What occupies an expected archive path, resolved WITHOUT
/// following symlinks. A symlink to a valid tarball must never read
/// as proof, and a dangling link must never read as absence.
pub enum ArchiveOccupant {
    Absent,
    File(Vec<u8>),
    Invalid(String),
}

/// Classifies the occupant of an archive path without following
/// symlinks. Only a missing path maps to [`ArchiveOccupant::Absent`];
/// symlinks and non-files are [`ArchiveOccupant::Invalid`], never
/// silently treated as unclaimed.
pub fn read_archive_occupant(path: &Path) -> Result<ArchiveOccupant, OperationError> {
    match std::fs::symlink_metadata(path) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(ArchiveOccupant::Absent),
        Err(source) => Err(OperationError::NoteRead {
            path: path.to_path_buf(),
            source,
        }),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                Ok(ArchiveOccupant::Invalid("path is a symlink".to_string()))
            } else if !metadata.is_file() {
                Ok(ArchiveOccupant::Invalid(
                    "path is not a regular file".to_string(),
                ))
            } else {
                std::fs::read(path)
                    .map(ArchiveOccupant::File)
                    .map_err(|source| OperationError::NoteRead {
                        path: path.to_path_buf(),
                        source,
                    })
            }
        }
    }
}

/// Refuses a non-file archive occupant with inspection guidance.
pub fn refuse_invalid_occupant(path: &Path, detail: &str) -> OperationError {
    OperationError::RetirementRefused {
        reason: format!("archive path {} {detail}", path.display()),
        guidance: "inspection required: resolve the archive path state before retrying; \
            nothing was published, moved, or deleted."
            .to_string(),
    }
}
/// Maps a deletion-transaction commit outcome to the retirement
/// contract. Clean failures leave the namespace wholly in place
/// (safe to resume); indeterminate or recovery-required outcomes
/// report inspection-required with no rollback claim.
pub fn classify_commit_outcome(
    outcome: Result<(), nb_api::NbError>,
    phases: &str,
) -> Result<(), OperationError> {
    match outcome {
        Ok(()) => Ok(()),
        Err(
            error @ (nb_api::NbError::IndeterminateCommit { .. }
            | nb_api::NbError::RecoveryRequired { .. }),
        ) => Err(OperationError::RetirementFailed {
            phases: phases.to_string(),
            guidance: format!(
                "inspection required: the delete may or may not have completed ({error}); re-read \
                 notebook and archive state before retrying; do not retry blind"
            ),
        }),
        Err(error) => Err(OperationError::RetirementFailed {
            phases: phases.to_string(),
            guidance: format!("namespace left wholly in place ({error}); re-run merge to resume"),
        }),
    }
}

/// Captures the notebook HEAD revision at the snapshot moment, for
/// the deletion transaction's `AssertRevision` precondition. A
/// notebook without a readable HEAD cannot retire: deletion without
/// a revision binding is refused, never silent.
pub fn capture_notebook_head(notebook_path: &Path) -> Result<String, OperationError> {
    let mut command = std::process::Command::new("git");
    crate::git_env::scrub_git_env(&mut command);
    let output = command
        .args(["rev-parse", "HEAD"])
        .current_dir(notebook_path)
        .output()
        .map_err(|_source| OperationError::RetirementRefused {
            reason: "cannot read the notebook revision for the retirement snapshot".to_string(),
            guidance: "Commit or repair the notebook repository state and re-run merge; \
                nothing was published, moved, or deleted."
                .to_string(),
        })?;
    if !output.status.success() {
        return Err(OperationError::RetirementRefused {
            reason: "cannot read the notebook revision for the retirement snapshot".to_string(),
            guidance: "Commit or repair the notebook repository state and re-run merge; \
                nothing was published, moved, or deleted."
                .to_string(),
        });
    }
    let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if revision.is_empty() {
        return Err(OperationError::RetirementRefused {
            reason: "the notebook revision is empty; refusing snapshot binding".to_string(),
            guidance: "Commit or repair the notebook repository state and re-run merge; \
                nothing was published, moved, or deleted."
                .to_string(),
        });
    }
    Ok(revision)
}
