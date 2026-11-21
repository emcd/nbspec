//! JSON-RPC client harness for the `nbspec serve mcp` subprocess.
//! Extracted from `mcp_lifecycle` so the integration test module
//! stays within the line budget; behavior unchanged.

use std::{
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientCapabilities,
    ClientJsonRpcMessage, Implementation, InitializeRequest,
    InitializeRequestParams, InitializedNotification, ListToolsRequest,
    PaginatedRequestParams, RequestId,
};
use serde_json::{Map, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command as TokioCommand},
    task::JoinHandle,
};

use super::harness::Fixture;

pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Spawns the `nbspec serve mcp` subprocess and speaks JSON-RPC over
/// its stdin/stdout. The harness owns the child for its lifetime and
/// kills it on drop. Stderr is drained into a shared buffer so the
/// EOF-on-stdout panic can surface the subprocess's exit reason
/// (startup log + anyhow banner) when `nbspec serve mcp` exits
/// before responding. Diagnostic per nbspec:issues/4.
pub(crate) struct McpHarness {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr_buffer: Arc<Mutex<Vec<u8>>>,
    stderr_task: JoinHandle<()>,
    next_id: AtomicI64,
}

impl McpHarness {
    pub(crate) async fn spawn(fixture: &Fixture) -> Self {
        let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_nbspec"));
        fixture.configure_tokio(&mut command);
        command
            .arg("serve")
            .arg("mcp")
            .arg("--notebook")
            .arg(fixture.notebook())
            .env("NBSPEC_CONFIG_DIR", fixture.configuration_directory())
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

        let stderr_buffer: Arc<Mutex<Vec<u8>>> =
            Arc::new(Mutex::new(Vec::new()));
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
                    },
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

    pub(crate) async fn list_tools(&mut self) -> Value {
        let id = self.next_id();
        self.send_request(
            ListToolsRequest::with_param(PaginatedRequestParams::default())
                .into(),
            id,
        )
        .await;
        self.read_response(id).await
    }

    pub(crate) async fn call_tool(
        &mut self,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Value {
        let id = self.next_id();
        let request = CallToolRequest::new(
            CallToolRequestParams::new(name.to_string())
                .with_arguments(arguments),
        );
        self.send_request(request.into(), id).await;
        self.read_response(id).await
    }

    async fn send_request(
        &mut self,
        request: rmcp::model::ClientRequest,
        id: i64,
    ) {
        let message =
            ClientJsonRpcMessage::request(request, RequestId::Number(id));
        self.send(message).await;
    }

    async fn send_notification(
        &mut self,
        notification: rmcp::model::ClientNotification,
    ) {
        let message = ClientJsonRpcMessage::notification(notification);
        self.send(message).await;
    }

    async fn send(&mut self, message: ClientJsonRpcMessage) {
        let stdin = self
            .stdin
            .as_mut()
            .expect("mcp stdin closed; cannot send more requests");
        let line =
            serde_json::to_string(&message).expect("encode mcp request");
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
                    let exit_status = match tokio::time::timeout(
                        SHUTDOWN_TIMEOUT,
                        self.child.wait(),
                    )
                    .await
                    {
                        Ok(Ok(status)) => Some(status),
                        Ok(Err(_)) | Err(_) => {
                            let _ = self.child.start_kill();
                            self.child.wait().await.ok()
                        },
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
                },
                Ok(Ok(_)) => {},
                Ok(Err(error)) => panic!("read mcp response line: {error}"),
                Err(_) => panic!("timed out waiting for MCP response id {id}"),
            }
            let decoded: Value = serde_json::from_str(line.trim_end())
                .expect("decode mcp response");
            let response_id = decoded.get("id").and_then(|id_value| {
                serde_json::from_value::<RequestId>(id_value.clone()).ok()
            });
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
