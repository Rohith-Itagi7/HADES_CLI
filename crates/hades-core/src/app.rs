use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info};

use crate::command::{CommandContext, CommandOutput, CommandRegistry};
use crate::context::{ContextManager, ContextReport, TokenEstimator};
use crate::error::CoreError;
use crate::notification::{NotificationKind, NotificationService};
use crate::state::AppState;
use hades_config::{ActiveModelConfig, ConfigService, HadesConfig};
use hades_events::{EventBus, HadesEvent};
use hades_mcp::McpServerManager;
use hades_provider::{
    CompletionRequest, CompletionResponse, Credential, CredentialBackend, FileCredentialBackend,
    Model, ModelManager, OpenAiProvider, StreamResult, Usage,
};
use hades_storage::{
    FileSessionRepository, Message, SessionMetadata, SessionRecord, SessionRepository,
    StorageHealth, StorageService,
};
use hades_tools::{
    ApprovalDecision, DynTool, EvaluationResult, PermissionEngine, RiskLevel, ToolCall,
    ToolContext, ToolRegistry, ToolResult, ToolStatus, WorkspaceDetector, WorkspaceMetadata,
};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Information describing a tool execution awaiting interactive user authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub call: ToolCall,
    pub risk: RiskLevel,
    pub summary: String,
    pub details: String,
    pub agent_role: Option<String>,
    pub agent_name: Option<String>,
}

/// Central core runtime managing application lifecycle, sessions, context, providers, and subsystems.
pub struct HadesApp {
    state: AppState,
    config_service: ConfigService,
    config: HadesConfig,
    storage_service: StorageService,
    session_repository: Arc<dyn SessionRepository>,
    active_session: Option<SessionRecord>,
    context_manager: ContextManager,
    event_bus: EventBus,
    command_registry: CommandRegistry,
    model_manager: ModelManager,
    credential_backend: Arc<dyn CredentialBackend>,
    workspace_info: WorkspaceMetadata,
    tool_registry: ToolRegistry,
    permission_engine: PermissionEngine,
    pending_approval: Option<PendingApproval>,
    mcp_manager: McpServerManager,
    orchestrator: hades_agent::AgentOrchestrator,
    browser_manager: Arc<hades_browser::BrowserManager>,
    notification_service: NotificationService,
    smart_orchestrator: crate::orchestration::SmartContextOrchestrator,
    last_request_plan: Option<crate::orchestration::RequestPlan>,
    version: &'static str,
}

impl HadesApp {
    /// Creates a new `HadesApp` instance with default provider engine and session repository registrations.
    pub fn new(
        config_service: ConfigService,
        storage_service: StorageService,
        event_bus: EventBus,
    ) -> Self {
        let mut model_manager = ModelManager::new();

        // Register Phase 1 OpenAI-compatible provider suite
        model_manager.register_provider(Arc::new(OpenAiProvider::openai()));
        model_manager.register_provider(Arc::new(OpenAiProvider::groq()));
        model_manager.register_provider(Arc::new(OpenAiProvider::ollama()));
        model_manager.register_provider(Arc::new(OpenAiProvider::custom()));

        let credential_backend: Arc<dyn CredentialBackend> =
            match FileCredentialBackend::default_location() {
                Ok(backend) => Arc::new(backend),
                Err(_) => Arc::new(FileCredentialBackend::with_path(".hades/credentials.json")),
            };

        let session_repository: Arc<dyn SessionRepository> = match FileSessionRepository::new() {
            Ok(repo) => Arc::new(repo),
            Err(_) => Arc::new(FileSessionRepository::with_dir(".hades/sessions")),
        };

        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let workspace_info = WorkspaceDetector::detect(&current_dir);
        let mut tool_registry = ToolRegistry::default_registry();
        let permission_engine = PermissionEngine::new();
        let mcp_manager =
            McpServerManager::new(&workspace_info.current_dir).with_event_bus(event_bus.clone());
        let orchestrator = hades_agent::AgentOrchestrator::new().with_event_bus(event_bus.clone());
        let browser_manager = Arc::new(
            hades_browser::BrowserManager::new(&workspace_info.current_dir)
                .with_event_bus(event_bus.clone()),
        );

        // Register Phase 6 Web Intelligence and Browser Automation Tool Suite
        hades_browser::BrowserToolSet::register_all(&mut tool_registry, browser_manager.clone());
        let notification_service =
            NotificationService::new(HadesConfig::default().notification, Some(event_bus.clone()));

        Self {
            state: AppState::Startup,
            config_service,
            config: HadesConfig::default(),
            storage_service,
            session_repository,
            active_session: None,
            context_manager: ContextManager::new(),
            event_bus,
            command_registry: CommandRegistry::with_defaults(),
            model_manager,
            credential_backend,
            workspace_info,
            tool_registry,
            permission_engine,
            pending_approval: None,
            mcp_manager,
            orchestrator,
            browser_manager,
            notification_service,
            smart_orchestrator: crate::orchestration::SmartContextOrchestrator::new(),
            last_request_plan: None,
            version: APP_VERSION,
        }
    }

    /// Creates a new `HadesApp` instance with custom credential backend and session repository (e.g. for testing).
    pub fn with_backends(
        config_service: ConfigService,
        storage_service: StorageService,
        event_bus: EventBus,
        credential_backend: Arc<dyn CredentialBackend>,
        session_repository: Arc<dyn SessionRepository>,
    ) -> Self {
        let mut app = Self::new(config_service, storage_service, event_bus);
        app.credential_backend = credential_backend;
        app.session_repository = session_repository;
        app
    }

    /// Backward compatibility constructor for existing tests.
    pub fn with_credential_backend(
        config_service: ConfigService,
        storage_service: StorageService,
        event_bus: EventBus,
        credential_backend: Arc<dyn CredentialBackend>,
    ) -> Self {
        let mut app = Self::new(config_service, storage_service, event_bus);
        app.credential_backend = credential_backend;
        app
    }

    /// Access notification service.
    pub fn notification_service(&self) -> &NotificationService {
        &self.notification_service
    }

    /// Triggers sound and desktop notification.
    pub fn notify(&self, kind: NotificationKind, title: &str, message: &str) {
        self.notification_service.notify(kind, title, message);
    }

    /// Initializes all underlying subsystems, loads configuration, active model, and session state.
    pub fn init(&mut self) -> Result<(), CoreError> {
        info!("Initializing Hades core runtime (version {})", self.version);

        // 1. Initialize storage
        self.storage_service.initialize()?;

        // 2. Load or create configuration
        self.config = self.config_service.load_or_create()?;
        self.notification_service
            .update_config(self.config.notification.clone());
        self.event_bus
            .publish(HadesEvent::config_loaded(self.config_service.config_path()));

        // 3. Model & Provider initialization
        let mut model_activated = false;
        if let Some(ref model_cfg) = self.config.model {
            if self
                .model_manager
                .get_provider(&model_cfg.provider_id)
                .is_some()
            {
                self.model_manager
                    .set_active(&model_cfg.provider_id, &model_cfg.model_id);
                self.event_bus.publish(HadesEvent::model_loaded(
                    &model_cfg.provider_id,
                    &model_cfg.model_id,
                ));
                model_activated = true;
            }
        }

        // 4. Initial session preparation
        let default_prov = self
            .model_manager
            .active_provider_id()
            .map(|s| s.to_string());
        let default_mod = self.model_manager.active_model_id().map(|s| s.to_string());
        self.active_session = Some(SessionRecord::new(None, default_prov, default_mod));

        // 5. Initial state determination:
        // If a valid model is already configured and active -> Running
        // Otherwise -> ProviderSelect (interactive setup on startup)
        if model_activated {
            self.transition_to(AppState::Running)?;
        } else {
            self.transition_to(AppState::ProviderSelect)?;
        }

        // 6. Publish startup event
        self.event_bus
            .publish(HadesEvent::app_started(self.version));

        info!(
            "Hades core runtime initialized successfully (state: {:?})",
            self.state
        );
        Ok(())
    }

    /// Initializes conversation session:
    /// - If `resume_session_id` is provided (e.g. via `hades --session <id>`), explicitly resumes that exact session.
    ///   If unavailable/corrupted, starts a new session and returns a user-facing error message.
    ///   If the session's original model is unavailable, restores history safely without crashing and returns a warning.
    /// - If `resume_session_id` is None (normal startup), ALWAYS starts a fresh new session.
    pub async fn init_session(
        &mut self,
        resume_session_id: Option<&str>,
    ) -> Result<Option<String>, CoreError> {
        if let Some(session_id) = resume_session_id {
            match self.session_repository.get_session(session_id).await {
                Ok(Some(session)) => {
                    info!(
                        session_id = %session.metadata.id,
                        title = %session.metadata.title,
                        messages = session.messages.len(),
                        "Explicitly resumed session from storage"
                    );

                    let mut warning = None;
                    // If session had a specific active model, attempt to activate it
                    if let (Some(ref p), Some(ref m)) = (
                        &session.metadata.active_provider,
                        &session.metadata.active_model,
                    ) {
                        if self.model_manager.get_provider(p).is_some() {
                            self.model_manager.set_active(p, m);
                        } else {
                            warning = Some(format!(
                                "Session restored. Original model unavailable: {p}/{m}. Select another model with /model."
                            ));
                        }
                    }

                    let _ = self
                        .session_repository
                        .set_active_session_id(&session.metadata.id)
                        .await;
                    self.active_session = Some(session);
                    self.start_configured_mcp_servers().await;
                    return Ok(warning);
                }
                _ => {
                    // Session not found -> create fresh session and return clear message
                    let default_prov = self
                        .model_manager
                        .active_provider_id()
                        .map(|s| s.to_string());
                    let default_mod = self.model_manager.active_model_id().map(|s| s.to_string());
                    let new_session = SessionRecord::new(None, default_prov, default_mod);
                    let _ = self.session_repository.save_session(&new_session).await;
                    let _ = self
                        .session_repository
                        .set_active_session_id(&new_session.metadata.id)
                        .await;
                    self.event_bus.publish(HadesEvent::session_created(
                        &new_session.metadata.id,
                        &new_session.metadata.title,
                    ));
                    self.active_session = Some(new_session);
                    self.start_configured_mcp_servers().await;

                    return Ok(Some(format!(
                        "Hades could not find session: {session_id}. Use /sessions to view available sessions."
                    )));
                }
            }
        }

        // Normal startup: ALWAYS create a brand new session
        let default_prov = self
            .model_manager
            .active_provider_id()
            .map(|s| s.to_string());
        let default_mod = self.model_manager.active_model_id().map(|s| s.to_string());
        let new_session = SessionRecord::new(None, default_prov, default_mod);
        let _ = self.session_repository.save_session(&new_session).await;
        let _ = self
            .session_repository
            .set_active_session_id(&new_session.metadata.id)
            .await;
        self.event_bus.publish(HadesEvent::session_created(
            &new_session.metadata.id,
            &new_session.metadata.title,
        ));
        self.active_session = Some(new_session);

        self.start_configured_mcp_servers().await;

        Ok(None)
    }

    async fn start_configured_mcp_servers(&mut self) {
        self.mcp_manager.load_from_config(&self.config.mcp).await;
        for name in self.mcp_manager.auto_start_server_names().await {
            info!(server = %name, "Auto-starting MCP server");
            if let Err(error) = self.start_mcp_server(&name).await {
                tracing::error!(server = %name, error = %error, "Failed to auto-start MCP server");
            }
        }
        self.sync_mcp_tools().await;
    }

    /// Returns current application state.
    pub fn state(&self) -> AppState {
        self.state
    }

    /// Returns the active configuration.
    pub fn config(&self) -> &HadesConfig {
        &self.config
    }

    /// Returns the storage service reference.
    pub fn storage(&self) -> &StorageService {
        &self.storage_service
    }

    /// Returns the session repository reference.
    pub fn session_repository(&self) -> &Arc<dyn SessionRepository> {
        &self.session_repository
    }

    /// Returns the active session record reference.
    pub fn active_session(&self) -> Option<&SessionRecord> {
        self.active_session.as_ref()
    }

    /// Returns mutable reference to the active session record.
    pub fn active_session_mut(&mut self) -> Option<&mut SessionRecord> {
        self.active_session.as_mut()
    }

    /// Returns the context manager reference.
    pub fn context_manager(&self) -> &ContextManager {
        &self.context_manager
    }

    /// Returns mutable reference to the context manager.
    pub fn context_manager_mut(&mut self) -> &mut ContextManager {
        &mut self.context_manager
    }

    /// Returns the event bus reference.
    pub fn events(&self) -> &EventBus {
        &self.event_bus
    }

    /// Returns the command registry reference.
    pub fn commands(&self) -> &CommandRegistry {
        &self.command_registry
    }

    /// Returns the model manager reference.
    pub fn model_manager(&self) -> &ModelManager {
        &self.model_manager
    }

    /// Returns mutable reference to the model manager.
    pub fn model_manager_mut(&mut self) -> &mut ModelManager {
        &mut self.model_manager
    }

    /// Returns the credential backend reference.
    pub fn credential_backend(&self) -> &Arc<dyn CredentialBackend> {
        &self.credential_backend
    }

    fn mcp_credential_id(server_name: &str) -> String {
        format!("mcp:{server_name}")
    }

    /// Stores an optional MCP server authentication token outside of configuration.
    pub async fn store_mcp_auth_token(
        &self,
        server_name: &str,
        token: Option<&str>,
    ) -> Result<(), CoreError> {
        if let Some(token) = token {
            let credential = Credential::with_api_key(Self::mcp_credential_id(server_name), token);
            self.credential_backend
                .store_credential(&credential)
                .await?;
        }
        Ok(())
    }

    /// Retrieves an MCP server authentication token, if one is stored.
    pub async fn mcp_auth_token(&self, server_name: &str) -> Result<Option<String>, CoreError> {
        let credential = self
            .credential_backend
            .get_credential(&Self::mcp_credential_id(server_name))
            .await?;
        Ok(credential.and_then(|credential| {
            credential
                .api_key
                .as_ref()
                .map(|secret| secret.expose_secret().to_string())
        }))
    }

    /// Deletes the stored authentication token for an MCP server.
    pub async fn delete_mcp_auth_token(&self, server_name: &str) -> Result<bool, CoreError> {
        Ok(self
            .credential_backend
            .delete_credential(&Self::mcp_credential_id(server_name))
            .await?)
    }

    /// Returns human-readable representation of the currently active model.
    pub fn active_model_display(&self) -> String {
        match (
            self.model_manager.active_provider_id(),
            self.model_manager.active_model_id(),
        ) {
            (Some(p), Some(m)) => format!("{p}/{m}"),
            _ => "Not configured".to_string(),
        }
    }

    /// Returns human-readable summary of context usage for the active session.
    pub fn context_usage_display(&self) -> String {
        let active_model = self
            .model_manager
            .active_model_id()
            .unwrap_or("llama-3.3-70b-versatile");
        let limit = self.context_manager.resolve_context_limit(active_model);

        let estimated_tokens: usize = self
            .active_session
            .as_ref()
            .map(|s| {
                s.messages
                    .iter()
                    .map(|m| TokenEstimator::estimate_message_tokens(m.role, &m.content))
                    .sum()
            })
            .unwrap_or(0);

        format!("{estimated_tokens} / {limit} (Estimated)")
    }

    /// Returns active workspace metadata.
    pub fn workspace(&self) -> &WorkspaceMetadata {
        &self.workspace_info
    }

    /// Re-detects and sets the active workspace from a directory path.
    pub fn set_workspace(&mut self, path: &Path) {
        self.workspace_info = WorkspaceDetector::detect(path);
        self.event_bus.publish(HadesEvent::WorkspaceDetected {
            timestamp: chrono::Utc::now(),
            root: self.workspace_info.root.clone(),
            project_type: self.workspace_info.project_type.to_string(),
        });
    }

    /// Returns the tool registry reference.
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// Returns mutable reference to the tool registry.
    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tool_registry
    }

    /// Returns the permission engine reference.
    pub fn permission_engine(&self) -> &PermissionEngine {
        &self.permission_engine
    }

    /// Returns mutable reference to the permission engine.
    pub fn permission_engine_mut(&mut self) -> &mut PermissionEngine {
        &mut self.permission_engine
    }

    /// Returns the MCP server manager reference.
    pub fn mcp_manager(&self) -> &McpServerManager {
        &self.mcp_manager
    }

    /// Returns mutable reference to the MCP server manager.
    pub fn mcp_manager_mut(&mut self) -> &mut McpServerManager {
        &mut self.mcp_manager
    }

    /// Returns the browser automation and web retrieval manager reference.
    pub fn browser_manager(&self) -> Arc<hades_browser::BrowserManager> {
        self.browser_manager.clone()
    }

    /// Returns the last smart orchestration request plan, if available.
    pub fn last_request_plan(&self) -> Option<&crate::orchestration::RequestPlan> {
        self.last_request_plan.as_ref()
    }

    /// Accessor for the smart context and tool orchestrator.
    pub fn smart_orchestrator(&self) -> &crate::orchestration::SmartContextOrchestrator {
        &self.smart_orchestrator
    }

    /// Mutable accessor for the smart context and tool orchestrator.
    pub fn smart_orchestrator_mut(
        &mut self,
    ) -> &mut crate::orchestration::SmartContextOrchestrator {
        &mut self.smart_orchestrator
    }

    /// Synchronizes discovered MCP tools from active servers into the core tool registry.
    pub async fn sync_mcp_tools(&mut self) -> usize {
        let mcp_tools = self.mcp_manager.discover_all_tools().await;
        let count = mcp_tools.len();
        for tool in mcp_tools {
            self.tool_registry.register_arc(tool);
        }
        info!(count = count, "Synchronized MCP tools into registry");
        count
    }

    /// Connects to a named MCP server and synchronizes its tools into Hades.
    pub async fn connect_mcp_server(&mut self, server_name: &str) -> Result<(), CoreError> {
        self.start_mcp_server(server_name).await?;
        self.sync_mcp_tools().await;
        Ok(())
    }

    /// Resets the tool registry with all native built-in tools (29 core + 22 browser tools).
    pub fn reset_native_tools(&mut self) {
        let mut registry = ToolRegistry::default_registry();
        hades_browser::BrowserToolSet::register_all(&mut registry, self.browser_manager.clone());
        self.tool_registry = registry;
    }

    /// Disconnects a named MCP server and refreshes the tool registry.
    pub async fn disconnect_mcp_server(&mut self, server_name: &str) -> Result<(), CoreError> {
        self.mcp_manager
            .stop_server(server_name)
            .await
            .map_err(|e| CoreError::Runtime(e.to_string()))?;
        // Reset to default native tools (29 core + 22 browser) + remaining MCP tools
        self.reset_native_tools();
        self.sync_mcp_tools().await;
        Ok(())
    }

    /// Returns the pending tool approval request, if any.
    pub fn pending_approval(&self) -> Option<&PendingApproval> {
        self.pending_approval.as_ref()
    }

    /// Adds a new MCP server to the configuration and persists it.
    pub async fn add_mcp_server(
        &mut self,
        name: &str,
        transport: &str,
        command_or_url: &str,
        args: &str,
        token_env: &str,
        auth_token: Option<&str>,
    ) -> Result<(), CoreError> {
        use hades_config::model::{McpServerConfig, McpTransportType};

        let transport_type = match transport.to_lowercase().as_str() {
            "http" => McpTransportType::Http,
            _ => McpTransportType::Stdio,
        };

        let mut server_config = McpServerConfig {
            transport: transport_type,
            enabled: true,
            auto_start: true,
            timeout_secs: 30,
            ..Default::default()
        };

        match server_config.transport {
            McpTransportType::Stdio => {
                server_config.command = Some(command_or_url.to_string());
                if !args.is_empty() {
                    server_config.args = args.split_whitespace().map(str::to_string).collect();
                }
            }
            McpTransportType::Http | McpTransportType::Sse => {
                server_config.url = Some(command_or_url.to_string());
            }
        }

        if !token_env.is_empty() {
            server_config.token_env = Some(token_env.to_string());
        }

        self.store_mcp_auth_token(name, auth_token.filter(|token| !token.trim().is_empty()))
            .await?;

        self.config
            .mcp
            .servers
            .insert(name.to_string(), server_config.clone());
        self.config_service
            .save(&self.config)
            .map_err(|e| CoreError::Runtime(format!("Failed to save config: {e}")))?;
        self.event_bus
            .publish(HadesEvent::config_saved(self.config_service.config_path()));

        self.mcp_manager
            .upsert_server_config(name, server_config)
            .await;

        if let Err(e) = self.start_mcp_server(name).await {
            tracing::warn!("Failed to start MCP server '{}': {}", name, e);
        } else {
            self.sync_mcp_tools().await;
        }

        Ok(())
    }

    /// Removes an MCP server from the configuration and disconnects it.
    pub async fn remove_mcp_server(&mut self, name: &str) -> Result<(), CoreError> {
        if let Err(e) = self.mcp_manager.stop_server(name).await {
            tracing::warn!("Failed to stop MCP server '{}': {}", name, e);
        }

        self.config.mcp.servers.remove(name);
        self.config_service
            .save(&self.config)
            .map_err(|e| CoreError::Runtime(format!("Failed to save config: {e}")))?;
        self.event_bus
            .publish(HadesEvent::config_saved(self.config_service.config_path()));

        self.mcp_manager.remove_server_config(name).await;
        self.delete_mcp_auth_token(name).await?;

        self.reset_native_tools();
        self.sync_mcp_tools().await;
        Ok(())
    }

    /// Tests the connection to an MCP server and reports its status.
    pub async fn test_mcp_server(&mut self, name: &str) -> Result<String, CoreError> {
        if let Err(e) = self.start_mcp_server(name).await {
            return Ok(format!(
                "✗ Failed to connect to MCP server '{}': {}",
                name, e
            ));
        }

        let summaries = self.mcp_manager.list_server_summaries().await;
        if let Some(summary) = summaries.iter().find(|s| s.name == name) {
            let status = match &summary.state {
                hades_mcp::McpServerState::Ready => "✓ Ready",
                hades_mcp::McpServerState::Connected => "✓ Connected",
                hades_mcp::McpServerState::Starting => "⟳ Starting",
                hades_mcp::McpServerState::Configured => "○ Configured (not started)",
                hades_mcp::McpServerState::Disconnected => "✗ Disconnected",
                hades_mcp::McpServerState::Failed(err) => return Ok(format!("✗ Failed: {err}")),
                hades_mcp::McpServerState::Stopping => "⟳ Stopping",
                hades_mcp::McpServerState::Stopped => "✗ Stopped",
            };

            let mut output = format!("MCP Server Test: {name}\n\n");
            output.push_str(&format!("Status:    {status}\n"));
            output.push_str(&format!("Transport: {}\n", summary.transport));
            output.push_str(&format!("Tools:     {}\n", summary.tool_count));
            output.push_str(&format!("Resources: {}\n", summary.resource_count));
            if let Some(error) = &summary.error {
                output.push_str(&format!("Error:     {error}\n"));
            }
            Ok(output)
        } else {
            Ok(format!("✗ MCP server '{name}' not found in summaries"))
        }
    }

    async fn start_mcp_server(&self, name: &str) -> Result<(), CoreError> {
        let auth_token = self.mcp_auth_token(name).await?;
        self.mcp_manager
            .start_server(name, auth_token.as_deref())
            .await
            .map(|_| ())
            .map_err(|error| CoreError::Runtime(error.to_string()))
    }

    /// Sets or clears the pending tool approval request.
    pub fn set_pending_approval(&mut self, approval: Option<PendingApproval>) {
        self.pending_approval = approval;
    }

    /// Executes a tool call through validation, permission evaluation, and execution.
    pub async fn execute_tool_call(&mut self, call: ToolCall) -> Result<ToolResult, CoreError> {
        let tool = match self.tool_registry.get(&call.tool_name) {
            Some(t) => t,
            None => {
                return Ok(ToolResult::failure(
                    &call.id,
                    &call.tool_name,
                    format!("Tool '{}' is not registered in Hades", call.tool_name),
                ));
            }
        };

        let session_id = self
            .active_session
            .as_ref()
            .map(|s| s.metadata.id.clone())
            .unwrap_or_else(|| "default".to_string());

        let context = ToolContext::new(
            &session_id,
            &self.workspace_info.root,
            &self.workspace_info.current_dir,
        );

        let execution_id = context.execution_id.clone();
        self.event_bus.publish(HadesEvent::ToolRequested {
            timestamp: chrono::Utc::now(),
            execution_id: execution_id.clone(),
            session_id: session_id.clone(),
            tool_name: call.tool_name.clone(),
            arguments: call.arguments.to_string(),
        });

        let eval = self
            .permission_engine
            .evaluate(&call, &tool.definition(), &context);

        match eval {
            EvaluationResult::Denied { reason } => {
                self.event_bus.publish(HadesEvent::ToolDenied {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    reason: reason.clone(),
                });
                Ok(ToolResult::permission_denied(
                    &call.id,
                    &call.tool_name,
                    reason,
                ))
            }
            EvaluationResult::RequiresApproval {
                risk,
                summary,
                details,
            } => {
                self.pending_approval = Some(PendingApproval {
                    call: call.clone(),
                    risk,
                    summary: summary.clone(),
                    details: details.clone(),
                    agent_role: None,
                    agent_name: None,
                });
                self.transition_to(AppState::ToolApproval)?;
                self.event_bus.publish(HadesEvent::ToolApprovalRequested {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    session_id,
                    tool_name: call.tool_name.clone(),
                    risk_level: risk.to_string(),
                    summary: summary.clone(),
                    details: details.clone(),
                });
                self.notify(
                    NotificationKind::InputRequired,
                    "Tool Approval Required",
                    &format!(
                        "Authorization required for tool '{}' ({})",
                        call.tool_name, risk
                    ),
                );
                Ok(ToolResult::permission_denied(
                    &call.id,
                    &call.tool_name,
                    "Tool execution paused awaiting user authorization",
                ))
            }
            EvaluationResult::Permitted { .. } => {
                let res = self.run_tool_internal(tool, call, context).await;
                Ok(res)
            }
        }
    }

    /// Resolves the pending tool approval request with the user's decision.
    pub async fn resolve_pending_approval(
        &mut self,
        decision: ApprovalDecision,
    ) -> Result<ToolResult, CoreError> {
        let approval = self.pending_approval.take().ok_or_else(|| {
            CoreError::Runtime("No pending tool approval request to resolve".to_string())
        })?;

        let session_id = self
            .active_session
            .as_ref()
            .map(|s| s.metadata.id.clone())
            .unwrap_or_else(|| "default".to_string());

        let context = ToolContext::new(
            &session_id,
            &self.workspace_info.root,
            &self.workspace_info.current_dir,
        );

        let execution_id = context.execution_id.clone();

        match decision {
            ApprovalDecision::Deny => {
                self.event_bus.publish(HadesEvent::ToolDenied {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    reason: "Denied by user".to_string(),
                });
                let _ = self.transition_to(AppState::Running);
                Ok(ToolResult::permission_denied(
                    &approval.call.id,
                    &approval.call.tool_name,
                    "Tool execution was denied by user",
                ))
            }
            ApprovalDecision::Cancel => {
                self.event_bus.publish(HadesEvent::ToolCancelled {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    tool_name: approval.call.tool_name.clone(),
                });
                let _ = self.transition_to(AppState::Running);
                Ok(ToolResult {
                    call_id: approval.call.id,
                    tool_name: approval.call.tool_name,
                    status: ToolStatus::Cancelled,
                    output: "Operation cancelled by user".to_string(),
                    error: Some("Operation cancelled by user".to_string()),
                    metadata: serde_json::json!({}),
                    is_truncated: false,
                    artifact_id: None,
                })
            }
            ApprovalDecision::AllowSession => {
                self.permission_engine
                    .grant_session_permission(&approval.call.tool_name);
                self.event_bus.publish(HadesEvent::ToolApproved {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    decision: "ALLOW_SESSION".to_string(),
                });
                let _ = self.transition_to(AppState::Running);
                let tool = self
                    .tool_registry
                    .get(&approval.call.tool_name)
                    .ok_or_else(|| {
                        CoreError::Runtime(format!("Tool '{}' not found", approval.call.tool_name))
                    })?;
                let res = self.run_tool_internal(tool, approval.call, context).await;
                Ok(res)
            }
            ApprovalDecision::AllowOnce => {
                self.event_bus.publish(HadesEvent::ToolApproved {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    decision: "ALLOW_ONCE".to_string(),
                });
                let _ = self.transition_to(AppState::Running);
                let tool = self
                    .tool_registry
                    .get(&approval.call.tool_name)
                    .ok_or_else(|| {
                        CoreError::Runtime(format!("Tool '{}' not found", approval.call.tool_name))
                    })?;
                let res = self.run_tool_internal(tool, approval.call, context).await;
                Ok(res)
            }
        }
    }

    async fn run_tool_internal(
        &mut self,
        tool: DynTool,
        call: ToolCall,
        context: ToolContext,
    ) -> ToolResult {
        let execution_id = context.execution_id.clone();
        self.event_bus.publish(HadesEvent::ToolStarted {
            timestamp: chrono::Utc::now(),
            execution_id: execution_id.clone(),
            tool_name: call.tool_name.clone(),
        });

        let result = tool
            .execute(&call.id, call.arguments.clone(), &context)
            .await;

        self.smart_orchestrator
            .record_tool_execution(&call.tool_name);

        match result.status {
            ToolStatus::Success => {
                self.event_bus.publish(HadesEvent::ToolCompleted {
                    timestamp: chrono::Utc::now(),
                    execution_id: execution_id.clone(),
                    tool_name: call.tool_name.clone(),
                    status: result.status.to_string(),
                    is_truncated: result.is_truncated,
                });

                // Specific audit events
                if let Some(path_str) = call.arguments.get("path").and_then(|v| v.as_str()) {
                    let path = PathBuf::from(path_str);
                    if call.tool_name == "filesystem.create" {
                        self.event_bus.publish(HadesEvent::FileCreated {
                            timestamp: chrono::Utc::now(),
                            path,
                            execution_id: execution_id.clone(),
                        });
                    } else if call.tool_name == "filesystem.write"
                        || call.tool_name == "filesystem.edit"
                    {
                        self.event_bus.publish(HadesEvent::FileModified {
                            timestamp: chrono::Utc::now(),
                            path,
                            execution_id: execution_id.clone(),
                        });
                    } else if call.tool_name == "filesystem.delete" {
                        self.event_bus.publish(HadesEvent::FileDeleted {
                            timestamp: chrono::Utc::now(),
                            path,
                            execution_id: execution_id.clone(),
                        });
                    }
                } else if call.tool_name == "shell.execute" {
                    let exe = call
                        .arguments
                        .get("executable")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let args: Vec<String> = call
                        .arguments
                        .get("arguments")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|v| v.as_str().unwrap_or_default().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let exit_code = result
                        .metadata
                        .get("exit_code")
                        .and_then(|v| v.as_i64())
                        .map(|c| c as i32);

                    self.event_bus.publish(HadesEvent::ProcessStarted {
                        timestamp: chrono::Utc::now(),
                        executable: exe.clone(),
                        arguments: args,
                        execution_id: execution_id.clone(),
                    });
                    self.event_bus.publish(HadesEvent::ProcessExited {
                        timestamp: chrono::Utc::now(),
                        executable: exe,
                        exit_code,
                        execution_id: execution_id.clone(),
                    });
                } else if call.tool_name == "environment.set"
                    || call.tool_name == "environment.unset"
                {
                    let key = call
                        .arguments
                        .get("key")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    self.event_bus.publish(HadesEvent::EnvironmentChanged {
                        timestamp: chrono::Utc::now(),
                        key,
                        execution_id: execution_id.clone(),
                    });
                }
            }
            ToolStatus::Failure | ToolStatus::InvalidInput => {
                self.event_bus.publish(HadesEvent::ToolFailed {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    tool_name: call.tool_name.clone(),
                    error: result
                        .error
                        .clone()
                        .unwrap_or_else(|| "Tool failed".to_string()),
                });
            }
            ToolStatus::Cancelled => {
                self.event_bus.publish(HadesEvent::ToolCancelled {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    tool_name: call.tool_name.clone(),
                });
            }
            ToolStatus::TimedOut => {
                self.event_bus.publish(HadesEvent::ToolTimedOut {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    tool_name: call.tool_name.clone(),
                    duration_ms: context.timeout.as_millis() as u64,
                });
            }
            ToolStatus::PermissionDenied => {
                self.event_bus.publish(HadesEvent::ToolDenied {
                    timestamp: chrono::Utc::now(),
                    execution_id,
                    reason: result
                        .error
                        .clone()
                        .unwrap_or_else(|| "Permission denied".to_string()),
                });
            }
        }

        // Persist tool message to active session
        if let Some(ref mut session) = self.active_session {
            let session_id = session.metadata.id.clone();
            let tool_output = if !result.output.is_empty() {
                &result.output
            } else if let Some(ref err) = result.error {
                err
            } else {
                "Tool executed with empty output."
            };
            let msg = Message::tool_result(&session_id, &result.call_id, tool_output);
            session.add_message(msg);
            let _ = self.session_repository.save_session(session).await;
        }

        result
    }

    /// Transitions application state if allowed, publishing state change event.
    pub fn transition_to(&mut self, target: AppState) -> Result<(), CoreError> {
        if self.state == target {
            return Ok(());
        }

        self.state.check_transition(target)?;
        let prev_state = self.state;
        self.state = target;

        debug!(
            from = ?prev_state,
            to = ?target,
            "AppState transition succeeded"
        );
        Ok(())
    }

    /// Saves the current active session to persistent storage.
    pub async fn save_active_session(&mut self) -> Result<(), CoreError> {
        if let Some(ref session) = self.active_session {
            self.session_repository.save_session(session).await?;
        }
        Ok(())
    }

    /// Creates a new persistent conversation session and activates it.
    pub async fn create_new_session(
        &mut self,
        title: Option<String>,
    ) -> Result<SessionRecord, CoreError> {
        let _ = self.save_active_session().await;

        let provider = self
            .model_manager
            .active_provider_id()
            .map(|s| s.to_string());
        let model = self.model_manager.active_model_id().map(|s| s.to_string());

        let record = self
            .session_repository
            .create_session(title, provider, model)
            .await?;

        self.event_bus.publish(HadesEvent::session_created(
            &record.metadata.id,
            &record.metadata.title,
        ));

        self.active_session = Some(record.clone());
        Ok(record)
    }

    /// Switches the active conversation session to the specified session ID.
    pub async fn switch_session(&mut self, session_id: &str) -> Result<SessionRecord, CoreError> {
        let _ = self.save_active_session().await;

        let from_id = self.active_session.as_ref().map(|s| s.metadata.id.clone());

        let record = self
            .session_repository
            .get_session(session_id)
            .await?
            .ok_or_else(|| {
                CoreError::Runtime(format!("Session {session_id} not found in repository."))
            })?;

        self.session_repository
            .set_active_session_id(session_id)
            .await?;

        // If switched session had a specific model configured, activate it
        if let (Some(ref p), Some(ref m)) = (
            &record.metadata.active_provider,
            &record.metadata.active_model,
        ) {
            if self.model_manager.get_provider(p).is_some() {
                self.model_manager.set_active(p, m);
            }
        }

        self.event_bus
            .publish(HadesEvent::session_switched(from_id, session_id));

        self.active_session = Some(record.clone());
        info!(session_id = %session_id, title = %record.metadata.title, "Switched active session");
        Ok(record)
    }

    /// Lists metadata for all stored conversation sessions.
    pub async fn list_sessions(&self) -> Result<Vec<SessionMetadata>, CoreError> {
        let list = self.session_repository.list_sessions().await?;
        Ok(list)
    }

    /// Renames a conversation session with a new title.
    pub async fn rename_session(
        &mut self,
        session_id: &str,
        new_title: &str,
    ) -> Result<(), CoreError> {
        let trimmed = new_title.trim();
        if trimmed.is_empty() {
            return Err(CoreError::Runtime(
                "Session title cannot be empty".to_string(),
            ));
        }

        let old_title = if let Some(ref mut session) = self.active_session {
            if session.metadata.id == session_id {
                let old = session.metadata.title.clone();
                session.metadata.title = trimmed.to_string();
                session.metadata.updated_at = chrono::Utc::now();
                old
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        self.session_repository
            .rename_session(session_id, trimmed)
            .await?;

        self.event_bus.publish(HadesEvent::SessionRenamed {
            timestamp: chrono::Utc::now(),
            session_id: session_id.to_string(),
            old_title,
            new_title: trimmed.to_string(),
        });

        info!(session_id = %session_id, title = %trimmed, "Renamed session");
        Ok(())
    }

    /// Deletes a conversation session from repository.
    pub async fn delete_session(&mut self, session_id: &str) -> Result<bool, CoreError> {
        let is_active = self
            .active_session
            .as_ref()
            .map(|s| s.metadata.id == session_id)
            .unwrap_or(false);

        if is_active {
            self.active_session = None;
        }

        let deleted = self.session_repository.delete_session(session_id).await?;
        if deleted {
            self.event_bus.publish(HadesEvent::SessionDeleted {
                timestamp: chrono::Utc::now(),
                session_id: session_id.to_string(),
            });

            if is_active {
                // If active session was deleted, create a new fresh session
                self.create_new_session(None).await?;
            }
        }
        Ok(deleted)
    }

    /// Imports an external conversation transcript from file into the repository and activates it.
    pub async fn import_session_from_file(
        &mut self,
        path: &Path,
    ) -> Result<SessionRecord, CoreError> {
        let record = hades_storage::SessionImporter::import_from_file(path)?;
        self.session_repository.save_session(&record).await?;
        self.session_repository
            .set_active_session_id(&record.metadata.id)
            .await?;
        let from_id = self.active_session.as_ref().map(|s| s.metadata.id.clone());
        self.active_session = Some(record.clone());
        self.event_bus.publish(HadesEvent::session_created(
            &record.metadata.id,
            &record.metadata.title,
        ));
        self.event_bus
            .publish(HadesEvent::session_switched(from_id, &record.metadata.id));
        info!(session_id = %record.metadata.id, title = %record.metadata.title, "Imported conversation session");
        Ok(record)
    }

    /// Exports the currently active conversation session to the target path in the specified format.
    pub fn export_active_session(
        &self,
        format: hades_storage::ExportFormat,
        target_path: &Path,
    ) -> Result<PathBuf, CoreError> {
        let session = self.active_session.as_ref().ok_or_else(|| {
            CoreError::Runtime("No active conversation session to export".to_string())
        })?;
        let saved = hades_storage::SessionExporter::save_export(session, format, target_path)?;
        Ok(saved)
    }

    /// Switches the active model for the CURRENT session without wiping messages or creating a new session.
    pub async fn switch_active_model_for_session(
        &mut self,
        provider_id: &str,
        model_id: &str,
        credential: &Credential,
    ) -> Result<Model, CoreError> {
        info!(provider = %provider_id, model = %model_id, "Switching model for current session");

        // 1. Verify model access
        let verified = self
            .verify_and_persist_active_model(provider_id, model_id, credential)
            .await?;

        // 2. Update active session metadata
        if let Some(ref mut session) = self.active_session {
            session.metadata.active_provider = Some(provider_id.to_string());
            session.metadata.active_model = Some(model_id.to_string());
            session.metadata.updated_at = chrono::Utc::now();
            self.session_repository.save_session(session).await?;

            self.event_bus.publish(HadesEvent::ModelSwitched {
                timestamp: chrono::Utc::now(),
                session_id: session.metadata.id.clone(),
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
            });
        }

        Ok(verified)
    }

    /// Verifies provider credentials, persists active model configuration, and activates the model.
    pub async fn verify_and_persist_active_model(
        &mut self,
        provider_id: &str,
        model_id: &str,
        credential: &Credential,
    ) -> Result<Model, CoreError> {
        info!(provider = %provider_id, model = %model_id, "Verifying and persisting model selection");

        self.event_bus
            .publish(HadesEvent::CredentialVerificationStarted {
                timestamp: chrono::Utc::now(),
                provider_id: provider_id.to_string(),
            });

        // 1. Verify with provider
        let verified_model = match self
            .model_manager
            .verify_provider_and_model(provider_id, model_id, credential)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                self.event_bus
                    .publish(HadesEvent::CredentialVerificationFailed {
                        timestamp: chrono::Utc::now(),
                        provider_id: provider_id.to_string(),
                        error: e.to_string(),
                    });
                return Err(CoreError::Provider(e));
            }
        };

        // 2. Persist credential in secure backend
        self.credential_backend.store_credential(credential).await?;

        // 3. Persist model configuration in config.toml
        self.config.model = Some(ActiveModelConfig {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            endpoint: credential.endpoint.clone(),
        });
        self.config_service.save(&self.config)?;
        self.event_bus
            .publish(HadesEvent::config_saved(self.config_service.config_path()));

        // 4. Activate in model manager
        self.model_manager.set_active(provider_id, model_id);

        // 5. Emit events
        self.event_bus
            .publish(HadesEvent::CredentialVerificationSucceeded {
                timestamp: chrono::Utc::now(),
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
            });
        self.event_bus
            .publish(HadesEvent::model_loaded(provider_id, model_id));

        Ok(verified_model)
    }

    /// Generates the system instruction prompt identifying Hades and explaining workspace and tool bounds.
    pub fn build_system_prompt(&self) -> String {
        let ws = &self.workspace_info;
        let mut sys = format!(
            "You are Hades, an autonomous AI pair programming assistant and universal coding agent.\n\
            CURRENT WORKSPACE ENVIRONMENT:\n\
            - Root directory: {}\n\
            - Working directory: {}\n\
            - Project ecosystem: {}\n",
            ws.root.display(),
            ws.current_dir.display(),
            ws.project_type
        );
        if ws.has_git {
            let branch = ws.git_branch.as_deref().unwrap_or("main");
            sys.push_str(&format!("- Git repository: active (branch: {branch})\n"));
        } else {
            sys.push_str("- Git repository: not detected\n");
        }
        sys.push_str(
            "\nTOOL USE POLICY & INSTRUCTIONS:\n\
            You have access to native workspace and system diagnostic tools:\n\
            - Filesystem tools: read, edit, write, delete, create, and list files inside the workspace.\n\
            - System info & environment tools: system.info, system.platform, system.architecture, system.hostname, system.uptime, environment.list, environment.get.\n\
            - Process tools: system.process.list (lists running processes with CPU/memory), system.process.inspect (inspects process by PID), system.process.find (finds process by name/query).\n\
            - Network tools: system.network.port_check (checks if a port is in use), system.network.port_process (identifies PID/process using a port), system.network.interfaces, system.network.connections.\n\
            - Runtime tools: system.runtime.which (finds executable in PATH), system.runtime.version (inspects installed runtime version).\n\
            - Shell & execution tools: shell.execute.\n"
        );

        let mcp_tools: Vec<_> = self
            .tool_registry
            .list()
            .into_iter()
            .filter(|t| {
                t.name.contains('.')
                    && !t.name.starts_with("system.")
                    && !t.name.starts_with("filesystem.")
                    && !t.name.starts_with("workspace.")
                    && !t.name.starts_with("shell.")
                    && !t.name.starts_with("environment.")
            })
            .collect();

        if !mcp_tools.is_empty() {
            sys.push_str("            - External Model Context Protocol (MCP) tools: ");
            let tool_names = mcp_tools
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            sys.push_str(&tool_names);
            sys.push_str(".\n");
        }

        sys.push_str(
            "            CRITICAL RULES:\n\
            1. Whenever the user asks to list files, read a file, create/modify files, or run commands, you MUST invoke the appropriate tool directly.\n\
            2. Whenever the user asks diagnostic questions about their system (e.g. running processes, what is using a port, installed runtime versions, PATH lookup, environment variables, OS details), you MUST invoke the corresponding system tool. DO NOT say you don't have access to the system when a tool is available.\n\
            3. DO NOT output code snippets or Python scripts explaining how the user can perform the task if a tool is available—INVOKE THE TOOL.\n\
            4. Inspect existing files before editing them.\n\
            5. Provide concise, direct, and factual answers once tool results are available.\n"
        );
        sys
    }

    /// Converts all registered tools into provider-compatible tool definition payloads.
    pub fn provider_tool_definitions(&self) -> Vec<hades_provider::ToolDefinitionPayload> {
        self.tool_registry
            .list()
            .into_iter()
            .map(|def| {
                hades_provider::ToolDefinitionPayload::function(
                    def.name,
                    def.description,
                    def.parameters_schema,
                )
            })
            .collect()
    }

    /// Submits a user prompt to the active model provider for a single-turn completion with persistent session context.
    pub async fn send_prompt(&mut self, prompt: &str) -> Result<CompletionResponse, CoreError> {
        let (provider_id, model_id) = match (
            self.model_manager.active_provider_id(),
            self.model_manager.active_model_id(),
        ) {
            (Some(p), Some(m)) => (p.to_string(), m.to_string()),
            _ => {
                return Err(CoreError::Runtime(
                    "No active AI model configured. Use /model to configure one.".to_string(),
                ))
            }
        };

        let session_id = self
            .active_session
            .as_ref()
            .map(|s| s.metadata.id.clone())
            .unwrap_or_else(|| "default".to_string());

        // 1. Append and persist user message to active session
        let user_msg = Message::user(&session_id, prompt);
        if let Some(ref mut session) = self.active_session {
            session.add_message(user_msg.clone());
            let _ = self.session_repository.save_session(session).await;
        }

        // 2. Orchestrate minimal context and tool capabilities
        let history = self
            .active_session
            .as_ref()
            .map(|s| s.messages.as_slice())
            .unwrap_or(&[]);

        let tool_defs = self.tool_registry.list();
        let active_mcp_servers = self.mcp_manager.active_server_names().await;
        let orch = self.smart_orchestrator.orchestrate(
            prompt,
            history,
            &tool_defs,
            &active_mcp_servers,
            &provider_id,
            &model_id,
            &self.workspace_info,
        );
        self.last_request_plan = Some(orch.plan.clone());

        let credential = self
            .credential_backend
            .get_credential(&provider_id)
            .await?
            .unwrap_or_else(|| Credential::with_api_key(&provider_id, ""));

        self.event_bus.publish(HadesEvent::ModelRequestStarted {
            timestamp: chrono::Utc::now(),
            provider_id: provider_id.clone(),
            model_id: model_id.clone(),
        });

        let mut request = CompletionRequest::single_prompt(&model_id, prompt);
        if !orch.tools.is_empty() {
            request = request.with_tools(orch.tools);
        }
        request.messages = orch.messages;

        // Calculate bounded max_tokens to strictly respect TPM limits and prevent rate limiter overestimation
        let allowed_output = if let Some(tpm) = orch.plan.provider_tpm_limit {
            let rem = tpm.saturating_sub(orch.plan.estimated_total_tokens);
            orch.plan.max_tokens_reserve.min(rem).max(256)
        } else {
            orch.plan.max_tokens_reserve
        };
        request.max_tokens = Some(allowed_output as u32);

        let serialized_bytes = serde_json::to_vec(&request).map(|b| b.len()).unwrap_or(0);
        if let Some(ref mut p) = self.last_request_plan {
            p.max_tokens_reserve = allowed_output;
            p.serialized_request_bytes = serialized_bytes;
        }

        tracing::info!(
            target: "orchestrator",
            provider = %provider_id,
            model = %model_id,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map(|t| t.len()).unwrap_or(0),
            max_tokens = allowed_output,
            serialized_bytes = serialized_bytes,
            tpm_limit = ?orch.plan.provider_tpm_limit,
            "Dispatched model completion request"
        );

        let response = match self.model_manager.complete(request, &credential).await {
            Ok(resp) => resp,
            Err(e) => {
                self.event_bus.publish(HadesEvent::ProviderErrorOccurred {
                    timestamp: chrono::Utc::now(),
                    provider_id,
                    error: e.to_string(),
                });
                return Err(CoreError::Provider(e));
            }
        };

        let total_tokens = response.usage.and_then(|u| u.total_tokens);
        self.event_bus.publish(HadesEvent::ModelResponseCompleted {
            timestamp: chrono::Utc::now(),
            provider_id: provider_id.clone(),
            model_id: model_id.clone(),
            total_tokens,
        });

        // 3. Append and persist assistant message to active session
        let mut assistant_msg = if !response.tool_calls.is_empty() {
            let tc_json = serde_json::to_string(&response.tool_calls).unwrap_or_default();
            Message::assistant_with_tools(
                &session_id,
                &response.content,
                tc_json,
                Some(provider_id),
                Some(model_id),
            )
        } else {
            Message::assistant(
                &session_id,
                &response.content,
                Some(provider_id),
                Some(model_id),
            )
        };
        if let Some(usage) = response.usage {
            assistant_msg.metadata.input_tokens = usage.input_tokens;
            assistant_msg.metadata.output_tokens = usage.output_tokens;
            assistant_msg.metadata.total_tokens = usage.total_tokens;
        }
        assistant_msg.metadata.finish_reason = Some(format!("{:?}", response.finish_reason));

        if let Some(ref mut session) = self.active_session {
            session.add_message(assistant_msg);
            let _ = self.session_repository.save_session(session).await;
        }

        Ok(response)
    }

    /// Submits a user prompt to the active model provider for streaming completion with persistent session context.
    pub async fn send_prompt_stream(
        &mut self,
        prompt: &str,
    ) -> Result<(StreamResult, ContextReport, String), CoreError> {
        let (provider_id, model_id) = match (
            self.model_manager.active_provider_id(),
            self.model_manager.active_model_id(),
        ) {
            (Some(p), Some(m)) => (p.to_string(), m.to_string()),
            _ => {
                return Err(CoreError::Runtime(
                    "No active AI model configured. Use /model to configure one.".to_string(),
                ))
            }
        };

        let session_id = self
            .active_session
            .as_ref()
            .map(|s| s.metadata.id.clone())
            .unwrap_or_else(|| "default".to_string());

        // 1. Append and persist user message
        let user_msg = Message::user(&session_id, prompt);
        if let Some(ref mut session) = self.active_session {
            session.add_message(user_msg);
            let _ = self.session_repository.save_session(session).await;
        }

        // 2. Orchestrate minimal context and tool capabilities
        let history = self
            .active_session
            .as_ref()
            .map(|s| s.messages.as_slice())
            .unwrap_or(&[]);

        let tool_defs = self.tool_registry.list();
        let active_mcp_servers = self.mcp_manager.active_server_names().await;
        let orch = self.smart_orchestrator.orchestrate(
            prompt,
            history,
            &tool_defs,
            &active_mcp_servers,
            &provider_id,
            &model_id,
            &self.workspace_info,
        );
        self.last_request_plan = Some(orch.plan.clone());

        let report = ContextReport {
            total_messages: history.len(),
            included_messages: orch.messages.len(),
            estimated_input_tokens: orch.plan.estimated_total_tokens,
            context_limit: orch.plan.token_budget,
            output_reserve: 1500,
            was_truncated: orch.plan.excluded_tools_count > 0,
        };

        if report.was_truncated {
            self.event_bus.publish(HadesEvent::ContextTruncated {
                timestamp: chrono::Utc::now(),
                session_id: session_id.clone(),
                total_messages: report.total_messages,
                included_messages: report.included_messages,
                estimated_tokens: report.estimated_input_tokens,
            });
        }

        let credential = self
            .credential_backend
            .get_credential(&provider_id)
            .await?
            .unwrap_or_else(|| Credential::with_api_key(&provider_id, ""));

        self.event_bus.publish(HadesEvent::ModelRequestStarted {
            timestamp: chrono::Utc::now(),
            provider_id: provider_id.clone(),
            model_id: model_id.clone(),
        });

        let mut request = CompletionRequest::single_prompt(&model_id, prompt).with_stream(true);
        if !orch.tools.is_empty() {
            request = request.with_tools(orch.tools);
        }
        request.messages = orch.messages;

        // Calculate bounded max_tokens to strictly respect TPM limits and prevent rate limiter overestimation
        let allowed_output = if let Some(tpm) = orch.plan.provider_tpm_limit {
            let rem = tpm.saturating_sub(orch.plan.estimated_total_tokens);
            orch.plan.max_tokens_reserve.min(rem).max(256)
        } else {
            orch.plan.max_tokens_reserve
        };
        request.max_tokens = Some(allowed_output as u32);

        let serialized_bytes = serde_json::to_vec(&request).map(|b| b.len()).unwrap_or(0);
        if let Some(ref mut p) = self.last_request_plan {
            p.max_tokens_reserve = allowed_output;
            p.serialized_request_bytes = serialized_bytes;
        }

        tracing::info!(
            target: "orchestrator",
            provider = %provider_id,
            model = %model_id,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map(|t| t.len()).unwrap_or(0),
            max_tokens = allowed_output,
            serialized_bytes = serialized_bytes,
            tpm_limit = ?orch.plan.provider_tpm_limit,
            "Dispatched model streaming request"
        );

        let stream = match self
            .model_manager
            .complete_stream(request, &credential)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                self.event_bus.publish(HadesEvent::ProviderErrorOccurred {
                    timestamp: chrono::Utc::now(),
                    provider_id,
                    error: e.to_string(),
                });
                return Err(CoreError::Provider(e));
            }
        };

        // 3. Create active assistant message in session
        let mut assistant_msg =
            Message::assistant(&session_id, "", Some(provider_id), Some(model_id));
        assistant_msg.metadata.streaming_complete = false;
        let message_id = assistant_msg.id.clone();

        if let Some(ref mut session) = self.active_session {
            session.add_message(assistant_msg);
            let _ = self.session_repository.save_session(session).await;
        }

        Ok((stream, report, message_id))
    }

    /// Resumes streaming generation for the active session following tool execution.
    pub async fn send_continuation_stream(
        &mut self,
    ) -> Result<(StreamResult, ContextReport, String), CoreError> {
        let (provider_id, model_id) = match (
            self.model_manager.active_provider_id(),
            self.model_manager.active_model_id(),
        ) {
            (Some(p), Some(m)) => (p.to_string(), m.to_string()),
            _ => {
                return Err(CoreError::Runtime(
                    "No active AI model configured. Use /model to configure one.".to_string(),
                ))
            }
        };

        let session_id = self
            .active_session
            .as_ref()
            .map(|s| s.metadata.id.clone())
            .unwrap_or_else(|| "default".to_string());

        let history = self
            .active_session
            .as_ref()
            .map(|s| s.messages.as_slice())
            .unwrap_or(&[]);

        let tool_defs = self.tool_registry.list();
        let active_mcp_servers = self.mcp_manager.active_server_names().await;
        let orch = self.smart_orchestrator.orchestrate_continuation(
            history,
            &tool_defs,
            &active_mcp_servers,
            &provider_id,
            &model_id,
            &self.workspace_info,
        );
        self.last_request_plan = Some(orch.plan.clone());

        let report = ContextReport {
            total_messages: history.len(),
            included_messages: orch.messages.len(),
            estimated_input_tokens: orch.plan.estimated_total_tokens,
            context_limit: orch.plan.token_budget,
            output_reserve: 1500,
            was_truncated: false,
        };

        let credential = self
            .credential_backend
            .get_credential(&provider_id)
            .await?
            .unwrap_or_else(|| Credential::with_api_key(&provider_id, ""));

        self.event_bus.publish(HadesEvent::ModelRequestStarted {
            timestamp: chrono::Utc::now(),
            provider_id: provider_id.clone(),
            model_id: model_id.clone(),
        });

        let mut request = CompletionRequest::single_prompt(&model_id, "").with_stream(true);
        if !orch.tools.is_empty() {
            request = request.with_tools(orch.tools);
        }
        request.messages = orch.messages;

        // Calculate bounded max_tokens to strictly respect TPM limits and prevent rate limiter overestimation
        let allowed_output = if let Some(tpm) = orch.plan.provider_tpm_limit {
            let rem = tpm.saturating_sub(orch.plan.estimated_total_tokens);
            orch.plan.max_tokens_reserve.min(rem).max(256)
        } else {
            orch.plan.max_tokens_reserve
        };
        request.max_tokens = Some(allowed_output as u32);

        let serialized_bytes = serde_json::to_vec(&request).map(|b| b.len()).unwrap_or(0);
        if let Some(ref mut p) = self.last_request_plan {
            p.max_tokens_reserve = allowed_output;
            p.serialized_request_bytes = serialized_bytes;
        }

        tracing::info!(
            target: "orchestrator",
            provider = %provider_id,
            model = %model_id,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map(|t| t.len()).unwrap_or(0),
            max_tokens = allowed_output,
            serialized_bytes = serialized_bytes,
            tpm_limit = ?orch.plan.provider_tpm_limit,
            "Dispatched continuation streaming request"
        );

        let stream = match self
            .model_manager
            .complete_stream(request, &credential)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                self.event_bus.publish(HadesEvent::ProviderErrorOccurred {
                    timestamp: chrono::Utc::now(),
                    provider_id,
                    error: e.to_string(),
                });
                return Err(CoreError::Provider(e));
            }
        };

        let mut assistant_msg =
            Message::assistant(&session_id, "", Some(provider_id), Some(model_id));
        assistant_msg.metadata.streaming_complete = false;
        let message_id = assistant_msg.id.clone();

        if let Some(ref mut session) = self.active_session {
            session.add_message(assistant_msg);
            let _ = self.session_repository.save_session(session).await;
        }

        Ok((stream, report, message_id))
    }

    /// Records structured tool calls attached to an assistant message in the active session.
    pub async fn record_assistant_tool_calls(
        &mut self,
        message_id: &str,
        content: &str,
        tool_calls: &[hades_provider::ProviderToolCall],
    ) -> Result<(), CoreError> {
        let tc_json = serde_json::to_string(tool_calls).unwrap_or_default();
        if let Some(ref mut session) = self.active_session {
            if let Some(msg) = session.messages.iter_mut().find(|m| m.id == message_id) {
                msg.content = content.to_string();
                msg.metadata.tool_calls = Some(tc_json);
                msg.metadata.finish_reason = Some("tool_calls".to_string());
                msg.metadata.streaming_complete = true;
                let _ = self.session_repository.save_session(session).await;
            }
        }
        Ok(())
    }

    /// Finalizes streaming response content and usage into the active session record.
    pub async fn finalize_streaming_response(
        &mut self,
        message_id: &str,
        content: &str,
        usage: Option<Usage>,
        is_interrupted: bool,
    ) -> Result<(), CoreError> {
        let (provider_id, model_id) = (
            self.model_manager
                .active_provider_id()
                .map(|s| s.to_string()),
            self.model_manager.active_model_id().map(|s| s.to_string()),
        );

        if let Some(ref mut session) = self.active_session {
            if let Some(msg) = session.messages.iter_mut().find(|m| m.id == message_id) {
                msg.content = content.to_string();
                msg.metadata.provider = provider_id;
                msg.metadata.model = model_id;
                msg.metadata.streaming_complete = !is_interrupted;
                msg.metadata.is_interrupted = is_interrupted;
                msg.metadata.finish_reason = Some(if is_interrupted {
                    "interrupted".to_string()
                } else {
                    "stop".to_string()
                });

                if let Some(u) = usage {
                    msg.metadata.input_tokens = u.input_tokens;
                    msg.metadata.output_tokens = u.output_tokens;
                    msg.metadata.total_tokens = u.total_tokens;
                }

                let session_id = session.metadata.id.clone();
                let _ = self.session_repository.save_session(session).await;

                self.event_bus.publish(HadesEvent::MessagePersisted {
                    timestamp: chrono::Utc::now(),
                    session_id,
                    message_id: message_id.to_string(),
                    role: "assistant".to_string(),
                });

                if !is_interrupted {
                    self.notify(
                        NotificationKind::TaskCompleted,
                        "Task Completed",
                        "Agent response completed successfully.",
                    );
                }
            }
        }
        Ok(())
    }

    /// Executes a command input string, publishing relevant lifecycle events.
    pub fn execute_command(&mut self, input: &str) -> Result<CommandOutput, CoreError> {
        self.event_bus.publish(HadesEvent::command_entered(input));

        let storage_health = self
            .storage_service
            .health()
            .unwrap_or_else(|e| StorageHealth {
                status: hades_storage::StorageStatus::Unhealthy(e.to_string()),
                root_dir: self.storage_service.root_dir().to_path_buf(),
                writable: false,
            });

        let active_model_str = self.active_model_display();
        let active_model_opt = if active_model_str == "Not configured" {
            None
        } else {
            Some(active_model_str.as_str())
        };

        let session_id = self.active_session.as_ref().map(|s| s.metadata.id.as_str());
        let session_title = self
            .active_session
            .as_ref()
            .map(|s| s.metadata.title.as_str());
        let message_count = self
            .active_session
            .as_ref()
            .map(|s| s.messages.len())
            .unwrap_or(0);
        let context_usage = Some(self.context_usage_display());

        let help_entries = self.command_registry.help_entries();
        let mcp_summaries: Vec<hades_mcp::McpServerSummary> = self
            .config
            .mcp
            .servers
            .iter()
            .map(|(name, cfg)| {
                let transport_str = match cfg.transport {
                    hades_config::McpTransportType::Stdio => "stdio".to_string(),
                    hades_config::McpTransportType::Http => "http".to_string(),
                    hades_config::McpTransportType::Sse => "sse".to_string(),
                };
                let tool_count = self
                    .tool_registry
                    .list()
                    .into_iter()
                    .filter(|t| t.name.starts_with(&format!("{name}.")))
                    .count();
                hades_mcp::McpServerSummary {
                    name: name.clone(),
                    state: if cfg.enabled {
                        if tool_count > 0 {
                            hades_mcp::McpServerState::Ready
                        } else {
                            hades_mcp::McpServerState::Configured
                        }
                    } else {
                        hades_mcp::McpServerState::Stopped
                    },
                    transport: transport_str,
                    tool_count,
                    resource_count: 0,
                    prompt_count: 0,
                    error: None,
                }
            })
            .collect();

        let browser_status = Some(self.browser_manager.status());

        let mut context = CommandContext::new(
            self.state,
            &self.config,
            &storage_health,
            session_id,
            session_title,
            message_count,
            context_usage,
            active_model_opt,
            self.version,
            help_entries,
        )
        .with_tools_and_workspace(
            Some(&self.workspace_info),
            Some(&self.tool_registry),
            Vec::new(),
        )
        .with_mcp_summaries(mcp_summaries)
        .with_browser(browser_status, Some(self.browser_manager.clone()))
        .with_active_session(self.active_session.as_ref())
        .with_request_plan(self.last_request_plan.clone())
        .with_raw_input(input);

        let result = self.command_registry.execute(input, &mut context);

        match result {
            Ok(output) => {
                self.event_bus
                    .publish(HadesEvent::command_executed(input, true));

                if context.open_model_setup_requested
                    || context.open_model_switch_requested
                    || matches!(
                        output,
                        CommandOutput::OpenModelSetup | CommandOutput::OpenModelSwitch
                    )
                {
                    self.transition_to(AppState::ProviderSelect)?;
                } else if context.open_session_picker_requested
                    || matches!(output, CommandOutput::OpenSessionPicker)
                {
                    self.transition_to(AppState::SessionSelect)?;
                } else if matches!(output, CommandOutput::OpenMcpSetup) {
                    self.transition_to(AppState::McpSetup)?;
                } else if matches!(
                    output,
                    CommandOutput::RemoveMcpServer(_) | CommandOutput::TestMcpServer(_)
                ) {
                    // Will be handled asynchronously in the runner
                } else if context.shutdown_requested || matches!(output, CommandOutput::Exit) {
                    self.request_shutdown(Some("Command exit requested".to_string()))?;
                } else if let CommandOutput::ImportSuccess(ref record) = output {
                    let from_id = self.active_session.as_ref().map(|s| s.metadata.id.clone());
                    self.active_session = Some((**record).clone());
                    self.event_bus.publish(HadesEvent::session_created(
                        &record.metadata.id,
                        &record.metadata.title,
                    ));
                    self.event_bus
                        .publish(HadesEvent::session_switched(from_id, &record.metadata.id));
                }

                Ok(output)
            }

            Err(e) => {
                self.event_bus
                    .publish(HadesEvent::command_executed(input, false));
                self.event_bus
                    .publish(HadesEvent::error_occurred(e.to_string()));
                Err(CoreError::Command(e))
            }
        }
    }

    /// Initiates graceful application shutdown and cleanup.
    pub fn request_shutdown(&mut self, reason: Option<String>) -> Result<(), CoreError> {
        if self.state == AppState::ShuttingDown || self.state == AppState::Exited {
            return Ok(());
        }

        info!(reason = ?reason, "Shutting down Hades core runtime");
        self.transition_to(AppState::ShuttingDown)?;

        self.event_bus.publish(HadesEvent::app_shutdown(reason));

        self.transition_to(AppState::Exited)?;
        info!("Hades core runtime shutdown complete");
        Ok(())
    }

    /// Returns an immutable reference to the multi-agent orchestrator.
    pub fn orchestrator(&self) -> &hades_agent::AgentOrchestrator {
        &self.orchestrator
    }

    /// Returns a mutable reference to the multi-agent orchestrator.
    pub fn orchestrator_mut(&mut self) -> &mut hades_agent::AgentOrchestrator {
        &mut self.orchestrator
    }

    /// Delegates an objective to specialized collaborative subagents, executing the plan and synthesizing results.
    pub async fn execute_orchestration(&mut self, objective: &str) -> Result<String, CoreError> {
        let (provider_id, model_id) = match (
            self.model_manager.active_provider_id(),
            self.model_manager.active_model_id(),
        ) {
            (Some(p), Some(m)) => (p.to_string(), m.to_string()),
            _ => {
                return Err(CoreError::Runtime(
                    "No active AI model configured. Use /model to configure one.".to_string(),
                ))
            }
        };

        let provider = self
            .model_manager
            .get_provider(&provider_id)
            .ok_or_else(|| CoreError::Runtime(format!("Provider {provider_id} not found")))?;

        let credential = self
            .credential_backend
            .get_credential(&provider_id)
            .await?
            .unwrap_or_else(|| Credential::with_api_key(&provider_id, ""));

        let session_id = self
            .active_session
            .as_ref()
            .map(|s| s.metadata.id.clone())
            .unwrap_or_else(|| "default".to_string());

        // 1. Record user message in session
        let user_msg = Message::user(&session_id, objective);
        if let Some(ref mut session) = self.active_session {
            session.add_message(user_msg);
            let _ = self.session_repository.save_session(session).await;
        }

        // 2. Formulate execution decision & plan
        let decision = hades_agent::DecisionEngine::evaluate(objective, true);
        let mut plan = hades_agent::DecisionEngine::build_plan(objective, &decision)
            .unwrap_or_else(|| {
                let t1 = hades_agent::Task::new(
                    "task-1-explore",
                    "Exploration & Analysis",
                    format!("Explore relevant context for: {objective}"),
                    hades_agent::AgentRole::Explorer,
                );
                let t2 = hades_agent::Task::new(
                    "task-2-execute",
                    "Task Execution",
                    format!("Execute required actions for: {objective}"),
                    hades_agent::AgentRole::Implementer,
                )
                .with_dependency("task-1-explore");
                hades_agent::TaskPlan::new(
                    objective,
                    hades_agent::OrchestrationStrategy::PlanAndExecute,
                    vec![t1, t2],
                )
            });

        let mut shared_context =
            hades_agent::SharedTaskContext::new(&session_id, objective, &self.workspace_info.root);

        // 3. Run multi-agent orchestration
        let synthesis = self
            .orchestrator
            .orchestrate(
                &mut plan,
                &mut shared_context,
                provider,
                &model_id,
                &credential,
                &self.tool_registry,
                &mut self.permission_engine,
            )
            .await
            .map_err(|e| CoreError::Runtime(format!("Multi-agent orchestration error: {e}")))?;

        // 4. Record synthesized assistant response in session
        let assistant_msg =
            Message::assistant(&session_id, &synthesis, Some(provider_id), Some(model_id));
        if let Some(ref mut session) = self.active_session {
            session.add_message(assistant_msg);
            let _ = self.session_repository.save_session(session).await;
        }

        Ok(synthesis)
    }
}
