use hades_provider::ChatMessage;
use hades_storage::{Message, MessageRole as StorageRole};
use hades_tools::WorkspaceMetadata;

use crate::context::TokenEstimator;

/// Utilities for smart context compaction, tool result budgeting, and minimal system prompts.
pub struct SmartContextBuilder;

impl SmartContextBuilder {
    /// Maximum tokens permitted for any single tool execution result in immediate model context.
    pub const MAX_TOOL_RESULT_TOKENS: usize = 1_200;

    /// Generates a compact system instruction prompt mentioning only active capabilities.
    pub fn build_system_prompt(ws: &WorkspaceMetadata, selected_tool_names: &[String]) -> String {
        let mut sys = format!(
            "You are Hades, an autonomous AI pair programming assistant and universal coding agent.\n\
            WORKSPACE ENVIRONMENT:\n\
            - Root: {}\n\
            - Project: {}\n",
            ws.root.display(),
            ws.project_type
        );
        if ws.has_git {
            let branch = ws.git_branch.as_deref().unwrap_or("main");
            sys.push_str(&format!("- Git repository: active ({branch})\n"));
        }

        if !selected_tool_names.is_empty() {
            sys.push_str("\nACTIVE CAPABILITIES FOR CURRENT TASK:\n");
            sys.push_str(&selected_tool_names.join(", "));
            sys.push_str(
                "\nINSTRUCTIONS:\n\
                1. When tools are provided, invoke the appropriate tool directly to fulfill the user request.\n\
                2. Provide concise, direct, and factual answers once tool results are available.\n"
            );
        } else {
            sys.push_str(
                "\nINSTRUCTIONS:\n\
                Answer the user's questions clearly, concisely, and factually.\n",
            );
        }

        sys
    }

    /// Compresses a tool execution result if it exceeds the token budget.
    pub fn compress_tool_result(output: &str, max_tokens: usize) -> (String, bool) {
        let estimated = TokenEstimator::estimate_tokens(output);
        if estimated <= max_tokens {
            return (output.to_string(), false);
        }

        // 1. Try structured JSON array compaction (common for list_issues, search, etc.)
        let trimmed = output.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
                let total_items = items.len();
                if total_items > 5 {
                    let preview_count = 5;
                    let preview_items: Vec<_> = items.into_iter().take(preview_count).collect();
                    let preview_json =
                        serde_json::to_string_pretty(&preview_items).unwrap_or_default();
                    let compressed = format!(
                        "{preview_json}\n\n[... {remaining} additional items omitted for token efficiency. Total: {total_items}]",
                        remaining = total_items - preview_count,
                    );
                    return (compressed, true);
                }
            }
        }

        // 2. Line-based truncation for text/logs/file contents
        let lines: Vec<&str> = output.lines().collect();
        if lines.len() > 30 {
            let head = &lines[..20];
            let tail = &lines[lines.len() - 5..];
            let compressed = format!(
                "{}\n\n[... {} lines omitted for token budget ...]\n\n{}",
                head.join("\n"),
                lines.len() - 25,
                tail.join("\n")
            );
            return (compressed, true);
        }

        // 3. Fallback character-slice truncation
        let char_limit = max_tokens * 4;
        if output.len() > char_limit {
            let truncated = format!(
                "{}...\n[Output truncated to fit model token budget: {} chars total]",
                &output[..char_limit],
                output.len()
            );
            return (truncated, true);
        }

        (output.to_string(), false)
    }

    /// Selects and compacts session history messages to fit strictly within available budget.
    pub fn build_messages(
        history: &[Message],
        system_prompt: &str,
        current_prompt: &str,
        available_history_budget: usize,
    ) -> (Vec<ChatMessage>, usize, bool) {
        let mut selected_history: Vec<(StorageRole, String, Option<String>)> = Vec::new();
        let mut remaining_budget = available_history_budget;
        let mut was_truncated = false;
        let mut included_count = 0;

        for msg in history.iter().rev() {
            if msg.role == StorageRole::Error {
                continue;
            }

            // Compact tool result messages if large
            let (content, _was_compressed) = if msg.role == StorageRole::Tool {
                Self::compress_tool_result(&msg.content, Self::MAX_TOOL_RESULT_TOKENS)
            } else {
                (msg.content.clone(), false)
            };

            let msg_tokens = TokenEstimator::estimate_message_tokens(msg.role, &content);

            if msg_tokens <= remaining_budget {
                remaining_budget -= msg_tokens;
                included_count += 1;
                selected_history.push((msg.role, content, msg.metadata.tool_calls.clone()));
            } else {
                was_truncated = true;
            }
        }

        selected_history.reverse();

        // Construct Provider ChatMessage vector
        let mut provider_messages = Vec::new();

        // System prompt
        if !system_prompt.trim().is_empty() {
            provider_messages.push(ChatMessage::system(system_prompt));
        }

        // History
        for (role, content, tool_calls_json) in selected_history {
            match role {
                StorageRole::User => {
                    provider_messages.push(ChatMessage::user(content));
                }
                StorageRole::Assistant => {
                    let mut asst = ChatMessage::assistant(content);
                    if let Some(tc_str) = tool_calls_json {
                        if let Ok(calls) =
                            serde_json::from_str::<Vec<hades_provider::ProviderToolCall>>(&tc_str)
                        {
                            asst.tool_calls = Some(calls);
                        }
                    }
                    provider_messages.push(asst);
                }
                StorageRole::Tool => {
                    provider_messages.push(ChatMessage::tool_result("", content));
                }
                _ => {}
            }
        }

        // Current prompt (if non-empty)
        if !current_prompt.trim().is_empty() {
            provider_messages.push(ChatMessage::user(current_prompt));
        }

        (provider_messages, included_count, was_truncated)
    }
}
