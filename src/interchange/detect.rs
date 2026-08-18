use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use crate::changes::validate_change_id;

/// Filename of the proposal document in an active change tree.
#[allow(dead_code)] // used by task 3.x
pub const ACTIVE_PROPOSAL_FILE: &str = "proposal.md";

/// Filename of the tasks document in an active change tree.
#[allow(dead_code)] // used by task 3.x
pub const ACTIVE_TASKS_FILE: &str = "tasks.md";

/// Filename of the design document in an active change tree.
#[allow(dead_code)] // used by task 3.x
pub const ACTIVE_DESIGN_FILE: &str = "design.md";

/// Subfolder holding capability specs in an active change tree.
#[allow(dead_code)] // used by task 3.x
pub const ACTIVE_SPECS_DIR: &str = "specs";

/// Subfolder holding decision records in an active change tree.
#[allow(dead_code)] // used by task 3.x
pub const ACTIVE_DECISIONS_DIR: &str = "decisions";

/// Legacy archive subpath beneath the `openspec/` directory.
pub const LEGACY_ARCHIVE_SUBDIR: &str = "changes/archive";

/// R7''''' / F7 third re-review 2026-07-25: process-local counter
/// for collision-safe quarantine and staging paths. Combined with
/// nanos + pid in the suffix, this closes any residual collision
/// window when nanos coincide across processes or within tight
/// loops in a single process. Exclusive creation (rather than
/// `create_dir_all`) is the final guard.
pub static QUARANTINE_COUNTER: AtomicU64 = AtomicU64::new(0);
pub static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What kind of filesystem tree a path corresponds to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectedTree {
    /// `<root>/<change-id>/{proposal.md, specs/<cap>/spec.md,
    /// design.md, tasks.md, decisions/<adr>.md}` — a live, authored
    /// change in the upstream OpenSpec filesystem shape.
    Active(ActiveTree),
    /// `<root>/openspec/changes/archive/<change-id>/...` — a legacy
    /// archive tree that converts to a deterministic tar.zst under
    /// `documentation/archives/`.
    Archive(ArchiveTree),
}

/// A detected active change tree at `<root>/<change-id>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveTree {
    /// `<root>/<change-id>` — absolute path to the change folder.
    pub root: PathBuf,
    /// Change identifier (the `<change-id>` folder name).
    pub change_id: String,
}

/// A detected legacy archive tree at
/// `<root>/openspec/changes/archive/<change-id>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveTree {
    /// `<root>/openspec/changes/archive/<change-id>` — absolute path
    /// to the legacy archive folder.
    pub root: PathBuf,
    /// Change identifier (the `<change-id>` folder name).
    pub change_id: String,
}

/// Atomically renames `src` to `dst` without replacing an
/// existing destination. On Linux, this uses `renameat2` with
/// `RENAME_NOREPLACE`, which is atomic and concurrent-occupant
/// safe. Elsewhere, falls back to a check-then-rename sequence
/// with a documented small race window (the publish re-check
/// closes the practical exposure on non-Linux systems; the race
/// window between the check and the rename is microseconds).
///
/// R7'''' / F7 third re-review 2026-07-25: addresses the F7
/// structural blocker that "non-overwrite export still samples
/// before staging; a target appearing before publication can be
/// backed up/replaced without authorization." With this helper
/// the non-overwrite path returns `EEXIST` deterministically
/// when an occupant appears at the publish boundary.
#[cfg(target_os = "linux")]
pub fn rename_no_replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let src_c = CString::new(src.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    // Safety: documented `libc::renameat2` invocation with
    // `AT_FDCWD` for both directories and `RENAME_NOREPLACE` to
    // fail with EEXIST if `dst` already exists. The C strings
    // are constructed from valid Rust paths with no interior NULs.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            src_c.as_ptr(),
            libc::AT_FDCWD,
            dst_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
pub fn rename_no_replace(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    // R7_fourth re-review 2026-07-25: the previous check-then-rename
    // fallback kept a residual race window between the `dst.exists()`
    // sample and the actual `rename` call. There is no portable
    // atomic no-replace rename on non-Linux platforms without
    // `renameat2`. Refuse explicitly so the operator can fail
    // closed rather than discover the race after the fact; the
    // publish re-check at the publish boundary is the wider guard
    // and remains in place.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is not available on this platform; \
         refusing rather than risking a race",
    ))
}

/// Creates a directory at `path` exclusively: returns an error if
/// the directory already exists. This is the final collision guard
/// after the suffix uniqueness (counter + nanos + pid).
///
/// R7''''' / F7 third re-review 2026-07-25.
pub fn create_dir_exclusive(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

/// Derives a sibling quarantine path for an import source tree.
///
/// R7'' / F7 re-review 2026-07-25: source deletion is staged via
/// rename-to-quarantine. The quarantine is a sibling of the
/// source path (same parent directory) so `std::fs::rename` is
/// atomic on POSIX.
///
/// R7''''' / F7 third re-review 2026-07-25: the suffix appends
/// `.quarantine-<nanos>-<pid>-<counter>` so concurrent
/// invocations within the same process (where nanos may collide
/// in tight loops) and across processes both avoid collisions.
/// The exclusive creation step (`create_dir_exclusive`) closes
/// any residual collision window.
pub fn quarantine_path_for(
    source: &Path,
) -> Result<PathBuf, crate::interchange::plan::InterchangeError> {
    let parent = source.parent().ok_or_else(|| {
        crate::interchange::plan::InterchangeError::LayoutAnomaly {
            path: source.to_path_buf(),
            message: "source path has no parent; cannot derive a quarantine".to_string(),
        }
    })?;
    let file_name = source.file_name().ok_or_else(|| {
        crate::interchange::plan::InterchangeError::LayoutAnomaly {
            path: source.to_path_buf(),
            message: "source path has no file name; cannot derive a quarantine".to_string(),
        }
    })?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut quarantine = parent.to_path_buf();
    quarantine.push(format!(
        "{}.quarantine-{}-{}-{}",
        file_name.to_string_lossy(),
        nanos,
        std::process::id(),
        QUARANTINE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    Ok(quarantine)
}

/// Detection walks `root` for both kinds of trees in a single pass,
/// skipping anything that fails structural validation (an active
/// tree without `proposal.md`, a folder whose name is not a
/// kebab-case change id, or an `openspec/changes/archive/` entry
/// that is not a directory). The result is sorted by change id so
/// plan output is deterministic across runs.
pub fn detect_trees(root: &Path) -> Vec<DetectedTree> {
    let mut trees: Vec<DetectedTree> = Vec::new();

    if !root.is_dir() {
        return trees;
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return trees,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        // The `openspec/` subdirectory is reserved for archive
        // detection; it is never itself an active tree, even when
        // it accidentally contains a `proposal.md` (e.g. a
        // misconfigured repo whose opsx root is the import root).
        if name == "openspec" {
            trees.extend(detect_archive_trees(&path));
            continue;
        }
        if let Some(active) = detect_active_tree(&path, &name) {
            trees.push(DetectedTree::Active(active));
        }
    }

    trees.sort_by(|left, right| {
        let left_id = match left {
            DetectedTree::Active(tree) => &tree.change_id,
            DetectedTree::Archive(tree) => &tree.change_id,
        };
        let right_id = match right {
            DetectedTree::Active(tree) => &tree.change_id,
            DetectedTree::Archive(tree) => &tree.change_id,
        };
        left_id.cmp(right_id)
    });
    trees
}

/// Walks `<root>/openspec/changes/archive/` for archive tree
/// candidates. Each immediate subdirectory is taken as an archive
/// tree; non-directory entries and entries with non-kebab-case
/// names are silently skipped. Returns detections in change-id
/// order.
fn detect_archive_trees(openspec_root: &Path) -> Vec<DetectedTree> {
    let archive_root = openspec_root.join(LEGACY_ARCHIVE_SUBDIR);
    let mut trees: Vec<DetectedTree> = Vec::new();
    let entries = match std::fs::read_dir(&archive_root) {
        Ok(entries) => entries,
        Err(_) => return trees,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        if validate_change_id(&name).is_err() {
            continue;
        }
        trees.push(DetectedTree::Archive(ArchiveTree {
            root: path,
            change_id: name,
        }));
    }
    trees
}

/// Detects an active change tree at `<root>/<change-id>/`.
/// Returns `None` when the folder name is not a valid kebab-case
/// change id or the folder does not carry `proposal.md`. Detection
/// is purely structural; parse failures inside `tasks.md` or
/// capability files are surfaced as plan refusals by the import
/// path (see tasks 3.x), not here.
fn detect_active_tree(folder: &Path, name: &str) -> Option<ActiveTree> {
    if validate_change_id(name).is_err() {
        return None;
    }
    let proposal = folder.join(ACTIVE_PROPOSAL_FILE);
    if !proposal.is_file() {
        return None;
    }
    Some(ActiveTree {
        root: folder.to_path_buf(),
        change_id: name.to_string(),
    })
}
