pub mod error;
pub mod input;
pub mod output;
pub mod prompt;
pub mod runner;
pub mod state;
pub mod terminal;
pub mod theme;
pub mod ui;

pub use error::TuiError;
pub use input::{InputHandler, KeyActionResult};
pub use output::ConversationPrinter;
pub use prompt::PromptManager;
pub use runner::TuiRunner;
pub use state::{ChatTurn, TuiState};
pub use terminal::{init_modal_terminal, init_terminal, leave_modal_terminal, restore_terminal};
pub use theme::HadesTheme;

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent, MouseEventKind,
    };
    use hades_core::{AppState, CommandOutput, HadesApp, StatusInfo};
    use hades_provider::Model;
    use ratatui::layout::Rect;
    use ratatui::style::Style;
    use ratatui::text::Line;
    use tempfile::tempdir;

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_mouse_scroll(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn create_test_app() -> (HadesApp, tempfile::TempDir) {
        let dir = tempdir().expect("create temp dir");
        let config_service = hades_config::ConfigService::with_path(dir.path().join("config.toml"));
        let storage_service = hades_storage::StorageService::with_root(dir.path().join("data"));
        let event_bus = hades_events::EventBus::new();
        let mut app = HadesApp::new(config_service, storage_service, event_bus);
        app.init().expect("init app");
        let _ = app.transition_to(AppState::Running);
        (app, dir)
    }

    #[test]
    fn test_slash_opens_command_palette() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        assert_eq!(app.state(), AppState::Running);

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state)
                .expect("handle key");

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::CommandPalette);
        assert_eq!(state.selected_palette_index, 0);
    }

    #[test]
    fn test_palette_navigation_up_down() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        // Open palette
        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state)
            .expect("open palette");

        let total = app.commands().list().len();
        assert!(total >= 3); // help, status, model, exit

        // Press Down
        InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state)
            .expect("press down");
        assert_eq!(state.selected_palette_index, 1);

        // Press Down
        InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state)
            .expect("press down");
        assert_eq!(state.selected_palette_index, 2);

        // Press Up
        InputHandler::handle_key_event(make_key(KeyCode::Up), &mut app, &mut state)
            .expect("press up");
        assert_eq!(state.selected_palette_index, 1);
    }

    #[test]
    fn test_palette_esc_closes_palette() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        // Open palette
        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state)
            .expect("open palette");
        assert_eq!(app.state(), AppState::CommandPalette);

        // Press Esc
        let action = InputHandler::handle_key_event(make_key(KeyCode::Esc), &mut app, &mut state)
            .expect("press esc");

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::Running);
    }

    #[test]
    fn test_palette_enter_executes_help() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        // Open palette
        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state)
            .expect("open palette");
        state.selected_palette_index = 0; // /help

        // Press Enter
        let action = InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state)
            .expect("press enter");

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::Running);
        assert!(matches!(state.active_output, Some(CommandOutput::Help(_))));
    }

    #[test]
    fn test_mcp_add_command_opens_setup() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();
        state.prompt_input = "/mcp add".to_string();

        let action = InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state)
            .expect("submit MCP add command");

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::McpSetup);
    }

    #[test]
    fn test_mcp_add_command_submits_server_configuration_action() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();
        state.prompt_input = "/mcp add".to_string();

        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state)
            .expect("open MCP setup");
        assert_eq!(app.state(), AppState::McpSetup);

        state.mcp_server_name = "github".to_string();
        state.mcp_server_cursor_position = state.mcp_server_name.len();
        state.mcp_command_input = "npx".to_string();
        state.mcp_command_cursor_position = state.mcp_command_input.len();
        state.mcp_args_input = "-y @modelcontextprotocol/server-github".to_string();
        state.mcp_args_cursor_position = state.mcp_args_input.len();
        state.mcp_auth_token_input = "test-mcp-token".to_string();
        state.mcp_auth_token_cursor_position = state.mcp_auth_token_input.len();
        state.mcp_token_env_input = "GITHUB_TOKEN".to_string();
        state.mcp_token_env_cursor_position = state.mcp_token_env_input.len();

        let action = InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state)
            .expect("submit MCP setup");

        assert_eq!(
            action,
            KeyActionResult::AddMcpServer {
                name: "github".to_string(),
                transport: "stdio".to_string(),
                command_or_url: "npx".to_string(),
                args: "-y @modelcontextprotocol/server-github".to_string(),
                auth_token: "test-mcp-token".to_string(),
                token_env: "GITHUB_TOKEN".to_string(),
            }
        );
        assert_eq!(app.state(), AppState::Running);
    }

    #[test]
    fn test_mcp_test_command_routes_to_live_test_action() {
        let dir = tempdir().expect("create temp dir");
        let config_service = hades_config::ConfigService::with_path(dir.path().join("config.toml"));
        let mut config = hades_config::HadesConfig::default();
        config.mcp.servers.insert(
            "github".to_string(),
            hades_config::McpServerConfig {
                auto_start: false,
                ..Default::default()
            },
        );
        config_service.save(&config).expect("save config");

        let storage_service = hades_storage::StorageService::with_root(dir.path().join("data"));
        let event_bus = hades_events::EventBus::new();
        let mut app = HadesApp::new(config_service, storage_service, event_bus);
        app.init().expect("init app");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let mut state = TuiState::new();
        state.prompt_input = "/mcp test github".to_string();

        let action = InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state)
            .expect("submit MCP test command");

        assert_eq!(action, KeyActionResult::TestMcpServer("github".to_string()));
    }

    #[test]
    fn test_mcp_remove_command_routes_to_removal_action() {
        let dir = tempdir().expect("create temp dir");
        let config_service = hades_config::ConfigService::with_path(dir.path().join("config.toml"));
        let mut config = hades_config::HadesConfig::default();
        config.mcp.servers.insert(
            "github".to_string(),
            hades_config::McpServerConfig {
                auto_start: false,
                ..Default::default()
            },
        );
        config_service.save(&config).expect("save config");

        let storage_service = hades_storage::StorageService::with_root(dir.path().join("data"));
        let event_bus = hades_events::EventBus::new();
        let mut app = HadesApp::new(config_service, storage_service, event_bus);
        app.init().expect("init app");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let mut state = TuiState::new();
        state.prompt_input = "/mcp remove github".to_string();

        let action = InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state)
            .expect("submit MCP remove command");

        assert_eq!(
            action,
            KeyActionResult::RemoveMcpServer("github".to_string())
        );
    }

    #[test]
    fn test_ctrl_c_initiates_shutdown() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        let action =
            InputHandler::handle_key_event(make_ctrl_key(KeyCode::Char('c')), &mut app, &mut state)
                .expect("press ctrl+c");

        assert_eq!(action, KeyActionResult::Quit);
        assert_eq!(app.state(), AppState::Exited);
    }

    #[test]
    fn test_provider_and_model_selection_flow() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        // 1. Transition to ProviderSelect
        app.transition_to(AppState::ProviderSelect).unwrap();
        state.providers = app.model_manager().list_providers();
        assert!(!state.providers.is_empty());

        // 2. Select Provider
        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert!(matches!(action, KeyActionResult::SelectProvider(_)));

        // 3. Transition to ModelSelect
        app.transition_to(AppState::ModelSelect).unwrap();
        state.models = vec![
            Model::new("gpt-4o", "openai", "GPT-4o Frontier"),
            Model::new("gpt-4o-mini", "openai", "GPT-4o Mini"),
        ];
        state.selected_model_index = 0;

        // Navigate down
        InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state).unwrap();
        assert_eq!(state.selected_model_index, 1);

        // Select model
        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(app.state(), AppState::ModelInfo);
        assert_eq!(state.selected_model.as_ref().unwrap().id, "gpt-4o-mini");

        // Proceed to CredentialInput
        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(app.state(), AppState::CredentialInput);

        // Type credentials
        InputHandler::handle_key_event(make_key(KeyCode::Char('s')), &mut app, &mut state).unwrap();
        InputHandler::handle_key_event(make_key(KeyCode::Char('k')), &mut app, &mut state).unwrap();
        InputHandler::handle_key_event(make_key(KeyCode::Char('-')), &mut app, &mut state).unwrap();
        assert_eq!(state.credential_input, "sk-");

        // Submit credential for verification
        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(action, KeyActionResult::VerifyModel);
        assert_eq!(app.state(), AppState::Verifying);
    }

    #[test]
    fn test_conversation_layout_calculation() {
        let lines = vec![
            Line::from("Short line"),
            Line::from("This is a significantly longer line of text designed to test wrapping calculations."),
            Line::from(""),
        ];

        let wrapped_wide = ui::estimate_wrapped_line_count(&lines, 120);
        assert_eq!(wrapped_wide, 3);

        let wrapped_narrow = ui::estimate_wrapped_line_count(&lines, 30);
        assert!(wrapped_narrow > 3);
    }

    #[test]
    fn test_conversation_overflow() {
        let mut state = TuiState::new();
        for i in 0..15 {
            state.turns.push(ChatTurn::with_response(
                format!("User question {}", i),
                format!("Hades response answering question {}", i),
            ));
        }

        assert_eq!(state.turns.len(), 15);
        let history = state.chat_history();
        assert_eq!(history.len(), 15);
    }

    #[test]
    fn test_automatic_scroll_to_bottom() {
        let mut state = TuiState::new();
        assert!(state.auto_scroll_to_bottom);

        // Adding turns maintains auto_scroll_to_bottom
        state
            .turns
            .push(ChatTurn::with_response("Hello", "Hi there!"));
        assert!(state.auto_scroll_to_bottom);

        // PageUp turns off auto_scroll_to_bottom
        let (mut app, _dir) = create_test_app();
        InputHandler::handle_key_event(make_key(KeyCode::PageUp), &mut app, &mut state).unwrap();
        assert!(!state.auto_scroll_to_bottom);

        // End key restores auto_scroll_to_bottom when prompt is empty
        InputHandler::handle_key_event(make_key(KeyCode::End), &mut app, &mut state).unwrap();
        assert!(state.auto_scroll_to_bottom);
    }

    #[test]
    fn test_wrapped_messages() {
        let long_message = "A".repeat(250);
        let lines = vec![Line::from(long_message)];
        let count = ui::estimate_wrapped_line_count(&lines, 80);
        assert_eq!(count, 4); // ceil(250 / 80) = 4
    }

    #[test]
    fn test_streaming_response_growth() {
        let mut turn = ChatTurn::new("What is Rust?");
        assert_eq!(turn.activity_text.as_deref(), Some("Thinking..."));
        assert_eq!(turn.assistant_response, None);

        turn.append_response_chunk("Rust is ");
        assert_eq!(turn.activity_text, None);
        assert_eq!(turn.assistant_response.as_deref(), Some("Rust is "));

        turn.append_response_chunk("a systems language.");
        assert_eq!(
            turn.assistant_response.as_deref(),
            Some("Rust is a systems language.")
        );
    }

    #[test]
    fn test_fixed_prompt_region() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('h')), &mut app, &mut state).unwrap();
        InputHandler::handle_key_event(make_key(KeyCode::Char('i')), &mut app, &mut state).unwrap();
        assert_eq!(state.prompt_input, "hi");
        assert_eq!(state.prompt_cursor_position, 2);

        // Prompt input is isolated from turns
        assert!(state.turns.is_empty());
    }

    #[test]
    fn test_fixed_status_region() {
        let (app, _dir) = create_test_app();
        assert_eq!(app.active_model_display(), "Not configured");
        assert_eq!(app.config().general.default_mode, "simple");
    }

    #[test]
    fn test_terminal_resize() {
        let lines = vec![Line::from(
            "Responsive line for testing terminal resize behavior.",
        )];

        let small_width = ui::estimate_wrapped_line_count(&lines, 20);
        let med_width = ui::estimate_wrapped_line_count(&lines, 60);
        let large_width = ui::estimate_wrapped_line_count(&lines, 120);

        assert!(small_width > med_width);
        assert_eq!(large_width, 1);
    }

    #[test]
    fn test_empty_conversation() {
        let state = TuiState::new();
        assert!(state.turns.is_empty());
        assert!(state.chat_history().is_empty());
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_long_conversation() {
        let mut state = TuiState::new();
        for i in 0..50 {
            state.turns.push(ChatTurn::with_response(
                format!("User message #{}", i),
                format!("Hades reply #{}", i),
            ));
        }
        assert_eq!(state.turns.len(), 50);
        assert_eq!(state.chat_history().len(), 50);
    }

    #[test]
    fn test_activity_lifecycle() {
        let mut state = TuiState::new();
        assert_eq!(state.spinner_frame, 0);

        state.tick_spinner();
        assert_eq!(state.spinner_frame, 1);
        assert_ne!(state.spinner_char(), "");

        let mut turn = ChatTurn::new("Task prompt");
        assert!(turn.activity_text.is_some());

        turn.set_response("Task completed");
        assert_eq!(turn.activity_text, None);
        assert_eq!(turn.assistant_response.as_deref(), Some("Task completed"));
    }

    #[test]
    fn test_user_message_activity_response_ordering() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        // Type prompt
        for c in "Hello Hades".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        assert_eq!(state.prompt_input, "Hello Hades");

        // Submit prompt
        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(
            action,
            KeyActionResult::SubmitPrompt("Hello Hades".to_string())
        );
        assert_eq!(state.prompt_input, "");

        // Turn is created with user prompt and thinking state
        assert_eq!(state.turns.len(), 1);
        let turn = &state.turns[0];
        assert_eq!(turn.user_prompt, "Hello Hades");
        assert_eq!(turn.activity_text.as_deref(), Some("Thinking..."));
        assert_eq!(turn.assistant_response, None);

        // Stream starts
        state.turns[0].append_response_chunk("Hello! ");
        assert_eq!(state.turns[0].activity_text, None);
        assert_eq!(
            state.turns[0].assistant_response.as_deref(),
            Some("Hello! ")
        );

        // Stream finishes
        state.turns[0].append_response_chunk("How can I assist?");
        assert_eq!(
            state.turns[0].assistant_response.as_deref(),
            Some("Hello! How can I assist?")
        );
    }

    #[test]
    fn test_terminal_restoration_is_idempotent() {
        let res1 = restore_terminal();
        assert!(res1.is_ok());
        let res2 = restore_terminal();
        assert!(res2.is_ok());
    }

    #[test]
    fn test_conversation_printer_helpers() {
        ConversationPrinter::print_user_prompt("Test prompt");
        ConversationPrinter::start_hades_turn("Thinking...", "⠋");
        ConversationPrinter::update_activity("Thinking...", "⠙");
        ConversationPrinter::start_streaming_chunk("First chunk");
        ConversationPrinter::append_streaming_chunk(" second chunk");
        ConversationPrinter::finalize_hades_turn();
        ConversationPrinter::print_hades_full_response("Full response text");
        ConversationPrinter::print_turn_error("Sample error");
        ConversationPrinter::print_command_output(&CommandOutput::Status(StatusInfo {
            application: "Hades".into(),
            version: "0.1.0".into(),
            session_id: "test-sess-1".into(),
            session_title: "Test Session".into(),
            messages: 4,
            context_usage: "120 / 32,768 (Estimated)".into(),
            model: "Not configured".into(),
            mode: "simple".into(),
            storage_status: "Healthy".into(),
            config_status: "Loaded".into(),
        }));
    }

    #[test]
    fn test_hades_theme_and_branding() {
        assert!(HadesTheme::TRIDENT == "🜲" || HadesTheme::TRIDENT == "🔱");
        let banner = HadesTheme::banner();
        assert!(!banner.lines.is_empty());
        let compact = HadesTheme::compact_banner();
        assert!(!compact.lines.is_empty());
        let combined: String = compact.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(combined.contains("HADES"));
    }

    #[test]
    fn test_prompt_manager_lifecycle() {
        PromptManager::render_prompt("hello", 5, "Not configured", "simple");
        PromptManager::clear_prompt();
    }

    #[test]
    fn test_prefix_aware_text_wrapping() {
        let text = "Feel free to ask for a deeper dive into any specific variant, a code example for a particular application, or guidance on scaling models.";
        let wrapped = ui::wrap_turn_text(text, 60, "  └─ ", "     ", Style::default());
        assert!(wrapped.len() >= 2);

        let first_prefix = &wrapped[0].spans[0].content;
        assert_eq!(first_prefix, "  └─ ");

        for line in &wrapped[1..] {
            let cont_prefix = &line.spans[0].content;
            assert_eq!(cont_prefix, "     ");
        }
    }

    #[test]
    fn test_no_horizontal_overflow() {
        let long_para = "Word ".repeat(50);
        let width = 40;
        let wrapped = ui::wrap_turn_text(&long_para, width, "  └─ ", "     ", Style::default());
        for line in wrapped {
            let line_len: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(
                line_len <= width,
                "Line length {} exceeded width {}",
                line_len,
                width
            );
        }
    }

    #[test]
    fn test_bottom_region_is_reserved() {
        let total_height = 24u16;
        let total_width = 80u16;
        let size = Rect::new(0, 0, total_width, total_height);
        let [chat, top_border, input, bottom_border, status] = ui::compute_layout_chunks(size, 1);

        assert_eq!(chat.height, 20);
        assert_eq!(top_border.height, 1);
        assert_eq!(input.height, 1);
        assert_eq!(bottom_border.height, 1);
        assert_eq!(status.height, 1);

        assert_eq!(top_border.y, 20);
        assert_eq!(input.y, 21);
        assert_eq!(bottom_border.y, 22);
        assert_eq!(status.y, 23);

        assert_eq!(
            chat.height + top_border.height + input.height + bottom_border.height + status.height,
            total_height
        );
    }

    #[test]
    fn test_multiline_input_dynamic_height_and_shift() {
        // 1. Single line prompt
        let h1 = ui::calculate_input_height("hello world", 80, 8);
        assert_eq!(h1, 1);

        // 2. Explicit multi-line prompt
        let h3 = ui::calculate_input_height("line 1\nline 2\nline 3", 80, 8);
        assert_eq!(h3, 3);

        // 3. Wrapping on narrow terminal
        let long_prompt =
            "This is a long prompt that should wrap across several lines when typed.".repeat(2);
        let h_wrapped = ui::calculate_input_height(&long_prompt, 30, 8);
        assert!(h_wrapped >= 3);

        // 4. Clamping to max height
        let huge_prompt = "line\n".repeat(20);
        let h_clamped = ui::calculate_input_height(&huge_prompt, 80, 5);
        assert_eq!(h_clamped, 5);

        // 5. Layout chunks with multiline input
        let size = Rect::new(0, 0, 100, 30);
        let [chat, top_b, inp, bot_b, stat] = ui::compute_layout_chunks(size, 3);

        assert_eq!(chat.height, 24);
        assert_eq!(top_b.height, 1);
        assert_eq!(inp.height, 3);
        assert_eq!(bot_b.height, 1);
        assert_eq!(stat.height, 1);

        assert_eq!(top_b.y, 24);
        assert_eq!(inp.y, 25);
        assert_eq!(bot_b.y, 28);
        assert_eq!(stat.y, 29);

        // Verify non-overlapping
        assert_eq!(top_b.y, chat.y + chat.height);
        assert_eq!(inp.y, top_b.y + top_b.height);
        assert_eq!(bot_b.y, inp.y + inp.height);
        assert_eq!(stat.y, bot_b.y + bot_b.height);
        assert_eq!(
            chat.height + top_b.height + inp.height + bot_b.height + stat.height,
            30
        );
    }

    #[test]
    fn test_layout_resize_and_geometry_invariants() {
        for (w, h) in [(20, 10), (40, 15), (80, 24), (120, 40), (200, 60)] {
            let size = Rect::new(0, 0, w, h);
            for input_h in [1, 2, 4] {
                let [chat, top_b, inp, bot_b, stat] = ui::compute_layout_chunks(size, input_h);

                assert_eq!(top_b.height, 1);
                assert!(inp.height >= 1);
                assert_eq!(bot_b.height, 1);
                assert_eq!(stat.height, 1);

                // No overlaps
                assert!(top_b.y >= chat.y + chat.height);
                assert_eq!(inp.y, top_b.y + top_b.height);
                assert_eq!(bot_b.y, inp.y + inp.height);
                assert_eq!(stat.y, bot_b.y + bot_b.height);

                // Total height matches
                assert_eq!(
                    chat.height + top_b.height + inp.height + bot_b.height + stat.height,
                    h
                );
            }
        }
    }

    // Granular Incremental Scrolling Tests

    #[test]
    fn test_one_line_up_and_down() {
        let mut state = TuiState::new();
        state.update_geometry(100, 30); // max_scroll = 70, scroll_offset = 70
        assert_eq!(state.scroll_offset, 70);

        // One line up (↑)
        state.scroll_up(1);
        assert_eq!(state.scroll_offset, 69);
        assert!(!state.auto_scroll_to_bottom);
        assert!(state.has_new_content_below);

        // Another line up (↑)
        state.scroll_up(1);
        assert_eq!(state.scroll_offset, 68);

        // One line down (↓)
        state.scroll_down(1);
        assert_eq!(state.scroll_offset, 69);

        // One line down to reach bottom (↓)
        state.scroll_down(1);
        assert_eq!(state.scroll_offset, 70);
        assert!(state.auto_scroll_to_bottom);
        assert!(!state.has_new_content_below);
    }

    #[test]
    fn test_wheel_up_and_down_small_delta() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();
        state.update_geometry(100, 30); // max_scroll = 70, scroll_offset = 70

        // One wheel-up notch (3 lines)
        let _ = InputHandler::handle_mouse_event(
            make_mouse_scroll(MouseEventKind::ScrollUp),
            &mut app,
            &mut state,
        );
        assert_eq!(state.scroll_offset, 67);
        assert!(!state.auto_scroll_to_bottom);

        // Another wheel-up notch (3 lines)
        let _ = InputHandler::handle_mouse_event(
            make_mouse_scroll(MouseEventKind::ScrollUp),
            &mut app,
            &mut state,
        );
        assert_eq!(state.scroll_offset, 64);

        // One wheel-down notch (3 lines)
        let _ = InputHandler::handle_mouse_event(
            make_mouse_scroll(MouseEventKind::ScrollDown),
            &mut app,
            &mut state,
        );
        assert_eq!(state.scroll_offset, 67);
    }

    #[test]
    fn test_page_up_and_page_down() {
        let mut state = TuiState::new();
        state.update_geometry(150, 30); // viewport = 30, step = 28, max_scroll = 120
        state.scroll_offset = 100;
        state.auto_scroll_to_bottom = false;

        // PageUp (moves by viewport - 2 = 28 lines)
        state.page_up();
        assert_eq!(state.scroll_offset, 72);

        // PageDown (moves by 28 lines)
        state.page_down();
        assert_eq!(state.scroll_offset, 100);
    }

    #[test]
    fn test_home_and_end_navigation() {
        let mut state = TuiState::new();
        state.update_geometry(100, 30); // max_scroll = 70

        // Home -> jumps to top (0)
        state.scroll_to_top();
        assert_eq!(state.scroll_offset, 0);
        assert!(!state.auto_scroll_to_bottom);
        assert!(state.has_new_content_below);

        // End -> jumps to bottom (70)
        state.scroll_to_bottom();
        assert_eq!(state.scroll_offset, 70);
        assert!(state.auto_scroll_to_bottom);
        assert!(!state.has_new_content_below);
    }

    #[test]
    fn test_boundary_clamping() {
        let mut state = TuiState::new();
        state.update_geometry(50, 20); // max_scroll = 30

        // Top boundary: Repeated scroll_up at 0 never underflows
        state.scroll_offset = 1;
        state.scroll_up(5);
        assert_eq!(state.scroll_offset, 0);
        state.scroll_up(10);
        assert_eq!(state.scroll_offset, 0);

        // Bottom boundary: Repeated scroll_down at max_scroll never exceeds max_scroll
        state.scroll_offset = 29;
        state.scroll_down(5);
        assert_eq!(state.scroll_offset, 30);
        state.scroll_down(10);
        assert_eq!(state.scroll_offset, 30);
    }

    #[test]
    fn test_streaming_does_not_override_manual_scroll() {
        let mut state = TuiState::new();
        state.update_geometry(50, 20); // max_scroll = 30, offset = 30

        // User scrolls up to line 15
        state.scroll_up(15);
        assert_eq!(state.scroll_offset, 15);
        assert!(!state.auto_scroll_to_bottom);

        // Stream adds 20 new lines (total = 70 lines, max_scroll = 50)
        state.update_geometry(70, 20);

        // Viewport MUST stay at 15!
        assert_eq!(state.scroll_offset, 15);
        assert!(state.has_new_content_below);

        // User presses End
        state.scroll_to_bottom();
        assert_eq!(state.scroll_offset, 50);
        assert!(state.auto_scroll_to_bottom);
        assert!(!state.has_new_content_below);
    }

    #[test]
    fn test_reconstruct_turns_from_session() {
        let mut state = TuiState::new();
        let mut session = hades_storage::SessionRecord::new(
            Some("Test Session".to_string()),
            Some("openai".to_string()),
            Some("gpt-4o".to_string()),
        );

        let msg1 = hades_storage::Message::user(&session.metadata.id, "First user query");
        let msg2 = hades_storage::Message::assistant(
            &session.metadata.id,
            "First assistant response",
            Some("openai".to_string()),
            Some("gpt-4o".to_string()),
        );
        let msg3 = hades_storage::Message::user(&session.metadata.id, "Second user query");

        session.add_message(msg1);
        session.add_message(msg2);
        session.add_message(msg3);

        state.reconstruct_turns_from_session(&session);

        assert_eq!(state.turns.len(), 2);
        assert_eq!(state.turns[0].user_prompt, "First user query");
        assert_eq!(
            state.turns[0].assistant_response.as_deref(),
            Some("First assistant response")
        );
        assert_eq!(state.turns[1].user_prompt, "Second user query");
        assert_eq!(state.turns[1].assistant_response, None);
    }

    #[test]
    fn test_session_select_keyboard_navigation() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        state.sessions = vec![
            hades_storage::SessionMetadata::new(
                "s1",
                "Session One",
                Some("openai".to_string()),
                Some("gpt-4o".to_string()),
            ),
            hades_storage::SessionMetadata::new(
                "s2",
                "Session Two",
                Some("groq".to_string()),
                Some("llama-3.3-70b-versatile".to_string()),
            ),
        ];
        state.selected_session_index = 0;
        app.transition_to(AppState::SessionSelect).unwrap();

        // Down arrow
        let res =
            InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state).unwrap();
        assert_eq!(res, KeyActionResult::Handled);
        assert_eq!(state.selected_session_index, 1);

        // Enter key selects session
        let res =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(res, KeyActionResult::SelectSession("s2".to_string()));
    }

    #[test]
    fn test_session_rename_keyboard_flow() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        state.sessions = vec![hades_storage::SessionMetadata::new(
            "s1",
            "Initial Title",
            None,
            None,
        )];
        state.selected_session_index = 0;
        app.transition_to(AppState::SessionSelect).unwrap();

        // Press 'r' to open rename modal
        let res =
            InputHandler::handle_key_event(make_key(KeyCode::Char('r')), &mut app, &mut state)
                .unwrap();
        assert_eq!(res, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::SessionRename);
        assert_eq!(state.rename_input, "Initial Title");

        // Clear and type new title
        state.rename_input = "Brand New Title".to_string();
        state.rename_cursor_position = state.rename_input.len();

        // Press Enter to confirm rename
        let res =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(
            res,
            KeyActionResult::RenameSession {
                session_id: "s1".to_string(),
                new_title: "Brand New Title".to_string(),
            }
        );
    }

    #[test]
    fn test_session_delete_confirm_keyboard_flow() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        state.sessions = vec![hades_storage::SessionMetadata::new(
            "s1",
            "To Be Deleted",
            None,
            None,
        )];
        state.selected_session_index = 0;
        app.transition_to(AppState::SessionSelect).unwrap();

        // Press 'd' to open delete confirmation
        let res =
            InputHandler::handle_key_event(make_key(KeyCode::Char('d')), &mut app, &mut state)
                .unwrap();
        assert_eq!(res, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::SessionDeleteConfirm);
        assert_eq!(state.delete_session_title, "To Be Deleted");

        // Press 'y' to confirm delete
        let res =
            InputHandler::handle_key_event(make_key(KeyCode::Char('y')), &mut app, &mut state)
                .unwrap();
        assert_eq!(res, KeyActionResult::DeleteSession("s1".to_string()));
    }

    #[test]
    fn test_local_model_bypasses_credential_input() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        // Register local provider with requires_api_key = false
        state.providers = vec![hades_provider::ProviderMetadata {
            id: "ollama".to_string(),
            name: "Ollama (Local)".to_string(),
            description: "Local inference".to_string(),
            default_endpoint: Some("http://localhost:11434/v1".to_string()),
            supports_dynamic_model_discovery: true,
            requires_api_key: false,
            is_local: true,
        }];
        state.selected_provider_index = 0;
        state.selected_model = Some(Model::new("llama3.2:latest", "ollama", "Llama 3.2"));
        app.transition_to(AppState::ProviderSelect).unwrap();
        app.transition_to(AppState::ModelSelect).unwrap();
        app.transition_to(AppState::ModelInfo).unwrap();

        // Pressing Enter on ModelInfo for local model skips CredentialInput and goes straight to Verifying
        let res =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(res, KeyActionResult::VerifyModel);
        assert_eq!(app.state(), AppState::Verifying);
    }

    // ──────────────────────────────────────────────────────────────────────────
    // Command Palette Redesign Integration Tests (Test Points 1 - 20)
    // ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_1_slash_opens_command_palette_and_leaves_prompt_slash() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        assert_eq!(app.state(), AppState::Running);
        assert_eq!(state.prompt_input, "");

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state)
                .expect("open palette");

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::CommandPalette);
        assert_eq!(state.prompt_input, "/");
        assert_eq!(state.prompt_cursor_position, 1);
    }

    #[test]
    fn test_2_subsequent_characters_append_to_prompt_input() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }

        assert_eq!(app.state(), AppState::CommandPalette);
        assert_eq!(state.prompt_input, "/mcp");
        assert_eq!(state.prompt_cursor_position, 4);
    }

    #[test]
    fn test_3_workflow_b_direct_mcp_add_opens_setup() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp add".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        assert_eq!(state.prompt_input, "/mcp add");

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::McpSetup);
    }

    #[test]
    fn test_4_slash_mcp_filters_palette() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }

        let filtered = app.commands().filter_palette(&state.prompt_input, None);
        assert!(!filtered.is_empty());
        assert_eq!(filtered[0].execution_text, "/mcp");
    }

    #[test]
    fn test_5_workflow_a_mcp_enter_enters_subcommands() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::CommandPalette);
        assert_eq!(state.active_subcommand_parent, Some("/mcp".to_string()));
        assert_eq!(state.selected_palette_index, 0);
    }

    #[test]
    fn test_6_subcommands_up_down_navigation() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(state.active_subcommand_parent, Some("/mcp".to_string()));
        assert_eq!(state.selected_palette_index, 0);

        InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state).unwrap();
        assert_eq!(state.selected_palette_index, 1);

        InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state).unwrap();
        assert_eq!(state.selected_palette_index, 2);

        InputHandler::handle_key_event(make_key(KeyCode::Up), &mut app, &mut state).unwrap();
        assert_eq!(state.selected_palette_index, 1);
    }

    #[test]
    fn test_7_subcommand_add_opens_mcp_setup() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(state.selected_palette_index, 0);

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::McpSetup);
    }

    #[test]
    fn test_8_subcommand_remove_populates_prompt() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state).unwrap();
        assert_eq!(state.selected_palette_index, 1);

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::CommandPalette);
        assert_eq!(state.prompt_input, "/mcp remove ");
        assert_eq!(state.active_subcommand_parent, None);
    }

    #[test]
    fn test_9_subcommand_test_populates_prompt() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state).unwrap();
        InputHandler::handle_key_event(make_key(KeyCode::Down), &mut app, &mut state).unwrap();
        assert_eq!(state.selected_palette_index, 2);

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::CommandPalette);
        assert_eq!(state.prompt_input, "/mcp test ");
        assert_eq!(state.active_subcommand_parent, None);
    }

    #[test]
    fn test_10_mcp_remove_with_argument_returns_action() {
        let dir = tempdir().expect("create temp dir");
        let config_service = hades_config::ConfigService::with_path(dir.path().join("config.toml"));
        let mut config = hades_config::HadesConfig::default();
        config.mcp.servers.insert(
            "my-server".to_string(),
            hades_config::McpServerConfig {
                auto_start: false,
                ..Default::default()
            },
        );
        config_service.save(&config).expect("save config");

        let storage_service = hades_storage::StorageService::with_root(dir.path().join("data"));
        let event_bus = hades_events::EventBus::new();
        let mut app = HadesApp::new(config_service, storage_service, event_bus);
        app.init().expect("init app");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let mut state = TuiState::new();
        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp remove my-server".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        assert_eq!(
            action,
            KeyActionResult::RemoveMcpServer("my-server".to_string())
        );
    }

    #[test]
    fn test_11_mcp_test_with_argument_returns_action() {
        let dir = tempdir().expect("create temp dir");
        let config_service = hades_config::ConfigService::with_path(dir.path().join("config.toml"));
        let mut config = hades_config::HadesConfig::default();
        config.mcp.servers.insert(
            "my-server".to_string(),
            hades_config::McpServerConfig {
                auto_start: false,
                ..Default::default()
            },
        );
        config_service.save(&config).expect("save config");

        let storage_service = hades_storage::StorageService::with_root(dir.path().join("data"));
        let event_bus = hades_events::EventBus::new();
        let mut app = HadesApp::new(config_service, storage_service, event_bus);
        app.init().expect("init app");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let mut state = TuiState::new();
        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp test my-server".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();

        assert_eq!(
            action,
            KeyActionResult::TestMcpServer("my-server".to_string())
        );
    }

    #[test]
    fn test_12_backspace_removes_characters_and_updates_filtering() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        assert_eq!(state.prompt_input, "/mcp");

        InputHandler::handle_key_event(make_key(KeyCode::Backspace), &mut app, &mut state).unwrap();
        assert_eq!(state.prompt_input, "/mc");
        assert_eq!(app.state(), AppState::CommandPalette);
    }

    #[test]
    fn test_13_backspace_on_single_slash_returns_to_running() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        assert_eq!(state.prompt_input, "/");
        assert_eq!(app.state(), AppState::CommandPalette);

        InputHandler::handle_key_event(make_key(KeyCode::Backspace), &mut app, &mut state).unwrap();
        assert_eq!(state.prompt_input, "");
        assert_eq!(app.state(), AppState::Running);
    }

    #[test]
    fn test_14_backspace_in_subcommands_clears_parent() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(state.active_subcommand_parent, Some("/mcp".to_string()));

        InputHandler::handle_key_event(make_key(KeyCode::Backspace), &mut app, &mut state).unwrap();
        assert_eq!(state.active_subcommand_parent, None);
        assert_eq!(app.state(), AppState::CommandPalette);
    }

    #[test]
    fn test_15_esc_in_command_palette_transitions_to_running_and_clears_subcommand() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mcp".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }
        InputHandler::handle_key_event(make_key(KeyCode::Enter), &mut app, &mut state).unwrap();
        assert_eq!(state.active_subcommand_parent, Some("/mcp".to_string()));

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Esc), &mut app, &mut state).unwrap();

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(app.state(), AppState::Running);
        assert_eq!(state.active_subcommand_parent, None);
        assert_eq!(state.prompt_input, "/mcp");
    }

    #[test]
    fn test_16_tab_completes_highlighted_command() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();
        for c in "mod".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }

        let items = app.commands().filter_palette(&state.prompt_input, None);
        assert_eq!(items[0].execution_text, "/model");

        let action =
            InputHandler::handle_key_event(make_key(KeyCode::Tab), &mut app, &mut state).unwrap();

        assert_eq!(action, KeyActionResult::Handled);
        assert_eq!(state.prompt_input, "/model");
    }

    #[test]
    fn test_17_typing_non_slash_does_not_open_palette() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        for c in "hello world".chars() {
            InputHandler::handle_key_event(make_key(KeyCode::Char(c)), &mut app, &mut state)
                .unwrap();
        }

        assert_eq!(app.state(), AppState::Running);
        assert_eq!(state.prompt_input, "hello world");
    }

    #[test]
    fn test_18_typing_slash_mid_sentence_does_not_open_palette() {
        let (mut app, _dir) = create_test_app();
        let mut state = TuiState::new();

        state.prompt_input = "see ".to_string();
        state.prompt_cursor_position = 4;

        InputHandler::handle_key_event(make_key(KeyCode::Char('/')), &mut app, &mut state).unwrap();

        assert_eq!(app.state(), AppState::Running);
        assert_eq!(state.prompt_input, "see /");
        assert_eq!(state.prompt_cursor_position, 5);
    }

    #[test]
    fn test_19_palette_scrolling_offset_updates_correctly() {
        let mut state = TuiState::new();
        let total = 20;
        let visible = 6;

        state.selected_palette_index = 0;
        state.adjust_palette_scroll(total, visible);
        assert_eq!(state.palette_scroll_offset, 0);

        state.selected_palette_index = 5;
        state.adjust_palette_scroll(total, visible);
        assert_eq!(state.palette_scroll_offset, 0);

        state.selected_palette_index = 6;
        state.adjust_palette_scroll(total, visible);
        assert_eq!(state.palette_scroll_offset, 1);

        state.selected_palette_index = 10;
        state.adjust_palette_scroll(total, visible);
        assert_eq!(state.palette_scroll_offset, 5);

        state.selected_palette_index = 3;
        state.adjust_palette_scroll(total, visible);
        assert_eq!(state.palette_scroll_offset, 3);
    }
}
