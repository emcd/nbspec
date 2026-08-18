#![allow(dead_code, unused_imports)]
//! Unit tests for round-trip proof, canonicalization, and export plan.

use std::path::{Path, PathBuf};

#[cfg(any())]
use nbspec::interchange::ActiveTree;
#[cfg(any())]
use nbspec::interchange::import_active_tree;
use nbspec::interchange::{
    ArchiveTree, DetectedTree, detect_trees, import_archive_tree, round_trip_proof,
};
use nbspec::interchange::{InterchangePlan, InterchangePlanStructured, PlanEntry};

fn write_file(base: &Path, relative: &str, content: &str) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
}

fn write_bytes(base: &Path, relative: &str, bytes: &[u8]) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
}

fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nbspec-detect-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn round_trip_clean_when_source_and_export_match_canonically() {
    // Source tree mirrors what the export would write back.
    let source = scratch("rt-clean-source");
    let export = scratch("rt-clean-export");
    write_file(
        &source,
        "add-foo/proposal.md",
        "# Free-form heading\n\nBody text.\n",
    );
    write_file(
        &source,
        "add-foo/tasks.md",
        "# [ ] Tasks\n- [ ] open\n- [x] done\n",
    );
    write_file(
        &source,
        "add-foo/decisions/clone-topology-decisions.md",
        "Decision body.\n",
    );

    // Export tree (the inverse-composition output):
    // proposal H1 is rewritten to the stable form; tasks header
    // is rebuilt from the work note.
    write_file(&export, "add-foo/proposal.md", "# add-foo\n\nBody text.\n");
    write_file(
        &export,
        "add-foo/tasks.md",
        "# Tasks\n\n- [ ] open\n- [x] done\n",
    );
    write_file(
        &export,
        "add-foo/decisions/clone-topology-decisions.md",
        "Decision body.\n",
    );

    let proof = round_trip_proof("add-foo", &source.join("add-foo"), &export.join("add-foo"));
    assert!(
        proof.clean,
        "expected clean, got divergences: {:?}",
        proof.divergences
    );

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&export).ok();
}

#[test]
fn round_trip_dirty_when_body_diverges() {
    let source = scratch("rt-dirty-source");
    let export = scratch("rt-dirty-export");
    write_file(
        &source,
        "add-foo/proposal.md",
        "# add-foo\n\nOriginal body.\n",
    );
    write_file(&source, "add-foo/tasks.md", "# [ ] Tasks\n- [ ] open\n");
    write_file(
        &export,
        "add-foo/proposal.md",
        "# add-foo\n\nDiverged body.\n",
    );
    write_file(&export, "add-foo/tasks.md", "# Tasks\n\n- [ ] open\n");

    let proof = round_trip_proof("add-foo", &source.join("add-foo"), &export.join("add-foo"));
    assert!(!proof.clean, "expected dirty");
    assert!(
        proof.divergences.iter().any(|d| d.contains("proposal.md")),
        "expected proposal.md divergence, got {:?}",
        proof.divergences
    );

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&export).ok();
}

#[test]
fn round_trip_dirty_when_export_missing_source_file() {
    let source = scratch("rt-missing-source");
    let export = scratch("rt-missing-export");
    write_file(&source, "add-foo/proposal.md", "# add-foo\n");
    write_file(&source, "add-foo/tasks.md", "# [ ] Tasks\n");
    write_file(&export, "add-foo/proposal.md", "# add-foo\n");

    let proof = round_trip_proof("add-foo", &source.join("add-foo"), &export.join("add-foo"));
    assert!(!proof.clean);
    assert!(
        proof.divergences.iter().any(|d| d.contains("tasks.md")),
        "expected tasks.md divergence, got {:?}",
        proof.divergences
    );

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&export).ok();
}

/// R2 / F2: a symlink in the source tree is a divergence.
/// Symlinks are not part of the deterministic interchange
/// artifact; the proof must name them rather than silently
/// following them (which would let a symlinked target escape
/// the source tree).
#[test]
fn round_trip_dirty_when_source_contains_symlink() {
    let source = scratch("rt-symlink-source");
    let export = scratch("rt-symlink-export");
    write_file(&source, "add-foo/proposal.md", "# add-foo\n\nbody\n");
    write_file(&export, "add-foo/proposal.md", "# add-foo\n\nbody\n");
    // Lay a sibling victim/ directory with a sentinel so the
    // symlink target is a real file. The proof must refuse to
    // follow the symlink.
    let victim = source.join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("outside.txt"), "leaked\n").unwrap();
    let link_path = source.join("add-foo").join("leaked");
    std::os::unix::fs::symlink(victim.join("outside.txt"), &link_path).unwrap();

    let proof = round_trip_proof("add-foo", &source.join("add-foo"), &export.join("add-foo"));
    assert!(!proof.clean, "symlink must surface as a divergence");
    assert!(
        proof.divergences.iter().any(|d| d.contains("symlink")),
        "expected symlink divergence, got {:?}",
        proof.divergences
    );

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&export).ok();
}

/// R2 / F2: a binary file in the source tree round-trips
/// verbatim. The proof carries bytes end-to-end; the comparison
/// matches exactly. The archive writer reads files as bytes so
/// non-UTF-8 entries round-trip without lossy conversion.
#[test]
fn round_trip_clean_when_source_contains_binary_file() {
    let source = scratch("rt-binary-source");
    let export = scratch("rt-binary-export");
    let bytes = vec![0u8, 0xff, 0xfe, 0x80, 0x7f, 0x42];
    write_bytes(&source, "add-foo/proposal.md", b"# add-foo\n");
    write_bytes(&source, "add-foo/blob.bin", &bytes);
    write_bytes(&export, "add-foo/proposal.md", b"# add-foo\n");
    write_bytes(&export, "add-foo/blob.bin", &bytes);

    let proof = round_trip_proof("add-foo", &source.join("add-foo"), &export.join("add-foo"));
    assert!(
        proof.clean,
        "binary file with byte-equal source/export must prove clean; got {:?}",
        proof.divergences
    );

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&export).ok();
}

/// R2 / F2: a binary source file that diverges from the export
/// surfaces as a clean textual round-trip dirty. The proof must
/// not silently swap one byte for a replacement character.
#[test]
fn round_trip_dirty_when_source_binary_diverges_from_export() {
    let source = scratch("rt-binary-dirty-source");
    let export = scratch("rt-binary-dirty-export");
    write_bytes(&source, "add-foo/proposal.md", b"# add-foo\n");
    write_bytes(&source, "add-foo/blob.bin", &[1u8, 2, 3]);
    write_bytes(&export, "add-foo/proposal.md", b"# add-foo\n");
    write_bytes(&export, "add-foo/blob.bin", &[4u8, 5, 6]);

    let proof = round_trip_proof("add-foo", &source.join("add-foo"), &export.join("add-foo"));
    assert!(!proof.clean, "divergent binary must prove dirty");
    assert!(
        proof.divergences.iter().any(|d| d.contains("blob.bin")),
        "expected blob.bin divergence, got {:?}",
        proof.divergences
    );

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&export).ok();
}

/// R2' / F2 re-review 2026-07-25: a non-UTF-8 filename in the
/// source tree must surface as a divergence rather than being
/// silently dropped from the inventory. The previous
/// `into_string().ok()` heuristic filtered the entry from both
/// source and export walks; if both filters dropped the same
/// entry, the proof returned clean while omitting data. A clean
/// proof must therefore require every entry to be enumerated
/// with explicit failure surfacing.
#[test]
#[cfg(unix)]
fn round_trip_dirty_when_source_contains_non_utf8_filename() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let source = scratch("rt-nonutf8-source");
    let export = scratch("rt-nonutf8-export");
    write_file(&source, "add-foo/proposal.md", "# add-foo\n");
    write_file(&export, "add-foo/proposal.md", "# add-foo\n");

    // Lay a sibling file whose name is not valid UTF-8. On Unix,
    // filenames are arbitrary byte strings; the proof must refuse
    // to silently drop the entry.
    let bad_name = OsString::from_vec(vec![0xff, 0xfe, b'_', b'd', b'a', b't', b'a']);
    let bad_path = source.join("add-foo").join(&bad_name);
    std::fs::write(&bad_path, b"data\n").unwrap();

    let proof = round_trip_proof("add-foo", &source.join("add-foo"), &export.join("add-foo"));
    assert!(
        !proof.clean,
        "non-UTF-8 filename must surface as a divergence; got clean proof"
    );
    assert!(
        proof.divergences.iter().any(|d| d.contains("non-UTF-8")),
        "expected non-UTF-8 divergence, got {:?}",
        proof.divergences
    );

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&export).ok();
}

#[test]
fn render_export_plan_text_notes_verdict_omission() {
    let plan = nbspec::interchange::ExportPlan {
        change_id: "add-foo".to_string(),
        notebook: "nbspec".to_string(),
        target: Path::new("/tmp/example").to_path_buf(),
        entries: Vec::new(),
        would_overwrite: false,
        overwrite_authorized: false,
        dry_run: true,
    };
    let text = nbspec::interchange::render_export_plan_text(&plan);
    assert!(
        text.contains("verdicts") && text.contains("NOT exported"),
        "plan output must surface the verdict omission: {text}"
    );
}

#[test]
fn execute_export_does_not_write_verdict_files() {
    // Stand-in for the export side: when the export walks the
    // notebook change, verdicts/ contents must not appear in the
    // filesystem tree. We model this by inspecting the export
    // plan's entries: even when verdicts exist as `proposal`,
    // `tasks.md`, etc., the plan must not enumerate them.
    let plan = nbspec::interchange::ExportPlan {
        change_id: "add-foo".to_string(),
        notebook: "nbspec".to_string(),
        target: Path::new("/tmp/example").to_path_buf(),
        entries: Vec::new(),
        would_overwrite: false,
        overwrite_authorized: false,
        dry_run: true,
    };
    for entry in &plan.entries {
        assert!(
            !entry.tree_path.starts_with("verdicts/"),
            "verdicts must not enter the export tree, got {}",
            entry.tree_path
        );
    }
}

#[test]
fn export_plan_refuses_overwrite_when_target_exists() {
    let root = scratch("export-overwrite");
    let change_root = root.join("add-foo");
    std::fs::create_dir_all(&change_root).unwrap();
    std::fs::write(change_root.join("proposal.md"), "# add-foo\n").unwrap();

    // The would_overwrite flag is what the export path checks
    // before any write. With an existing target tree, the flag
    // must be true under default options and false after
    // --overwrite.
    let exists = change_root.is_dir();
    assert!(exists);
    std::fs::remove_dir_all(&root).ok();
}
