use nb_api::{NbClient, NbError};
use serde_json::{Map, Value, json};

use crate::changes::{change_folder, namespace_folders, namespace_notes, validate_change_id};
use crate::merging::target_status;
use crate::rendering::render_documents;
use crate::schemata::WorkflowSchema;
use crate::worknotes::WorkChecklist;

use super::context::project_root;
use super::helpers::{
    artifact_has_content, folder_listing, load_metadata, metadata_summary, read_work_note,
    render_work_checklist, schema_for, show_note_source,
};
use super::{OperationError, OperationOutcome, OperationResult};

/// Displays a change. The short form reports the meta summary,
/// artifact readiness against the schema `requires` graph, `work`
/// todo progress, and drift; `full` additionally includes root
/// artifact note contents and per-artifact-folder listings.
///
/// # Errors
///
/// Returns notebook access errors and meta note parse failures.
pub async fn display(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
    full: bool,
) -> OperationResult {
    validate_change_id(change_id)?;
    let notebook_name = super::context::resolve_notebook_name(notebook)?;
    let notebook = Some(notebook_name.as_str());
    let folder = change_folder(change_id);
    let metadata = load_metadata(client, &folder, notebook).await?;
    let schema = schema_for(&metadata)?;

    let mut output = metadata_summary(&metadata);
    if full {
        for note in namespace_notes(&schema) {
            let selector = format!("{folder}/{note}.md");
            let body = match client.show_note(&selector, notebook).await {
                Ok(show) => {
                    show_note_source(&show).unwrap_or_else(|error| format!("(unreadable: {error})"))
                }
                Err(NbError::NotFound { .. }) => "(missing)".to_string(),
                Err(error) => format!("(unreadable: {error})"),
            };
            output.push_str(&format!("\n## {note}\n\n{body}\n"));
        }
    }
    output.push_str("\n## artifacts\n\n");
    let mut authored: Vec<&str> = Vec::new();
    let mut artifact_states: Vec<Value> = Vec::new();
    for artifact in schema.authoring_order() {
        let content_result =
            artifact_has_content(client, &folder, &schema, &artifact.id, notebook).await;
        let unmet: Vec<&str> = artifact
            .requires
            .iter()
            .map(String::as_str)
            .filter(|dependency| !authored.contains(dependency))
            .collect();
        let state = match &content_result {
            Ok(true) => {
                authored.push(artifact.id.as_str());
                "authored".to_string()
            }
            Ok(false) if unmet.is_empty() => "ready to author".to_string(),
            Ok(false) => format!("blocked on {}", unmet.join(", ")),
            Err(msg) => format!("unreadable: {msg}"),
        };
        output.push_str(&format!("- {}: {state}\n", artifact.id));
        let mut entry = Map::new();
        entry.insert("id".to_string(), Value::String(artifact.id.clone()));
        entry.insert("state".to_string(), Value::String(state));
        if !unmet.is_empty() {
            entry.insert(
                "blocked_on".to_string(),
                Value::Array(unmet.iter().map(|s| Value::String(s.to_string())).collect()),
            );
        }
        artifact_states.push(Value::Object(entry));
    }
    if full {
        for subfolder in namespace_folders(&schema) {
            let listing = folder_listing(client, &format!("{folder}/{subfolder}"), notebook).await;
            let listing_text = listing.unwrap_or_else(|msg| format!("(unreadable: {msg})"));
            output.push_str(&format!("\n## {subfolder}/\n\n{listing_text}\n"));
        }
    }

    output.push_str("\n## work\n\n");
    let change_directory = client
        .show_notebook_path(notebook)
        .await?
        .join(change_folder(change_id));
    let work_summary = work_summary(&change_directory);
    let work_structured = match &work_summary {
        WorkSummary::Checklist(checklist) => {
            let (complete, total) = checklist.progress();
            json!({ "complete": complete, "total": total })
        }
        WorkSummary::Missing => json!({ "complete": 0, "total": 0, "missing": true }),
        WorkSummary::ParseError(message) => {
            json!({ "complete": 0, "total": 0, "parse_error": message })
        }
    };
    output.push_str(&render_work_report(&work_summary));

    output.push_str("\n## review\n\n");
    let (review_text, review_structured) =
        super::review::review_report(&change_directory, &folder, &schema);
    output.push_str(&review_text);

    output.push_str("\n## drift\n\n");
    let drift_lines = drift_report_lines(&change_directory, &folder, &schema, change_id)?;
    output.push_str(&drift_lines.text);
    let structured_drift: Vec<Value> = drift_lines
        .items
        .iter()
        .map(|item| {
            json!({
                "path": item.path,
                "status": item.status,
            })
        })
        .collect();

    let mut structured = Map::new();
    structured.insert(
        "change_id".to_string(),
        Value::String(metadata.change_id.clone()),
    );
    structured.insert(
        "title".to_string(),
        metadata
            .title
            .as_ref()
            .map(|t| Value::String(t.clone()))
            .unwrap_or(Value::Null),
    );
    structured.insert(
        "status".to_string(),
        Value::String(metadata.status.to_string()),
    );
    structured.insert("schema".to_string(), Value::String(metadata.schema.clone()));
    structured.insert(
        "notebook".to_string(),
        Value::String(metadata.notebook.clone()),
    );
    structured.insert("review".to_string(), review_structured);
    structured.insert(
        "updated_at".to_string(),
        Value::String(metadata.updated_at.to_string()),
    );
    structured.insert("artifacts".to_string(), Value::Array(artifact_states));
    structured.insert("work".to_string(), work_structured);
    structured.insert("drift".to_string(), Value::Array(structured_drift));

    Ok(OperationOutcome::new(output, Value::Object(structured)))
}

/// Reports the merge-target status of every durable document for
/// `display`. Returns both a text rendering and a typed list of
/// `(path, status)` items so the structured payload does not have to
/// scrape the text.
fn drift_report_lines(
    change_directory: &std::path::Path,
    folder: &str,
    schema: &WorkflowSchema,
    change_id: &str,
) -> Result<DriftReportLines, OperationError> {
    let root = project_root();
    let documents = match render_documents(change_directory, folder, schema) {
        Ok(documents) => documents,
        Err(error) => {
            return Ok(DriftReportLines {
                text: format!("cannot compute drift: {error}\n"),
                items: Vec::new(),
            });
        }
    };
    let mut items: Vec<DriftItem> = Vec::new();
    let mut text = String::new();
    for document in &documents {
        let Some(target_path) = &document.target_path else {
            continue;
        };
        let status = target_status(document, &root, change_id)?;
        let status_text = status.to_string();
        text.push_str(&format!("- {target_path}: {status_text}\n"));
        items.push(DriftItem {
            path: target_path.clone(),
            status: status_text,
        });
    }
    if text.is_empty() {
        text.push_str("no durable documents with merge targets yet\n");
    }
    Ok(DriftReportLines { text, items })
}

#[derive(Debug)]
struct DriftReportLines {
    text: String,
    items: Vec<DriftItem>,
}

#[derive(Debug)]
struct DriftItem {
    path: String,
    status: String,
}

/// Categorizes what `work_report` should render for a given change
/// directory. The display path needs both a typed summary (for the
/// structured payload) and a text rendering (for the existing
/// `display --full` view); parsing the text back out is fragile.
enum WorkSummary {
    Checklist(WorkChecklist),
    Missing,
    ParseError(String),
}

/// Reads the work todo note and returns a typed summary without
/// rendering any text.
fn work_summary(change_directory: &std::path::Path) -> WorkSummary {
    let Some(content) = read_work_note(change_directory) else {
        return WorkSummary::Missing;
    };
    match crate::worknotes::parse_work_note(&content) {
        Ok(checklist) => WorkSummary::Checklist(checklist),
        Err(error) => WorkSummary::ParseError(error.to_string()),
    }
}

/// Renders the text form of a `WorkSummary`. Kept separate from
/// `work_summary` so callers needing structured data only can stop
/// at `work_summary` without paying for text formatting.
fn render_work_report(summary: &WorkSummary) -> String {
    match summary {
        WorkSummary::Checklist(checklist) => render_work_checklist(checklist),
        WorkSummary::Missing => "(no work todo note found)\n".to_string(),
        WorkSummary::ParseError(message) => format!("{message}\n"),
    }
}
