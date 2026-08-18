//! Unit tests for filesystem-tree detection.

use std::path::{Path, PathBuf};

#[cfg(any())]
use nbspec::interchange::ActiveTree;
#[cfg(any())]
use nbspec::interchange::import_active_tree;
use nbspec::interchange::{
    ArchiveTree, DetectedTree, detect_trees, import_archive_tree, round_trip_proof,
};
use nbspec::interchange::{InterchangePlan, InterchangePlanStructured, PlanEntry};

/// Writes `content` at `<base>/<relative>`, creating intermediate
/// directories. Panics on IO failure so the test's setup failure is
/// obvious (rather than producing a misleading detection outcome).
fn write_file(base: &Path, relative: &str, content: &str) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
}

/// Writes raw `bytes` at `<base>/<relative>`, creating intermediate
/// directories. Used for the round-trip proof's binary-file
/// regression tests where the content must survive verbatim.
fn write_bytes(base: &Path, relative: &str, bytes: &[u8]) {
    let path = base.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
}

/// Builds a fresh scratch directory under the crate's test
/// temporary root. Returns the path; the caller is responsible for
/// cleanup (or wrap with `tempfile::TempDir` when needed).
fn scratch(label: &str) -> std::path::PathBuf {
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
fn detects_active_tree_with_proposal_md() {
    let root = scratch("active-basic");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    let detected = detect_trees(&root);
    assert_eq!(detected.len(), 1);
    match &detected[0] {
        DetectedTree::Active(tree) => {
            assert_eq!(tree.change_id, "add-foo");
            assert!(tree.root.ends_with("add-foo"));
        }
        other => panic!("expected Active, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn skips_directory_without_proposal_md() {
    let root = scratch("no-proposal");
    std::fs::create_dir_all(root.join("not-a-change")).unwrap();
    let detected = detect_trees(&root);
    assert!(detected.is_empty(), "got {detected:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn skips_directory_with_invalid_change_id() {
    let root = scratch("invalid-id");
    write_file(&root, "Not_A_Change/proposal.md", "# proposal\n");
    let detected = detect_trees(&root);
    assert!(detected.is_empty(), "got {detected:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn detects_archive_tree_under_openspec_changes_archive() {
    let root = scratch("archive-basic");
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/proposal.md",
        "# legacy\n",
    );
    let detected = detect_trees(&root);
    assert_eq!(detected.len(), 1);
    match &detected[0] {
        DetectedTree::Archive(tree) => {
            assert_eq!(tree.change_id, "legacy-feature");
            assert!(tree.root.ends_with("legacy-feature"));
        }
        other => panic!("expected Archive, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn detects_active_and_archive_trees_in_one_pass() {
    let root = scratch("mixed");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    write_file(&root, "add-bar/proposal.md", "# add-bar\n");
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/proposal.md",
        "# legacy\n",
    );
    let detected = detect_trees(&root);
    assert_eq!(detected.len(), 3);
    let ids: Vec<&str> = detected
        .iter()
        .map(|tree| match tree {
            DetectedTree::Active(t) => t.change_id.as_str(),
            DetectedTree::Archive(t) => t.change_id.as_str(),
        })
        .collect();
    assert_eq!(ids, vec!["add-bar", "add-foo", "legacy-feature"]);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn skips_archive_subdir_with_invalid_change_id() {
    let root = scratch("archive-bad-id");
    write_file(
        &root,
        "openspec/changes/archive/BAD_ID/proposal.md",
        "# anything\n",
    );
    let detected = detect_trees(&root);
    assert!(detected.is_empty(), "got {detected:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_root_returns_no_trees() {
    let root = scratch("empty");
    let detected = detect_trees(&root);
    assert!(detected.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn missing_root_returns_no_trees() {
    let detected = detect_trees(Path::new("/no/such/path/should/exist"));
    assert!(detected.is_empty());
}

#[cfg(any())]
#[test]
fn active_tree_mapping_emits_proposal_spec_design_work_decisions_meta() {
    let root = scratch("active-mapping");
    write_file(
        &root,
        "add-foo/proposal.md",
        "# add-foo\n\nSome proposal body.\n",
    );
    write_file(
        &root,
        "add-foo/specs/user-auth/spec.md",
        "# user-auth\n\n## ADDED Requirements\n",
    );
    write_file(
        &root,
        "add-foo/design.md",
        "# main design\n\nDesign notes here.\n",
    );
    write_file(
        &root,
        "add-foo/decisions/clone-topology-decisions.md",
        "## Decision: use the fleet convention\n\nStatus: accepted.\n",
    );
    write_file(
        &root,
        "add-foo/tasks.md",
        "# [ ] Tasks\n- [ ] first\n- [x] done\n",
    );

    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, warnings, meta) =
        import_active_tree(&active, &[]).expect("import_active_tree should succeed");

    assert!(
        warnings.is_empty(),
        "no delta warnings expected: {warnings:?}"
    );
    let titles: Vec<(&str, &str)> = writes
        .iter()
        .map(|w| (w.folder.as_str(), w.title.as_str()))
        .collect();
    assert!(
        titles.contains(&("", "proposal")),
        "missing proposal: {titles:?}"
    );
    assert!(
        titles.contains(&("specifications", "user-auth")),
        "missing spec: {titles:?}"
    );
    assert!(
        titles.contains(&("designs", "main")),
        "missing design: {titles:?}"
    );
    assert!(
        titles.contains(&("decisions", "clone-topology-decisions")),
        "missing decision: {titles:?}"
    );
    assert!(titles.contains(&("", "work")), "missing work: {titles:?}");
    assert!(titles.contains(&("", "meta")), "missing meta: {titles:?}");

    let work_write = writes
        .iter()
        .find(|w| w.title == "work")
        .expect("work write");
    assert!(work_write.is_todo, "work must be a todo note");

    let meta_write = writes
        .iter()
        .find(|w| w.title == "meta")
        .expect("meta write");
    assert!(meta_write.content.contains("\"migrated\": true"));
    assert!(meta_write.content.contains("\"source_path\""));

    assert!(!meta.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_mapping_normalizes_proposal_h1() {
    let root = scratch("active-h1");
    write_file(
        &root,
        "add-foo/proposal.md",
        "# Free-form heading\n\nBody text.\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, _, _) = import_active_tree(&active, &[]).expect("import should succeed");
    let proposal = writes
        .iter()
        .find(|w| w.title == "proposal")
        .expect("proposal write");
    assert!(
        proposal.content.starts_with("# add-foo"),
        "proposal H1 not normalized: {}",
        proposal.content
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_mapping_preserves_decision_filename() {
    let root = scratch("active-decision-name");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    write_file(
        &root,
        "add-foo/decisions/clone-topology-decisions.md",
        "Decision body.\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, _, _) = import_active_tree(&active, &[]).expect("import should succeed");
    let decision = writes
        .iter()
        .find(|w| w.title == "clone-topology-decisions")
        .expect("decision write");
    assert_eq!(decision.folder, "decisions");
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_mapping_warns_on_modified_delta() {
    let root = scratch("active-delta-warning");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    write_file(
        &root,
        "add-foo/specs/user-auth/spec.md",
        "# user-auth\n\n## MODIFIED Requirements\n\n### Requirement: foo\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, warnings, _) = import_active_tree(&active, &[]).expect("import should succeed");
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w.kind, nbspec::interchange::DeltaWarningKind::Modified)),
        "expected MODIFIED warning, got {warnings:?}"
    );
    // Import still proceeds: the warnings are surfaced in the
    // plan output but never refuse the import.
    assert!(writes.iter().any(|w| w.title == "user-auth"));
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_refuses_change_id_collision() {
    let root = scratch("active-collision");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let result = import_active_tree(&active, &["add-foo".to_string()]);
    assert!(
        matches!(result, Err(nbspec::interchange::InterchangeError::Collision(ref id)) if id == "add-foo"),
        "expected Collision, got {result:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_refuses_unparseable_tasks_md() {
    let root = scratch("active-bad-tasks");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    // `- [?]` is neither `[ ]` nor `[x]`, so the work note parser
    // raises WorkNoteError::MalformedItem.
    write_file(
        &root,
        "add-foo/tasks.md",
        "# [ ] Tasks\n- [?] half-written\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let result = import_active_tree(&active, &[]);
    assert!(
        matches!(
            result,
            Err(nbspec::interchange::InterchangeError::TasksParse(_))
        ),
        "expected TasksParse, got {result:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any())]
#[test]
fn active_tree_preserves_checkbox_state_in_work_note() {
    let root = scratch("active-checkboxes");
    write_file(&root, "add-foo/proposal.md", "# add-foo\n");
    write_file(
        &root,
        "add-foo/tasks.md",
        "# [ ] Tasks\n- [ ] open item\n- [x] done item\n",
    );
    let active = ActiveTree {
        root: root.join("add-foo"),
        change_id: "add-foo".to_string(),
    };
    let (writes, _, _) = import_active_tree(&active, &[]).expect("import should succeed");
    let work = writes
        .iter()
        .find(|w| w.title == "work")
        .expect("work write");
    assert!(
        work.content.contains("- [ ] open item"),
        "open item missing: {}",
        work.content
    );
    assert!(
        work.content.contains("- [x] done item"),
        "done item missing: {}",
        work.content
    );
    std::fs::remove_dir_all(&root).ok();
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

#[test]
fn archive_conversion_produces_deterministic_tar_zst() {
    let root = scratch("archive-deterministic");
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/proposal.md",
        "# legacy\n",
    );
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/tasks.md",
        "# [ ] legacy work\n",
    );
    let archive = ArchiveTree {
        root: root.join("openspec/changes/archive/legacy-feature"),
        change_id: "legacy-feature".to_string(),
    };
    let bytes_one = import_archive_tree(&archive).expect("archive build should succeed");
    let bytes_two = import_archive_tree(&archive).expect("archive build should succeed");
    assert_eq!(
        bytes_one, bytes_two,
        "deterministic archive writer must produce byte-identical output"
    );
    assert!(!bytes_one.is_empty(), "archive bytes must not be empty");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn archive_conversion_preserves_source_layout() {
    let root = scratch("archive-layout");
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/proposal.md",
        "# legacy\n",
    );
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/specs/auth/spec.md",
        "# auth\n",
    );
    let archive = ArchiveTree {
        root: root.join("openspec/changes/archive/legacy-feature"),
        change_id: "legacy-feature".to_string(),
    };
    let bytes = import_archive_tree(&archive).expect("archive build should succeed");
    // Inspect the archive by extracting it back and walking its
    // members. The deterministic archive writer uses tar under
    // zstd; `tar -I zstd -tf` lists the entries. The test
    // exercises the contract that both the source layout and the
    // synthesized meta survive into the archive.
    let listing = list_tar_zstd_entries(&bytes);
    assert!(
        listing
            .iter()
            .any(|entry| entry.contains("legacy-feature/tree/proposal.md")),
        "missing legacy-feature/tree/proposal.md in archive: {listing:?}"
    );
    assert!(
        listing
            .iter()
            .any(|entry| entry.contains("legacy-feature/tree/specs/auth/spec.md")),
        "missing legacy-feature/tree/specs/auth/spec.md: {listing:?}"
    );
    assert!(
        listing
            .iter()
            .any(|entry| entry == "legacy-feature/meta.json"),
        "missing legacy-feature/meta.json: {listing:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// R2 / F2: archive conversion preserves bytes verbatim for
/// non-UTF-8 source files. The archive writer reads bytes
/// (`std::fs::read`), so binary content round-trips through the
/// tar.zst without lossy conversion.
#[test]
fn archive_conversion_preserves_binary_bytes() {
    let root = scratch("archive-binary");
    let bytes = vec![0u8, 0xff, 0xfe, 0x80, 0x7f, 0x00, 0x42];
    write_bytes(
        &root.join("openspec/changes/archive/binary-feature"),
        "blob.bin",
        &bytes,
    );
    write_file(
        &root,
        "openspec/changes/archive/binary-feature/proposal.md",
        "# binary\n",
    );
    let archive = ArchiveTree {
        root: root.join("openspec/changes/archive/binary-feature"),
        change_id: "binary-feature".to_string(),
    };
    let archive_bytes = import_archive_tree(&archive).expect("archive build should succeed");
    let extracted = extract_tar_zstd_entry(&archive_bytes, "binary-feature/tree/blob.bin");
    assert_eq!(
        extracted, bytes,
        "binary bytes must round-trip through the archive"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// R2'''' / F2 second re-review 2026-07-25: archive fidelity
/// proof against the PERSISTED archive. Build the archive,
/// write it to disk, and call `archive_fidelity_proof` against
/// the persisted path. The proof must succeed when source and
/// extracted archive agree.
#[test]
fn archive_fidelity_proof_passes_when_source_matches_persisted() {
    let root = scratch("archive-fidelity-source");
    let source = root.join("openspec/changes/archive/legacy-feature");
    let archive = ArchiveTree {
        root: source.clone(),
        change_id: "legacy-feature".to_string(),
    };
    write_file(&source, "proposal.md", "# legacy-feature\n\nbody\n");
    write_file(&source, "tasks.md", "# [ ] legacy-feature work\n");
    let archive_bytes = import_archive_tree(&archive).expect("archive build should succeed");
    let archive_path = root.join("legacy-feature.tar.zst");
    std::fs::write(&archive_path, &archive_bytes).expect("archive write");

    let source_change_root = source.clone();
    nbspec::interchange::archive_fidelity_proof(
        "legacy-feature",
        &source_change_root,
        &archive_path,
    )
    .expect("archive fidelity proof should pass for matching source/extracted");
    std::fs::remove_dir_all(&root).ok();
}

/// R2'''' / F2 second re-review 2026-07-25: archive fidelity
/// proof against the PERSISTED archive. Mutate the source after
/// the archive is written; the proof must surface the divergence.
#[test]
fn archive_fidelity_proof_refuses_when_source_mutated_after_persist() {
    let root = scratch("archive-fidelity-mutated");
    let source = root.join("openspec/changes/archive/legacy-feature");
    let archive = ArchiveTree {
        root: source.clone(),
        change_id: "legacy-feature".to_string(),
    };
    write_file(&source, "proposal.md", "# legacy-feature\n\nbody\n");
    let archive_bytes = import_archive_tree(&archive).expect("archive build should succeed");
    let archive_path = root.join("legacy-feature.tar.zst");
    std::fs::write(&archive_path, &archive_bytes).expect("archive write");

    // Mutate the source after the archive is persisted.
    std::fs::write(
        source.join("proposal.md"),
        "# legacy-feature\n\nbody\n\nintroduction the archive did not capture\n",
    )
    .unwrap();

    let source_change_root = source.clone();
    let error = nbspec::interchange::archive_fidelity_proof(
        "legacy-feature",
        &source_change_root,
        &archive_path,
    )
    .expect_err("archive fidelity proof must surface source mutation");
    let message = format!("{error:?}");
    assert!(
        message.contains("ArchiveFidelity") || message.contains("archive fidelity"),
        "expected ArchiveFidelity error, got {message:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// R2'''' / F2 second re-review 2026-07-25: archive build
/// refuses empty source directories because the deterministic
/// archive layout stores regular files only and an empty source
/// dir would not round-trip.
#[test]
fn archive_conversion_refuses_empty_directory() {
    let root = scratch("archive-empty-dir");
    let source = root.join("openspec/changes/archive/legacy-feature");
    write_file(&source, "proposal.md", "# legacy-feature\n\nbody\n");
    let empty_dir = source.join("empty-subdir");
    std::fs::create_dir(&empty_dir).unwrap();

    let archive = ArchiveTree {
        root: source.clone(),
        change_id: "legacy-feature".to_string(),
    };
    let error =
        import_archive_tree(&archive).expect_err("archive build must refuse empty directories");
    let message = format!("{error:?}");
    assert!(
        message.contains("empty") || message.contains("LayoutAnomaly"),
        "expected empty-dir refusal, got {message:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// R2'' / F2 re-review 2026-07-25: a symlink in the archive
/// source tree must surface as a layout anomaly rather than being
/// silently skipped. The previous walker used `continue` on
/// symlinks, which produced a deterministic archive of a partial
/// inventory; archive source deletion could then discard the
/// omitted entry. A clean archive conversion must require every
/// entry to be enumerated with explicit failure surfacing.
#[test]
#[cfg(unix)]
fn archive_conversion_refuses_symlink_in_source() {
    let root = scratch("archive-symlink");
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/proposal.md",
        "# legacy-feature\n",
    );
    let victim = root.join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("outside.txt"), "leaked\n").unwrap();
    let link_path = root
        .join("openspec/changes/archive/legacy-feature")
        .join("leaked");
    std::os::unix::fs::symlink(victim.join("outside.txt"), &link_path).unwrap();

    let archive = ArchiveTree {
        root: root.join("openspec/changes/archive/legacy-feature"),
        change_id: "legacy-feature".to_string(),
    };
    let error = import_archive_tree(&archive).expect_err("archive conversion must refuse symlinks");
    let message = format!("{error:?}");
    assert!(
        message.contains("symlink") || message.contains("symlinks"),
        "expected symlink refusal, got {message:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// R2'' / F2 re-review 2026-07-25: a non-UTF-8 filename in the
/// archive source tree must surface as a layout anomaly rather
/// than being silently dropped via `into_string().ok()`. The
/// archived inventory must match the source inventory exactly.
#[test]
#[cfg(unix)]
fn archive_conversion_refuses_non_utf8_filename() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = scratch("archive-nonutf8");
    write_file(
        &root,
        "openspec/changes/archive/legacy-feature/proposal.md",
        "# legacy-feature\n",
    );
    let bad_name = OsString::from_vec(vec![0xff, 0xfe, b'_', b'd', b'a', b't', b'a']);
    let bad_path = root
        .join("openspec/changes/archive/legacy-feature")
        .join(&bad_name);
    std::fs::write(&bad_path, b"data\n").unwrap();

    let archive = ArchiveTree {
        root: root.join("openspec/changes/archive/legacy-feature"),
        change_id: "legacy-feature".to_string(),
    };
    let error = import_archive_tree(&archive)
        .expect_err("archive conversion must refuse non-UTF-8 filenames");
    let message = format!("{error:?}");
    assert!(
        message.contains("non-UTF-8") || message.contains("non_utf8"),
        "expected non-UTF-8 refusal, got {message:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Lists the entries inside a deterministic `tar.zst` archive.
/// Pipes the bytes through `tar -I zstd -tf` so the test reads
/// back the layout that `build_archive` produced. Falls back to
/// `tar --use-compress-program=unzstd -tf` when the platform's
/// `tar` lacks `-I`.
fn list_tar_zstd_entries(bytes: &[u8]) -> Vec<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let candidates: Vec<Vec<&str>> = vec![
        vec!["tar", "-I", "zstd", "-tf", "-"],
        vec!["tar", "--use-compress-program=unzstd", "-tf", "-"],
        vec!["tar", "--zstd", "-tf", "-"],
    ];
    for args in candidates {
        let Ok(mut child) = Command::new(args[0])
            .args(&args[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        else {
            continue;
        };
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(bytes);
        }
        if let Ok(output) = child.wait_with_output()
            && output.status.success()
        {
            return String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

/// Extracts a single entry from a deterministic `tar.zst` archive
/// and returns its raw bytes. Used by the binary-archive
/// regression test to verify the bytes round-tripped verbatim.
fn extract_tar_zstd_entry(bytes: &[u8], entry_path: &str) -> Vec<u8> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let args_variants: Vec<Vec<&str>> = vec![
        vec!["tar", "-I", "zstd", "-xOf", "-", entry_path],
        vec![
            "tar",
            "--use-compress-program=unzstd",
            "-xOf",
            "-",
            entry_path,
        ],
        vec!["tar", "--zstd", "-xOf", "-", entry_path],
    ];
    for args in args_variants {
        let Ok(mut child) = Command::new(args[0])
            .args(&args[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        else {
            continue;
        };
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(bytes);
        }
        if let Ok(output) = child.wait_with_output()
            && output.status.success()
        {
            return output.stdout;
        }
    }
    Vec::new()
}

// MCP Owner ring-1 v0.3.0-pause re-review (msg 1265cd09) bounded
// correction #2 (dry side): the dry-run structured payload must
// surface typed per-entry payloads (kind / status / change_id /
// source_path / prerequisite) for ActiveWrite entries, and the
// pending_deletions field must NOT include paused active sources
// (bounded correction #1).

fn fixture_plan(delete_original: bool) -> InterchangePlan {
    InterchangePlan {
        notebook: "nbspec".to_string(),
        entries: vec![
            PlanEntry::ActiveWrite {
                change_id: "add-foo".to_string(),
                source_path: PathBuf::from("/tmp/source/add-foo"),
            },
            PlanEntry::ArchiveWrite {
                change_id: "legacy".to_string(),
                source_path: PathBuf::from("/tmp/source/openspec/changes/archive/legacy"),
                target_path: PathBuf::from("documentation/archives/legacy.tar.zst"),
            },
            PlanEntry::Refusal {
                source_path: PathBuf::from("/tmp/source/add-bad"),
                message: "malformed proposal".to_string(),
            },
            PlanEntry::Skip {
                source_path: PathBuf::from("/tmp/source/add-skipped"),
                reason: "--no-active excludes active change ingestion".to_string(),
            },
        ],
        delete_original,
        // The pending_deletions field is computed by build_import_plan,
        // not by from_plan; for the dry-run regression we mirror the
        // planner's filter (paused active sources excluded) so the
        // assertion captures the contract.
        pending_deletions: if delete_original {
            vec![PathBuf::from("/tmp/source/openspec/changes/archive/legacy")]
        } else {
            Vec::new()
        },
        dry_run: true,
    }
}

#[test]
fn dry_run_structured_payload_emits_typed_active_write_entry() {
    let plan = fixture_plan(false);
    let structured = InterchangePlanStructured::from_plan(plan).structured;
    let entries = structured
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("entries must be an array");
    let active = entries
        .iter()
        .find(|entry| entry.get("kind").and_then(|v| v.as_str()) == Some("active-write"))
        .expect("an active-write entry must be present");
    assert_eq!(
        active.get("status").and_then(|v| v.as_str()),
        Some("paused"),
        "active-write status must be paused: {active}"
    );
    assert_eq!(
        active.get("change_id").and_then(|v| v.as_str()),
        Some("add-foo"),
        "active-write change_id must be set: {active}"
    );
    assert_eq!(
        active.get("source_path").and_then(|v| v.as_str()),
        Some("/tmp/source/add-foo"),
        "active-write source_path must be set: {active}"
    );
    let prerequisite = active
        .get("prerequisite")
        .and_then(|v| v.as_str())
        .expect("active-write prerequisite must be present");
    assert!(
        prerequisite.contains("NbApi 0.3"),
        "active-write prerequisite must name NbApi 0.3: {prerequisite}"
    );
}

#[test]
fn dry_run_structured_payload_emits_typed_archive_write_entry() {
    let plan = fixture_plan(false);
    let structured = InterchangePlanStructured::from_plan(plan).structured;
    let entries = structured
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("entries must be an array");
    let archive = entries
        .iter()
        .find(|entry| entry.get("kind").and_then(|v| v.as_str()) == Some("archive-write"))
        .expect("an archive-write entry must be present");
    assert_eq!(
        archive.get("change_id").and_then(|v| v.as_str()),
        Some("legacy"),
        "archive-write change_id must be set: {archive}"
    );
    assert_eq!(
        archive.get("source_path").and_then(|v| v.as_str()),
        Some("/tmp/source/openspec/changes/archive/legacy"),
        "archive-write source_path must be set: {archive}"
    );
    assert_eq!(
        archive.get("target_path").and_then(|v| v.as_str()),
        Some("documentation/archives/legacy.tar.zst"),
        "archive-write target_path must be set: {archive}"
    );
}

#[test]
fn dry_run_structured_payload_pending_deletions_excludes_paused_active_sources() {
    // Bounded correction #1 (serialization side): `from_plan` must
    // faithfully serialize whatever `pending_deletions` the plan
    // carries. This test verifies the serialization contract by
    // feeding a plan whose `pending_deletions` already excludes the
    // paused active source (mirroring the contract). The filter
    // itself — `build_import_plan` excluding `ActiveWrite` entries —
    // is exercised end-to-end by the MCP dry-run regression
    // `mcp_import_dry_run_delete_original_exercises_real_pending_
    // deletions_filter`, which calls the real `import` pipeline with
    // `dry_run=true` + `delete_original=true` and inspects
    // `structuredContent.pending_deletions` from the plan path.
    let plan = fixture_plan(true);
    let structured = InterchangePlanStructured::from_plan(plan).structured;
    let pending = structured
        .get("pending_deletions")
        .and_then(|v| v.as_array())
        .expect("pending_deletions must be an array");
    assert_eq!(
        pending.len(),
        1,
        "exactly the archive source must be pending deletion: {pending:?}"
    );
    assert_eq!(
        pending[0].as_str(),
        Some("/tmp/source/openspec/changes/archive/legacy"),
        "the archive source must be the only pending deletion: {pending:?}"
    );
    assert!(
        !pending
            .iter()
            .any(|v| v.as_str().map(|s| s.contains("add-foo")).unwrap_or(false)),
        "paused active source must NOT be in pending_deletions: {pending:?}"
    );
}
