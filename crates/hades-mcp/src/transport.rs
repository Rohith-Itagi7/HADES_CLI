use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace};

use crate::error::McpError;
use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// Core abstraction for MCP message transport.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Sends a JSON-RPC request and awaits the corresponding response.
    async fn send_request(
        &self,
        request: JsonRpcRequest,
        timeout: Duration,
    ) -> Result<JsonRpcResponse, McpError>;

    /// Sends a fire-and-forget JSON-RPC notification.
    async fn send_notification(&self, notification: JsonRpcNotification) -> Result<(), McpError>;

    /// Returns whether the transport connection is currently active and healthy.
    fn is_alive(&self) -> bool;

    /// Gracefully closes and cleans up the transport connection.
    async fn close(&self) -> Result<(), McpError>;
}

/// Standard I/O (STDIO) process transport for local MCP servers.
pub struct StdioTransport {
    server_name: String,
    stdin_writer: Arc<Mutex<Option<ChildStdin>>>,
    pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<JsonRpcResponse>>>>,
    is_running: Arc<AtomicBool>,
    child_process: Arc<Mutex<Option<Child>>>,
    request_counter: AtomicU64,
}

impl StdioTransport {
    /// Spawns a child process and initializes the bidirectional STDIO transport.
    pub async fn spawn(
        server_name: impl Into<String>,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        working_dir: Option<&PathBuf>,
    ) -> Result<Self, McpError> {
        let name = server_name.into();
        info!(server = %name, cmd = %command, "Spawning STDIO MCP server process");

        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            McpError::StartupFailed(
                name.clone(),
                format!("Failed to execute command '{command}': {e}"),
            )
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            McpError::StartupFailed(name.clone(), "Failed to capture stdin pipe".to_string())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            McpError::StartupFailed(name.clone(), "Failed to capture stdout pipe".to_string())
        })?;

        let stderr = child.stderr.take();

        let pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let is_running = Arc::new(AtomicBool::new(true));

        // Background reader loop for stdout
        let pending_clone = pending_requests.clone();
        let running_clone = is_running.clone();
        let name_clone = name.clone();

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                trace!(server = %name_clone, raw_line = %trimmed, "STDIO frame received");

                // Parse as JSON-RPC response
                if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                    let req_id_str = match &response.id {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        other => other.to_string(),
                    };

                    let mut pending = pending_clone.write().await;
                    if let Some(tx) = pending.remove(&req_id_str) {
                        let _ = tx.send(response);
                    } else {
                        debug!(server = %name_clone, id = %req_id_str, "Unmatched response ID");
                    }
                }
            }

            running_clone.store(false, Ordering::SeqCst);
            debug!(server = %name_clone, "STDIO stdout reader exited");
        });

        // Background reader loop for stderr (logs/diagnostics)
        if let Some(stderr_pipe) = stderr {
            let name_err = name.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr_pipe);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        debug!(server = %name_err, stderr = %trimmed, "MCP server stderr");
                    }
                }
            });
        }

        Ok(Self {
            server_name: name,
            stdin_writer: Arc::new(Mutex::new(Some(stdin))),
            pending_requests,
            is_running,
            child_process: Arc::new(Mutex::new(Some(child))),
            request_counter: AtomicU64::new(1),
        })
    }

    /// Allocates an incremental request ID.
    pub fn next_request_id(&self) -> String {
        self.request_counter
            .fetch_add(1, Ordering::SeqCst)
            .to_string()
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(
        &self,
        mut request: JsonRpcRequest,
        timeout_dur: Duration,
    ) -> Result<JsonRpcResponse, McpError> {
        if !self.is_alive() {
            return Err(McpError::NotConnected(self.server_name.clone()));
        }

        let id_str = match &request.id {
            serde_json::Value::Null => {
                let generated = self.next_request_id();
                request.id = serde_json::Value::String(generated.clone());
                generated
            }
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            other => other.to_string(),
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.write().await;
            pending.insert(id_str.clone(), tx);
        }

        let mut payload = serde_json::to_string(&request)?;
        payload.push('\n');

        {
            let mut stdin_guard = self.stdin_writer.lock().await;
            if let Some(ref mut stdin) = *stdin_guard {
                if let Err(e) = stdin.write_all(payload.as_bytes()).await {
                    self.is_running.store(false, Ordering::SeqCst);
                    let mut pending = self.pending_requests.write().await;
                    pending.remove(&id_str);
                    return Err(McpError::Transport(format!("Write to stdin failed: {e}")));
                }
                let _ = stdin.flush().await;
            } else {
                let mut pending = self.pending_requests.write().await;
                pending.remove(&id_str);
                return Err(McpError::NotConnected(self.server_name.clone()));
            }
        }

        match tokio::time::timeout(timeout_dur, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                let mut pending = self.pending_requests.write().await;
                pending.remove(&id_str);
                Err(McpError::ProcessTerminated(format!(
                    "Server '{}' terminated while waiting for response",
                    self.server_name
                )))
            }
            Err(_) => {
                let mut pending = self.pending_requests.write().await;
                pending.remove(&id_str);
                Err(McpError::Timeout(self.server_name.clone(), timeout_dur))
            }
        }
    }

    async fn send_notification(&self, notification: JsonRpcNotification) -> Result<(), McpError> {
        if !self.is_alive() {
            return Err(McpError::NotConnected(self.server_name.clone()));
        }

        let mut payload = serde_json::to_string(&notification)?;
        payload.push('\n');

        let mut stdin_guard = self.stdin_writer.lock().await;
        if let Some(ref mut stdin) = *stdin_guard {
            stdin
                .write_all(payload.as_bytes())
                .await
                .map_err(|e| McpError::Transport(format!("Notification write failed: {e}")))?;
            let _ = stdin.flush().await;
            Ok(())
        } else {
            Err(McpError::NotConnected(self.server_name.clone()))
        }
    }

    fn is_alive(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    async fn close(&self) -> Result<(), McpError> {
        self.is_running.store(false, Ordering::SeqCst);

        // Close stdin to signal EOF to child
        {
            let mut stdin_guard = self.stdin_writer.lock().await;
            *stdin_guard = None;
        }

        // Cleanly terminate child process
        let mut child_guard = self.child_process.lock().await;
        if let Some(mut child) = child_guard.take() {
            info!(server = %self.server_name, "Terminating STDIO MCP server process");
            let _ = child.kill().await;
            let _ = child.wait().await;
        }

        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct SseEvent {
    event_type: String,
    data: String,
}

fn parse_sse_event(frame: &str) -> Result<Option<SseEvent>, McpError> {
    let mut event_type = None;
    let mut data = Vec::new();

    for line in frame.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = line
            .split_once(':')
            .ok_or_else(|| McpError::Protocol(format!("Malformed SSE field: {line}")))?;
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event_type = Some(value.to_string()),
            "data" => data.push(value.to_string()),
            "id" | "retry" => {}
            _ => {}
        }
    }

    if data.is_empty() {
        return Ok(None);
    }

    Ok(Some(SseEvent {
        event_type: event_type.unwrap_or_else(|| "message".to_string()),
        data: data.join("\n"),
    }))
}

fn endpoint_from_sse_event(event: &SseEvent) -> Result<Option<String>, McpError> {
    if event.event_type != "endpoint" {
        return Ok(None);
    }

    let endpoint = event.data.trim();
    if endpoint.is_empty() {
        return Err(McpError::Protocol(
            "MCP SSE endpoint event contained no POST endpoint".to_string(),
        ));
    }
    Ok(Some(endpoint.to_string()))
}

fn response_from_sse_event(event: &SseEvent) -> Result<Option<JsonRpcResponse>, McpError> {
    if event.event_type != "message" {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&event.data)?))
}

async fn route_sse_response(
    pending_requests: &RwLock<HashMap<String, oneshot::Sender<JsonRpcResponse>>>,
    response: JsonRpcResponse,
) -> bool {
    let id = json_rpc_id(&response.id);
    if let Some(sender) = pending_requests.write().await.remove(&id) {
        let _ = sender.send(response);
        true
    } else {
        false
    }
}

fn json_rpc_id(id: &serde_json::Value) -> String {
    match id {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Number(value) => value.to_string(),
        other => other.to_string(),
    }
}

/// Legacy MCP HTTP+SSE transport with a persistent server-to-client event stream.
pub struct SseTransport {
    server_name: String,
    client: reqwest::Client,
    headers: HashMap<String, String>,
    post_endpoint: Arc<RwLock<Option<String>>>,
    pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<JsonRpcResponse>>>>,
    is_running: Arc<AtomicBool>,
    request_counter: AtomicU64,
    reader_task: Mutex<Option<JoinHandle<()>>>,
}

impl SseTransport {
    /// Opens a legacy MCP SSE stream and waits for its POST endpoint event.
    pub async fn connect(
        server_name: impl Into<String>,
        sse_url: &str,
        headers: HashMap<String, String>,
        timeout: Duration,
    ) -> Result<Self, McpError> {
        let server_name = server_name.into();
        let client = reqwest::Client::builder().build().unwrap_or_default();
        let mut request = client.get(sse_url).header("Accept", "text/event-stream");
        for (key, value) in &headers {
            request = request.header(key, value);
        }

        let response = tokio::time::timeout(timeout, request.send())
            .await
            .map_err(|_| McpError::Timeout(server_name.clone(), timeout))?
            .map_err(|error| McpError::Transport(format!("SSE connection failed: {error}")))?;
        if !response.status().is_success() {
            return Err(McpError::Transport(format!(
                "SSE server returned status {}",
                response.status()
            )));
        }

        let post_endpoint = Arc::new(RwLock::new(None));
        let pending_requests = Arc::new(RwLock::new(HashMap::new()));
        let is_running = Arc::new(AtomicBool::new(true));
        let (endpoint_tx, endpoint_rx) = oneshot::channel();

        let task = Self::spawn_reader(
            response,
            server_name.clone(),
            post_endpoint.clone(),
            pending_requests.clone(),
            is_running.clone(),
            endpoint_tx,
        );

        let endpoint = match tokio::time::timeout(timeout, endpoint_rx).await {
            Ok(Ok(Ok(endpoint))) => endpoint,
            Ok(Ok(Err(error))) => {
                task.abort();
                return Err(error);
            }
            Ok(Err(_)) => {
                task.abort();
                return Err(McpError::ProcessTerminated(format!(
                    "SSE stream for '{}' closed before its endpoint event",
                    server_name
                )));
            }
            Err(_) => {
                task.abort();
                return Err(McpError::Timeout(server_name.clone(), timeout));
            }
        };
        *post_endpoint.write().await = Some(endpoint);

        Ok(Self {
            server_name,
            client,
            headers,
            post_endpoint,
            pending_requests,
            is_running,
            request_counter: AtomicU64::new(1),
            reader_task: Mutex::new(Some(task)),
        })
    }

    fn spawn_reader(
        response: reqwest::Response,
        server_name: String,
        post_endpoint: Arc<RwLock<Option<String>>>,
        pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<JsonRpcResponse>>>>,
        is_running: Arc<AtomicBool>,
        endpoint_tx: oneshot::Sender<Result<String, McpError>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut endpoint_tx = Some(endpoint_tx);
            let mut stream = response.bytes_stream();
            let mut buffer = Vec::new();

            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        debug!(server = %server_name, error = %error, "SSE stream read failed");
                        break;
                    }
                };
                buffer.extend_from_slice(&chunk);

                while let Some(end) = sse_frame_end(&buffer) {
                    let frame: Vec<u8> = buffer.drain(..end).collect();
                    buffer.drain(..sse_delimiter_len(&buffer));
                    let frame = match String::from_utf8(frame) {
                        Ok(frame) => frame,
                        Err(error) => {
                            debug!(server = %server_name, error = %error, "SSE frame was not UTF-8");
                            continue;
                        }
                    };
                    let event = match parse_sse_event(&frame) {
                        Ok(Some(event)) => event,
                        Ok(None) => continue,
                        Err(error) => {
                            debug!(server = %server_name, error = %error, "Malformed SSE frame");
                            continue;
                        }
                    };

                    match endpoint_from_sse_event(&event) {
                        Ok(Some(endpoint)) => {
                            *post_endpoint.write().await = Some(endpoint.clone());
                            if let Some(sender) = endpoint_tx.take() {
                                let _ = sender.send(Ok(endpoint));
                            }
                        }
                        Ok(None) => match response_from_sse_event(&event) {
                            Ok(Some(response)) => {
                                let id = json_rpc_id(&response.id);
                                if !route_sse_response(&pending_requests, response).await {
                                    debug!(server = %server_name, id = %id, "Unmatched SSE response ID");
                                }
                            }
                            Ok(None) => {}
                            Err(error) => {
                                debug!(server = %server_name, error = %error, "Malformed SSE message payload")
                            }
                        },
                        Err(error) => {
                            if let Some(sender) = endpoint_tx.take() {
                                let _ = sender.send(Err(error));
                            }
                        }
                    }
                }
            }

            is_running.store(false, Ordering::SeqCst);
            pending_requests.write().await.clear();
            debug!(server = %server_name, "SSE reader exited");
        })
    }

    pub fn next_request_id(&self) -> String {
        self.request_counter
            .fetch_add(1, Ordering::SeqCst)
            .to_string()
    }

    async fn post_json(
        &self,
        payload: &impl serde::Serialize,
        timeout: Duration,
    ) -> Result<(), McpError> {
        let endpoint = self
            .post_endpoint
            .read()
            .await
            .clone()
            .ok_or_else(|| McpError::NotConnected(self.server_name.clone()))?;
        let mut request = self.client.post(endpoint).timeout(timeout).json(payload);
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| McpError::Transport(format!("SSE POST request failed: {error}")))?;
        if !response.status().is_success() {
            return Err(McpError::Transport(format!(
                "SSE POST endpoint returned status {}",
                response.status()
            )));
        }
        Ok(())
    }
}

fn sse_frame_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .or_else(|| buffer.windows(4).position(|window| window == b"\r\n\r\n"))
}

fn sse_delimiter_len(buffer: &[u8]) -> usize {
    if buffer.starts_with(b"\r\n\r\n") {
        4
    } else {
        2
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send_request(
        &self,
        mut request: JsonRpcRequest,
        timeout: Duration,
    ) -> Result<JsonRpcResponse, McpError> {
        if !self.is_alive() {
            return Err(McpError::NotConnected(self.server_name.clone()));
        }
        if request.id.is_null() {
            request.id = serde_json::Value::String(self.next_request_id());
        }
        let id = json_rpc_id(&request.id);
        let (sender, receiver) = oneshot::channel();
        self.pending_requests
            .write()
            .await
            .insert(id.clone(), sender);

        if let Err(error) = self.post_json(&request, timeout).await {
            self.pending_requests.write().await.remove(&id);
            return Err(error);
        }

        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(McpError::ProcessTerminated(format!(
                "SSE stream for '{}' terminated while waiting for response",
                self.server_name
            ))),
            Err(_) => {
                self.pending_requests.write().await.remove(&id);
                Err(McpError::Timeout(self.server_name.clone(), timeout))
            }
        }
    }

    async fn send_notification(&self, notification: JsonRpcNotification) -> Result<(), McpError> {
        if !self.is_alive() {
            return Err(McpError::NotConnected(self.server_name.clone()));
        }
        self.post_json(&notification, Duration::from_secs(30)).await
    }

    fn is_alive(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    async fn close(&self) -> Result<(), McpError> {
        self.is_running.store(false, Ordering::SeqCst);
        if let Some(task) = self.reader_task.lock().await.take() {
            task.abort();
        }
        self.pending_requests.write().await.clear();
        Ok(())
    }
}

/// HTTP JSON-RPC 2.0 Transport for remote MCP servers.
pub struct HttpTransport {
    server_name: String,
    endpoint_url: String,
    client: reqwest::Client,
    headers: HashMap<String, String>,
    is_running: Arc<AtomicBool>,
    request_counter: AtomicU64,
}

impl HttpTransport {
    pub fn new(
        server_name: impl Into<String>,
        endpoint_url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            endpoint_url: endpoint_url.into(),
            client: reqwest::Client::builder().build().unwrap_or_default(),
            headers,
            is_running: Arc::new(AtomicBool::new(true)),
            request_counter: AtomicU64::new(1),
        }
    }

    pub fn next_request_id(&self) -> String {
        self.request_counter
            .fetch_add(1, Ordering::SeqCst)
            .to_string()
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send_request(
        &self,
        mut request: JsonRpcRequest,
        timeout_dur: Duration,
    ) -> Result<JsonRpcResponse, McpError> {
        if !self.is_alive() {
            return Err(McpError::NotConnected(self.server_name.clone()));
        }

        if request.id.is_null() {
            request.id = serde_json::Value::String(self.next_request_id());
        }

        let mut req_builder = self
            .client
            .post(&self.endpoint_url)
            .timeout(timeout_dur)
            .json(&request);

        for (k, v) in &self.headers {
            req_builder = req_builder.header(k, v);
        }

        let resp = req_builder
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("HTTP request failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(McpError::Transport(format!(
                "HTTP server returned status {}",
                resp.status()
            )));
        }

        let json_rpc_resp: JsonRpcResponse = resp.json().await.map_err(|e| {
            McpError::Transport(format!("Failed to parse HTTP JSON-RPC response: {e}"))
        })?;

        Ok(json_rpc_resp)
    }

    async fn send_notification(&self, notification: JsonRpcNotification) -> Result<(), McpError> {
        let mut req_builder = self.client.post(&self.endpoint_url).json(&notification);

        for (k, v) in &self.headers {
            req_builder = req_builder.header(k, v);
        }

        let _ = req_builder.send().await;
        Ok(())
    }

    fn is_alive(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    async fn close(&self) -> Result<(), McpError> {
        self.is_running.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_sse_event_framing() {
        let event = parse_sse_event("event: message\ndata: {\"id\":\"1\"}\ndata: tail\n")
            .expect("parse SSE event")
            .expect("event data");
        assert_eq!(event.event_type, "message");
        assert_eq!(event.data, "{\"id\":\"1\"}\ntail");
    }

    #[test]
    fn extracts_legacy_mcp_post_endpoint() {
        let event = parse_sse_event("event: endpoint\ndata: https://example.com/messages\n")
            .expect("parse endpoint event")
            .expect("endpoint event data");
        assert_eq!(
            endpoint_from_sse_event(&event).expect("extract endpoint"),
            Some("https://example.com/messages".to_string())
        );
    }

    #[test]
    fn parses_json_rpc_message_event() {
        let event =
            parse_sse_event("event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{}}\n")
                .expect("parse message event")
                .expect("message data");
        let response = response_from_sse_event(&event)
            .expect("parse JSON-RPC response")
            .expect("message response");
        assert_eq!(response.id, serde_json::json!(7));
    }

    #[tokio::test]
    async fn routes_sse_responses_by_request_id() {
        let pending = RwLock::new(HashMap::new());
        let (sender, receiver) = oneshot::channel();
        pending
            .write()
            .await
            .insert("request-2".to_string(), sender);
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!("request-2"),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        };

        assert!(route_sse_response(&pending, response).await);
        assert_eq!(
            receiver.await.expect("routed response").result,
            Some(serde_json::json!({"ok": true}))
        );
    }

    #[test]
    fn rejects_malformed_sse_and_json_rpc_data() {
        assert!(parse_sse_event("not-a-field\n").is_err());
        let event = parse_sse_event("event: message\ndata: not-json\n")
            .expect("parse SSE event")
            .expect("event data");
        assert!(response_from_sse_event(&event).is_err());
    }

    #[tokio::test]
    async fn close_unblocks_pending_sse_requests() {
        let pending_requests = Arc::new(RwLock::new(HashMap::new()));
        let (sender, receiver) = oneshot::channel();
        pending_requests
            .write()
            .await
            .insert("request-3".to_string(), sender);
        let transport = SseTransport {
            server_name: "test".to_string(),
            client: reqwest::Client::new(),
            headers: HashMap::new(),
            post_endpoint: Arc::new(RwLock::new(None)),
            pending_requests,
            is_running: Arc::new(AtomicBool::new(true)),
            request_counter: AtomicU64::new(1),
            reader_task: Mutex::new(None),
        };

        transport.close().await.expect("close transport");
        assert!(receiver.await.is_err());
        assert!(!transport.is_alive());
    }
}
