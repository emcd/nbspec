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

use crate::archives::{ArchiveEntry, ArchiveError, build_archive, gitattributes_covers_lfs};
use crate::changes::{
    ChangeError, ChangeMetadata, META_NOTE, PROPOSALS_FOLDER, WORK_NOTE, change_folder,
    namespace_folders, namespace_notes, parse_meta_note, render_meta_note, validate_change_id,
};
use crate::configuration::{Configuration, ConfigurationError, load_configuration};
use crate::merging::{MergeError, merge_documents, target_status};
use crate::rendering::{RenderError, render_documents, review_diff, write_tree};
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

    #[error("cannot write archive {path}: {source}")]
    ArchiveWrite {
        path: PathBuf,
        source: std::io::Error,
    },
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

    output.push_str("\n## drift\n\n");
    output.push_str(&drift_report(
        &change_directory,
        &folder,
        &schema,
        change_id,
    )?);
    Ok(output)
}

/// Reports the merge-target status of every durable document for
/// `display`.
fn drift_report(
    change_directory: &std::path::Path,
    folder: &str,
    schema: &WorkflowSchema,
    change_id: &str,
) -> Result<String, OperationError> {
    let root = project_root();
    let documents = render_documents(change_directory, folder, schema)?;
    let mut output = String::new();
    for document in &documents {
        let Some(target_path) = &document.target_path else {
            continue;
        };
        let status = target_status(document, &root, change_id)?;
        output.push_str(&format!("- {}: {status}\n", target_path.display()));
    }
    if output.is_empty() {
        output.push_str("no durable documents with merge targets yet\n");
    }
    Ok(output)
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
        return Ok(review_diff(&context.documents, &context.root)?);
    }
    Ok(format!(
        "Rendered {count} documents of change {change_id} to {destination}.",
        count = context.documents.len(),
        destination = destination.display(),
    ))
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
    let report = merge_documents(
        &context.documents,
        &context.root,
        change_id,
        &context.notebook_name,
        force,
    )?;

    let mut output = String::new();
    for path in &report.written {
        output.push_str(&format!("wrote {}\n", path.display()));
    }
    for path in &report.unchanged {
        output.push_str(&format!("unchanged {}\n", path.display()));
    }
    if report.written.is_empty() && report.unchanged.is_empty() {
        output.push_str("no durable documents to merge\n");
    }
    if context.configuration.archives {
        output.push_str(&write_change_archive(
            &context.configuration,
            &context.root,
            &context.change_directory,
            change_id,
            &context.documents,
        )?);
    }
    output.push_str(&format!(
        "Merged change {change_id}: {written} written, {unchanged} unchanged.",
        written = report.written.len(),
        unchanged = report.unchanged.len(),
    ));
    Ok(output)
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
            path: prefix.join(&document.tree_path),
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

/// Validates a change against the OpenSpec grammar.
///
/// # Errors
///
/// Returns [`OperationError::Unimplemented`] until tasks 4.1 through 4.3 land.
pub async fn validate(_client: &NbClient, _change_id: &str) -> OperationResult {
    Err(OperationError::Unimplemented("validate"))
}

/// Resolved context shared by operations that read a change from the
/// notebook filesystem (render, merge): the effective notebook and
/// project roots, loaded configuration, and the change's rendered
/// document list.
struct ChangeContext {
    notebook_name: String,
    root: PathBuf,
    configuration: Configuration,
    change_directory: PathBuf,
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
        .notebook_path(Some(notebook_name.as_str()))
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
        change_directory,
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
