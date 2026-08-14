//! Unit tests for operation-level review refusals and nb absence
//! classification.
//!
//! Review refusal paths fire before any notebook or filesystem access,
//! so a plain [`NbClient`] (pure configuration) suffices. Happy-path
//! recording is covered by the lifecycle integration test against a
//! real scratch notebook. Absence classification is pure string
//! matching against the pinned nb 7.24.0 diagnostic shape.

use nb_api::{Config, NbClient, NbError};
use nbspec::operations::{
    self, OperationError, classify_folder_listing, classify_note_content, is_nb_missing_item,
};
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

#[test]
fn missing_item_matches_pinned_nb_diagnostic_with_si_control() {
    // nb 7.24.0 emits SI (\u{0f}) after SGR reset; nb-api strips ANSI
    // but leaves the C0 control. The classifier must still bind.
    let msg = "!\u{0f} Not found: scratch:proposals/add-demo/proposal.md\n";
    assert!(is_nb_missing_item(msg, "proposals/add-demo/proposal.md"));
    assert!(is_nb_missing_item(
        msg,
        "scratch:proposals/add-demo/proposal.md"
    ));
}

#[test]
fn missing_item_matches_folder_with_trailing_slash() {
    let msg = "!\u{0f} Not found: scratch:proposals/add-demo/specifications/\n";
    assert!(is_nb_missing_item(msg, "proposals/add-demo/specifications"));
    assert!(is_nb_missing_item(
        msg,
        "proposals/add-demo/specifications/"
    ));
}

#[test]
fn missing_item_rejects_wrong_selector_and_compound_prose() {
    let other = "!\u{0f} Not found: scratch:other.md\n";
    assert!(!is_nb_missing_item(other, "proposals/add-demo/proposal.md"));

    let compound = "backend exploded while handling ! Not found: proposals/add-demo/proposal.md\n";
    assert!(
        !is_nb_missing_item(compound, "proposals/add-demo/proposal.md"),
        "prose embedding the token must not classify as absence"
    );

    let search = "! Not found in scratch: some query\n";
    assert!(!is_nb_missing_item(search, "some query"));
}

#[test]
fn missing_item_matches_bare_and_qualified_selectors() {
    let inner = "! Not found: home:proposals/x/proposal.md\n";
    assert!(is_nb_missing_item(inner, "proposals/x/proposal.md"));
    assert!(is_nb_missing_item(inner, "home:proposals/x/proposal.md"));
}

#[test]
fn classify_note_content_maps_absence_and_failures() {
    let selector = "proposals/x/proposal.md";
    assert!(
        !classify_note_content(Ok("# proposal\n\n<!-- scaffold -->\n".into()), selector).unwrap()
    );
    assert!(classify_note_content(Ok("# proposal\n\nBody.\n".into()), selector).unwrap());
    assert!(
        !classify_note_content(
            Err(NbError::CommandFailed {
                command: "nb show".into(),
                stderr: "!\u{0f} Not found: home:proposals/x/proposal.md\n".into(),
                exit_code: Some(1),
            }),
            selector
        )
        .unwrap()
    );
    let err = classify_note_content(
        Err(NbError::CommandFailed {
            command: "nb show".into(),
            stderr: "permission denied".into(),
            exit_code: Some(1),
        }),
        selector,
    )
    .unwrap_err();
    assert!(err.contains("permission denied"), "{err}");
}

#[test]
fn classify_folder_listing_maps_absence_and_failures() {
    let folder = "proposals/x/specifications";
    assert_eq!(
        classify_folder_listing(Ok("0 items.\n".into()), folder).unwrap(),
        "(empty)"
    );
    assert_eq!(
        classify_folder_listing(
            Err(NbError::CommandFailed {
                command: "nb ls".into(),
                stderr: "!\u{0f} Not found: home:proposals/x/specifications/\n".into(),
                exit_code: Some(1),
            }),
            folder
        )
        .unwrap(),
        "(empty)"
    );
    let err = classify_folder_listing(
        Err(NbError::CommandFailed {
            command: "nb ls".into(),
            stderr: "backend exploded".into(),
            exit_code: Some(1),
        }),
        folder,
    )
    .unwrap_err();
    assert!(err.contains("backend exploded"), "{err}");
    let listing = classify_folder_listing(Ok("[home:1] alpha.md\n".into()), folder).unwrap();
    assert!(listing.contains("alpha.md"), "{listing}");
}
