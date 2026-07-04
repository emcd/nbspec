//! Deterministic merge-time change archives.
//!
//! An archive is a zstd-compressed tarball of a change's rendered
//! artifact tree plus its control-plane snapshots (`meta.md`,
//! `work.md`). Determinism is a contract: entries are sorted by
//! path, all header metadata is normalized (zero mtime, uid, and
//! gid; fixed mode), and the compression level is fixed, so
//! identical notebook content produces a byte-identical archive.

use std::path::{Path, PathBuf};

use thiserror::Error;

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

/// Reports whether the project `.gitattributes` marks `path` for Git
/// LFS. Understands the pattern subset LFS rules use in practice —
/// literal paths, `*`, `?`, and `**` (with basename matching for
/// slash-free patterns) — not full gitattributes semantics.
pub fn gitattributes_covers_lfs(project_root: &Path, path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(project_root.join(".gitattributes")) else {
        return false;
    };
    let path = path.to_string_lossy();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((pattern, attributes)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if !attributes
            .split_whitespace()
            .any(|attribute| attribute == "filter=lfs")
        {
            continue;
        }
        if pattern_matches(pattern, &path) {
            return true;
        }
    }
    false
}

/// Matches a gitattributes-style pattern against a repository-
/// relative path: slash-free patterns match the basename, patterns
/// with slashes match the whole path.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    if pattern.contains('/') {
        let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
        glob_match(pattern.as_bytes(), path.as_bytes())
    } else {
        let basename = path.rsplit('/').next().unwrap_or(path);
        glob_match(pattern.as_bytes(), basename.as_bytes())
    }
}

/// Glob matcher: `**` crosses path segments, `*` and `?` do not.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    if let Some(rest) = pattern.strip_prefix(b"**") {
        // `**/` may also consume nothing (match zero directories).
        let rest = rest.strip_prefix(b"/").unwrap_or(rest);
        return (0..=text.len()).any(|split| {
            (split == 0 || text[split - 1] == b'/' || split == text.len())
                && glob_match(rest, &text[split..])
        });
    }
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(b'*'), _) => (0..=text.len()).any(|length| {
            !text[..length].contains(&b'/') && glob_match(&pattern[1..], &text[length..])
        }),
        (Some(b'?'), Some(&character)) => {
            character != b'/' && glob_match(&pattern[1..], &text[1..])
        }
        (Some(&expected), Some(&character)) => {
            expected == character && glob_match(&pattern[1..], &text[1..])
        }
        (Some(_), None) => false,
    }
}
