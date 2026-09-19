//! Surgical delta planning: pre-validation, per-operation application,
//! and recomposition through `delta_apply::plan_delta_merge`.

use nbspec::delta_apply::plan_delta_merge;

const TARGET: &str = "\
# alpha

## ADDED Requirements

### Requirement: Alpha
The system SHALL alpha.

#### Scenario: Alphas
- **WHEN** alpha
- **THEN** alpha

### Requirement: Beta
The system SHALL beta.
";

fn plan(delta: &str, target: Option<&str>) -> Result<String, String> {
    plan_delta_merge(delta, target, "alpha", false)
        .map(|applied| applied.body)
        .map_err(|failure| failure.message)
}

#[test]
fn removed_deletes_block_preserving_order() {
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Gamma
The system SHALL gamma.

## REMOVED Requirements

### Requirement: Beta
";
    let body = plan(delta, Some(TARGET)).unwrap();
    assert!(
        !body.contains("### Requirement: Beta"),
        "removed block gone: {body}"
    );
    assert!(
        body.contains("### Requirement: Alpha"),
        "kept block stays: {body}"
    );
    assert!(
        body.contains("### Requirement: Gamma"),
        "added block appended: {body}"
    );
    assert!(
        body.find("### Requirement: Alpha").unwrap() < body.find("### Requirement: Gamma").unwrap(),
        "original order kept, additions appended: {body}"
    );
}

#[test]
fn removed_missing_warns_and_leaves_body_untouched() {
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Alpha
The system SHALL alpha.

#### Scenario: Alphas
- **WHEN** alpha
- **THEN** alpha

### Requirement: Beta
The system SHALL beta.

## REMOVED Requirements

### Requirement: Ghost
";
    let applied = plan_delta_merge(delta, Some(TARGET), "alpha", false).unwrap();
    assert_eq!(applied.body, TARGET, "already-removed is a no-op");
    assert_eq!(applied.warnings.len(), 1, "one already-removed warning");
    assert!(
        applied.warnings[0].contains("Ghost"),
        "warning names the requirement"
    );
    assert!(!applied.skipped);
}

#[test]
fn removed_near_miss_is_a_typo_error() {
    let delta = "\
# alpha

## REMOVED Requirements

### Requirement: alpha
";
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(
        message.contains("not found"),
        "missing reports not-found: {message}"
    );
    assert!(
        message.contains("Alpha"),
        "typo guard names the near miss: {message}"
    );
}

#[test]
fn modified_replaces_block_with_scenarios() {
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
The system SHALL alpha loudly.

#### Scenario: Alphas
- **WHEN** alpha
- **THEN** alpha loudly
";
    let body = plan(delta, Some(TARGET)).unwrap();
    assert!(
        body.contains("alpha loudly"),
        "modified text applied: {body}"
    );
    assert!(!body.contains("SHALL alpha.\n"), "old text gone: {body}");
    assert!(
        body.contains("### Requirement: Beta"),
        "untouched block kept: {body}"
    );
}

#[test]
fn modified_missing_refuses() {
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Ghost
New text.
";
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(
        message.contains("MODIFIED"),
        "names the operation: {message}"
    );
    assert!(
        message.contains("not found"),
        "dangling diagnosis: {message}"
    );
}

#[test]
fn modified_dropping_scenario_refuses() {
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
The system SHALL alpha loudly.
";
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(
        message.contains("Alphas"),
        "names the dropped scenario: {message}"
    );
}

#[test]
fn modified_against_absent_target_refuses() {
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Alpha
Changed text.
";
    let message = plan(delta, None).unwrap_err();
    assert!(
        message.contains("only ADDED"),
        "new documents take ADDED only: {message}"
    );
}

#[test]
fn renamed_reheaders_in_place() {
    let delta = "\
# alpha

## RENAMED Requirements

- FROM: `### Requirement: Alpha`
- TO: `### Requirement: Alpha2`
";
    let body = plan(delta, Some(TARGET)).unwrap();
    assert!(
        body.contains("### Requirement: Alpha2"),
        "renamed header: {body}"
    );
    assert!(
        !body.contains("Requirement: Alpha\n"),
        "old header gone: {body}"
    );
    assert!(
        body.find("Alpha2").unwrap() < body.find("### Requirement: Beta").unwrap(),
        "renamed block keeps its slot: {body}"
    );
    assert!(
        body.contains("SHALL alpha."),
        "block body untouched: {body}"
    );
}

#[test]
fn renamed_both_missing_refuses() {
    let delta = "\
# alpha

## RENAMED Requirements

- FROM: `### Requirement: Ghost`
- TO: `### Requirement: Spectre`
";
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(
        message.contains("source not found"),
        "dangling rename: {message}"
    );
}

#[test]
fn renamed_already_synced_is_a_noop() {
    let target = TARGET.replace("### Requirement: Alpha", "### Requirement: Alpha2");
    let delta = "\
# alpha

## RENAMED Requirements

- FROM: `### Requirement: Alpha`
- TO: `### Requirement: Alpha2`
";
    let applied = plan_delta_merge(delta, Some(&target), "alpha", false).unwrap();
    assert_eq!(applied.body, target, "synced rename changes nothing");
    assert!(applied.warnings.is_empty());
}

#[test]
fn renamed_to_existing_refuses() {
    let delta = "\
# alpha

## RENAMED Requirements

- FROM: `### Requirement: Alpha`
- TO: `### Requirement: Beta`
";
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(
        message.contains("already exists"),
        "collision refused: {message}"
    );
}

#[test]
fn added_appends_new_blocks() {
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Gamma
The system SHALL gamma.
";
    let body = plan(delta, Some(TARGET)).unwrap();
    let beta = body.find("### Requirement: Beta").unwrap();
    let gamma = body.find("### Requirement: Gamma").unwrap();
    assert!(
        beta < gamma,
        "additions append after existing blocks: {body}"
    );
}

#[test]
fn added_identical_is_a_noop() {
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Beta
The system SHALL beta.
";
    let applied = plan_delta_merge(delta, Some(TARGET), "alpha", false).unwrap();
    assert_eq!(applied.body, TARGET, "identical re-add changes nothing");
}

#[test]
fn added_differing_refuses() {
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Beta
The system SHALL beta differently.
";
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(
        message.contains("different content"),
        "collision refused: {message}"
    );
}

#[test]
fn incoherent_deltas_refuse() {
    let duplicate = "\
# alpha

## ADDED Requirements

### Requirement: Same
Text one.

### Requirement: Same
Text two.
";
    assert!(
        plan(duplicate, None).unwrap_err().contains("duplicate"),
        "duplicates within a section refuse"
    );
    let cross = "\
# alpha

## ADDED Requirements

### Requirement: Same
Text.

## REMOVED Requirements

### Requirement: Same
";
    assert!(
        plan(cross, Some(TARGET))
            .unwrap_err()
            .contains("multiple sections"),
        "cross-section conflicts refuse"
    );
    let rename_remove = "\
# alpha

## RENAMED Requirements

- FROM: `### Requirement: Alpha`
- TO: `### Requirement: Alpha2`

## REMOVED Requirements

### Requirement: Alpha
";
    assert!(
        plan(rename_remove, Some(TARGET))
            .unwrap_err()
            .contains("RENAMED"),
        "rename-source removal refuses"
    );
    let unpaired = "\
# alpha

## RENAMED Requirements

- FROM: `### Requirement: Alpha`
";
    assert!(
        plan(unpaired, Some(TARGET))
            .unwrap_err()
            .contains("no matching TO"),
        "unpaired FROM refuses"
    );
    let lone_to = "\
# alpha

## RENAMED Requirements

- TO: `### Requirement: Alpha2`
";
    assert!(
        plan(lone_to, Some(TARGET))
            .unwrap_err()
            .contains("no matching FROM"),
        "lone TO refuses"
    );
}

#[test]
fn new_document_builds_title_purpose_and_added() {
    let delta = "\
# Fresh capability

## Purpose

Why this exists.

## ADDED Requirements

### Requirement: First
The system SHALL first.

## REMOVED Requirements

### Requirement: Ghost
";
    let applied = plan_delta_merge(delta, None, "fresh", false).unwrap();
    assert!(
        applied.body.starts_with("# Fresh capability\n"),
        "title kept"
    );
    assert!(
        applied.body.contains("## Purpose\n\nWhy this exists."),
        "purpose seeded"
    );
    assert!(
        applied.body.contains("### Requirement: First"),
        "added carried"
    );
    assert!(
        !applied.body.contains("Ghost"),
        "ignored removals leave no trace"
    );
    assert_eq!(applied.warnings.len(), 1, "ignored removal warns");
    assert!(!applied.skipped);
}

#[test]
fn removed_only_against_absent_target_skips() {
    let delta = "\
# alpha

## REMOVED Requirements

### Requirement: Ghost
";
    let applied = plan_delta_merge(delta, None, "alpha", false).unwrap();
    assert!(applied.skipped, "nothing effective means skip");
    assert!(applied.body.is_empty(), "no skeleton materializes");
    assert_eq!(applied.warnings.len(), 1, "skip warns");
}

#[test]
fn purpose_difference_warns() {
    let target = "# alpha\n\n## Purpose\n\nOld why.\n\n## ADDED Requirements\n";
    let delta = "\
# alpha

## Purpose

New why.

## ADDED Requirements

### Requirement: First
The system SHALL first.
";
    let applied = plan_delta_merge(delta, Some(target), "alpha", false).unwrap();
    assert!(
        applied
            .warnings
            .iter()
            .any(|warning| warning.contains("Purpose")),
        "differing delta Purpose warns"
    );
    assert!(applied.body.contains("Old why."), "target Purpose stands");
}

#[test]
fn rename_then_remove_by_new_name() {
    let delta = "\
# alpha

## RENAMED Requirements

- FROM: `### Requirement: Alpha`
- TO: `### Requirement: Alpha2`

## REMOVED Requirements

### Requirement: Alpha2
";
    let body = plan(delta, Some(TARGET)).unwrap();
    assert!(
        !body.contains("Alpha2"),
        "renamed-then-removed is gone: {body}"
    );
    assert!(
        !body.contains("Requirement: Alpha\n"),
        "old header gone: {body}"
    );
    assert!(
        body.contains("### Requirement: Beta"),
        "others kept: {body}"
    );
}

#[test]
fn all_four_operations_compose_in_order() {
    let delta = "\
# alpha

## ADDED Requirements

### Requirement: Gamma
The system SHALL gamma.

## MODIFIED Requirements

### Requirement: Beta2
The system SHALL beta, revised.

## REMOVED Requirements

### Requirement: Alpha

## RENAMED Requirements

- FROM: `### Requirement: Beta`
- TO: `### Requirement: Beta2`
";
    let body = plan(delta, Some(TARGET)).unwrap();
    assert!(!body.contains("Requirement: Alpha\n"), "removed: {body}");
    assert!(
        !body.contains("### Requirement: Beta\n"),
        "old name gone: {body}"
    );
    assert!(body.contains("### Requirement: Beta2"), "renamed: {body}");
    assert!(body.contains("beta, revised"), "modified: {body}");
    assert!(body.contains("### Requirement: Gamma"), "added: {body}");
    assert!(
        body.find("Beta2").unwrap() < body.find("Gamma").unwrap(),
        "renamed block keeps its slot, additions append: {body}"
    );
}

#[test]
fn modified_naming_rename_source_refuses() {
    let delta = "\
# alpha

## MODIFIED Requirements

### Requirement: Beta
The system SHALL beta, revised.

## RENAMED Requirements

- FROM: `### Requirement: Beta`
- TO: `### Requirement: Beta2`
";
    // MODIFIED must reference the NEW header once a rename exists.
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(
        message.contains("NEW header"),
        "rename/modified interplay: {message}"
    );
}

#[test]
fn crlf_noop_round_trips_bytes() {
    // The reviewer's exact case: missing REMOVED on a CRLF target
    // with deliberate spacing must not rewrite a single byte.
    let target = "# alpha\r\n\r\n## ADDED Requirements\r\n\r\n### Requirement: Alpha\r\nText.\r\n";
    let delta = "# alpha\n\n## REMOVED Requirements\n\n### Requirement: Ghost\n";
    let applied = plan_delta_merge(delta, Some(target), "alpha", false).unwrap();
    assert_eq!(applied.body, target, "no-op preserves bytes and EOL");
    assert_eq!(applied.warnings.len(), 1);
}

#[test]
fn deliberate_blanks_survive_noop() {
    let target = "# alpha\n\n\n## ADDED Requirements\n\n\n### Requirement: Alpha\nText.\n";
    let delta = "# alpha\n\n## REMOVED Requirements\n\n### Requirement: Ghost\n";
    let applied = plan_delta_merge(delta, Some(target), "alpha", false).unwrap();
    assert_eq!(applied.body, target, "deliberate spacing preserved");
}

#[test]
fn crlf_edits_adopt_crlf() {
    let target = "# alpha\r\n\r\n## ADDED Requirements\r\n\r\n### Requirement: Alpha\r\nText.\r\n\r\n### Requirement: Beta\r\nOld.\r\n";
    let delta = "# alpha\n\n## REMOVED Requirements\n\n### Requirement: Beta\n";
    let applied = plan_delta_merge(delta, Some(target), "alpha", false).unwrap();
    assert!(!applied.body.contains("Beta"), "removed");
    let bare_lf = applied
        .body
        .match_indices('\n')
        .any(|(index, _)| index == 0 || applied.body.as_bytes()[index - 1] != b'\r');
    assert!(!bare_lf, "no bare LF in CRLF output");
}

#[test]
fn duplicate_target_names_refuse() {
    let target = "# alpha\n\n## ADDED Requirements\n\n### Requirement: Same\nOne.\n\n### Requirement: Same\nTwo.\n";
    let delta = "# alpha\n\n## MODIFIED Requirements\n\n### Requirement: Same\nThree.\n";
    let message = plan(delta, Some(target)).unwrap_err();
    assert!(
        message.contains("duplicate"),
        "twin blocks refuse: {message}"
    );
}

#[test]
fn duplicate_sections_refuse() {
    let delta = "# alpha\n\n## ADDED Requirements\n\n### Requirement: A\nX.\n\n## ADDED Requirements\n\n### Requirement: B\nY.\n";
    let message = plan(delta, None).unwrap_err();
    assert!(
        message.contains("duplicate"),
        "repeated sections refuse: {message}"
    );
}

#[test]
fn rename_chain_remerges_clean() {
    let target = "# alpha\n\n## ADDED Requirements\n\n### Requirement: C\nText.\n";
    let delta = "# alpha\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: A`\n- TO: `### Requirement: B`\n\n- FROM: `### Requirement: B`\n- TO: `### Requirement: C`\n";
    let applied = plan_delta_merge(delta, Some(target), "alpha", false).unwrap();
    assert_eq!(applied.body, target, "applied chain remerges silently");
    assert!(applied.warnings.is_empty());
}

#[test]
fn rename_cycle_refuses() {
    let delta = "# alpha\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: A`\n- TO: `### Requirement: B`\n\n- FROM: `### Requirement: B`\n- TO: `### Requirement: A`\n";
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(message.contains("cycle"), "cycles refuse: {message}");
}

#[test]
fn delta_purpose_without_target_purpose_warns() {
    let target = "# alpha\n\n## ADDED Requirements\n\n### Requirement: Alpha\nText.\n";
    let delta =
        "# alpha\n\n## Purpose\n\nWhy.\n\n## ADDED Requirements\n\n### Requirement: Alpha\nText.\n";
    let applied = plan_delta_merge(delta, Some(target), "alpha", false).unwrap();
    assert!(
        applied
            .warnings
            .iter()
            .any(|warning| warning.contains("Purpose")),
        "missing target Purpose still warns"
    );
    assert!(
        !applied.body.contains("## Purpose"),
        "nothing seeded into existing targets"
    );
}

#[test]
fn rename_targets_recorded_span_not_scan() {
    // A same-named header under a non-addressable section must not
    // capture the rename: the live block moves, the prose stays.
    let target = "# alpha\n\n## REMOVED Requirements\n\n### Requirement: Old\nRecord.\n\n## ADDED Requirements\n\n### Requirement: Old\nLive.\n";
    let delta = "# alpha\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: Old`\n- TO: `### Requirement: New`\n";
    let body = plan(delta, Some(target)).unwrap();
    assert_eq!(
        body.matches("### Requirement: New").count(),
        1,
        "exactly one rename lands: {body}"
    );
    assert!(
        body.contains("### Requirement: Old\nRecord."),
        "prose untouched: {body}"
    );
}

#[test]
fn fenced_added_heading_does_not_capture_insertion() {
    // A fenced example ADDED section before the live structure must
    // not capture the append; remerging the result is a silent no-op.
    let target = "# alpha\n\n```md\n## ADDED Requirements\n\n### Requirement: Ghost\nText.\n```\n\n## ADDED Requirements\n\n### Requirement: Alpha\nText.\n";
    let delta = "# alpha\n\n## ADDED Requirements\n\n### Requirement: Beta\nText.\n";
    let first = plan_delta_merge(delta, Some(target), "alpha", false).unwrap();
    assert!(
        first.body.contains("```md\n## ADDED Requirements"),
        "example preserved verbatim"
    );
    let live = first
        .body
        .find("```\n\n## ADDED Requirements")
        .expect("live section");
    assert!(
        first.body[live..].contains("### Requirement: Beta"),
        "append lands in the live section: {}",
        first.body
    );
    let second = plan_delta_merge(delta, Some(&first.body), "alpha", false).unwrap();
    assert_eq!(second.body, first.body, "remerge is byte-identical");
    assert!(second.warnings.is_empty());
}

#[test]
fn tilde_fenced_added_heading_does_not_capture() {
    let target = "# alpha\n\n~~~\n## ADDED Requirements\n~~~\n\n## ADDED Requirements\n\n### Requirement: Alpha\nText.\n";
    let delta = "# alpha\n\n## ADDED Requirements\n\n### Requirement: Beta\nText.\n";
    let applied = plan_delta_merge(delta, Some(target), "alpha", false).unwrap();
    assert_eq!(applied.body.matches("### Requirement: Beta").count(), 1);
    assert!(applied.body.contains("~~~\n## ADDED Requirements\n~~~"));
}

#[test]
fn repeated_target_sections_all_apply() {
    let target = "# alpha\n\n## ADDED Requirements\n\n### Requirement: A\nOne.\n\n## ADDED Requirements\n\n### Requirement: B\nTwo.\n";
    let delta = "# alpha\n\n## REMOVED Requirements\n\n### Requirement: B\n";
    let body = plan(delta, Some(target)).unwrap();
    assert!(
        !body.contains("### Requirement: B"),
        "second-section block removed: {body}"
    );
    assert!(
        body.contains("### Requirement: A"),
        "first-section block kept: {body}"
    );
}

#[test]
fn initial_chain_applies_transitively() {
    // A→B→C against a target holding only A ends as C: the created
    // intermediate satisfies the second link in the same fixpoint.
    let target = "# alpha\n\n## ADDED Requirements\n\n### Requirement: A\nOne.\n";
    let delta = "# alpha\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: A`\n- TO: `### Requirement: B`\n\n- FROM: `### Requirement: B`\n- TO: `### Requirement: C`\n";
    let body = plan(delta, Some(target)).unwrap();
    assert!(
        body.contains("### Requirement: C\nOne."),
        "transitive rename: {body}"
    );
    assert!(
        !body.contains("Requirement: A\n"),
        "no stale source: {body}"
    );
    assert!(
        !body.contains("Requirement: B\n"),
        "no stranded intermediate: {body}"
    );
}

#[test]
fn repeated_target_modified_sections_all_apply() {
    let target = "# alpha\n\n## MODIFIED Requirements\n\n### Requirement: A\nOne.\n\n## MODIFIED Requirements\n\n### Requirement: B\nTwo.\n";
    let delta = "# alpha\n\n## MODIFIED Requirements\n\n### Requirement: B\nRevised.\n";
    let body = plan(delta, Some(target)).unwrap();
    assert!(
        body.contains("Revised."),
        "second-section block replaced: {body}"
    );
}

#[test]
fn reverse_declared_chain_converges() {
    // Same chain, opposite declaration order: identical outcome.
    let target = "# alpha\n\n## ADDED Requirements\n\n### Requirement: A\nOne.\n\n### Requirement: B\nTwo.\n";
    let forward = "# alpha\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: A`\n- TO: `### Requirement: B2`\n\n- FROM: `### Requirement: B`\n- TO: `### Requirement: C`\n";
    let reverse = "# alpha\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: B`\n- TO: `### Requirement: C`\n\n- FROM: `### Requirement: A`\n- TO: `### Requirement: B2`\n";
    let forward_body = plan(forward, Some(target)).unwrap();
    let reverse_body = plan(reverse, Some(target)).unwrap();
    assert_eq!(
        forward_body, reverse_body,
        "declaration order is irrelevant"
    );
    assert!(forward_body.contains("### Requirement: B2"));
    assert!(forward_body.contains("### Requirement: C"));
}

#[test]
fn partially_applied_chain_completes() {
    // B→C already applied earlier (A and C present): A→B completes
    // the chain, B→C resolves already-synced.
    let target = "# alpha\n\n## ADDED Requirements\n\n### Requirement: A\nOne.\n\n### Requirement: C\nThree.\n";
    let delta = "# alpha\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: A`\n- TO: `### Requirement: B`\n\n- FROM: `### Requirement: B`\n- TO: `### Requirement: C`\n";
    let body = plan(delta, Some(target)).unwrap();
    assert!(
        body.contains("### Requirement: B\nOne."),
        "chain completes: {body}"
    );
    assert!(
        body.contains("### Requirement: C\nThree."),
        "synced link untouched: {body}"
    );
}

#[test]
fn folded_cycle_refuses() {
    let delta = "# alpha\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: A`\n- TO: `### Requirement: b`\n\n- FROM: `### Requirement: B`\n- TO: `### Requirement: A`\n";
    let message = plan(delta, Some(TARGET)).unwrap_err();
    assert!(message.contains("cycle"), "folded cycles refuse: {message}");
}

#[test]
fn append_preserves_bytes_lf_and_crlf() {
    // Exact byte assertions: deliberate blanks before a following
    // H2 and at EOF survive on both LF and CRLF, existing-section
    // and create-section paths.
    let lf =
        "# alpha\n\n## ADDED Requirements\n\n### Requirement: A\nOne.\n\n\n## Designs\n\nProse.\n";
    let delta = "# alpha\n\n## ADDED Requirements\n\n### Requirement: B\nTwo.\n";
    let lf_body = plan(delta, Some(lf)).unwrap();
    assert!(
        lf_body.contains("### Requirement: B\nTwo.\n\n\n## Designs"),
        "append tucks before existing separators: {lf_body:?}"
    );
    let crlf = lf.replace('\n', "\r\n");
    let crlf_body = plan(delta, Some(&crlf)).unwrap();
    assert_eq!(
        crlf_body,
        crlf_body.replace("\r\n", "\n").replace('\n', "\r\n"),
        "CRLF stays CRLF throughout"
    );
    assert!(crlf_body.contains("### Requirement: B\r\nTwo.\r\n\r\n\r\n## Designs"));
    // Create-section path on a section-less target keeps EOF shape.
    let plain = "# alpha\n\n## Purpose\n\nWhy.\n";
    let created = plan(delta, Some(plain)).unwrap();
    assert!(created.contains("## ADDED Requirements\n\n### Requirement: B\nTwo.\n"));
    assert!(created.ends_with("Two.\n") && !created.ends_with("\n\n"));
    let plain_crlf = plain.replace('\n', "\r\n");
    let created_crlf = plan(delta, Some(&plain_crlf)).unwrap();
    assert!(created_crlf.contains("## ADDED Requirements\r\n\r\n### Requirement: B\r\nTwo.\r\n"));
}
