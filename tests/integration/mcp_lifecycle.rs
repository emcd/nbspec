//! End-to-end MCP integration test: spawn `nbspec serve mcp` on stdio
//! inside a scratch project, drive the six tools (create, display,
//! validate, render, merge, review) over JSON-RPC, and verify text
//! + structured responses match the contracts the specification
//!   pins.
//!
//! Like the CLI lifecycle test, this requires `nb` to be installed.
//! The scratch notebook lives in a per-test isolated `NB_DIR` (see
//! `super::harness`); the directory is removed on drop, so the
//! operator's real notebook list never sees scratch notebooks from
//! this test. The `nbspec serve mcp` subprocess inherits the same
//! isolated `NB_DIR` so its internal `nb` invocations see the
//! just-added scratch notebook.
//!
//! All spawned subprocesses — both the test-side `nb` invocations
//! and the `nbspec serve mcp` tokio subprocess — have `GIT_*`
//! environment variables scrubbed (see `super::harness::scrub_git_env`).
//! Without this, a hook or CI environment that exports
//! `GIT_DIR`/`GIT_INDEX_FILE` redirects every git call inside `nb`
//! away from the notebook's repository, the `nb notebooks add` and
//! the subsequent `nb notebooks` listing see different roots, and
//! the subprocess exits before responding. Per `nbspec:issues/4`.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicI64, Ordering},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientCapabilities, ClientJsonRpcMessage,
    Implementation, InitializeRequest, InitializeRequestParams, InitializedNotification,
    ListToolsRequest, PaginatedRequestParams, RequestId,
};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command as TokioCommand},
    task::JoinHandle,
};

use super::harness::{IsolatedNbDir, scrub_git_env, scrub_git_env_async};

const TEMP_TEST_ROOT: &str = ".auxiliary/temporary/tests";
const CHANGE_ID: &str = "add-mcp-demo";
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

const SPECIFICATION: &str = "\
# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users before granting access.

#### Scenario: Valid login
- **WHEN** a user submits correct credentials
- **THEN** a session begins
";

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// A scratch nb notebook, isolated to a per-test `NB_DIR` and
/// removed on drop along with the directory.
struct ScratchNotebook {
    name: String,
    nb_dir: IsolatedNbDir,
}

impl ScratchNotebook {
    fn create() -> Self {
        let nb_dir = IsolatedNbDir::new();
        let name = format!("nbspec-mcp-itest-{}", unique_suffix());
        let mut command = Command::new("nb");
        scrub_git_env(&mut command);
        let output = command
            .env("NB_DIR", nb_dir.path())
            .args(["notebooks", "add", &name])
            .output()
            .expect("nb must be installed for MCP integration tests");
        assert!(output.status.success(), "cannot create scratch notebook");
        ScratchNotebook { name, nb_dir }
    }

    /// Filesystem path of the notebook directory inside the
    /// isolated `NB_DIR`.
    fn path(&self) -> PathBuf {
        let mut command = Command::new("nb");
        scrub_git_env(&mut command);
        let output = command
            .env("NB_DIR", self.nb_dir.path())
            .args(["notebooks", "--paths", "--no-color"])
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&output.stdout);
        listing
            .lines()
            .map(str::trim)
            .find(|line| line.ends_with(&self.name))
            .map(PathBuf::from)
            .expect("scratch notebook path must be listed")
    }

    /// The isolated `NB_DIR`; passed to the `nbspec serve mcp`
    /// subprocess so its internal `nb` invocations see the
    /// just-added scratch notebook.
    fn nb_dir_path(&self) -> &Path {
        self.nb_dir.path()
    }
}

impl Drop for ScratchNotebook {
    fn drop(&mut self) {
        // Best-effort: retry the delete because transient
        // contention can fail an `nb notebooks delete`.
        for _ in 0..3 {
            let mut command = Command::new("nb");
            scrub_git_env(&mut command);
            let deleted = command
                .env("NB_DIR", self.nb_dir.path())
                .args(["notebooks", "delete", &self.name, "--force"])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);
            if deleted {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        // IsolatedNbDir::drop removes the directory itself.
    }
}

/// A scratch project repository: an initialized git repository with a
/// project configuration keeping render scratch inside the sandbox.
struct ScratchProject {
    root: PathBuf,
}

impl ScratchProject {
    fn create() -> Self {
        let root = PathBuf::from(TEMP_TEST_ROOT)
            .join(format!("mcp-lifecycle-{}", unique_suffix()))
            .canonicalize_base();
        std::fs::create_dir_all(&root).unwrap();
        let mut command = Command::new("git");
        scrub_git_env(&mut command);
        let output = command
            .args(["init", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(output.status.success(), "cannot initialize scratch repo");
        let configuration_directory = root.join(".auxiliary/configuration/nbspec");
        std::fs::create_dir_all(&configuration_directory).unwrap();
        std::fs::write(
            configuration_directory.join("general.toml"),
            "schema = \"nbspec-default\"\n\
             scratch_directory = \".auxiliary/temporary/renders\"\n\
             archives = true\n\
             archive_directory = \"documentation/archives\"\n",
        )
        .unwrap();
        ScratchProject { root }
    }
}

impl Drop for ScratchProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

trait CanonicalizeBase {
    fn canonicalize_base(self) -> PathBuf;
}

impl CanonicalizeBase for PathBuf {
    fn canonicalize_base(self) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(self)
    }
}

/// Spawns the `nbspec serve mcp` subprocess and speaks JSON-RPC over
/// its stdin/stdout. The harness owns the child for its lifetime and
/// kills it on drop. Stderr is drained into a shared buffer so the
/// EOF-on-stdout panic can surface the subprocess's exit reason
/// (startup log + anyhow banner) when `nbspec serve mcp` exits
/// before responding. Diagnostic per nbspec:issues/4.
struct McpHarness {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr_buffer: Arc<Mutex<Vec<u8>>>,
    stderr_task: JoinHandle<()>,
    next_id: AtomicI64,
}

impl McpHarness {
    async fn spawn(project: &ScratchProject, notebook: &ScratchNotebook) -> Self {
        let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_nbspec"));
        scrub_git_env_async(&mut command);
        command
            .arg("serve")
            .arg("mcp")
            .arg("--notebook")
            .arg(&notebook.name)
            .current_dir(&project.root)
            .env(
                "NBSPEC_CONFIG_DIR",
                project.root.join(".auxiliary/configuration/nbspec"),
            )
            .env("NB_DIR", notebook.nb_dir_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().expect("spawn nbspec serve mcp");
        let stdin = child.stdin.take().expect("take mcp stdin");
        let stdout = child.stdout.take().expect("take mcp stdout");
        let stderr = child.stderr.take().expect("take mcp stderr");
        // stdin is held in an Option so the EOF panic path can
        // take it and signal the subprocess to exit gracefully;
        // see McpHarness::read_response.

        let stderr_buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_drain_target = Arc::clone(&stderr_buffer);
        // Drain the subprocess's stderr into a shared buffer. The
        // task ends naturally when the subprocess closes its stderr
        // (i.e., on exit). EOF-on-stdout panic awaits the task
        // before reading the buffer so all output is captured.
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(mut buffer) = stderr_drain_target.lock() {
                            buffer.extend_from_slice(line.as_bytes());
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let mut harness = Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr_buffer,
            stderr_task,
            next_id: AtomicI64::new(1),
        };
        harness.initialize().await;
        harness
    }

    fn next_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Returns the captured stderr text from the subprocess. Empty
    /// if nothing was emitted (e.g., the test panicked before any
    /// stderr was produced).
    fn captured_stderr(&self) -> String {
        let buffer = self
            .stderr_buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        String::from_utf8_lossy(&buffer).into_owned()
    }

    async fn initialize(&mut self) {
        let initialize = InitializeRequest::new(InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("nbspec-mcp-itest", "0.0.0"),
        ));
        let id = self.next_id();
        self.send_request(initialize.into(), id).await;
        let response = self.read_response(id).await;
        assert!(
            response.get("result").is_some(),
            "initialize response must contain result: {response}"
        );

        self.send_notification(InitializedNotification::default().into())
            .await;
    }

    async fn list_tools(&mut self) -> Value {
        let id = self.next_id();
        self.send_request(
            ListToolsRequest::with_param(PaginatedRequestParams::default()).into(),
            id,
        )
        .await;
        self.read_response(id).await
    }

    async fn call_tool(&mut self, name: &str, arguments: Map<String, Value>) -> Value {
        let id = self.next_id();
        let request = CallToolRequest::new(
            CallToolRequestParams::new(name.to_string()).with_arguments(arguments),
        );
        self.send_request(request.into(), id).await;
        self.read_response(id).await
    }

    async fn send_request(&mut self, request: rmcp::model::ClientRequest, id: i64) {
        let message = ClientJsonRpcMessage::request(request, RequestId::Number(id));
        self.send(message).await;
    }

    async fn send_notification(&mut self, notification: rmcp::model::ClientNotification) {
        let message = ClientJsonRpcMessage::notification(notification);
        self.send(message).await;
    }

    async fn send(&mut self, message: ClientJsonRpcMessage) {
        let stdin = self
            .stdin
            .as_mut()
            .expect("mcp stdin closed; cannot send more requests");
        let line = serde_json::to_string(&message).expect("encode mcp request");
        stdin
            .write_all(line.as_bytes())
            .await
            .expect("write mcp request");
        stdin.write_all(b"\n").await.expect("write mcp newline");
        stdin.flush().await.expect("flush mcp request");
    }

    async fn read_response(&mut self, id: i64) -> Value {
        let expected = RequestId::Number(id);
        let deadline = Instant::now() + READ_TIMEOUT;
        let mut line = String::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for MCP response id {id}"
            );
            line.clear();
            match tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                self.stdout.read_line(&mut line),
            )
            .await
            {
                Ok(Ok(0)) => {
                    // EOF: subprocess closed stdout. Bound the wait
                    // so a misbehaving subprocess that ignores stdout
                    // close (and stays blocked on our still-piped
                    // stdin) cannot wedge cargo test indefinitely.
                    // Drop stdin to signal graceful shutdown; if the
                    // subprocess doesn't exit within SHUTDOWN_TIMEOUT,
                    // send SIGKILL. Per Codex review of PR #1.
                    drop(self.stdin.take());
                    let exit_status =
                        match tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
                            Ok(Ok(status)) => Some(status),
                            Ok(Err(_)) | Err(_) => {
                                let _ = self.child.start_kill();
                                self.child.wait().await.ok()
                            }
                        };
                    // Take ownership of the stderr drain task via
                    // mem::replace so we can await it (JoinHandle
                    // is not Clone and IntoFuture consumes self).
                    // The replacement is a no-op task that never
                    // gets awaited again.
                    let stderr_task = std::mem::replace(
                        &mut self.stderr_task,
                        tokio::spawn(std::future::ready(())),
                    );
                    let _ = stderr_task.await;
                    let stderr_text = self.captured_stderr();
                    panic!(
                        "mcp process closed stdout (exit status: {exit_status:?}); \
                         captured stderr:\n{stderr_text}"
                    );
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("read mcp response line: {error}"),
                Err(_) => panic!("timed out waiting for MCP response id {id}"),
            }
            let decoded: Value =
                serde_json::from_str(line.trim_end()).expect("decode mcp response");
            let response_id = decoded
                .get("id")
                .and_then(|id_value| serde_json::from_value::<RequestId>(id_value.clone()).ok());
            if response_id.as_ref() == Some(&expected) {
                return decoded;
            }
        }
    }
}

impl Drop for McpHarness {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

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
    let notebook = ScratchNotebook::create();
    let project = ScratchProject::create();
    let mut harness = McpHarness::spawn(&project, &notebook).await;

    // List tools: must include exactly the five verbs.
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
        vec!["create", "display", "merge", "render", "review", "validate"],
        "tools/list must expose exactly the six CLI verbs"
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
    assert_eq!(create_structured["notebook"], json!(&notebook.name));

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
    assert_eq!(display_structured["notebook"], json!(&notebook.name));
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
    let change_directory = notebook.path().join("proposals").join(CHANGE_ID);
    let mut proposal = std::fs::read_to_string(change_directory.join("proposal.md")).unwrap();
    proposal.push_str("\n## Why\n\nDrive the MCP lifecycle.\n");
    std::fs::write(change_directory.join("proposal.md"), proposal).unwrap();
    let specification_note = change_directory.join("specifications/user-auth.md");
    std::fs::write(&specification_note, SPECIFICATION).unwrap();

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
    let scratch_document = project
        .root
        .join(".auxiliary/temporary/renders")
        .join(&notebook.name)
        .join(CHANGE_ID)
        .join("specifications/user-auth.md");
    assert_eq!(
        std::fs::read_to_string(&scratch_document).unwrap(),
        SPECIFICATION
    );
    assert!(!project.root.join("documentation").exists());

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
        first_text(refused_result).contains("review gate unsatisfied: no verdict"),
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
        first_text(reviewed_result).contains("Recorded approve verdict by itest"),
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
    let structured_note_basename = std::path::Path::new(structured_note_relpath)
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
        !structured_verdict_id.is_empty()
            && structured_verdict_id
                .chars()
                .take(15)
                .all(|c| c.is_ascii_digit() || c == '-'),
        "structured note basename must be a non-empty compact-timestamp verdict id"
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
    assert!(merged_text.contains("wrote documentation/specifications/user-auth.md"));
    assert!(merged_text.contains("archived documentation/archives/add-mcp-demo.tar.zst"));
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
        written
            .iter()
            .any(|p| p.as_str() == Some("documentation/specifications/user-auth.md")),
        "written list must include the spec: {merge_structured}"
    );
    assert_eq!(
        merge_structured["archived"]
            .as_str()
            .expect("archived path"),
        "documentation/archives/add-mcp-demo.tar.zst"
    );
    let target = project
        .root
        .join("documentation/specifications/user-auth.md");
    let merged_content = std::fs::read_to_string(&target).unwrap();
    assert!(merged_content.starts_with("<!-- nbspec: change=add-mcp-demo notebook="));
    assert!(merged_content.ends_with(SPECIFICATION));
    assert!(
        project
            .root
            .join("documentation/archives/add-mcp-demo.tar.zst")
            .is_file()
    );
}

#[tokio::test]
async fn mcp_server_rejects_unknown_field() {
    let notebook = ScratchNotebook::create();
    let project = ScratchProject::create();
    let mut harness = McpHarness::spawn(&project, &notebook).await;

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
