use std::io::Stdout;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tracing::{error, info};

use crate::error::TuiError;
use crate::input::{InputHandler, KeyActionResult};
use crate::state::TuiState;
use crate::terminal::{init_terminal, restore_terminal};
use crate::ui;
use hades_core::{AppState, CommandOutput, HadesApp};
use hades_provider::{Credential, Model, StreamEvent};

/// Runs the full-screen Ratatui UI event loop with application-owned scrollable conversation viewport.
pub struct TuiRunner;

impl TuiRunner {
    /// Starts the main UI rendering and event processing loop.
    /// Starts the main UI rendering and event processing loop.
    pub async fn run(
        app: &mut HadesApp,
        resume_session_id: Option<String>,
    ) -> Result<(), TuiError> {
        let mut terminal = init_terminal()?;
        let mut tui_state = TuiState::new();
        tui_state.providers = app.model_manager().list_providers();

        // Initialize session: explicit resume if requested, otherwise brand new session
        match app.init_session(resume_session_id.as_deref()).await {
            Ok(Some(msg)) => {
                tui_state.set_error(msg);
            }
            Err(e) => {
                tui_state.set_error(e.to_string());
            }
            _ => {}
        }

        if let Some(session) = app.active_session() {
            tui_state.reconstruct_turns_from_session(session);
        }

        info!("Starting Hades Full-Screen Ratatui TUI event loop");

        let loop_result: Result<(), TuiError> = async {
            while app.state() != AppState::Exited {
                // Advance transient activity spinner
                tui_state.tick_spinner();

                // 1. Draw current UI frame with strictly partitioned geometry and scrollbar
                terminal.draw(|frame| {
                    ui::render(frame, app, &mut tui_state);
                })?;

                // 2. Poll for terminal input events (keyboard and mouse)
                if event::poll(Duration::from_millis(50))? {
                    match event::read()? {
                        Event::Key(key_event) if key_event.kind == event::KeyEventKind::Press => {
                            let action =
                                InputHandler::handle_key_event(key_event, app, &mut tui_state)?;
                            match action {
                                KeyActionResult::Quit => break,
                                KeyActionResult::Handled => {}
                                KeyActionResult::NewSession => {
                                    match app.create_new_session(None).await {
                                        Ok(_) => {
                                            tui_state.turns.clear();
                                            tui_state.active_output = None;
                                            tui_state.scroll_to_bottom();
                                            app.transition_to(AppState::Running)?;
                                        }
                                        Err(e) => {
                                            tui_state.set_error(e.to_string());
                                        }
                                    }
                                }
                                KeyActionResult::OpenSessionPicker => {
                                    match app.list_sessions().await {
                                        Ok(sessions) => {
                                            tui_state.sessions = sessions;
                                            tui_state.selected_session_index = 0;
                                            app.transition_to(AppState::SessionSelect)?;
                                        }
                                        Err(e) => {
                                            tui_state.set_error(e.to_string());
                                        }
                                    }
                                }
                                KeyActionResult::SelectSession(session_id) => {
                                    match app.switch_session(&session_id).await {
                                        Ok(session) => {
                                            tui_state.reconstruct_turns_from_session(&session);
                                            tui_state.active_output = None;
                                            app.transition_to(AppState::Running)?;
                                        }
                                        Err(e) => {
                                            tui_state.set_error(e.to_string());
                                            app.transition_to(AppState::Running)?;
                                        }
                                    }
                                }
                                KeyActionResult::RenameSession {
                                    session_id,
                                    new_title,
                                } => {
                                    match app.rename_session(&session_id, &new_title).await {
                                        Ok(_) => {
                                            if let Ok(sessions) = app.list_sessions().await {
                                                tui_state.sessions = sessions;
                                            }
                                            app.transition_to(AppState::SessionSelect)?;
                                        }
                                        Err(e) => {
                                            tui_state.set_error(e.to_string());
                                            app.transition_to(AppState::SessionSelect)?;
                                        }
                                    }
                                }
                                KeyActionResult::DeleteSession(session_id) => {
                                    let was_active = app
                                        .active_session()
                                        .map(|s| s.metadata.id == session_id)
                                        .unwrap_or(false);

                                    match app.delete_session(&session_id).await {
                                        Ok(_) => {
                                            if was_active {
                                                tui_state.turns.clear();
                                                tui_state.active_output = None;
                                                tui_state.scroll_to_bottom();
                                            }
                                            if let Ok(sessions) = app.list_sessions().await {
                                                tui_state.sessions = sessions;
                                                if tui_state.selected_session_index
                                                    >= tui_state.sessions.len()
                                                    && !tui_state.sessions.is_empty()
                                                {
                                                    tui_state.selected_session_index =
                                                        tui_state.sessions.len() - 1;
                                                }
                                            }
                                            app.transition_to(AppState::SessionSelect)?;
                                        }
                                        Err(e) => {
                                            tui_state.set_error(e.to_string());
                                            app.transition_to(AppState::SessionSelect)?;
                                        }
                                    }
                                }
                                KeyActionResult::SelectProvider(provider_id) => {
                                    let is_local = app
                                        .model_manager()
                                        .get_provider(&provider_id)
                                        .map(|p| p.metadata().is_local)
                                        .unwrap_or(false);

                                    let cred = app
                                        .credential_backend()
                                        .get_credential(&provider_id)
                                        .await
                                        .unwrap_or_default()
                                        .unwrap_or_else(|| {
                                            Credential::with_api_key(&provider_id, "")
                                        });

                                    match app
                                        .model_manager_mut()
                                        .discover_models(&provider_id, &cred)
                                        .await
                                    {
                                        Ok(models) => {
                                            if is_local && models.is_empty() {
                                                tui_state.set_error(
                                                    "Ollama is running, but no local models were detected. Pull a model with 'ollama pull <model>' first.".to_string(),
                                                );
                                                app.transition_to(AppState::ProviderSelect)?;
                                            } else if !models.is_empty() {
                                                tui_state.models = models;
                                                tui_state.selected_model_index = 0;
                                                app.transition_to(AppState::ModelSelect)?;
                                            } else {
                                                let fallback_models = match provider_id.as_str() {
                                                    "groq" => vec![
                                                        Model::new(
                                                            "llama-3.3-70b-versatile",
                                                            "groq",
                                                            "Llama 3.3 70B Versatile",
                                                        ),
                                                        Model::new(
                                                            "llama-3.1-8b-instant",
                                                            "groq",
                                                            "Llama 3.1 8B Instant",
                                                        ),
                                                        Model::new(
                                                            "mixtral-8x7b-32768",
                                                            "groq",
                                                            "Mixtral 8x7B Instruct",
                                                        ),
                                                    ],
                                                    _ => vec![
                                                        Model::new(
                                                            "gpt-4o",
                                                            "openai",
                                                            "GPT-4o Frontier Multimodal",
                                                        ),
                                                        Model::new(
                                                            "gpt-4o-mini",
                                                            "openai",
                                                            "GPT-4o Mini Fast",
                                                        ),
                                                        Model::new(
                                                            "o1",
                                                            "openai",
                                                            "o1 Reasoning",
                                                        ),
                                                    ],
                                                };
                                                tui_state.models = fallback_models;
                                                tui_state.selected_model_index = 0;
                                                app.transition_to(AppState::ModelSelect)?;
                                            }
                                        }
                                        Err(e) => {
                                            if is_local {
                                                tui_state.set_error(e.to_string());
                                                app.transition_to(AppState::ProviderSelect)?;
                                            } else {
                                                let fallback_models = match provider_id.as_str() {
                                                    "groq" => vec![
                                                        Model::new(
                                                            "llama-3.3-70b-versatile",
                                                            "groq",
                                                            "Llama 3.3 70B Versatile",
                                                        ),
                                                        Model::new(
                                                            "llama-3.1-8b-instant",
                                                            "groq",
                                                            "Llama 3.1 8B Instant",
                                                        ),
                                                        Model::new(
                                                            "mixtral-8x7b-32768",
                                                            "groq",
                                                            "Mixtral 8x7B Instruct",
                                                        ),
                                                    ],
                                                    _ => vec![
                                                        Model::new(
                                                            "gpt-4o",
                                                            "openai",
                                                            "GPT-4o Frontier Multimodal",
                                                        ),
                                                        Model::new(
                                                            "gpt-4o-mini",
                                                            "openai",
                                                            "GPT-4o Mini Fast",
                                                        ),
                                                        Model::new(
                                                            "o1",
                                                            "openai",
                                                            "o1 Reasoning",
                                                        ),
                                                    ],
                                                };
                                                tui_state.models = fallback_models;
                                                tui_state.selected_model_index = 0;
                                                app.transition_to(AppState::ModelSelect)?;
                                            }
                                        }
                                    }
                                }
                                KeyActionResult::VerifyModel => {
                                    terminal.draw(|frame| {
                                        ui::render(frame, app, &mut tui_state);
                                    })?;

                                    let provider_id = tui_state
                                        .selected_model
                                        .as_ref()
                                        .map(|m| m.provider_id.clone())
                                        .unwrap_or_default();
                                    let model_id = tui_state
                                        .selected_model
                                        .as_ref()
                                        .map(|m| m.id.clone())
                                        .unwrap_or_default();
                                    let endpoint =
                                        if tui_state.custom_endpoint_input.trim().is_empty() {
                                            None
                                        } else {
                                            Some(tui_state.custom_endpoint_input.trim().to_string())
                                        };

                                    let mut cred = Credential::with_api_key(
                                        &provider_id,
                                        tui_state.credential_input.trim(),
                                    );
                                    cred.endpoint = endpoint;

                                    if tui_state.is_model_switch_flow {
                                        match app
                                            .switch_active_model_for_session(
                                                &provider_id,
                                                &model_id,
                                                &cred,
                                            )
                                            .await
                                        {
                                            Ok(_) => {
                                                tui_state.clear_error();
                                                tui_state.is_model_switch_flow = false;
                                                app.transition_to(AppState::Running)?;
                                            }
                                            Err(e) => {
                                                tui_state.verification_error = Some(e.to_string());
                                                tui_state.verification_action_index = 0;
                                                app.transition_to(AppState::VerificationFailed)?;
                                            }
                                        }
                                    } else {
                                        match app
                                            .verify_and_persist_active_model(
                                                &provider_id,
                                                &model_id,
                                                &cred,
                                            )
                                            .await
                                        {
                                            Ok(_) => {
                                                tui_state.clear_error();
                                                app.transition_to(AppState::Running)?;
                                            }
                                            Err(e) => {
                                                tui_state.verification_error = Some(e.to_string());
                                                tui_state.verification_action_index = 0;
                                                app.transition_to(AppState::VerificationFailed)?;
                                            }
                                        }
                                    }
                                }
                                KeyActionResult::ResolveToolApproval(decision) => {
                                    match app.resolve_pending_approval(decision).await {
                                        Ok(_result) => {
                                            if let Some(turn) = tui_state.turns.last_mut() {
                                                turn.clear_activity();
                                            }
                                            // Resume agent loop with continuation stream!
                                            let _ = execute_agent_loop(
                                                app,
                                                &mut tui_state,
                                                &mut terminal,
                                                None,
                                            )
                                            .await;
                                            tui_state.scroll_to_bottom();
                                        }
                                        Err(e) => {
                                            tui_state.set_error(e.to_string());
                                            let _ = app.transition_to(AppState::Running);
                                        }
                                    }
                                }
                                KeyActionResult::AddMcpServer {
                                    name,
                                    transport,
                                    command_or_url,
                                    args,
                                    auth_token,
                                    token_env,
                                } => {
                                    // Add MCP server configuration and persist it
                                    app.transition_to(AppState::Running)?;
                                    let auth_token = (!auth_token.is_empty()).then_some(auth_token.as_str());
                                    match app.add_mcp_server(&name, &transport, &command_or_url, &args, &token_env, auth_token).await {
                                        Ok(_) => {
                                            tui_state.show_toast(format!("✓ MCP server '{}' added successfully", name));
                                        }
                                        Err(e) => {
                                            tui_state.set_error(format!("Failed to add MCP server '{}': {}", name, e));
                                        }
                                    }
                                }
                                KeyActionResult::RemoveMcpServer(name) => {
                                    match app.remove_mcp_server(&name).await {
                                        Ok(()) => tui_state.show_toast(format!("MCP server '{}' removed", name)),
                                        Err(e) => tui_state.set_error(e.to_string()),
                                    }
                                }
                                KeyActionResult::TestMcpServer(name) => {
                                    match app.test_mcp_server(&name).await {
                                        Ok(result) => tui_state.set_output(CommandOutput::Text(result)),
                                        Err(e) => tui_state.set_error(e.to_string()),
                                    }
                                }
                                KeyActionResult::SubmitPrompt(prompt) => {
                                    // 1. Enter thinking state & render immediately showing user turn + activity state
                                    app.transition_to(AppState::AiThinking)?;
                                    tui_state.scroll_to_bottom();
                                    terminal.draw(|frame| {
                                        ui::render(frame, app, &mut tui_state);
                                    })?;

                                    // 2. Execute streaming request with persistent session context
                                    match app.send_prompt_stream(&prompt).await {
                                        Ok((stream, _report, message_id)) => {
                                            let _ = execute_agent_loop(
                                                app,
                                                &mut tui_state,
                                                &mut terminal,
                                                Some((stream, message_id)),
                                            )
                                            .await;
                                        }
                                        Err(e) => {
                                            if let Some(turn) = tui_state.turns.last_mut() {
                                                turn.set_error(e.to_string());
                                            }
                                            let _ = app.transition_to(AppState::Running);
                                        }
                                    }
                                }
                            }
                        }
                        Event::Mouse(mouse_event) => {
                            let _ =
                                InputHandler::handle_mouse_event(mouse_event, app, &mut tui_state)?;
                        }
                        _ => {}
                    }
                }
            }
            Ok(())
        }
        .await;

        // 3. Always restore terminal safely upon exit
        if let Err(e) = restore_terminal() {
            error!(error = %e, "Failed to restore terminal state");
        }

        if loop_result.is_ok() {
            if let Some(session) = app.active_session() {
                println!(
                    "\nSession saved.\n\nTo resume this session:\n    hades --session {}\n",
                    session.metadata.id
                );
            }
        }

        loop_result
    }
}

/// Runs a single streaming request pass, accumulating text and tool calls.
async fn run_single_stream(
    mut stream: hades_provider::StreamResult,
    app: &mut HadesApp,
    tui_state: &mut TuiState,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    message_id: &str,
) -> Result<Option<Vec<hades_provider::ProviderToolCall>>, TuiError> {
    app.transition_to(AppState::AiStreaming)?;
    let mut accumulated_response = String::new();
    let mut reported_usage = None;
    let mut was_interrupted = false;
    let mut ready_tool_calls = None;

    while let Some(item) = stream.next().await {
        // Non-blocking poll for scrolling / interrupt events during active streaming
        if event::poll(Duration::from_millis(5))? {
            match event::read()? {
                Event::Key(k) if k.kind == event::KeyEventKind::Press => {
                    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                        was_interrupted = true;
                        break;
                    }
                    let _ = InputHandler::handle_key_event(k, app, tui_state);
                }
                Event::Mouse(m) => {
                    let _ = InputHandler::handle_mouse_event(m, app, tui_state);
                }
                _ => {}
            }
        }

        match item {
            Ok(StreamEvent::Delta(text)) => {
                tui_state.tick_spinner();
                accumulated_response.push_str(&text);
                if let Some(turn) = tui_state.turns.last_mut() {
                    turn.append_response_chunk(&text);
                }
                terminal.draw(|frame| {
                    ui::render(frame, app, tui_state);
                })?;
            }
            Ok(StreamEvent::ToolCallChunk { name, .. }) => {
                tui_state.tick_spinner();
                if let Some(turn) = tui_state.turns.last_mut() {
                    if let Some(tool_name) = name {
                        turn.set_activity(format!("Calling {tool_name}..."));
                    } else {
                        turn.set_activity("Preparing tool call...");
                    }
                }
                terminal.draw(|frame| {
                    ui::render(frame, app, tui_state);
                })?;
            }
            Ok(StreamEvent::ToolCallsReady(calls)) => {
                ready_tool_calls = Some(calls);
            }
            Ok(StreamEvent::Usage(usage)) => {
                reported_usage = Some(usage);
                tui_state.current_usage = Some(usage);
            }
            Ok(StreamEvent::Finished(_)) => break,
            Ok(StreamEvent::Started) => {}
            Ok(StreamEvent::Error(err)) => {
                if let Some(turn) = tui_state.turns.last_mut() {
                    turn.set_error(err);
                }
                break;
            }
            Err(err) => {
                if let Some(turn) = tui_state.turns.last_mut() {
                    turn.set_error(err.to_string());
                }
                break;
            }
        }
    }

    if let Some(ref calls) = ready_tool_calls {
        let _ = app
            .record_assistant_tool_calls(message_id, &accumulated_response, calls)
            .await;
    } else {
        let _ = app
            .finalize_streaming_response(
                message_id,
                &accumulated_response,
                reported_usage,
                was_interrupted,
            )
            .await;
    }

    Ok(ready_tool_calls)
}

/// Coordinates the multi-turn agent loop executing tools and passing results back to the model.
async fn execute_agent_loop(
    app: &mut HadesApp,
    tui_state: &mut TuiState,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    initial_stream: Option<(hades_provider::StreamResult, String)>,
) -> Result<(), TuiError> {
    const MAX_TOOL_ITERATIONS: usize = 15;
    let mut iteration = 0;

    let (mut current_stream, mut current_msg_id) = match initial_stream {
        Some((s, id)) => (s, id),
        None => {
            let (s, _report, id) = app.send_continuation_stream().await?;
            (s, id)
        }
    };

    loop {
        iteration += 1;
        if iteration > MAX_TOOL_ITERATIONS {
            if let Some(turn) = tui_state.turns.last_mut() {
                turn.append_response_chunk("\n\n[Max tool call iterations reached]");
                turn.clear_activity();
            }
            app.transition_to(AppState::Running)?;
            break;
        }

        let tool_calls_opt =
            run_single_stream(current_stream, app, tui_state, terminal, &current_msg_id).await?;

        match tool_calls_opt {
            Some(tool_calls) if !tool_calls.is_empty() => {
                let mut approval_required = false;

                for tc in tool_calls {
                    let args_val: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(serde_json::Value::Object(Default::default()));
                    let call = hades_tools::ToolCall::new(&tc.id, &tc.function.name, args_val);

                    if let Some(turn) = tui_state.turns.last_mut() {
                        turn.set_activity(format!("Executing {}...", tc.function.name));
                    }
                    terminal.draw(|frame| {
                        ui::render(frame, app, tui_state);
                    })?;

                    let _ = app.execute_tool_call(call).await?;

                    if app.state() == AppState::ToolApproval {
                        approval_required = true;
                        if let Some(turn) = tui_state.turns.last_mut() {
                            turn.set_activity("Awaiting user approval...");
                        }
                        break;
                    }
                }

                if approval_required {
                    // Pause loop, user will interact with ToolApproval modal in TUI
                    break;
                }

                // If all tools executed and approved, continue turn with model
                match app.send_continuation_stream().await {
                    Ok((next_s, _report, next_id)) => {
                        current_stream = next_s;
                        current_msg_id = next_id;
                    }
                    Err(e) => {
                        if let Some(turn) = tui_state.turns.last_mut() {
                            turn.set_error(e.to_string());
                        }
                        app.transition_to(AppState::Running)?;
                        break;
                    }
                }
            }
            _ => {
                // Done with generation
                if let Some(turn) = tui_state.turns.last_mut() {
                    turn.clear_activity();
                }
                app.transition_to(AppState::Running)?;
                break;
            }
        }
    }

    Ok(())
}
