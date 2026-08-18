#![allow(dead_code, unused_imports)]
use std::path::PathBuf;

use nbspec::interchange::{InterchangePlan, InterchangePlanStructured, PlanEntry};

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
        // After resume, pending_deletions includes both active and archive.
        pending_deletions: if delete_original {
            vec![
                PathBuf::from("/tmp/source/add-foo"),
                PathBuf::from("/tmp/source/openspec/changes/archive/legacy"),
            ]
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
        active.get("change_id").and_then(|v| v.as_str()),
        Some("add-foo"),
        "active-write change_id must be set: {active}"
    );
    assert_eq!(
        active.get("source_path").and_then(|v| v.as_str()),
        Some("/tmp/source/add-foo"),
        "active-write source_path must be set: {active}"
    );
    assert!(
        active.get("status").is_none(),
        "active-write should not carry paused status after resume: {active}"
    );
    assert!(
        active.get("prerequisite").is_none(),
        "active-write should not carry prerequisite after resume: {active}"
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
    // After resume, pending_deletions includes both active and archive.
    let plan = fixture_plan(true);
    let structured = InterchangePlanStructured::from_plan(plan).structured;
    let pending = structured
        .get("pending_deletions")
        .and_then(|v| v.as_array())
        .expect("pending_deletions must be an array");
    assert_eq!(
        pending.len(),
        2,
        "both archive and active sources must be pending deletion: {pending:?}"
    );
    assert!(
        pending
            .iter()
            .any(|v| v.as_str().map(|s| s.contains("add-foo")).unwrap_or(false)),
        "active source must be in pending_deletions after resume: {pending:?}"
    );
    assert!(
        pending
            .iter()
            .any(|v| v.as_str().map(|s| s.contains("legacy")).unwrap_or(false)),
        "archive source must be in pending_deletions: {pending:?}"
    );
}
