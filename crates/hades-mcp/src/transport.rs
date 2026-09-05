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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event_type: String,
    pub data: String,
}

pub fn parse_sse_event(frame: &str) -> Result<Option<SseEvent>, McpError> {
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

/// Finds the boundary and delimiter length of the next complete SSE frame in a byte buffer.
pub fn next_sse_frame(buffer: &[u8]) -> Option<(usize, usize)> {
    let rn_pos = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let n_pos = buffer.windows(2).position(|window| window == b"\n\n");

    match (rn_pos, n_pos) {
        (Some(rn), Some(n)) => {
            if rn <= n {
                Some((rn, 4))
            } else {
                Some((n, 2))
            }
        }
        (Some(rn), None) => Some((rn, 4)),
        (None, Some(n)) => Some((n, 2)),
        (None, None) => None,
    }
}

/// Parses a JSON-RPC response from event data, matching against `target_id`.
pub fn parse_json_rpc_response_from_data(data: &str, target_id: &str) -> Option<JsonRpcResponse> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
        let resp_id = json_rpc_id(&resp.id);
        if resp_id == target_id || (resp.error.is_some() && resp.id.is_null()) {
            return Some(resp);
        }
    }

    if let Ok(batch) = serde_json::from_str::<Vec<JsonRpcResponse>>(trimmed) {
        for resp in batch {
            let resp_id = json_rpc_id(&resp.id);
            if resp_id == target_id || (resp.error.is_some() && resp.id.is_null()) {
                return Some(resp);
            }
        }
    }

    None
}

pub fn endpoint_from_sse_event(event: &SseEvent) -> Result<Option<String>, McpError> {
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

pub fn response_from_sse_event(event: &SseEvent) -> Result<Option<JsonRpcResponse>, McpError> {
    if event.event_type != "message" {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&event.data)?))
}

pub async fn route_sse_response(
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

pub fn json_rpc_id(id: &serde_json::Value) -> String {
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

                while let Some((end, delim_len)) = next_sse_frame(&buffer) {
                    let frame: Vec<u8> = buffer.drain(..end).collect();
                    buffer.drain(..delim_len);
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

/// HTTP / Streamable HTTP Transport for remote MCP servers.
///
/// Implements the modern MCP Streamable HTTP specification.
/// Supports both `application/json` (direct JSON-RPC responses) and
/// `text/event-stream` (SSE-framed JSON-RPC responses), streaming chunked
/// frames, session ID tracking (`mcp-session-id`), and request ID routing.
pub struct HttpTransport {
    server_name: String,
    endpoint_url: String,
    client: reqwest::Client,
    headers: HashMap<String, String>,
    session_id: Arc<RwLock<Option<String>>>,
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
            session_id: Arc::new(RwLock::new(None)),
            is_running: Arc::new(AtomicBool::new(true)),
            request_counter: AtomicU64::new(1),
        }
    }

    /// Allocates an incremental request ID.
    pub fn next_request_id(&self) -> String {
        self.request_counter
            .fetch_add(1, Ordering::SeqCst)
            .to_string()
    }

    /// Returns the active session ID, if negotiated.
    pub async fn session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Explicitly sets or overrides the session ID.
    pub async fn set_session_id(&self, id: Option<String>) {
        *self.session_id.write().await = id;
    }

    async fn decode_json_response(
        resp: reqwest::Response,
        target_id: &str,
        server_name: &str,
    ) -> Result<JsonRpcResponse, McpError> {
        let body_bytes = resp.bytes().await.map_err(|e| {
            McpError::Transport(format!(
                "Failed to read response body from '{server_name}': {e}"
            ))
        })?;

        if let Ok(json_rpc_resp) = serde_json::from_slice::<JsonRpcResponse>(&body_bytes) {
            let resp_id = json_rpc_id(&json_rpc_resp.id);
            if resp_id == target_id || (json_rpc_resp.error.is_some() && json_rpc_resp.id.is_null())
            {
                return Ok(json_rpc_resp);
            }
        }

        if let Ok(batch) = serde_json::from_slice::<Vec<JsonRpcResponse>>(&body_bytes) {
            for resp in batch {
                let resp_id = json_rpc_id(&resp.id);
                if resp_id == target_id || (resp.error.is_some() && resp.id.is_null()) {
                    return Ok(resp);
                }
            }
        }

        let body_str = String::from_utf8_lossy(&body_bytes);
        if body_str.contains("event:") || body_str.contains("data:") {
            if let Some(resp) = Self::decode_sse_from_text(&body_str, target_id) {
                return Ok(resp);
            }
        }

        let preview = if body_str.len() > 200 {
            format!("{}...", &body_str[..200])
        } else {
            body_str.to_string()
        };
        Err(McpError::Transport(format!(
            "Failed to parse HTTP JSON-RPC response from server '{server_name}': {preview}"
        )))
    }

    fn decode_inferred_response(
        bytes: &[u8],
        target_id: &str,
        server_name: &str,
    ) -> Result<JsonRpcResponse, McpError> {
        if let Ok(json_rpc_resp) = serde_json::from_slice::<JsonRpcResponse>(bytes) {
            let resp_id = json_rpc_id(&json_rpc_resp.id);
            if resp_id == target_id || (json_rpc_resp.error.is_some() && json_rpc_resp.id.is_null())
            {
                return Ok(json_rpc_resp);
            }
        }

        if let Ok(batch) = serde_json::from_slice::<Vec<JsonRpcResponse>>(bytes) {
            for resp in batch {
                let resp_id = json_rpc_id(&resp.id);
                if resp_id == target_id || (resp.error.is_some() && resp.id.is_null()) {
                    return Ok(resp);
                }
            }
        }

        let text = String::from_utf8_lossy(bytes);
        if let Some(resp) = Self::decode_sse_from_text(&text, target_id) {
            return Ok(resp);
        }

        Err(McpError::Transport(format!(
            "Failed to parse response body from server '{server_name}'"
        )))
    }

    pub fn decode_sse_from_text(text: &str, target_id: &str) -> Option<JsonRpcResponse> {
        let mut buffer = text.as_bytes().to_vec();
        while let Some((end, delim_len)) = next_sse_frame(&buffer) {
            let frame_bytes: Vec<u8> = buffer.drain(..end).collect();
            buffer.drain(..delim_len);
            if let Ok(frame_str) = String::from_utf8(frame_bytes) {
                if let Ok(Some(event)) = parse_sse_event(&frame_str) {
                    if event.event_type == "message" || event.event_type.is_empty() {
                        if let Some(resp) =
                            parse_json_rpc_response_from_data(&event.data, target_id)
                        {
                            return Some(resp);
                        }
                    }
                }
            }
        }
        if !buffer.is_empty() {
            if let Ok(frame_str) = String::from_utf8(buffer) {
                if let Ok(Some(event)) = parse_sse_event(&frame_str) {
                    if event.event_type == "message" || event.event_type.is_empty() {
                        if let Some(resp) =
                            parse_json_rpc_response_from_data(&event.data, target_id)
                        {
                            return Some(resp);
                        }
                    }
                }
            }
        }
        None
    }

    async fn decode_sse_response_stream<S, B, E>(
        mut stream: S,
        target_id: &str,
        server_name: &str,
    ) -> Result<JsonRpcResponse, McpError>
    where
        S: futures::Stream<Item = Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::fmt::Display,
    {
        let mut buffer = Vec::new();

        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res.map_err(|e| {
                McpError::Transport(format!(
                    "Error reading SSE response stream from '{server_name}': {e}"
                ))
            })?;
            buffer.extend_from_slice(chunk.as_ref());

            while let Some((end, delim_len)) = next_sse_frame(&buffer) {
                let frame_bytes: Vec<u8> = buffer.drain(..end).collect();
                buffer.drain(..delim_len);

                let frame_str = match String::from_utf8(frame_bytes) {
                    Ok(s) => s,
                    Err(e) => {
                        debug!(server = %server_name, error = %e, "Invalid UTF-8 in SSE frame");
                        continue;
                    }
                };

                let event = match parse_sse_event(&frame_str) {
                    Ok(Some(ev)) => ev,
                    Ok(None) => continue,
                    Err(e) => {
                        debug!(server = %server_name, error = %e, "Malformed SSE frame");
                        continue;
                    }
                };

                if event.event_type == "message" || event.event_type.is_empty() {
                    if let Some(resp) = parse_json_rpc_response_from_data(&event.data, target_id) {
                        return Ok(resp);
                    }
                }
            }
        }

        if !buffer.is_empty() {
            if let Ok(frame_str) = String::from_utf8(buffer) {
                if let Ok(Some(event)) = parse_sse_event(&frame_str) {
                    if event.event_type == "message" || event.event_type.is_empty() {
                        if let Some(resp) =
                            parse_json_rpc_response_from_data(&event.data, target_id)
                        {
                            return Ok(resp);
                        }
                    }
                }
            }
        }

        Err(McpError::Protocol(format!(
            "No matching JSON-RPC response found for request ID '{target_id}' in SSE stream from server '{server_name}'"
        )))
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
        let target_id = json_rpc_id(&request.id);

        let mut req_builder = self
            .client
            .post(&self.endpoint_url)
            .timeout(timeout_dur)
            .header("Accept", "application/json, text/event-stream")
            .json(&request);

        for (k, v) in &self.headers {
            req_builder = req_builder.header(k, v);
        }

        if let Some(session_id) = self.session_id.read().await.as_ref() {
            req_builder = req_builder.header("mcp-session-id", session_id);
        }

        let resp = req_builder
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("HTTP request failed: {e}")))?;

        if let Some(session_header) = resp.headers().get("mcp-session-id") {
            if let Ok(val) = session_header.to_str() {
                let mut sid = self.session_id.write().await;
                *sid = Some(val.to_string());
            }
        }

        let status = resp.status();
        if !status.is_success() {
            let error_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(McpError::Transport(format!(
                "HTTP server returned status {status}: {error_text}"
            )));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        if content_type.contains("text/event-stream") {
            Self::decode_sse_response_stream(resp.bytes_stream(), &target_id, &self.server_name)
                .await
        } else if content_type.contains("json") {
            Self::decode_json_response(resp, &target_id, &self.server_name).await
        } else if content_type.is_empty() {
            let bytes = resp.bytes().await.map_err(|e| {
                McpError::Transport(format!(
                    "Failed to read response body from '{}': {e}",
                    self.server_name
                ))
            })?;
            Self::decode_inferred_response(&bytes, &target_id, &self.server_name)
        } else {
            Err(McpError::Transport(format!(
                "Unexpected Content-Type '{content_type}' from MCP server '{}'",
                self.server_name
            )))
        }
    }

    async fn send_notification(&self, notification: JsonRpcNotification) -> Result<(), McpError> {
        if !self.is_alive() {
            return Err(McpError::NotConnected(self.server_name.clone()));
        }

        let mut req_builder = self
            .client
            .post(&self.endpoint_url)
            .header("Accept", "application/json, text/event-stream")
            .json(&notification);

        for (k, v) in &self.headers {
            req_builder = req_builder.header(k, v);
        }

        if let Some(session_id) = self.session_id.read().await.as_ref() {
            req_builder = req_builder.header("mcp-session-id", session_id);
        }

        let resp = req_builder
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("Failed to send notification: {e}")))?;

        if let Some(session_header) = resp.headers().get("mcp-session-id") {
            if let Ok(val) = session_header.to_str() {
                let mut sid = self.session_id.write().await;
                *sid = Some(val.to_string());
            }
        }

        Ok(())
    }

    fn is_alive(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    async fn close(&self) -> Result<(), McpError> {
        self.is_running.store(false, Ordering::SeqCst);
        *self.session_id.write().await = None;
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

    // ──────────────────────────────────────────────────────────────────────────
    // Streamable HTTP Regression Tests (Points 1 - 11)
    // ──────────────────────────────────────────────────────────────────────────

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn spawn_mock_mcp_server<F>(handler: F) -> (String, oneshot::Sender<()>)
    where
        F: Fn(String, String) -> (u16, Vec<(&'static str, String)>, Vec<u8>)
            + Send
            + Sync
            + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let handler = Arc::new(handler);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    Ok((mut socket, _)) = listener.accept() => {
                        let handler_clone = handler.clone();
                        tokio::spawn(async move {
                            let mut buf = vec![0u8; 8192];
                            let mut total = Vec::new();
                            loop {
                                let n = match socket.read(&mut buf).await {
                                    Ok(0) => break,
                                    Ok(n) => n,
                                    Err(_) => break,
                                };
                                total.extend_from_slice(&buf[..n]);
                                if let Some(pos) = total.windows(4).position(|w| w == b"\r\n\r\n") {
                                    let header_bytes = &total[..pos];
                                    let headers_str = String::from_utf8_lossy(header_bytes).to_string();
                                    let mut content_len = 0;
                                    for line in headers_str.lines() {
                                        if let Some((k, v)) = line.split_once(':') {
                                            if k.trim().eq_ignore_ascii_case("content-length") {
                                                content_len = v.trim().parse().unwrap_or(0);
                                            }
                                        }
                                    }
                                    let body_start = pos + 4;
                                    let body_bytes = &total[body_start..];
                                    if body_bytes.len() >= content_len {
                                        let body_str = String::from_utf8_lossy(&body_bytes[..content_len]).to_string();
                                        let (status, resp_headers, resp_body) = handler_clone(headers_str, body_str);
                                        let status_text = match status {
                                            200 => "200 OK",
                                            400 => "400 Bad Request",
                                            401 => "401 Unauthorized",
                                            404 => "404 Not Found",
                                            500 => "500 Internal Server Error",
                                            _ => "200 OK",
                                        };
                                        let mut response = format!(
                                            "HTTP/1.1 {status_text}\r\nContent-Length: {}\r\nConnection: close\r\n",
                                            resp_body.len()
                                        );
                                        for (hk, hv) in resp_headers {
                                            response.push_str(&format!("{hk}: {hv}\r\n"));
                                        }
                                        response.push_str("\r\n");
                                        let _ = socket.write_all(response.as_bytes()).await;
                                        let _ = socket.write_all(&resp_body).await;
                                        let _ = socket.flush().await;
                                        break;
                                    }
                                }
                            }
                        });
                    }
                }
            }
        });

        (format!("http://127.0.0.1:{}", addr.port()), shutdown_tx)
    }

    async fn spawn_chunked_sse_mock_server(chunks: Vec<Vec<u8>>) -> (String, oneshot::Sender<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            tokio::select! {
                _ = &mut shutdown_rx => {},
                Ok((mut socket, _)) = listener.accept() => {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(header.as_bytes()).await;
                    for c in chunks {
                        let chunk_hdr = format!("{:X}\r\n", c.len());
                        let _ = socket.write_all(chunk_hdr.as_bytes()).await;
                        let _ = socket.write_all(&c).await;
                        let _ = socket.write_all(b"\r\n").await;
                        let _ = socket.flush().await;
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    let _ = socket.write_all(b"0\r\n\r\n").await;
                    let _ = socket.flush().await;
                }
            }
        });

        (format!("http://127.0.0.1:{}", addr.port()), shutdown_tx)
    }

    #[tokio::test]
    async fn test_1_application_json_mcp_response_parsing() {
        let (url, _shutdown) = spawn_mock_mcp_server(|_, _| {
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": "req-1",
                "result": { "status": "json_ok" }
            });
            (
                200,
                vec![("Content-Type", "application/json".to_string())],
                serde_json::to_vec(&body).unwrap(),
            )
        })
        .await;

        let transport = HttpTransport::new("test", url, HashMap::new());
        let req = JsonRpcRequest::new("req-1", "ping", None);
        let resp = transport
            .send_request(req, Duration::from_secs(5))
            .await
            .expect("send request");

        assert_eq!(resp.id, serde_json::json!("req-1"));
        assert_eq!(
            resp.result,
            Some(serde_json::json!({ "status": "json_ok" }))
        );
    }

    #[tokio::test]
    async fn test_2_text_event_stream_mcp_response_parsing() {
        let (url, _shutdown) = spawn_mock_mcp_server(|_, _| {
            let sse_body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"req-2\",\"result\":{\"status\":\"sse_ok\"}}\n\n";
            (
                200,
                vec![("Content-Type", "text/event-stream".to_string())],
                sse_body.as_bytes().to_vec(),
            )
        })
        .await;

        let transport = HttpTransport::new("test", url, HashMap::new());
        let req = JsonRpcRequest::new("req-2", "ping", None);
        let resp = transport
            .send_request(req, Duration::from_secs(5))
            .await
            .expect("send request over SSE");

        assert_eq!(resp.id, serde_json::json!("req-2"));
        assert_eq!(resp.result, Some(serde_json::json!({ "status": "sse_ok" })));
    }

    #[tokio::test]
    async fn test_3_multiple_sse_events_in_response() {
        let (url, _shutdown) = spawn_mock_mcp_server(|_, _| {
            let sse_body = concat!(
                ": keepalive comment\n\n",
                "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n",
                "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"req-other\",\"result\":{\"other\":true}}\n\n",
                "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"req-target\",\"result\":{\"target\":true}}\n\n"
            );
            (
                200,
                vec![("Content-Type", "text/event-stream".to_string())],
                sse_body.as_bytes().to_vec(),
            )
        })
        .await;

        let transport = HttpTransport::new("test", url, HashMap::new());
        let req = JsonRpcRequest::new("req-target", "ping", None);
        let resp = transport
            .send_request(req, Duration::from_secs(5))
            .await
            .expect("parse target event");

        assert_eq!(resp.id, serde_json::json!("req-target"));
        assert_eq!(resp.result, Some(serde_json::json!({ "target": true })));
    }

    #[tokio::test]
    async fn test_4_partial_chunked_sse_frames() {
        let chunks = vec![
            b"event: mess".to_vec(),
            b"age\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"chunk-1\",".to_vec(),
            b"\"result\":{\"chunked\":true}}\r\n\r\n".to_vec(),
        ];
        let (url, _shutdown) = spawn_chunked_sse_mock_server(chunks).await;

        let transport = HttpTransport::new("test", url, HashMap::new());
        let req = JsonRpcRequest::new("chunk-1", "ping", None);
        let resp = transport
            .send_request(req, Duration::from_secs(5))
            .await
            .expect("parse chunked SSE");

        assert_eq!(resp.id, serde_json::json!("chunk-1"));
        assert_eq!(resp.result, Some(serde_json::json!({ "chunked": true })));
    }

    #[test]
    fn test_5_correct_json_rpc_request_id_routing() {
        let data_single = r#"{"jsonrpc":"2.0","id":42,"result":{"answer":42}}"#;
        let matched = parse_json_rpc_response_from_data(data_single, "42");
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().id, serde_json::json!(42));

        let unmatched = parse_json_rpc_response_from_data(data_single, "99");
        assert!(unmatched.is_none());

        let data_batch = r#"[
            {"jsonrpc":"2.0","id":"a","result":{"val":"A"}},
            {"jsonrpc":"2.0","id":"b","result":{"val":"B"}}
        ]"#;
        let matched_b = parse_json_rpc_response_from_data(data_batch, "b");
        assert!(matched_b.is_some());
        assert_eq!(
            matched_b.unwrap().result,
            Some(serde_json::json!({ "val": "B" }))
        );
    }

    #[tokio::test]
    async fn test_6_mcp_initialize_over_streamable_http() {
        let (url, _shutdown) = spawn_mock_mcp_server(|_, _| {
            let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"1\",\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"serverInfo\":{\"name\":\"mock-server\",\"version\":\"1.0.0\"}}}\n\n";
            (
                200,
                vec![("Content-Type", "text/event-stream".to_string())],
                body.as_bytes().to_vec(),
            )
        })
        .await;

        let transport = Arc::new(HttpTransport::new("test", url, HashMap::new()));
        let client = crate::client::McpClient::new("test", transport, Duration::from_secs(5));

        let init_result = client.initialize().await.expect("initialize client");
        assert_eq!(init_result.server_info.name, "mock-server");
        assert_eq!(client.state().await, crate::client::McpServerState::Ready);
    }

    #[tokio::test]
    async fn test_7_tools_list_over_streamable_http() {
        let (url, _shutdown) = spawn_mock_mcp_server(|_, body| {
            let id_str = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("id").cloned())
                .map(|id| match id {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Number(n) => n.to_string(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| "1".to_string());

            if body.contains("\"method\":\"initialize\"") {
                let init = format!("event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":\"{id_str}\",\"result\":{{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{{}},\"serverInfo\":{{\"name\":\"mock-server\",\"version\":\"1.0.0\"}}}}}}\n\n");
                (
                    200,
                    vec![("Content-Type", "text/event-stream".to_string())],
                    init.as_bytes().to_vec(),
                )
            } else if body.contains("\"method\":\"tools/list\"") {
                let tools = format!("event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":\"{id_str}\",\"result\":{{\"tools\":[{{\"name\":\"test_tool\",\"description\":\"A test tool\",\"inputSchema\":{{\"type\":\"object\"}}}}]}}}}\n\n");
                (
                    200,
                    vec![("Content-Type", "text/event-stream".to_string())],
                    tools.as_bytes().to_vec(),
                )
            } else {
                // notification
                (200, vec![], vec![])
            }
        })
        .await;

        let transport = Arc::new(HttpTransport::new("test", url, HashMap::new()));
        let client = crate::client::McpClient::new("test", transport, Duration::from_secs(5));
        client.initialize().await.expect("initialize client");

        let tools = client.list_tools().await.expect("list tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
    }

    #[tokio::test]
    async fn test_8_authentication_header_generation() {
        let (url, _shutdown) = spawn_mock_mcp_server(|headers, _| {
            let has_auth = headers
                .to_lowercase()
                .contains("authorization: bearer test-secret-pat");
            if has_auth {
                let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"auth-1\",\"result\":{\"authorized\":true}}\n\n";
                (
                    200,
                    vec![("Content-Type", "text/event-stream".to_string())],
                    body.as_bytes().to_vec(),
                )
            } else {
                (401, vec![], b"Unauthorized".to_vec())
            }
        })
        .await;

        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            "Bearer test-secret-pat".to_string(),
        );
        let transport = HttpTransport::new("test", url, headers);

        let req = JsonRpcRequest::new("auth-1", "ping", None);
        let resp = transport
            .send_request(req, Duration::from_secs(5))
            .await
            .expect("authenticated request");

        assert_eq!(resp.result, Some(serde_json::json!({ "authorized": true })));
    }

    #[tokio::test]
    async fn test_9_invalid_json_response_handling() {
        let (url, _shutdown) = spawn_mock_mcp_server(|_, _| {
            (
                200,
                vec![("Content-Type", "application/json".to_string())],
                b"invalid-json-body".to_vec(),
            )
        })
        .await;

        let transport = HttpTransport::new("test", url, HashMap::new());
        let req = JsonRpcRequest::new("1", "ping", None);
        let res = transport.send_request(req, Duration::from_secs(5)).await;

        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to parse HTTP JSON-RPC response"));
    }

    #[tokio::test]
    async fn test_10_unexpected_content_type_handling() {
        let (url, _shutdown) = spawn_mock_mcp_server(|_, _| {
            (
                200,
                vec![("Content-Type", "text/html".to_string())],
                b"<html><body>502 Bad Gateway</body></html>".to_vec(),
            )
        })
        .await;

        let transport = HttpTransport::new("test", url, HashMap::new());
        let req = JsonRpcRequest::new("1", "ping", None);
        let res = transport.send_request(req, Duration::from_secs(5)).await;

        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("Unexpected Content-Type 'text/html'"));
    }

    #[tokio::test]
    async fn test_11_github_style_remote_mcp_response_behavior() {
        let (url, _shutdown) = spawn_mock_mcp_server(|headers, body| {
            if body.contains("\"method\":\"initialize\"") {
                let init = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"1\",\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"serverInfo\":{\"name\":\"github-mcp-server\",\"version\":\"remote-1.0\"}}}\n\n";
                (
                    200,
                    vec![
                        ("Content-Type", "text/event-stream".to_string()),
                        ("mcp-session-id", "gh-session-token-999".to_string()),
                    ],
                    init.as_bytes().to_vec(),
                )
            } else {
                let has_session_hdr = headers
                    .to_lowercase()
                    .contains("mcp-session-id: gh-session-token-999");
                let resp = format!(
                    "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":\"2\",\"result\":{{\"session_verified\":{has_session_hdr}}}}}\n\n"
                );
                (
                    200,
                    vec![("Content-Type", "text/event-stream".to_string())],
                    resp.as_bytes().to_vec(),
                )
            }
        })
        .await;

        let transport = Arc::new(HttpTransport::new("github", url, HashMap::new()));
        let client =
            crate::client::McpClient::new("github", transport.clone(), Duration::from_secs(5));

        // 1. Initialize
        client.initialize().await.expect("initialize github mcp");
        assert_eq!(
            transport.session_id().await,
            Some("gh-session-token-999".to_string())
        );

        // 2. Subsequent request
        let req2 = JsonRpcRequest::new("2", "custom/check", None);
        let resp2 = transport
            .send_request(req2, Duration::from_secs(5))
            .await
            .expect("subsequent request with session id");

        assert_eq!(
            resp2.result,
            Some(serde_json::json!({ "session_verified": true }))
        );
    }

    #[tokio::test]
    #[ignore = "Live integration test requiring GITHUB_TOKEN or GH_TOKEN"]
    async fn test_live_github_remote_mcp_server() {
        let token = std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .unwrap_or_default();
        if token.is_empty() {
            println!("Skipping live GitHub MCP test: GITHUB_TOKEN / GH_TOKEN not set");
            return;
        }

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {}", token));

        let transport = Arc::new(HttpTransport::new(
            "github",
            "https://api.githubcopilot.com/mcp/",
            headers,
        ));
        let client =
            crate::client::McpClient::new("github", transport.clone(), Duration::from_secs(30));

        // 1. Initialize
        let init_result = client.initialize().await.expect("Live initialize failed");
        assert_eq!(init_result.server_info.name, "github-mcp-server");
        assert!(transport.session_id().await.is_some());

        // 2. List tools
        let tools = client.list_tools().await.expect("Live list_tools failed");
        assert!(!tools.is_empty(), "Tools list should not be empty");
        let has_list_issues = tools.iter().any(|t| t.name == "list_issues");
        assert!(has_list_issues, "list_issues tool should be present");

        // 3. Call tool (list_issues for PareekshithPalat/HADES_CLI)
        let call_res = client
            .call_tool(
                "list_issues",
                serde_json::json!({
                    "owner": "PareekshithPalat",
                    "repo": "HADES_CLI"
                }),
            )
            .await
            .expect("Live call_tool failed");

        assert!(
            !call_res.content.is_empty(),
            "Tool call should return non-empty content"
        );
    }
}
