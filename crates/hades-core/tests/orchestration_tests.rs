use std::path::PathBuf;

use hades_core::orchestration::*;
use hades_core::TokenEstimator;
use hades_storage::Message;
use hades_tools::{
    EvaluationResult, PermissionEngine, RiskLevel, ToolCall, ToolContext, ToolDefinition,
    ToolRegistry, WorkspaceDetector, WorkspaceMetadata,
};

/// Helper to simulate a realistic GitHub MCP server toolset (44 tools).
fn create_simulated_github_tools() -> Vec<ToolDefinition> {
    let mut tools = Vec::new();
    let github_tool_names = [
        (
            "github.list_issues",
            "List issues in a repository",
            false,
            RiskLevel::Low,
        ),
        (
            "github.get_issue",
            "Get details of a specific issue",
            false,
            RiskLevel::Low,
        ),
        (
            "github.create_issue",
            "Create a new issue in a repository",
            true,
            RiskLevel::Medium,
        ),
        (
            "github.update_issue",
            "Update an existing issue",
            true,
            RiskLevel::Medium,
        ),
        (
            "github.add_issue_comment",
            "Add a comment to an issue",
            true,
            RiskLevel::Low,
        ),
        (
            "github.list_pull_requests",
            "List pull requests in a repository",
            false,
            RiskLevel::Low,
        ),
        (
            "github.get_pull_request",
            "Get details of a pull request",
            false,
            RiskLevel::Low,
        ),
        (
            "github.create_pull_request",
            "Create a pull request",
            true,
            RiskLevel::High,
        ),
        (
            "github.merge_pull_request",
            "Merge a pull request",
            true,
            RiskLevel::Critical,
        ),
        (
            "github.get_repository",
            "Get repository metadata",
            false,
            RiskLevel::Low,
        ),
        (
            "github.search_repositories",
            "Search GitHub repositories",
            false,
            RiskLevel::Low,
        ),
        (
            "github.list_commits",
            "List commits on a branch",
            false,
            RiskLevel::Low,
        ),
        (
            "github.get_commit",
            "Get commit details",
            false,
            RiskLevel::Low,
        ),
        (
            "github.list_branches",
            "List repository branches",
            false,
            RiskLevel::Low,
        ),
        (
            "github.create_branch",
            "Create a new git branch",
            true,
            RiskLevel::Medium,
        ),
        (
            "github.delete_branch",
            "Delete a git branch",
            true,
            RiskLevel::High,
        ),
        (
            "github.list_tags",
            "List repository tags",
            false,
            RiskLevel::Low,
        ),
        (
            "github.get_release",
            "Get repository release",
            false,
            RiskLevel::Low,
        ),
        (
            "github.create_release",
            "Create a new release",
            true,
            RiskLevel::High,
        ),
        (
            "github.list_collaborators",
            "List repository collaborators",
            false,
            RiskLevel::Low,
        ),
        (
            "github.add_collaborator",
            "Add a collaborator",
            true,
            RiskLevel::High,
        ),
        (
            "github.remove_collaborator",
            "Remove a collaborator",
            true,
            RiskLevel::High,
        ),
        (
            "github.list_workflows",
            "List GitHub Actions workflows",
            false,
            RiskLevel::Low,
        ),
        (
            "github.trigger_workflow",
            "Trigger a workflow run",
            true,
            RiskLevel::Medium,
        ),
        (
            "github.get_workflow_run",
            "Get workflow run details",
            false,
            RiskLevel::Low,
        ),
        (
            "github.list_workflow_runs",
            "List workflow runs",
            false,
            RiskLevel::Low,
        ),
        (
            "github.delete_workflow_run",
            "Delete workflow run logs",
            true,
            RiskLevel::Medium,
        ),
        (
            "github.get_file_contents",
            "Get repository file contents",
            false,
            RiskLevel::Low,
        ),
        (
            "github.create_or_update_file",
            "Commit a file to repository",
            true,
            RiskLevel::High,
        ),
        (
            "github.delete_file",
            "Delete a file in repository",
            true,
            RiskLevel::High,
        ),
        (
            "github.list_deployments",
            "List deployments",
            false,
            RiskLevel::Low,
        ),
        (
            "github.create_deployment",
            "Create a deployment",
            true,
            RiskLevel::High,
        ),
        (
            "github.list_environments",
            "List deployment environments",
            false,
            RiskLevel::Low,
        ),
        ("github.get_user", "Get user profile", false, RiskLevel::Low),
        (
            "github.get_authenticated_user",
            "Get current user profile",
            false,
            RiskLevel::Low,
        ),
        (
            "github.list_org_repos",
            "List organization repositories",
            false,
            RiskLevel::Low,
        ),
        (
            "github.list_teams",
            "List organization teams",
            false,
            RiskLevel::Low,
        ),
        (
            "github.list_team_members",
            "List members of a team",
            false,
            RiskLevel::Low,
        ),
        (
            "github.create_gist",
            "Create a new GitHub Gist",
            true,
            RiskLevel::Low,
        ),
        (
            "github.delete_gist",
            "Delete a GitHub Gist",
            true,
            RiskLevel::Medium,
        ),
        (
            "github.star_repository",
            "Star a repository",
            true,
            RiskLevel::Low,
        ),
        (
            "github.unstar_repository",
            "Unstar a repository",
            true,
            RiskLevel::Low,
        ),
        (
            "github.fork_repository",
            "Fork a repository",
            true,
            RiskLevel::Medium,
        ),
        (
            "github.delete_repository",
            "Delete an entire repository",
            true,
            RiskLevel::Critical,
        ),
    ];

    for (name, desc, mutating, risk) in github_tool_names {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "owner": { "type": "string", "description": "Repository owner" },
                "repo": { "type": "string", "description": "Repository name" },
                "query": { "type": "string", "description": "Optional search query" }
            },
            "required": ["owner", "repo"]
        });
        tools.push(ToolDefinition::new(name, desc, schema, risk, mutating));
    }

    tools
}

/// Helper to assemble full toolset (29 built-ins + 44 GitHub tools = 73 tools).
fn create_test_toolset() -> Vec<ToolDefinition> {
    let mut tools = ToolRegistry::default_registry().list();
    tools.extend(create_simulated_github_tools());
    tools
}

fn dummy_workspace() -> WorkspaceMetadata {
    WorkspaceDetector::detect(&PathBuf::from("."))
}

// --------------------------------------------------------------------------
// TEST A: Generic Question
// "What is 2 + 2?" -> 0 MCP tools exposed (Tier 0)
// --------------------------------------------------------------------------
#[test]
fn test_a_generic_question_zero_tools_exposed() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let tool_defs = create_test_toolset();
    let servers = vec!["github".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "What is 2 + 2?",
        &[],
        &tool_defs,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    assert_eq!(result.plan.selection_tier, 0, "Should be Tier 0");
    assert!(
        result.tools.is_empty(),
        "Tools payload must be completely empty"
    );
    assert_eq!(result.plan.selected_tools.len(), 0);
    assert_eq!(result.plan.excluded_tools_count, tool_defs.len());
    assert_eq!(result.plan.estimated_tool_tokens, 0);
}

// --------------------------------------------------------------------------
// TEST B: GitHub Issue Question
// "List open issues in PareekshithPalat/HADES_CLI"
// Expected: GitHub selected. Issue-related tools selected. Unrelated 40+ tools excluded.
// --------------------------------------------------------------------------
#[test]
fn test_b_github_issue_question_selects_only_relevant_tools() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let tool_defs = create_test_toolset();
    let servers = vec!["github".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "List open issues in PareekshithPalat/HADES_CLI",
        &[],
        &tool_defs,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    assert_eq!(
        result.plan.selection_tier, 2,
        "Should be Tier 2 (Domain-specific MCP)"
    );
    assert_eq!(result.plan.selected_server, Some("github".to_string()));

    // Must include list_issues
    assert!(
        result
            .plan
            .selected_tools
            .contains(&"github.list_issues".to_string()),
        "Must select github.list_issues"
    );

    // Must NOT include unrelated destructive or PR tools
    assert!(!result
        .plan
        .selected_tools
        .contains(&"github.delete_repository".to_string()));
    assert!(!result
        .plan
        .selected_tools
        .contains(&"github.merge_pull_request".to_string()));
    assert!(!result
        .plan
        .selected_tools
        .contains(&"github.create_release".to_string()));
    assert!(!result
        .plan
        .selected_tools
        .contains(&"filesystem.delete".to_string()));

    // Selected count must be small (e.g. <= 5) rather than 73 tools
    assert!(result.plan.selected_tools.len() <= 5);
    assert!(result.plan.excluded_tools_count >= 65);
}

// --------------------------------------------------------------------------
// TEST C: GitHub PR Question
// Expected: PR-related tools selected. Issue creation and unrelated tools excluded.
// --------------------------------------------------------------------------
#[test]
fn test_c_github_pr_question_selects_pr_tools() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let tool_defs = create_test_toolset();
    let servers = vec!["github".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "Review the pull requests in this repo and check their status",
        &[],
        &tool_defs,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    assert!(result
        .plan
        .selected_tools
        .contains(&"github.list_pull_requests".to_string()));
    assert!(!result
        .plan
        .selected_tools
        .contains(&"github.create_issue".to_string()));
    assert!(!result
        .plan
        .selected_tools
        .contains(&"github.delete_repository".to_string()));
}

// --------------------------------------------------------------------------
// TEST D: Filesystem Task
// Expected: filesystem tools selected. GitHub excluded.
// --------------------------------------------------------------------------
#[test]
fn test_d_filesystem_task_excludes_github() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let tool_defs = create_test_toolset();
    let servers = vec!["github".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "Create a file called test.txt with hello world",
        &[],
        &tool_defs,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    assert_eq!(
        result.plan.selection_tier, 1,
        "Should be Tier 1 (Small local built-in set)"
    );
    assert_eq!(result.plan.task_domain, "filesystem");

    // Must include filesystem create or write
    let has_fs_tool = result
        .plan
        .selected_tools
        .iter()
        .any(|t| t.starts_with("filesystem."));
    assert!(has_fs_tool, "Must include filesystem tools");

    // Must NOT include any GitHub tools
    let has_gh_tool = result
        .plan
        .selected_tools
        .iter()
        .any(|t| t.starts_with("github."));
    assert!(
        !has_gh_tool,
        "Must NOT include GitHub tools for a local file creation task"
    );
}

// --------------------------------------------------------------------------
// TEST E: Multi-Domain Task
// Expected: Only required capabilities from both domains.
// --------------------------------------------------------------------------
#[test]
fn test_e_multi_domain_task_combines_selectively() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let tool_defs = create_test_toolset();
    let servers = vec!["github".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "Check the GitHub issues and write a summary to issues_summary.txt",
        &[],
        &tool_defs,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    assert_eq!(
        result.plan.selection_tier, 3,
        "Should be Tier 3 (Multi-domain)"
    );
    let has_gh = result
        .plan
        .selected_tools
        .iter()
        .any(|t| t.starts_with("github."));
    let has_fs = result
        .plan
        .selected_tools
        .iter()
        .any(|t| t.starts_with("filesystem."));

    assert!(has_gh, "Must include GitHub issue tool");
    assert!(has_fs, "Must include Filesystem write tool");

    // Still bounded and must exclude unrelated tools
    assert!(result.plan.selected_tools.len() <= 6);
    assert!(!result
        .plan
        .selected_tools
        .contains(&"github.delete_repository".to_string()));
}

// --------------------------------------------------------------------------
// TEST F: Explicit MCP Request
// Expected: Requested MCP server receives strong selection priority.
// --------------------------------------------------------------------------
#[test]
fn test_f_explicit_mcp_request_gives_strong_priority() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let tool_defs = create_test_toolset();
    let servers = vec!["github".to_string(), "linear".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "Use GitHub MCP to check the repository issues",
        &[],
        &tool_defs,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    assert_eq!(result.plan.selected_server, Some("github".to_string()));
    assert!(result
        .plan
        .selected_tools
        .iter()
        .any(|t| t.starts_with("github.")));
}

// --------------------------------------------------------------------------
// TEST G: Large Tool Registry
// Simulate 100+ tools. Verify request schema size remains strictly bounded.
// --------------------------------------------------------------------------
#[test]
fn test_g_large_tool_registry_remains_strictly_bounded() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let mut tools = create_test_toolset();

    // Add 80 simulated custom tools across various namespaces
    for i in 0..80 {
        let name = format!("custom_server_{}.tool_{}", i % 5, i);
        let schema = serde_json::json!({ "type": "object", "properties": { "param": { "type": "string" } } });
        tools.push(ToolDefinition::new(
            name,
            "Simulated tool",
            schema,
            RiskLevel::Low,
            false,
        ));
    }

    assert!(tools.len() > 150, "Tool registry should have 150+ tools");

    let servers = vec!["github".to_string(), "custom_server_0".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "List open issues in PareekshithPalat/HADES_CLI",
        &[],
        &tools,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    // Must still select <= 6 tools
    assert!(result.plan.selected_tools.len() <= 6);
    assert!(result.tools.len() <= 6);
    assert!(result.plan.excluded_tools_count >= 140);
    assert!(result.plan.estimated_tool_tokens < 2_000);
}

// --------------------------------------------------------------------------
// TEST H: Groq-like 8K Budget
// Verify planner keeps request well below the 8,000 TPM limit (max 6,000 input tokens).
// --------------------------------------------------------------------------
#[test]
fn test_h_groq_8k_budget_enforcement() {
    let profile = ProviderTokenProfile::for_model("groq", "openai/gpt-oss-20b", None);
    assert_eq!(profile.tpm_limit, Some(8_000));
    assert_eq!(profile.max_request_input_tokens, 6_000);

    let mut orchestrator = SmartContextOrchestrator::new();
    let tools = create_test_toolset();
    let servers = vec!["github".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "List open issues in PareekshithPalat/HADES_CLI",
        &[],
        &tools,
        &servers,
        "groq",
        "openai/gpt-oss-20b",
        &ws,
    );

    assert!(
        result.plan.estimated_total_tokens <= profile.max_request_input_tokens,
        "Total request tokens {} must not exceed budget {}",
        result.plan.estimated_total_tokens,
        profile.max_request_input_tokens
    );
}

// --------------------------------------------------------------------------
// TEST I: Tool Result Explosion
// Simulate a tool returning huge output (100 items). Verify result budgeting.
// --------------------------------------------------------------------------
#[test]
fn test_i_tool_result_budgeting_compresses_massive_output() {
    // Generate a massive array of 100 JSON items
    let mut large_items = Vec::new();
    for i in 1..=100 {
        large_items.push(serde_json::json!({
            "id": i,
            "title": format!("Issue title number {i}"),
            "body": "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.",
            "state": "open"
        }));
    }
    let raw_output = serde_json::to_string_pretty(&large_items).unwrap();
    let raw_tokens = TokenEstimator::estimate_tokens(&raw_output);
    assert!(raw_tokens > 2_500, "Raw output should be large");

    let (compressed, was_compressed) =
        SmartContextBuilder::compress_tool_result(&raw_output, 1_200);
    assert!(was_compressed, "Output should have been compressed");
    let compressed_tokens = TokenEstimator::estimate_tokens(&compressed);
    assert!(
        compressed_tokens <= 1_200,
        "Compressed output must be <= 1200 tokens"
    );
    assert!(compressed.contains("additional items omitted for token efficiency"));
}

// --------------------------------------------------------------------------
// TEST J: Tool Loop Optimization
// Verify irrelevant tools are not reintroduced after tool execution.
// --------------------------------------------------------------------------
#[test]
fn test_j_tool_loop_continuation_does_not_reintroduce_all_tools() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let tools = create_test_toolset();
    let servers = vec!["github".to_string()];
    let ws = dummy_workspace();

    // Create session history where github.list_issues just ran
    let session_id = "test-session";
    let history = vec![
        Message::user(session_id, "List open issues and summarize them"),
        Message::assistant_with_tools(
            session_id,
            "",
            serde_json::json!([{
                "id": "call_1",
                "type": "function",
                "function": { "name": "github.list_issues", "arguments": "{}" }
            }])
            .to_string(),
            Some("groq".to_string()),
            Some("llama-3.3-70b-versatile".to_string()),
        ),
        Message::tool_result(
            session_id,
            "call_1",
            "[{\"id\": 1, \"title\": \"Bug fix\"}]",
        ),
    ];

    let result = orchestrator.orchestrate_continuation(
        &history,
        &tools,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    // In continuation summarization turn, tools should be minimal (<= 2) or 0, definitely not 73 tools!
    assert!(result.tools.len() <= 2);
    assert!(result.plan.excluded_tools_count >= 70);
}

// --------------------------------------------------------------------------
// TEST K: Ambiguous Task
// Verify HADES starts with minimal tools rather than exposing all 73 tools.
// --------------------------------------------------------------------------
#[test]
fn test_k_ambiguous_task_starts_minimal() {
    let mut orchestrator = SmartContextOrchestrator::new();
    let tools = create_test_toolset();
    let servers = vec!["github".to_string()];
    let ws = dummy_workspace();

    let result = orchestrator.orchestrate(
        "Can you help me investigate what is happening?",
        &[],
        &tools,
        &servers,
        "groq",
        "llama-3.3-70b-versatile",
        &ws,
    );

    // Should NOT expose all 44 GitHub tools for an ambiguous query
    let gh_count = result
        .plan
        .selected_tools
        .iter()
        .filter(|t| t.starts_with("github."))
        .count();
    assert_eq!(
        gh_count, 0,
        "Ambiguous general question should not expose GitHub tools"
    );
}

// --------------------------------------------------------------------------
// TEST L: Provider without TPM Metadata
// Verify safe fallback behavior.
// --------------------------------------------------------------------------
#[test]
fn test_l_provider_without_tpm_metadata_uses_safe_fallback() {
    let profile = ProviderTokenProfile::for_model("custom_provider", "custom_model", Some(16_384));
    assert_eq!(profile.tpm_limit, None);
    assert_eq!(profile.context_window, 16_384);
    assert!(profile.max_request_input_tokens > 0);
    assert!(profile.max_request_input_tokens < 16_384);
}

// --------------------------------------------------------------------------
// TEST M: Permissions and Safety are Preserved
// Tool selection does not bypass PermissionEngine.
// --------------------------------------------------------------------------
#[tokio::test]
async fn test_m_permissions_are_not_bypassed_by_orchestration() {
    let engine = PermissionEngine::new();
    let context = ToolContext::new("test-session", PathBuf::from("."), PathBuf::from("."));

    // Even if github.create_issue is selected by the orchestrator,
    // it is high/medium risk and must evaluate through PermissionEngine
    let call = ToolCall::new(
        "call-1",
        "github.create_issue",
        serde_json::json!({ "title": "New Bug" }),
    );
    let schema = serde_json::json!({ "type": "object" });
    let def = ToolDefinition::new(
        "github.create_issue",
        "Create issue",
        schema,
        RiskLevel::Medium,
        true,
    );
    let decision = engine.evaluate(&call, &def, &context);

    // Must still require interactive user approval
    assert!(matches!(
        decision,
        EvaluationResult::RequiresApproval { .. }
    ));
}
