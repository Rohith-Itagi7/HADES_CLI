use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use tracing::info;

use crate::state::{ChatTurn, TuiState};
use hades_core::{AppState, CommandOutput, CoreError, HadesApp};

/// Outcome of processing an input event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyActionResult {
    /// Input event was processed internally by UI state machine.
    Handled,

    /// User submitted a prompt for AI model generation.
    SubmitPrompt(String),

    /// User selected a provider from the list to discover models.
    SelectProvider(String),

    /// User initiated provider credential and model verification.
    VerifyModel,

    /// User requested starting a brand new conversation session.
    NewSession,

    /// User requested opening the session switcher modal.
    OpenSessionPicker,

    /// User selected a session from the session switcher modal.
    SelectSession(String),

    /// User confirmed renaming a session.
    RenameSession {
        session_id: String,
        new_title: String,
    },

    /// User confirmed deleting a session.
    DeleteSession(String),

    /// User resolved a tool approval request.
    ResolveToolApproval(hades_tools::ApprovalDecision),

    /// User confirmed adding a new MCP server with configuration.
    AddMcpServer {
        name: String,
        transport: String,
        command_or_url: String,
        args: String,
        auth_token: String,
        token_env: String,
    },

    /// User requested removing an MCP server.
    RemoveMcpServer(String),

    /// User requested testing an MCP server.
    TestMcpServer(String),

    /// Application should initiate graceful shutdown and terminate.
    Quit,
}

/// Decoupled handler translating Crossterm keyboard and mouse events into application state transitions.
pub struct InputHandler;

impl InputHandler {
    /// Processes a single `KeyEvent` against the current application state.
    pub fn handle_key_event(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        // Global Ctrl+C handler -> Immediate graceful shutdown
        if key_event.modifiers.contains(KeyModifiers::CONTROL)
            && key_event.code == KeyCode::Char('c')
        {
            info!("Received Ctrl+C interrupt signal");
            app.request_shutdown(Some("SIGINT / Ctrl+C".to_string()))?;
            return Ok(KeyActionResult::Quit);
        }

        match app.state() {
            AppState::Running => Self::handle_running(key_event, app, tui_state),
            AppState::CommandPalette => Self::handle_command_palette(key_event, app, tui_state),
            AppState::SessionSelect => Self::handle_session_select(key_event, app, tui_state),
            AppState::SessionRename => Self::handle_session_rename(key_event, app, tui_state),
            AppState::SessionDeleteConfirm => {
                Self::handle_session_delete_confirm(key_event, app, tui_state)
            }
            AppState::ProviderSelect => Self::handle_provider_select(key_event, app, tui_state),
            AppState::ModelSelect => Self::handle_model_select(key_event, app, tui_state),
            AppState::ModelInfo => Self::handle_model_info(key_event, app, tui_state),
            AppState::CredentialInput => Self::handle_credential_input(key_event, app, tui_state),
            AppState::VerificationFailed => {
                Self::handle_verification_failed(key_event, app, tui_state)
            }
            AppState::ToolApproval => Self::handle_tool_approval(key_event, app, tui_state),
            AppState::CopySelect => Self::handle_copy_select(key_event, app, tui_state),
            AppState::McpSetup => Self::handle_mcp_setup(key_event, app, tui_state),
            _ => Ok(KeyActionResult::Handled),
        }
    }

    /// Processes a single `MouseEvent` (mouse-wheel scrolling).
    pub fn handle_mouse_event(
        mouse_event: MouseEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        if app.state() == AppState::Running
            || app.state() == AppState::AiThinking
            || app.state() == AppState::AiStreaming
        {
            match mouse_event.kind {
                MouseEventKind::ScrollUp => {
                    tui_state.scroll_up(3);
                    return Ok(KeyActionResult::Handled);
                }
                MouseEventKind::ScrollDown => {
                    tui_state.scroll_down(3);
                    return Ok(KeyActionResult::Handled);
                }
                _ => {}
            }
        }
        Ok(KeyActionResult::Handled)
    }

    fn handle_running(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            // Open command palette on '/' when prompt input is empty
            KeyCode::Char('/') if tui_state.prompt_input.is_empty() => {
                tui_state.push_prompt_char('/');
                tui_state.selected_palette_index = 0;
                tui_state.active_subcommand_parent = None;
                tui_state.palette_scroll_offset = 0;
                tui_state.clear_error();
                app.transition_to(AppState::CommandPalette)?;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => {
                let input = tui_state.prompt_input.trim().to_string();
                if input.is_empty() {
                    return Ok(KeyActionResult::Handled);
                }

                tui_state.prompt_input.clear();
                tui_state.prompt_cursor_position = 0;

                if input.starts_with('/') {
                    // Execute CLI slash command
                    match app.execute_command(&input) {
                        Ok(output) => match output {
                            CommandOutput::OpenModelSetup => {
                                tui_state.is_model_switch_flow = false;
                                tui_state.providers = app.model_manager().list_providers();
                                tui_state.selected_provider_index = 0;
                                Ok(KeyActionResult::Handled)
                            }
                            CommandOutput::OpenModelSwitch => {
                                tui_state.is_model_switch_flow = true;
                                tui_state.providers = app.model_manager().list_providers();
                                tui_state.selected_provider_index = 0;
                                Ok(KeyActionResult::Handled)
                            }
                            CommandOutput::NewSession => Ok(KeyActionResult::NewSession),
                            CommandOutput::OpenSessionPicker => {
                                Ok(KeyActionResult::OpenSessionPicker)
                            }
                            CommandOutput::OpenMcpSetup => Ok(KeyActionResult::Handled),
                            CommandOutput::RemoveMcpServer(name) => {
                                Ok(KeyActionResult::RemoveMcpServer(name))
                            }
                            CommandOutput::TestMcpServer(name) => {
                                Ok(KeyActionResult::TestMcpServer(name))
                            }
                            CommandOutput::ExportSuccess(path) => {
                                tui_state.show_toast(format!(
                                    "Successfully exported to {}",
                                    path.display()
                                ));
                                tui_state.set_output(CommandOutput::Text(format!(
                                    "✓ Successfully exported session to {}",
                                    path.display()
                                )));
                                Ok(KeyActionResult::Handled)
                            }
                            CommandOutput::ImportSuccess(record) => {
                                tui_state.reconstruct_turns_from_session(&record);
                                tui_state.show_toast(format!(
                                    "Successfully imported: {}",
                                    record.metadata.title
                                ));
                                tui_state.set_output(CommandOutput::Text(format!(
                                    "✓ Successfully imported session '{}' ({} messages)",
                                    record.metadata.title,
                                    record.messages.len()
                                )));
                                tui_state.scroll_to_bottom();
                                Ok(KeyActionResult::Handled)
                            }
                            _ => {
                                tui_state.set_output(output);
                                Ok(KeyActionResult::Handled)
                            }
                        },
                        Err(e) => {
                            tui_state.set_error(e.to_string());
                            Ok(KeyActionResult::Handled)
                        }
                    }
                } else {
                    // Add new user turn immediately to conversation stream
                    tui_state.turns.push(ChatTurn::new(&input));
                    tui_state.scroll_to_bottom();
                    tui_state.active_output = None;
                    tui_state.clear_error();

                    // Send user prompt to model runner
                    Ok(KeyActionResult::SubmitPrompt(input))
                }
            }
            KeyCode::Up => {
                if tui_state.prompt_input.is_empty() {
                    tui_state.scroll_up(1);
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Down => {
                if tui_state.prompt_input.is_empty() {
                    tui_state.scroll_down(1);
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::PageUp => {
                tui_state.page_up();
                Ok(KeyActionResult::Handled)
            }
            KeyCode::PageDown => {
                tui_state.page_down();
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Home => {
                if tui_state.prompt_input.is_empty() {
                    tui_state.scroll_to_top();
                } else {
                    tui_state.prompt_cursor_position = 0;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::End => {
                if tui_state.prompt_input.is_empty() {
                    tui_state.scroll_to_bottom();
                } else {
                    tui_state.prompt_cursor_position = tui_state.prompt_input.len();
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Left => {
                if tui_state.prompt_cursor_position > 0 {
                    tui_state.prompt_cursor_position -= 1;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Right => {
                if tui_state.prompt_cursor_position < tui_state.prompt_input.len() {
                    tui_state.prompt_cursor_position += 1;
                }
                Ok(KeyActionResult::Handled)
            }
            // Clipboard / Copy: Ctrl+Y
            KeyCode::Char('y') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                if tui_state.turns.is_empty() {
                    tui_state.set_toast("No conversation turns to copy.");
                } else if tui_state.turns.len() == 1 {
                    // Directly copy the single assistant response or turn
                    if let Some(resp) = tui_state.copy_latest_assistant_response() {
                        if let Err(e) = crate::state::copy_to_clipboard(&resp) {
                            tui_state.set_error(e);
                        } else {
                            tui_state.set_toast("✓ Copied assistant response to clipboard");
                        }
                    } else if let Some(turn_text) = tui_state.copy_selected_turn_text() {
                        if let Err(e) = crate::state::copy_to_clipboard(&turn_text) {
                            tui_state.set_error(e);
                        } else {
                            tui_state.set_toast("✓ Copied turn to clipboard");
                        }
                    }
                } else {
                    // Open interactive Copy Selection mode
                    tui_state.copy_selected_turn_index = tui_state.turns.len().saturating_sub(1);
                    app.transition_to(AppState::CopySelect)?;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Char(c) => {
                tui_state.push_prompt_char(c);
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Backspace => {
                tui_state.pop_prompt_char();
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Esc => {
                tui_state.clear_error();
                tui_state.active_output = None;
                tui_state.prompt_input.clear();
                tui_state.prompt_cursor_position = 0;
                tui_state.scroll_to_bottom();
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    /// Handles keyboard events when in interactive conversation turn copy selection mode.
    fn handle_copy_select(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            KeyCode::Up | KeyCode::Char('k') => {
                tui_state.copy_selected_turn_index =
                    tui_state.copy_selected_turn_index.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !tui_state.turns.is_empty()
                    && tui_state.copy_selected_turn_index + 1 < tui_state.turns.len()
                {
                    tui_state.copy_selected_turn_index += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('c') => {
                if let Some(text) = tui_state.copy_selected_turn_text() {
                    if let Err(e) = crate::state::copy_to_clipboard(&text) {
                        tui_state.set_error(e);
                    } else {
                        tui_state.set_toast("✓ Copied selected turn to clipboard");
                    }
                }
                app.transition_to(AppState::Running)?;
            }
            KeyCode::Char('a') => {
                let all_text = tui_state.copy_all_conversation_text();
                if let Err(e) = crate::state::copy_to_clipboard(&all_text) {
                    tui_state.set_error(e);
                } else {
                    tui_state.set_toast("✓ Copied entire conversation to clipboard");
                }
                app.transition_to(AppState::Running)?;
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                app.transition_to(AppState::Running)?;
            }
            _ => {}
        }
        Ok(KeyActionResult::Handled)
    }

    fn handle_command_palette(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        let items = app.commands().filter_palette(
            &tui_state.prompt_input,
            tui_state.active_subcommand_parent.as_deref(),
        );
        let count = items.len();

        match key_event.code {
            KeyCode::Char(c) => {
                // If currently in a subcommand menu, smoothly exit menu into continuous query
                if tui_state.active_subcommand_parent.is_some() {
                    if !tui_state.prompt_input.ends_with(' ') && c != ' ' {
                        tui_state.push_prompt_char(' ');
                    }
                    tui_state.active_subcommand_parent = None;
                }
                tui_state.push_prompt_char(c);
                tui_state.selected_palette_index = 0;
                tui_state.palette_scroll_offset = 0;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Backspace => {
                if tui_state.active_subcommand_parent.is_some() {
                    // Backspace in subcommand menu returns to top-level command palette
                    tui_state.active_subcommand_parent = None;
                    tui_state.selected_palette_index = 0;
                    tui_state.palette_scroll_offset = 0;
                } else {
                    tui_state.pop_prompt_char();
                    tui_state.selected_palette_index = 0;
                    tui_state.palette_scroll_offset = 0;
                    // If prompt input became empty (deleted '/'), return cleanly to Running state
                    if tui_state.prompt_input.is_empty() {
                        app.transition_to(AppState::Running)?;
                    }
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Delete => {
                tui_state.delete_prompt_char();
                tui_state.selected_palette_index = 0;
                tui_state.palette_scroll_offset = 0;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Left => {
                if tui_state.prompt_cursor_position > 0 {
                    tui_state.prompt_cursor_position -= 1;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Right => {
                if tui_state.prompt_cursor_position < tui_state.prompt_input.len() {
                    tui_state.prompt_cursor_position += 1;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Home => {
                tui_state.prompt_cursor_position = 0;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::End => {
                tui_state.prompt_cursor_position = tui_state.prompt_input.len();
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Up => {
                if count > 0 {
                    tui_state.selected_palette_index = if tui_state.selected_palette_index == 0 {
                        count - 1
                    } else {
                        tui_state.selected_palette_index - 1
                    };
                    tui_state.adjust_palette_scroll(count, 8);
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Down => {
                if count > 0 {
                    tui_state.selected_palette_index =
                        (tui_state.selected_palette_index + 1) % count;
                    tui_state.adjust_palette_scroll(count, 8);
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Tab => {
                if let Some(item) = items.get(tui_state.selected_palette_index) {
                    let mut completion = item.execution_text.clone();
                    if item.has_subcommands && !completion.ends_with(' ') {
                        completion.push(' ');
                    }
                    tui_state.prompt_input = completion;
                    tui_state.prompt_cursor_position = tui_state.prompt_input.len();
                    tui_state.active_subcommand_parent = None;
                    tui_state.selected_palette_index = 0;
                    tui_state.palette_scroll_offset = 0;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Esc => {
                tui_state.active_subcommand_parent = None;
                tui_state.selected_palette_index = 0;
                tui_state.palette_scroll_offset = 0;
                app.transition_to(AppState::Running)?;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => {
                // Check if user selected a command with subcommands in top-level mode
                if tui_state.active_subcommand_parent.is_none() {
                    if let Some(selected) = items.get(tui_state.selected_palette_index) {
                        let typed_trimmed = tui_state.prompt_input.trim();
                        if selected.has_subcommands
                            && (typed_trimmed == selected.execution_text
                                || typed_trimmed == "/"
                                || typed_trimmed == selected.display_name)
                        {
                            tui_state.active_subcommand_parent =
                                Some(selected.execution_text.clone());
                            tui_state.selected_palette_index = 0;
                            tui_state.palette_scroll_offset = 0;
                            return Ok(KeyActionResult::Handled);
                        }
                    }
                }

                // Check if user selected a subcommand that requires additional arguments
                if tui_state.active_subcommand_parent.is_some() {
                    if let Some(selected) = items.get(tui_state.selected_palette_index) {
                        if selected.requires_args {
                            tui_state.prompt_input = selected.execution_text.clone();
                            tui_state.prompt_cursor_position = tui_state.prompt_input.len();
                            tui_state.active_subcommand_parent = None;
                            tui_state.selected_palette_index = 0;
                            tui_state.palette_scroll_offset = 0;
                            return Ok(KeyActionResult::Handled);
                        }
                    }
                }

                // Determine final command string to execute
                let typed = tui_state.prompt_input.trim().to_string();
                let command_to_run = if typed.contains(' ') {
                    typed
                } else if let Some(selected) = items.get(tui_state.selected_palette_index) {
                    selected.execution_text.clone()
                } else if !typed.is_empty() {
                    typed
                } else {
                    return Ok(KeyActionResult::Handled);
                };

                tui_state.prompt_input.clear();
                tui_state.prompt_cursor_position = 0;
                tui_state.active_subcommand_parent = None;
                tui_state.selected_palette_index = 0;
                tui_state.palette_scroll_offset = 0;

                app.transition_to(AppState::Running)?;
                match app.execute_command(&command_to_run) {
                    Ok(output) => match output {
                        CommandOutput::OpenModelSetup => {
                            tui_state.is_model_switch_flow = false;
                            tui_state.providers = app.model_manager().list_providers();
                            tui_state.selected_provider_index = 0;
                            Ok(KeyActionResult::Handled)
                        }
                        CommandOutput::OpenModelSwitch => {
                            tui_state.is_model_switch_flow = true;
                            tui_state.providers = app.model_manager().list_providers();
                            tui_state.selected_provider_index = 0;
                            Ok(KeyActionResult::Handled)
                        }
                        CommandOutput::NewSession => Ok(KeyActionResult::NewSession),
                        CommandOutput::OpenSessionPicker => Ok(KeyActionResult::OpenSessionPicker),
                        CommandOutput::OpenMcpSetup => Ok(KeyActionResult::Handled),
                        CommandOutput::ExportSuccess(path) => {
                            tui_state
                                .show_toast(format!("Successfully exported to {}", path.display()));
                            tui_state.set_output(CommandOutput::Text(format!(
                                "✓ Successfully exported session to {}",
                                path.display()
                            )));
                            Ok(KeyActionResult::Handled)
                        }
                        CommandOutput::ImportSuccess(record) => {
                            tui_state.reconstruct_turns_from_session(&record);
                            tui_state.show_toast(format!(
                                "Successfully imported: {}",
                                record.metadata.title
                            ));
                            tui_state.set_output(CommandOutput::Text(format!(
                                "✓ Successfully imported session '{}' ({} messages)",
                                record.metadata.title,
                                record.messages.len()
                            )));
                            tui_state.scroll_to_bottom();
                            Ok(KeyActionResult::Handled)
                        }
                        CommandOutput::Exit => Ok(KeyActionResult::Quit),
                        CommandOutput::RemoveMcpServer(name) => {
                            Ok(KeyActionResult::RemoveMcpServer(name))
                        }
                        CommandOutput::TestMcpServer(name) => {
                            Ok(KeyActionResult::TestMcpServer(name))
                        }
                        _ => {
                            tui_state.set_output(output);
                            Ok(KeyActionResult::Handled)
                        }
                    },
                    Err(e) => {
                        tui_state.set_error(e.to_string());
                        Ok(KeyActionResult::Handled)
                    }
                }
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_session_select(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        let count = tui_state.sessions.len();

        match key_event.code {
            KeyCode::Up => {
                if count > 0 {
                    tui_state.selected_session_index = if tui_state.selected_session_index == 0 {
                        count - 1
                    } else {
                        tui_state.selected_session_index - 1
                    };
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Down => {
                if count > 0 {
                    tui_state.selected_session_index =
                        (tui_state.selected_session_index + 1) % count;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => {
                if let Some(session) = tui_state.sessions.get(tui_state.selected_session_index) {
                    let sid = session.id.clone();
                    Ok(KeyActionResult::SelectSession(sid))
                } else {
                    app.transition_to(AppState::Running)?;
                    Ok(KeyActionResult::Handled)
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::Char('e') | KeyCode::Char('E') => {
                if let Some(session) = tui_state.sessions.get(tui_state.selected_session_index) {
                    tui_state.rename_session_id = Some(session.id.clone());
                    tui_state.rename_input = session.title.clone();
                    tui_state.rename_cursor_position = tui_state.rename_input.len();
                    app.transition_to(AppState::SessionRename)?;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Delete => {
                if let Some(session) = tui_state.sessions.get(tui_state.selected_session_index) {
                    tui_state.delete_session_id = Some(session.id.clone());
                    tui_state.delete_session_title = session.title.clone();
                    tui_state.delete_confirm_action = 0; // 0 = Delete, 1 = Cancel
                    app.transition_to(AppState::SessionDeleteConfirm)?;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Esc => {
                app.transition_to(AppState::Running)?;
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_session_rename(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            KeyCode::Char(c) => {
                let idx = tui_state.rename_cursor_position;
                if idx >= tui_state.rename_input.len() {
                    tui_state.rename_input.push(c);
                } else {
                    tui_state.rename_input.insert(idx, c);
                }
                tui_state.rename_cursor_position += 1;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Backspace => {
                if tui_state.rename_cursor_position > 0 {
                    tui_state.rename_cursor_position -= 1;
                    if tui_state.rename_cursor_position < tui_state.rename_input.len() {
                        tui_state
                            .rename_input
                            .remove(tui_state.rename_cursor_position);
                    }
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Left => {
                if tui_state.rename_cursor_position > 0 {
                    tui_state.rename_cursor_position -= 1;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Right => {
                if tui_state.rename_cursor_position < tui_state.rename_input.len() {
                    tui_state.rename_cursor_position += 1;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => {
                let trimmed = tui_state.rename_input.trim();
                if !trimmed.is_empty() {
                    if let Some(ref sid) = tui_state.rename_session_id {
                        let sid = sid.clone();
                        let title = trimmed.to_string();
                        return Ok(KeyActionResult::RenameSession {
                            session_id: sid,
                            new_title: title,
                        });
                    }
                }
                app.transition_to(AppState::SessionSelect)?;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Esc => {
                app.transition_to(AppState::SessionSelect)?;
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_session_delete_confirm(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                tui_state.delete_confirm_action = 1 - tui_state.delete_confirm_action;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(ref sid) = tui_state.delete_session_id {
                    let sid = sid.clone();
                    Ok(KeyActionResult::DeleteSession(sid))
                } else {
                    app.transition_to(AppState::SessionSelect)?;
                    Ok(KeyActionResult::Handled)
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.transition_to(AppState::SessionSelect)?;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => {
                if tui_state.delete_confirm_action == 0 {
                    if let Some(ref sid) = tui_state.delete_session_id {
                        let sid = sid.clone();
                        Ok(KeyActionResult::DeleteSession(sid))
                    } else {
                        app.transition_to(AppState::SessionSelect)?;
                        Ok(KeyActionResult::Handled)
                    }
                } else {
                    app.transition_to(AppState::SessionSelect)?;
                    Ok(KeyActionResult::Handled)
                }
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_provider_select(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        if tui_state.providers.is_empty() {
            tui_state.providers = app.model_manager().list_providers();
        }
        let count = tui_state.providers.len();

        match key_event.code {
            KeyCode::Up => {
                if count > 0 {
                    tui_state.selected_provider_index = if tui_state.selected_provider_index == 0 {
                        count - 1
                    } else {
                        tui_state.selected_provider_index - 1
                    };
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Down => {
                if count > 0 {
                    tui_state.selected_provider_index =
                        (tui_state.selected_provider_index + 1) % count;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => {
                if let Some(provider) = tui_state.providers.get(tui_state.selected_provider_index) {
                    let pid = provider.id.clone();
                    Ok(KeyActionResult::SelectProvider(pid))
                } else {
                    Ok(KeyActionResult::Handled)
                }
            }
            KeyCode::Esc => {
                let _ = app.transition_to(AppState::Running);
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_model_select(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        let count = tui_state.models.len();

        match key_event.code {
            KeyCode::Up => {
                if count > 0 {
                    tui_state.selected_model_index = if tui_state.selected_model_index == 0 {
                        count - 1
                    } else {
                        tui_state.selected_model_index - 1
                    };
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Down => {
                if count > 0 {
                    tui_state.selected_model_index = (tui_state.selected_model_index + 1) % count;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => {
                if let Some(model) = tui_state.models.get(tui_state.selected_model_index) {
                    tui_state.selected_model = Some(model.clone());
                    app.transition_to(AppState::ModelInfo)?;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Esc => {
                app.transition_to(AppState::ProviderSelect)?;
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_model_info(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            KeyCode::Enter => {
                let requires_api_key = tui_state
                    .providers
                    .get(tui_state.selected_provider_index)
                    .map(|p| p.requires_api_key)
                    .unwrap_or(true);

                if !requires_api_key {
                    app.transition_to(AppState::Verifying)?;
                    Ok(KeyActionResult::VerifyModel)
                } else {
                    app.transition_to(AppState::CredentialInput)?;
                    Ok(KeyActionResult::Handled)
                }
            }
            KeyCode::Esc => {
                app.transition_to(AppState::ModelSelect)?;
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_credential_input(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            KeyCode::Enter => {
                app.transition_to(AppState::Verifying)?;
                Ok(KeyActionResult::VerifyModel)
            }
            KeyCode::Tab => {
                tui_state.is_editing_endpoint = !tui_state.is_editing_endpoint;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Char(c) => {
                tui_state.push_credential_char(c);
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Backspace => {
                tui_state.pop_credential_char();
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Esc => {
                app.transition_to(AppState::ModelInfo)?;
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_verification_failed(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            KeyCode::Left | KeyCode::Up => {
                if tui_state.verification_action_index > 0 {
                    tui_state.verification_action_index -= 1;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Right | KeyCode::Down => {
                if tui_state.verification_action_index < 2 {
                    tui_state.verification_action_index += 1;
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => match tui_state.verification_action_index {
                0 => {
                    app.transition_to(AppState::Verifying)?;
                    Ok(KeyActionResult::VerifyModel)
                }
                1 => {
                    app.transition_to(AppState::CredentialInput)?;
                    Ok(KeyActionResult::Handled)
                }
                _ => {
                    app.transition_to(AppState::ModelSelect)?;
                    Ok(KeyActionResult::Handled)
                }
            },
            KeyCode::Esc => {
                app.transition_to(AppState::ModelSelect)?;
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_tool_approval(
        key_event: KeyEvent,
        _app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            KeyCode::Left | KeyCode::BackTab => {
                tui_state.tool_approval_selection = if tui_state.tool_approval_selection == 0 {
                    3
                } else {
                    tui_state.tool_approval_selection - 1
                };
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Right | KeyCode::Tab => {
                tui_state.tool_approval_selection = (tui_state.tool_approval_selection + 1) % 4;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => Ok(KeyActionResult::ResolveToolApproval(
                hades_tools::ApprovalDecision::AllowOnce,
            )),
            KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Char('a') | KeyCode::Char('A') => {
                Ok(KeyActionResult::ResolveToolApproval(
                    hades_tools::ApprovalDecision::AllowSession,
                ))
            }
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Char('n') | KeyCode::Char('N') => {
                Ok(KeyActionResult::ResolveToolApproval(
                    hades_tools::ApprovalDecision::Deny,
                ))
            }
            KeyCode::Esc => Ok(KeyActionResult::ResolveToolApproval(
                hades_tools::ApprovalDecision::Cancel,
            )),
            KeyCode::Enter => {
                let decision = match tui_state.tool_approval_selection {
                    0 => hades_tools::ApprovalDecision::AllowOnce,
                    1 => hades_tools::ApprovalDecision::AllowSession,
                    2 => hades_tools::ApprovalDecision::Deny,
                    _ => hades_tools::ApprovalDecision::Cancel,
                };
                Ok(KeyActionResult::ResolveToolApproval(decision))
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }

    fn handle_mcp_setup(
        key_event: KeyEvent,
        app: &mut HadesApp,
        tui_state: &mut TuiState,
    ) -> Result<KeyActionResult, CoreError> {
        match key_event.code {
            KeyCode::Tab | KeyCode::Down => {
                tui_state.mcp_current_field = (tui_state.mcp_current_field + 1) % 6;
                tui_state.mcp_setup_error = None;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::BackTab | KeyCode::Up => {
                tui_state.mcp_current_field = if tui_state.mcp_current_field == 0 {
                    5
                } else {
                    tui_state.mcp_current_field - 1
                };
                tui_state.mcp_setup_error = None;
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Char(c) => {
                match tui_state.mcp_current_field {
                    0 => {
                        // Server name field
                        tui_state
                            .mcp_server_name
                            .insert(tui_state.mcp_server_cursor_position, c);
                        tui_state.mcp_server_cursor_position += 1;
                    }
                    1 => {
                        // Transport selection: Left/Right to change, this is numbers only
                    }
                    2 => {
                        // Command/URL field based on transport
                        let max_len = 256;
                        if tui_state.mcp_transport_selection == 0 {
                            if tui_state.mcp_command_input.len() < max_len {
                                tui_state
                                    .mcp_command_input
                                    .insert(tui_state.mcp_command_cursor_position, c);
                                tui_state.mcp_command_cursor_position += 1;
                            }
                        } else {
                            if tui_state.mcp_url_input.len() < max_len {
                                tui_state
                                    .mcp_url_input
                                    .insert(tui_state.mcp_url_cursor_position, c);
                                tui_state.mcp_url_cursor_position += 1;
                            }
                        }
                    }
                    3 => {
                        // Args field
                        tui_state
                            .mcp_args_input
                            .insert(tui_state.mcp_args_cursor_position, c);
                        tui_state.mcp_args_cursor_position += 1;
                    }
                    4 => {
                        tui_state
                            .mcp_auth_token_input
                            .insert(tui_state.mcp_auth_token_cursor_position, c);
                        tui_state.mcp_auth_token_cursor_position += 1;
                    }
                    5 => {
                        // Token env field
                        tui_state
                            .mcp_token_env_input
                            .insert(tui_state.mcp_token_env_cursor_position, c);
                        tui_state.mcp_token_env_cursor_position += 1;
                    }
                    _ => {}
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Backspace => {
                match tui_state.mcp_current_field {
                    0 => {
                        if tui_state.mcp_server_cursor_position > 0 {
                            tui_state.mcp_server_cursor_position -= 1;
                            tui_state
                                .mcp_server_name
                                .remove(tui_state.mcp_server_cursor_position);
                        }
                    }
                    2 => {
                        if tui_state.mcp_transport_selection == 0 {
                            if tui_state.mcp_command_cursor_position > 0 {
                                tui_state.mcp_command_cursor_position -= 1;
                                tui_state
                                    .mcp_command_input
                                    .remove(tui_state.mcp_command_cursor_position);
                            }
                        } else {
                            if tui_state.mcp_url_cursor_position > 0 {
                                tui_state.mcp_url_cursor_position -= 1;
                                tui_state
                                    .mcp_url_input
                                    .remove(tui_state.mcp_url_cursor_position);
                            }
                        }
                    }
                    3 => {
                        if tui_state.mcp_args_cursor_position > 0 {
                            tui_state.mcp_args_cursor_position -= 1;
                            tui_state
                                .mcp_args_input
                                .remove(tui_state.mcp_args_cursor_position);
                        }
                    }
                    4 if tui_state.mcp_auth_token_cursor_position > 0 => {
                        tui_state.mcp_auth_token_cursor_position -= 1;
                        tui_state
                            .mcp_auth_token_input
                            .remove(tui_state.mcp_auth_token_cursor_position);
                    }
                    5 if tui_state.mcp_token_env_cursor_position > 0 => {
                        tui_state.mcp_token_env_cursor_position -= 1;
                        tui_state
                            .mcp_token_env_input
                            .remove(tui_state.mcp_token_env_cursor_position);
                    }
                    _ => {}
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Left => {
                match tui_state.mcp_current_field {
                    0 => {
                        if tui_state.mcp_server_cursor_position > 0 {
                            tui_state.mcp_server_cursor_position -= 1;
                        }
                    }
                    1 => {
                        tui_state.mcp_transport_selection = 0; // STDIO
                    }
                    2 => {
                        if tui_state.mcp_transport_selection == 0
                            && tui_state.mcp_command_cursor_position > 0
                        {
                            tui_state.mcp_command_cursor_position -= 1;
                        } else if tui_state.mcp_transport_selection == 1
                            && tui_state.mcp_url_cursor_position > 0
                        {
                            tui_state.mcp_url_cursor_position -= 1;
                        }
                    }
                    3 => {
                        if tui_state.mcp_args_cursor_position > 0 {
                            tui_state.mcp_args_cursor_position -= 1;
                        }
                    }
                    4 if tui_state.mcp_auth_token_cursor_position > 0 => {
                        tui_state.mcp_auth_token_cursor_position -= 1;
                    }
                    5 if tui_state.mcp_token_env_cursor_position > 0 => {
                        tui_state.mcp_token_env_cursor_position -= 1;
                    }
                    _ => {}
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Right => {
                match tui_state.mcp_current_field {
                    0 => {
                        if tui_state.mcp_server_cursor_position < tui_state.mcp_server_name.len() {
                            tui_state.mcp_server_cursor_position += 1;
                        }
                    }
                    1 => {
                        tui_state.mcp_transport_selection = 1; // HTTP
                    }
                    2 => {
                        if tui_state.mcp_transport_selection == 0
                            && tui_state.mcp_command_cursor_position
                                < tui_state.mcp_command_input.len()
                        {
                            tui_state.mcp_command_cursor_position += 1;
                        } else if tui_state.mcp_transport_selection == 1
                            && tui_state.mcp_url_cursor_position < tui_state.mcp_url_input.len()
                        {
                            tui_state.mcp_url_cursor_position += 1;
                        }
                    }
                    3 => {
                        if tui_state.mcp_args_cursor_position < tui_state.mcp_args_input.len() {
                            tui_state.mcp_args_cursor_position += 1;
                        }
                    }
                    4 if tui_state.mcp_auth_token_cursor_position
                        < tui_state.mcp_auth_token_input.len() =>
                    {
                        tui_state.mcp_auth_token_cursor_position += 1;
                    }
                    5 if tui_state.mcp_token_env_cursor_position
                        < tui_state.mcp_token_env_input.len() =>
                    {
                        tui_state.mcp_token_env_cursor_position += 1;
                    }
                    _ => {}
                }
                Ok(KeyActionResult::Handled)
            }
            KeyCode::Enter => {
                // Validate inputs
                if tui_state.mcp_server_name.trim().is_empty() {
                    tui_state.mcp_setup_error = Some("Server name cannot be empty".to_string());
                    return Ok(KeyActionResult::Handled);
                }

                if tui_state.mcp_transport_selection == 0
                    && tui_state.mcp_command_input.trim().is_empty()
                {
                    tui_state.mcp_setup_error =
                        Some("Command cannot be empty for STDIO transport".to_string());
                    return Ok(KeyActionResult::Handled);
                }

                if tui_state.mcp_transport_selection == 1
                    && tui_state.mcp_url_input.trim().is_empty()
                {
                    tui_state.mcp_setup_error =
                        Some("URL cannot be empty for HTTP transport".to_string());
                    return Ok(KeyActionResult::Handled);
                }

                // Create result with captured values
                let transport = if tui_state.mcp_transport_selection == 0 {
                    "stdio".to_string()
                } else {
                    "http".to_string()
                };

                let command_or_url = if tui_state.mcp_transport_selection == 0 {
                    tui_state.mcp_command_input.trim().to_string()
                } else {
                    tui_state.mcp_url_input.trim().to_string()
                };

                let result = KeyActionResult::AddMcpServer {
                    name: tui_state.mcp_server_name.trim().to_string(),
                    transport,
                    command_or_url,
                    args: tui_state.mcp_args_input.trim().to_string(),
                    auth_token: tui_state.mcp_auth_token_input.trim().to_string(),
                    token_env: tui_state.mcp_token_env_input.trim().to_string(),
                };

                // Reset fields
                tui_state.mcp_server_name.clear();
                tui_state.mcp_command_input.clear();
                tui_state.mcp_url_input.clear();
                tui_state.mcp_args_input.clear();
                tui_state.mcp_auth_token_input.clear();
                tui_state.mcp_token_env_input.clear();
                tui_state.mcp_current_field = 0;
                tui_state.mcp_transport_selection = 0;
                tui_state.mcp_setup_error = None;

                app.transition_to(AppState::Running)?;
                Ok(result)
            }
            KeyCode::Esc => {
                // Reset fields
                tui_state.mcp_server_name.clear();
                tui_state.mcp_command_input.clear();
                tui_state.mcp_url_input.clear();
                tui_state.mcp_args_input.clear();
                tui_state.mcp_auth_token_input.clear();
                tui_state.mcp_token_env_input.clear();
                tui_state.mcp_current_field = 0;
                tui_state.mcp_transport_selection = 0;
                tui_state.mcp_setup_error = None;

                app.transition_to(AppState::Running)?;
                Ok(KeyActionResult::Handled)
            }
            _ => Ok(KeyActionResult::Handled),
        }
    }
}
