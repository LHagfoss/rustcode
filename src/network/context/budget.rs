use super::tokens::{estimate_message_tokens, estimate_tokens, estimate_tool_schema_tokens};
use crate::app::ChatMessage;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreflightBudget {
    pub system_tokens: usize,
    pub tool_schema_tokens: usize,
    pub history_tokens: usize,
    pub dynamic_tail_tokens: usize,
    pub continuation_overhead_tokens: usize,
    pub provider_margin: usize,
    pub total_estimated_prompt: usize,
    pub completion_reserve: usize,
    pub soft_context_target: usize,
    pub hard_effective_limit: usize,
    pub context_window: usize,
}

impl PreflightBudget {
    pub fn fits_hard_limit(&self) -> bool {
        self.total_estimated_prompt
            .saturating_add(self.completion_reserve)
            <= self.hard_effective_limit
    }

    pub fn fits_soft_target(&self) -> bool {
        self.total_estimated_prompt <= self.soft_context_target
    }
}

/// Calculate the comprehensive preflight budget before sending a request to the provider.
pub fn calculate_preflight_budget(
    system_prompt: &str,
    tool_schemas: &[serde_json::Value],
    history: &[ChatMessage],
    dynamic_context_tail: &str,
    continuation_overhead: usize,
    budget: &crate::config::ContextBudget,
) -> PreflightBudget {
    let system_tokens = estimate_tokens(system_prompt);
    let tool_schema_tokens = estimate_tool_schema_tokens(tool_schemas);
    let history_tokens: usize = history.iter().map(estimate_message_tokens).sum();
    let dynamic_tail_tokens = estimate_tokens(dynamic_context_tail);
    let provider_margin = budget.provider_overhead_margin as usize;
    let total_estimated_prompt = system_tokens
        .saturating_add(tool_schema_tokens)
        .saturating_add(history_tokens)
        .saturating_add(dynamic_tail_tokens)
        .saturating_add(continuation_overhead)
        .saturating_add(provider_margin);

    PreflightBudget {
        system_tokens,
        tool_schema_tokens,
        history_tokens,
        dynamic_tail_tokens,
        continuation_overhead_tokens: continuation_overhead,
        provider_margin,
        total_estimated_prompt,
        completion_reserve: budget.completion_reserve as usize,
        soft_context_target: budget.soft_context_target as usize,
        hard_effective_limit: budget.hard_effective_limit as usize,
        context_window: budget.context_window as usize,
    }
}
