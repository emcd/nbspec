use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use nbspec::reviews::{
    GateEvaluation, VerdictError, VerdictRecord, VerdictValue, evaluate_gate, read_verdicts,
    render_verdict_note, reviewer_positions, verdict_note_name,
};

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

fn record(
    reviewer: &str,
    gate: &str,
    verdict: VerdictValue,
    hash: &str,
    at: &str,
) -> VerdictRecord {
    VerdictRecord {
        reviewer: reviewer.to_string(),
        gate: gate.to_string(),
        verdict,
        aggregate_hash: hash.to_string(),
        timestamp: at.parse::<Timestamp>().unwrap(),
        comment: None,
    }
}

fn write_verdict(change: &Path, name: &str, payload: &VerdictRecord) {
    let directory = change.join("verdicts");
    fs::create_dir_all(&directory).unwrap();
    let body = render_verdict_note(name, payload).unwrap();
    fs::write(directory.join(format!("{name}.md")), body).unwrap();
}

#[test]
fn round_trips_a_rendered_verdict_note() {
    let root = unique_temp_root("reviews-round-trip");
    let payload = record(
        "advisor",
        "merge",
        VerdictValue::Approve,
        "abc123",
        "2026-07-11T02:00:00Z",
    );
    write_verdict(&root, "20260711020000-1-000001", &payload);
    let verdicts = read_verdicts(&root).unwrap();
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].record, payload);
    assert_eq!(verdicts[0].note, "20260711020000-1-000001.md");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn absent_folder_means_no_verdicts() {
    let root = unique_temp_root("reviews-absent");
    fs::create_dir_all(&root).unwrap();
    assert!(read_verdicts(&root).unwrap().is_empty());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn skips_dotfiles_and_non_markdown() {
    let root = unique_temp_root("reviews-skip");
    let payload = record(
        "advisor",
        "merge",
        VerdictValue::Approve,
        "abc",
        "2026-07-11T02:00:00Z",
    );
    write_verdict(&root, "20260711020000-1-000001", &payload);
    fs::write(root.join("verdicts/.index"), "20260711020000-1-000001.md\n").unwrap();
    fs::write(root.join("verdicts/notes.txt"), "not a verdict\n").unwrap();
    let verdicts = read_verdicts(&root).unwrap();
    assert_eq!(verdicts.len(), 1);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn malformed_note_fails_loudly_naming_the_note() {
    let root = unique_temp_root("reviews-malformed");
    fs::create_dir_all(root.join("verdicts")).unwrap();
    fs::write(root.join("verdicts/bogus.md"), "# bogus\n\nno fence here\n").unwrap();
    let error = read_verdicts(&root).expect_err("malformed note must fail the read");
    match error {
        VerdictError::Malformed { note, reason } => {
            assert_eq!(note, "verdicts/bogus.md");
            assert!(reason.contains("no fenced JSON block"), "reason: {reason}");
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn empty_reviewer_is_malformed() {
    let root = unique_temp_root("reviews-empty-reviewer");
    let payload = record(
        "  ",
        "merge",
        VerdictValue::Approve,
        "abc",
        "2026-07-11T02:00:00Z",
    );
    write_verdict(&root, "20260711020000-1-000001", &payload);
    let error = read_verdicts(&root).expect_err("empty reviewer must fail");
    assert!(error.to_string().contains("reviewer is empty"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn two_fenced_blocks_are_malformed() {
    let root = unique_temp_root("reviews-two-fences");
    fs::create_dir_all(root.join("verdicts")).unwrap();
    fs::write(
        root.join("verdicts/double.md"),
        "# double\n\n```json\n{}\n```\n\n```json\n{}\n```\n",
    )
    .unwrap();
    let error = read_verdicts(&root).expect_err("two fences must fail");
    assert!(error.to_string().contains("more than one fenced block"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn crlf_note_parses_identically() {
    let root = unique_temp_root("reviews-crlf");
    let payload = record(
        "advisor",
        "merge",
        VerdictValue::Revise,
        "abc",
        "2026-07-11T02:00:00Z",
    );
    let body = render_verdict_note("20260711020000-1-000002", &payload).unwrap();
    let crlf = body.replace('\n', "\r\n");
    fs::create_dir_all(root.join("verdicts")).unwrap();
    fs::write(root.join("verdicts/20260711020000-1-000002.md"), crlf).unwrap();
    let verdicts = read_verdicts(&root).unwrap();
    assert_eq!(verdicts[0].record, payload);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unknown_payload_fields_are_tolerated() {
    let root = unique_temp_root("reviews-unknown-field");
    fs::create_dir_all(root.join("verdicts")).unwrap();
    fs::write(
        root.join("verdicts/future.md"),
        "# future\n\n```json\n{\n  \"reviewer\": \"advisor\",\n  \"gate\": \"merge\",\n  \
         \"verdict\": \"approve\",\n  \"aggregate_hash\": \"abc\",\n  \
         \"timestamp\": \"2026-07-11T02:00:00Z\",\n  \"quorum_role\": \"owner\"\n}\n```\n",
    )
    .unwrap();
    let verdicts = read_verdicts(&root).unwrap();
    assert_eq!(verdicts[0].record.reviewer, "advisor");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn supersession_prefers_newer_timestamp_then_note_identifier() {
    let root = unique_temp_root("reviews-supersession");
    write_verdict(
        &root,
        "20260711020000-1-000001",
        &record(
            "advisor",
            "merge",
            VerdictValue::Revise,
            "old",
            "2026-07-11T02:00:00Z",
        ),
    );
    write_verdict(
        &root,
        "20260711030000-1-000002",
        &record(
            "advisor",
            "merge",
            VerdictValue::Approve,
            "new",
            "2026-07-11T03:00:00Z",
        ),
    );
    let verdicts = read_verdicts(&root).unwrap();
    let positions = reviewer_positions(&verdicts, "merge", "new");
    assert_eq!(positions.len(), 1, "one reviewer, one standing position");
    assert_eq!(positions[0].verdict.record.verdict, VerdictValue::Approve);
    assert!(positions[0].current);

    // Exact timestamp tie: the lexicographically later note wins,
    // deterministically.
    write_verdict(
        &root,
        "20260711030000-1-000003",
        &record(
            "advisor",
            "merge",
            VerdictValue::Revise,
            "new",
            "2026-07-11T03:00:00Z",
        ),
    );
    let verdicts = read_verdicts(&root).unwrap();
    let positions = reviewer_positions(&verdicts, "merge", "new");
    assert_eq!(positions[0].verdict.note, "20260711030000-1-000003.md");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn distinct_reviewers_coexist_and_other_gates_are_non_applicable() {
    let root = unique_temp_root("reviews-coexist");
    write_verdict(
        &root,
        "20260711020000-1-000001",
        &record(
            "advisor",
            "merge",
            VerdictValue::Approve,
            "hash",
            "2026-07-11T02:00:00Z",
        ),
    );
    write_verdict(
        &root,
        "20260711020100-1-000002",
        &record(
            "owner",
            "merge",
            VerdictValue::Revise,
            "hash",
            "2026-07-11T02:01:00Z",
        ),
    );
    write_verdict(
        &root,
        "20260711020200-1-000003",
        &record(
            "owner",
            "publish",
            VerdictValue::Approve,
            "hash",
            "2026-07-11T02:02:00Z",
        ),
    );
    let verdicts = read_verdicts(&root).unwrap();
    let positions = reviewer_positions(&verdicts, "merge", "hash");
    assert_eq!(positions.len(), 2, "publish-gate verdict is non-applicable");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn gate_evaluation_covers_all_slice_one_outcomes() {
    let root = unique_temp_root("reviews-evaluate");
    let verdicts = read_verdicts(&root.join("nonexistent")).unwrap();
    let positions = reviewer_positions(&verdicts, "merge", "hash");
    assert!(matches!(
        evaluate_gate(&positions),
        GateEvaluation::NoVerdict
    ));

    write_verdict(
        &root,
        "20260711020000-1-000001",
        &record(
            "advisor",
            "merge",
            VerdictValue::Approve,
            "stale-hash",
            "2026-07-11T02:00:00Z",
        ),
    );
    write_verdict(
        &root,
        "20260711020100-1-000002",
        &record(
            "owner",
            "merge",
            VerdictValue::Revise,
            "current",
            "2026-07-11T02:01:00Z",
        ),
    );
    let verdicts = read_verdicts(&root).unwrap();
    let positions = reviewer_positions(&verdicts, "merge", "current");
    // A stale approval is a re-review away from satisfied; it is
    // reported in preference to the outstanding revise.
    assert!(matches!(
        evaluate_gate(&positions),
        GateEvaluation::StaleApproval(v) if v.record.reviewer == "advisor"
    ));

    write_verdict(
        &root,
        "20260711030000-1-000003",
        &record(
            "advisor",
            "merge",
            VerdictValue::Approve,
            "current",
            "2026-07-11T03:00:00Z",
        ),
    );
    let verdicts = read_verdicts(&root).unwrap();
    let positions = reviewer_positions(&verdicts, "merge", "current");
    assert!(matches!(
        evaluate_gate(&positions),
        GateEvaluation::Satisfied(v) if v.record.reviewer == "advisor"
    ));

    let positions = reviewer_positions(&verdicts, "merge", "moved-again");
    assert!(matches!(
        evaluate_gate(&positions),
        GateEvaluation::StaleApproval(_)
    ));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn revise_outstanding_reported_when_no_approvals_exist() {
    let root = unique_temp_root("reviews-revise");
    write_verdict(
        &root,
        "20260711020000-1-000001",
        &record(
            "owner",
            "merge",
            VerdictValue::Revise,
            "hash",
            "2026-07-11T02:00:00Z",
        ),
    );
    let verdicts = read_verdicts(&root).unwrap();
    let positions = reviewer_positions(&verdicts, "merge", "hash");
    assert!(matches!(
        evaluate_gate(&positions),
        GateEvaluation::ReviseOutstanding(_)
    ));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn note_names_are_unique_across_calls() {
    let timestamp = "2026-07-11T02:00:00Z".parse::<Timestamp>().unwrap();
    let first = verdict_note_name(&timestamp);
    let second = verdict_note_name(&timestamp);
    assert_ne!(first, second, "entropy suffix must differ across calls");
    assert!(first.starts_with("20260711"));
}

#[test]
fn gate_refusal_state_describes_each_unsatisfied_kind() {
    use nbspec::reviews::gate_refusal_state;
    let root = unique_temp_root("reviews-refusal-state");
    write_verdict(
        &root,
        "20260711020000-1-000001",
        &record(
            "advisor",
            "merge",
            VerdictValue::Approve,
            "bound-hash",
            "2026-07-11T02:00:00Z",
        ),
    );
    let verdicts = read_verdicts(&root).unwrap();

    let positions = reviewer_positions(&verdicts, "merge", "bound-hash");
    assert_eq!(
        gate_refusal_state(&evaluate_gate(&positions), "bound-hash"),
        None,
        "a satisfied gate needs no refusal state"
    );

    let positions = reviewer_positions(&verdicts, "merge", "current-hash");
    let state = gate_refusal_state(&evaluate_gate(&positions), "current-hash").unwrap();
    assert!(
        state.contains("stale approval by advisor"),
        "state: {state}"
    );
    assert!(state.contains("bound-hash"), "names the bound hash");
    assert!(state.contains("current-hash"), "names the current hash");

    let positions = reviewer_positions(&verdicts, "publish", "current-hash");
    let state = gate_refusal_state(&evaluate_gate(&positions), "current-hash").unwrap();
    assert!(state.contains("no verdict"), "state: {state}");

    let mut revise = record(
        "owner",
        "merge",
        VerdictValue::Revise,
        "current-hash",
        "2026-07-11T03:00:00Z",
    );
    revise.comment = Some("findings at reviews/9".to_string());
    write_verdict(&root, "20260711030000-1-000002", &revise);
    let verdicts = read_verdicts(&root).unwrap();
    let positions = reviewer_positions(&verdicts, "merge", "current-hash");
    // The stale approval still outranks revise in reporting; drop the
    // approve reviewer's currency entirely to surface the revise arm.
    let owner_only: Vec<_> = positions
        .into_iter()
        .filter(|p| p.verdict.record.reviewer == "owner")
        .collect();
    let state = gate_refusal_state(&evaluate_gate(&owner_only), "current-hash").unwrap();
    assert!(
        state.contains("revise by owner (findings at reviews/9)"),
        "state: {state}"
    );
}
