use hades_core::CommandOutput;
use hades_provider::{Model, ProviderMetadata, Usage};
use hades_storage::{MessageRole, SessionMetadata, SessionRecord};

/// Represents a single chronological turn in the conversation stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurn {
    /// User submitted prompt text.
    pub user_prompt: String,
    /// Progressive streamed or finalized assistant response text.
    pub assistant_response: Option<String>,
    /// Transient activity status text (e.g. "Thinking...", "Working..."). Cleared when streaming begins.
    pub activity_text: Option<String>,
    /// Diagnostic error message if turn execution failed.
    pub error_text: Option<String>,
}

impl ChatTurn {
    /// Constructs a new active turn with initial activity indicator.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            user_prompt: prompt.into(),
            assistant_response: None,
            activity_text: Some("Thinking...".to_string()),
            error_text: None,
        }
    }

    /// Constructs a completed turn with an established response.
    pub fn with_response(prompt: impl Into<String>, response: impl Into<String>) -> Self {
        Self {
            user_prompt: prompt.into(),
            assistant_response: Some(response.into()),
            activity_text: None,
            error_text: None,
        }
    }

    /// Appends delta text to the assistant response and clears transient activity text.
    pub fn append_response_chunk(&mut self, chunk: &str) {
        self.activity_text = None;
        let response = self.assistant_response.get_or_insert_with(String::new);
        response.push_str(chunk);
    }

    /// Sets a full response and clears transient activity text.
    pub fn set_response(&mut self, response: impl Into<String>) {
        self.activity_text = None;
        self.assistant_response = Some(response.into());
    }

    /// Records an error for this turn and clears transient activity text.
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.activity_text = None;
        self.error_text = Some(error.into());
    }

    /// Sets a transient activity text.
    pub fn set_activity(&mut self, activity: impl Into<String>) {
        self.activity_text = Some(activity.into());
    }

    /// Clears any transient activity text.
    pub fn clear_activity(&mut self) {
        self.activity_text = None;
    }

    /// Extracts clean, unformatted text for copying (without ANSI codes, UI branches or borders).
    pub fn clean_text(&self) -> String {
        let mut out = String::new();
        out.push_str("You:\n");
        out.push_str(&self.user_prompt);
        out.push_str("\n\n");
        out.push_str("Hades:\n");
        if let Some(ref resp) = self.assistant_response {
            out.push_str(resp);
        } else if let Some(ref err) = self.error_text {
            out.push_str("Error: ");
            out.push_str(err);
        }
        out
    }

    /// Extracts clean assistant response text only.
    pub fn clean_response_text(&self) -> Option<String> {
        self.assistant_response.clone()
    }
}

/// Transient UI view state for rendering, scrolling, and input handling.
#[derive(Debug, Clone, Default)]
pub struct TuiState {
    /// Active command output message displayed in main viewport.
    pub active_output: Option<CommandOutput>,

    /// Global error message banner, if any.
    pub error_message: Option<String>,

    /// Currently highlighted index within the command palette.
    pub selected_palette_index: usize,

    /// Optional parent command if currently in a second-level subcommand selection menu.
    pub active_subcommand_parent: Option<String>,

    /// Vertical scroll offset in the command palette when items overflow visible area.
    pub palette_scroll_offset: usize,

    /// User prompt input buffer for chat in Running state.
    pub prompt_input: String,

    /// Cursor position in prompt input.
    pub prompt_cursor_position: usize,

    /// Chronological list of conversation turns.
    pub turns: Vec<ChatTurn>,

    /// Number of rendered lines scrolled from top.
    pub scroll_offset: usize,

    /// Height of conversation area in rows.
    pub viewport_height: usize,

    /// Total number of rendered wrapped lines in the conversation stream.
    pub content_height: usize,

    /// Whether the conversation view automatically follows newly arrived streaming tokens.
    pub auto_scroll_to_bottom: bool,

    /// Whether new content has arrived while the user is scrolled away from the bottom.
    pub has_new_content_below: bool,

    /// Frame index for subtle activity spinner animation.
    pub spinner_frame: usize,

    /// Token usage metrics for the latest generation.
    pub current_usage: Option<Usage>,

    // Session Management Fields
    /// List of available sessions for the SessionSelect modal.
    pub sessions: Vec<SessionMetadata>,

    /// Selected session index in SessionSelect modal.
    pub selected_session_index: usize,

    /// Whether the active setup workflow was triggered by /switch rather than /model.
    pub is_model_switch_flow: bool,

    /// Session ID being renamed.
    pub rename_session_id: Option<String>,

    /// Buffer holding the new session title during rename.
    pub rename_input: String,

    /// Cursor position within rename input.
    pub rename_cursor_position: usize,

    /// Session ID targeted for deletion.
    pub delete_session_id: Option<String>,

    /// Session title targeted for deletion display.
    pub delete_session_title: String,

    /// Action index in delete confirmation modal (0 = Delete, 1 = Cancel).
    pub delete_confirm_action: usize,

    // Provider Setup Workflow Fields
    /// List of available AI providers.
    pub providers: Vec<ProviderMetadata>,

    /// Currently selected provider index in `ProviderSelect` screen.
    pub selected_provider_index: usize,

    /// List of discovered or supported models for the chosen provider.
    pub models: Vec<Model>,

    /// Currently selected model index in `ModelSelect` screen.
    pub selected_model_index: usize,

    /// Selected model being inspected on the `ModelInfo` screen.
    pub selected_model: Option<Model>,

    /// Plaintext credential input buffer (displayed as masked `******`).
    pub credential_input: String,

    /// Cursor position in credential input.
    pub credential_cursor_position: usize,

    /// Optional custom endpoint override URL.
    pub custom_endpoint_input: String,

    /// Whether editing custom endpoint field instead of API key.
    pub is_editing_endpoint: bool,

    /// Diagnostic error message when verification fails.
    pub verification_error: Option<String>,

    /// Selected action index on `VerificationFailed` screen (0 = Retry, 1 = Change Credential, 2 = Back).
    pub verification_action_index: usize,

    // Tool Approval Modal Fields
    /// Selected button index in ToolApproval modal (0 = Allow Once, 1 = Allow Session, 2 = Deny, 3 = Cancel).
    pub tool_approval_selection: usize,

    // Copy / Selection Mode Fields
    /// Selected turn index in CopySelect mode.
    pub copy_selected_turn_index: usize,

    // MCP Setup Workflow Fields
    /// MCP server name input buffer.
    pub mcp_server_name: String,

    /// MCP server name input cursor position.
    pub mcp_server_cursor_position: usize,

    /// Selected transport type index (0 = STDIO, 1 = HTTP).
    pub mcp_transport_selection: usize,

    /// Command input for STDIO transport.
    pub mcp_command_input: String,

    /// Command input cursor position.
    pub mcp_command_cursor_position: usize,

    /// URL input for HTTP transport.
    pub mcp_url_input: String,

    /// URL input cursor position.
    pub mcp_url_cursor_position: usize,

    /// Command arguments input for STDIO.
    pub mcp_args_input: String,

    /// Args input cursor position.
    pub mcp_args_cursor_position: usize,

    /// Plaintext MCP authentication token, persisted securely outside configuration.
    pub mcp_auth_token_input: String,

    /// Cursor position in the MCP authentication token input.
    pub mcp_auth_token_cursor_position: usize,

    /// Environment variable name for token/auth fallback.
    pub mcp_token_env_input: String,

    /// Token env input cursor position.
    pub mcp_token_env_cursor_position: usize,

    /// Current field being edited in MCP setup (0=name, 1=transport, 2=command/url, 3=args, 4=token, 5=token_env).
    pub mcp_current_field: usize,

    /// Diagnostic error message for MCP setup.
    pub mcp_setup_error: Option<String>,

    /// Ephemeral toast notification banner (text, creation instant).
    pub toast: Option<(String, std::time::Instant)>,
}

impl TuiState {
    /// Creates a new `TuiState` instance initialized with auto-scrolling enabled.
    pub fn new() -> Self {
        Self {
            auto_scroll_to_bottom: true,
            has_new_content_below: false,
            ..Default::default()
        }
    }

    /// Reconstructs UI chat turns from a loaded persistent session record.
    pub fn reconstruct_turns_from_session(&mut self, session: &SessionRecord) {
        self.turns.clear();
        let mut pending_user_prompt: Option<String> = None;

        for msg in &session.messages {
            match msg.role {
                MessageRole::User => {
                    if let Some(prompt) = pending_user_prompt.take() {
                        self.turns.push(ChatTurn::new(prompt));
                    }
                    pending_user_prompt = Some(msg.content.clone());
                }
                MessageRole::Assistant => {
                    let prompt = pending_user_prompt
                        .take()
                        .unwrap_or_else(|| "(Conversation)".to_string());
                    self.turns
                        .push(ChatTurn::with_response(prompt, &msg.content));
                }
                MessageRole::Error => {
                    let prompt = pending_user_prompt
                        .take()
                        .unwrap_or_else(|| "(Conversation)".to_string());
                    let mut turn = ChatTurn::new(prompt);
                    turn.set_error(&msg.content);
                    self.turns.push(turn);
                }
                MessageRole::Tool => {
                    if let Some(last_turn) = self.turns.last_mut() {
                        let tool_formatted = format!("\n\n[Tool Result]\n{}", msg.content);
                        if let Some(resp) = &mut last_turn.assistant_response {
                            resp.push_str(&tool_formatted);
                        } else {
                            last_turn.assistant_response = Some(tool_formatted);
                        }
                    }
                }
                MessageRole::System => {}
            }
        }

        if let Some(prompt) = pending_user_prompt {
            self.turns.push(ChatTurn::new(prompt));
        }

        self.scroll_to_bottom();
    }

    /// Sets an error message to display in the UI.
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error_message = Some(error.into());
    }

    /// Clears any active error message.
    pub fn clear_error(&mut self) {
        self.error_message = None;
    }

    /// Sets active command output and scrolls to show it.
    pub fn set_output(&mut self, output: CommandOutput) {
        self.active_output = Some(output);
        self.error_message = None;
        self.scroll_to_bottom();
    }

    /// Appends a character to the user chat prompt.
    pub fn push_prompt_char(&mut self, c: char) {
        self.prompt_input.insert(self.prompt_cursor_position, c);
        self.prompt_cursor_position += 1;
    }

    /// Removes a character preceding the cursor in the user chat prompt.
    pub fn pop_prompt_char(&mut self) {
        if self.prompt_cursor_position > 0 {
            self.prompt_cursor_position -= 1;
            self.prompt_input.remove(self.prompt_cursor_position);
        }
    }

    /// Removes a character following the cursor in the user chat prompt.
    pub fn delete_prompt_char(&mut self) {
        if self.prompt_cursor_position < self.prompt_input.len() {
            self.prompt_input.remove(self.prompt_cursor_position);
        }
    }

    /// Adjusts command palette scroll offset so the selected item remains visible.
    pub fn adjust_palette_scroll(&mut self, total_items: usize, visible_capacity: usize) {
        if total_items <= visible_capacity || visible_capacity == 0 {
            self.palette_scroll_offset = 0;
            return;
        }
        if self.selected_palette_index < self.palette_scroll_offset {
            self.palette_scroll_offset = self.selected_palette_index;
        } else if self.selected_palette_index >= self.palette_scroll_offset + visible_capacity {
            self.palette_scroll_offset = self.selected_palette_index + 1 - visible_capacity;
        }
        let max_offset = total_items.saturating_sub(visible_capacity);
        if self.palette_scroll_offset > max_offset {
            self.palette_scroll_offset = max_offset;
        }
    }

    /// Appends a character to the credential input buffer.
    pub fn push_credential_char(&mut self, c: char) {
        if self.is_editing_endpoint {
            self.custom_endpoint_input.push(c);
        } else {
            self.credential_input
                .insert(self.credential_cursor_position, c);
            self.credential_cursor_position += 1;
        }
    }

    /// Removes a character from the credential input buffer.
    pub fn pop_credential_char(&mut self) {
        if self.is_editing_endpoint {
            self.custom_endpoint_input.pop();
        } else if self.credential_cursor_position > 0 {
            self.credential_cursor_position -= 1;
            self.credential_input
                .remove(self.credential_cursor_position);
        }
    }

    /// Advances the spinner animation frame.
    pub fn tick_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % 10;
    }

    /// Returns the current spinner character glyph.
    pub fn spinner_char(&self) -> &'static str {
        const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()]
    }

    /// Computes the maximum valid scroll offset given current content and viewport dimensions.
    pub fn max_scroll_offset(&self) -> usize {
        self.content_height.saturating_sub(self.viewport_height)
    }

    /// Updates rendered conversation geometry and synchronizes scroll offset safely.
    pub fn update_geometry(&mut self, content_height: usize, viewport_height: usize) {
        self.content_height = content_height;
        self.viewport_height = viewport_height;

        let max_scroll = self.max_scroll_offset();
        if self.auto_scroll_to_bottom {
            self.scroll_offset = max_scroll;
            self.has_new_content_below = false;
        } else {
            if self.scroll_offset > max_scroll {
                self.scroll_offset = max_scroll;
            }
            self.has_new_content_below = self.scroll_offset < max_scroll;
        }
    }

    /// Legacy alias for update_geometry.
    pub fn update_viewport_dimensions(&mut self, total_lines: usize, height: usize) {
        self.update_geometry(total_lines, height);
    }

    /// Handles arrival of new streaming or turn content.
    pub fn on_new_content(&mut self, total_lines: usize) {
        self.content_height = total_lines;
        let max_scroll = self.max_scroll_offset();
        if self.auto_scroll_to_bottom {
            self.scroll_offset = max_scroll;
            self.has_new_content_below = false;
        } else {
            self.has_new_content_below = self.scroll_offset < max_scroll;
        }
    }

    /// Scrolls the conversation view upward by the specified number of rows.
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.auto_scroll_to_bottom = false;
        self.has_new_content_below = self.scroll_offset < self.max_scroll_offset();
    }

    /// Scrolls the conversation view downward by the specified number of rows.
    pub fn scroll_down(&mut self, lines: usize) {
        let max_scroll = self.max_scroll_offset();
        self.scroll_offset = (self.scroll_offset + lines).min(max_scroll);
        if self.scroll_offset >= max_scroll {
            self.auto_scroll_to_bottom = true;
            self.has_new_content_below = false;
        } else {
            self.has_new_content_below = true;
        }
    }

    /// Scrolls one viewport upward (PageUp).
    pub fn page_up(&mut self) {
        let step = self.viewport_height.saturating_sub(2).max(1);
        self.scroll_up(step);
    }

    /// Scrolls one viewport downward (PageDown).
    pub fn page_down(&mut self) {
        let step = self.viewport_height.saturating_sub(2).max(1);
        self.scroll_down(step);
    }

    /// Scrolls directly to the beginning of the conversation history (Home).
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll_to_bottom = false;
        self.has_new_content_below = self.content_height > self.viewport_height;
    }

    /// Scrolls directly to the latest conversation content at the bottom (End).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_scroll_offset();
        self.auto_scroll_to_bottom = true;
        self.has_new_content_below = false;
    }

    /// Backward compatibility helper returning completed prompt/response pairs.
    pub fn chat_history(&self) -> Vec<(String, String)> {
        self.turns
            .iter()
            .filter_map(|turn| {
                turn.assistant_response
                    .as_ref()
                    .map(|resp| (turn.user_prompt.clone(), resp.clone()))
            })
            .collect()
    }

    /// Sets a temporary toast notification message displayed in the UI.
    pub fn set_toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), std::time::Instant::now()));
    }

    /// Emits a UI toast notification event.
    pub fn show_toast(&mut self, message: impl Into<String>) {
        self.set_toast(message);
    }

    /// Returns the active toast message if within display duration (3 seconds).
    pub fn toast_text(&self) -> Option<&str> {
        if let Some((ref text, instant)) = self.toast {
            if instant.elapsed() < std::time::Duration::from_secs(3) {
                return Some(text);
            }
        }
        None
    }

    /// Extracts clean text of the currently selected turn.
    pub fn copy_selected_turn_text(&self) -> Option<String> {
        self.turns
            .get(self.copy_selected_turn_index)
            .map(|t| t.clean_text())
    }

    /// Extracts clean text of the latest assistant response.
    pub fn copy_latest_assistant_response(&self) -> Option<String> {
        self.turns.last().and_then(|t| t.clean_response_text())
    }

    /// Extracts clean text of the entire conversation.
    pub fn copy_all_conversation_text(&self) -> String {
        self.turns
            .iter()
            .map(|t| t.clean_text())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    }
}

/// Helper copying arbitrary string to host OS clipboard cross-platform.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Clipboard initialization error: {e}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("Clipboard write failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_turn_clean_text() {
        let mut turn = ChatTurn::new("What is the speed of light?");
        turn.set_response("The speed of light is approximately 299,792,458 m/s.");

        let clean = turn.clean_text();
        assert!(clean.contains("You:\nWhat is the speed of light?"));
        assert!(clean.contains("Hades:\nThe speed of light is approximately 299,792,458 m/s."));
        assert_eq!(
            turn.clean_response_text().as_deref(),
            Some("The speed of light is approximately 299,792,458 m/s.")
        );
    }

    #[test]
    fn test_tui_state_copy_methods() {
        let mut state = TuiState::new();
        state.turns.push(ChatTurn::with_response(
            "Hello",
            "Hello! How can I help you today?",
        ));
        state.turns.push(ChatTurn::with_response(
            "List files",
            "Files: main.rs, lib.rs",
        ));

        state.copy_selected_turn_index = 0;
        let t0 = state.copy_selected_turn_text().unwrap();
        assert!(t0.contains("You:\nHello"));

        state.copy_selected_turn_index = 1;
        let t1 = state.copy_selected_turn_text().unwrap();
        assert!(t1.contains("You:\nList files"));

        let latest = state.copy_latest_assistant_response().unwrap();
        assert_eq!(latest, "Files: main.rs, lib.rs");

        let all = state.copy_all_conversation_text();
        assert!(all.contains("Hello"));
        assert!(all.contains("List files"));
        assert!(all.contains("---"));
    }

    #[test]
    fn test_toast_message_lifecycle() {
        let mut state = TuiState::new();
        assert_eq!(state.toast_text(), None);

        state.set_toast("✓ Copied to clipboard");
        assert_eq!(state.toast_text(), Some("✓ Copied to clipboard"));
    }
}
