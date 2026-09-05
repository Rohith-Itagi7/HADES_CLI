use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::CommandError;
use crate::state::AppState;
use hades_config::HadesConfig;
use std::path::{Path, PathBuf};

use hades_storage::{
    ExportFormat, SessionExporter, SessionImporter, SessionRecord, StorageHealth, StorageStatus,
};

/// Information entry for help listings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpEntry {
    pub name: String,
    pub description: String,
}

/// Structured status snapshot of the application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub application: String,
    pub version: String,
    pub session_id: String,
    pub session_title: String,
    pub messages: usize,
    pub context_usage: String,
    pub model: String,
    pub mode: String,
    pub storage_status: String,
    pub config_status: String,
}

impl fmt::Display for StatusInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "HADES STATUS")?;
        writeln!(f)?;
        writeln!(f, "Application:   {}", self.application)?;
        writeln!(f, "Version:       {}", self.version)?;
        writeln!(f, "Session ID:    {}", self.session_id)?;
        writeln!(f, "Session Title: {}", self.session_title)?;
        writeln!(f, "Messages:      {}", self.messages)?;
        writeln!(f, "Context:       {}", self.context_usage)?;
        writeln!(f, "Active Model:  {}", self.model)?;
        writeln!(f, "Mode:          {}", self.mode)?;
        writeln!(f, "Storage:       {}", self.storage_status)?;
        write!(f, "Configuration: {}", self.config_status)
    }
}

/// Output returned by command execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandOutput {
    /// Generic text output message.
    Text(String),

    /// Help listing showing all registered commands.
    Help(Vec<HelpEntry>),

    /// Application status report.
    Status(StatusInfo),

    /// Signal to open the interactive model/provider selection workflow.
    OpenModelSetup,

    /// Signal to switch active model for the current session.
    OpenModelSwitch,

    /// Signal to create a new session.
    NewSession,

    /// Signal to open the interactive session switcher overlay.
    OpenSessionPicker,

    /// Successful export of active conversation session.
    ExportSuccess(PathBuf),

    /// Successful import of session transcript.
    ImportSuccess(Box<SessionRecord>),

    /// Signal to open the interactive MCP server setup workflow.
    OpenMcpSetup,

    /// Remove an MCP server from configuration.
    RemoveMcpServer(String),

    /// Test connection to an MCP server.
    TestMcpServer(String),

    /// Application exit signal.
    Exit,
}

impl fmt::Display for CommandOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(msg) => write!(f, "{}", msg),
            Self::Help(entries) => {
                writeln!(f, "HADES COMMANDS")?;
                writeln!(f)?;
                for entry in entries {
                    writeln!(f, "  {:<14} {}", entry.name, entry.description)?;
                }
                writeln!(f)?;
                writeln!(f, "KEYBOARD SHORTCUTS & CONTROLS")?;
                writeln!(f, "  {:<14} Submit prompt / Confirm selection", "Enter")?;
                writeln!(
                    f,
                    "  {:<14} Copy selected conversation / assistant response to clipboard",
                    "Ctrl+Y"
                )?;
                writeln!(
                    f,
                    "  {:<14} Interrupt active response / Shutdown Hades",
                    "Ctrl+C"
                )?;
                writeln!(f, "  {:<14} Open interactive command palette", "/")?;
                writeln!(
                    f,
                    "  {:<14} Scroll conversation / Navigate lists & palettes",
                    "Up / Down"
                )?;
                writeln!(f, "  {:<14} Scroll conversation by page", "PageUp / PageDn")?;
                writeln!(
                    f,
                    "  {:<14} Jump to top / bottom of conversation",
                    "Home / End"
                )?;
                writeln!(f, "  {:<14} Dismiss active modal / Close palette", "Esc")?;
                Ok(())
            }
            Self::Status(status) => write!(f, "{}", status),
            Self::OpenModelSetup => write!(f, "Opening AI model configuration..."),
            Self::OpenModelSwitch => write!(f, "Opening model switch for current session..."),
            Self::NewSession => write!(f, "Created new conversation session."),
            Self::OpenSessionPicker => write!(f, "Opening session switcher..."),
            Self::ExportSuccess(path) => {
                write!(f, "Successfully exported session to {}", path.display())
            }
            Self::ImportSuccess(record) => write!(
                f,
                "Successfully imported session '{}' ({} messages)",
                record.metadata.title,
                record.messages.len()
            ),
            Self::OpenMcpSetup => write!(f, "Opening MCP server configuration..."),
            Self::RemoveMcpServer(name) => write!(f, "Removing MCP server '{}'...", name),
            Self::TestMcpServer(name) => write!(f, "Testing MCP server '{}'...", name),
            Self::Exit => write!(f, "Exiting Hades..."),
        }
    }
}

/// Metadata describing a subcommand under a hierarchical command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubcommandInfo {
    /// Canonical subcommand token (e.g. "add", "remove").
    pub name: String,
    /// Human-friendly display label (e.g. "Add MCP server").
    pub display_name: String,
    /// Description of what the subcommand does.
    pub description: String,
    /// Complete command template (e.g. "/mcp add", "/mcp remove ").
    pub command_template: String,
    /// Whether this subcommand requires further arguments before execution.
    pub requires_args: bool,
}

/// Filtered or browsable item rendered in the command palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteItem {
    /// Display text shown in the palette list (e.g. "/mcp add" or "Add MCP server").
    pub display_name: String,
    /// Detailed description of the command or action.
    pub description: String,
    /// Full command string to execute or populate in input.
    pub execution_text: String,
    /// Whether this item represents a subcommand rather than a root command.
    pub is_subcommand: bool,
    /// Whether this command possesses child subcommands that can be explored.
    pub has_subcommands: bool,
    /// Optional parent command if this item is a subcommand.
    pub parent_command: Option<String>,
    /// Whether this item requires additional arguments to be typed.
    pub requires_args: bool,
}

/// Metadata describing a command for palettes and help menus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub subcommands: Vec<SubcommandInfo>,
}

use hades_tools::{ToolRegistry, WorkspaceMetadata};

/// Context provided to commands during execution.
pub struct CommandContext<'a> {
    pub app_state: AppState,
    pub config: &'a HadesConfig,
    pub storage_health: &'a StorageHealth,
    pub session_id: Option<&'a str>,
    pub session_title: Option<&'a str>,
    pub message_count: usize,
    pub context_usage: Option<String>,
    pub active_model: Option<&'a str>,
    pub version: &'a str,
    pub shutdown_requested: bool,
    pub open_model_setup_requested: bool,
    pub open_model_switch_requested: bool,
    pub new_session_requested: bool,
    pub open_session_picker_requested: bool,
    pub available_commands: Vec<HelpEntry>,
    pub workspace_info: Option<&'a WorkspaceMetadata>,
    pub tool_registry: Option<&'a ToolRegistry>,
    pub session_permissions: Vec<String>,
    pub mcp_summaries: Vec<hades_mcp::McpServerSummary>,
    pub browser_status: Option<hades_browser::BrowserStatus>,
    pub browser_manager: Option<Arc<hades_browser::BrowserManager>>,
    pub active_session: Option<&'a SessionRecord>,
    pub raw_input: String,
    pub last_request_plan: Option<crate::orchestration::RequestPlan>,
}

impl<'a> CommandContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app_state: AppState,
        config: &'a HadesConfig,
        storage_health: &'a StorageHealth,
        session_id: Option<&'a str>,
        session_title: Option<&'a str>,
        message_count: usize,
        context_usage: Option<String>,
        active_model: Option<&'a str>,
        version: &'a str,
        available_commands: Vec<HelpEntry>,
    ) -> Self {
        Self {
            app_state,
            config,
            storage_health,
            session_id,
            session_title,
            message_count,
            context_usage,
            active_model,
            version,
            shutdown_requested: false,
            open_model_setup_requested: false,
            open_model_switch_requested: false,
            new_session_requested: false,
            open_session_picker_requested: false,
            available_commands,
            workspace_info: None,
            tool_registry: None,
            session_permissions: Vec::new(),
            mcp_summaries: Vec::new(),

            browser_status: None,
            browser_manager: None,
            active_session: None,
            raw_input: String::new(),
            last_request_plan: None,
        }
    }

    /// Attaches last smart orchestration request plan.
    pub fn with_request_plan(mut self, plan: Option<crate::orchestration::RequestPlan>) -> Self {
        self.last_request_plan = plan;
        self
    }

    /// Sets the active session reference on the command context.
    pub fn with_active_session(mut self, session: Option<&'a SessionRecord>) -> Self {
        self.active_session = session;
        self
    }

    /// Sets the raw command input string.
    pub fn with_raw_input(mut self, input: impl Into<String>) -> Self {
        self.raw_input = input.into();
        self
    }

    /// Attaches workspace, tools, and permission information to the command context.
    pub fn with_tools_and_workspace(
        mut self,
        workspace_info: Option<&'a WorkspaceMetadata>,
        tool_registry: Option<&'a ToolRegistry>,
        session_permissions: Vec<String>,
    ) -> Self {
        self.workspace_info = workspace_info;
        self.tool_registry = tool_registry;
        self.session_permissions = session_permissions;
        self
    }

    /// Attaches MCP server diagnostic summaries to the command context.
    pub fn with_mcp_summaries(mut self, summaries: Vec<hades_mcp::McpServerSummary>) -> Self {
        self.mcp_summaries = summaries;
        self
    }

    /// Attaches browser status and manager to the command context.
    pub fn with_browser(
        mut self,
        status: Option<hades_browser::BrowserStatus>,
        manager: Option<Arc<hades_browser::BrowserManager>>,
    ) -> Self {
        self.browser_status = status;
        self.browser_manager = manager;
        self
    }

    /// Requests application shutdown from within a command.
    pub fn request_shutdown(&mut self) {
        self.shutdown_requested = true;
    }

    /// Requests opening the interactive model setup workflow.
    pub fn request_model_setup(&mut self) {
        self.open_model_setup_requested = true;
    }

    /// Requests opening model switch workflow for current session.
    pub fn request_model_switch(&mut self) {
        self.open_model_switch_requested = true;
    }

    /// Requests creating a new session.
    pub fn request_new_session(&mut self) {
        self.new_session_requested = true;
    }

    /// Requests opening interactive session picker.
    pub fn request_session_picker(&mut self) {
        self.open_session_picker_requested = true;
    }
}

/// Abstraction for all Hades commands.
pub trait Command: Send + Sync {
    /// Canonical command name (including leading slash, e.g. "/help").
    fn name(&self) -> &'static str;

    /// Secondary aliases for the command (e.g. ["/h", "/?"]).
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    /// Short human-readable description of what the command does.
    fn description(&self) -> &'static str;

    /// Optional list of subcommands under this command for hierarchical palettes.
    fn subcommands(&self) -> Vec<SubcommandInfo> {
        Vec::new()
    }

    /// Executes the command with the provided context.
    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError>;
}

// Built-in Phase 0, Phase 1 & Phase 2 Commands

/// Command: `/help`
pub struct HelpCommand;

impl Command for HelpCommand {
    fn name(&self) -> &'static str {
        "/help"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/?", "/h"]
    }

    fn description(&self) -> &'static str {
        "Show available commands"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput::Help(context.available_commands.clone()))
    }
}

/// Command: `/status`
pub struct StatusCommand;

impl Command for StatusCommand {
    fn name(&self) -> &'static str {
        "/status"
    }

    fn description(&self) -> &'static str {
        "Show current application and session status"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let storage_status_str = match &context.storage_health.status {
            StorageStatus::Ready => "Ready".to_string(),
            StorageStatus::Degraded(msg) => format!("Degraded ({})", msg),
            StorageStatus::Unhealthy(msg) => format!("Unhealthy ({})", msg),
        };

        let mode_display = match context.config.general.default_mode.as_str() {
            "simple" => "Simple",
            other => other,
        };

        let model_display = context.active_model.unwrap_or("Not configured").to_string();
        let session_id_str = context.session_id.unwrap_or("None").to_string();
        let session_title_str = context.session_title.unwrap_or("None").to_string();
        let context_usage_str = if let Some(ref plan) = context.last_request_plan {
            format!(
                "{} / {} tokens (Tier {}, {} selected, {} excluded)",
                plan.estimated_total_tokens,
                plan.token_budget,
                plan.selection_tier,
                plan.selected_tools.len(),
                plan.excluded_tools_count
            )
        } else {
            context
                .context_usage
                .clone()
                .unwrap_or_else(|| "0 / 32,768 (Estimated)".to_string())
        };

        let status = StatusInfo {
            application: "Running".to_string(),
            version: context.version.to_string(),
            session_id: session_id_str,
            session_title: session_title_str,
            messages: context.message_count,
            context_usage: context_usage_str,
            model: model_display,
            mode: mode_display.to_string(),
            storage_status: storage_status_str,
            config_status: "Ready".to_string(),
        };

        Ok(CommandOutput::Status(status))
    }
}

/// Command: `/debug`
pub struct DebugCommand;

impl Command for DebugCommand {
    fn name(&self) -> &'static str {
        "/debug"
    }

    fn description(&self) -> &'static str {
        "Show smart context orchestration diagnostics and token budget plans"
    }

    fn subcommands(&self) -> Vec<SubcommandInfo> {
        vec![SubcommandInfo {
            name: "context".to_string(),
            description: "Inspect token budget, capability index, and last request plan"
                .to_string(),
            display_name: "context".to_string(),
            command_template: "/debug context".to_string(),
            requires_args: false,
        }]
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let mut out = String::from("HADES SMART CONTEXT & TOOL ORCHESTRATION\n\n");
        if let Some(ref plan) = context.last_request_plan {
            out.push_str(&format!("Task Domain:    {}\n", plan.task_domain));
            out.push_str(&format!("Selection Tier: Tier {}\n", plan.selection_tier));
            out.push_str(&format!("Reasoning:      {}\n\n", plan.reasoning));
            out.push_str("Token Budget & Estimates:\n");
            out.push_str(&format!(
                "  Estimated Total:   {} tokens\n",
                plan.estimated_total_tokens
            ));
            out.push_str(&format!(
                "  System Prompt:     {} tokens\n",
                plan.estimated_system_tokens
            ));
            out.push_str(&format!(
                "  Context Messages:  {} tokens\n",
                plan.estimated_context_tokens
            ));
            out.push_str(&format!(
                "  Tool Schemas:      {} tokens\n",
                plan.estimated_tool_tokens
            ));
            out.push_str(&format!(
                "  Available Budget:  {} tokens\n",
                plan.token_budget
            ));
            if let Some(tpm) = plan.provider_tpm_limit {
                out.push_str(&format!("  Provider TPM:      {} TPM\n", tpm));
            }
            out.push_str("\nTool Capability Selection:\n");
            out.push_str(&format!(
                "  Total Available:   {} tools\n",
                plan.available_tools_count
            ));
            out.push_str(&format!(
                "  Selected:          {} tools\n",
                plan.selected_tools.len()
            ));
            for t in &plan.selected_tools {
                out.push_str(&format!("    • {t}\n"));
            }
            out.push_str(&format!(
                "  Excluded:          {} irrelevant tools (schemas omitted)\n",
                plan.excluded_tools_count
            ));
        } else {
            out.push_str("No model request has been executed in the active session yet.\n\n");
            let tool_count = context.tool_registry.map(|r| r.count()).unwrap_or(0);
            out.push_str(&format!("Available Registered Tools: {}\n", tool_count));
            let mcp_count = context.mcp_summaries.len();
            out.push_str(&format!("Configured MCP Servers:     {}\n", mcp_count));
        }

        Ok(CommandOutput::Text(out))
    }
}

/// Command: `/model` (or `/provider`)
pub struct ModelCommand;

impl Command for ModelCommand {
    fn name(&self) -> &'static str {
        "/model"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/provider", "/models"]
    }

    fn description(&self) -> &'static str {
        "Configure default AI model and provider"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        context.request_model_setup();
        Ok(CommandOutput::OpenModelSetup)
    }
}

/// Command: `/switch`
pub struct SwitchCommand;

impl Command for SwitchCommand {
    fn name(&self) -> &'static str {
        "/switch"
    }

    fn description(&self) -> &'static str {
        "Switch active model for current conversation session"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        context.request_model_switch();
        Ok(CommandOutput::OpenModelSwitch)
    }
}

/// Command: `/new`
pub struct NewSessionCommand;

impl Command for NewSessionCommand {
    fn name(&self) -> &'static str {
        "/new"
    }

    fn description(&self) -> &'static str {
        "Start a new conversation session"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        context.request_new_session();
        Ok(CommandOutput::NewSession)
    }
}

/// Command: `/sessions` (or `/history`)
pub struct SessionsCommand;

impl Command for SessionsCommand {
    fn name(&self) -> &'static str {
        "/sessions"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/history"]
    }

    fn description(&self) -> &'static str {
        "List and switch conversation sessions"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        context.request_session_picker();
        Ok(CommandOutput::OpenSessionPicker)
    }
}

/// Command: `/tools`
pub struct ToolsCommand;

impl Command for ToolsCommand {
    fn name(&self) -> &'static str {
        "/tools"
    }

    fn description(&self) -> &'static str {
        "List available tools, capabilities, and risk levels"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let mut output = String::from("HADES TOOLS & CAPABILITIES\n\n");

        if let Some(reg) = context.tool_registry {
            let tools = reg.list();

            let categories = [
                ("FILESYSTEM TOOLS (Workspace-Bound)", "filesystem."),
                ("WORKSPACE TOOLS (Workspace-Bound)", "workspace."),
                ("SHELL & EXECUTION TOOLS", "shell."),
                ("ENVIRONMENT TOOLS (System-Wide)", "environment."),
                (
                    "SYSTEM DIAGNOSTIC & PROCESS TOOLS (System-Wide)",
                    "system.process.",
                ),
                (
                    "SYSTEM DIAGNOSTIC & RUNTIME TOOLS (System-Wide)",
                    "system.runtime.",
                ),
                ("SYSTEM INFORMATION TOOLS (System-Wide)", "system.info"),
                ("NETWORK DIAGNOSTIC TOOLS (System-Wide)", "system.network."),
            ];

            for (cat_name, prefix) in categories {
                let cat_tools: Vec<_> = if cat_name.contains("INFORMATION") {
                    tools
                        .iter()
                        .filter(|t| {
                            t.name == "system.info"
                                || t.name == "system.platform"
                                || t.name == "system.architecture"
                                || t.name == "system.hostname"
                                || t.name == "system.uptime"
                        })
                        .collect()
                } else {
                    tools
                        .iter()
                        .filter(|t| t.name.starts_with(prefix))
                        .collect()
                };

                if !cat_tools.is_empty() {
                    output.push_str(&format!("── {cat_name} ──\n"));
                    for def in cat_tools {
                        let scope = if def.name.starts_with("filesystem.")
                            || def.name.starts_with("workspace.")
                        {
                            "Workspace-Bound"
                        } else {
                            "System-Wide"
                        };

                        let params = if let Some(props) = def
                            .parameters_schema
                            .get("properties")
                            .and_then(|p| p.as_object())
                        {
                            if props.is_empty() {
                                "none".to_string()
                            } else {
                                props.keys().cloned().collect::<Vec<_>>().join(", ")
                            }
                        } else {
                            "none".to_string()
                        };

                        output.push_str(&format!(
                            "  • {:<26} [{:<6} | mut: {:<3} | {}]\n    Params: {}\n    {}\n\n",
                            def.name,
                            def.risk_level.to_string(),
                            if def.is_mutating { "yes" } else { "no" },
                            scope,
                            params,
                            def.description
                        ));
                    }
                }
            }
        } else {
            output.push_str("No active tool registry available.\n");
        }

        Ok(CommandOutput::Text(output))
    }
}

/// Command: `/workspace`
pub struct WorkspaceCommand;

impl Command for WorkspaceCommand {
    fn name(&self) -> &'static str {
        "/workspace"
    }

    fn description(&self) -> &'static str {
        "Display active workspace root, project type, and VCS status"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let mut output = String::from("WORKSPACE OVERVIEW\n\n");
        if let Some(ws) = context.workspace_info {
            output.push_str(&format!("Name:             {}\n", ws.name()));
            output.push_str(&format!("Root Path:        {}\n", ws.root.display()));
            output.push_str(&format!("Working Dir:      {}\n", ws.current_dir.display()));
            output.push_str(&format!("Project Type:     {}\n", ws.project_type));
            if ws.has_git {
                let branch_str = ws.git_branch.as_deref().unwrap_or("detached");
                output.push_str(&format!(
                    "Git VCS:          Initialized (branch: {branch_str})\n"
                ));
            } else {
                output.push_str("Git VCS:          Not initialized\n");
            }
            output.push_str(&format!(
                "Languages:        {}\n",
                ws.detected_languages.join(", ")
            ));
            output.push_str("\nTop-level layout:\n");
            for entry in &ws.top_level_entries {
                output.push_str(&format!("  - {entry}\n"));
            }
        } else {
            output.push_str("No workspace metadata available.\n");
        }

        Ok(CommandOutput::Text(output))
    }
}

/// Command: `/permissions`
pub struct PermissionsCommand;

impl Command for PermissionsCommand {
    fn name(&self) -> &'static str {
        "/permissions"
    }

    fn description(&self) -> &'static str {
        "Display current session authorizations and security policy"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let mut output = String::from("SECURITY & PERMISSION POLICY\n\n");
        output.push_str("Default Policy:\n");
        output.push_str("  - SAFE:     Permitted automatically within workspace\n");
        output.push_str("  - LOW:      Permitted automatically within workspace\n");
        output.push_str("  - MEDIUM:   Requires approval unless granted for session\n");
        output.push_str("  - HIGH:     Requires approval per invocation\n");
        output.push_str("  - CRITICAL: Requires explicit confirmation every time\n\n");

        output.push_str("Session Authorizations (granted via 'Allow Session'):\n");
        if context.session_permissions.is_empty() {
            output.push_str("  (None - standard approval prompts apply)\n");
        } else {
            for perm in &context.session_permissions {
                output.push_str(&format!("  - {perm}\n"));
            }
        }

        Ok(CommandOutput::Text(output))
    }
}

/// Command: `/mcp`
pub struct McpCommand;

impl Command for McpCommand {
    fn name(&self) -> &'static str {
        "/mcp"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/mcps"]
    }

    fn description(&self) -> &'static str {
        "Manage Model Context Protocol (MCP) servers and tools"
    }

    fn subcommands(&self) -> Vec<SubcommandInfo> {
        vec![
            SubcommandInfo {
                name: "add".to_string(),
                display_name: "Add MCP server".to_string(),
                description: "Add and configure an MCP server".to_string(),
                command_template: "/mcp add".to_string(),
                requires_args: false,
            },
            SubcommandInfo {
                name: "remove".to_string(),
                display_name: "Remove MCP server".to_string(),
                description: "Remove an MCP server".to_string(),
                command_template: "/mcp remove ".to_string(),
                requires_args: true,
            },
            SubcommandInfo {
                name: "test".to_string(),
                display_name: "Test MCP server".to_string(),
                description: "Test connection to an MCP server".to_string(),
                command_template: "/mcp test ".to_string(),
                requires_args: true,
            },
            SubcommandInfo {
                name: "list".to_string(),
                display_name: "List MCP servers".to_string(),
                description: "List configured MCP servers and their status".to_string(),
                command_template: "/mcp list".to_string(),
                requires_args: false,
            },
        ]
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let trimmed = context.raw_input.trim();
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();

        // Parse subcommands: /mcp [add|remove|test|list]
        if tokens.len() > 1 {
            match tokens[1].to_lowercase().as_str() {
                "add" => {
                    return Ok(CommandOutput::OpenMcpSetup);
                }
                "remove" => {
                    if tokens.len() < 3 {
                        return Ok(CommandOutput::Text(
                            "Usage: /mcp remove <server_name>\nExample: /mcp remove github"
                                .to_string(),
                        ));
                    }
                    let server_name = tokens[2];

                    // Check if server exists
                    if !context.mcp_summaries.iter().any(|s| s.name == server_name) {
                        return Ok(CommandOutput::Text(format!(
                            "Error: MCP server '{}' is not configured.",
                            server_name
                        )));
                    }

                    // Return command output for async removal handling
                    return Ok(CommandOutput::RemoveMcpServer(server_name.to_string()));
                }
                "test" => {
                    if tokens.len() < 3 {
                        return Ok(CommandOutput::Text(
                            "Usage: /mcp test <server_name>\nExample: /mcp test github".to_string(),
                        ));
                    }
                    let server_name = tokens[2];

                    if !context.mcp_summaries.iter().any(|s| s.name == server_name) {
                        return Ok(CommandOutput::Text(format!(
                            "Error: MCP server '{}' is not configured.",
                            server_name
                        )));
                    }

                    return Ok(CommandOutput::TestMcpServer(server_name.to_string()));
                }
                _ => {
                    // Fall through to list behavior
                }
            }
        }

        // Default: list all servers
        let mut output = String::from("MODEL CONTEXT PROTOCOL (MCP) SERVERS\n\n");

        if context.mcp_summaries.is_empty() {
            output.push_str("No MCP servers configured.\n\n");
            output.push_str("To add an MCP server interactively, use: /mcp add\n\n");
            output.push_str("Or configure in your config.toml (~/.hades/config.toml):\n");
            output.push_str("  [mcp.servers.github]\n");
            output.push_str("  transport = \"stdio\"\n");
            output.push_str("  command = \"npx\"\n");
            output.push_str("  args = [\"-y\", \"@modelcontextprotocol/server-github\"]\n");
            output.push_str("  token_env = \"GITHUB_TOKEN\"\n");
            return Ok(CommandOutput::Text(output));
        }

        output.push_str(&format!(
            "  {:<16} {:<12} {:<10} {:<8} {:<10} {}\n",
            "SERVER", "STATUS", "TRANSPORT", "TOOLS", "RESOURCES", "DIAGNOSTICS"
        ));
        output.push_str(&format!("  {}\n", "─".repeat(70)));

        for s in &context.mcp_summaries {
            let status_str = match &s.state {
                hades_mcp::McpServerState::Ready => "READY",
                hades_mcp::McpServerState::Connected => "CONNECTED",
                hades_mcp::McpServerState::Starting => "STARTING",
                hades_mcp::McpServerState::Configured => "CONFIGURED",
                hades_mcp::McpServerState::Disconnected => "DISCONNECTED",
                hades_mcp::McpServerState::Failed(_) => "FAILED",
                hades_mcp::McpServerState::Stopping => "STOPPING",
                hades_mcp::McpServerState::Stopped => "STOPPED",
            };

            let diag = s.error.as_deref().unwrap_or("ok");

            output.push_str(&format!(
                "  {:<16} {:<12} {:<10} {:<8} {:<10} {}\n",
                s.name, status_str, s.transport, s.tool_count, s.resource_count, diag
            ));
        }

        output.push_str("\nDiscovered MCP Tools:\n");
        if let Some(reg) = context.tool_registry {
            let mcp_tools: Vec<_> = reg
                .list()
                .into_iter()
                .filter(|t| t.name.contains('.'))
                .collect();
            if mcp_tools.is_empty() {
                output.push_str("  (No tools registered from active MCP servers)\n");
            } else {
                for t in mcp_tools {
                    output.push_str(&format!(
                        "  • {:<30} [{:<6}] {}\n",
                        t.name,
                        t.risk_level.to_string(),
                        t.description
                    ));
                }
            }
        }

        output.push_str("\nUsage:\n");
        output.push_str("  /mcp list         Show configured MCP servers\n");
        output.push_str("  /mcp add          Add a new MCP server interactively\n");
        output.push_str("  /mcp remove <name> Remove an MCP server\n");
        output.push_str("  /mcp test <name>  Test connection to MCP server\n");

        Ok(CommandOutput::Text(output))
    }
}

/// Command: `/notify` (or `/sound`, `/notifications`)
pub struct NotifyCommand;

impl Command for NotifyCommand {
    fn name(&self) -> &'static str {
        "/notify"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/sound", "/notifications"]
    }

    fn description(&self) -> &'static str {
        "Inspect and test sound and desktop notification configuration"
    }

    fn subcommands(&self) -> Vec<SubcommandInfo> {
        vec![
            SubcommandInfo {
                name: "test".to_string(),
                display_name: "Test alert".to_string(),
                description: "Dispatch test audio chimes and desktop notification".to_string(),
                command_template: "/notify test".to_string(),
                requires_args: false,
            },
            SubcommandInfo {
                name: "enable".to_string(),
                display_name: "Enable notifications".to_string(),
                description: "Enable audio chimes and system notifications".to_string(),
                command_template: "/notify enable".to_string(),
                requires_args: false,
            },
            SubcommandInfo {
                name: "disable".to_string(),
                display_name: "Disable notifications".to_string(),
                description: "Disable all audio chimes and notifications".to_string(),
                command_template: "/notify disable".to_string(),
                requires_args: false,
            },
        ]
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let n = &context.config.notification;
        let mut output = String::from("NOTIFICATION & SOUND CONFIGURATION\n\n");
        output.push_str(&format!(
            "Master Notifications:  {}\n",
            if n.enabled { "Enabled" } else { "Disabled" }
        ));
        output.push_str(&format!(
            "Audio Sounds:          {}\n",
            if n.sound_enabled {
                "Enabled"
            } else {
                "Disabled"
            }
        ));
        output.push_str(&format!(
            "Desktop Popups:        {}\n",
            if n.desktop_enabled {
                "Enabled"
            } else {
                "Disabled"
            }
        ));
        output.push_str(&format!(
            "Notify Input Required: {}\n",
            if n.notify_on_input_required {
                "Yes"
            } else {
                "No"
            }
        ));
        output.push_str(&format!(
            "Notify Task Completed: {}\n",
            if n.notify_on_task_completed {
                "Yes"
            } else {
                "No"
            }
        ));
        output.push_str(&format!(
            "Notify On Error:       {}\n",
            if n.notify_on_error { "Yes" } else { "No" }
        ));
        output.push_str(&format!("Sound Theme:           {}\n\n", n.sound_theme));

        if n.enabled {
            output
                .push_str("🔔 Dispatched test audio sound chimes & desktop notification alert.\n");
            let config_clone = n.clone();
            std::thread::spawn(move || {
                let service = crate::notification::NotificationService::new(config_clone, None);
                service.notify(
                    crate::notification::NotificationKind::InputRequired,
                    "Input Required Alert",
                    "Hades CLI is waiting for user action.",
                );
                std::thread::sleep(std::time::Duration::from_millis(500));
                service.notify(
                    crate::notification::NotificationKind::TaskCompleted,
                    "Task Completed Alert",
                    "Hades CLI task completed successfully.",
                );
            });
        } else {
            output.push_str("ℹ️ Notifications/sounds are currently disabled in configuration.\n");
        }

        Ok(CommandOutput::Text(output))
    }
}

/// Command: `/exit`
pub struct ExitCommand;

impl Command for ExitCommand {
    fn name(&self) -> &'static str {
        "/exit"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/quit", "/q"]
    }

    fn description(&self) -> &'static str {
        "Exit Hades"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        context.request_shutdown();
        Ok(CommandOutput::Exit)
    }
}

/// Command: `/agents`
pub struct AgentsCommand;

impl Command for AgentsCommand {
    fn name(&self) -> &'static str {
        "/agents"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/agent", "/subagents", "/team"]
    }

    fn description(&self) -> &'static str {
        "Inspect specialized collaborative subagents and orchestration status"
    }

    fn subcommands(&self) -> Vec<SubcommandInfo> {
        vec![SubcommandInfo {
            name: "plan".to_string(),
            display_name: "Plan execution".to_string(),
            description: "Formulate and inspect a multi-agent plan".to_string(),
            command_template: "/agents plan ".to_string(),
            requires_args: true,
        }]
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let trimmed = context.raw_input.trim();
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();

        if tokens.len() > 1 && tokens[1].eq_ignore_ascii_case("plan") {
            let objective = tokens[2..].join(" ");
            if objective.is_empty() {
                return Ok(CommandOutput::Text(
                    "Usage: /agents plan <objective>\nExample: /agents plan Audit dependencies and implement security fix"
                        .to_string(),
                ));
            }
            let decision = hades_agent::DecisionEngine::evaluate(&objective, true);
            let plan = hades_agent::DecisionEngine::build_plan(&objective, &decision);
            let mut out = format!(
                "MULTI-AGENT EXECUTION PLAN\n\nObjective: {}\nStrategy:  {}\nReason:    {}\n\nProposed Subtasks:\n",
                objective,
                decision.strategy,
                decision.reason
            );
            if let Some(p) = plan {
                for (i, t) in p.tasks.iter().enumerate() {
                    let deps = if t.dependencies.is_empty() {
                        "none".to_string()
                    } else {
                        t.dependencies.join(", ")
                    };
                    out.push_str(&format!(
                        "  {}. [{}] {}\n     Role: {}\n     Dependencies: {}\n\n",
                        i + 1,
                        t.id,
                        t.title,
                        t.assigned_role.name(),
                        deps
                    ));
                }
            } else {
                out.push_str(
                    "  (Direct single-agent execution recommended - no subagents needed)\n",
                );
            }
            return Ok(CommandOutput::Text(out));
        }

        let mut output = String::from("SPECIALIST SUBAGENTS & ORCHESTRATION ROLES\n\n");
        output.push_str(&format!(
            "  {:<20} {:<8} {:<10} {}\n",
            "ROLE", "MUTATING", "TIMEOUT", "RESPONSIBILITY & SPECIALIZATION"
        ));
        output.push_str(&format!("  {}\n", "─".repeat(80)));

        let roles = vec![
            hades_agent::AgentRole::Planner,
            hades_agent::AgentRole::Explorer,
            hades_agent::AgentRole::Researcher,
            hades_agent::AgentRole::Analyst,
            hades_agent::AgentRole::Implementer,
            hades_agent::AgentRole::Reviewer,
            hades_agent::AgentRole::Tester,
            hades_agent::AgentRole::Debugger,
            hades_agent::AgentRole::SecurityReviewer,
            hades_agent::AgentRole::FileInvestigator,
            hades_agent::AgentRole::SystemInvestigator,
            hades_agent::AgentRole::GeneralSpecialist,
        ];

        for role in roles {
            output.push_str(&format!(
                "  {:<20} {:<8} {:<10} {}\n",
                role.name(),
                if role.is_mutating_allowed() {
                    "Yes"
                } else {
                    "No"
                },
                format!("{}s", role.default_timeout_secs()),
                role.description()
            ));
        }

        output.push_str("\nSupported Orchestration Strategies:\n");
        output.push_str(
            "  - Direct:           Single primary agent execution (zero subagent overhead)\n",
        );
        output.push_str("  - Sequential:       Linear dependent subtask execution\n");
        output.push_str(
            "  - Parallel:         Concurrent execution of independent tasks (max 4 concurrent)\n",
        );
        output.push_str(
            "  - Plan & Execute:   Upfront planning -> Subtask execution -> Primary synthesis\n",
        );
        output.push_str(
            "  - Review & Refine:  Implementation -> Independent peer audit -> Primary synthesis\n\n",
        );
        output.push_str("Commands:\n");
        output.push_str("  /agents                   List available roles & strategies\n");
        output.push_str("  /agents plan <objective>  Formulate and inspect a multi-agent plan\n");

        Ok(CommandOutput::Text(output))
    }
}

/// Command to inspect and manage headless browser sidecar and web retrieval.
pub struct BrowserCommand;

impl Command for BrowserCommand {
    fn name(&self) -> &'static str {
        "/browser"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/web"]
    }

    fn description(&self) -> &'static str {
        "Inspect browser automation state, active tabs, and web capabilities (/browser, /web)"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let mut out = String::from("HADES WEB INTELLIGENCE & BROWSER SIDECAR\n\n");
        if let Some(ref status) = context.browser_status {
            out.push_str(&format!("  Browser Engine:     {}\n", status.browser_name));
            out.push_str(&format!("  Version:            {}\n", status.version));
            out.push_str(&format!(
                "  Status:             {}\n",
                if status.is_running {
                    "Running (Active Sidecar)"
                } else {
                    "Idle / Standby"
                }
            ));
            out.push_str(&format!("  Mode:               {}\n", status.mode));
            out.push_str(&format!("  Active Tabs:        {}\n", status.active_tabs));
            if let Some(port) = status.cdp_port {
                out.push_str(&format!("  CDP Port:           {}\n", port));
            }
            if let Some(ref path) = status.binary_path {
                out.push_str(&format!("  Binary Location:    {}\n", path.display()));
            }
        } else {
            out.push_str("  Status:             Idle (Starts automatically on first web/browser tool call)\n");
        }

        out.push_str("\nAvailable Web Retrieval & Automation Capabilities:\n");
        out.push_str("  1. Search Layer:    Fast DuckDuckGo search (web.search)\n");
        out.push_str(
            "  2. Fetch Layer:     Direct HTTP page reading & Markdown conversion (web.fetch)\n",
        );
        out.push_str(
            "  3. Browser Sidecar: Headless Chromium engine (browser.open, browser.snapshot)\n",
        );
        out.push_str(
            "  4. Actions:         Accessibility-first interaction (browser.click, browser.fill)\n",
        );
        out.push_str(
            "  5. Artifacts:       Screenshots & PDF documents (browser.screenshot, browser.pdf)\n",
        );
        out.push_str("  6. Diagnostics:     Console & Network telemetry (browser.console, browser.network)\n\n");
        out.push_str("Usage:\n");
        out.push_str("  /browser                  Show browser and web status\n");
        out.push_str("  /browser status           Show detailed status\n");

        Ok(CommandOutput::Text(out))
    }
}

/// Command: `/export`
pub struct ExportCommand;

impl Command for ExportCommand {
    fn name(&self) -> &'static str {
        "/export"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/save-as"]
    }

    fn description(&self) -> &'static str {
        "Export conversation transcript to Markdown or JSON (/export [format] [filepath])"
    }

    fn subcommands(&self) -> Vec<SubcommandInfo> {
        vec![
            SubcommandInfo {
                name: "md".to_string(),
                display_name: "Export to Markdown".to_string(),
                description: "Export conversation to Markdown transcript".to_string(),
                command_template: "/export md ".to_string(),
                requires_args: false,
            },
            SubcommandInfo {
                name: "json".to_string(),
                display_name: "Export to JSON".to_string(),
                description: "Export conversation to JSON format".to_string(),
                command_template: "/export json ".to_string(),
                requires_args: false,
            },
        ]
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let session = context.active_session.ok_or_else(|| {
            CommandError::ExecutionFailed("No active conversation session to export".to_string())
        })?;

        let tokens: Vec<&str> = context.raw_input.split_whitespace().collect();

        let mut format = ExportFormat::Markdown;
        let mut target_path: Option<PathBuf> = None;

        if tokens.len() > 1 {
            for &token in &tokens[1..] {
                let lower = token.to_lowercase();
                if lower == "md" || lower == "markdown" {
                    format = ExportFormat::Markdown;
                } else if lower == "json" {
                    format = ExportFormat::Json;
                } else {
                    let path = PathBuf::from(token);
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if ext.eq_ignore_ascii_case("json") {
                            format = ExportFormat::Json;
                        } else if ext.eq_ignore_ascii_case("md")
                            || ext.eq_ignore_ascii_case("markdown")
                        {
                            format = ExportFormat::Markdown;
                        }
                    }
                    target_path = Some(path);
                }
            }
        }

        let final_path = target_path.unwrap_or_else(|| {
            let ext = match format {
                ExportFormat::Markdown => "md",
                ExportFormat::Json => "json",
            };
            let safe_title: String = session
                .metadata
                .title
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect();
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            PathBuf::from(format!("hades_export_{safe_title}_{timestamp}.{ext}"))
        });

        let saved = SessionExporter::save_export(session, format, &final_path).map_err(|e| {
            CommandError::ExecutionFailed(format!(
                "Failed to export session to '{}': {e}",
                final_path.display()
            ))
        })?;

        Ok(CommandOutput::ExportSuccess(saved))
    }
}

/// Command: `/import`
pub struct ImportCommand;

impl Command for ImportCommand {
    fn name(&self) -> &'static str {
        "/import"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["/load-session"]
    }

    fn description(&self) -> &'static str {
        "Import conversation transcript from Hades, ChatGPT, Claude, or Markdown (/import <filepath>)"
    }

    fn execute(&self, context: &mut CommandContext) -> Result<CommandOutput, CommandError> {
        let tokens: Vec<&str> = context.raw_input.split_whitespace().collect();

        if tokens.len() < 2 {
            return Ok(CommandOutput::Text(
                "Usage: /import <filepath>\nSupported formats:\n  - Hades JSON (*.json)\n  - OpenAI ChatGPT export (conversations.json)\n  - Anthropic Claude transcript (*.json)\n  - Markdown transcript (*.md)".to_string(),
            ));
        }

        let filepath_str = tokens[1..].join(" ");
        let path = Path::new(&filepath_str);

        if !path.exists() {
            return Err(CommandError::ExecutionFailed(format!(
                "Import file not found: {}",
                path.display()
            )));
        }

        let session = SessionImporter::import_from_file(path).map_err(|e| {
            CommandError::ExecutionFailed(format!(
                "Failed to parse and import session from '{}': {e}",
                path.display()
            ))
        })?;

        Ok(CommandOutput::ImportSuccess(Box::new(session)))
    }
}

/// Extensible command registry storing and dispatching commands.
#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<Arc<dyn Command>>,
    lookup: HashMap<String, usize>,
}

impl CommandRegistry {
    /// Creates an empty `CommandRegistry`.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            lookup: HashMap::new(),
        }
    }

    /// Creates a registry pre-populated with standard default commands (`/help`, `/status`, `/model`, `/switch`, `/new`, `/sessions`, `/tools`, `/workspace`, `/permissions`, `/mcp`, `/agents`, `/browser`, `/export`, `/import`, `/notify`, `/exit`).
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(HelpCommand);
        registry.register(StatusCommand);
        registry.register(DebugCommand);
        registry.register(ModelCommand);
        registry.register(SwitchCommand);
        registry.register(NewSessionCommand);
        registry.register(SessionsCommand);
        registry.register(ToolsCommand);
        registry.register(WorkspaceCommand);
        registry.register(PermissionsCommand);
        registry.register(McpCommand);
        registry.register(AgentsCommand);
        registry.register(BrowserCommand);
        registry.register(ExportCommand);
        registry.register(ImportCommand);
        registry.register(NotifyCommand);
        registry.register(ExitCommand);
        registry
    }

    /// Registers a new command into the registry.
    pub fn register<C: Command + 'static>(&mut self, command: C) {
        let idx = self.commands.len();
        let cmd_arc = Arc::new(command);

        self.lookup.insert(cmd_arc.name().to_lowercase(), idx);
        for alias in cmd_arc.aliases() {
            self.lookup.insert(alias.to_lowercase(), idx);
        }

        self.commands.push(cmd_arc);
    }

    /// Finds a command by name or alias.
    pub fn find(&self, name_or_alias: &str) -> Option<Arc<dyn Command>> {
        let key = name_or_alias.trim().to_lowercase();
        self.lookup
            .get(&key)
            .map(|&idx| Arc::clone(&self.commands[idx]))
    }

    /// Lists all unique registered commands in order of registration.
    pub fn list(&self) -> Vec<CommandInfo> {
        self.commands
            .iter()
            .map(|cmd| CommandInfo {
                name: cmd.name().to_string(),
                aliases: cmd.aliases().iter().map(|s| s.to_string()).collect(),
                description: cmd.description().to_string(),
                subcommands: cmd.subcommands(),
            })
            .collect()
    }

    /// Filters available commands and subcommands based on user input and optional active parent.
    pub fn filter_palette(
        &self,
        input: &str,
        active_subcommand_parent: Option<&str>,
    ) -> Vec<PaletteItem> {
        let trimmed = input.trim();

        // 1. If in hierarchical subcommand mode for a specific parent command:
        if let Some(parent_key) = active_subcommand_parent {
            if let Some(parent_cmd) = self.find(parent_key) {
                let subcommands = parent_cmd.subcommands();
                if trimmed.is_empty() || trimmed == parent_cmd.name() {
                    return subcommands
                        .into_iter()
                        .map(|sub| PaletteItem {
                            display_name: sub.display_name,
                            description: sub.description,
                            execution_text: sub.command_template,
                            is_subcommand: true,
                            has_subcommands: false,
                            parent_command: Some(parent_cmd.name().to_string()),
                            requires_args: sub.requires_args,
                        })
                        .collect();
                }

                let query = if trimmed.starts_with(parent_cmd.name()) {
                    trimmed[parent_cmd.name().len()..].trim().to_lowercase()
                } else {
                    trimmed.trim_start_matches('/').to_lowercase()
                };

                let prefix_matches: Vec<PaletteItem> = subcommands
                    .iter()
                    .filter(|sub| {
                        query.is_empty()
                            || sub.name.to_lowercase().starts_with(&query)
                            || sub
                                .display_name
                                .to_lowercase()
                                .split_whitespace()
                                .any(|w| w.starts_with(&query))
                    })
                    .map(|sub| PaletteItem {
                        display_name: sub.display_name.clone(),
                        description: sub.description.clone(),
                        execution_text: sub.command_template.clone(),
                        is_subcommand: true,
                        has_subcommands: false,
                        parent_command: Some(parent_cmd.name().to_string()),
                        requires_args: sub.requires_args,
                    })
                    .collect();

                if !prefix_matches.is_empty() {
                    return prefix_matches;
                }

                let fallback_matches: Vec<PaletteItem> = subcommands
                    .into_iter()
                    .filter(|sub| {
                        sub.name.to_lowercase().contains(&query)
                            || sub.display_name.to_lowercase().contains(&query)
                            || sub.command_template.to_lowercase().contains(&query)
                    })
                    .map(|sub| PaletteItem {
                        display_name: sub.display_name,
                        description: sub.description,
                        execution_text: sub.command_template,
                        is_subcommand: true,
                        has_subcommands: false,
                        parent_command: Some(parent_cmd.name().to_string()),
                        requires_args: sub.requires_args,
                    })
                    .collect();

                return fallback_matches;
            }
        }

        // 2. Normal / Top-level filtering:
        if trimmed.is_empty() || trimmed == "/" {
            return self
                .commands
                .iter()
                .map(|cmd| PaletteItem {
                    display_name: cmd.name().to_string(),
                    description: cmd.description().to_string(),
                    execution_text: cmd.name().to_string(),
                    is_subcommand: false,
                    has_subcommands: !cmd.subcommands().is_empty(),
                    parent_command: None,
                    requires_args: false,
                })
                .collect();
        }

        let query = trimmed.to_lowercase();
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();

        // 3. Check if first token matches a command that has subcommands:
        if !tokens.is_empty() {
            let first_token = tokens[0];
            if let Some(cmd) = self.find(first_token) {
                let subcommands = cmd.subcommands();
                if !subcommands.is_empty() && (tokens.len() > 1 || input.ends_with(' ')) {
                    let sub_token = tokens.get(1).copied().unwrap_or("");
                    let sub_query = sub_token.to_lowercase();

                    if tokens.len() > 2 {
                        let desc = subcommands
                            .iter()
                            .find(|s| s.name.eq_ignore_ascii_case(sub_token))
                            .map(|s| s.description.clone())
                            .unwrap_or_else(|| format!("Execute: {}", trimmed));

                        return vec![PaletteItem {
                            display_name: trimmed.to_string(),
                            description: desc,
                            execution_text: trimmed.to_string(),
                            is_subcommand: true,
                            has_subcommands: false,
                            parent_command: Some(cmd.name().to_string()),
                            requires_args: false,
                        }];
                    }

                    let matching_subs: Vec<PaletteItem> = subcommands
                        .iter()
                        .filter(|sub| {
                            sub_query.is_empty()
                                || sub.name.to_lowercase().starts_with(&sub_query)
                                || sub
                                    .display_name
                                    .to_lowercase()
                                    .split_whitespace()
                                    .any(|w| w.starts_with(&sub_query))
                        })
                        .map(|sub| PaletteItem {
                            display_name: format!("{} {}", cmd.name(), sub.name),
                            description: sub.description.clone(),
                            execution_text: sub.command_template.clone(),
                            is_subcommand: true,
                            has_subcommands: false,
                            parent_command: Some(cmd.name().to_string()),
                            requires_args: sub.requires_args,
                        })
                        .collect();

                    if !matching_subs.is_empty() {
                        return matching_subs;
                    }

                    return vec![PaletteItem {
                        display_name: trimmed.to_string(),
                        description: format!("Execute: {}", trimmed),
                        execution_text: trimmed.to_string(),
                        is_subcommand: true,
                        has_subcommands: false,
                        parent_command: Some(cmd.name().to_string()),
                        requires_args: false,
                    }];
                }
            }
        }

        // 4. Prefix match against top-level command names and aliases:
        let mut prefix_matches = Vec::new();
        for cmd in &self.commands {
            let name_lower = cmd.name().to_lowercase();
            let matches_name = name_lower.starts_with(&query);
            let matches_alias = cmd
                .aliases()
                .iter()
                .any(|a| a.to_lowercase().starts_with(&query));

            if matches_name || matches_alias {
                prefix_matches.push(PaletteItem {
                    display_name: cmd.name().to_string(),
                    description: cmd.description().to_string(),
                    execution_text: cmd.name().to_string(),
                    is_subcommand: false,
                    has_subcommands: !cmd.subcommands().is_empty(),
                    parent_command: None,
                    requires_args: false,
                });
            }
        }

        if !prefix_matches.is_empty() {
            return prefix_matches;
        }

        // 5. Fallback: substring matching on name or description
        let clean_query = query.trim_start_matches('/');
        let mut fallback_matches = Vec::new();
        for cmd in &self.commands {
            let name_lower = cmd.name().to_lowercase();
            let desc_lower = cmd.description().to_lowercase();
            if name_lower.contains(clean_query) || desc_lower.contains(clean_query) {
                fallback_matches.push(PaletteItem {
                    display_name: cmd.name().to_string(),
                    description: cmd.description().to_string(),
                    execution_text: cmd.name().to_string(),
                    is_subcommand: false,
                    has_subcommands: !cmd.subcommands().is_empty(),
                    parent_command: None,
                    requires_args: false,
                });
            }
        }

        if !fallback_matches.is_empty() {
            return fallback_matches;
        }

        // 6. If user typed an arbitrary command string starting with '/':
        if trimmed.starts_with('/') {
            vec![PaletteItem {
                display_name: trimmed.to_string(),
                description: format!("Run command: {}", trimmed),
                execution_text: trimmed.to_string(),
                is_subcommand: false,
                has_subcommands: false,
                parent_command: None,
                requires_args: false,
            }]
        } else {
            Vec::new()
        }
    }

    /// Formats help entries for all registered commands.
    pub fn help_entries(&self) -> Vec<HelpEntry> {
        self.commands
            .iter()
            .map(|cmd| HelpEntry {
                name: cmd.name().to_string(),
                description: cmd.description().to_string(),
            })
            .collect()
    }

    /// Parses input, finds the matching command, and executes it.
    pub fn execute(
        &self,
        input: &str,
        context: &mut CommandContext,
    ) -> Result<CommandOutput, CommandError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(CommandError::EmptyInput);
        }

        // Extract command name (first word)
        let command_token = trimmed.split_whitespace().next().unwrap_or("");

        match self.find(command_token) {
            Some(cmd) => cmd.execute(context),
            None => Err(CommandError::UnknownCommand(command_token.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_palette_all_commands_on_empty_and_slash() {
        let registry = CommandRegistry::with_defaults();
        let items_empty = registry.filter_palette("", None);
        let items_slash = registry.filter_palette("/", None);

        assert_eq!(items_empty.len(), items_slash.len());
        assert!(items_empty.iter().any(|i| i.execution_text == "/mcp"));
        assert!(items_empty.iter().any(|i| i.execution_text == "/help"));
        assert!(items_empty.iter().any(|i| i.execution_text == "/exit"));
    }

    #[test]
    fn test_filter_palette_prefix_matching() {
        let registry = CommandRegistry::with_defaults();

        // Typing /m matches /model and /mcp
        let items_m = registry.filter_palette("/m", None);
        assert!(items_m.iter().any(|i| i.execution_text == "/mcp"));
        assert!(items_m.iter().any(|i| i.execution_text == "/model"));

        // Typing /mc matches only /mcp
        let items_mc = registry.filter_palette("/mc", None);
        assert_eq!(items_mc.len(), 1);
        assert_eq!(items_mc[0].execution_text, "/mcp");
        assert!(items_mc[0].has_subcommands);

        // Typing /mcp matches /mcp
        let items_mcp = registry.filter_palette("/mcp", None);
        assert_eq!(items_mcp[0].execution_text, "/mcp");
    }

    #[test]
    fn test_filter_palette_subcommand_inline_filtering() {
        let registry = CommandRegistry::with_defaults();

        // Typing /mcp a filters to /mcp add
        let items_a = registry.filter_palette("/mcp a", None);
        assert_eq!(items_a.len(), 1);
        assert_eq!(items_a[0].execution_text, "/mcp add");
        assert_eq!(items_a[0].display_name, "/mcp add");
        assert!(!items_a[0].requires_args);

        // Typing /mcp r filters to /mcp remove
        let items_r = registry.filter_palette("/mcp r", None);
        assert_eq!(items_r.len(), 1);
        assert_eq!(items_r[0].execution_text, "/mcp remove ");
        assert!(items_r[0].requires_args);

        // Typing /mcp space lists all 4 subcommands
        let items_space = registry.filter_palette("/mcp ", None);
        assert_eq!(items_space.len(), 4);
    }

    #[test]
    fn test_filter_palette_hierarchical_subcommand_mode() {
        let registry = CommandRegistry::with_defaults();

        // When user enters hierarchical mode for /mcp:
        let subs = registry.filter_palette("/mcp", Some("/mcp"));
        assert_eq!(subs.len(), 4);
        assert_eq!(subs[0].display_name, "Add MCP server");
        assert_eq!(subs[0].execution_text, "/mcp add");
        assert_eq!(subs[1].display_name, "Remove MCP server");
        assert_eq!(subs[2].display_name, "Test MCP server");
        assert_eq!(subs[3].display_name, "List MCP servers");

        // Filtering within hierarchical mode:
        let subs_add = registry.filter_palette("add", Some("/mcp"));
        assert_eq!(subs_add.len(), 1);
        assert_eq!(subs_add[0].display_name, "Add MCP server");
    }

    #[test]
    fn test_filter_palette_arguments_typing() {
        let registry = CommandRegistry::with_defaults();

        // Direct typing of command with arguments
        let items_remove = registry.filter_palette("/mcp remove github", None);
        assert_eq!(items_remove.len(), 1);
        assert_eq!(items_remove[0].execution_text, "/mcp remove github");

        let items_test = registry.filter_palette("/mcp test github", None);
        assert_eq!(items_test.len(), 1);
        assert_eq!(items_test[0].execution_text, "/mcp test github");
    }
}
