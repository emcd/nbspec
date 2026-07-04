//! Core change operations shared by the CLI and future MCP surface.
//!
//! Each public function corresponds to one user-facing verb. All
//! notebook access flows through [`nb_api::NbClient`]; only `merge`
//! may write to the repository working tree.

use nb_api::NbClient;
use thiserror::Error;

/// Errors from nbspec core operations.
#[derive(Debug, Error)]
pub enum OperationError {
    #[error("operation not implemented yet: {0}")]
    Unimplemented(&'static str),

    #[error("nb invocation failed: {0}")]
    Nb(#[from] nb_api::NbError),
}

/// Result alias for core operations.
pub type OperationResult = Result<String, OperationError>;

/// Creates a change namespace in the project notebook.
///
/// # Errors
///
/// Returns [`OperationError::Unimplemented`] until task 2.5 lands.
pub async fn change_new(
    _client: &NbClient,
    _change_id: &str,
    _title: Option<&str>,
) -> OperationResult {
    Err(OperationError::Unimplemented("change new"))
}

/// Shows a change's notes.
///
/// # Errors
///
/// Returns [`OperationError::Unimplemented`] until task 2.6 lands.
pub async fn change_show(_client: &NbClient, _change_id: &str) -> OperationResult {
    Err(OperationError::Unimplemented("change show"))
}

/// Reports a change's artifact, todo, and drift state.
///
/// # Errors
///
/// Returns [`OperationError::Unimplemented`] until task 2.6 lands.
pub async fn change_status(_client: &NbClient, _change_id: &str) -> OperationResult {
    Err(OperationError::Unimplemented("change status"))
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
