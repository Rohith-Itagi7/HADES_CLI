use serde::{Deserialize, Serialize};

/// Provider and model token limits distinguishing Context Window from TPM (Tokens Per Minute).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderTokenProfile {
    pub provider_id: String,
    pub model_id: String,
    /// Absolute context window capacity of the model architecture.
    pub context_window: usize,
    /// Rate limit constraint (Tokens Per Minute), if applicable (e.g. Groq 8,000 TPM limit).
    pub tpm_limit: Option<usize>,
    /// Request rate limit (Requests Per Minute), if applicable.
    pub rpm_limit: Option<usize>,
    /// Upper bound on input tokens for a single request to guarantee staying within TPM and context.
    pub max_request_input_tokens: usize,
    /// Tokens reserved for the model's generated response.
    pub output_reserve: usize,
}

impl ProviderTokenProfile {
    /// Resolves conservative token limits based on provider and model metadata.
    pub fn for_model(
        provider_id: &str,
        model_id: &str,
        configured_context_window: Option<u32>,
    ) -> Self {
        let p_lower = provider_id.to_lowercase();
        let m_lower = model_id.to_lowercase();

        let raw_context = configured_context_window
            .map(|c| c as usize)
            .unwrap_or_else(|| {
                if m_lower.contains("70b") || m_lower.contains("gpt-4o") {
                    131_072
                } else if m_lower.contains("32k") || m_lower.contains("mixtral") {
                    32_768
                } else {
                    8_192
                }
            });

        // Groq has tight Tokens Per Minute limits on free/standard tiers (typically 6,000 to 8,000 TPM)
        if p_lower.contains("groq") || m_lower.contains("groq") {
            let tpm: usize = 8_000;
            let reserve: usize = 2_000;
            // Never send more than 6,000 input tokens in a single request to Groq (leaves 2,000 for output + headroom under 8,000 TPM limit)
            let max_input = 6_000.min(raw_context.saturating_sub(reserve));
            return Self {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                context_window: raw_context,
                tpm_limit: Some(tpm),
                rpm_limit: Some(30),
                max_request_input_tokens: max_input,
                output_reserve: reserve,
            };
        }

        // OpenAI standard / enterprise
        if p_lower.contains("openai") {
            let reserve = 4_096;
            let max_input = raw_context.saturating_sub(reserve).min(64_000);
            return Self {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                context_window: raw_context,
                tpm_limit: None,
                rpm_limit: None,
                max_request_input_tokens: max_input,
                output_reserve: reserve,
            };
        }

        // Ollama / Local inference
        if p_lower.contains("ollama") || p_lower.contains("local") {
            let reserve = 1_024;
            let max_input = raw_context.saturating_sub(reserve);
            return Self {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                context_window: raw_context,
                tpm_limit: None,
                rpm_limit: None,
                max_request_input_tokens: max_input,
                output_reserve: reserve,
            };
        }

        // Generic fallback
        let reserve = 2_048;
        let max_input = raw_context.saturating_sub(reserve).min(16_384);
        Self {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            context_window: raw_context,
            tpm_limit: None,
            rpm_limit: None,
            max_request_input_tokens: max_input,
            output_reserve: reserve,
        }
    }
}

/// Token budget accounting and enforcement manager.
pub struct TokenBudgetManager;

impl TokenBudgetManager {
    /// Determines whether the estimated request payload fits comfortably within provider limits.
    pub fn fits_budget(
        profile: &ProviderTokenProfile,
        system_tokens: usize,
        context_tokens: usize,
        tool_tokens: usize,
    ) -> bool {
        let total = system_tokens + context_tokens + tool_tokens;
        total <= profile.max_request_input_tokens
    }

    /// Calculates available budget remaining for conversation history after accounting for system prompt and tools.
    pub fn available_history_budget(
        profile: &ProviderTokenProfile,
        system_tokens: usize,
        current_prompt_tokens: usize,
        tool_tokens: usize,
    ) -> usize {
        let fixed_cost = system_tokens + current_prompt_tokens + tool_tokens;
        profile.max_request_input_tokens.saturating_sub(fixed_cost)
    }
}
