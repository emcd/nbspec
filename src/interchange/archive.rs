use std::path::{Path, PathBuf};

use crate::archives::{ArchiveEntry, ArchiveError, build_archive};
use crate::interchange::detect::{ArchiveTree, STAGING_COUNTER, rename_no_replace};
use crate::interchange::plan::InterchangeError;
use crate::interchange::proof::walk_for_proof;

/// Staging result for an archive: the staging file path and the
/// target path; the caller commits via `commit_archive_staging`
/// after every entry's stage has succeeded (R7 fourth re-review
/// 2026-07-25 — multi-entry import all-or-nothing across entries).
pub struct ArchiveStaging {
    pub staging_file: PathBuf,
    pub target_file: PathBuf,
}

/// Stages an archive at `<target>.staging-<nanos>-<pid>-<counter>`
/// without committing. The bytes are written via
/// `OpenOptions::create_new(true)` so a residual staging file
/// from a prior failed run is surfaced as `AlreadyExists` rather
/// than silently overwritten. Combined with the collision-safe
/// suffix and `rename_no_replace`, this closes every cross-process
/// path-reuse window.
///
/// R9 / F7 third re-review 2026-07-25 (final-stack blocker).
pub fn stage_archive_file(
    _change_id: &str,
    target: &Path,
    bytes: &[u8],
) -> Result<ArchiveStaging, crate::operations::OperationError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::operations::OperationError::ArchiveWrite {
                path: parent.to_path_buf(),
                source,
            }
        })?;
    }
    let counter = STAGING_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let staging = match target.file_name() {
        Some(name) => target.with_file_name(format!(
            "{}.staging-{}-{}-{}",
            name.to_string_lossy(),
            nanos,
            std::process::id(),
            counter,
        )),
        None => {
            return Err(crate::operations::OperationError::ArchiveWrite {
                path: target.to_path_buf(),
                source: std::io::Error::other("archive target has no file name"),
            });
        }
    };
    use std::io::Write;
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|source| crate::operations::OperationError::ArchiveWrite {
                path: target.to_path_buf(),
                source,
            })?;
        file.write_all(bytes).map_err(|source| {
            let _ = std::fs::remove_file(&staging);
            crate::operations::OperationError::ArchiveWrite {
                path: target.to_path_buf(),
                source,
            }
        })?;
        file.sync_all().ok();
    }
    Ok(ArchiveStaging {
        staging_file: staging,
        target_file: target.to_path_buf(),
    })
}

/// Commits a previously staged archive via `rename_no_replace`.
pub fn commit_archive_staging(
    staging: ArchiveStaging,
) -> Result<(), crate::operations::OperationError> {
    match rename_no_replace(&staging.staging_file, &staging.target_file) {
        Ok(()) => Ok(()),
        Err(error) => Err(crate::operations::OperationError::ArchiveWrite {
            path: staging.target_file.clone(),
            source: error,
        }),
    }
}

/// Writes a deterministic archive to `<target>` on disk. Convenience
/// wrapper that stages and immediately commits; callers that need
/// multi-entry atomicity should use `stage_archive_file` and
/// `commit_archive_staging` directly.
pub fn write_archive_file(
    change_id: &str,
    target: &Path,
    bytes: &[u8],
) -> Result<(), crate::operations::OperationError> {
    let staging = stage_archive_file(change_id, target, bytes)?;
    commit_archive_staging(staging)
}

/// Synthesized archive-conversion provenance: identifies the source
/// tree and records that the archive came from a legacy opsx layout
/// rather than a notebook-resident change. Stored as JSON inside
/// the archive under `<change-id>/meta.json`.
fn render_archive_meta(change_id: &str, source_root: &Path) -> Vec<u8> {
    use serde_json::json;
    let payload = json!({
        "change_id": change_id,
        "migrated": true,
        "source_path": source_root.display().to_string(),
        "kind": "legacy-archive",
    });
    serde_json::to_string_pretty(&payload)
        .unwrap_or_default()
        .into_bytes()
}

/// Walks an archive tree recursively, gathering every regular file
/// as an `ArchiveEntry` rooted at `<change-id>/tree/<relative>`.
///
/// R2 (F2): reads files as bytes (`std::fs::read`), not text.
/// The previous `read_to_string` would refuse on non-UTF-8
/// files; the spec requires the archive to be "tarred as-is"
/// with no fake normalization. Bytes round-trip verbatim
/// regardless of encoding.
///
/// R2'' (F2 re-review 2026-07-25): the walker still used
/// `ReadDir::flatten()` (silently dropping iterator errors) and
/// `into_string().ok()` (silently dropping non-UTF-8 filenames);
/// symlinks were silently skipped via `continue`. A successful
/// deterministic archive build proved only the partial inventory
/// it happened to collect, then archive source deletion could
/// discard omitted entries. The walker now fails closed on every
/// iterator error, non-UTF-8 filename, symlink, and unsupported
/// entry type. The archive conversion (and the source deletion
/// that follows) refuses unless the source inventory is
/// completely enumerable.
pub fn collect_archive_entries(tree: &ArchiveTree) -> Result<Vec<ArchiveEntry>, InterchangeError> {
    // R2'''' / F2 second re-review 2026-07-25: the deterministic
    // archive layout stores regular files only; empty source
    // directories would not round-trip through the archive. We
    // refuse the archive build at the source instead of producing
    // an archive whose fidelity proof would necessarily diverge.
    refuse_empty_directories(&tree.root)?;
    let mut entries: Vec<ArchiveEntry> = Vec::new();
    walk_archive_tree(&tree.root, tree.root.as_path(), &mut entries)?;
    Ok(entries)
}

/// Walks the archive source tree and refuses on any empty
/// directory. The deterministic archive layout does not preserve
/// directory entries, so an empty source dir would not round-trip
/// through the archive. Operators must populate or remove the
/// directory before importing.
pub fn refuse_empty_directories(root: &Path) -> Result<(), InterchangeError> {
    walk_for_empty_directories(root, root)
}

fn walk_for_empty_directories(root: &Path, current: &Path) -> Result<(), InterchangeError> {
    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) => {
            return Err(InterchangeError::SourceRead {
                path: current.to_path_buf(),
                source: error,
            });
        }
    };
    let mut children: Vec<PathBuf> = Vec::new();
    let mut has_file = false;
    for entry_result in entries {
        let entry = entry_result.map_err(|source| InterchangeError::SourceRead {
            path: current.to_path_buf(),
            source,
        })?;
        let name =
            entry
                .file_name()
                .into_string()
                .map_err(|os_str| InterchangeError::LayoutAnomaly {
                    path: current.to_path_buf(),
                    message: format!("non-UTF-8 archive filename: {os_str:?}"),
                })?;
        let path = current.join(&name);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|source| InterchangeError::SourceRead {
                path: path.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            return Err(InterchangeError::LayoutAnomaly {
                path: path.clone(),
                message: "symlinks are not part of the deterministic interchange archive; refusing"
                    .to_string(),
            });
        }
        if metadata.is_dir() {
            children.push(path);
        } else if metadata.is_file() {
            has_file = true;
        } else {
            return Err(InterchangeError::LayoutAnomaly {
                path: path.clone(),
                message: format!("unsupported archive entry type: {:?}", metadata.file_type()),
            });
        }
    }
    if !has_file && children.is_empty() && current != root {
        return Err(InterchangeError::LayoutAnomaly {
            path: current.to_path_buf(),
            message: "empty directories are not preserved in the deterministic interchange archive; populate or remove before importing".to_string(),
        });
    }
    for child in children {
        walk_for_empty_directories(root, &child)?;
    }
    Ok(())
}

pub fn walk_archive_tree(
    root: &Path,
    current: &Path,
    entries: &mut Vec<ArchiveEntry>,
) -> Result<(), InterchangeError> {
    let dir_entries =
        std::fs::read_dir(current).map_err(|source| InterchangeError::SourceRead {
            path: current.to_path_buf(),
            source,
        })?;
    let mut names: Vec<String> = Vec::new();
    for entry_result in dir_entries {
        let entry = entry_result.map_err(|source| InterchangeError::SourceRead {
            path: current.to_path_buf(),
            source,
        })?;
        match entry.file_name().into_string() {
            Ok(name) => names.push(name),
            Err(os_str) => {
                return Err(InterchangeError::LayoutAnomaly {
                    path: current.to_path_buf(),
                    message: format!("non-UTF-8 archive filename: {os_str:?}"),
                });
            }
        }
    }
    names.sort();
    for name in names {
        let path = current.join(&name);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|source| InterchangeError::SourceRead {
                path: path.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            return Err(InterchangeError::LayoutAnomaly {
                path: path.clone(),
                message:
                    "symlinks are not part of the deterministic interchange archive; refusing to silently skip"
                        .to_string(),
            });
        }
        if metadata.is_dir() {
            walk_archive_tree(root, &path, entries)?;
        } else if metadata.is_file() {
            let relative =
                path.strip_prefix(root)
                    .map_err(|error| InterchangeError::SourceRead {
                        path: path.clone(),
                        source: std::io::Error::other(error),
                    })?;
            // Use forward slashes so paths are stable across
            // platforms; tar entries are conventionally posix-style.
            let mut posix = PathBuf::from("tree");
            for component in relative.components() {
                let component_str = component.as_os_str().to_str().ok_or_else(|| {
                    InterchangeError::LayoutAnomaly {
                        path: path.clone(),
                        message: format!(
                            "non-UTF-8 archive path component: {:?}",
                            component.as_os_str()
                        ),
                    }
                })?;
                posix.push(component_str);
            }
            // R2 (F2): bytes, not text. Binary files round-trip
            // verbatim per the spec.
            let content = std::fs::read(&path).map_err(|source| InterchangeError::SourceRead {
                path: path.clone(),
                source,
            })?;
            entries.push(ArchiveEntry {
                path: posix,
                content,
            });
        } else {
            return Err(InterchangeError::LayoutAnomaly {
                path: path.clone(),
                message: format!("unsupported archive entry type: {:?}", metadata.file_type()),
            });
        }
    }
    Ok(())
}

/// Converts an archive tree into deterministic tar.zst bytes,
/// preserving the source layout under `<change-id>/tree/` and
/// appending a synthesized `<change-id>/meta.json` provenance
/// record. The result is byte-identical across runs against
/// unchanged inputs (determinism comes from the archive writer's
/// sorted-entries + zeroed-metadata contract).
///
/// # Errors
///
/// Returns [`InterchangeError::SourceRead`] on filesystem failures
/// during the walk and [`InterchangeError::Archive`] on archive
/// construction failures.
pub fn import_archive_tree(tree: &ArchiveTree) -> Result<Vec<u8>, InterchangeError> {
    let mut entries = collect_archive_entries(tree)?;
    let meta = render_archive_meta(&tree.change_id, &tree.root);
    entries.push(ArchiveEntry {
        path: PathBuf::from("meta.json"),
        content: meta,
    });
    // Move every entry under the `<change-id>/` prefix so the
    // archive root owns the change namespace, matching the
    // merge-time archive layout (one archive per change, named
    // by change id).
    let prefixed: Vec<ArchiveEntry> = entries
        .into_iter()
        .map(|mut entry| {
            let mut new_path = PathBuf::from(&tree.change_id);
            new_path.push(entry.path);
            entry.path = new_path;
            entry
        })
        .collect();
    build_archive(&prefixed).map_err(|error: ArchiveError| match error {
        ArchiveError::Build(source) => InterchangeError::SourceRead {
            path: PathBuf::from("archive"),
            source,
        },
        ArchiveError::Extract(source) => InterchangeError::SourceRead {
            path: PathBuf::from("archive"),
            source,
        },
    })
}

/// R2'''' / F2 second re-review 2026-07-25: archive fidelity
/// proof against the **persisted** archive (not the in-memory
/// bytes that may have been written to disk before the source
/// mutated). The proof extracts the persisted archive to a
/// scratch dir, builds the exact file inventory on both sides
/// (path + bytes), and refuses on any divergence. The active-tree
/// canonicalizer (`canonical_bytes_for_proof`) is not applied;
/// archive fidelity is a byte-equal contract, not a
/// normalized-equality contract.
///
/// R2'''''/ F7 third re-review 2026-07-25: the source change
/// root is now passed directly (rather than reconstructed as
/// `<source_root>/<change_id>/`). The quarantine is a renamed
/// sibling that no longer contains a `<change_id>/`
/// subdirectory, so the previous reconstruction was reading the
/// wrong path and any clean archive quarantine could not pass.
/// The empty-dir validation is re-run on the exact quarantine;
/// the build-time check at the original source no longer
/// covers state changes between build and proof.
pub fn archive_fidelity_proof(
    change_id: &str,
    source_change_root: &Path,
    archive_path: &Path,
) -> Result<(), InterchangeError> {
    let archive_bytes =
        std::fs::read(archive_path).map_err(|source| InterchangeError::SourceRead {
            path: archive_path.to_path_buf(),
            source,
        })?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let scratch = std::env::temp_dir().join(format!(
        "nbspec-archive-fidelity-{change_id}-{}-{nanos}",
        std::process::id()
    ));
    let extract_root = scratch.join(change_id);
    if let Err(error) = std::fs::create_dir_all(&scratch) {
        return Err(InterchangeError::SourceRead {
            path: scratch.clone(),
            source: error,
        });
    }
    if let Err(error) = crate::archives::extract_archive(
        &archive_bytes,
        &extract_root,
        Some(&format!("{change_id}/tree/")),
    ) {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(InterchangeError::ArchiveFidelity {
            change_id: change_id.to_string(),
            divergences: vec![format!("cannot extract persisted archive: {error}")],
        });
    }
    // R2''''' third re-review: re-run the empty-dir refusal at
    // proof time on the exact quarantine. The source-side check
    // ran at build time against the original source; an empty
    // dir could have appeared between build and proof and would
    // be captured here.
    if let Err(error) = refuse_empty_directories(source_change_root) {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(InterchangeError::ArchiveFidelity {
            change_id: change_id.to_string(),
            divergences: vec![format!("source rejected at proof time: {error}")],
        });
    }
    // Build exact file inventories (path + bytes, no canonicalization).
    let source_change = source_change_root;
    let mut source_files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut divergences: Vec<String> = Vec::new();
    walk_for_proof(
        source_change,
        source_change,
        "source",
        &mut source_files,
        &mut divergences,
    );
    source_files.sort();
    let mut extracted_files: Vec<(String, Vec<u8>)> = Vec::new();
    walk_for_proof(
        &extract_root,
        &extract_root,
        "extracted",
        &mut extracted_files,
        &mut divergences,
    );
    extracted_files.sort();
    let _ = std::fs::remove_dir_all(&scratch);
    let source_map: std::collections::HashMap<String, Vec<u8>> =
        source_files.iter().cloned().collect();
    let extracted_map: std::collections::HashMap<String, Vec<u8>> =
        extracted_files.iter().cloned().collect();
    for (relative, bytes) in &source_files {
        match extracted_map.get(relative) {
            None => divergences.push(format!("{relative}: missing from archive")),
            Some(extracted) => {
                if bytes != extracted {
                    divergences.push(format!("{relative}: byte divergence"));
                }
            }
        }
    }
    for (relative, _) in &extracted_files {
        if !source_map.contains_key(relative) {
            divergences.push(format!(
                "{relative}: present in archive but missing from source"
            ));
        }
    }
    if !divergences.is_empty() {
        return Err(InterchangeError::ArchiveFidelity {
            change_id: change_id.to_string(),
            divergences,
        });
    }
    Ok(())
}
