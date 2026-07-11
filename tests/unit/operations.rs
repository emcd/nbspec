//! Unit tests for operation-level review refusals.
//!
//! These cover the refusal paths that fire before any notebook or
//! filesystem access, so a plain [`NbClient`] (pure configuration)
//! suffices. Happy-path recording is covered by the lifecycle
//! integration test against a real scratch notebook.

use nb_api::{Config, NbClient};
use nbspec::operations::{self, OperationError};
use nbspec::reviews::VerdictValue;

fn client() -> NbClient {
    NbClient::new(&Config::default()).expect("client construction is pure configuration")
}

#[tokio::test]
async fn review_refuses_unknown_gate() {
    let error = operations::review(
        &client(),
        Some("nbspec-test"),
        "add-demo",
        "publish",
        VerdictValue::Approve,
        Some("advisor"),
        None,
    )
    .await
    .expect_err("unknown gate must refuse");
    match error {
        OperationError::GateUnknown { gate, known } => {
            assert_eq!(gate, "publish");
            assert!(known.contains("merge"));
        }
        other => panic!("expected GateUnknown, got {other}"),
    }
}

#[tokio::test]
async fn review_refuses_explicit_empty_reviewer() {
    // Explicit is never absence: an empty --reviewer refuses rather
    // than falling through to Git identity.
    let error = operations::review(
        &client(),
        Some("nbspec-test"),
        "add-demo",
        "merge",
        VerdictValue::Approve,
        Some("   "),
        None,
    )
    .await
    .expect_err("explicit empty reviewer must refuse");
    assert!(matches!(error, OperationError::ReviewerUnresolved));
}

#[tokio::test]
async fn review_refuses_comment_less_revise() {
    let error = operations::review(
        &client(),
        Some("nbspec-test"),
        "add-demo",
        "merge",
        VerdictValue::Revise,
        Some("advisor"),
        None,
    )
    .await
    .expect_err("revise without a comment must refuse");
    assert!(matches!(error, OperationError::ReviseCommentMissing));
}

#[tokio::test]
async fn review_refuses_whitespace_only_revise_comment() {
    let error = operations::review(
        &client(),
        Some("nbspec-test"),
        "add-demo",
        "merge",
        VerdictValue::Revise,
        Some("advisor"),
        Some("  \n "),
    )
    .await
    .expect_err("whitespace-only comment must refuse");
    assert!(matches!(error, OperationError::ReviseCommentMissing));
}
