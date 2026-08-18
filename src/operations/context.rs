use std::path::PathBuf;

use crate::changes::{META_NOTE, change_folder, parse_meta_note, validate_change_id};
use crate::configuration::{Configuration, load_configuration};
use crate::rendering::render_documents;
use crate::schemata::{WorkflowSchema, resolve_schema};

use super::OperationError;

/// Resolved context shared by operations that read a change from the
/// notebook filesystem (render, merge, validate): the effective
/// notebook and project roots, loaded configuration, resolved
/// schema, and the change's rendered document list.
pub(crate) struct ChangeContext {
    pub(crate) notebook_name: String,
    pub(crate) root: PathBuf,
    pub(crate) configuration: Configuration,
    pub(crate) folder: String,
    pub(crate) change_directory: PathBuf,
    pub(crate) schema: WorkflowSchema,
    pub(crate) documents: Vec<crate::rendering::RenderedDocument>,
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
pub(crate) async fn load_change_context(
    client: &nb_api::NbClient,
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
pub(crate) fn resolve_notebook_name(notebook: Option<&str>) -> Result<String, OperationError> {
    notebook
        .map(String::from)
        .or_else(nb_api::derive_git_notebook_name)
        .ok_or(OperationError::NotebookUnresolved)
}

/// Resolves the project repository root, falling back to the current
/// directory outside a Git repository.
pub(crate) fn project_root() -> PathBuf {
    nb_api::git_rev_parse(&["--show-toplevel"]).unwrap_or_else(|| PathBuf::from("."))
}

/// Reads and parses a change's meta note from the notebook
/// filesystem.
pub(crate) fn read_metadata(
    change_directory: &std::path::Path,
) -> Result<crate::changes::ChangeMetadata, OperationError> {
    let path = change_directory.join(format!("{META_NOTE}.md"));
    let content = std::fs::read_to_string(&path)
        .map_err(|source| OperationError::NoteRead { path, source })?;
    Ok(parse_meta_note(&content)?)
}

/// Resolves the scratch destination for a change's rendered tree:
/// the configured scratch directory, or the platform cache directory,
/// namespaced by notebook and change so renders never collide.
pub(crate) fn render_destination(
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
