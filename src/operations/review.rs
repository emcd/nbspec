use nb_api::NbClient;
use serde_json::{Value, json};

use crate::rendering::{aggregate_content_hash, render_documents};
use crate::reviews::{
    KNOWN_GATES, VERDICTS_FOLDER, VerdictError, VerdictRecord, VerdictValue, read_verdicts,
    render_verdict_note, resolve_reviewer, reviewer_positions, verdict_note_name,
};
use crate::schemata::WorkflowSchema;

use super::context::load_change_context;
use super::helpers::folder_exists;
use super::{OperationError, OperationOutcome, OperationResult};

/// Builds the display `review` section: each reviewer's latest
/// verdict per known gate (supersession is an evaluation detail; the
/// operator sees every standing position), with parse failures
/// surfaced as explicit status rather than omission.
pub(crate) fn review_report(
    change_directory: &std::path::Path,
    folder: &str,
    schema: &WorkflowSchema,
) -> (String, Value) {
    let verdicts = match read_verdicts(change_directory) {
        Ok(verdicts) => verdicts,
        Err(VerdictError::Malformed { note, reason }) => {
            return (
                format!("verdicts unreadable: {note}: {reason}\n"),
                json!({ "parse_error": { "note": note, "reason": reason } }),
            );
        }
        Err(VerdictError::Io { path, source }) => {
            return (
                format!("verdicts unreadable: {path}: {source}\n"),
                json!({ "io_error": format!("{path}: {source}") }),
            );
        }
    };
    let documents = match render_documents(change_directory, folder, schema) {
        Ok(documents) => documents,
        Err(error) => {
            return (
                format!("cannot compute review status: {error}\n"),
                json!({ "render_error": error.to_string() }),
            );
        }
    };
    let current_hash = aggregate_content_hash(&documents);
    let mut text = String::new();
    let mut items: Vec<Value> = Vec::new();
    for gate in KNOWN_GATES {
        let positions = reviewer_positions(&verdicts, gate, &current_hash);
        if positions.is_empty() {
            text.push_str(&format!("{gate}: no verdicts recorded\n"));
            continue;
        }
        for position in &positions {
            let record = &position.verdict.record;
            let state = match (record.verdict, position.current) {
                (VerdictValue::Approve, true) => "current",
                (VerdictValue::Approve, false) => "stale",
                (VerdictValue::Revise, _) => "outstanding",
            };
            let comment = record
                .comment
                .as_deref()
                .map(|body| format!(" — {body}"))
                .unwrap_or_default();
            text.push_str(&format!(
                "{gate}: {verdict} by {reviewer} ({state}, {timestamp}){comment}\n",
                verdict = record.verdict,
                reviewer = record.reviewer,
                timestamp = record.timestamp,
            ));
            items.push(json!({
                "gate": gate,
                "reviewer": record.reviewer,
                "verdict": record.verdict.to_string(),
                "state": state,
                "current": position.current,
                "timestamp": record.timestamp.to_string(),
                "comment": record.comment,
            }));
        }
    }
    (text, json!({ "positions": items }))
}

/// Records a review verdict for a change at a gate.
///
/// Renders the change in memory, computes the aggregate content hash
/// of the rendered set, and creates ONE immutable verdict note under
/// the change's `verdicts/` subfolder. Existing verdict notes are
/// never modified; recording never transitions change lifecycle.
/// Writes nothing to the repository working tree.
///
/// # Errors
///
/// Returns [`OperationError::GateUnknown`] for a gate outside the
/// slice-1 set, [`OperationError::ReviewerUnresolved`] when neither
/// an explicit reviewer nor Git `user.name` yields a non-empty
/// identity, [`OperationError::ChangeNotFound`] when the change
/// namespace is absent, and notebook, schema, or IO errors otherwise.
pub async fn review(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
    gate: &str,
    verdict: VerdictValue,
    reviewer: Option<&str>,
    comment: Option<&str>,
) -> OperationResult {
    if !KNOWN_GATES.contains(&gate) {
        return Err(OperationError::GateUnknown {
            gate: gate.to_string(),
            known: KNOWN_GATES.join(", "),
        });
    }
    let reviewer = resolve_reviewer(reviewer).ok_or(OperationError::ReviewerUnresolved)?;
    let comment = comment.map(str::trim).filter(|text| !text.is_empty());
    if verdict == VerdictValue::Revise && comment.is_none() {
        return Err(OperationError::ReviseCommentMissing);
    }
    let context = load_change_context(client, notebook, change_id).await?;
    let aggregate_hash = aggregate_content_hash(&context.documents);
    let record = VerdictRecord {
        reviewer: reviewer.clone(),
        gate: gate.to_string(),
        verdict,
        aggregate_hash: aggregate_hash.clone(),
        timestamp: jiff::Timestamp::now(),
        comment: comment.map(str::to_string),
    };
    let name = verdict_note_name(&record.timestamp);
    let body = render_verdict_note(&name, &record)?;
    let verdicts_folder = format!("{}/{VERDICTS_FOLDER}", context.folder);
    let notebook = Some(context.notebook_name.as_str());
    // nb-api 0.3.0 one-shot `add_note` auto-names (ignores the title
    // argument for the filename), so the verdict note is written via
    // a transaction with an explicit path: the on-disk filename is
    // the verdict id (`{name}.md`). `Some(&name)` is passed as the
    // title so the note's display title is the verdict id; the body
    // deliberately omits the leading `# {name}` H1 to avoid the
    // duplicate-title-heading rejection. The transaction's
    // `CommitOutcome.ops` carries the authoritative note path, which
    // both the text output and the structured `note` field report.
    // The `verdicts/` parent folder (when missing, i.e. first review)
    // is enqueued in the SAME transaction — never committed by a
    // separate `ensure_folder` checkpoint — so a failed verdict
    // transaction rolls the parent root back too and leaves no
    // durable partial folder or checkpoint.
    let verdicts_missing = !folder_exists(client, &verdicts_folder, notebook).await;
    let mut tx = client.transaction(notebook).await?;
    if verdicts_missing {
        tx.add_folder(&verdicts_folder)?;
    }
    let note_path = format!("{verdicts_folder}/{name}.md");
    tx.add_note(&note_path, Some(&name), &body, &[])?;
    let outcome = tx.commit().await?;
    // Find the verdict-note operation by its explicit final path, not
    // by `ops.first()`: when the `verdicts/` folder was created in the
    // same transaction (first review), the folder op is op zero and
    // carries no selector, so the first op is not the note. The note
    // op's `selector` is the authoritative qualified
    // `<notebook>:<folder>/<file>` path; fall back to the explicit
    // unqualified path only if the op is absent.
    let created_note_path = outcome
        .ops
        .iter()
        .find(|op| op.path.as_deref() == Some(note_path.as_str()))
        .and_then(|op| op.selector.clone())
        .unwrap_or_else(|| note_path.clone());
    let text = format!(
        "Recorded {verdict} verdict by {reviewer} for change {change_id} at gate {gate}.\n\
         aggregate=sha256:{aggregate_hash}\n\
         note={created_note_path}",
    );
    let structured = json!({
        "change_id": change_id,
        "gate": gate,
        "verdict": verdict.to_string(),
        "reviewer": reviewer,
        "aggregate_hash": aggregate_hash,
        "note": created_note_path,
        "timestamp": record.timestamp.to_string(),
    });
    Ok(OperationOutcome::new(text, structured))
}
