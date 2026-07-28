use std::fs;
use std::path::{Path, PathBuf};

use nbspec::rendering::{
    RenderedDocument, aggregate_content_hash, render_documents, review_diff, write_tree,
};
use nbspec::schemata::{default_schema, parse_schema};

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

/// Builds a fixture change directory shaped like an authored change
/// namespace on the notebook filesystem.
fn fixture_change(root: &Path) -> PathBuf {
    let change = root.join("notebook/proposals/add-demo");
    fs::create_dir_all(change.join("specifications/nested")).unwrap();
    fs::create_dir_all(change.join("designs")).unwrap();
    fs::create_dir_all(change.join("decisions")).unwrap();
    fs::write(change.join("proposal.md"), "# proposal\n\nWhy: reasons.\n").unwrap();
    fs::write(change.join("meta.md"), "# meta\n\n```json\n{}\n```\n").unwrap();
    fs::write(
        change.join("20260101000000.todo.md"),
        "# [ ] work\n\n- [ ] Do it.\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/alpha.md"),
        "# alpha\n\n## ADDED Requirements\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/nested/beta.md"),
        "# beta\n\n## ADDED Requirements\n",
    )
    .unwrap();
    fs::write(change.join("designs/gamma.md"), "# gamma\n\nDesign.\n").unwrap();
    change
}

#[test]
fn renders_documents_in_schema_and_path_order() {
    let root = unique_temp_root("rendering-order");
    let change = fixture_change(&root);
    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let paths: Vec<String> = documents
        .iter()
        .map(|document| document.tree_path.clone())
        .collect();
    assert_eq!(
        paths,
        vec![
            "proposal.md",
            "specifications/alpha.md",
            "specifications/nested/beta.md",
            "designs/gamma.md",
        ]
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn excludes_control_plane_files() {
    let root = unique_temp_root("rendering-control-plane");
    let change = fixture_change(&root);
    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    assert!(
        documents
            .iter()
            .all(|document| !document.tree_path.contains("meta"))
    );
    assert!(
        documents
            .iter()
            .all(|document| !document.tree_path.contains("todo"))
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn copies_content_verbatim_with_provenance_paths() {
    let root = unique_temp_root("rendering-content");
    let change = fixture_change(&root);
    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let alpha = documents
        .iter()
        .find(|document| document.tree_path == "specifications/alpha.md")
        .unwrap();
    assert_eq!(alpha.content, "# alpha\n\n## ADDED Requirements\n");
    assert_eq!(alpha.artifact_id, "specifications");
    assert_eq!(
        alpha.source_note,
        "proposals/add-demo/specifications/alpha.md"
    );
    assert_eq!(
        alpha.target_path.as_deref(),
        Some("documentation/specifications/alpha.md")
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn proposal_renders_without_merge_target() {
    let root = unique_temp_root("rendering-proposal");
    let change = fixture_change(&root);
    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let proposal = documents
        .iter()
        .find(|document| document.artifact_id == "proposal")
        .unwrap();
    assert_eq!(proposal.target_path, None);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn skips_unauthored_artifacts() {
    let root = unique_temp_root("rendering-unauthored");
    let change = root.join("notebook/proposals/add-bare");
    fs::create_dir_all(change.join("specifications")).unwrap();
    let documents = render_documents(&change, "proposals/add-bare", &default_schema()).unwrap();
    assert!(documents.is_empty());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn rendering_is_deterministic() {
    let root = unique_temp_root("rendering-deterministic");
    let change = fixture_change(&root);
    let first = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let second = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    assert_eq!(first, second);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn write_tree_replaces_stale_contents() {
    let root = unique_temp_root("rendering-write");
    let change = fixture_change(&root);
    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let destination = root.join("render");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("stale.md"), "left over\n").unwrap();
    write_tree(&documents, &destination).unwrap();
    assert!(!destination.join("stale.md").exists());
    assert!(destination.join("specifications/nested/beta.md").is_file());
    assert_eq!(
        fs::read_to_string(destination.join("proposal.md")).unwrap(),
        "# proposal\n\nWhy: reasons.\n"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn diff_reports_new_documents_from_dev_null() {
    let root = unique_temp_root("rendering-diff-new");
    let change = fixture_change(&root);
    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let project = root.join("project");
    fs::create_dir_all(&project).unwrap();
    let diff = review_diff(&documents, &project).unwrap();
    assert!(diff.contains(
        "diff --git a/documentation/specifications/alpha.md \
         b/documentation/specifications/alpha.md"
    ));
    assert!(diff.contains("new file mode 100644"));
    assert!(diff.contains("--- /dev/null"));
    assert!(diff.contains("+# alpha"));
    assert!(!diff.contains("proposal.md"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn diff_omits_unchanged_targets() {
    let root = unique_temp_root("rendering-diff-unchanged");
    let change = fixture_change(&root);
    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let project = root.join("project");
    fs::create_dir_all(project.join("documentation/specifications")).unwrap();
    fs::write(
        project.join("documentation/specifications/alpha.md"),
        "# alpha\n\n## ADDED Requirements\n",
    )
    .unwrap();
    let diff = review_diff(&documents, &project).unwrap();
    assert!(!diff.contains("alpha.md"));
    assert!(diff.contains("nested/beta.md"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn diff_shows_changed_targets() {
    let root = unique_temp_root("rendering-diff-changed");
    let change = fixture_change(&root);
    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let project = root.join("project");
    fs::create_dir_all(project.join("documentation/designs")).unwrap();
    fs::write(
        project.join("documentation/designs/gamma.md"),
        "# gamma\n\nOld design.\n",
    )
    .unwrap();
    let diff = review_diff(&documents, &project).unwrap();
    assert!(diff.contains("a/documentation/designs/gamma.md"));
    assert!(diff.contains("-Old design."));
    assert!(diff.contains("+Design."));
    assert!(!diff.contains("new file mode 100644\n--- a/documentation/designs"));
    fs::remove_dir_all(&root).unwrap();
}

/// Builds an in-memory rendered document for aggregate-hash tests.
fn hash_fixture_document(tree_path: &str, content: &str) -> RenderedDocument {
    RenderedDocument {
        artifact_id: "specifications".to_string(),
        tree_path: tree_path.to_string(),
        target_path: None,
        source_note: format!("proposals/add-demo/{tree_path}"),
        content: content.to_string(),
    }
}

#[test]
fn aggregate_hash_is_deterministic_and_order_independent() {
    let alpha = hash_fixture_document("specifications/alpha.md", "# alpha\n");
    let beta = hash_fixture_document("specifications/beta.md", "# beta\n");
    let forward = aggregate_content_hash(&[alpha.clone(), beta.clone()]);
    let reversed = aggregate_content_hash(&[beta, alpha]);
    assert_eq!(
        forward, reversed,
        "aggregate must not depend on enumeration order"
    );
    assert_eq!(forward.len(), 64, "SHA-256 hex digest expected");
}

#[test]
fn aggregate_hash_changes_on_body_edit() {
    let original = hash_fixture_document("proposal.md", "# proposal\n\nWhy: reasons.\n");
    let edited = hash_fixture_document("proposal.md", "# proposal\n\nWhy: better reasons.\n");
    assert_ne!(
        aggregate_content_hash(&[original]),
        aggregate_content_hash(&[edited]),
        "any body edit must change the aggregate"
    );
}

#[test]
fn aggregate_hash_changes_on_added_document() {
    let proposal = hash_fixture_document("proposal.md", "# proposal\n");
    let spec = hash_fixture_document("specifications/alpha.md", "# alpha\n");
    assert_ne!(
        aggregate_content_hash(std::slice::from_ref(&proposal)),
        aggregate_content_hash(&[proposal, spec]),
        "set membership growth must change the aggregate"
    );
}

#[test]
fn aggregate_hash_changes_on_rename_without_body_edit() {
    let original = hash_fixture_document("specifications/alpha.md", "# alpha\n");
    let renamed = hash_fixture_document("specifications/renamed.md", "# alpha\n");
    assert_ne!(
        aggregate_content_hash(&[original]),
        aggregate_content_hash(&[renamed]),
        "a rename with identical body must change the aggregate"
    );
}

#[test]
fn aggregate_hash_of_empty_set_is_stable() {
    assert_eq!(
        aggregate_content_hash(&[]),
        aggregate_content_hash(&[]),
        "empty rendered set must hash deterministically"
    );
}

#[test]
fn timestamp_filename_replaced_by_h1_slug_in_tree_path() {
    let root = unique_temp_root("rendering-timestamp-slug");
    let change = root.join("notebook/proposals/add-demo");
    fs::create_dir_all(change.join("specifications")).unwrap();
    fs::write(change.join("proposal.md"), "# proposal\n\nWhy.\n").unwrap();
    fs::write(change.join("meta.md"), "# meta\n\n```json\n{}\n```\n").unwrap();
    fs::write(
        change.join("20260101000000.todo.md"),
        "# [ ] work\n\n- [ ] Do it.\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/20260710175830.md"),
        "# Review Gating Model\n\n## ADDED Requirements\n",
    )
    .unwrap();

    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let spec = documents
        .iter()
        .find(|d| d.artifact_id == "specifications")
        .unwrap();
    assert_eq!(
        spec.tree_path, "specifications/review-gating-model.md",
        "timestamp filename must be replaced by H1-derived slug"
    );
    assert_eq!(
        spec.target_path.as_deref(),
        Some("documentation/specifications/review-gating-model.md"),
        "target path must use the slug"
    );
    assert_eq!(
        spec.source_note, "proposals/add-demo/specifications/20260710175830.md",
        "source note must preserve the original notebook filename"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn non_timestamp_filename_preserved_in_tree_path() {
    let root = unique_temp_root("rendering-non-timestamp");
    let change = root.join("notebook/proposals/add-demo");
    fs::create_dir_all(change.join("specifications")).unwrap();
    fs::write(change.join("proposal.md"), "# proposal\n\nWhy.\n").unwrap();
    fs::write(change.join("meta.md"), "# meta\n\n```json\n{}\n```\n").unwrap();
    fs::write(
        change.join("20260101000000.todo.md"),
        "# [ ] work\n\n- [ ] Do it.\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/user-auth.md"),
        "# User Authentication\n\n## ADDED Requirements\n",
    )
    .unwrap();

    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let spec = documents
        .iter()
        .find(|d| d.artifact_id == "specifications")
        .unwrap();
    assert_eq!(
        spec.tree_path, "specifications/user-auth.md",
        "non-timestamp filename must be preserved"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn timestamp_filename_without_h1_falls_back_to_original() {
    let root = unique_temp_root("rendering-timestamp-no-h1");
    let change = root.join("notebook/proposals/add-demo");
    fs::create_dir_all(change.join("specifications")).unwrap();
    fs::write(change.join("proposal.md"), "# proposal\n\nWhy.\n").unwrap();
    fs::write(change.join("meta.md"), "# meta\n\n```json\n{}\n```\n").unwrap();
    fs::write(
        change.join("20260101000000.todo.md"),
        "# [ ] work\n\n- [ ] Do it.\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/20260710175830.md"),
        "<!-- no H1 here -->\n\n## ADDED Requirements\n",
    )
    .unwrap();

    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let spec = documents
        .iter()
        .find(|d| d.artifact_id == "specifications")
        .unwrap();
    assert_eq!(
        spec.tree_path, "specifications/20260710175830.md",
        "timestamp without H1 must fall back to original filename"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn duplicate_h1_slugs_across_timestamp_notes_collide() {
    let root = unique_temp_root("rendering-collision-duplicate-h1");
    let change = root.join("notebook/proposals/add-demo");
    fs::create_dir_all(change.join("specifications")).unwrap();
    fs::write(change.join("proposal.md"), "# proposal\n\n## Why\n\nWhy.\n").unwrap();
    fs::write(change.join("meta.md"), "# meta\n\n```json\n{}\n```\n").unwrap();
    fs::write(
        change.join("20260101000000.todo.md"),
        "# [ ] work\n\n- [ ] Do it.\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/20260710175830.md"),
        "# User Auth\n\n## ADDED Requirements\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/20260710175831.md"),
        "# User Auth\n\n## ADDED Requirements\n",
    )
    .unwrap();

    let result = render_documents(&change, "proposals/add-demo", &default_schema());
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("duplicate"), "{msg}");
    assert!(msg.contains("user-auth.md"), "{msg}");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn timestamp_slug_colliding_with_named_note_is_rejected() {
    let root = unique_temp_root("rendering-collision-named");
    let change = root.join("notebook/proposals/add-demo");
    fs::create_dir_all(change.join("specifications")).unwrap();
    fs::write(change.join("proposal.md"), "# proposal\n\n## Why\n\nWhy.\n").unwrap();
    fs::write(change.join("meta.md"), "# meta\n\n```json\n{}\n```\n").unwrap();
    fs::write(
        change.join("20260101000000.todo.md"),
        "# [ ] work\n\n- [ ] Do it.\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/user-auth.md"),
        "# User Auth\n\n## ADDED Requirements\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/20260710175830.md"),
        "# User Auth\n\n## ADDED Requirements\n",
    )
    .unwrap();

    let result = render_documents(&change, "proposals/add-demo", &default_schema());
    assert!(
        result.is_err(),
        "timestamp slug colliding with named note must error"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn distinct_tree_paths_sharing_durable_target_collide() {
    let root = unique_temp_root("rendering-collision-shared-target");
    let change = root.join("notebook/proposals/add-demo");
    fs::create_dir_all(change.join("alpha")).unwrap();
    fs::create_dir_all(change.join("beta")).unwrap();
    fs::write(change.join("alpha/same.md"), "# Alpha same\n\nBody.\n").unwrap();
    fs::write(change.join("beta/same.md"), "# Beta same\n\nBody.\n").unwrap();

    let schema = parse_schema(
        r#"
name = "shared-target"
version = 1

[[artifacts]]
id = "alpha"
generates = "alpha/**/*.md"
target = "documentation/shared"

[[artifacts]]
id = "beta"
generates = "beta/**/*.md"
target = "documentation/shared"
"#,
    )
    .unwrap();

    let result = render_documents(&change, "proposals/add-demo", &schema);
    let err = result.expect_err("distinct tree paths sharing one durable target must collide");
    let msg = err.to_string();
    assert!(msg.contains("durable target"), "{msg}");
    assert!(msg.contains("documentation/shared/same.md"), "{msg}");
    assert!(msg.contains("alpha/same.md"), "{msg}");
    assert!(msg.contains("beta/same.md"), "{msg}");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn timestamp_filename_with_fenced_h1_falls_back_to_original() {
    let root = unique_temp_root("rendering-timestamp-fenced-h1");
    let change = root.join("notebook/proposals/add-demo");
    fs::create_dir_all(change.join("specifications")).unwrap();
    fs::write(change.join("proposal.md"), "# proposal\n\n## Why\n\nWhy.\n").unwrap();
    fs::write(change.join("meta.md"), "# meta\n\n```json\n{}\n```\n").unwrap();
    fs::write(
        change.join("20260101000000.todo.md"),
        "# [ ] work\n\n- [ ] Do it.\n",
    )
    .unwrap();
    fs::write(
        change.join("specifications/20260710175830.md"),
        "```\n# Not A Real H1\n```\n\n## ADDED Requirements\n",
    )
    .unwrap();

    let documents = render_documents(&change, "proposals/add-demo", &default_schema()).unwrap();
    let spec = documents
        .iter()
        .find(|d| d.artifact_id == "specifications")
        .unwrap();
    assert_eq!(
        spec.tree_path, "specifications/20260710175830.md",
        "fenced H1 must not produce a slug; fall back to original filename"
    );
    fs::remove_dir_all(&root).unwrap();
}
