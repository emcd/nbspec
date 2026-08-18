use nb_api::NbClient;
use serde_json::json;

use crate::validation::{ValidationFailure, validate_change};

use super::context::load_change_context;
use super::{OperationOutcome, OperationResult};

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
        return Err(crate::operations::OperationError::Invalid(
            ValidationFailure {
                change_id: change_id.to_string(),
                diagnostics,
            },
        ));
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
