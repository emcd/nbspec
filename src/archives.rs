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
}

/// One file within an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Archive-relative path.
    pub path: PathBuf,
    /// File content.
    pub content: String,
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
        builder.append_data(&mut header, &entry.path, entry.content.as_bytes())?;
    }
    let tar_bytes = builder.into_inner()?;
    Ok(zstd::encode_all(tar_bytes.as_slice(), COMPRESSION_LEVEL)?)
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
