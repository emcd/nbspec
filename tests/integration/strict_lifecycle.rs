//! Strict-mode scenario-drop lifecycle: first change merges a spec
//! with two scenarios, second change MODIFIED-drops one (warns and
//! applies), and a strict attempt at the same drop refuses before any
//! write. Drives the compiled binary against scratch fixtures.

use super::harness::Fixture;
use super::lifecycle::{CHANGE_ID, nbspec, stderr_of, stdout_of};

/// Scenario-drop end-to-end with strict: the first change merges a
/// spec carrying a scenario; the second change MODIFIED-drops it.
/// Without strict the merge warns ScenarioDropped and applies; with
/// strict on a third change the same drop refuses before any write.
#[test]
fn scenario_drop_warns_applies_and_strict_refuses() {
    const DROP_ID: &str = "add-drop";
    const STRICT_ID: &str = "add-strict";
    const BASE_SPEC: &str = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users.

#### Scenario: Valid login
- **WHEN** valid credentials arrive
- **THEN** a session begins

#### Scenario: Legacy path
- **WHEN** legacy credentials arrive
- **THEN** a session begins
";
    const DROP_SPEC: &str = "\
# user-auth

## MODIFIED Requirements

### Requirement: User authentication
The system SHALL authenticate users.

#### Scenario: Valid login
- **WHEN** valid credentials arrive
- **THEN** a session begins
";
    let fixture = Fixture::new();
    for (id, spec) in [(CHANGE_ID, BASE_SPEC), (DROP_ID, DROP_SPEC)] {
        let created = nbspec(&fixture, &["create", id, "--title", "Demo"]);
        assert!(created.status.success(), "{}", stderr_of(&created));
        let dir = fixture.notebook_path().join("proposals").join(id);
        let mut proposal =
            std::fs::read_to_string(dir.join("proposal.md")).unwrap();
        proposal.push_str("\n## Why\n\nProve scenario-drop.\n");
        std::fs::write(dir.join("proposal.md"), proposal).unwrap();
        std::fs::write(dir.join("specifications/user-auth.md"), spec).unwrap();
        let valid = nbspec(&fixture, &["validate", id]);
        assert!(valid.status.success(), "{}", stderr_of(&valid));
        let approved = nbspec(
            &fixture,
            &[
                "review",
                id,
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
    let first = nbspec(&fixture, &["merge", CHANGE_ID]);
    assert!(first.status.success(), "{}", stderr_of(&first));
    // Non-strict drop warns with the ScenarioDropped code and applies.
    let second = nbspec(&fixture, &["merge", DROP_ID]);
    assert!(second.status.success(), "{}", stderr_of(&second));
    let output = stdout_of(&second);
    assert!(
        output.contains("ScenarioDropped"),
        "text names the diagnostic code: {output}"
    );
    assert!(
        output.contains("Legacy path"),
        "text names the dropped scenario: {output}"
    );
    let target = fixture
        .project_root()
        .join("documentation/specifications/user-auth.md");
    let merged = std::fs::read_to_string(&target).unwrap();
    assert!(
        !merged.contains("Legacy path"),
        "dropped scenario gone: {merged}"
    );
    assert!(merged.contains("Valid login"), "kept scenario stays");
    // Strict on the same drop shape refuses before any write. Build
    // a third change against the CURRENT target (which still holds
    // both scenarios after we restore it below is unnecessary —
    // instead reuse the base target by merging strict first is
    // impossible post-drop; so assert refusal against a fresh
    // fixture target holding both scenarios).
    let fixture2 = Fixture::new();
    for (id, spec) in [(CHANGE_ID, BASE_SPEC), (STRICT_ID, DROP_SPEC)] {
        let created = nbspec(&fixture2, &["create", id, "--title", "Demo"]);
        assert!(created.status.success(), "{}", stderr_of(&created));
        let dir = fixture2.notebook_path().join("proposals").join(id);
        let mut proposal =
            std::fs::read_to_string(dir.join("proposal.md")).unwrap();
        proposal.push_str("\n## Why\n\nProve strict refusal.\n");
        std::fs::write(dir.join("proposal.md"), proposal).unwrap();
        std::fs::write(dir.join("specifications/user-auth.md"), spec).unwrap();
        let approved = nbspec(
            &fixture2,
            &[
                "review",
                id,
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
    let base = nbspec(&fixture2, &["merge", CHANGE_ID]);
    assert!(base.status.success(), "{}", stderr_of(&base));
    let refused = nbspec(&fixture2, &["merge", "--strict", STRICT_ID]);
    assert!(!refused.status.success(), "strict must refuse");
    let stderr = stderr_of(&refused);
    assert!(
        stderr.contains("ScenarioDropped"),
        "refusal names the code: {stderr}"
    );
    assert!(
        stderr.contains("Legacy path"),
        "refusal names the scenario: {stderr}"
    );
    let current = std::fs::read_to_string(
        fixture2
            .project_root()
            .join("documentation/specifications/user-auth.md"),
    )
    .unwrap();
    assert!(
        current.contains("Legacy path"),
        "strict refusal writes nothing: {current}"
    );
}

/// Strict with `archives = false` on a clean change: materializes
/// with the retention notice (outside strict scope), keeps warnings
/// compatibility, and reports no diagnostics. A strict refusal
/// publishes no archive and retires nothing.
#[test]
fn strict_archives_disabled_clean_merges_and_refusal_retires_nothing() {
    // Part A: clean change, strict, archives disabled.
    let fixture = Fixture::new();
    let created = nbspec(&fixture, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created.status.success(), "{}", stderr_of(&created));
    let dir = fixture.notebook_path().join("proposals").join(CHANGE_ID);
    let mut proposal =
        std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nProve strict with archives disabled.\n");
    std::fs::write(dir.join("proposal.md"), proposal).unwrap();
    std::fs::write(
        dir.join("specifications/user-auth.md"),
        "# user-auth\n\n## ADDED Requirements\n\n### Requirement: Clean\nThe system SHALL stay clean.\n",
    )
    .unwrap();
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
    let config_dir = fixture
        .project_root()
        .join(".auxiliary/configuration/nbspec");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("general.toml"), "archives = false\n")
        .unwrap();
    let merged = nbspec(&fixture, &["merge", "--strict", CHANGE_ID]);
    assert!(merged.status.success(), "{}", stderr_of(&merged));
    let output = stdout_of(&merged);
    assert!(
        output.contains("archives disabled"),
        "retention notice preserved: {output}"
    );
    assert!(
        output.contains("wrote documentation/specifications/user-auth.md"),
        "clean strict merge writes: {output}"
    );
    assert!(
        fixture
            .notebook_path()
            .join("proposals")
            .join(CHANGE_ID)
            .exists(),
        "archives=false retains the namespace"
    );
    assert!(
        !fixture
            .project_root()
            .join("documentation/archives")
            .join(format!("{CHANGE_ID}.tar.zst"))
            .exists(),
        "archives=false publishes no tarball"
    );

    // Part B: strict refusal publishes no archive and retires nothing.
    let fixture2 = Fixture::new();
    let created2 =
        nbspec(&fixture2, &["create", CHANGE_ID, "--title", "Demo"]);
    assert!(created2.status.success(), "{}", stderr_of(&created2));
    let dir2 = fixture2.notebook_path().join("proposals").join(CHANGE_ID);
    let mut proposal2 =
        std::fs::read_to_string(dir2.join("proposal.md")).unwrap();
    proposal2.push_str("\n## Why\n\nProve refusal side effects.\n");
    std::fs::write(dir2.join("proposal.md"), proposal2).unwrap();
    std::fs::write(
        dir2.join("specifications/user-auth.md"),
        "# user-auth\n\n## ADDED Requirements\n\n### Requirement: New\nText.\n\n## REMOVED Requirements\n\n### Requirement: Ghost\n",
    )
    .unwrap();
    let approved2 = nbspec(
        &fixture2,
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
    assert!(approved2.status.success(), "{}", stderr_of(&approved2));
    let refused = nbspec(&fixture2, &["merge", "--strict", CHANGE_ID]);
    assert!(!refused.status.success(), "strict must refuse");
    let stderr = stderr_of(&refused);
    assert!(
        stderr.contains("AlreadyRemoved"),
        "refusal names the code: {stderr}"
    );
    assert!(
        !fixture2
            .project_root()
            .join("documentation/specifications/user-auth.md")
            .exists(),
        "refusal writes no target"
    );
    assert!(
        !fixture2
            .project_root()
            .join("documentation/archives")
            .join(format!("{CHANGE_ID}.tar.zst"))
            .exists(),
        "refusal publishes no archive"
    );
    assert!(
        fixture2
            .notebook_path()
            .join("proposals")
            .join(CHANGE_ID)
            .exists(),
        "refusal retires no namespace"
    );
}
