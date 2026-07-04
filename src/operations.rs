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

use std::path::PathBuf;

use nb_api::NbClient;
use thiserror::Error;

use crate::changes::{
    ChangeError, ChangeMetadata, META_NOTE, PROPOSALS_FOLDER, WORK_NOTE, change_folder,
    namespace_folders, namespace_notes, parse_meta_note, render_meta_note, validate_change_id,
};
use crate::configuration::{ConfigurationError, load_configuration};
use crate::schemata::{SchemaError, WorkflowSchema, resolve_schema};
use crate::worknotes::{WorkChecklist, WorkNoteError, parse_work_note};

/// Tag applied to nbspec-managed control-plane notes.
const META_TAG: &str = "nbspec";

/// Errors from nbspec core operations.
#[derive(Debug, Error)]
pub enum OperationError {
    #[error("operation not implemented yet: {0}")]
    Unimplemented(&'static str),

    #[error("change already exists: {0}")]
    AlreadyExists(String),

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
}

/// Result alias for core operations.
pub type OperationResult = Result<String, OperationError>;

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

    ensure_folder(client, PROPOSALS_FOLDER, notebook).await?;
    if folder_exists(client, &folder, notebook).await {
        return Err(OperationError::AlreadyExists(change_id.to_string()));
    }
    client.mkdir(&folder, notebook).await?;
    for subfolder in namespace_folders(&schema) {
        client
            .mkdir(&format!("{folder}/{subfolder}"), notebook)
            .await?;
    }

    let metadata = ChangeMetadata::new(change_id, title, &schema.name, &notebook_name)?;
    client
        .add(
            Some(META_NOTE),
            &render_meta_note(&metadata)?,
            &[META_TAG.to_string()],
            Some(&folder),
            notebook,
        )
        .await?;
    for note in namespace_notes(&schema) {
        let placeholder = format!("<!-- Draft the {note} here. -->\n");
        client
            .add(Some(&note), &placeholder, &[], Some(&folder), notebook)
            .await?;
    }
    client
        .todo(
            WORK_NOTE,
            Some(&format!("Execution checklist for {change_id}.")),
            &[],
            &[META_TAG.to_string()],
            Some(&folder),
            notebook,
        )
        .await?;

    Ok(format!(
        "Created change {change_id} (schema {schema_name}) under {folder}/ in notebook {notebook_name}.",
        schema_name = schema.name,
    ))
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
            let content = client.show(&format!("{folder}/{note}"), notebook).await?;
            output.push_str(&format!("\n## {note}\n\n{}\n", content.trim_end()));
        }
    }
    output.push_str("\n## artifacts\n\n");
    let mut authored: Vec<&str> = Vec::new();
    for artifact in schema.authoring_order() {
        let has_content =
            artifact_has_content(client, &folder, &schema, &artifact.id, notebook).await;
        let unmet: Vec<&str> = artifact
            .requires
            .iter()
            .map(String::as_str)
            .filter(|dependency| !authored.contains(dependency))
            .collect();
        let state = if has_content {
            authored.push(artifact.id.as_str());
            "authored".to_string()
        } else if unmet.is_empty() {
            "ready to author".to_string()
        } else {
            format!("blocked on {}", unmet.join(", "))
        };
        output.push_str(&format!("- {}: {state}\n", artifact.id));
    }
    if full {
        for subfolder in namespace_folders(&schema) {
            let listing = folder_listing(client, &format!("{folder}/{subfolder}"), notebook).await;
            output.push_str(&format!("\n## {subfolder}/\n\n{listing}\n"));
        }
    }

    output.push_str("\n## work\n\n");
    let change_directory = client
        .notebook_path(notebook)
        .await?
        .join(change_folder(change_id));
    output.push_str(&work_report(&change_directory));

    output
        .push_str("\n## drift\n\nnot yet tracked (merge drift detection arrives with task 3.5)\n");
    Ok(output)
}

/// Renders a change to a scratch workspace for review.
///
/// # Errors
///
/// Returns [`OperationError::Unimplemented`] until tasks 3.1 and 3.2 land.
pub async fn render(_client: &NbClient, _change_id: &str, _diff: bool) -> OperationResult {
    Err(OperationError::Unimplemented("render"))
}

/// Transfers a change's durable artifacts into the repository.
///
/// # Errors
///
/// Returns [`OperationError::Unimplemented`] until tasks 3.4 through 3.6 land.
pub async fn merge(_client: &NbClient, _change_id: &str, _force: bool) -> OperationResult {
    Err(OperationError::Unimplemented("merge"))
}

/// Validates a change against the OpenSpec grammar.
///
/// # Errors
///
/// Returns [`OperationError::Unimplemented`] until tasks 4.1 through 4.3 land.
pub async fn validate(_client: &NbClient, _change_id: &str) -> OperationResult {
    Err(OperationError::Unimplemented("validate"))
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

/// Resolves the project repository root, falling back to the current
/// directory outside a Git repository.
fn project_root() -> PathBuf {
    nb_api::git_rev_parse(&["--show-toplevel"]).unwrap_or_else(|| PathBuf::from("."))
}

async fn load_metadata(
    client: &NbClient,
    folder: &str,
    notebook: Option<&str>,
) -> Result<ChangeMetadata, OperationError> {
    let content = client
        .show(&format!("{folder}/{META_NOTE}"), notebook)
        .await?;
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
        .list(Some(folder), &[], Some(1), notebook)
        .await
        .is_ok()
}

async fn ensure_folder(
    client: &NbClient,
    folder: &str,
    notebook: Option<&str>,
) -> Result<(), OperationError> {
    if folder_exists(client, folder, notebook).await {
        return Ok(());
    }
    client.mkdir(folder, notebook).await?;
    Ok(())
}

async fn folder_listing(client: &NbClient, folder: &str, notebook: Option<&str>) -> String {
    match client.list(Some(folder), &[], None, notebook).await {
        Ok(listing) => {
            let trimmed = listing.trim();
            // nb reports empty folders as "0 items." followed by help text.
            if trimmed.is_empty() || trimmed.starts_with("0 items") {
                "(empty)".to_string()
            } else {
                trimmed.to_string()
            }
        }
        Err(_) => "(empty)".to_string(),
    }
}

/// Reports the `work` checklist section for `display`: progress
/// counts and the item list, or a loud per-section diagnostic when
/// the note is missing or malformed — never misreported numbers.
fn work_report(change_directory: &std::path::Path) -> String {
    let Some(content) = read_work_note(change_directory) else {
        return "(no work todo note found)\n".to_string();
    };
    match parse_work_note(&content) {
        Ok(checklist) => render_work_checklist(&checklist),
        Err(error) => format!("{error}\n"),
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
) -> bool {
    use crate::changes::{ArtifactLayout, artifact_layout};
    let Some(artifact) = schema.artifact(artifact_id) else {
        return false;
    };
    match artifact_layout(artifact) {
        ArtifactLayout::Note(note) => {
            match client.show(&format!("{folder}/{note}"), notebook).await {
                Ok(content) => note_has_authored_content(&content),
                Err(_) => false,
            }
        }
        ArtifactLayout::Folder(subfolder) => {
            let listing = folder_listing(client, &format!("{folder}/{subfolder}"), notebook).await;
            listing != "(empty)" && !listing.starts_with("0 ")
        }
    }
}

/// Reports whether note content goes beyond its title heading and
/// scaffold placeholder comment.
fn note_has_authored_content(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        let scaffold = trimmed.is_empty()
            || trimmed.starts_with('#')
            || (trimmed.starts_with("<!--") && trimmed.ends_with("-->"));
        !scaffold
    })
}
