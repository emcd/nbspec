//! Unit tests for the MCP module.
//!
//! Exercises argument validation (`deny_unknown_fields` rejection),
//! notebook-resolution flow (explicit configuration vs. git-derived
//! fallback), and structured-return shape (text + diagnostics
//! array). The latter is co-located with the helpers in
//! [`crate::mcp::errors`] because it owns the assertion logic; this
//! file covers everything else.
//!
//! Notebook-resolution tests run against the current working
//! directory, which during `cargo test` is the crate root (a git
//! repository whose basename is `nbspec`). That is sufficient to
//! exercise the git-derived fallback without changing the process
//! working directory (which would race with other test modules).
//! The end-to-end `McpServer::new` flow — including the nb
//! existence check — is covered by the integration tests in
//! `tests/integration/mcp_lifecycle.rs`, which use scratch
//! notebooks that are registered with `nb` for the test run.

use nbspec::mcp::params::{
    CreateArgs, DisplayArgs, MergeArgs, RenderArgs, ReviewArgs, ValidateArgs,
};
use nbspec::mcp::server::{NotebookSource, resolve_notebook};
use rmcp::handler::server::wrapper::Parameters;
use schemars::schema_for;
use serde_json::json;

#[test]
fn create_args_round_trip_minimum_fields() {
    let args: CreateArgs = serde_json::from_value(json!({"change_id": "add-foo"})).unwrap();
    assert_eq!(args.change_id, "add-foo");
    assert_eq!(args.title, None);
}

#[test]
fn create_args_rejects_unknown_field() {
    let result = serde_json::from_value::<CreateArgs>(json!({
        "change_id": "add-foo",
        "title": "Add Foo",
        "schemars_field_that_does_not_exist": true,
    }));
    let error = result.expect_err("deny_unknown_fields must reject unknown field");
    assert!(
        error.to_string().contains("unknown field"),
        "unexpected error message: {error}"
    );
}

#[test]
fn display_args_default_full_is_false() {
    let args: DisplayArgs = serde_json::from_value(json!({"change_id": "add-foo"})).unwrap();
    assert!(!args.full);
}

#[test]
fn validate_args_rejects_unknown_field() {
    let result = serde_json::from_value::<ValidateArgs>(json!({
        "change_id": "add-foo",
        "notebook": "should-be-rejected",
    }));
    assert!(
        result.is_err(),
        "notebook per-tool override must be rejected"
    );
}

#[test]
fn render_args_default_diff_is_false() {
    let args: RenderArgs = serde_json::from_value(json!({"change_id": "add-foo"})).unwrap();
    assert!(!args.diff);
}

#[test]
fn merge_args_default_force_is_false() {
    let args: MergeArgs = serde_json::from_value(json!({"change_id": "add-foo"})).unwrap();
    assert!(!args.force);
}

#[test]
fn tool_schemas_expose_expected_fields_only() {
    // A change_id is required on every tool; notebook overrides are
    // absent by design (server-lifetime resolution).
    let schemas = [
        ("create", schema_for!(CreateArgs)),
        ("display", schema_for!(DisplayArgs)),
        ("validate", schema_for!(ValidateArgs)),
        ("render", schema_for!(RenderArgs)),
        ("merge", schema_for!(MergeArgs)),
        ("review", schema_for!(ReviewArgs)),
    ];
    for (name, schema) in schemas {
        let value = serde_json::to_value(&schema).unwrap();
        let properties = value
            .get("properties")
            .and_then(|p| p.as_object())
            .unwrap_or_else(|| panic!("{name}: schema missing properties"));
        assert!(
            properties.contains_key("change_id"),
            "{name}: schema must expose change_id"
        );
        assert!(
            !properties.contains_key("notebook"),
            "{name}: schema must NOT expose notebook (server-lifetime resolution)"
        );
    }
}

#[test]
fn parameters_wrapper_accepts_typed_args() {
    // Defensive: Parameters<T> is what every tool handler destructures;
    // a regression that breaks the wrapper would surface here.
    let params = Parameters(CreateArgs {
        change_id: "add-foo".to_string(),
        title: Some("Add Foo".to_string()),
    });
    let Parameters(args) = params;
    assert_eq!(args.change_id, "add-foo");
}

#[test]
fn resolve_notebook_prefers_explicit_when_non_empty() {
    let (notebook, source) =
        resolve_notebook(&Some("explicit-name".to_string())).expect("resolution must succeed");
    assert_eq!(notebook, "explicit-name");
    assert_eq!(source, NotebookSource::Explicit);
}

#[test]
fn resolve_notebook_uses_git_derived_when_explicit_is_none() {
    // The test runs from the crate root, a git repository. The
    // expected name is derived via the same helper the resolver
    // calls (`nb_api::derive_git_notebook_name`), so the assertion
    // holds for any checkout or worktree whose primary repository
    // directory name follows the same derivation rule.
    let expected =
        nb_api::derive_git_notebook_name().expect("test must run from within a git repository");
    let (notebook, source) =
        resolve_notebook(&None).expect("resolution must succeed in a git repo");
    assert_eq!(
        notebook, expected,
        "git-derived name must equal nb_api's derivation"
    );
    assert_eq!(source, NotebookSource::GitDerived);
}

#[test]
fn resolve_notebook_treats_empty_as_explicit_not_as_absence() {
    // An explicit `Some("")` is preserved as the empty string and
    // labeled Explicit. The startup validation step will then
    // surface it as a missing notebook rather than silently
    // falling through to git derivation. This matches the CLI's
    // behavior (`nbspec --notebook ""` fails the same way), so
    // the MCP surface does not diverge from the CLI on operator-
    // supplied values.
    let (notebook, source) = resolve_notebook(&Some(String::new()))
        .expect("resolution preserves the explicit empty value");
    assert_eq!(notebook, "");
    assert_eq!(source, NotebookSource::Explicit);
}

#[test]
fn review_args_gate_defaults_to_merge() {
    let args: ReviewArgs =
        serde_json::from_value(json!({"change_id": "add-foo", "verdict": "approve"})).unwrap();
    assert_eq!(args.gate, "merge");
    assert_eq!(args.comment, None);
    assert_eq!(args.reviewer, None);
}

#[test]
fn review_args_comment_dash_is_literal() {
    // Stdin reading is a CLI-only affordance: the MCP surface records
    // a dash verbatim rather than reinterpreting the payload.
    let args: ReviewArgs = serde_json::from_value(json!({
        "change_id": "add-foo",
        "verdict": "revise",
        "comment": "-",
    }))
    .unwrap();
    assert_eq!(args.comment.as_deref(), Some("-"));
}

#[test]
fn review_args_rejects_notebook_override() {
    let result = serde_json::from_value::<ReviewArgs>(json!({
        "change_id": "add-foo",
        "verdict": "approve",
        "notebook": "should-be-rejected",
    }));
    assert!(
        result.is_err(),
        "notebook per-tool override must be rejected"
    );
}

#[test]
fn review_args_rejects_unknown_verdict_value() {
    let result = serde_json::from_value::<ReviewArgs>(json!({
        "change_id": "add-foo",
        "verdict": "acclaim",
    }));
    assert!(result.is_err(), "verdict values are approve|revise only");
}
