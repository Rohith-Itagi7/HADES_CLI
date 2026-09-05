use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

use super::intent::{TaskDomain, TaskIntent};
use super::metadata::{CapabilityIndex, ToolMetadata};

/// Structured outcome of tool relevance evaluation and selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSelectionResult {
    /// Ordered list of selected tool names to attach to the LLM request.
    pub selected_tools: Vec<String>,
    /// Candidates that scored above baseline relevance but were omitted due to budget/tier.
    pub candidate_tools: Vec<String>,
    /// Total available tools that were excluded.
    pub excluded_tool_count: usize,
    /// Selection tier (0 = None, 1 = Small local, 2 = Domain specific, 3 = Multi-domain, 4 = Broad).
    pub tier: usize,
    /// Estimated tokens consumed by the selected tools' schemas.
    pub estimated_schema_tokens: usize,
    /// Concise explanation of why tools were or were not selected.
    pub reasoning: String,
}

/// Scored candidate tool during ranking.
#[derive(Debug, Clone)]
struct ScoredTool {
    name: String,
    score: f64,
    schema_tokens: usize,
}

/// Deterministic, fast tool relevance engine.
pub struct ToolRelevanceEngine;

impl ToolRelevanceEngine {
    /// Ranks all available tools against the analyzed task intent and selects the optimal minimal set.
    pub fn select_tools(
        intent: &TaskIntent,
        index: &CapabilityIndex,
        recent_tool_names: &[String],
        max_tools_limit: usize,
    ) -> ToolSelectionResult {
        // TIER 0: Pure reasoning / conversational questions requiring no tools
        if !intent.requires_tools || intent.primary_domain == TaskDomain::GenericReasoning {
            return ToolSelectionResult {
                selected_tools: Vec::new(),
                candidate_tools: Vec::new(),
                excluded_tool_count: index.count(),
                tier: 0,
                estimated_schema_tokens: 0,
                reasoning:
                    "Tier 0: Pure reasoning/conversational query. Zero tool schemas attached."
                        .to_string(),
            };
        }

        let mut scored_tools: Vec<ScoredTool> = Vec::new();

        for name in index.all_tool_names() {
            if let Some(meta) = index.get(name) {
                let score = Self::score_tool(meta, intent, recent_tool_names);
                if score > 20.0 {
                    scored_tools.push(ScoredTool {
                        name: name.clone(),
                        score,
                        schema_tokens: meta.estimated_schema_tokens,
                    });
                }
            }
        }

        // Sort descending by score, breaking ties by lower schema token size
        scored_tools.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.schema_tokens.cmp(&b.schema_tokens))
        });

        if scored_tools.is_empty() {
            return ToolSelectionResult {
                selected_tools: Vec::new(),
                candidate_tools: Vec::new(),
                excluded_tool_count: index.count(),
                tier: 0,
                estimated_schema_tokens: 0,
                reasoning: "No tools scored above relevance threshold for this query.".to_string(),
            };
        }

        // Determine selection tier
        let tier = match &intent.primary_domain {
            TaskDomain::Filesystem
            | TaskDomain::Workspace
            | TaskDomain::Shell
            | TaskDomain::System => {
                if intent.secondary_domains.is_empty() {
                    1 // Tier 1: Small built-in set
                } else {
                    3 // Tier 3: Multi-domain
                }
            }
            TaskDomain::Mcp(_) => {
                if intent.secondary_domains.is_empty() {
                    2 // Tier 2: Domain-specific MCP set
                } else {
                    3 // Tier 3: Multi-domain
                }
            }
            TaskDomain::MultiDomain(_) => 3,
            TaskDomain::Browser => 2,
            TaskDomain::GenericReasoning => 0,
        };

        // Determine effective tool limit based on tier
        let effective_limit = match tier {
            1 => max_tools_limit.min(4),
            2 => max_tools_limit.min(5),
            3 => max_tools_limit.min(8),
            _ => max_tools_limit,
        };

        let mut selected_tools = Vec::new();
        let mut candidate_tools = Vec::new();
        let mut total_schema_tokens = 0;

        if tier == 3 {
            // Guarantee cross-domain representation: select at least 1 top tool from each active domain
            let mut all_domains = vec![&intent.primary_domain];
            all_domains.extend(&intent.secondary_domains);

            for dom in all_domains {
                let dom_ns = match dom {
                    TaskDomain::Mcp(s) => s.as_str(),
                    TaskDomain::Filesystem => "filesystem",
                    TaskDomain::Workspace => "workspace",
                    TaskDomain::Shell => "shell",
                    TaskDomain::System => "system",
                    TaskDomain::Browser => "browser",
                    _ => "",
                };
                if !dom_ns.is_empty() {
                    if let Some(top_in_dom) = scored_tools.iter().find(|t| {
                        index.get(&t.name).is_some_and(|m| m.namespace == dom_ns)
                            && !selected_tools.contains(&t.name)
                    }) {
                        selected_tools.push(top_in_dom.name.clone());
                        total_schema_tokens += top_in_dom.schema_tokens;
                    }
                }
            }
        }

        // Fill remaining slots up to effective_limit in descending score order
        for tool in &scored_tools {
            if selected_tools.contains(&tool.name) {
                continue;
            }
            if selected_tools.len() < effective_limit {
                selected_tools.push(tool.name.clone());
                total_schema_tokens += tool.schema_tokens;
            } else {
                candidate_tools.push(tool.name.clone());
            }
        }

        let excluded_count = index.count().saturating_sub(selected_tools.len());

        let reasoning = format!(
            "Tier {}: Selected {} relevant tool(s) ({} schema tokens). Excluded {} irrelevant tools.",
            tier,
            selected_tools.len(),
            total_schema_tokens,
            excluded_count
        );

        ToolSelectionResult {
            selected_tools,
            candidate_tools,
            excluded_tool_count: excluded_count,
            tier,
            estimated_schema_tokens: total_schema_tokens,
            reasoning,
        }
    }

    /// Evaluates a single tool's semantic and task relevance.
    fn score_tool(meta: &ToolMetadata, intent: &TaskIntent, recent_tool_names: &[String]) -> f64 {
        let mut score = 0.0;
        let q_lower = intent.user_query.to_lowercase();

        // 1. Explicit tool name match (strongest signal)
        if intent.entities.explicit_tools.contains(&meta.name)
            || intent.entities.explicit_tools.contains(&meta.raw_name)
        {
            score += 150.0;
        }

        // 2. Explicit server match
        if intent.entities.explicit_servers.contains(&meta.namespace) {
            score += 90.0;
        }

        // 3. Domain matching
        let matches_primary = match &intent.primary_domain {
            TaskDomain::Mcp(server) => meta.namespace == *server,
            TaskDomain::Filesystem => meta.namespace == "filesystem",
            TaskDomain::Workspace => meta.namespace == "workspace",
            TaskDomain::Shell => meta.namespace == "shell",
            TaskDomain::System => meta.namespace == "system",
            TaskDomain::Browser => meta.namespace == "browser",
            TaskDomain::GenericReasoning => false,
            TaskDomain::MultiDomain(domains) => domains.iter().any(|d| match d {
                TaskDomain::Mcp(s) => meta.namespace == *s,
                TaskDomain::Filesystem => meta.namespace == "filesystem",
                TaskDomain::Workspace => meta.namespace == "workspace",
                TaskDomain::Shell => meta.namespace == "shell",
                TaskDomain::System => meta.namespace == "system",
                TaskDomain::Browser => meta.namespace == "browser",
                TaskDomain::GenericReasoning | TaskDomain::MultiDomain(_) => false,
            }),
        };

        if matches_primary {
            score += 60.0;
        }

        // Secondary domain matches (in multi-domain tasks, secondary domains are also essential)
        for sec in &intent.secondary_domains {
            let matches_sec = match sec {
                TaskDomain::Mcp(server) => meta.namespace == *server,
                TaskDomain::Filesystem => meta.namespace == "filesystem",
                TaskDomain::Workspace => meta.namespace == "workspace",
                TaskDomain::Shell => meta.namespace == "shell",
                TaskDomain::System => meta.namespace == "system",
                TaskDomain::Browser => meta.namespace == "browser",
                _ => false,
            };
            if matches_sec {
                score += 55.0;
            }
        }

        // Functional tag matching (e.g. "issues", "pull_requests", "files", "process", "network")
        for domain_tag in &meta.domains {
            if domain_tag == "issues"
                && (q_lower.contains("issue") || !intent.entities.repo_names.is_empty())
            {
                score += 50.0;
            }
            if domain_tag == "pull_requests"
                && (q_lower.contains("pull request")
                    || q_lower.contains("pull_request")
                    || q_lower.contains("pull requests")
                    || q_lower.contains(" pr ")
                    || q_lower.contains("prs")
                    || q_lower.ends_with(" pr")
                    || q_lower.starts_with("pr "))
            {
                score += 70.0;
            }
            if domain_tag == "files"
                && (intent.primary_domain == TaskDomain::Filesystem
                    || intent.secondary_domains.contains(&TaskDomain::Filesystem)
                    || !intent.entities.file_paths.is_empty()
                    || q_lower.contains("file"))
            {
                score += 45.0;
            }
            if domain_tag == "repositories"
                && (q_lower.contains("repo") || !intent.entities.repo_names.is_empty())
            {
                score += 25.0;
            }
            if domain_tag == "process" && q_lower.contains("process") {
                score += 40.0;
            }
            if domain_tag == "network" && (q_lower.contains("port") || q_lower.contains("network"))
            {
                score += 40.0;
            }
        }

        // 4. Read-only vs. Mutating alignment
        if intent.is_read_only {
            if meta.is_read_only {
                score += 25.0;
            } else {
                // Heavy penalty for mutating tools when user asked a read-only query
                score -= 60.0;
            }
        } else if meta.is_mutating {
            score += 20.0;
        }

        // 5. Semantic action keywords in prompt
        let raw_lower = meta.raw_name.to_lowercase();
        if (raw_lower.starts_with("list") || raw_lower.contains("list"))
            && (q_lower.contains("list")
                || q_lower.contains("show")
                || q_lower.contains("find")
                || q_lower.contains("review")
                || q_lower.contains("check")
                || q_lower.contains("status"))
        {
            score += 35.0;
        }
        if (raw_lower.starts_with("get") || raw_lower.contains("read"))
            && (q_lower.contains("get")
                || q_lower.contains("view")
                || q_lower.contains("read")
                || q_lower.contains("inspect"))
        {
            score += 25.0;
        }
        if (raw_lower.starts_with("create")
            || raw_lower.contains("create")
            || raw_lower.contains("mkdir"))
            && (q_lower.contains("create")
                || q_lower.contains("make")
                || q_lower.contains("new")
                || q_lower.contains("add"))
        {
            score += 40.0;
        }
        if (raw_lower.starts_with("write")
            || raw_lower.contains("write")
            || raw_lower.contains("edit")
            || raw_lower.contains("save"))
            && (q_lower.contains("write")
                || q_lower.contains("edit")
                || q_lower.contains("save")
                || q_lower.contains("update"))
        {
            score += 45.0;
        }
        if (raw_lower.starts_with("delete")
            || raw_lower.contains("delete")
            || raw_lower.contains("remove"))
            && (q_lower.contains("delete")
                || q_lower.contains("remove")
                || q_lower.contains("drop"))
        {
            score += 50.0;
        }

        // Check tool keywords match
        for kw in &meta.keywords {
            let kw_lower = kw.to_lowercase();
            if kw_lower.len() > 3 && q_lower.contains(&kw_lower) {
                score += 15.0;
            }
        }

        // 6. Recent tool usage continuity in multi-step workflows
        if recent_tool_names.contains(&meta.name) {
            score += 20.0;
        }

        // 7. Schema efficiency tie-breaker (minor penalty for huge schemas)
        score -= (meta.estimated_schema_tokens as f64) / 300.0;

        score
    }
}
