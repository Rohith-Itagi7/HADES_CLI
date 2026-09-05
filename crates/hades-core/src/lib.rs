pub mod app;
pub mod command;
pub mod context;
pub mod error;
pub mod notification;
pub mod state;

pub use app::{HadesApp, APP_VERSION};
pub use command::{
    Command, CommandContext, CommandInfo, CommandOutput, CommandRegistry, ExitCommand,
    ExportCommand, HelpCommand, HelpEntry, ImportCommand, ModelCommand, NewSessionCommand,
    NotifyCommand, SessionsCommand, StatusCommand, StatusInfo, SwitchCommand,
};

pub use context::{ContextManager, ContextReport, TokenEstimator, UsageKind};
pub use error::{CommandError, CoreError};
pub use notification::{NotificationKind, NotificationService, SoundPlayer};
pub use state::AppState;

#[cfg(test)]
mod tests {
    use super::*;
    use hades_config::ActiveModelConfig;
    use hades_storage::{FileSessionRepository, Message};
    use tempfile::tempdir;

    fn create_test_app() -> (HadesApp, tempfile::TempDir) {
        let dir = tempdir().expect("create temp dir");
        let config_path = dir.path().join("config.toml");
        let storage_path = dir.path().join("data");
        let sessions_path = dir.path().join("sessions");

        let config_service = hades_config::ConfigService::with_path(config_path);
        let storage_service = hades_storage::StorageService::with_root(storage_path);
        let session_repo = std::sync::Arc::new(FileSessionRepository::with_dir(sessions_path));
        let event_bus = hades_events::EventBus::new();

        let app = HadesApp::with_backends(
            config_service,
            storage_service,
            event_bus,
            std::sync::Arc::new(hades_provider::FileCredentialBackend::with_path(
                dir.path().join("credentials.json"),
            )),
            session_repo,
        );
        (app, dir)
    }

    #[test]
    fn test_app_initialization_unconfigured_opens_provider_select() {
        let (mut app, _dir) = create_test_app();
        assert_eq!(app.state(), AppState::Startup);

        app.init().expect("app init");
        assert_eq!(app.state(), AppState::ProviderSelect);
    }

    #[test]
    fn test_app_initialization_with_model_enters_running() {
        let dir = tempdir().expect("create temp dir");
        let config_path = dir.path().join("config.toml");
        let storage_path = dir.path().join("data");
        let sessions_path = dir.path().join("sessions");

        let config_service = hades_config::ConfigService::with_path(&config_path);
        let config = hades_config::HadesConfig {
            model: Some(ActiveModelConfig::new("openai", "gpt-4o")),
            ..Default::default()
        };
        config_service.save(&config).expect("save config");

        let storage_service = hades_storage::StorageService::with_root(storage_path);
        let session_repo = std::sync::Arc::new(FileSessionRepository::with_dir(sessions_path));
        let event_bus = hades_events::EventBus::new();

        let mut app = HadesApp::with_backends(
            config_service,
            storage_service,
            event_bus,
            std::sync::Arc::new(hades_provider::FileCredentialBackend::with_path(
                dir.path().join("credentials.json"),
            )),
            session_repo,
        );
        app.init().expect("app init");
        assert_eq!(app.state(), AppState::Running);
        assert_eq!(app.active_model_display(), "openai/gpt-4o");
    }

    #[test]
    fn test_state_machine_valid_and_invalid_transitions() {
        assert!(AppState::Startup.can_transition_to(AppState::Running));
        assert!(AppState::Startup.can_transition_to(AppState::ProviderSelect));
        assert!(AppState::Startup.can_transition_to(AppState::ShuttingDown));

        assert!(AppState::Running.can_transition_to(AppState::CommandPalette));
        assert!(AppState::Running.can_transition_to(AppState::SessionSelect));
        assert!(AppState::Running.can_transition_to(AppState::ProviderSelect));
        assert!(AppState::Running.can_transition_to(AppState::ShuttingDown));

        assert!(AppState::SessionSelect.can_transition_to(AppState::Running));
        assert!(AppState::SessionSelect.can_transition_to(AppState::CommandPalette));

        assert!(AppState::ProviderSelect.can_transition_to(AppState::ModelSelect));
        assert!(AppState::ProviderSelect.can_transition_to(AppState::Running));
        assert!(AppState::ModelSelect.can_transition_to(AppState::ModelInfo));
        assert!(AppState::ModelInfo.can_transition_to(AppState::CredentialInput));
        assert!(AppState::CredentialInput.can_transition_to(AppState::Verifying));
        assert!(AppState::Verifying.can_transition_to(AppState::Running));
        assert!(AppState::Verifying.can_transition_to(AppState::VerificationFailed));
        assert!(AppState::VerificationFailed.can_transition_to(AppState::CredentialInput));

        assert!(AppState::ShuttingDown.can_transition_to(AppState::Exited));
        assert!(!AppState::ShuttingDown.can_transition_to(AppState::Running));
        assert!(!AppState::Exited.can_transition_to(AppState::Running));
    }

    #[test]
    fn test_help_command_execution() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");

        let output = app.execute_command("/help").expect("execute /help");
        match output {
            CommandOutput::Help(entries) => {
                let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
                assert!(names.contains(&"/help".to_string()));
                assert!(names.contains(&"/status".to_string()));
                assert!(names.contains(&"/model".to_string()));
                assert!(names.contains(&"/switch".to_string()));
                assert!(names.contains(&"/new".to_string()));
                assert!(names.contains(&"/sessions".to_string()));
                assert!(names.contains(&"/notify".to_string()));
                assert!(names.contains(&"/exit".to_string()));
            }
            _ => panic!("Expected Help output"),
        }
    }

    #[test]
    fn test_notify_command_execution() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");

        let output = app.execute_command("/notify").expect("execute /notify");
        match output {
            CommandOutput::Text(msg) => {
                assert!(msg.contains("NOTIFICATION & SOUND CONFIGURATION"));
                assert!(msg.contains("Master Notifications:"));
                assert!(msg.contains("Audio Sounds:"));
            }
            _ => panic!("Expected Text output for /notify"),
        }
    }

    #[test]
    fn test_status_command_execution() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let output = app.execute_command("/status").expect("execute /status");
        match output {
            CommandOutput::Status(status) => {
                assert_eq!(status.application, "Running");
                assert_eq!(status.version, APP_VERSION);
                assert_eq!(status.model, "Not configured");
                assert_eq!(status.mode, "Simple");
                assert_eq!(status.storage_status, "Ready");
                assert_eq!(status.config_status, "Ready");
                assert_ne!(status.session_id, "");
            }
            _ => panic!("Expected Status output"),
        }
    }

    #[test]
    fn test_model_command_triggers_provider_select() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let output = app.execute_command("/model").expect("execute /model");
        assert_eq!(output, CommandOutput::OpenModelSetup);
        assert_eq!(app.state(), AppState::ProviderSelect);
    }

    #[test]
    fn test_switch_command_triggers_provider_select() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let output = app.execute_command("/switch").expect("execute /switch");
        assert_eq!(output, CommandOutput::OpenModelSwitch);
        assert_eq!(app.state(), AppState::ProviderSelect);
    }

    #[test]
    fn test_sessions_command_triggers_session_select() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let output = app.execute_command("/sessions").expect("execute /sessions");
        assert_eq!(output, CommandOutput::OpenSessionPicker);
        assert_eq!(app.state(), AppState::SessionSelect);
    }

    #[test]
    fn test_new_command_creates_new_session() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        let output = app.execute_command("/new").expect("execute /new");
        assert_eq!(output, CommandOutput::NewSession);
    }

    #[test]
    fn test_exit_command_triggers_shutdown() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");

        let output = app.execute_command("/exit").expect("execute /exit");
        assert_eq!(output, CommandOutput::Exit);
        assert_eq!(app.state(), AppState::Exited);
    }

    #[test]
    fn test_unknown_command() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");

        let result = app.execute_command("/doesnotexist");
        assert!(result.is_err());
        match result {
            Err(CoreError::Command(CommandError::UnknownCommand(cmd))) => {
                assert_eq!(cmd, "/doesnotexist");
            }
            _ => panic!("Expected UnknownCommand error"),
        }
    }

    #[test]
    fn test_empty_command() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");

        let result = app.execute_command("   ");
        assert!(result.is_err());
        match result {
            Err(CoreError::Command(CommandError::EmptyInput)) => {}
            _ => panic!("Expected EmptyInput error"),
        }
    }

    #[test]
    fn test_command_aliases() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");

        let output = app.execute_command("/h").expect("execute alias /h");
        assert!(matches!(output, CommandOutput::Help(_)));

        let output_provider = app
            .execute_command("/provider")
            .expect("execute alias /provider");
        assert_eq!(output_provider, CommandOutput::OpenModelSetup);

        let output_history = app
            .execute_command("/history")
            .expect("execute alias /history");
        assert_eq!(output_history, CommandOutput::OpenSessionPicker);

        let output_mcp = app.execute_command("/mcp").expect("execute /mcp");
        assert!(
            matches!(output_mcp, CommandOutput::Text(t) if t.contains("MODEL CONTEXT PROTOCOL"))
        );
    }

    #[tokio::test]
    async fn test_mcp_auth_token_storage_lifecycle() {
        let (app, _dir) = create_test_app();

        assert_eq!(
            app.mcp_auth_token("github")
                .await
                .expect("read missing MCP token"),
            None
        );

        app.store_mcp_auth_token("github", Some("test-mcp-token"))
            .await
            .expect("store MCP token");
        assert_eq!(
            app.mcp_auth_token("github").await.expect("read MCP token"),
            Some("test-mcp-token".to_string())
        );

        let stored = app
            .credential_backend()
            .get_credential("mcp:github")
            .await
            .expect("read namespaced credential")
            .expect("stored MCP credential");
        assert_eq!(stored.provider_id, "mcp:github");

        assert!(app
            .delete_mcp_auth_token("github")
            .await
            .expect("delete MCP token"));
        assert_eq!(
            app.mcp_auth_token("github")
                .await
                .expect("read deleted MCP token"),
            None
        );
    }

    #[tokio::test]
    async fn test_mcp_add_and_remove_keep_manager_configuration_synchronized() {
        let (mut app, dir) = create_test_app();
        app.init().expect("app init");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        app.add_mcp_server(
            "test-server",
            "stdio",
            "__hades_missing_mcp_test_server__",
            "",
            "",
            Some("test-mcp-token"),
        )
        .await
        .expect("add MCP server");

        assert!(app.config().mcp.servers.contains_key("test-server"));
        assert!(app.config().mcp.servers["test-server"].auto_start);
        assert_eq!(
            app.mcp_auth_token("test-server")
                .await
                .expect("read stored MCP token"),
            Some("test-mcp-token".to_string())
        );
        let config_contents =
            std::fs::read_to_string(dir.path().join("config.toml")).expect("read MCP config");
        assert!(!config_contents.contains("test-mcp-token"));
        assert!(app
            .mcp_manager()
            .list_server_summaries()
            .await
            .iter()
            .any(|summary| summary.name == "test-server"));

        app.remove_mcp_server("test-server")
            .await
            .expect("remove MCP server");

        assert!(!app.config().mcp.servers.contains_key("test-server"));
        assert_eq!(
            app.mcp_auth_token("test-server")
                .await
                .expect("read deleted MCP token"),
            None
        );
        assert!(app.mcp_manager().list_server_summaries().await.is_empty());
    }

    #[tokio::test]
    async fn test_mcp_add_without_token_does_not_create_credential() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("app init");
        app.transition_to(AppState::Running)
            .expect("transition to running");

        app.add_mcp_server(
            "env-only-server",
            "stdio",
            "__hades_missing_mcp_test_server__",
            "",
            "HADES_TEST_MCP_TOKEN",
            None,
        )
        .await
        .expect("add MCP server without token");

        assert_eq!(
            app.mcp_auth_token("env-only-server")
                .await
                .expect("read missing MCP token"),
            None
        );
        assert_eq!(
            app.config()
                .mcp
                .servers
                .get("env-only-server")
                .and_then(|server| server.token_env.as_deref()),
            Some("HADES_TEST_MCP_TOKEN")
        );
    }

    #[tokio::test]
    async fn test_export_and_import_commands() {
        let (mut app, dir) = create_test_app();
        app.init().expect("app init");
        app.create_new_session(Some("Export Import Test".to_string()))
            .await
            .expect("create session");

        if let Some(session) = app.active_session_mut() {
            session.add_message(Message::user(&session.metadata.id, "Explain microservices"));
            session.add_message(Message::assistant(
                &session.metadata.id,
                "Microservices divide applications into independent services.",
                Some("groq".to_string()),
                Some("llama-3.3-70b-versatile".to_string()),
            ));
        }

        // Test export command to Markdown
        let export_md_path = dir.path().join("exported.md");
        let export_md_cmd = format!("/export md {}", export_md_path.display());
        let output = app
            .execute_command(&export_md_cmd)
            .expect("execute /export md");
        assert!(
            matches!(output, CommandOutput::ExportSuccess(ref p) if p.ends_with("exported.md"))
        );
        assert!(export_md_path.exists());

        // Test export command to JSON
        let export_json_path = dir.path().join("exported.json");
        let export_json_cmd = format!("/export json {}", export_json_path.display());
        let output_json = app
            .execute_command(&export_json_cmd)
            .expect("execute /export json");
        assert!(
            matches!(output_json, CommandOutput::ExportSuccess(ref p) if p.ends_with("exported.json"))
        );
        assert!(export_json_path.exists());

        // Test importing the exported Markdown file
        let import_cmd = format!("/import {}", export_md_path.display());
        let import_output = app.execute_command(&import_cmd).expect("execute /import");
        assert!(matches!(import_output, CommandOutput::ImportSuccess(_)));

        assert_eq!(
            app.active_session().unwrap().metadata.title,
            "Export Import Test"
        );
        assert_eq!(app.active_session().unwrap().messages.len(), 2);
    }

    #[tokio::test]
    async fn test_context_manager_truncation_and_preservation() {
        let mut cm = ContextManager::new();
        // Register small context limit for testing (e.g. 100 tokens)
        cm.register_model_limit("test-small", 100);

        let mut history = Vec::new();
        for i in 0..10 {
            let msg = Message::user(
                "session-1",
                format!(
                    "Message number {} explaining complex system design concepts",
                    i
                ),
            );
            history.push(msg);
        }

        let current_prompt = "What is the summary?";
        let (context_messages, report) = cm
            .build_context(&history, "test-small", None, current_prompt)
            .expect("build context");

        assert!(report.was_truncated);
        assert!(report.included_messages < history.len() + 1);
        // Current prompt must ALWAYS be the last message
        assert_eq!(
            context_messages.last().unwrap().content.as_deref(),
            Some(current_prompt)
        );
    }

    #[tokio::test]
    async fn test_session_switching_and_isolation() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("init");

        // 1. Create Session A
        let s_a = app
            .create_new_session(Some("Session A".to_string()))
            .await
            .expect("create A");
        assert_eq!(s_a.metadata.title, "Session A");

        // Add message to Session A
        let msg_a = Message::user(&s_a.metadata.id, "Hello in session A");
        if let Some(s) = app.active_session_mut() {
            s.add_message(msg_a);
        }

        // 2. Create Session B
        let s_b = app
            .create_new_session(Some("Session B".to_string()))
            .await
            .expect("create B");
        assert_eq!(s_b.metadata.title, "Session B");

        // Add message to Session B
        let msg_b = Message::user(&s_b.metadata.id, "Hello in session B");
        if let Some(s) = app.active_session_mut() {
            s.add_message(msg_b);
        }

        // 3. Switch back to Session A
        let loaded_a = app
            .switch_session(&s_a.metadata.id)
            .await
            .expect("switch to A");
        assert_eq!(loaded_a.messages.len(), 1);
        assert_eq!(loaded_a.messages[0].content, "Hello in session A");

        // 4. Switch to Session B
        let loaded_b = app
            .switch_session(&s_b.metadata.id)
            .await
            .expect("switch to B");
        assert_eq!(loaded_b.messages.len(), 1);
        assert_eq!(loaded_b.messages[0].content, "Hello in session B");
    }

    #[tokio::test]
    async fn test_normal_startup_creates_new_session_every_time() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("init");

        // First launch initializes session 1
        app.init_session(None).await.expect("init session 1");
        let id_1 = app.active_session().unwrap().metadata.id.clone();

        // Normal second launch creates a completely new session (does not auto-restore id_1)
        app.init_session(None).await.expect("init session 2");
        let id_2 = app.active_session().unwrap().metadata.id.clone();

        assert_ne!(id_1, id_2);

        let sessions = app.list_sessions().await.expect("list sessions");
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_explicit_session_resumption_success_and_failure() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("init");

        // 1. Create a session and add messages
        let s = app
            .create_new_session(Some("Persistent Session".to_string()))
            .await
            .expect("create");
        let sid = s.metadata.id.clone();
        if let Some(active) = app.active_session_mut() {
            active.add_message(Message::user(&sid, "Stored prompt"));
        }
        app.save_active_session().await.expect("save");

        // 2. Explicitly resume that session
        let warn = app.init_session(Some(&sid)).await.expect("explicit resume");
        assert!(warn.is_none());
        assert_eq!(app.active_session().unwrap().metadata.id, sid);
        assert_eq!(app.active_session().unwrap().messages.len(), 1);
        assert_eq!(
            app.active_session().unwrap().messages[0].content,
            "Stored prompt"
        );

        // 3. Explicitly resume a non-existent session
        let warn_missing = app
            .init_session(Some("non-existent-uuid"))
            .await
            .expect("missing session resume");
        assert!(warn_missing.is_some());
        assert!(warn_missing
            .unwrap()
            .contains("Hades could not find session: non-existent-uuid"));
        // Hades remains in a valid usable state with a fresh session
        assert!(app.active_session().is_some());
        assert_ne!(
            app.active_session().unwrap().metadata.id,
            "non-existent-uuid"
        );
    }

    #[tokio::test]
    async fn test_session_rename_and_deletion_lifecycle() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("init");

        let s = app
            .create_new_session(Some("Original Title".to_string()))
            .await
            .expect("create");
        let sid = s.metadata.id.clone();

        // Rename session
        app.rename_session(&sid, "Updated Title")
            .await
            .expect("rename");
        assert_eq!(
            app.active_session().unwrap().metadata.title,
            "Updated Title"
        );

        // Verify in storage
        let loaded = app
            .session_repository()
            .get_session(&sid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.metadata.title, "Updated Title");

        // Delete active session
        let deleted = app.delete_session(&sid).await.expect("delete");
        assert!(deleted);

        // Active session automatically replaced by fresh new session
        assert!(app.active_session().is_some());
        assert_ne!(app.active_session().unwrap().metadata.id, sid);
    }

    #[tokio::test]
    async fn test_agent_system_prompt_and_tool_payloads() {
        let (mut app, _dir) = create_test_app();
        app.init().expect("init");

        let sys = app.build_system_prompt();
        assert!(sys.contains("You are Hades"));
        assert!(sys.contains("CURRENT WORKSPACE ENVIRONMENT:"));
        assert!(sys.contains("TOOL USE POLICY & INSTRUCTIONS:"));

        let tools = app.provider_tool_definitions();
        assert!(!tools.is_empty());
        assert!(tools.iter().any(|t| t.function.name == "filesystem.read"));
        assert!(tools.iter().any(|t| t.function.name == "filesystem.list"));
        assert!(tools.iter().any(|t| t.function.name == "filesystem.create"));
        assert!(tools.iter().any(|t| t.function.name == "shell.execute"));
    }
}
