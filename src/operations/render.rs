use nb_api::NbClient;
use serde_json::json;

use crate::rendering::{review_diff, write_tree};

use super::context::{load_change_context, render_destination};
use super::{OperationOutcome, OperationResult};

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
