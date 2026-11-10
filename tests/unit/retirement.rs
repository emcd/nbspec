use std::path::{Path, PathBuf};

use nbspec::archives::{ArchiveEntry, build_archive};
use nbspec::changes::{ChangeMetadata, render_meta_note};
use nbspec::retirement::{
    capture_snapshot, classify_commit_outcome, namespace_entries, validate_archive_identity,
    verify_persisted_archive, verify_snapshot,
};

const TEMP_TEST_ROOT: &str = ".auxiliary/temporary/tests";

fn unique_root(label: &str) -> PathBuf {
    let unique = format!(
        "retirement-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    PathBuf::from(TEMP_TEST_ROOT).join(unique)
}

fn seed_namespace(root: &Path) -> PathBuf {
    let namespace = root.join("proposals").join("add-demo");
    std::fs::create_dir_all(namespace.join("specifications")).unwrap();
    std::fs::write(
        namespace.join("proposal.md"),
        "# Demo\n\n## Why\n\nBecause.\n",
    )
    .unwrap();
    std::fs::write(
        namespace.join("specifications/user-auth.md"),
        "# user-auth\n",
    )
    .unwrap();
    std::fs::write(namespace.join(".gitkeep"), "").unwrap();
    std::fs::write(namespace.join("specifications/.index"), "").unwrap();
    namespace
}

fn seed_meta(namespace: &std::path::Path) {
    let metadata =
        nbspec::changes::ChangeMetadata::new("add-demo", Some("Demo"), "nbspec-default", "nbspec")
            .unwrap();
    let body = render_meta_note(&metadata).unwrap();
    std::fs::write(namespace.join("meta.md"), body).unwrap();
}

#[test]
fn snapshot_captures_user_files_and_skips_bookkeeping() {
    let root = unique_root("capture");
    let namespace = seed_namespace(&root);
    let snapshot = capture_snapshot(&namespace).unwrap();
    let mut relatives: Vec<&str> = snapshot
        .files
        .iter()
        .map(|file| file.relative.as_str())
        .collect();
    relatives.sort();
    assert_eq!(
        relatives,
        vec!["proposal.md", "specifications/user-auth.md"],
        "bookkeeping excluded, user files kept"
    );
    for file in &snapshot.files {
        assert!(!file.bytes.is_empty());
        assert_eq!(file.hash.len(), 32);
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn snapshot_verification_detects_edit_add_and_remove() {
    let root = unique_root("drift");
    let namespace = seed_namespace(&root);
    let snapshot = capture_snapshot(&namespace).unwrap();
    verify_snapshot(&namespace, &snapshot).unwrap();

    std::fs::write(namespace.join("proposal.md"), "# Changed\n").unwrap();
    assert!(
        verify_snapshot(&namespace, &snapshot).is_err(),
        "edit refuses"
    );

    std::fs::write(
        namespace.join("proposal.md"),
        "# Demo\n\n## Why\n\nBecause.\n",
    )
    .unwrap();
    verify_snapshot(&namespace, &snapshot).unwrap();
    std::fs::write(namespace.join("extra.md"), "# Extra\n").unwrap();
    assert!(
        verify_snapshot(&namespace, &snapshot).is_err(),
        "addition refuses"
    );

    std::fs::remove_file(namespace.join("extra.md")).unwrap();
    std::fs::remove_file(namespace.join("specifications/user-auth.md")).unwrap();
    assert!(
        verify_snapshot(&namespace, &snapshot).is_err(),
        "removal refuses"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn namespace_entries_carry_original_paths_as_raw_bytes() {
    let root = unique_root("entries");
    let namespace = seed_namespace(&root);
    let snapshot = capture_snapshot(&namespace).unwrap();
    let entries = namespace_entries("add-demo", &snapshot);
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert!(
            entry.path.starts_with("add-demo/namespace/"),
            "unexpected prefix: {}",
            entry.path.display()
        );
    }
    std::fs::remove_dir_all(&root).ok();
}

fn valid_tarball_bytes(change_id: &str) -> Vec<u8> {
    let metadata =
        ChangeMetadata::new(change_id, Some("Demo"), "nbspec-default", "nbspec").unwrap();
    let entries = vec![ArchiveEntry {
        path: PathBuf::from(format!("{change_id}/meta.md")),
        content: render_meta_note(&metadata).unwrap().into_bytes(),
    }];
    build_archive(&entries).unwrap()
}

#[test]
fn archive_identity_accepts_matching_and_rejects_the_rest() {
    let good = valid_tarball_bytes("add-demo");
    validate_archive_identity(&good, "add-demo").unwrap();
    assert!(
        validate_archive_identity(&good, "other-id").is_err(),
        "wrong change refuses"
    );
    assert!(
        validate_archive_identity(b"not a tarball", "add-demo").is_err(),
        "corrupt bytes refuse"
    );
    let other_meta = valid_tarball_bytes("other-id");
    assert!(
        validate_archive_identity(&other_meta, "add-demo").is_err(),
        "foreign meta refuses"
    );
}

#[test]
fn persisted_verification_matches_inventory_and_catches_divergence() {
    let root = unique_root("persisted");
    let namespace = seed_namespace(&root);
    seed_meta(&namespace);
    let snapshot = capture_snapshot(&namespace).unwrap();
    let tarball = root.join("add-demo.tar.zst");
    let mut entries = vec![ArchiveEntry {
        path: PathBuf::from("add-demo/meta.md"),
        content: std::fs::read(namespace.join("meta.md")).unwrap(),
    }];
    entries.extend(namespace_entries("add-demo", &snapshot));
    std::fs::write(&tarball, build_archive(&entries).unwrap()).unwrap();
    verify_persisted_archive(&tarball, "add-demo", &snapshot).unwrap();

    // Tamper with one archived byte: verification must fail.
    let mut tampered = std::fs::read(&tarball).unwrap();
    tampered.push(0xAA);
    std::fs::write(&tarball, tampered).unwrap();
    assert!(
        verify_persisted_archive(&tarball, "add-demo", &snapshot).is_err(),
        "tampered tarball refuses"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn archive_identity_rejects_prefix_lookalikes_and_duplicate_meta() {
    let metadata =
        ChangeMetadata::new("add-demo", Some("Demo"), "nbspec-default", "nbspec").unwrap();
    let meta_body = render_meta_note(&metadata).unwrap().into_bytes();
    // `add-demometa.md` names the change but lives outside `add-demo/`.
    let tricky = vec![ArchiveEntry {
        path: PathBuf::from("add-demometa.md"),
        content: meta_body.clone(),
    }];
    assert!(
        validate_archive_identity(&build_archive(&tricky).unwrap(), "add-demo").is_err(),
        "string-prefix lookalike refuses"
    );
    // Sibling directory with the id as a string prefix.
    let sibling = vec![
        ArchiveEntry {
            path: PathBuf::from("add-demo/meta.md"),
            content: meta_body.clone(),
        },
        ArchiveEntry {
            path: PathBuf::from("add-demo-other/note.md"),
            content: b"# other\n".to_vec(),
        },
    ];
    assert!(
        validate_archive_identity(&build_archive(&sibling).unwrap(), "add-demo").is_err(),
        "sibling directory refuses"
    );
    // Two meta snapshots is ambiguous, not proof.
    let duplicated = vec![
        ArchiveEntry {
            path: PathBuf::from("add-demo/meta.md"),
            content: meta_body.clone(),
        },
        ArchiveEntry {
            path: PathBuf::from("add-demo/meta.md"),
            content: meta_body,
        },
    ];
    assert!(
        validate_archive_identity(&build_archive(&duplicated).unwrap(), "add-demo").is_err(),
        "duplicate meta refuses"
    );
}

#[test]
#[cfg(unix)]
fn snapshot_refuses_backslash_filenames() {
    let root = unique_root("backslash");
    let namespace = seed_namespace(&root);
    std::fs::write(namespace.join("a\\b"), "# backslash\n").unwrap();
    assert!(
        capture_snapshot(&namespace).is_err(),
        "backslash filename refuses before publication"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
/// APFS rejects non-UTF-8 filenames at the filesystem level, so
/// this is Linux-specific, not merely Unix.
#[cfg(target_os = "linux")]
fn snapshot_refuses_non_utf8_filenames() {
    use std::os::unix::ffi::OsStringExt;
    let root = unique_root("nonutf8");
    let namespace = seed_namespace(&root);
    let hostile = std::ffi::OsString::from_vec(b"hostile-\xff-note.md".to_vec());
    std::fs::write(namespace.join(hostile), "# hostile\n").unwrap();
    assert!(
        capture_snapshot(&namespace).is_err(),
        "non-UTF-8 filename refuses before publication"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
#[cfg(unix)]
fn persisted_verification_refuses_backslash_masquerade() {
    // Unix-only: on Windows a\b is a normal nested path that tar
    // normalizes to a/b, so verification correctly succeeds there.
    // A tarball entry with a literal backslash must not match the
    // slash-path inventory entry: extraction creates a file named
    // `a\b`, and the shared key helper refuses it instead of
    // normalizing it to `a/b` and losing the original name.
    use nbspec::retirement::{NamespaceSnapshot, SnapshotFile};
    use sha2::{Digest, Sha256};
    let snapshot = NamespaceSnapshot {
        files: vec![SnapshotFile {
            relative: "a/b".to_string(),
            hash: Sha256::digest(b"X").into(),
            bytes: b"X".to_vec(),
        }],
    };
    let root = unique_root("masquerade");
    let tarball = root.join("add-demo.tar.zst");
    let entries = vec![ArchiveEntry {
        path: PathBuf::from("add-demo/namespace/a\\b"),
        content: b"X".to_vec(),
    }];
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&tarball, build_archive(&entries).unwrap()).unwrap();
    assert!(
        verify_persisted_archive(&tarball, "add-demo", &snapshot).is_err(),
        "literal-backslash entry must not match the slash-path inventory"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn commit_outcome_mapping_matches_the_retirement_contract() {
    use nb_api::NbError;
    let phases = "materialization complete; tarball published at x";
    assert!(classify_commit_outcome(Ok(()), phases).is_ok());
    let indeterminate = classify_commit_outcome(
        Err(NbError::IndeterminateCommit {
            pre_revision: "a".to_string(),
            post_revision_observed: None,
            guidance: "maybe".to_string(),
        }),
        phases,
    );
    let message = format!("{}", indeterminate.unwrap_err());
    assert!(
        message.contains("inspection required"),
        "indeterminate demands inspection: {message}"
    );
    let recovery = classify_commit_outcome(
        Err(NbError::RecoveryRequired {
            pre_revision: "a".to_string(),
            post_revision_observed: None,
            status_observed: None,
            preserved_paths: None,
            guidance: "fix".to_string(),
        }),
        phases,
    );
    let message = format!("{}", recovery.unwrap_err());
    assert!(
        message.contains("inspection required"),
        "recovery demands inspection: {message}"
    );
    let clean = classify_commit_outcome(
        Err(NbError::ValidationError {
            reason: "no".to_string(),
            location: None,
        }),
        phases,
    );
    let message = format!("{}", clean.unwrap_err());
    assert!(
        message.contains("re-run merge to resume"),
        "clean failure resumes: {message}"
    );
    assert!(
        message.contains("materialization complete"),
        "phases travel with the error: {message}"
    );
}
