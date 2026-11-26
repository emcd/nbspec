//! End-to-end MCP integration test: spawn `nbspec serve mcp` on stdio
//! inside a scratch fixture, drive the six tools (create, display,
//! validate, render, merge, review) over JSON-RPC, and verify text
//! + structured responses match the contracts the specification
//!   pins.
//!
//! Like the CLI lifecycle test, this requires `nb` to be installed.
//! The scratch notebook lives in nb-api's isolated [`NbTestEnv`]
//! `NB_DIR` (see `super::harness`); the fixture root is removed on
//! drop, so the operator's real notebook list never sees scratch
//! notebooks from this test. The `nbspec serve mcp` subprocess
//! inherits the same isolated `NB_DIR` so its internal `nb`
//! invocations see the scratch notebook.
//!
//! All spawned subprocesses — the `nbspec serve mcp` tokio subprocess
//! and its internal `nb` invocations — have `GIT_*` environment
//! variables scrubbed and the fixture's environment applied via
//! `configure_tokio`. Without this, a hook or CI environment that
//! exports `GIT_DIR`/`GIT_INDEX_FILE` redirects every git call inside
//! `nb` away from the notebook's repository and the subprocess exits
//! before responding. Per `nbspec:issues/4`.

use std::path::Path;

use serde_json::{Value, json};

use super::harness::Fixture;
use super::mcp_client::McpHarness;

const CHANGE_ID: &str = "add-mcp-demo";

const SPECIFICATION: &str = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users before granting access.

#### Scenario: Valid login
- **WHEN** a user submits correct credentials
- **THEN** a session begins
";

/// Reads the first text content block from a `CallToolResult`. Tools
/// other than `validate` return success as a text block; `validate`
/// additionally carries structured diagnostics.
fn first_text(result: &Value) -> String {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .expect("result.content array");
    let first = content.first().expect("at least one content block");
    first
        .get("text")
        .and_then(Value::as_str)
        .expect("text content block")
        .to_string()
}

fn result_of(response: &Value) -> &Value {
    response
        .get("result")
        .unwrap_or_else(|| panic!("expected result in response: {response}"))
}

fn assert_success(response: &Value) -> &Value {
    let result = result_of(response);
    assert!(
        result.get("isError").and_then(Value::as_bool) != Some(true),
        "expected success; got error result: {response}"
    );
    result
}

fn assert_tool_error(response: &Value) -> &Value {
    let result = result_of(response);
    assert_eq!(
        result.get("isError").and_then(Value::as_bool),
        Some(true),
        "expected isError=true in result: {response}"
    );
    result
}

#[tokio::test]
async fn mcp_server_drives_change_lifecycle() {
    let fixture = Fixture::new();
    let mut harness = McpHarness::spawn(&fixture).await;

    // List tools: must include exactly the eight verbs.
    let tools_response = harness.list_tools().await;
    let tools = tools_response["result"]["tools"]
        .as_array()
        .expect("result.tools array");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "create", "display", "export", "import", "merge", "render",
            "review", "validate",
        ],
        "tools/list must expose exactly the eight CLI verbs"
    );

    // create: scaffold the change namespace.
    let created = harness
        .call_tool(
            "create",
            json!({"change_id": CHANGE_ID, "title": "Demo"})
                .as_object()
                .cloned()
                .expect("create args object"),
        )
        .await;
    let created_result = assert_success(&created);
    assert!(
        first_text(created_result).contains("Created change add-mcp-demo"),
        "create text: {created_result}"
    );
    // Every tool returns text plus a structured payload. The
    // create payload carries the change_id, schema, folder, and
    // resolved notebook; clients branch on these instead of
    // scraping the success prose.
    let create_structured = created_result
        .get("structuredContent")
        .expect("create structuredContent");
    assert_eq!(create_structured["change_id"], json!(CHANGE_ID));
    assert_eq!(create_structured["schema"], json!("nbspec-default"));
    assert_eq!(create_structured["folder"], json!("proposals/add-mcp-demo"));
    assert_eq!(create_structured["notebook"], json!(fixture.notebook()));

    // display: short form reports status + authored/ready.
    let displayed = harness
        .call_tool(
            "display",
            json!({"change_id": CHANGE_ID})
                .as_object()
                .cloned()
                .expect("display args object"),
        )
        .await;
    let display_result = assert_success(&displayed);
    let display_text = first_text(display_result);
    assert!(display_text.contains("Status: draft"));
    assert!(display_text.contains("- proposal: ready to author"));
    // The display payload is the typed mirror of the text: status,
    // schema, artifacts array (each with state), work progress,
    // and drift list. Agents branch on these instead of regexing
    // the text.
    let display_structured = display_result
        .get("structuredContent")
        .expect("display structuredContent");
    assert_eq!(display_structured["change_id"], json!(CHANGE_ID));
    assert_eq!(display_structured["status"], json!("draft"));
    assert_eq!(display_structured["schema"], json!("nbspec-default"));
    assert_eq!(display_structured["notebook"], json!(fixture.notebook()));
    let artifacts = display_structured["artifacts"]
        .as_array()
        .expect("artifacts array");
    assert!(artifacts.iter().any(|a| a["id"] == "proposal"));
    assert!(artifacts.iter().any(|a| a["id"] == "specifications"));
    assert!(display_structured["work"]["total"].as_u64().is_some());
    assert!(display_structured["drift"].is_array());

    // specifications/designs/decisions all require proposal first,
    // so they appear as `blocked on proposal` until the proposal is
    // authored.
    assert!(display_text.contains("- specifications: blocked on proposal"));
    assert!(display_text.contains("- designs: blocked on proposal"));
    assert!(display_text.contains("- decisions: blocked on proposal"));

    // validate: failure path returns text + structured diagnostics.
    let invalid = harness
        .call_tool(
            "validate",
            json!({"change_id": CHANGE_ID})
                .as_object()
                .cloned()
                .expect("validate args object"),
        )
        .await;
    let invalid_result = assert_tool_error(&invalid);
    let invalid_text = first_text(invalid_result);
    assert!(
        invalid_text.contains("change add-mcp-demo is invalid: 2 violations"),
        "validate text: {invalid_text}"
    );
    let structured = invalid_result
        .get("structuredContent")
        .expect("structuredContent");
    assert_eq!(structured["valid"], json!(false));
    assert_eq!(structured["change_id"], json!(CHANGE_ID));
    let diagnostics = structured["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert_eq!(diagnostics.len(), 2, "diagnostics count");
    for diagnostic in diagnostics {
        assert!(diagnostic["note"].is_string());
        assert!(diagnostic["artifact_id"].is_string());
        assert!(diagnostic["message"].is_string());
        // `line` is null for required-artifact failures.
        assert!(diagnostic["line"].is_null());
    }

    // Author the proposal and specification directly on the notebook
    // filesystem, as an agent's editor would.
    let change_directory =
        fixture.notebook_path().join("proposals").join(CHANGE_ID);
    let mut proposal =
        std::fs::read_to_string(change_directory.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nDrive the MCP lifecycle.\n");
    std::fs::write(change_directory.join("proposal.md"), proposal).unwrap();
    let specification_note =
        change_directory.join("specifications/user-auth.md");
    std::fs::write(&specification_note, SPECIFICATION).unwrap();
    super::lifecycle::commit_notebook(&fixture);

    // validate: success path returns text + structured success payload.
    let valid = harness
        .call_tool(
            "validate",
            json!({"change_id": CHANGE_ID})
                .as_object()
                .cloned()
                .expect("validate args object"),
        )
        .await;
    let valid_result = assert_success(&valid);
    assert!(
        first_text(valid_result).contains(
            "Change add-mcp-demo is valid: 2 documents checked against schema nbspec-default"
        ),
        "validate success text: {valid_result}"
    );
    let valid_structured = valid_result
        .get("structuredContent")
        .expect("validate success structured payload");
    assert_eq!(valid_structured["valid"], json!(true));
    assert_eq!(valid_structured["change_id"], json!(CHANGE_ID));
    assert_eq!(valid_structured["documents_checked"], json!(2));
    assert_eq!(valid_structured["schema"], json!("nbspec-default"));

    // render: writes the scratch tree; repository untouched.
    let rendered = harness
        .call_tool(
            "render",
            json!({"change_id": CHANGE_ID})
                .as_object()
                .cloned()
                .expect("render args object"),
        )
        .await;
    let rendered_result = assert_success(&rendered);
    assert!(
        first_text(rendered_result).contains("Rendered 2 documents"),
        "render text: {rendered_result}"
    );
    // Structured payload: format discriminator plus counts so
    // agents can branch on tree-vs-diff without parsing the prose.
    let render_structured = rendered_result
        .get("structuredContent")
        .expect("render structuredContent");
    assert_eq!(render_structured["change_id"], json!(CHANGE_ID));
    assert_eq!(render_structured["format"], json!("tree"));
    assert_eq!(render_structured["documents_count"], json!(2));
    assert!(render_structured["destination"].is_string());
    let scratch_document = fixture
        .project_root()
        .join(".auxiliary/temporary/renders")
        .join(fixture.notebook())
        .join(CHANGE_ID)
        .join("specifications/user-auth.md");
    assert_eq!(
        std::fs::read_to_string(&scratch_document).unwrap(),
        SPECIFICATION
    );
    assert!(!fixture.project_root().join("documentation").exists());

    // render with diff=true: emits unified diff suitable for difit.
    let diffed = harness
        .call_tool(
            "render",
            json!({"change_id": CHANGE_ID, "diff": true})
                .as_object()
                .cloned()
                .expect("render args object"),
        )
        .await;
    let diffed_result = assert_success(&diffed);
    let diff_text = first_text(diffed_result);
    assert!(
        diff_text.starts_with(
            "diff --git a/documentation/specifications/user-auth.md \
             b/documentation/specifications/user-auth.md"
        ),
        "diff text head: {diff_text}"
    );
    assert!(diff_text.contains("+### Requirement: User authentication"));
    let diff_structured = diffed_result
        .get("structuredContent")
        .expect("render diff structuredContent");
    assert_eq!(diff_structured["format"], json!("diff"));
    assert!(diff_structured["lines"].as_u64().unwrap() > 0);

    // An unreviewed merge refuses at the review gate over the MCP
    // surface exactly as it does on the CLI.
    let unreviewed = harness
        .call_tool(
            "merge",
            json!({"change_id": CHANGE_ID})
                .as_object()
                .cloned()
                .expect("merge args object"),
        )
        .await;
    let refused_result = assert_tool_error(&unreviewed);
    assert!(
        first_text(refused_result)
            .contains("review gate unsatisfied: no verdict"),
        "refusal text: {}",
        first_text(refused_result)
    );

    // A comment-less revise refuses over the tool surface exactly as
    // on the CLI; then record the approving verdict through the
    // review tool itself — surface parity, proven.
    let moodless = harness
        .call_tool(
            "review",
            json!({"change_id": CHANGE_ID, "verdict": "revise", "reviewer": "itest"})
                .as_object()
                .cloned()
                .expect("review args object"),
        )
        .await;
    let moodless_result = assert_tool_error(&moodless);
    assert!(
        first_text(moodless_result).contains("requires a comment"),
        "refusal text: {}",
        first_text(moodless_result)
    );
    let reviewed = harness
        .call_tool(
            "review",
            json!({"change_id": CHANGE_ID, "verdict": "approve", "reviewer": "itest"})
                .as_object()
                .cloned()
                .expect("review args object"),
        )
        .await;
    let reviewed_result = assert_success(&reviewed);
    assert!(
        first_text(reviewed_result)
            .contains("Recorded approve verdict by itest"),
        "review text: {}",
        first_text(reviewed_result)
    );
    let review_structured = reviewed_result
        .get("structuredContent")
        .expect("review structuredContent");
    assert_eq!(review_structured["gate"], json!("merge"));
    assert_eq!(review_structured["verdict"], json!("approve"));
    assert_eq!(review_structured["reviewer"], json!("itest"));
    assert!(
        review_structured["aggregate_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() == 64),
        "aggregate hash must be a SHA-256 hex digest"
    );
    // MCP callers receive the authoritative on-disk note path
    // (text and structured surfaces must agree). The path is
    // `<notebook>:<relative-path>` per `nb`'s qualified output;
    // the basename must equal the verdict id so the structured
    // field is selector-stable, not a synthesized pre-normalization
    // destination.
    let structured_note = review_structured["note"]
        .as_str()
        .expect("structured note must be a string");
    let structured_note_relpath = structured_note
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(structured_note);
    let structured_note_basename =
        std::path::Path::new(structured_note_relpath)
            .file_name()
            .and_then(|name| name.to_str())
            .expect("structured note must have a UTF-8 basename");
    assert!(
        structured_note_basename.ends_with(".md"),
        "structured note filename must end with `.md`: {structured_note_basename:?}"
    );
    let structured_verdict_id = structured_note_basename
        .strip_suffix(".md")
        .expect(".md suffix");
    assert!(
        nbspec::reviews::is_verdict_note_id(structured_verdict_id),
        "structured note basename must be a collision-resistant verdict id, got: {structured_verdict_id:?}"
    );
    // MCP text and structured surfaces must agree on the path.
    let review_text = first_text(reviewed_result);
    let text_note_line = review_text
        .lines()
        .find(|line| line.starts_with("note="))
        .expect("review text must contain `note=...` line");
    assert_eq!(
        text_note_line,
        &format!("note={structured_note}"),
        "MCP text and structured surfaces must agree on the note path"
    );

    // merge: transfers durable documents with provenance + archive.
    let merged = harness
        .call_tool(
            "merge",
            json!({"change_id": CHANGE_ID})
                .as_object()
                .cloned()
                .expect("merge args object"),
        )
        .await;
    let merged_result = assert_success(&merged);
    let merged_text = first_text(merged_result);
    assert!(
        merged_text
            .contains("wrote documentation/specifications/user-auth.md")
    );
    assert!(
        merged_text
            .contains("archived documentation/archives/add-mcp-demo.tar.zst")
    );
    // Structured payload: written/unchanged/archived lists so
    // agents can verify merge effects without scraping the
    // multi-line text report.
    let merge_structured = merged_result
        .get("structuredContent")
        .expect("merge structuredContent");
    assert_eq!(merge_structured["change_id"], json!(CHANGE_ID));
    let written = merge_structured["written"]
        .as_array()
        .expect("written array");
    assert!(
        written.iter().any(|p| p.as_str()
            == Some("documentation/specifications/user-auth.md")),
        "written list must include the spec: {merge_structured}"
    );
    assert_eq!(
        merge_structured["archived"]
            .as_str()
            .expect("archived path"),
        "documentation/archives/add-mcp-demo.tar.zst"
    );
    let target = fixture
        .project_root()
        .join("documentation/specifications/user-auth.md");
    let merged_content = std::fs::read_to_string(&target).unwrap();
    assert!(
        merged_content
            .starts_with("<!-- nbspec: change=add-mcp-demo notebook=")
    );
    assert!(merged_content.ends_with(SPECIFICATION));
    assert!(
        fixture
            .project_root()
            .join("documentation/archives/add-mcp-demo.tar.zst")
            .is_file()
    );
}

/// Merge warnings reach text and structured payloads together: a
/// REMOVED name with no target match warns (already-removed) while
/// the effective ADDED block still merges.
#[tokio::test]
async fn mcp_merge_warnings_reach_text_and_structured() {
    let fixture = Fixture::new();
    let mut harness = McpHarness::spawn(&fixture).await;
    harness
        .call_tool(
            "create",
            json!({"change_id": CHANGE_ID, "title": "Demo"})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let dir = fixture.notebook_path().join("proposals").join(CHANGE_ID);
    let mut proposal =
        std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nProve warning parity.\n");
    std::fs::write(dir.join("proposal.md"), proposal).unwrap();
    std::fs::write(
        dir.join("specifications/user-auth.md"),
        "# user-auth\n\n## ADDED Requirements\n\n### Requirement: New\nText.\n\n#### Scenario: S\n- **WHEN** x\n- **THEN** y\n\n## REMOVED Requirements\n\n### Requirement: Ghost\n",
    )
    .unwrap();
    super::lifecycle::commit_notebook(&fixture);
    harness
        .call_tool(
            "review",
            json!({"change_id": CHANGE_ID, "verdict": "approve", "reviewer": "itest"})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let merged = harness
        .call_tool(
            "merge",
            json!({"change_id": CHANGE_ID})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let result = assert_success(&merged);
    let text = first_text(result);
    assert!(text.contains("warning:"), "text carries warnings: {text}");
    assert!(text.contains("Ghost"), "text names the requirement: {text}");
    let structured =
        result.get("structuredContent").expect("structuredContent");
    let warnings = structured["warnings"].as_array().expect("warnings array");
    assert_eq!(warnings.len(), 1, "structured carries one warning");
    assert!(
        warnings[0]
            .as_str()
            .expect("warning text")
            .contains("Ghost")
    );
}

#[tokio::test]
async fn mcp_server_rejects_unknown_field() {
    let fixture = Fixture::new();
    let mut harness = McpHarness::spawn(&fixture).await;

    // `notebook` is a per-tool override; deny_unknown_fields rejects
    // it before the tool handler runs. rmcp converts the resulting
    // schema-deserialization failure into a tool-level error
    // (`result.isError: true`) so the caller sees a clear message
    // instead of an opaque protocol error.
    let response = harness
        .call_tool(
            "display",
            json!({
                "change_id": CHANGE_ID,
                "notebook": "should-be-rejected",
            })
            .as_object()
            .cloned()
            .expect("display args object"),
        )
        .await;
    let result = assert_tool_error(&response);
    let text = first_text(result);
    assert!(
        text.contains("unknown field `notebook`"),
        "expected unknown-field message, got: {text}"
    );
}

/// MCP Owner ring-1 v0.3.0-pause re-review (msg 1265cd09) bounded
/// correction #2 (live side): the MCP `import` tool's
/// `structuredContent` must surface typed per-entry payloads
/// (kind / status / change_id / source_path / prerequisite) for
/// `ActiveWrite` entries and the `ArchiveWrite` entries. The CLI
/// subprocess surface emits text only; the MCP surface carries the
/// structured payload that agents branch on.
#[tokio::test]
async fn mcp_import_structured_payload_carries_typed_entries() {
    let fixture = Fixture::new();

    // Symlink the canonical interchange fixture (one active change
    // `add-foo/` and one legacy archive tree under
    // `openspec/changes/archive/legacy/`) into the scratch fixture.
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/import-classic");
    let fixture_link = fixture.project_root().join("import-classic");
    std::os::unix::fs::symlink(&fixture_root, &fixture_link)
        .expect("symlinking fixture should succeed");

    let mut harness = McpHarness::spawn(&fixture).await;

    // import: mixed fixture, no --no-active, no --delete-original.
    let response = harness
        .call_tool(
            "import",
            json!({
                "root": fixture_link.display().to_string(),
            })
            .as_object()
            .cloned()
            .expect("import args object"),
        )
        .await;
    let result = assert_success(&response);
    let structured = result
        .get("structuredContent")
        .expect("import must return structuredContent");
    let entries = structured
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("structured.entries must be an array");

    // ActiveWrite typed payload — now via Transaction (no paused status).
    let active = entries
        .iter()
        .find(|entry| entry.get("kind").and_then(|v| v.as_str()) == Some("active-write"))
        .expect("an active-write entry must be present in the import structured payload");
    assert_eq!(
        active.get("change_id").and_then(|v| v.as_str()),
        Some("add-foo"),
        "active-write change_id must match fixture: {active}"
    );
    assert!(
        active
            .get("source_path")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("add-foo"))
            .unwrap_or(false),
        "active-write source_path must point at the active source: {active}"
    );
    // After resume, active-write is no longer paused (no status/prerequisite).
    assert!(
        active.get("status").is_none(),
        "active-write should not carry paused status after resume: {active}"
    );
    assert!(
        active.get("prerequisite").is_none(),
        "active-write should not carry prerequisite after resume: {active}"
    );

    // ArchiveWrite typed payload.
    let archive = entries
        .iter()
        .find(|entry| {
            entry.get("kind").and_then(|v| v.as_str()) == Some("archive-write")
        })
        .expect("an archive-write entry must be present");
    assert_eq!(
        archive.get("change_id").and_then(|v| v.as_str()),
        Some("legacy"),
        "archive-write change_id must match fixture: {archive}"
    );
    assert!(
        archive
            .get("target_path")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("legacy.tar.zst"))
            .unwrap_or(false),
        "archive-write target_path must point at the tar.zst: {archive}"
    );

    // pending_deletions is empty (delete_original was not set).
    let pending = structured
        .get("pending_deletions")
        .and_then(|v| v.as_array())
        .expect("pending_deletions must be an array");
    assert!(
        pending.is_empty(),
        "pending_deletions must be empty without delete_original: {pending:?}"
    );

    // The archive write is reported in `written`.
    let written = structured
        .get("written")
        .and_then(|v| v.as_array())
        .expect("written must be an array");
    assert!(
        written.iter().any(|v| v
            .as_str()
            .map(|s| s.contains("legacy"))
            .unwrap_or(false)),
        "archive write must be reported in `written`: {written:?}"
    );
}

/// MCP Owner ring-1 v0.3.0-pause re-review of 48967d8 (msg
/// 3fba4c1a) bounded correction B2: the prior pending-deletion
/// unit test manually injected `pending_deletions` into the plan
/// fixture (testing `from_plan`'s serialization, not the filter),
/// and the prior live MCP test omitted `delete_original`. This
/// test exercises the real `build_import_plan` filter end-to-end:
/// dry_run=true + delete_original=true against the mixed fixture
/// (one active `add-foo`, one archive `legacy`). Dry-run writes
/// nothing, so the fixture is not mutated; the structured payload
/// carries the plan's pending_deletions field directly from
/// `build_import_plan`'s filter. Asserts the paused active source
/// is excluded and only the archive source appears.
#[tokio::test]
async fn mcp_import_dry_run_delete_original_exercises_real_pending_deletions_filter()
 {
    let fixture = Fixture::new();

    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/import-classic");
    let fixture_link = fixture.project_root().join("import-classic");
    std::os::unix::fs::symlink(&fixture_root, &fixture_link)
        .expect("symlinking fixture should succeed");

    let mut harness = McpHarness::spawn(&fixture).await;

    let response = harness
        .call_tool(
            "import",
            json!({
                "root": fixture_link.display().to_string(),
                "dry_run": true,
                "delete_original": true,
            })
            .as_object()
            .cloned()
            .expect("import args object"),
        )
        .await;
    let result = assert_success(&response);
    let structured = result
        .get("structuredContent")
        .expect("import dry-run must return structuredContent");

    // dry_run must be reflected in the structured payload.
    assert_eq!(
        structured.get("dry_run").and_then(|v| v.as_bool()),
        Some(true),
        "dry_run must be true in structured payload: {structured}"
    );

    // After resume, pending_deletions includes both archive and active sources.
    let pending = structured
        .get("pending_deletions")
        .and_then(|v| v.as_array())
        .expect("pending_deletions must be an array");
    assert_eq!(
        pending.len(),
        2,
        "both archive and active sources must be pending deletion: {pending:?}"
    );
    assert!(
        pending.iter().any(|v| v
            .as_str()
            .map(|s| s.contains("legacy"))
            .unwrap_or(false)),
        "pending deletion must include the archive source: {pending:?}"
    );
    assert!(
        pending.iter().any(|v| v
            .as_str()
            .map(|s| s.contains("add-foo"))
            .unwrap_or(false)),
        "pending deletion must include the active source after resume: {pending:?}"
    );

    // delete_original flag is reflected for downstream consumers.
    assert_eq!(
        structured.get("delete_original").and_then(|v| v.as_bool()),
        Some(true),
        "delete_original must be true in structured payload: {structured}"
    );

    // The dry-run must not have written any archive.
    assert!(
        !fixture
            .project_root()
            .join("documentation/archives/legacy.tar.zst")
            .exists(),
        "dry-run must not write the archive"
    );
}

/// Strict MCP merge refusal carries typed diagnostics in both
/// channels: the text names the ScenarioDropped code with identity,
/// and structuredContent carries the same codes without prose
/// parsing.
#[tokio::test]
async fn mcp_strict_merge_refusal_carries_typed_diagnostics() {
    let fixture = Fixture::new();
    let mut harness = McpHarness::spawn(&fixture).await;
    harness
        .call_tool(
            "create",
            json!({"change_id": CHANGE_ID, "title": "Demo"})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let dir = fixture.notebook_path().join("proposals").join(CHANGE_ID);
    let mut proposal =
        std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nProve strict diagnostics.\n");
    std::fs::write(dir.join("proposal.md"), proposal).unwrap();
    // ADDED applies cleanly but the REMOVED Ghost warns; strict
    // promotes that preflight warning to a refusal.
    std::fs::write(
        dir.join("specifications/user-auth.md"),
        "# user-auth\n\n## ADDED Requirements\n\n### Requirement: New\nText.\n\n#### Scenario: S\n- **WHEN** x\n- **THEN** y\n\n## REMOVED Requirements\n\n### Requirement: Ghost\n",
    )
    .unwrap();
    super::lifecycle::commit_notebook(&fixture);
    harness
        .call_tool(
            "review",
            json!({"change_id": CHANGE_ID, "verdict": "approve", "reviewer": "itest"})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let refused = harness
        .call_tool(
            "merge",
            json!({"change_id": CHANGE_ID, "strict": true})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let result = assert_tool_error(&refused);
    let text = first_text(result);
    assert!(
        text.contains("AlreadyRemoved") && text.contains("Ghost"),
        "error text names code and identity: {text}"
    );
    let structured = result
        .get("structuredContent")
        .expect("structuredContent on strict refusal");
    let diagnostics = structured["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], json!("AlreadyRemoved"));
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .expect("message")
            .contains("Ghost"),
        "diagnostic message names the requirement"
    );
    // Non-strict merge of the same change warns and applies with the
    // same code in the success structured payload.
    let merged = harness
        .call_tool(
            "merge",
            json!({"change_id": CHANGE_ID})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let ok = assert_success(&merged);
    let ok_text = first_text(ok);
    assert!(
        ok_text.contains("AlreadyRemoved"),
        "success text names the code: {ok_text}"
    );
    let ok_structured = ok
        .get("structuredContent")
        .expect("structuredContent on success");
    let ok_diagnostics = ok_structured["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        ok_diagnostics
            .iter()
            .any(|d| d["code"] == json!("AlreadyRemoved")),
        "success diagnostics carry the code"
    );
    // Compatibility: the legacy warnings string array stays populated.
    let warnings = ok_structured["warnings"]
        .as_array()
        .expect("warnings array");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().is_some_and(|s| s.contains("Ghost"))),
        "warnings projection kept"
    );
}

/// Mixed strict refusal: one target fires a preflight diagnostic
/// while another is blocked by a non-file occupant. The MCP error
/// must still carry the strict diagnostics (not fall back to
/// text-only), and neither target, the archive, nor the namespace
/// may change.
#[tokio::test]
async fn mcp_mixed_strict_refusal_keeps_diagnostics_and_writes_nothing() {
    let fixture = Fixture::new();
    let mut harness = McpHarness::spawn(&fixture).await;
    harness
        .call_tool(
            "create",
            json!({"change_id": CHANGE_ID, "title": "Demo"})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let dir = fixture.notebook_path().join("proposals").join(CHANGE_ID);
    let mut proposal =
        std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nProve mixed strict refusal.\n");
    std::fs::write(dir.join("proposal.md"), proposal).unwrap();
    std::fs::write(
        dir.join("specifications/user-auth.md"),
        "# user-auth\n\n## ADDED Requirements\n\n### Requirement: New\nText.\n\n## REMOVED Requirements\n\n### Requirement: Ghost\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("specifications/blocked.md"),
        "# blocked\n\n## ADDED Requirements\n\n### Requirement: Fresh\nText.\n",
    )
    .unwrap();
    // Occupy the blocked target with a directory (non-file).
    let blocked_target = fixture
        .project_root()
        .join("documentation/specifications/blocked.md");
    std::fs::create_dir_all(&blocked_target).unwrap();
    super::lifecycle::commit_notebook(&fixture);
    harness
        .call_tool(
            "review",
            json!({"change_id": CHANGE_ID, "verdict": "approve", "reviewer": "itest"})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let refused = harness
        .call_tool(
            "merge",
            json!({"change_id": CHANGE_ID, "strict": true})
                .as_object()
                .cloned()
                .expect("args"),
        )
        .await;
    let result = assert_tool_error(&refused);
    let text = first_text(result);
    assert!(
        text.contains("AlreadyRemoved") && text.contains("Ghost"),
        "error text keeps strict identity alongside legacy: {text}"
    );
    let structured = result
        .get("structuredContent")
        .expect("structuredContent on mixed refusal");
    let diagnostics = structured["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|d| d["code"] == json!("AlreadyRemoved")),
        "strict diagnostics survive the mixed refusal: {diagnostics:?}"
    );
    // Neither target changed: user-auth unwritten, blocked still a dir.
    assert!(
        !fixture
            .project_root()
            .join("documentation/specifications/user-auth.md")
            .exists(),
        "strict target unwritten"
    );
    assert!(
        fixture
            .project_root()
            .join("documentation/specifications/blocked.md")
            .is_dir(),
        "non-file occupant untouched"
    );
    // No archive published, namespace retained.
    assert!(
        !fixture
            .project_root()
            .join("documentation/archives")
            .join(format!("{CHANGE_ID}.tar.zst"))
            .exists(),
        "no archive on refusal"
    );
    assert!(
        fixture
            .notebook_path()
            .join("proposals")
            .join(CHANGE_ID)
            .exists(),
        "namespace retained on refusal"
    );
}
