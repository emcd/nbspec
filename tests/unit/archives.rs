use std::fs;
use std::path::{Path, PathBuf};

use nbspec::archives::{ArchiveEntry, build_archive, gitattributes_covers_lfs};

const TEMP_TEST_ROOT: &str = ".auxiliary/temporary/tests";

fn unique_temp_root(label: &str) -> PathBuf {
    let unique = format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    PathBuf::from(TEMP_TEST_ROOT).join(unique)
}

fn entry(path: &str, content: &str) -> ArchiveEntry {
    ArchiveEntry {
        path: PathBuf::from(path),
        content: content.to_string(),
    }
}

fn sample_entries() -> Vec<ArchiveEntry> {
    vec![
        entry("add-demo/proposal.md", "# proposal\n"),
        entry("add-demo/specifications/alpha.md", "# alpha\n"),
        entry("add-demo/meta.md", "{}\n"),
        entry("add-demo/work.md", "# [ ] work\n"),
    ]
}

#[test]
fn archives_are_deterministic() {
    let first = build_archive(&sample_entries()).unwrap();
    let second = build_archive(&sample_entries()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn entry_order_does_not_affect_bytes() {
    let mut reversed = sample_entries();
    reversed.reverse();
    assert_eq!(
        build_archive(&sample_entries()).unwrap(),
        build_archive(&reversed).unwrap()
    );
}

#[test]
fn archives_round_trip_through_tar_and_zstd() {
    let bytes = build_archive(&sample_entries()).unwrap();
    let tar_bytes = zstd::decode_all(bytes.as_slice()).unwrap();
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let mut recovered = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_path_buf();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entry, &mut content).unwrap();
        assert_eq!(entry.header().mtime().unwrap(), 0);
        assert_eq!(entry.header().uid().unwrap(), 0);
        recovered.push((path, content));
    }
    let paths: Vec<String> = recovered
        .iter()
        .map(|(path, _)| path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        paths,
        vec![
            "add-demo/meta.md",
            "add-demo/proposal.md",
            "add-demo/specifications/alpha.md",
            "add-demo/work.md",
        ]
    );
    let alpha = recovered
        .iter()
        .find(|(path, _)| path.ends_with("alpha.md"))
        .unwrap();
    assert_eq!(alpha.1, "# alpha\n");
}

fn project_with_gitattributes(label: &str, rules: &str) -> PathBuf {
    let root = unique_temp_root(label);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".gitattributes"), rules).unwrap();
    root
}

const ARCHIVE: &str = "documentation/archives/add-demo.tar.zst";

#[test]
fn basename_pattern_covers_archive() {
    let root = project_with_gitattributes(
        "gitattributes-basename",
        "*.tar.zst filter=lfs diff=lfs merge=lfs -text\n",
    );
    assert!(gitattributes_covers_lfs(&root, Path::new(ARCHIVE)));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn directory_glob_pattern_covers_archive() {
    let root = project_with_gitattributes(
        "gitattributes-directory",
        "documentation/archives/** filter=lfs diff=lfs merge=lfs -text\n",
    );
    assert!(gitattributes_covers_lfs(&root, Path::new(ARCHIVE)));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn single_star_does_not_cross_directories() {
    let root = project_with_gitattributes(
        "gitattributes-single-star",
        "documentation/*.tar.zst filter=lfs\n",
    );
    assert!(!gitattributes_covers_lfs(&root, Path::new(ARCHIVE)));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn rule_without_lfs_filter_does_not_cover() {
    let root = project_with_gitattributes("gitattributes-no-lfs", "*.tar.zst -text\n");
    assert!(!gitattributes_covers_lfs(&root, Path::new(ARCHIVE)));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn missing_gitattributes_does_not_cover() {
    let root = unique_temp_root("gitattributes-missing");
    fs::create_dir_all(&root).unwrap();
    assert!(!gitattributes_covers_lfs(&root, Path::new(ARCHIVE)));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let root = project_with_gitattributes(
        "gitattributes-comments",
        "# archives belong in LFS\n\n*.tar.zst filter=lfs\n",
    );
    assert!(gitattributes_covers_lfs(&root, Path::new(ARCHIVE)));
    fs::remove_dir_all(&root).unwrap();
}
