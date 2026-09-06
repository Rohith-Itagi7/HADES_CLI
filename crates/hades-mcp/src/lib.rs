pub mod client;
pub mod error;
pub mod manager;
pub mod protocol;
pub mod server;
pub mod tool_adapter;
pub mod transport;

pub use client::{McpClient, McpServerState};
pub use error::McpError;
pub use manager::{McpServerManager, McpServerSummary};
pub use protocol::{
    CallToolParams, CallToolResult, InitializeParams, InitializeResult, JsonRpcError,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, ListPromptsResult, ListResourcesResult,
    ListToolsResult, McpContent, McpPrompt, McpResource, McpResourceContents, McpToolDefinition,
    ReadResourceResult, ServerCapabilities, LATEST_PROTOCOL_VERSION,
};
pub use server::HadesMcpServer;
pub use tool_adapter::McpToolAdapter;
pub use transport::{HttpTransport, McpTransport, SseTransport, StdioTransport};

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use hades_tools::{RiskLevel, Tool, ToolContext};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    /// Mock in-memory MCP transport for automated testing without spawning child processes.
    struct MockMcpTransport {
        responses: Mutex<HashMap<String, JsonRpcResponse>>,
    }

    impl MockMcpTransport {
        fn new() -> Self {
            Self {
                responses: Mutex::new(HashMap::new()),
            }
        }

        async fn set_response(&self, method: &str, result: serde_json::Value) {
            let mut resp = self.responses.lock().await;
            resp.insert(
                method.to_string(),
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: serde_json::json!("1"),
                    result: Some(result),
                    error: None,
                },
            );
        }
    }

    #[async_trait]
    impl McpTransport for MockMcpTransport {
        async fn send_request(
            &self,
            request: JsonRpcRequest,
            _timeout: Duration,
        ) -> Result<JsonRpcResponse, McpError> {
            let resp = self.responses.lock().await;
            if let Some(r) = resp.get(&request.method) {
                let mut cloned = r.clone();
                cloned.id = request.id;
                Ok(cloned)
            } else {
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Method '{}' not mocked", request.method),
                        data: None,
                    }),
                })
            }
        }

        async fn send_notification(
            &self,
            _notification: JsonRpcNotification,
        ) -> Result<(), McpError> {
            Ok(())
        }

        fn is_alive(&self) -> bool {
            true
        }

        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_mcp_client_initialization_and_ping() {
        let mock = Arc::new(MockMcpTransport::new());
        mock.set_response(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": { "listChanged": true },
                    "resources": { "subscribe": true }
                },
                "serverInfo": {
                    "name": "test-mock-server",
                    "version": "1.0.0"
                }
            }),
        )
        .await;

        mock.set_response("ping", serde_json::json!({})).await;

        let client = McpClient::new("test-server", mock, Duration::from_secs(5));
        assert_eq!(client.state().await, McpServerState::Configured);

        let init = client.initialize().await.expect("initialize succeeds");
        assert_eq!(init.protocol_version, "2024-11-05");
        assert_eq!(init.server_info.name, "test-mock-server");
        assert_eq!(client.state().await, McpServerState::Ready);

        let ping_dur = client.ping().await.expect("ping succeeds");
        assert!(ping_dur.as_millis() < 500);
    }

    #[tokio::test]
    async fn test_upserted_server_config_is_immediately_startable() {
        let manager = McpServerManager::new(".");
        manager
            .upsert_server_config("new-server", hades_config::McpServerConfig::default())
            .await;

        let summaries = manager.list_server_summaries().await;
        assert!(summaries.iter().any(|summary| summary.name == "new-server"));

        let error = match manager.start_server("new-server", None).await {
            Ok(_) => panic!("configured server without a command should fail startup validation"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("missing 'command'"));
    }

    #[tokio::test]
    async fn test_removed_server_config_is_no_longer_startable() {
        let manager = McpServerManager::new(".");
        manager
            .upsert_server_config("removed-server", hades_config::McpServerConfig::default())
            .await;

        assert!(manager
            .remove_server_config("removed-server")
            .await
            .is_some());
        assert!(manager.list_server_summaries().await.is_empty());

        let error = match manager.start_server("removed-server", None).await {
            Ok(_) => panic!("removed server should not be startable"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("is not configured"));
    }

    #[tokio::test]
    async fn test_mcp_tool_discovery_and_adapter_execution() {
        let mock = Arc::new(MockMcpTransport::new());
        mock.set_response(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "github-mcp", "version": "0.1.0" }
            }),
        )
        .await;

        mock.set_response(
            "tools/list",
            serde_json::json!({
                "tools": [
                    {
                        "name": "search_repositories",
                        "description": "Search public repositories on GitHub",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string" }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "create_issue",
                        "description": "Create a new issue on GitHub repository",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" }
                            }
                        }
                    }
                ]
            }),
        )
        .await;

        mock.set_response(
            "tools/call",
            serde_json::json!({
                "content": [
                    { "type": "text", "text": "Found 3 matching repositories." }
                ],
                "isError": false
            }),
        )
        .await;

        let client = Arc::new(McpClient::new("github", mock, Duration::from_secs(5)));
        client.initialize().await.expect("client init");

        let tools = client.list_tools().await.expect("list tools");
        assert_eq!(tools.len(), 2);

        // Test search_repositories adapter (read-only -> Low risk)
        let search_tool = McpToolAdapter::new("github", tools[0].clone(), client.clone());
        assert_eq!(search_tool.definition().name, "github.search_repositories");
        assert_eq!(search_tool.definition().risk_level, RiskLevel::Low);
        assert!(!search_tool.definition().is_mutating);

        // Test create_issue adapter (mutating -> High risk)
        let create_tool = McpToolAdapter::new("github", tools[1].clone(), client.clone());
        assert_eq!(create_tool.definition().name, "github.create_issue");
        assert_eq!(create_tool.definition().risk_level, RiskLevel::High);
        assert!(create_tool.definition().is_mutating);

        // Execute search tool via Tool trait
        let context = ToolContext::new("test-session", ".", ".");
        let result = search_tool
            .execute("call-1", serde_json::json!({ "query": "hades" }), &context)
            .await;

        assert_eq!(result.status, hades_tools::ToolStatus::Success);
        assert_eq!(result.output, "Found 3 matching repositories.");
    }

    #[tokio::test]
    async fn test_mcp_resources_and_prompts_discovery() {
        let mock = Arc::new(MockMcpTransport::new());
        mock.set_response(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "resources": {}, "prompts": {} },
                "serverInfo": { "name": "docs-mcp", "version": "1.0" }
            }),
        )
        .await;

        mock.set_response(
            "resources/list",
            serde_json::json!({
                "resources": [
                    {
                        "uri": "docs://hades/architecture",
                        "name": "Hades Architecture Guide",
                        "mimeType": "text/markdown"
                    }
                ]
            }),
        )
        .await;

        mock.set_response(
            "resources/read",
            serde_json::json!({
                "contents": [
                    {
                        "uri": "docs://hades/architecture",
                        "text": "# Hades Architecture Overview"
                    }
                ]
            }),
        )
        .await;

        mock.set_response(
            "prompts/list",
            serde_json::json!({
                "prompts": [
                    {
                        "name": "code_review",
                        "description": "Perform comprehensive code review",
                        "arguments": [
                            { "name": "path", "required": true }
                        ]
                    }
                ]
            }),
        )
        .await;

        let client = Arc::new(McpClient::new("docs", mock, Duration::from_secs(5)));
        client.initialize().await.expect("init");

        let resources = client.list_resources().await.expect("list resources");
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].uri, "docs://hades/architecture");

        let read_res = client
            .read_resource("docs://hades/architecture")
            .await
            .expect("read resource");
        assert_eq!(
            read_res.contents[0].as_text(),
            "# Hades Architecture Overview"
        );

        let prompts = client.list_prompts().await.expect("list prompts");
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "code_review");
    }

    #[tokio::test]
    async fn test_hades_mcp_server_request_handling() {
        let server = HadesMcpServer::new(".");

        // Test initialize
        let init_req = JsonRpcRequest::new("1", "initialize", None);
        let init_resp = server.handle_request(init_req).await;
        assert!(init_resp.error.is_none());
        assert!(init_resp.result.is_some());

        // Test tools/list
        let list_req = JsonRpcRequest::new("2", "tools/list", None);
        let list_resp = server.handle_request(list_req).await;
        assert!(list_resp.error.is_none());
        let list_result: ListToolsResult =
            serde_json::from_value(list_resp.result.unwrap()).unwrap();
        assert!(list_result
            .tools
            .iter()
            .any(|t| t.name == "workspace.detect"));
        assert!(list_result.tools.iter().any(|t| t.name == "system.info"));

        // Test tools/call (system.info)
        let call_req = JsonRpcRequest::new(
            "3",
            "tools/call",
            Some(serde_json::json!({
                "name": "system.info",
                "arguments": {}
            })),
        );
        let call_resp = server.handle_request(call_req).await;
        assert!(call_resp.error.is_none());
        let call_result: CallToolResult =
            serde_json::from_value(call_resp.result.unwrap()).unwrap();
        assert!(!call_result.content.is_empty());
    }
}
