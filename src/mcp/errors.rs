//! Operation result → MCP tool result mapping.
//!
//! Every successful operation produces an [`OperationOutcome`] with a
//! text block (what the CLI prints) and a structured payload (typed
//! data the MCP client can branch on). The MCP tool result carries
//! both — text in `content`, structured in `structured_content`. The
//! spec pins this contract for every tool, not only `validate`.
//!
//! Validation failures remain operation errors carrying
//! [`ValidationFailure`] (whose display is the established
//! `note:line: [artifact] message` text); the wire-format
//! `diagnostics` array is serialized directly from each
//! [`Diagnostic`] so struct-field drift is structurally impossible.
//!
//! [`Diagnostic`]: crate::validation::Diagnostic
//! [`OperationOutcome`]: crate::operations::OperationOutcome

use rmcp::model::{CallToolResult, Content};
use serde_json::{Map, Value};

use crate::operations::{OperationError, OperationOutcome};
use crate::validation::ValidationFailure;

/// Maps a non-validation operation outcome to a tool result with text
/// plus the structured payload the operation produced. `validate`
/// uses [`validation_result`] instead because its failure mode carries
/// structured diagnostics.
pub fn operation_result(
    output: Result<OperationOutcome, OperationError>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    match output {
        Ok(outcome) => Ok(outcome_to_result(outcome, false)),
        Err(error) => Ok(tool_error(error.to_string())),
    }
}

/// Maps a `validate` operation outcome, returning text + structured
/// data on both success and failure paths. Success carries the typed
/// `valid: true` payload (built alongside the text by the
/// operations library, not reconstructed by parsing CLI prose).
/// Failure carries `valid: false` plus the diagnostics array
/// serialized verbatim from [`Diagnostic`].
pub fn validation_result(
    output: Result<OperationOutcome, OperationError>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    match output {
        Ok(outcome) => Ok(outcome_to_result(outcome, false)),
        Err(OperationError::Invalid(failure)) => Ok(validation_failure_result(failure)),
        Err(error) => Ok(tool_error(error.to_string())),
    }
}

/// Builds a tool result from an [`OperationOutcome`]: text in
/// `content`, structured payload in `structured_content`. The
/// `is_error` parameter lets the caller mark the result as a
/// tool-level error (used by `validation_failure_result`).
fn outcome_to_result(outcome: OperationOutcome, is_error: bool) -> CallToolResult {
    text_and_structured(outcome.text, ensure_object(outcome.structured), is_error)
}

/// Wraps a validation failure as a tool-level result carrying both
/// the conventional failure text and a structured payload with the
/// diagnostics vector serialized directly from each [`Diagnostic`].
fn validation_failure_result(failure: ValidationFailure) -> CallToolResult {
    let diagnostics: Vec<Value> = failure
        .diagnostics
        .iter()
        .map(serde_json::to_value)
        .collect::<serde_json::Result<Vec<_>>>()
        .expect("Diagnostic derives Serialize; to_value cannot fail");
    let mut structured = Map::new();
    structured.insert("valid".to_string(), Value::Bool(false));
    structured.insert(
        "change_id".to_string(),
        Value::String(failure.change_id.clone()),
    );
    structured.insert("diagnostics".to_string(), Value::Array(diagnostics));
    text_and_structured(failure.to_string(), structured, true)
}

fn text_and_structured(
    text: String,
    structured: Map<String, Value>,
    is_error: bool,
) -> CallToolResult {
    // `CallToolResult` is `#[non_exhaustive]`, so it must be
    // constructed via a helper and then mutated to attach the second
    // channel. The success helper seeds `is_error = Some(false)`;
    // we override when the result represents a validation failure.
    let mut result = CallToolResult::success(vec![Content::text(text)]);
    result.structured_content = Some(Value::Object(structured));
    if is_error {
        result.is_error = Some(true);
    }
    result
}

fn tool_error(message: String) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message)])
}

/// Coerces a structured payload to an object. Operations are
/// expected to produce JSON objects, but a value that is not one
/// (a scalar, an array, etc.) is wrapped in `{"value": ...}` so
/// the MCP wire format always sees an object. This is defensive:
/// the operations library builds objects directly today.
fn ensure_object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("value".to_string(), other);
            map
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::Diagnostic;
    use serde_json::json;

    #[test]
    fn outcome_with_object_payload_carries_structured_content() {
        let outcome = OperationOutcome::new(
            "created change add-foo",
            json!({
                "change_id": "add-foo",
                "schema": "nbspec-default",
                "folder": "proposals/add-foo",
                "notebook": "nbspec",
            }),
        );
        let result = outcome_to_result(outcome, false);
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.expect("structured payload");
        assert_eq!(structured["change_id"], json!("add-foo"));
        assert_eq!(structured["schema"], json!("nbspec-default"));
    }

    #[test]
    fn outcome_with_non_object_payload_is_wrapped() {
        let outcome = OperationOutcome::new("count: 3", json!(3));
        let result = outcome_to_result(outcome, false);
        let structured = result.structured_content.expect("structured payload");
        assert_eq!(structured, json!({ "value": 3 }));
    }

    #[test]
    fn validation_failure_carries_diagnostics_serialized_from_diagnostic() {
        let failure = ValidationFailure {
            change_id: "add-foo".to_string(),
            diagnostics: vec![
                Diagnostic {
                    note: "proposals/add-foo/proposal.md".to_string(),
                    artifact_id: "proposal".to_string(),
                    line: Some(3),
                    message: "missing H1".to_string(),
                },
                Diagnostic {
                    note: "proposals/add-foo/specifications/x.md".to_string(),
                    artifact_id: "specifications".to_string(),
                    line: None,
                    message: "required artifact has no authored content".to_string(),
                },
            ],
        };
        let result = validation_failure_result(failure);
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured payload");
        assert_eq!(structured.get("valid"), Some(&json!(false)));
        assert_eq!(structured.get("change_id"), Some(&json!("add-foo")));
        let diagnostics = structured
            .get("diagnostics")
            .and_then(|v| v.as_array())
            .expect("diagnostics array");
        assert_eq!(diagnostics.len(), 2);
        // The serialized form is exactly what `serde_json::to_value`
        // would produce from the Diagnostic struct, so any drift
        // between the wire format and the underlying struct is
        // structurally impossible.
        assert_eq!(
            diagnostics[0],
            json!({
                "note": "proposals/add-foo/proposal.md",
                "artifact_id": "proposal",
                "line": 3,
                "message": "missing H1",
            })
        );
        assert_eq!(
            diagnostics[1],
            json!({
                "note": "proposals/add-foo/specifications/x.md",
                "artifact_id": "specifications",
                "line": null,
                "message": "required artifact has no authored content",
            })
        );
    }

    #[test]
    fn validation_failure_text_matches_failure_display() {
        let failure = ValidationFailure {
            change_id: "add-foo".to_string(),
            diagnostics: vec![Diagnostic {
                note: "proposals/add-foo/proposal.md".to_string(),
                artifact_id: "proposal".to_string(),
                line: Some(3),
                message: "missing H1".to_string(),
            }],
        };
        let result = validation_failure_result(failure);
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .expect("text content");
        assert!(text.starts_with("change add-foo is invalid: 1 violation"));
        assert!(text.contains("proposals/add-foo/proposal.md:3: [proposal] missing H1"));
    }
}
