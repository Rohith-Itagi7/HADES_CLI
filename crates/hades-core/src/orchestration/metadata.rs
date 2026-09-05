use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::context::TokenEstimator;
use hades_tools::{RiskLevel, ToolDefinition};

/// Source origin of a tool definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolSource {
    Native,
    Browser,
    Mcp(String),
}

/// Rich semantic metadata associated with an indexed tool capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMetadata {
    /// Full namespaced tool name (e.g., "github.list_issues", "filesystem.read").
    pub name: String,
    /// Top-level namespace (e.g., "github", "filesystem", "system", "browser", "shell").
    pub namespace: String,
    /// Raw un-namespaced name (e.g., "list_issues", "read").
    pub raw_name: String,
    /// Origin of this tool capability.
    pub source: ToolSource,
    /// Clean human/model-readable description.
    pub description: String,
    /// Functional domains (e.g. ["github", "issues"], ["filesystem", "files"]).
    pub domains: Vec<String>,
    /// Whether this capability is read-only / query / inspection.
    pub is_read_only: bool,
    /// Whether this capability mutates state, creates records, or edits files.
    pub is_mutating: bool,
    /// Security risk level according to Hades policy.
    pub risk_level: RiskLevel,
    /// Estimated token cost of including this tool's full JSON schema in LLM payload.
    pub estimated_schema_tokens: usize,
    /// Semantic keywords extracted for relevance scoring.
    pub keywords: Vec<String>,
}

/// Lightweight capability index for two-stage tool discovery and ranking.
#[derive(Debug, Clone, Default)]
pub struct CapabilityIndex {
    metadata: HashMap<String, ToolMetadata>,
    by_namespace: HashMap<String, Vec<String>>,
    by_domain: HashMap<String, Vec<String>>,
    all_tool_names: Vec<String>,
    total_schema_tokens: usize,
}

impl CapabilityIndex {
    /// Builds a capability index from any slice of `ToolDefinition`s.
    pub fn build(tool_definitions: &[ToolDefinition], active_mcp_servers: &[String]) -> Self {
        let mut metadata_map = HashMap::new();
        let mut by_namespace = HashMap::new();
        let mut by_domain = HashMap::new();
        let mut all_tool_names = Vec::new();
        let mut total_tokens = 0;

        let mcp_set: HashSet<String> = active_mcp_servers.iter().cloned().collect();

        for def in tool_definitions {
            let meta = Self::analyze_tool_definition(def, &mcp_set);
            total_tokens += meta.estimated_schema_tokens;

            by_namespace
                .entry(meta.namespace.clone())
                .or_insert_with(Vec::new)
                .push(meta.name.clone());

            for d in &meta.domains {
                by_domain
                    .entry(d.clone())
                    .or_insert_with(Vec::new)
                    .push(meta.name.clone());
            }

            all_tool_names.push(meta.name.clone());
            metadata_map.insert(meta.name.clone(), meta);
        }

        Self {
            metadata: metadata_map,
            by_namespace,
            by_domain,
            all_tool_names,
            total_schema_tokens: total_tokens,
        }
    }

    /// Derives rich capability metadata from a single `ToolDefinition`.
    fn analyze_tool_definition(
        def: &ToolDefinition,
        mcp_servers: &HashSet<String>,
    ) -> ToolMetadata {
        let (namespace, raw_name) = match def.name.split_once('.') {
            Some((ns, raw)) => (ns.to_string(), raw.to_string()),
            None => ("general".to_string(), def.name.clone()),
        };

        let source = if mcp_servers.contains(&namespace) {
            ToolSource::Mcp(namespace.clone())
        } else if namespace == "browser" {
            ToolSource::Browser
        } else {
            ToolSource::Native
        };

        let is_mutating =
            def.is_mutating || matches!(def.risk_level, RiskLevel::High | RiskLevel::Critical);
        let is_read_only = !is_mutating;

        // Derive functional domains and keywords
        let mut domains = Vec::new();
        domains.push(namespace.clone());

        let raw_lower = raw_name.to_lowercase();
        let desc_lower = def.description.to_lowercase();
        let full_text = format!("{} {} {}", def.name, raw_lower, desc_lower);

        // Common domain tags
        if full_text.contains("issue") {
            domains.push("issues".to_string());
        }
        if full_text.contains("pull_request")
            || full_text.contains("pull request")
            || full_text.contains("pr")
        {
            domains.push("pull_requests".to_string());
        }
        if full_text.contains("repo") || full_text.contains("repository") {
            domains.push("repositories".to_string());
        }
        if full_text.contains("file")
            || full_text.contains("directory")
            || namespace == "filesystem"
        {
            domains.push("files".to_string());
        }
        if full_text.contains("process") {
            domains.push("process".to_string());
        }
        if full_text.contains("port") || full_text.contains("network") {
            domains.push("network".to_string());
        }
        if full_text.contains("shell")
            || full_text.contains("exec")
            || full_text.contains("command")
        {
            domains.push("shell".to_string());
        }
        if full_text.contains("git") || namespace == "workspace" {
            domains.push("workspace".to_string());
        }

        // Extract semantic keywords for relevance scoring
        let mut keywords = Vec::new();
        for word in full_text.split(|c: char| !c.is_alphanumeric() && c != '_') {
            let w = word.trim();
            if w.len() > 2 && !keywords.contains(&w.to_string()) {
                keywords.push(w.to_string());
            }
        }

        // Estimate schema token cost
        let schema_json = serde_json::to_string(&def.parameters_schema).unwrap_or_default();
        let schema_tokens = TokenEstimator::estimate_tokens(&schema_json);
        let name_tokens = TokenEstimator::estimate_tokens(&def.name);
        let desc_tokens = TokenEstimator::estimate_tokens(&def.description);
        // Framing overhead per tool definition payload in JSON-RPC / OpenAI format is ~15 tokens
        let estimated_schema_tokens = name_tokens + desc_tokens + schema_tokens + 15;

        ToolMetadata {
            name: def.name.clone(),
            namespace,
            raw_name,
            source,
            description: def.description.clone(),
            domains,
            is_read_only,
            is_mutating,
            risk_level: def.risk_level,
            estimated_schema_tokens,
            keywords,
        }
    }

    /// Returns metadata for a tool by full namespaced name.
    pub fn get(&self, name: &str) -> Option<&ToolMetadata> {
        self.metadata.get(name)
    }

    /// Returns all registered tool names.
    pub fn all_tool_names(&self) -> &[String] {
        &self.all_tool_names
    }

    /// Total count of indexed tools.
    pub fn count(&self) -> usize {
        self.metadata.len()
    }

    /// Returns tool names belonging to a specific namespace.
    pub fn tools_in_namespace(&self, namespace: &str) -> &[String] {
        self.by_namespace
            .get(namespace)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Returns tool names tagged with a specific functional domain.
    pub fn tools_in_domain(&self, domain: &str) -> &[String] {
        self.by_domain
            .get(domain)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Total estimated tokens if all tools' schemas were injected simultaneously.
    pub fn total_schema_tokens(&self) -> usize {
        self.total_schema_tokens
    }
}
