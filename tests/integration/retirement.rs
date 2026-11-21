//! Namespace retirement end-to-end: archives-disabled retention
//! plus the drift, force, and validation behaviors that stay live
//! while the namespace survives.

use super::harness::Fixture;
use super::lifecycle::{
    CHANGE_ID, SPECIFICATION, nbspec, stderr_of, stdout_of,
};

/// Merge with `archives = false` materializes, warns loudly, and
/// retains the namespace as the only record: no tarball, no
/// retirement. The live namespace keeps every post-merge behavior
/// (remerge, drift refusal, force restore, validation diagnostics).
#[test]
fn archives_disabled_merge_warns_and_retains_namespace() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.configuration_directory().join("general.toml"),
        "schema = \"nbspec-default\"\n\
         scratch_directory = \".auxiliary/temporary/renders\"\n\
         archives = false\n\
         archive_directory = \"documentation/archives\"\n",
    )
    .unwrap();

    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    let change_directory =
        fixture.notebook_path().join("proposals").join(CHANGE_ID);
    let mut proposal =
        std::fs::read_to_string(change_directory.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nProve the archives=false path.\n");
    std::fs::write(change_directory.join("proposal.md"), proposal).unwrap();
    let specification_note =
        change_directory.join("specifications/user-auth.md");
    std::fs::write(&specification_note, SPECIFICATION).unwrap();
    let approved = nbspec(
        &fixture,
        &[
            "review",
            CHANGE_ID,
            "--verdict",
            "approve",
            "--reviewer",
            "itest",
            "--comment",
            "ok",
        ],
    );
    assert!(approved.status.success(), "{}", stderr_of(&approved));

    let merged = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(merged.status.success(), "{}", stderr_of(&merged));
    let merge_output = stdout_of(&merged);
    assert!(
        merge_output
            .contains("wrote documentation/specifications/user-auth.md")
    );
    assert!(
        merge_output.contains("archives disabled"),
        "merge warns loudly: {merge_output}"
    );
    assert!(
        merge_output.contains("retirement skipped"),
        "merge reports the skipped retirement: {merge_output}"
    );
    assert!(
        !fixture
            .project_root()
            .join("documentation/archives/add-demo.tar.zst")
            .exists(),
        "no tarball is written"
    );
    assert!(
        change_directory.is_dir(),
        "namespace retained as the record"
    );
    let target = fixture
        .project_root()
        .join("documentation/specifications/user-auth.md");
    let merged_content = std::fs::read_to_string(&target).unwrap();

    // Re-merge is idempotent against the live namespace.
    let remerged = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(remerged.status.success(), "{}", stderr_of(&remerged));
    assert!(
        stdout_of(&remerged)
            .contains("unchanged documentation/specifications/user-auth.md"),
        "{}",
        stdout_of(&remerged)
    );

    // A hand-edited target refuses without force and nothing changes.
    let drifted_content = format!("{merged_content}\nEdited by hand.\n");
    std::fs::write(&target, &drifted_content).unwrap();
    let refused = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert_eq!(refused.status.code(), Some(1));
    let refusal = stderr_of(&refused);
    assert!(refusal.contains("merge refused; no files were written"));
    assert!(refusal.contains("documentation/specifications/user-auth.md"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), drifted_content);

    // Force restores the notebook's version.
    let forced = nbspec(&fixture, &["merge", CHANGE_ID, "--force"]);
    assert!(forced.status.success(), "{}", stderr_of(&forced));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), merged_content);

    // Breaking the specification surfaces a line-anchored diagnostic.
    let broken = SPECIFICATION
        .split("#### Scenario:")
        .next()
        .unwrap()
        .to_string();
    std::fs::write(&specification_note, broken).unwrap();
    let rebroken = nbspec(&fixture, &["validate", CHANGE_ID]);
    assert_eq!(rebroken.status.code(), Some(1));
    assert!(stderr_of(&rebroken).contains(
        "proposals/add-demo/specifications/user-auth.md:5: [specifications] \
             requirement User authentication has no #### Scenario: block"
    ));
}

use std::path::Path;

fn set_status(fixture: &Fixture, change_id: &str, status: &str) {
    let meta = fixture
        .notebook_path()
        .join("proposals")
        .join(change_id)
        .join("meta.md");
    let body = std::fs::read_to_string(&meta).unwrap();
    assert!(
        body.contains("\"status\": \"draft\""),
        "precondition: fresh change is draft: {body}"
    );
    std::fs::write(
        &meta,
        body.replace(
            "\"status\": \"draft\"",
            &format!("\"status\": \"{status}\""),
        ),
    )
    .unwrap();
}

fn author_and_approve(fixture: &Fixture, change_id: &str) {
    let dir = fixture.notebook_path().join("proposals").join(change_id);
    let mut proposal =
        std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nProve retirement.\n");
    std::fs::write(dir.join("proposal.md"), proposal).unwrap();
    std::fs::write(dir.join("specifications/user-auth.md"), SPECIFICATION)
        .unwrap();
    let approved = nbspec(
        fixture,
        &[
            "review",
            change_id,
            "--verdict",
            "approve",
            "--reviewer",
            "itest",
            "--comment",
            "ok",
        ],
    );
    assert!(approved.status.success(), "{}", stderr_of(&approved));
}

fn copy_dir_recursive(source: &Path, destination: &Path) {
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir_recursive(&entry.path(), &target);
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Restores the retired namespace from the tarball's `namespace/`
/// subtree, as an operator would when recovering history. Restored
/// bytes are identical to the archived inventory.
fn restore_namespace_from_tarball(fixture: &Fixture, change_id: &str) {
    let tarball = fixture
        .project_root()
        .join("documentation/archives")
        .join(format!("{change_id}.tar.zst"));
    let bytes = std::fs::read(&tarball).unwrap();
    let scratch = fixture.project_root().join("restore-scratch");
    nbspec::archives::extract_archive(
        &bytes,
        &scratch,
        Some(&format!("{change_id}/namespace/")),
    )
    .unwrap();
    let destination =
        fixture.notebook_path().join("proposals").join(change_id);
    copy_dir_recursive(&scratch, &destination);
    std::fs::remove_dir_all(&scratch).ok();
}

/// Dead changes never merge: superseded refuses, and `--force` does
/// not bypass the refusal. No repository effects occur.
#[test]
fn dead_superseded_refuses_with_and_without_force() {
    for status in ["superseded", "abandoned"] {
        let fixture = Fixture::new();
        let created =
            nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
        assert!(created.status.success(), "{}", stderr_of(&created));
        set_status(&fixture, CHANGE_ID, status);
        for extra in [&[][..], &["--force"][..]] {
            let mut arguments = vec!["merge", CHANGE_ID];
            arguments.extend(extra.iter());
            let refused = nbspec(&fixture, &arguments);
            assert_eq!(refused.status.code(), Some(1), "status={status}");
            assert!(
                stderr_of(&refused).contains("dead changes do not merge"),
                "status={status}: {}",
                stderr_of(&refused)
            );
        }
        assert!(
            !fixture
                .project_root()
                .join("documentation/specifications/user-auth.md")
                .exists(),
            "status={status}: no repository effects"
        );
        assert!(
            !fixture
                .project_root()
                .join("documentation/archives/add-demo.tar.zst")
                .exists(),
            "status={status}: no archive"
        );
    }
}

/// The dead-status gate holds when archiving is disabled.
#[test]
fn dead_superseded_refuses_when_archives_disabled() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.configuration_directory().join("general.toml"),
        "schema = \"nbspec-default\"\n\
         scratch_directory = \".auxiliary/temporary/renders\"\n\
         archives = false\n\
         archive_directory = \"documentation/archives\"\n",
    )
    .unwrap();
    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    set_status(&fixture, CHANGE_ID, "superseded");
    let refused = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr_of(&refused).contains("dead changes do not merge"),
        "{}",
        stderr_of(&refused)
    );
}

/// A restored namespace byte-identical to the tarball resumes
/// deletion only: no materialization, no archive rewrite.
#[test]
fn restored_namespace_resumes_deletion() {
    let fixture = Fixture::new();
    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    author_and_approve(&fixture, CHANGE_ID);
    let merged = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(merged.status.success(), "{}", stderr_of(&merged));
    let tarball = fixture
        .project_root()
        .join("documentation/archives/add-demo.tar.zst");
    assert!(tarball.is_file());
    let target = fixture
        .project_root()
        .join("documentation/specifications/user-auth.md");
    let target_before = std::fs::read(&target).unwrap();
    let tarball_before = std::fs::read(&tarball).unwrap();

    restore_namespace_from_tarball(&fixture, CHANGE_ID);
    let resumed = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(resumed.status.success(), "{}", stderr_of(&resumed));
    let output = stdout_of(&resumed);
    assert!(
        output.contains("already archived"),
        "resume reports the prior archive: {output}"
    );
    assert!(
        output.contains("retired proposals/add-demo/"),
        "resume deletes: {output}"
    );
    assert!(
        !fixture
            .notebook_path()
            .join("proposals")
            .join(CHANGE_ID)
            .exists(),
        "namespace deleted on resume"
    );
    assert_eq!(
        std::fs::read(&target).unwrap(),
        target_before,
        "no materialization on resume"
    );
    assert_eq!(
        std::fs::read(&tarball).unwrap(),
        tarball_before,
        "no archive rewrite on resume"
    );
}

/// A restored namespace that drifted from the tarball refuses as
/// changed source: no writes, no publication, no deletion.
#[test]
fn modified_restored_namespace_refuses_changed_source() {
    let fixture = Fixture::new();
    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    author_and_approve(&fixture, CHANGE_ID);
    let merged = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(merged.status.success(), "{}", stderr_of(&merged));
    let tarball = fixture
        .project_root()
        .join("documentation/archives/add-demo.tar.zst");
    let target = fixture
        .project_root()
        .join("documentation/specifications/user-auth.md");
    let target_before = std::fs::read(&target).unwrap();
    let tarball_before = std::fs::read(&tarball).unwrap();

    restore_namespace_from_tarball(&fixture, CHANGE_ID);
    let drifted = fixture
        .notebook_path()
        .join("proposals")
        .join(CHANGE_ID)
        .join("specifications/user-auth.md");
    let mut content = std::fs::read_to_string(&drifted).unwrap();
    content.push_str("\nExtra unarchived line.\n");
    std::fs::write(&drifted, content).unwrap();

    let refused = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr_of(&refused).contains("diverges from the published tarball"),
        "{}",
        stderr_of(&refused)
    );
    assert!(
        fixture
            .notebook_path()
            .join("proposals")
            .join(CHANGE_ID)
            .is_dir(),
        "namespace untouched"
    );
    assert_eq!(
        std::fs::read(&tarball).unwrap(),
        tarball_before,
        "tarball untouched"
    );
    assert_eq!(
        std::fs::read(&target).unwrap(),
        target_before,
        "repository untouched"
    );
}

/// A symlink at the tarball path fails closed: presence alone never
/// proves already-merged and never authorizes deletion.
#[test]
fn symlink_tarball_fails_closed() {
    let fixture = Fixture::new();
    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    author_and_approve(&fixture, CHANGE_ID);
    let merged = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(merged.status.success(), "{}", stderr_of(&merged));
    restore_namespace_from_tarball(&fixture, CHANGE_ID);
    let tarball = fixture
        .project_root()
        .join("documentation/archives/add-demo.tar.zst");
    std::fs::remove_file(&tarball).unwrap();
    std::os::unix::fs::symlink("/dev/null", &tarball).unwrap();

    let refused = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr_of(&refused).contains("inspection"),
        "{}",
        stderr_of(&refused)
    );
    assert!(
        fixture
            .notebook_path()
            .join("proposals")
            .join(CHANGE_ID)
            .is_dir(),
        "namespace untouched"
    );
}

/// Import refuses a source id claimed by an on-disk tarball under
/// the configured archive directory.
#[test]
fn import_refuses_tarball_claimed_id() {
    let fixture = Fixture::new();
    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    author_and_approve(&fixture, CHANGE_ID);
    let merged = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(merged.status.success(), "{}", stderr_of(&merged));

    let source = fixture.project_root().join("import-source");
    std::fs::create_dir_all(source.join(CHANGE_ID)).unwrap();
    std::fs::write(
        source.join(CHANGE_ID).join("proposal.md"),
        "# Demo\n\nbody\n",
    )
    .unwrap();
    std::fs::write(
        source.join(CHANGE_ID).join("tasks.md"),
        "# [ ] Tasks\n- [ ] todo\n",
    )
    .unwrap();
    let imported =
        nbspec(&fixture, &["import", &source.display().to_string()]);
    assert_eq!(imported.status.code(), Some(1));
    let combined = format!("{}{}", stdout_of(&imported), stderr_of(&imported));
    assert!(
        combined.contains("retired tarball"),
        "collision names the tarball: {combined}"
    );
}

/// Post-materialization drift refuses before publication: the merge
/// writes repository outputs, then the snapshot barrier fails with
/// completed-phase context. No tarball appears, the namespace (drift
/// included) is retained, and retirement is not reported. The drift
/// is injected through the private `test-support`-gated
/// NBSPEC_TEST_DRIFT_AFTER_MATERIALIZE seam after materialization
/// returns, so the test is deterministic without cross-process
/// timing. Requires `--features test-support` (default builds omit
/// both the seam and this test).
#[cfg(feature = "test-support")]
#[test]
fn drift_after_materialization_refuses_before_publication() {
    use std::process::Command;
    let fixture = Fixture::new();
    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    author_and_approve(&fixture, CHANGE_ID);
    let target = fixture
        .project_root()
        .join("documentation/specifications/user-auth.md");
    let tarball = fixture
        .project_root()
        .join("documentation/archives/add-demo.tar.zst");

    let mut command = Command::new(env!("CARGO_BIN_EXE_nbspec"));
    fixture.configure_std(&mut command);
    let output = command
        .env("NBSPEC_CONFIG_DIR", fixture.configuration_directory())
        .args(["--notebook", fixture.notebook()])
        .args(["merge", CHANGE_ID])
        .env(
            "NBSPEC_TEST_DRIFT_AFTER_MATERIALIZE",
            "specifications/user-auth.md",
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let refusal = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        refusal.contains("materialization complete; archive not published"),
        "phase-aware failure: {refusal}"
    );
    assert!(
        target.is_file(),
        "repository outputs from materialization remain (as declared)"
    );
    assert!(
        !tarball.exists(),
        "no tarball is published from the drifted state"
    );
    assert!(
        fixture
            .notebook_path()
            .join("proposals")
            .join(CHANGE_ID)
            .is_dir(),
        "namespace retained"
    );
    let drifted = std::fs::read_to_string(
        fixture
            .notebook_path()
            .join("proposals")
            .join(CHANGE_ID)
            .join("specifications/user-auth.md"),
    )
    .unwrap();
    assert!(
        drifted.contains("Unarchived test drift."),
        "drift landed where the barrier checks"
    );
}
