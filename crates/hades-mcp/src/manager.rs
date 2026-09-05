use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hades_config::{McpConfig, McpServerConfig, McpTransportType};
use hades_events::EventBus;
use hades_tools::DynTool;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::client::{McpClient, McpServerState};
use crate::error::McpError;
use crate::protocol::{
    GetPromptResult, McpPrompt, McpResource, McpToolDefinition, ReadResourceResult,
};
use crate::tool_adapter::McpToolAdapter;
use crate::transport::{HttpTransport, McpTransport, SseTransport, StdioTransport};

/// High-level diagnostic summary of an MCP server for status inspection and TUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerSummary {
    pub name: String,
    pub state: McpServerState,
    pub transport: String,
    pub tool_count: usize,
    pub resource_count: usize,
    pub prompt_count: usize,
    pub error: Option<String>,
}

/// Central manager orchestrating MCP server lifecycles, transports, tool discovery, and health.
pub struct McpServerManager {
    servers: Arc<RwLock<BTreeMap<String, Arc<McpClient>>>>,
    configs: Arc<RwLock<BTreeMap<String, McpServerConfig>>>,
    discovered_tools: Arc<RwLock<BTreeMap<String, Vec<McpToolDefinition>>>>,
    discovered_resources: Arc<RwLock<BTreeMap<String, Vec<McpResource>>>>,
    discovered_prompts: Arc<RwLock<BTreeMap<String, Vec<McpPrompt>>>>,
    working_dir: PathBuf,
    event_bus: Option<EventBus>,
}

impl Default for McpServerManager {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

impl McpServerManager {
    /// Creates a new MCP server manager bound to the given working directory.
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            servers: Arc::new(RwLock::new(BTreeMap::new())),
            configs: Arc::new(RwLock::new(BTreeMap::new())),
            discovered_tools: Arc::new(RwLock::new(BTreeMap::new())),
            discovered_resources: Arc::new(RwLock::new(BTreeMap::new())),
            discovered_prompts: Arc::new(RwLock::new(BTreeMap::new())),
            working_dir: working_dir.into(),
            event_bus: None,
        }
    }

    /// Associates an event bus for telemetry and audit events.
    pub fn with_event_bus(mut self, event_bus: EventBus) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Loads server configurations from Hades configuration.
    pub async fn load_from_config(&self, config: &McpConfig) {
        if !config.enabled {
            info!("MCP subsystem is disabled in configuration");
            return;
        }

        let mut configs_guard = self.configs.write().await;
        *configs_guard = config.servers.clone();
        info!(
            count = configs_guard.len(),
            "Loaded MCP server configurations"
        );
    }

    /// Adds or updates a server configuration in the running manager.
    pub async fn upsert_server_config(&self, name: impl Into<String>, config: McpServerConfig) {
        self.configs.write().await.insert(name.into(), config);
    }

    /// Removes a server configuration from the running manager.
    pub async fn remove_server_config(&self, name: &str) -> Option<McpServerConfig> {
        self.configs.write().await.remove(name)
    }

    /// Returns enabled server names configured to auto-start.
    pub async fn auto_start_server_names(&self) -> Vec<String> {
        let configs = self.configs.read().await.clone();
        configs
            .into_iter()
            .filter_map(|(name, cfg)| (cfg.enabled && cfg.auto_start).then_some(name))
            .collect()
    }

    /// Starts and initializes an MCP server by name.
    ///
    /// An explicitly supplied token takes precedence over `token_env`. The token is
    /// ephemeral and is never retained in the server configuration.
    pub async fn start_server(
        &self,
        name: &str,
        auth_token: Option<&str>,
    ) -> Result<Arc<McpClient>, McpError> {
        let cfg = {
            let configs = self.configs.read().await;
            configs.get(name).cloned().ok_or_else(|| {
                McpError::Configuration(format!("Server '{name}' is not configured"))
            })?
        };

        if !cfg.enabled {
            return Err(McpError::Configuration(format!(
                "Server '{name}' is disabled in configuration"
            )));
        }

        // Stop existing instance if already running
        let _ = self.stop_server(name).await;

        let timeout = Duration::from_secs(cfg.timeout_secs.max(5));

        // Prepare environment variables with secret token resolution
        let mut env_map: HashMap<String, String> = cfg
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let auth_token = auth_token
            .filter(|token| !token.trim().is_empty())
            .map(str::trim)
            .map(str::to_string)
            .or_else(|| {
                cfg.token_env.as_ref().and_then(|token_env_var| {
                    std::env::var(token_env_var)
                        .ok()
                        .filter(|token| !token.trim().is_empty())
                })
            });
        if let (Some(token_env_var), Some(token)) = (&cfg.token_env, &auth_token) {
            env_map.insert(token_env_var.clone(), token.clone());
        }

        let transport: Arc<dyn McpTransport> = match cfg.transport {
            McpTransportType::Stdio => {
                let cmd = cfg.command.as_deref().ok_or_else(|| {
                    McpError::Configuration(format!(
                        "Server '{name}' missing 'command' for STDIO transport"
                    ))
                })?;

                let work_dir = cfg
                    .working_dir
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.working_dir.clone());

                let stdio =
                    StdioTransport::spawn(name, cmd, &cfg.args, &env_map, Some(&work_dir)).await?;
                Arc::new(stdio)
            }
            McpTransportType::Http => {
                let url = cfg.url.as_deref().ok_or_else(|| {
                    McpError::Configuration(format!(
                        "Server '{name}' missing 'url' for HTTP transport"
                    ))
                })?;

                let mut headers = HashMap::new();
                if let Some(token) = &auth_token {
                    headers.insert("Authorization".to_string(), format!("Bearer {token}"));
                }

                let http = HttpTransport::new(name, url, headers);
                Arc::new(http)
            }
            McpTransportType::Sse => {
                let url = cfg.url.as_deref().ok_or_else(|| {
                    McpError::Configuration(format!(
                        "Server '{name}' missing 'url' for SSE transport"
                    ))
                })?;
                let mut headers = HashMap::new();
                if let Some(token) = &auth_token {
                    headers.insert("Authorization".to_string(), format!("Bearer {token}"));
                }

                let sse = SseTransport::connect(name, url, headers, timeout).await?;
                Arc::new(sse)
            }
        };

        let client = Arc::new(McpClient::new(name, transport, timeout));

        // Handshake
        client.initialize().await?;

        // Discover tools, resources, and prompts
        if let Ok(tools) = client.list_tools().await {
            let mut dt = self.discovered_tools.write().await;
            dt.insert(name.to_string(), tools);
        }

        if let Ok(resources) = client.list_resources().await {
            let mut dr = self.discovered_resources.write().await;
            dr.insert(name.to_string(), resources);
        }

        if let Ok(prompts) = client.list_prompts().await {
            let mut dp = self.discovered_prompts.write().await;
            dp.insert(name.to_string(), prompts);
        }

        {
            let mut servers = self.servers.write().await;
            servers.insert(name.to_string(), client.clone());
        }

        info!(server = %name, "MCP server is online and ready");
        Ok(client)
    }

    /// Stops and disconnects an active MCP server.
    pub async fn stop_server(&self, name: &str) -> Result<(), McpError> {
        let client = {
            let mut servers = self.servers.write().await;
            servers.remove(name)
        };

        if let Some(c) = client {
            let _ = c.disconnect().await;
        }

        {
            let mut dt = self.discovered_tools.write().await;
            dt.remove(name);
            let mut dr = self.discovered_resources.write().await;
            dr.remove(name);
            let mut dp = self.discovered_prompts.write().await;
            dp.remove(name);
        }

        info!(server = %name, "Stopped MCP server");
        Ok(())
    }

    /// Restarts an MCP server.
    pub async fn restart_server(&self, name: &str) -> Result<Arc<McpClient>, McpError> {
        self.stop_server(name).await?;
        self.start_server(name, None).await
    }

    /// Retrieves an active MCP client instance if connected.
    pub async fn get_client(&self, name: &str) -> Option<Arc<McpClient>> {
        let servers = self.servers.read().await;
        servers.get(name).cloned()
    }

    /// Returns a list of all configured server summaries.
    pub async fn list_server_summaries(&self) -> Vec<McpServerSummary> {
        let configs = self.configs.read().await.clone();
        let servers = self.servers.read().await;
        let tools = self.discovered_tools.read().await;
        let resources = self.discovered_resources.read().await;
        let prompts = self.discovered_prompts.read().await;

        let mut summaries = Vec::new();

        for (name, cfg) in configs {
            let (state, err) = if let Some(client) = servers.get(&name) {
                let st = client.state().await;
                let err_msg = match &st {
                    McpServerState::Failed(msg) => Some(msg.clone()),
                    _ => None,
                };
                (st, err_msg)
            } else if !cfg.enabled {
                (McpServerState::Stopped, None)
            } else {
                (McpServerState::Configured, None)
            };

            let transport_str = match cfg.transport {
                McpTransportType::Stdio => "stdio".to_string(),
                McpTransportType::Http => "http".to_string(),
                McpTransportType::Sse => "sse".to_string(),
            };

            let tool_count = tools.get(&name).map(|t| t.len()).unwrap_or(0);
            let resource_count = resources.get(&name).map(|r| r.len()).unwrap_or(0);
            let prompt_count = prompts.get(&name).map(|p| p.len()).unwrap_or(0);

            summaries.push(McpServerSummary {
                name,
                state,
                transport: transport_str,
                tool_count,
                resource_count,
                prompt_count,
                error: err,
            });
        }

        summaries
    }

    /// Returns all discovered MCP tools wrapped as standard `hades_tools::Tool` instances.
    pub async fn discover_all_tools(&self) -> Vec<DynTool> {
        let servers = self.servers.read().await;
        let tools_map = self.discovered_tools.read().await;

        let mut out: Vec<DynTool> = Vec::new();

        for (server_name, tools) in tools_map.iter() {
            if let Some(client) = servers.get(server_name) {
                for t in tools {
                    let adapter = McpToolAdapter::new(server_name, t.clone(), client.clone());
                    out.push(Arc::new(adapter));
                }
            }
        }

        out
    }

    /// Discovers tools for a specific server.
    pub async fn discover_server_tools(&self, server: &str) -> Result<Vec<DynTool>, McpError> {
        let client = self
            .get_client(server)
            .await
            .ok_or_else(|| McpError::NotConnected(server.to_string()))?;

        let tools = client.list_tools().await?;
        {
            let mut dt = self.discovered_tools.write().await;
            dt.insert(server.to_string(), tools.clone());
        }

        let adapters: Vec<DynTool> = tools
            .into_iter()
            .map(|t| Arc::new(McpToolAdapter::new(server, t, client.clone())) as DynTool)
            .collect();

        Ok(adapters)
    }

    /// Discovers resources for a specific server.
    pub async fn discover_server_resources(
        &self,
        server: &str,
    ) -> Result<Vec<McpResource>, McpError> {
        let client = self
            .get_client(server)
            .await
            .ok_or_else(|| McpError::NotConnected(server.to_string()))?;

        let resources = client.list_resources().await?;
        {
            let mut dr = self.discovered_resources.write().await;
            dr.insert(server.to_string(), resources.clone());
        }

        Ok(resources)
    }

    /// Discovers prompts for a specific server.
    pub async fn discover_server_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, McpError> {
        let client = self
            .get_client(server)
            .await
            .ok_or_else(|| McpError::NotConnected(server.to_string()))?;

        let prompts = client.list_prompts().await?;
        {
            let mut dp = self.discovered_prompts.write().await;
            dp.insert(server.to_string(), prompts.clone());
        }

        Ok(prompts)
    }

    /// Reads resource contents on a specific server.
    pub async fn read_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<ReadResourceResult, McpError> {
        let client = self
            .get_client(server)
            .await
            .ok_or_else(|| McpError::NotConnected(server.to_string()))?;

        client.read_resource(uri).await
    }

    /// Evaluates and retrieves a prompt from a specific server.
    pub async fn get_prompt(
        &self,
        server: &str,
        name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<GetPromptResult, McpError> {
        let client = self
            .get_client(server)
            .await
            .ok_or_else(|| McpError::NotConnected(server.to_string()))?;

        client.get_prompt(name, arguments).await
    }

    /// Gracefully shuts down all active MCP servers.
    pub async fn shutdown_all(&self) {
        let servers: Vec<(String, Arc<McpClient>)> = {
            let mut s = self.servers.write().await;
            std::mem::take(&mut *s).into_iter().collect()
        };

        for (name, client) in servers {
            info!(server = %name, "Shutting down MCP server");
            let _ = client.disconnect().await;
        }
    }
}
