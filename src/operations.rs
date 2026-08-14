//! Core change operations shared by the CLI and future MCP surface.
//!
//! Each public function corresponds to one user-facing verb. All
//! notebook access flows through [`nb_api::NbClient`]; only `merge`
//! may write to the repository working tree. Operations resolve the
//! effective notebook themselves — the explicit per-call argument, or
//! the Git-derived project notebook when `None` — and pass the
//! resolved name to every client call, so recorded metadata and
//! notebook writes always agree. The client's own configured default
//! is never consulted, because [`nb_api::NbClient`] does not expose
//! it; callers targeting a non-derived notebook must pass it
//! explicitly. Project configuration resolves against the Git
//! repository root, so operations behave identically from any
//! subdirectory.

use std::path::{Path, PathBuf};

use nb_api::{NbClient, NbError, ShowNote};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::archives::{ArchiveEntry, ArchiveError, build_archive, gitattributes_covers_lfs};
use crate::changes::{
    ChangeError, ChangeMetadata, META_NOTE, PROPOSALS_FOLDER, WORK_NOTE, change_folder,
    namespace_folders, namespace_notes, note_has_authored_content, parse_meta_note,
    render_meta_note, validate_change_id,
};
use crate::configuration::{Configuration, ConfigurationError, load_configuration};
use crate::merging::{MergeError, merge_documents, target_status};
use crate::rendering::{
    RenderError, aggregate_content_hash, render_documents, review_diff, write_tree,
};
use crate::reviews::{
    KNOWN_GATES, MERGE_GATE, VERDICTS_FOLDER, VerdictError, VerdictRecord, VerdictValue,
    evaluate_gate, gate_refusal_state, read_verdicts, render_verdict_note, resolve_reviewer,
    reviewer_positions, verdict_note_name,
};
use crate::schemata::{SchemaError, WorkflowSchema, resolve_schema};
use crate::validation::{ValidationFailure, validate_change};
use crate::worknotes::{WorkChecklist, WorkNoteError, parse_work_note};

/// Tag applied to nbspec-managed control-plane notes.
const META_TAG: &str = "nbspec";

/// Errors from nbspec core operations.
#[derive(Debug, Error)]
pub enum OperationError {
    #[error("change already exists: {0}")]
    AlreadyExists(String),

    #[error("change not found in notebook {notebook}: {change_id}")]
    ChangeNotFound { notebook: String, change_id: String },

    #[error("cannot read note file {path}: {source}")]
    NoteRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error(
        "notebook not configured; pass --notebook or run within a Git repository \
         with a derivable notebook name"
    )]
    NotebookUnresolved,

    #[error("nb invocation failed: {0}")]
    Nb(#[from] nb_api::NbError),

    #[error(transparent)]
    Change(#[from] ChangeError),

    #[error(transparent)]
    Configuration(#[from] ConfigurationError),

    #[error(transparent)]
    Schema(#[from] SchemaError),

    #[error(transparent)]
    WorkNote(#[from] WorkNoteError),

    #[error(transparent)]
    Render(#[from] RenderError),

    #[error(transparent)]
    Merge(#[from] MergeError),

    #[error(transparent)]
    Archive(#[from] ArchiveError),

    #[error(transparent)]
    Invalid(#[from] ValidationFailure),

    #[error("cannot write archive {path}: {source}")]
    ArchiveWrite {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("unknown review gate {gate:?}; known gates: {known}")]
    GateUnknown { gate: String, known: String },

    #[error("reviewer identity unresolved; pass --reviewer or set git config user.name")]
    ReviewerUnresolved,

    #[error(
        "a revise verdict requires a comment naming the findings; pass --comment \
         (or --comment - on the CLI to read standard input)"
    )]
    ReviseCommentMissing,

    #[error(transparent)]
    Verdict(#[from] VerdictError),

    #[error("cannot encode verdict payload: {0}")]
    VerdictEncode(#[from] serde_json::Error),
}

/// Result alias for core operations.
pub type OperationResult = Result<OperationOutcome, OperationError>;

/// The outcome of a successful operation: the same text the CLI prints,
/// plus a structured payload covering the operation's natural data. Both
/// surfaces consume this — the CLI prints `text`, the MCP server returns
/// `text` and `structured` together so clients can branch on typed data
/// instead of scraping prose.
#[derive(Clone, Debug)]
pub struct OperationOutcome {
    pub text: String,
    pub structured: Value,
}

impl OperationOutcome {
    /// Wraps `text` and `structured` as a successful outcome.
    pub fn new(text: impl Into<String>, structured: Value) -> Self {
        Self {
            text: text.into(),
            structured,
        }
    }
}

/// Creates a change namespace in the project notebook.
///
/// Scaffolds `proposals/<change-id>/` with a populated `meta` note, a
/// `work` todo note, artifact notes, and artifact subfolders per the
/// resolved schema. Writes nothing to the repository working tree.
///
/// # Errors
///
/// Returns [`OperationError::AlreadyExists`] when the namespace is
/// already present, and notebook, schema, or validation errors
/// otherwise.
pub async fn create(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
    title: Option<&str>,
) -> OperationResult {
    validate_change_id(change_id)?;
    let notebook_name = resolve_notebook_name(notebook)?;
    let notebook = Some(notebook_name.as_str());
    let configuration = load_configuration(&project_root())?;
    let schema = resolve_schema(None, &configuration)?;
    let folder = change_folder(change_id);

    if folder_exists(client, &folder, notebook).await {
        return Err(OperationError::AlreadyExists(change_id.to_string()));
    }

    // One transaction materializes the whole namespace under a single
    // checkpoint: the `proposals/` parent root (when missing), the
    // change folder, its artifact subfolders, the meta note, the
    // placeholder notes, and the work todo. Explicit paths are
    // mandatory because nb-api 0.3.0 one-shot `add_note` auto-names
    // (ignoring the title argument for the filename), and the change
    // namespace depends on stable names (`meta.md`, `proposal.md`,
    // `work.todo.md`, ...). Enqueueing the parent root here (rather
    // than committing it via a separate `ensure_folder` checkpoint)
    // keeps first-create atomic: a transaction failure rolls back the
    // parent root too, so no durable partial folder or checkpoint
    // survives.
    let proposals_missing = !folder_exists(client, PROPOSALS_FOLDER, notebook).await;
    let metadata = ChangeMetadata::new(change_id, title, &schema.name, &notebook_name)?;
    let mut tx = client.transaction(notebook).await?;
    if proposals_missing {
        tx.add_folder(PROPOSALS_FOLDER)?;
    }
    tx.add_folder(&folder)?;
    for subfolder in namespace_folders(&schema) {
        tx.add_folder(&format!("{folder}/{subfolder}"))?;
    }
    tx.add_note(
        &format!("{folder}/{META_NOTE}.md"),
        Some(META_NOTE),
        &render_meta_note(&metadata)?,
        &[META_TAG.to_string()],
    )?;
    for note in namespace_notes(&schema) {
        let placeholder = format!("<!-- Draft the {note} here. -->\n");
        tx.add_note(
            &format!("{folder}/{note}.md"),
            Some(&note),
            &placeholder,
            &[],
        )?;
    }
    tx.add_todo(
        &format!("{folder}/{WORK_NOTE}.todo.md"),
        WORK_NOTE,
        Some(&format!("Execution checklist for {change_id}.")),
        &[],
        &[META_TAG.to_string()],
    )?;
    tx.commit().await?;

    let text = format!(
        "Created change {change_id} (schema {schema_name}) under {folder}/ in notebook {notebook_name}.",
        schema_name = schema.name,
    );
    let structured = json!({
        "change_id": change_id,
        "schema": schema.name,
        "folder": folder,
        "notebook": notebook_name,
    });
    Ok(OperationOutcome::new(text, structured))
}

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
    let notebook_name = resolve_notebook_name(notebook)?;
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
    let (review_text, review_structured) = review_report(&change_directory, &folder, &schema);
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
    match parse_work_note(&content) {
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

/// Renders a change to a scratch workspace for review.
///
/// Reads artifact notes from the notebook directory and writes the
/// tree the schema `generates` paths declare, replacing any previous
/// render of the same change. With `diff`, the returned output is a
/// unified diff against current merge targets — nothing else — so it
/// pipes cleanly into review tooling; otherwise it reports the
/// scratch destination. The repository working tree is never
/// modified.
///
/// # Errors
///
/// Returns [`OperationError::ChangeNotFound`] when the change
/// namespace is absent, and notebook, configuration, schema, or IO
/// errors otherwise.
pub async fn render(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
    diff: bool,
) -> OperationResult {
    let context = load_change_context(client, notebook, change_id).await?;
    let destination = render_destination(&context.configuration, &context.notebook_name, change_id);
    write_tree(&context.documents, &destination)?;
    if diff {
        let text = review_diff(&context.documents, &context.root)?;
        let lines = text.lines().count();
        let structured = json!({
            "change_id": change_id,
            "format": "diff",
            "lines": lines,
        });
        return Ok(OperationOutcome::new(text, structured));
    }
    let text = format!(
        "Rendered {count} documents of change {change_id} to {destination}.",
        count = context.documents.len(),
        destination = destination.display(),
    );
    let structured = json!({
        "change_id": change_id,
        "format": "tree",
        "documents_count": context.documents.len(),
        "destination": destination.display().to_string(),
    });
    Ok(OperationOutcome::new(text, structured))
}

/// Transfers a change's durable artifacts into the repository.
///
/// Renders the change from its notes and writes the target-bearing
/// documents to their configured repository destinations with
/// provenance headers. Planning collects every violation before any
/// write, so a refused merge modifies nothing; `force` overrides
/// target-state refusals (drift, unmanaged files, foreign ownership)
/// but never unsupported delta operations or non-file occupants.
/// This is the only nbspec operation that writes to the repository,
/// and it creates no git commits. Archive writing happens after the
/// documents transfer: an archive IO failure therefore leaves
/// already-merged documents in place — an accepted trade-off, since
/// rerunning merge is idempotent and completes the archive.
///
/// # Errors
///
/// Returns [`OperationError::ChangeNotFound`] when the change
/// namespace is absent, [`MergeError::Refused`] (wrapped) listing
/// every violating target, and notebook, configuration, schema, or
/// IO errors otherwise.
pub async fn merge(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
    force: bool,
) -> OperationResult {
    let context = load_change_context(client, notebook, change_id).await?;
    let aggregate = aggregate_content_hash(&context.documents);
    let review_gate_state = match read_verdicts(&context.change_directory) {
        Ok(verdicts) => {
            let positions = reviewer_positions(&verdicts, MERGE_GATE, &aggregate);
            gate_refusal_state(&evaluate_gate(&positions), &aggregate)
        }
        // An unparseable verdict is a plan-phase POLICY refusal
        // (force-overridable), not a hard error: it blocks the gate
        // while naming the note, but never hides behind an abort.
        Err(VerdictError::Malformed { note, reason }) => {
            Some(format!("verdict unparseable: {note}: {reason}"))
        }
        Err(error @ VerdictError::Io { .. }) => return Err(error.into()),
    };
    let report = merge_documents(
        &context.documents,
        &context.root,
        change_id,
        &context.notebook_name,
        review_gate_state.as_deref(),
        force,
    )?;

    let mut output = String::new();
    if let Some(state) = &report.review_gate_overridden {
        output.push_str(&format!("REVIEW GATE OVERRIDDEN (--force): {state}\n"));
    }
    for takeover in &report.drift_overrides {
        output.push_str(&format!(
            "DRIFTED TARGET OVERRIDDEN (--force): {target} (was owned by {previous})\n",
            target = takeover.target,
            previous = takeover.previous_owner,
        ));
    }
    for path in &report.stale_target_overrides {
        output.push_str(&format!(
            "STALE TARGET (--force): {path} left behind by H1-slug rename; remove manually\n"
        ));
    }
    for succession in &report.successions {
        output.push_str(&format!(
            "TAKEOVER (clean succession): {target}: {previous} -> {change_id}\n",
            target = succession.target,
            previous = succession.previous_owner,
        ));
    }
    for path in &report.written {
        output.push_str(&format!("wrote {path}\n"));
    }
    for path in &report.unchanged {
        output.push_str(&format!("unchanged {path}\n"));
    }
    if report.written.is_empty() && report.unchanged.is_empty() {
        output.push_str("no durable documents to merge\n");
    }
    let archived_path = if context.configuration.archives {
        let archive_output = write_change_archive(
            &context.configuration,
            &context.root,
            &context.change_directory,
            change_id,
            &context.documents,
        )?;
        output.push_str(&archive_output);
        // Parse the "archived <path>" line for structured reporting;
        // warnings are kept in text only.
        archive_output
            .lines()
            .find_map(|line| line.strip_prefix("archived ").map(|rest| rest.to_string()))
    } else {
        None
    };
    output.push_str(&format!(
        "Merged change {change_id}: {written} written, {unchanged} unchanged.",
        written = report.written.len(),
        unchanged = report.unchanged.len(),
    ));
    let successions: Vec<Value> = report
        .successions
        .iter()
        .map(|succession| {
            json!({
                "target": succession.target,
                "previous_owner": succession.previous_owner,
            })
        })
        .collect();
    let drift_overrides: Vec<Value> = report
        .drift_overrides
        .iter()
        .map(|takeover| {
            json!({
                "target": takeover.target,
                "previous_owner": takeover.previous_owner,
            })
        })
        .collect();
    let structured = json!({
        "change_id": change_id,
        "written": report.written,
        "unchanged": report.unchanged,
        "archived": archived_path,
        "review_gate_overridden": report.review_gate_overridden,
        "successions": successions,
        "drift_overrides": drift_overrides,
        "stale_target_overrides": report.stale_target_overrides,
    });
    Ok(OperationOutcome::new(output, structured))
}

/// Writes the merge-time change archive: the rendered artifact tree
/// plus `meta.md` and a `work.md` checklist snapshot, packed
/// deterministically under a top-level `<change-id>/` prefix.
/// Returns report lines, including a warning when no `.gitattributes`
/// rule marks the archive path for Git LFS.
fn write_change_archive(
    configuration: &Configuration,
    root: &std::path::Path,
    change_directory: &std::path::Path,
    change_id: &str,
    documents: &[crate::rendering::RenderedDocument],
) -> Result<String, OperationError> {
    let prefix = PathBuf::from(change_id);
    let mut entries: Vec<ArchiveEntry> = documents
        .iter()
        .map(|document| ArchiveEntry {
            path: prefix.join(Path::new(&document.tree_path)),
            content: document.content.clone(),
        })
        .collect();
    let meta_path = change_directory.join(format!("{META_NOTE}.md"));
    let meta_content =
        std::fs::read_to_string(&meta_path).map_err(|source| OperationError::NoteRead {
            path: meta_path,
            source,
        })?;
    entries.push(ArchiveEntry {
        path: prefix.join(format!("{META_NOTE}.md")),
        content: meta_content,
    });
    if let Some(work_content) = read_work_note(change_directory) {
        entries.push(ArchiveEntry {
            path: prefix.join(format!("{WORK_NOTE}.md")),
            content: work_content,
        });
    }
    // Verdict notes ride the archive EXPLICITLY: nothing from the
    // change namespace is included automatically, and the review
    // trail must survive the change. Files are copied raw — the
    // archive preserves even a malformed verdict rather than
    // validating it away. (build_archive sorts entries by path, so
    // determinism holds regardless of push order.)
    let verdicts_directory = change_directory.join(VERDICTS_FOLDER);
    if verdicts_directory.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(&verdicts_directory)
            .map_err(|source| OperationError::NoteRead {
                path: verdicts_directory.clone(),
                source,
            })?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.') && name.ends_with(".md"))
            .collect();
        names.sort();
        for name in names {
            let path = verdicts_directory.join(&name);
            let content =
                std::fs::read_to_string(&path).map_err(|source| OperationError::NoteRead {
                    path: path.clone(),
                    source,
                })?;
            entries.push(ArchiveEntry {
                path: prefix.join(VERDICTS_FOLDER).join(&name),
                content,
            });
        }
    }
    let bytes = build_archive(&entries)?;

    let archive_path = configuration
        .archive_directory
        .join(format!("{change_id}.tar.zst"));
    let absolute = root.join(&archive_path);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).map_err(|source| OperationError::ArchiveWrite {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&absolute, &bytes).map_err(|source| OperationError::ArchiveWrite {
        path: absolute.clone(),
        source,
    })?;

    let mut output = format!("archived {}\n", archive_path.display());
    if !gitattributes_covers_lfs(root, &archive_path) {
        output.push_str(&format!(
            "warning: no .gitattributes rule marks {} for Git LFS\n",
            archive_path.display()
        ));
    }
    Ok(output)
}

/// Validates a change against the OpenSpec grammar and its schema.
///
/// Checks schema-required artifacts for authored content and
/// delta-specification documents for grammar conformance, natively —
/// no external binary. A valid change yields a single summary line
/// and process success; an invalid change yields
/// [`ValidationFailure`] (wrapped), whose display lists one
/// `note:line: [artifact] message` diagnostic per line, anchored to
/// notebook notes rather than filesystem paths. Neither outcome
/// touches the repository working tree or the scratch workspace.
///
/// # Errors
///
/// Returns [`OperationError::Invalid`] listing every violation,
/// [`OperationError::ChangeNotFound`] when the change namespace is
/// absent, and notebook, configuration, schema, or IO errors
/// otherwise.
pub async fn validate(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
) -> OperationResult {
    let context = load_change_context(client, notebook, change_id).await?;
    let diagnostics = validate_change(&context.documents, &context.schema, &context.folder);
    if !diagnostics.is_empty() {
        return Err(OperationError::Invalid(ValidationFailure {
            change_id: change_id.to_string(),
            diagnostics,
        }));
    }
    let text = format!(
        "Change {change_id} is valid: {count} documents checked against schema {schema}.",
        count = context.documents.len(),
        schema = context.schema.name,
    );
    let structured = json!({
        "valid": true,
        "change_id": change_id,
        "documents_checked": context.documents.len(),
        "schema": context.schema.name,
    });
    Ok(OperationOutcome::new(text, structured))
}

/// Resolved context shared by operations that read a change from the
/// notebook filesystem (render, merge, validate): the effective
/// notebook and project roots, loaded configuration, resolved
/// schema, and the change's rendered document list.
struct ChangeContext {
    notebook_name: String,
    root: PathBuf,
    configuration: Configuration,
    folder: String,
    change_directory: PathBuf,
    schema: WorkflowSchema,
    documents: Vec<crate::rendering::RenderedDocument>,
}

/// Resolves the shared operation preamble: validates the change id,
/// resolves the notebook and project root, loads configuration,
/// locates the change directory, and renders the change's documents
/// in memory per its meta-note schema.
///
/// # Errors
///
/// Returns [`OperationError::ChangeNotFound`] when the change
/// namespace is absent, and notebook, configuration, schema, or IO
/// errors otherwise.
async fn load_change_context(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
) -> Result<ChangeContext, OperationError> {
    validate_change_id(change_id)?;
    let notebook_name = resolve_notebook_name(notebook)?;
    let root = project_root();
    let configuration = load_configuration(&root)?;
    let folder = change_folder(change_id);
    let change_directory = client
        .show_notebook_path(Some(notebook_name.as_str()))
        .await?
        .join(&folder);
    if !change_directory.is_dir() {
        return Err(OperationError::ChangeNotFound {
            notebook: notebook_name,
            change_id: change_id.to_string(),
        });
    }
    let metadata = read_metadata(&change_directory)?;
    let schema = resolve_schema(Some(&metadata.schema), &configuration)?;
    let documents = render_documents(&change_directory, &folder, &schema)?;
    Ok(ChangeContext {
        notebook_name,
        root,
        configuration,
        folder,
        change_directory,
        schema,
        documents,
    })
}

/// Resolves the effective notebook for an operation: the explicit
/// argument, or the Git-derived project notebook. Operations pass the
/// resolved name to every client call rather than deferring to the
/// client's configured default, which [`nb_api::NbClient`] does not
/// expose; an effective-default getter in nb-api would allow deferring
/// instead.
fn resolve_notebook_name(notebook: Option<&str>) -> Result<String, OperationError> {
    notebook
        .map(String::from)
        .or_else(nb_api::derive_git_notebook_name)
        .ok_or(OperationError::NotebookUnresolved)
}

/// Builds the display `review` section: each reviewer's latest
/// verdict per known gate (supersession is an evaluation detail; the
/// operator sees every standing position), with parse failures
/// surfaced as explicit status rather than omission.
fn review_report(
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

/// Resolves the project repository root, falling back to the current
/// directory outside a Git repository.
fn project_root() -> PathBuf {
    nb_api::git_rev_parse(&["--show-toplevel"]).unwrap_or_else(|| PathBuf::from("."))
}

/// Reads and parses a change's meta note from the notebook
/// filesystem.
fn read_metadata(change_directory: &std::path::Path) -> Result<ChangeMetadata, OperationError> {
    let path = change_directory.join(format!("{META_NOTE}.md"));
    let content = std::fs::read_to_string(&path)
        .map_err(|source| OperationError::NoteRead { path, source })?;
    Ok(parse_meta_note(&content)?)
}

/// Resolves the scratch destination for a change's rendered tree:
/// the configured scratch directory, or the platform cache directory,
/// namespaced by notebook and change so renders never collide.
fn render_destination(
    configuration: &Configuration,
    notebook_name: &str,
    change_id: &str,
) -> PathBuf {
    let base = configuration.scratch_directory.clone().unwrap_or_else(|| {
        directories::ProjectDirs::from("", "", "nbspec")
            .map(|dirs| dirs.cache_dir().join("renders"))
            .unwrap_or_else(|| PathBuf::from(".auxiliary/temporary/nbspec/renders"))
    });
    base.join(notebook_name).join(change_id)
}

async fn load_metadata(
    client: &NbClient,
    folder: &str,
    notebook: Option<&str>,
) -> Result<ChangeMetadata, OperationError> {
    let show = client
        .show_note(&format!("{folder}/{META_NOTE}.md"), notebook)
        .await?;
    let content = show_note_body(&show)?;
    Ok(parse_meta_note(&content)?)
}

fn schema_for(metadata: &ChangeMetadata) -> Result<WorkflowSchema, OperationError> {
    let configuration = load_configuration(&project_root())?;
    Ok(resolve_schema(Some(&metadata.schema), &configuration)?)
}

fn metadata_summary(metadata: &ChangeMetadata) -> String {
    let title = metadata.title.as_deref().unwrap_or("(untitled)");
    format!(
        "Change: {id}\nTitle: {title}\nStatus: {status}\nSchema: {schema}\nNotebook: {notebook}\nUpdated: {updated}\n",
        id = metadata.change_id,
        status = metadata.status,
        schema = metadata.schema,
        notebook = metadata.notebook,
        updated = metadata.updated_at,
    )
}

async fn folder_exists(client: &NbClient, folder: &str, notebook: Option<&str>) -> bool {
    client
        .list_notes(Some(folder), &[], Some(1), notebook)
        .await
        .is_ok()
}

async fn folder_listing(
    client: &NbClient,
    folder: &str,
    notebook: Option<&str>,
) -> Result<String, String> {
    classify_folder_listing(
        client.list_notes(Some(folder), &[], None, notebook).await,
        folder,
    )
}

/// Maps a `list_notes` result to display text: real listings pass
/// through, genuine absence becomes `(empty)`, and any other failure
/// becomes `Err` for the display unreadable path.
pub fn classify_folder_listing(
    result: Result<String, NbError>,
    folder: &str,
) -> Result<String, String> {
    match result {
        Ok(listing) => {
            let trimmed = listing.trim();
            if trimmed.is_empty() || trimmed.starts_with("0 items") {
                Ok("(empty)".to_string())
            } else {
                Ok(trimmed.to_string())
            }
        }
        Err(NbError::CommandFailed { stderr, .. }) if is_nb_missing_item(&stderr, folder) => {
            Ok("(empty)".to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Maps a `show_note` result to authored-content detection: genuine
/// absence is `Ok(false)`, other failures are `Err` for the display
/// unreadable path.
pub fn classify_note_content(
    result: Result<String, NbError>,
    selector: &str,
) -> Result<bool, String> {
    match result {
        Ok(content) => Ok(note_has_authored_content(&content)),
        Err(NbError::NotFound { .. }) => Ok(false),
        Err(NbError::CommandFailed { stderr, .. }) if is_nb_missing_item(&stderr, selector) => {
            Ok(false)
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Extracts the body bytes of a structured `show_note` result as a
/// lossy UTF-8 string. nb-api 0.3.0 `show_note` returns [`ShowNote`]
/// with the body as a base64 [`nb_api::ByteString`]; the display and
/// metadata paths operate on the decoded body text.
fn show_note_body(show: &ShowNote) -> Result<String, NbError> {
    Ok(String::from_utf8_lossy(&show.body.as_bytes()?).into_owned())
}

/// Extracts the raw source bytes of a structured `show_note` result
/// as a lossy UTF-8 string. Unlike the parsed `body` (which excludes
/// the title heading and tags prefix), `source` is the complete note
/// file, matching what `nb show` printed before nb-api returned
/// structured results.
fn show_note_source(show: &ShowNote) -> Result<String, NbError> {
    Ok(String::from_utf8_lossy(&show.source.as_bytes()?).into_owned())
}

/// Recognizes the pinned `nb` 7.24.0 missing-item diagnostic for a
/// requested note or folder selector.
///
/// After `nb-api` strips ANSI, the diagnostic is a line of the form
/// `!<C0>* Not found: <target>` where `<target>` is the bare selector
/// or `notebook:selector`, and folders may carry a trailing slash.
/// Compound backend output that merely embeds the `Not found:` token
/// against a different selector is not treated as absence.
pub fn is_nb_missing_item(message: &str, selector: &str) -> bool {
    let requested = selector.trim().trim_end_matches('/');
    if requested.is_empty() {
        return false;
    }
    message.lines().any(|line| {
        let Some(target) = nb_not_found_target(line) else {
            return false;
        };
        let target = target.trim_end_matches('/');
        target == requested
            || target
                .rsplit_once(':')
                .is_some_and(|(_, path)| path == requested)
    })
}

/// Extracts the target from a single pinned `! Not found: <target>`
/// line. Prefix may include C0 controls (nb emits SI after SGR reset).
fn nb_not_found_target(line: &str) -> Option<&str> {
    const MARKER: &str = "Not found: ";
    let trimmed = line.trim();
    let index = trimmed.find(MARKER)?;
    let prefix = &trimmed[..index];
    if !prefix.contains('!') {
        return None;
    }
    if !prefix
        .chars()
        .all(|c| c == '!' || c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let target = trimmed[index + MARKER.len()..].trim();
    if target.is_empty() {
        None
    } else {
        Some(target)
    }
}

/// Reads the change's work todo note from the notebook filesystem:
/// the `*.todo.md` file whose checkbox title is [`WORK_NOTE`].
/// Parsing the file directly avoids scraping `nb tasks` output,
/// which embeds terminal control sequences even with `--no-color`.
fn read_work_note(change_directory: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(change_directory).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".todo.md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let open_title = format!("# [ ] {WORK_NOTE}");
        let done_title = format!("# [x] {WORK_NOTE}");
        if content
            .lines()
            .any(|line| line == open_title || line == done_title)
        {
            return Some(content);
        }
    }
    None
}

fn render_work_checklist(checklist: &WorkChecklist) -> String {
    let (complete, total) = checklist.progress();
    if total == 0 {
        return "no task items yet\n".to_string();
    }
    let mut output = format!("{complete}/{total} items complete\n");
    for item in &checklist.items {
        let marker = if item.complete { "x" } else { " " };
        output.push_str(&format!("- [{marker}] {}\n", item.text));
    }
    output
}

/// Reports whether an artifact has authored content: notes must have
/// body content beyond their placeholder, folders must contain notes.
async fn artifact_has_content(
    client: &NbClient,
    folder: &str,
    schema: &WorkflowSchema,
    artifact_id: &str,
    notebook: Option<&str>,
) -> Result<bool, String> {
    use crate::changes::{ArtifactLayout, artifact_layout};
    let Some(artifact) = schema.artifact(artifact_id) else {
        return Ok(false);
    };
    match artifact_layout(artifact) {
        ArtifactLayout::Note(note) => {
            let selector = format!("{folder}/{note}.md");
            let content = client
                .show_note(&selector, notebook)
                .await
                .and_then(|show| show_note_body(&show));
            classify_note_content(content, &selector)
        }
        ArtifactLayout::Folder(subfolder) => {
            let listing =
                folder_listing(client, &format!("{folder}/{subfolder}"), notebook).await?;
            Ok(listing != "(empty)" && !listing.starts_with("0 "))
        }
    }
}
