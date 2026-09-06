use serde::{Deserialize, Serialize};

/// High-level capability domain inferred from a user request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskDomain {
    /// Pure conversational, general knowledge, conceptual, or math query requiring no tools.
    GenericReasoning,
    /// Workspace filesystem manipulation (reading, writing, editing, creating, listing files).
    Filesystem,
    /// Workspace ecosystem and git metadata inspection.
    Workspace,
    /// Command-line shell execution.
    Shell,
    /// Operating system, process, network, and runtime diagnostics.
    System,
    /// Web intelligence and browser automation.
    Browser,
    /// External Model Context Protocol server domain (e.g. "github", "linear", "slack").
    Mcp(String),
    /// Multi-domain task requiring capabilities from multiple sources.
    MultiDomain(Vec<TaskDomain>),
}

impl TaskDomain {
    pub fn name(&self) -> String {
        match self {
            Self::GenericReasoning => "generic_reasoning".to_string(),
            Self::Filesystem => "filesystem".to_string(),
            Self::Workspace => "workspace".to_string(),
            Self::Shell => "shell".to_string(),
            Self::System => "system".to_string(),
            Self::Browser => "browser".to_string(),
            Self::Mcp(s) => format!("mcp:{s}"),
            Self::MultiDomain(domains) => {
                let names: Vec<_> = domains.iter().map(|d| d.name()).collect();
                names.join("+")
            }
        }
    }
}

/// Extracted semantic entities from a user request.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExtractedEntities {
    /// Referenced relative or absolute file paths (e.g., "Cargo.toml", "src/main.rs").
    pub file_paths: Vec<String>,
    /// Referenced repository coordinates (e.g., "PareekshithPalat/HADES_CLI").
    pub repo_names: Vec<String>,
    /// Referenced URLs (e.g., `https://api.github.com`).
    pub urls: Vec<String>,
    /// Explicit tool names (e.g., "github.list_issues", "filesystem.read").
    pub explicit_tools: Vec<String>,
    /// Explicit MCP server names (e.g., "github", "linear").
    pub explicit_servers: Vec<String>,
}

/// Structured task intent analysis result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskIntent {
    /// Original user prompt.
    pub user_query: String,
    /// Primary detected task domain.
    pub primary_domain: TaskDomain,
    /// Secondary domains if multi-domain task.
    pub secondary_domains: Vec<TaskDomain>,
    /// Extracted named entities.
    pub entities: ExtractedEntities,
    /// Whether user intent appears strictly read-only / inspection.
    pub is_read_only: bool,
    /// Whether this task requires any tool execution (false for generic Q&A/math).
    pub requires_tools: bool,
    /// Concise human-readable explanation of the detected intent.
    pub summary: String,
}

/// Lightweight, deterministic rule-based task intent analyzer.
pub struct TaskIntentAnalyzer;

impl TaskIntentAnalyzer {
    /// Analyzes a prompt and optional conversation history to classify task intent without extra LLM overhead.
    pub fn analyze(
        prompt: &str,
        available_servers: &[String],
        registered_tool_names: &[String],
    ) -> TaskIntent {
        let trimmed = prompt.trim();
        let lower = trimmed.to_lowercase();

        // 1. Extract entities
        let entities = Self::extract_entities(trimmed, available_servers, registered_tool_names);

        // 2. Check for pure generic reasoning / conversational queries
        if Self::is_pure_reasoning(&lower, &entities) {
            return TaskIntent {
                user_query: trimmed.to_string(),
                primary_domain: TaskDomain::GenericReasoning,
                secondary_domains: Vec::new(),
                entities,
                is_read_only: true,
                requires_tools: false,
                summary: "Generic conversational or reasoning query (no tools needed)".to_string(),
            };
        }

        // 3. Detect domains
        let mut detected_domains: Vec<TaskDomain> = Vec::new();

        // Explicit MCP server mentions
        for s in &entities.explicit_servers {
            let domain = TaskDomain::Mcp(s.clone());
            if !detected_domains.contains(&domain) {
                detected_domains.push(domain);
            }
        }

        // Check for MCP server domain triggers
        for server in available_servers {
            let s_lower = server.to_lowercase();
            if s_lower == "github" {
                if lower.contains("issue")
                    || lower.contains("pull request")
                    || lower.contains(" pr")
                    || lower.contains("prs")
                    || lower.contains("repo")
                    || lower.contains("github")
                    || !entities.repo_names.is_empty()
                {
                    let d = TaskDomain::Mcp("github".to_string());
                    if !detected_domains.contains(&d) {
                        detected_domains.push(d);
                    }
                }
            } else if lower.contains(&s_lower) {
                let d = TaskDomain::Mcp(server.clone());
                if !detected_domains.contains(&d) {
                    detected_domains.push(d);
                }
            }
        }

        // Check for Filesystem triggers
        let has_fs_trigger = lower.contains("file")
            || lower.contains("folder")
            || lower.contains("directory")
            || lower.contains("read ")
            || lower.contains("edit ")
            || lower.contains("write ")
            || lower.contains("create ")
            || lower.contains("mkdir")
            || lower.contains("touch ")
            || lower.contains("delete ")
            || !entities.file_paths.is_empty();

        let has_fs_context = !entities.file_paths.is_empty()
            || lower.contains("local")
            || lower.contains("workspace")
            || lower.contains("create a file")
            || lower.contains("write to")
            || lower.contains("edit")
            || lower.contains("save")
            || (!lower.contains("issue")
                && !lower.contains("github")
                && !lower.contains("pull request"));

        if has_fs_trigger && has_fs_context && !detected_domains.contains(&TaskDomain::Filesystem) {
            detected_domains.push(TaskDomain::Filesystem);
        }

        // Check for System & Process triggers
        let has_sys_trigger = lower.contains("process")
            || lower.contains("port ")
            || lower.contains("port:")
            || lower.contains("system info")
            || lower.contains("uptime")
            || lower.contains("hostname")
            || lower.contains("cpu")
            || lower.contains("memory")
            || lower.contains("platform")
            || lower.contains("environment variable")
            || lower.contains("env var")
            || lower.contains("which ")
            || lower.contains("version of ")
            || lower.contains("network");

        if has_sys_trigger && !detected_domains.contains(&TaskDomain::System) {
            detected_domains.push(TaskDomain::System);
        }

        // Check for Workspace / Git triggers
        let has_ws_trigger = lower.contains("git ")
            || lower.contains("git status")
            || lower.contains("git branch")
            || lower.contains("workspace")
            || lower.contains("project type");

        if has_ws_trigger && !detected_domains.contains(&TaskDomain::Workspace) {
            detected_domains.push(TaskDomain::Workspace);
        }

        // Check for Shell triggers
        let has_shell_trigger = lower.contains("run ")
            || lower.contains("execute")
            || lower.contains("cargo ")
            || lower.contains("npm ")
            || lower.contains("bash ")
            || lower.contains("shell ")
            || lower.contains("terminal");

        if has_shell_trigger && !detected_domains.contains(&TaskDomain::Shell) {
            detected_domains.push(TaskDomain::Shell);
        }

        // Check for Browser triggers
        let has_browser_trigger = lower.contains("browse")
            || lower.contains("website")
            || lower.contains("web page")
            || lower.contains("navigate to")
            || lower.contains("click on")
            || (!entities.urls.is_empty() && !lower.contains("mcp"));

        if has_browser_trigger && !detected_domains.contains(&TaskDomain::Browser) {
            detected_domains.push(TaskDomain::Browser);
        }

        // Determine primary and secondary domains
        let is_read_only = Self::is_read_only_intent(&lower);

        if detected_domains.is_empty() {
            return TaskIntent {
                user_query: trimmed.to_string(),
                primary_domain: TaskDomain::GenericReasoning,
                secondary_domains: Vec::new(),
                entities,
                is_read_only,
                requires_tools: false,
                summary: "General inquiry with no active tool domain matched".to_string(),
            };
        }

        let primary_domain = detected_domains[0].clone();
        let secondary_domains = detected_domains[1..].to_vec();

        let summary = format!(
            "Task domain: {}{}{}",
            primary_domain.name(),
            if !secondary_domains.is_empty() {
                format!(
                    " (+{})",
                    secondary_domains
                        .iter()
                        .map(|d| d.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                String::new()
            },
            if is_read_only {
                " [read-only]"
            } else {
                " [mutating/active]"
            }
        );

        TaskIntent {
            user_query: trimmed.to_string(),
            primary_domain,
            secondary_domains,
            entities,
            is_read_only,
            requires_tools: true,
            summary,
        }
    }

    /// Determines if a prompt is clearly conversational, mathematical, or general reasoning.
    fn is_pure_reasoning(lower: &str, entities: &ExtractedEntities) -> bool {
        if !entities.explicit_tools.is_empty() || !entities.explicit_servers.is_empty() {
            return false;
        }

        // Common conversational greetings
        let greetings = [
            "hi",
            "hello",
            "hey",
            "good morning",
            "good evening",
            "how are you",
            "who are you",
            "what can you do",
        ];
        if greetings
            .iter()
            .any(|&g| lower == g || lower == format!("{g}!"))
        {
            return true;
        }

        // Arithmetic / Math expressions
        let math_patterns = [
            "2 + 2",
            "2+2",
            "what is 2 + 2",
            "what's 2 + 2",
            "calculate",
            "solve",
            "math",
        ];
        if math_patterns.iter().any(|&p| lower.contains(p))
            && !lower.contains("file")
            && !lower.contains("issue")
        {
            return true;
        }

        // Explanations / Conceptual questions without workspace references
        if (lower.starts_with("explain ")
            || lower.starts_with("what is ")
            || lower.starts_with("how does ")
            || lower.starts_with("why is ")
            || lower.starts_with("write a "))
            && !lower.contains("file")
            && !lower.contains("issue")
            && !lower.contains("repo")
            && !lower.contains("process")
            && !lower.contains("port")
            && !lower.contains("git")
            && !lower.contains("workspace")
            && !lower.contains("run")
            && entities.file_paths.is_empty()
            && entities.repo_names.is_empty()
            && entities.urls.is_empty()
        {
            return true;
        }

        false
    }

    /// Determines if user intent is read-only (query/list/get) versus mutating (create/delete/modify).
    fn is_read_only_intent(lower: &str) -> bool {
        let mutating_indicators = [
            "create",
            "write",
            "edit",
            "modify",
            "delete",
            "remove",
            "add",
            "post",
            "insert",
            "update",
            "patch",
            "drop",
            "terminate",
            "kill",
            "purge",
            "touch",
        ];

        if mutating_indicators.iter().any(|&w| lower.contains(w)) {
            return false;
        }

        true
    }

    /// Extracts file paths, repo names, URLs, explicit tool and server names.
    fn extract_entities(
        prompt: &str,
        available_servers: &[String],
        registered_tool_names: &[String],
    ) -> ExtractedEntities {
        let mut file_paths = Vec::new();
        let mut repo_names = Vec::new();
        let mut urls = Vec::new();
        let mut explicit_tools = Vec::new();
        let mut explicit_servers = Vec::new();

        let lower_prompt = prompt.to_lowercase();

        // Check for explicit server names
        for server in available_servers {
            let s_lower = server.to_lowercase();
            if lower_prompt.contains(&s_lower) {
                explicit_servers.push(server.clone());
            }
        }

        // Check for explicit tool names
        for tool in registered_tool_names {
            let t_lower = tool.to_lowercase();
            if lower_prompt.contains(&t_lower) {
                explicit_tools.push(tool.clone());
            }
        }

        // Token scan
        for token in prompt.split_whitespace() {
            let clean = token.trim_matches(|c: char| {
                !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-'
            });

            // URLs
            if clean.starts_with("http://") || clean.starts_with("https://") {
                urls.push(clean.to_string());
                continue;
            }

            // GitHub repository coordinates: "owner/repo"
            if clean.contains('/') && !clean.starts_with('/') && !clean.ends_with('/') {
                let parts: Vec<&str> = clean.split('/').collect();
                if parts.len() == 2
                    && !parts[0].is_empty()
                    && !parts[1].is_empty()
                    && !parts[0].contains('.')
                    && (parts[1].contains('-')
                        || parts[1].contains('_')
                        || parts[0].chars().next().is_some_and(|c| c.is_uppercase()))
                {
                    repo_names.push(clean.to_string());
                    continue;
                }
            }

            // File paths: e.g. foo.rs, test.txt, package.json
            let file_extensions = [
                ".rs", ".txt", ".toml", ".json", ".md", ".js", ".ts", ".jsx", ".tsx", ".py", ".c",
                ".cpp", ".h", ".go", ".java", ".sh", ".yaml", ".yml", ".html", ".css", ".lock",
            ];
            if file_extensions.iter().any(|ext| clean.ends_with(ext)) {
                file_paths.push(clean.to_string());
            }
        }

        ExtractedEntities {
            file_paths,
            repo_names,
            urls,
            explicit_tools,
            explicit_servers,
        }
    }
}
