#![allow(dead_code, unused_imports)]
//! Unit tests for archive conversion and fidelity.

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
