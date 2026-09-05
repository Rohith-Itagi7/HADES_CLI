use serde::{Deserialize, Serialize};
use tracing::info;

use hades_provider::{ChatMessage, ToolDefinitionPayload};
use hades_storage::Message;
use hades_tools::{ToolDefinition, WorkspaceMetadata};

use super::budget::{ProviderTokenProfile, TokenBudgetManager};
use super::context::SmartContextBuilder;
use super::intent::TaskIntentAnalyzer;
use super::metadata::CapabilityIndex;
use super::relevance::ToolRelevanceEngine;
use crate::context::TokenEstimator;

/// Concise machine-readable planning metadata generated before every LLM request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPlan {
    pub task_domain: String,
    pub selected_server: Option<String>,
    pub selected_tools: Vec<String>,
    pub candidate_tools_count: usize,
    pub available_tools_count: usize,
    pub excluded_tools_count: usize,
    pub estimated_system_tokens: usize,
    pub estimated_context_tokens: usize,
    pub estimated_tool_tokens: usize,
    pub estimated_total_tokens: usize,
    pub token_budget: usize,
    pub provider_tpm_limit: Option<usize>,
    pub selection_tier: usize,
    pub reasoning: String,
}

/// Outcome of the smart orchestration pipeline ready to feed into `CompletionRequest`.
#[derive(Debug, Clone)]
pub struct OrchestrationResult {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinitionPayload>,
    pub plan: RequestPlan,
}

/// Central coordinator for the Smart Tool & Context Orchestration layer.
#[derive(Debug, Clone, Default)]
pub struct SmartContextOrchestrator {
    recent_tools: Vec<String>,
}

impl SmartContextOrchestrator {
    pub fn new() -> Self {
        Self {
            recent_tools: Vec::new(),
        }
    }

    /// Records executed tool name to inform future turn relevance in multi-step loops.
    pub fn record_tool_execution(&mut self, tool_name: &str) {
        if !self.recent_tools.contains(&tool_name.to_string()) {
            self.recent_tools.push(tool_name.to_string());
        }
        if self.recent_tools.len() > 10 {
            self.recent_tools.remove(0);
        }
    }

    /// Primary orchestration entry point for a user turn.
    #[allow(clippy::too_many_arguments)]
    pub fn orchestrate(
        &mut self,
        prompt: &str,
        history: &[Message],
        all_tool_defs: &[ToolDefinition],
        active_mcp_servers: &[String],
        provider_id: &str,
        model_id: &str,
        workspace: &WorkspaceMetadata,
    ) -> OrchestrationResult {
        // 1. Build capability index over all registered tools
        let index = CapabilityIndex::build(all_tool_defs, active_mcp_servers);
        let available_count = index.count();

        // 2. Classify task intent deterministically
        let intent =
            TaskIntentAnalyzer::analyze(prompt, active_mcp_servers, index.all_tool_names());

        // 3. Resolve provider token profile (distinguishing Context Window vs TPM limit)
        let profile = ProviderTokenProfile::for_model(provider_id, model_id, None);

        // 4. Rank and select relevant tools
        let mut selection =
            ToolRelevanceEngine::select_tools(&intent, &index, &self.recent_tools, 6);

        // 5. Adaptive replanning: Ensure selected tools fit token budget
        let system_prompt_initial =
            SmartContextBuilder::build_system_prompt(workspace, &selection.selected_tools);
        let system_tokens_est = TokenEstimator::estimate_tokens(&system_prompt_initial) + 4;
        let prompt_tokens_est = TokenEstimator::estimate_tokens(prompt) + 4;

        while selection.estimated_schema_tokens + system_tokens_est + prompt_tokens_est
            > profile.max_request_input_tokens
            && selection.selected_tools.len() > 1
        {
            // Drop lowest-ranked tool to save tokens
            let dropped = selection.selected_tools.pop();
            if let Some(name) = dropped {
                if let Some(m) = index.get(&name) {
                    selection.estimated_schema_tokens = selection
                        .estimated_schema_tokens
                        .saturating_sub(m.estimated_schema_tokens);
                }
            }
        }

        // 6. Finalize compact system prompt
        let final_system_prompt =
            SmartContextBuilder::build_system_prompt(workspace, &selection.selected_tools);
        let final_system_tokens = TokenEstimator::estimate_tokens(&final_system_prompt) + 4;

        // 7. Calculate remaining history budget and build message payload
        let available_history = TokenBudgetManager::available_history_budget(
            &profile,
            final_system_tokens,
            prompt_tokens_est,
            selection.estimated_schema_tokens,
        );

        let (messages, _included_msgs, _was_truncated) = SmartContextBuilder::build_messages(
            history,
            &final_system_prompt,
            prompt,
            available_history,
        );

        // 8. Attach full schemas ONLY for the selected tools
        let selected_payloads: Vec<ToolDefinitionPayload> = if selection.selected_tools.is_empty() {
            Vec::new()
        } else {
            all_tool_defs
                .iter()
                .filter(|def| selection.selected_tools.contains(&def.name))
                .map(|def| {
                    ToolDefinitionPayload::function(
                        def.name.clone(),
                        def.description.clone(),
                        def.parameters_schema.clone(),
                    )
                })
                .collect()
        };

        let est_context_tokens: usize = messages
            .iter()
            .map(|m| TokenEstimator::estimate_tokens(m.content.as_deref().unwrap_or_default()) + 6)
            .sum();

        let total_est = est_context_tokens + selection.estimated_schema_tokens;

        let primary_server = match &intent.primary_domain {
            super::intent::TaskDomain::Mcp(s) => Some(s.clone()),
            _ => None,
        };

        info!(
            domain = %intent.primary_domain.name(),
            tier = selection.tier,
            available = available_count,
            selected = selection.selected_tools.len(),
            excluded = selection.excluded_tool_count,
            est_tokens = total_est,
            budget = profile.max_request_input_tokens,
            "Smart orchestration completed"
        );

        let plan = RequestPlan {
            task_domain: intent.primary_domain.name(),
            selected_server: primary_server,
            selected_tools: selection.selected_tools,
            candidate_tools_count: selection.candidate_tools.len(),
            available_tools_count: available_count,
            excluded_tools_count: selection.excluded_tool_count,
            estimated_system_tokens: final_system_tokens,
            estimated_context_tokens: est_context_tokens,
            estimated_tool_tokens: selection.estimated_schema_tokens,
            estimated_total_tokens: total_est,
            token_budget: profile.max_request_input_tokens,
            provider_tpm_limit: profile.tpm_limit,
            selection_tier: selection.tier,
            reasoning: selection.reasoning,
        };

        OrchestrationResult {
            messages,
            tools: selected_payloads,
            plan,
        }
    }

    /// Orchestrates follow-up continuation requests after tool execution.
    pub fn orchestrate_continuation(
        &mut self,
        history: &[Message],
        all_tool_defs: &[ToolDefinition],
        active_mcp_servers: &[String],
        provider_id: &str,
        model_id: &str,
        workspace: &WorkspaceMetadata,
    ) -> OrchestrationResult {
        // Inspect the last tool call and execution result
        let last_tool_call_name = history.iter().rev().find_map(|m| {
            if let Some(ref tc_json) = m.metadata.tool_calls {
                if let Ok(calls) =
                    serde_json::from_str::<Vec<hades_provider::ProviderToolCall>>(tc_json)
                {
                    return calls.into_iter().next().map(|c| c.function.name);
                }
            }
            None
        });

        // If the last tool was a query/read tool and returned data, model typically needs to formulate the response
        let is_summarization_turn = last_tool_call_name.as_deref().is_some_and(|name| {
            name.contains("list")
                || name.contains("read")
                || name.contains("get")
                || name.contains("inspect")
        });

        let index = CapabilityIndex::build(all_tool_defs, active_mcp_servers);
        let available_count = index.count();
        let profile = ProviderTokenProfile::for_model(provider_id, model_id, None);

        // Continuation tool selection:
        // If summarization turn, expose only continuation read tools or 0 tools if the answer is ready
        let (selected_tools, schema_tokens) = if is_summarization_turn {
            // Keep at most 2 relevant continuation tools
            let mut tools = Vec::new();
            let mut tokens = 0;
            if let Some(ref name) = last_tool_call_name {
                if let Some(m) = index.get(name) {
                    tools.push(name.clone());
                    tokens += m.estimated_schema_tokens;
                }
            }
            (tools, tokens)
        } else {
            // Re-evaluate with previous tools
            let intent = TaskIntentAnalyzer::analyze(
                "continue task",
                active_mcp_servers,
                index.all_tool_names(),
            );
            let sel = ToolRelevanceEngine::select_tools(&intent, &index, &self.recent_tools, 3);
            (sel.selected_tools, sel.estimated_schema_tokens)
        };

        let system_prompt = SmartContextBuilder::build_system_prompt(workspace, &selected_tools);
        let system_tokens = TokenEstimator::estimate_tokens(&system_prompt) + 4;

        let available_history =
            TokenBudgetManager::available_history_budget(&profile, system_tokens, 0, schema_tokens);

        let (messages, _included_msgs, _was_truncated) =
            SmartContextBuilder::build_messages(history, &system_prompt, "", available_history);

        let selected_payloads: Vec<ToolDefinitionPayload> = all_tool_defs
            .iter()
            .filter(|def| selected_tools.contains(&def.name))
            .map(|def| {
                ToolDefinitionPayload::function(
                    def.name.clone(),
                    def.description.clone(),
                    def.parameters_schema.clone(),
                )
            })
            .collect();

        let est_context_tokens: usize = messages
            .iter()
            .map(|m| TokenEstimator::estimate_tokens(m.content.as_deref().unwrap_or_default()) + 6)
            .sum();

        let total_est = est_context_tokens + schema_tokens;

        let plan = RequestPlan {
            task_domain: "continuation".to_string(),
            selected_server: active_mcp_servers.first().cloned(),
            selected_tools: selected_tools.clone(),
            candidate_tools_count: 0,
            available_tools_count: available_count,
            excluded_tools_count: available_count.saturating_sub(selected_tools.len()),
            estimated_system_tokens: system_tokens,
            estimated_context_tokens: est_context_tokens,
            estimated_tool_tokens: schema_tokens,
            estimated_total_tokens: total_est,
            token_budget: profile.max_request_input_tokens,
            provider_tpm_limit: profile.tpm_limit,
            selection_tier: if selected_tools.is_empty() { 0 } else { 1 },
            reasoning: format!(
                "Continuation turn: {} tool(s) attached.",
                selected_tools.len()
            ),
        };

        OrchestrationResult {
            messages,
            tools: selected_payloads,
            plan,
        }
    }
}
