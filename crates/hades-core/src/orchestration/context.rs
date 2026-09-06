use hades_provider::ChatMessage;
use hades_storage::{Message, MessageRole as StorageRole};
use hades_tools::WorkspaceMetadata;

use crate::context::TokenEstimator;

/// Utilities for smart context compaction, tool result budgeting, and minimal system prompts.
pub struct SmartContextBuilder;

impl SmartContextBuilder {
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

    /// Maximum tokens permitted for any single tool execution result in immediate model context.
    pub const MAX_TOOL_RESULT_TOKENS: usize = 800;

    /// Compresses a tool execution result if it exceeds the token budget or contains verbose metadata.
    pub fn compress_tool_result(output: &str, max_tokens: usize) -> (String, bool) {
        let trimmed = output.trim();

        // 1. Try structured JSON compaction (cleans raw MCP metadata and projects essential fields)
        if (trimmed.starts_with('[') && trimmed.ends_with(']'))
            || (trimmed.starts_with('{') && trimmed.ends_with('}'))
        {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                let cleaned = Self::clean_tool_json(&val);
                let cleaned_str = serde_json::to_string_pretty(&cleaned).unwrap_or_default();
                let est = TokenEstimator::estimate_tokens(&cleaned_str);
                if est <= max_tokens {
                    return (cleaned_str, true);
                }

                // If cleaned JSON array is still larger than max_tokens, slice array items
                if let serde_json::Value::Array(items) = cleaned {
                    let total = items.len();
                    let take_count = 5.min(total);
                    let sliced: Vec<_> = items.into_iter().take(take_count).collect();
                    let sliced_str = serde_json::to_string_pretty(&sliced).unwrap_or_default();
                    let compressed = format!(
                        "{sliced_str}\n\n[... {rem} additional items omitted for token efficiency. Total: {total}]",
                        rem = total - take_count
                    );
                    return (compressed, true);
                }
            }
        }

        let estimated = TokenEstimator::estimate_tokens(output);
        if estimated <= max_tokens {
            return (output.to_string(), false);
        }

        // 2. Line-based truncation for text/logs/file contents
        let lines: Vec<&str> = output.lines().collect();
        if lines.len() > 25 {
            let head = &lines[..15];
            let tail = &lines[lines.len().saturating_sub(5)..];
            let compressed = format!(
                "{}\n\n[... {} lines omitted for token budget ...]\n\n{}",
                head.join("\n"),
                lines.len().saturating_sub(20),
                tail.join("\n")
            );
            return (compressed, true);
        }

        // 3. Fallback character-slice truncation
        let char_limit = max_tokens * 3;
        if output.len() > char_limit {
            let truncated = format!(
                "{}...\n[Output truncated to fit token budget: {} chars total]",
                &output[..char_limit],
                output.len()
            );
            return (truncated, true);
        }

        (output.to_string(), false)
    }

    /// Recursively strips redundant metadata fields from tool output (e.g. GitHub URLs, node IDs, avatars).
    fn clean_tool_json(val: &serde_json::Value) -> serde_json::Value {
        match val {
            serde_json::Value::Array(arr) => {
                let cleaned: Vec<_> = arr.iter().map(Self::clean_tool_json).collect();
                serde_json::Value::Array(cleaned)
            }
            serde_json::Value::Object(map) => {
                let mut cleaned = serde_json::Map::new();
                for (k, v) in map {
                    // Filter out redundant URL fields, internal IDs, and reaction blocks
                    if k.ends_with("_url")
                        || k == "url"
                        || k == "node_id"
                        || k == "avatar_url"
                        || k == "gravatar_id"
                        || k == "reactions"
                        || k == "_links"
                        || k == "timeline_url"
                    {
                        continue;
                    }

                    // For 'user' or 'author' object, reduce to just login/name string if available
                    if (k == "user" || k == "author") && v.is_object() {
                        if let Some(login) = v.get("login").and_then(|l| l.as_str()) {
                            cleaned.insert(k.clone(), serde_json::Value::String(login.to_string()));
                            continue;
                        }
                    }

                    // For 'labels' array of objects, reduce to array of label names
                    if k == "labels" && v.is_array() {
                        if let Some(arr) = v.as_array() {
                            let label_names: Vec<serde_json::Value> = arr
                                .iter()
                                .filter_map(|l| {
                                    l.get("name")
                                        .and_then(|n| n.as_str())
                                        .or_else(|| l.as_str())
                                        .map(|s| serde_json::Value::String(s.to_string()))
                                })
                                .collect();
                            cleaned.insert(k.clone(), serde_json::Value::Array(label_names));
                            continue;
                        }
                    }

                    // Truncate long issue bodies / text fields to 300 characters
                    if (k == "body" || k == "content" || k == "description") && v.is_string() {
                        if let Some(s) = v.as_str() {
                            if s.len() > 300 {
                                let truncated = format!("{}... [truncated]", &s[..300]);
                                cleaned.insert(k.clone(), serde_json::Value::String(truncated));
                                continue;
                            }
                        }
                    }

                    cleaned.insert(k.clone(), Self::clean_tool_json(v));
                }
                serde_json::Value::Object(cleaned)
            }
            other => other.clone(),
        }
    }

    /// Selects and compacts session history messages to fit strictly within available budget.
    pub fn build_messages(
        history: &[Message],
        system_prompt: &str,
        current_prompt: &str,
        available_history_budget: usize,
    ) -> (Vec<ChatMessage>, usize, bool) {
        let mut selected_history: Vec<(StorageRole, String, Option<String>, Option<String>)> =
            Vec::new();
        let mut remaining_budget = available_history_budget;
        let mut was_truncated = false;
        let mut included_count = 0;
        let mut seen_active_tool_result = false;

        // Limit conversation history to the last 10 messages (5 exchanges) to avoid unbounded context accumulation
        let bounded_history: Vec<_> = history.iter().rev().take(10).collect();

        for msg in bounded_history {
            if msg.role == StorageRole::Error {
                continue;
            }

            // Differentiate active tool result from older completed historical tool results
            let content = if msg.role == StorageRole::Tool {
                if !seen_active_tool_result {
                    seen_active_tool_result = true;
                    let (c, _) =
                        Self::compress_tool_result(&msg.content, Self::MAX_TOOL_RESULT_TOKENS);
                    c
                } else {
                    // Older historical tool result: task already completed in prior turn
                    "[Previous tool result omitted: task completed in prior turn]".to_string()
                }
            } else {
                msg.content.clone()
            };

            let msg_tokens = TokenEstimator::estimate_message_tokens(msg.role, &content);

            if msg_tokens <= remaining_budget {
                remaining_budget -= msg_tokens;
                included_count += 1;
                selected_history.push((
                    msg.role,
                    content,
                    msg.metadata.tool_calls.clone(),
                    msg.metadata.tool_call_id.clone(),
                ));
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
        for (role, content, tool_calls_json, tool_call_id) in selected_history {
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
                    let id = tool_call_id.unwrap_or_else(|| "call_0".to_string());
                    provider_messages.push(ChatMessage::tool_result(id, content));
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
