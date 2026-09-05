use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// High-level lifecycle and interactive workflow states of the Hades application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AppState {
    /// Initializing application subsystems (config, storage, events, models).
    Startup,

    /// Normal interactive chat/execution state.
    Running,

    /// Interactive command palette overlay is open and capturing input.
    CommandPalette,

    /// Interactive session selection / switcher screen.
    SessionSelect,

    /// Modal input dialog to rename the selected session.
    SessionRename,

    /// Modal confirmation dialog to delete the selected session.
    SessionDeleteConfirm,

    /// Interactive provider selection screen.
    ProviderSelect,

    /// Interactive model selection screen for chosen provider.
    ModelSelect,

    /// Model information and capability overview screen.
    ModelInfo,

    /// Secure, masked credential (API key / endpoint) input screen.
    CredentialInput,

    /// Asynchronous provider authentication and model verification in progress.
    Verifying,

    /// Verification failed; displaying actionable diagnostic error screen.
    VerificationFailed,
    /// Interactive MCP server setup workflow.
    McpSetup,
    /// AI generation in progress, awaiting first token chunk.
    AiThinking,

    /// AI streaming generation in progress, actively receiving tokens.
    AiStreaming,

    /// Explicit user authorization required for tool execution.
    ToolApproval,

    /// Interactive conversation turn selection and clipboard copy mode.
    CopySelect,

    /// Graceful shutdown in progress (flushing state, releasing resources).
    ShuttingDown,

    /// Final terminated state; event loop should exit.
    Exited,
}

impl AppState {
    /// Validates whether transitioning from `self` to `target` is allowed.
    pub fn can_transition_to(&self, target: AppState) -> bool {
        match (self, target) {
            // From Startup
            (AppState::Startup, AppState::Running) => true,
            (AppState::Startup, AppState::ProviderSelect) => true,
            (AppState::Startup, AppState::ShuttingDown) => true,

            // From Running
            (AppState::Running, AppState::CommandPalette) => true,
            (AppState::Running, AppState::SessionSelect) => true,
            (AppState::Running, AppState::ProviderSelect) => true,
            (AppState::Running, AppState::McpSetup) => true,
            (AppState::Running, AppState::AiThinking) => true,
            (AppState::Running, AppState::AiStreaming) => true,
            (AppState::Running, AppState::ToolApproval) => true,
            (AppState::Running, AppState::CopySelect) => true,
            (AppState::Running, AppState::ShuttingDown) => true,

            // From CopySelect
            (AppState::CopySelect, AppState::Running) => true,
            (AppState::CopySelect, AppState::ShuttingDown) => true,

            // From CommandPalette
            (AppState::CommandPalette, AppState::Running) => true,
            (AppState::CommandPalette, AppState::SessionSelect) => true,
            (AppState::CommandPalette, AppState::ProviderSelect) => true,
            (AppState::CommandPalette, AppState::McpSetup) => true,
            (AppState::CommandPalette, AppState::ShuttingDown) => true,

            // From McpSetup
            (AppState::McpSetup, AppState::Running) => true,
            (AppState::McpSetup, AppState::CommandPalette) => true,
            (AppState::McpSetup, AppState::ShuttingDown) => true,

            // From SessionSelect
            (AppState::SessionSelect, AppState::Running) => true,
            (AppState::SessionSelect, AppState::SessionRename) => true,
            (AppState::SessionSelect, AppState::SessionDeleteConfirm) => true,
            (AppState::SessionSelect, AppState::CommandPalette) => true,
            (AppState::SessionSelect, AppState::ShuttingDown) => true,

            // From SessionRename
            (AppState::SessionRename, AppState::SessionSelect) => true,
            (AppState::SessionRename, AppState::Running) => true,
            (AppState::SessionRename, AppState::ShuttingDown) => true,

            // From SessionDeleteConfirm
            (AppState::SessionDeleteConfirm, AppState::SessionSelect) => true,
            (AppState::SessionDeleteConfirm, AppState::Running) => true,
            (AppState::SessionDeleteConfirm, AppState::ShuttingDown) => true,

            // From ProviderSelect
            (AppState::ProviderSelect, AppState::Running) => true,
            (AppState::ProviderSelect, AppState::ModelSelect) => true,
            (AppState::ProviderSelect, AppState::SessionSelect) => true,
            (AppState::ProviderSelect, AppState::CommandPalette) => true,
            (AppState::ProviderSelect, AppState::ShuttingDown) => true,

            // From ModelSelect
            (AppState::ModelSelect, AppState::ProviderSelect) => true,
            (AppState::ModelSelect, AppState::ModelInfo) => true,
            (AppState::ModelSelect, AppState::CredentialInput) => true,
            (AppState::ModelSelect, AppState::Running) => true,
            (AppState::ModelSelect, AppState::ShuttingDown) => true,

            // From ModelInfo
            (AppState::ModelInfo, AppState::ModelSelect) => true,
            (AppState::ModelInfo, AppState::CredentialInput) => true,
            (AppState::ModelInfo, AppState::Verifying) => true,
            (AppState::ModelInfo, AppState::Running) => true,
            (AppState::ModelInfo, AppState::ShuttingDown) => true,

            // From CredentialInput
            (AppState::CredentialInput, AppState::ModelInfo) => true,
            (AppState::CredentialInput, AppState::ModelSelect) => true,
            (AppState::CredentialInput, AppState::Verifying) => true,
            (AppState::CredentialInput, AppState::Running) => true,
            (AppState::CredentialInput, AppState::ShuttingDown) => true,

            // From Verifying
            (AppState::Verifying, AppState::Running) => true,
            (AppState::Verifying, AppState::VerificationFailed) => true,
            (AppState::Verifying, AppState::ShuttingDown) => true,

            // From VerificationFailed
            (AppState::VerificationFailed, AppState::Verifying) => true,
            (AppState::VerificationFailed, AppState::CredentialInput) => true,
            (AppState::VerificationFailed, AppState::ProviderSelect) => true,
            (AppState::VerificationFailed, AppState::ModelSelect) => true,
            (AppState::VerificationFailed, AppState::Running) => true,
            (AppState::VerificationFailed, AppState::ShuttingDown) => true,

            // From AiThinking
            (AppState::AiThinking, AppState::AiStreaming) => true,
            (AppState::AiThinking, AppState::ToolApproval) => true,
            (AppState::AiThinking, AppState::Running) => true,
            (AppState::AiThinking, AppState::ShuttingDown) => true,

            // From AiStreaming
            (AppState::AiStreaming, AppState::ToolApproval) => true,
            (AppState::AiStreaming, AppState::Running) => true,
            (AppState::AiStreaming, AppState::ShuttingDown) => true,

            // From ToolApproval
            (AppState::ToolApproval, AppState::Running) => true,
            (AppState::ToolApproval, AppState::AiThinking) => true,
            (AppState::ToolApproval, AppState::AiStreaming) => true,
            (AppState::ToolApproval, AppState::ShuttingDown) => true,

            // From ShuttingDown
            (AppState::ShuttingDown, AppState::Exited) => true,

            // Self transitions are no-ops / allowed
            (a, b) if *a == b => true,

            // All other transitions are invalid
            _ => false,
        }
    }

    /// Verifies and returns an error if the transition is illegal.
    pub fn check_transition(&self, target: AppState) -> Result<(), CoreError> {
        if self.can_transition_to(target) {
            Ok(())
        } else {
            Err(CoreError::InvalidStateTransition {
                from: *self,
                to: target,
            })
        }
    }
}
