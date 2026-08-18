//! Deterministic merge-time change archives.
//!
//! An archive is a zstd-compressed tarball of a change's rendered
//! artifact tree plus its control-plane snapshots (`meta.md`,
//! `work.md`). Determinism is a contract: entries are sorted by
//! path, all header metadata is normalized (zero mtime, uid, and
//! gid; fixed mode), and the compression level is fixed, so
//! identical notebook content produces a byte-identical archive.

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use crate::git_env::scrub_git_env;

/// Fixed zstd compression level. Archives are written rarely and
/// read rarely; favor density. Part of the determinism contract.
const COMPRESSION_LEVEL: i32 = 19;

/// Errors from archive construction.
#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("archive construction failure: {0}")]
    Build(#[from] std::io::Error),

    #[error("archive extraction failure: {0}")]
    Extract(std::io::Error),
}

/// One file within an archive.
///
/// R2 (F2): `content` carries raw bytes, not text. The previous
/// `String` representation lossy-decoded any non-UTF-8 source
/// file through `String::from_utf8_lossy` at archive-build time;
/// the spec requires archives to be tarred as-is with no fake
/// normalization, so binary files must round-trip verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Archive-relative path.
    pub path: PathBuf,
    /// File content (raw bytes).
    pub content: Vec<u8>,
}

/// Builds a deterministic tar + zstd archive from entries. Input
/// order does not matter: entries are sorted by path before packing.
///
/// # Errors
///
/// Returns [`ArchiveError::Build`] when tar packing or zstd
/// compression fails.
pub fn build_archive(entries: &[ArchiveEntry]) -> Result<Vec<u8>, ArchiveError> {
    let mut sorted: Vec<&ArchiveEntry> = entries.iter().collect();
    sorted.sort_by(|first, second| first.path.cmp(&second.path));
    let mut builder = tar::Builder::new(Vec::new());
    for entry in sorted {
        let mut header = tar::Header::new_gnu();
        header.set_size(entry.content.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, &entry.path, entry.content.as_slice())?;
    }
    let tar_bytes = builder.into_inner()?;
    Ok(zstd::encode_all(tar_bytes.as_slice(), COMPRESSION_LEVEL)?)
}

/// Extracts a deterministic tar + zstd archive to `destination`.
///
/// R2''' (F2 re-review 2026-07-25): archive deletion previously
/// relied on the deterministic build as an "implicit fidelity
/// proof" without verifying that the archive actually contains
/// the complete source inventory. We now extract the archive and
/// compare its contents against the source tree before allowing
/// archive source deletion. The function refuses:
/// - entries with absolute paths or `..` components,
/// - entries whose types are not regular files,
/// - entries whose declared size disagrees with the actual
///   payload length,
/// - IO failures while writing.
///
/// When `strip_prefix` is `Some(prefix)`, every entry whose path
/// starts with the prefix is extracted to the remainder of the
/// path; entries that do not start with the prefix are skipped
/// (this matches the merge-time archive layout, which keeps
/// synthesized `<change-id>/meta.json` provenance alongside the
/// `<change-id>/tree/<file>` source files). The proof path uses
/// `strip_prefix = Some("<change-id>/tree/")` so the extracted
/// tree mirrors the source tree for byte-by-byte comparison.
///
/// # Errors
///
/// Returns [`ArchiveError::Extract`] on any tar / zstd decoding
/// or filesystem failure. The `destination` may be partially
/// populated on failure; callers should clean it up.
pub fn extract_archive(
    bytes: &[u8],
    destination: &Path,
    strip_prefix: Option<&str>,
) -> Result<(), ArchiveError> {
    std::fs::create_dir_all(destination).map_err(ArchiveError::Extract)?;
    let decoder = zstd::decode_all(bytes).map_err(ArchiveError::Extract)?;
    let mut archive = tar::Archive::new(decoder.as_slice());
    let entries = archive.entries().map_err(ArchiveError::Extract)?;
    for entry in entries {
        let mut entry = entry.map_err(ArchiveError::Extract)?;
        let path = entry.path().map_err(ArchiveError::Extract)?.into_owned();
        if path.is_absolute() {
            return Err(ArchiveError::Extract(std::io::Error::other(format!(
                "archive entry has absolute path: {}",
                path.display()
            ))));
        }
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                return Err(ArchiveError::Extract(std::io::Error::other(format!(
                    "archive entry escapes via parent: {}",
                    path.display()
                ))));
            }
        }
        let header = entry.header().clone();
        if !header.entry_type().is_file() {
            return Err(ArchiveError::Extract(std::io::Error::other(format!(
                "archive entry is not a regular file: {} (type {:?})",
                path.display(),
                header.entry_type()
            ))));
        }
        let declared_size = header.size().map_err(ArchiveError::Extract)?;
        let relative = match strip_prefix {
            Some(prefix) => match path.to_str() {
                Some(p) if p.starts_with(prefix) => &p[prefix.len()..],
                Some(_) => continue,
                None => {
                    return Err(ArchiveError::Extract(std::io::Error::other(format!(
                        "non-UTF-8 archive entry path: {}",
                        path.display()
                    ))));
                }
            },
            None => path.to_str().ok_or_else(|| {
                ArchiveError::Extract(std::io::Error::other(format!(
                    "non-UTF-8 archive entry path: {}",
                    path.display()
                )))
            })?,
        };
        let relative_path = std::path::Path::new(relative);
        let absolute = destination.join(relative_path);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).map_err(ArchiveError::Extract)?;
        }
        let mut file = std::fs::File::create(&absolute).map_err(ArchiveError::Extract)?;
        let copied = std::io::copy(&mut entry, &mut file).map_err(ArchiveError::Extract)?;
        if copied != declared_size {
            return Err(ArchiveError::Extract(std::io::Error::other(format!(
                "archive entry size mismatch at {}: declared {declared_size}, copied {copied}",
                path.display()
            ))));
        }
    }
    Ok(())
}

/// Reports whether Git attributes mark `path` for Git LFS, by asking
/// `git check-attr filter` in the project root. Delegating to git
/// gives full gitattributes semantics — nested `.gitattributes`
/// files along the path, precedence, macros, `core.attributesFile` —
/// without hand-rolled pattern matching or a git library dependency.
/// Outside a git repository (or without git available) nothing is
/// LFS-tracked, so the answer is `false`.
pub fn gitattributes_covers_lfs(project_root: &Path, path: &Path) -> bool {
    let mut command = Command::new("git");
    scrub_git_env(&mut command);
    let Ok(output) = command
        .args(["check-attr", "filter", "--"])
        .arg(path)
        .current_dir(project_root)
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    // Output format: `<path>: filter: <value>`.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.ends_with(": filter: lfs"))
}
