use nb_api::NbClient;
use serde_json::json;

use crate::changes::{
    ChangeMetadata, META_NOTE, PROPOSALS_FOLDER, WORK_NOTE, change_folder, namespace_folders,
    namespace_notes, render_meta_note, validate_change_id,
};
use crate::configuration::load_configuration;
use crate::schemata::resolve_schema;

use super::context::{project_root, resolve_notebook_name};
use super::helpers::folder_exists;
use super::{META_TAG, OperationError, OperationOutcome, OperationResult};

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
