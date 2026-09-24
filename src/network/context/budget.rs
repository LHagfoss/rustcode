#[cfg(test)]
use super::tokens::estimate_message_tokens;
use super::tokens::{estimate_tokens, estimate_tool_schema_tokens};
#[cfg(test)]
use crate::app::ChatMessage;
use crate::network::messages::estimate_msg_tokens;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreflightBudget {
    pub system_tokens: usize,
    pub tool_schema_tokens: usize,
    pub history_tokens: usize,
    pub dynamic_tail_tokens: usize,
    pub continuation_overhead_tokens: usize,
    pub provider_margin: usize,
    pub total_estimated_prompt: usize,
    /// Prompt tokens plus the provider framing margin. This is telemetry only;
    /// the hard decision uses `hard_effective_limit`, which already excludes
    /// the same margin.
    pub prompt_with_provider_overhead: usize,
    /// Tokens in the final rendered message projection, before native schemas.
    pub projected_message_tokens: usize,
    /// Metadata embedded in projected tool results. It is part of
    /// `projected_message_tokens`, and is reported separately without being
    /// added a second time.
    pub metadata_tokens: usize,
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
#[cfg(test)]
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
        .saturating_add(continuation_overhead);

    PreflightBudget {
        system_tokens,
        tool_schema_tokens,
        history_tokens,
        dynamic_tail_tokens,
        continuation_overhead_tokens: continuation_overhead,
        provider_margin,
        total_estimated_prompt,
        prompt_with_provider_overhead: total_estimated_prompt.saturating_add(provider_margin),
        projected_message_tokens: system_tokens
            .saturating_add(history_tokens)
            .saturating_add(dynamic_tail_tokens),
        metadata_tokens: 0,
        completion_reserve: budget.completion_reserve as usize,
        soft_context_target: budget.soft_context_target as usize,
        hard_effective_limit: budget.hard_effective_limit as usize,
        context_window: budget.context_window as usize,
    }
}

/// Calculate budget usage from the exact provider message projection that will
/// be sent. `history::to_messages_for_request_with_scope` has already applied
/// the selected history scope and attached developer instructions; callers
/// should invoke this after request-local reminders/nudges are injected as well.
///
/// Native schemas are separate from the message array and are counted here.
/// Text-protocol tool definitions are already inside the system message and
/// therefore pass an empty schema slice, avoiding double counting.
pub fn calculate_preflight_budget_for_projection(
    projected_messages: &[serde_json::Value],
    tool_schemas: &[serde_json::Value],
    continuation_overhead: usize,
    budget: &crate::config::ContextBudget,
) -> PreflightBudget {
    let projected_message_tokens = projected_messages
        .iter()
        .map(estimate_msg_tokens)
        .sum::<u32>() as usize;
    let system_tokens = projected_messages
        .iter()
        .filter(|message| {
            matches!(
                message.get("role").and_then(serde_json::Value::as_str),
                Some("system" | "developer")
            )
        })
        .map(estimate_msg_tokens)
        .sum::<u32>() as usize;
    let dynamic_tail_tokens = projected_messages
        .iter()
        .filter(|message| {
            message
                .get("content")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|content| content.starts_with("<rustcode_context>"))
        })
        .map(estimate_msg_tokens)
        .sum::<u32>() as usize;
    let metadata_tokens = projected_messages
        .iter()
        .filter_map(|message| message.get("content").and_then(serde_json::Value::as_str))
        .filter_map(|content| {
            content
                .find("[result_metadata:")
                .map(|start| &content[start..])
        })
        .map(estimate_tokens)
        .sum::<usize>();
    let history_tokens = projected_message_tokens
        .saturating_sub(system_tokens)
        .saturating_sub(dynamic_tail_tokens);
    let tool_schema_tokens = estimate_tool_schema_tokens(tool_schemas);
    let provider_margin = budget.provider_overhead_margin as usize;
    let total_estimated_prompt = projected_message_tokens
        .saturating_add(tool_schema_tokens)
        .saturating_add(continuation_overhead);

    PreflightBudget {
        system_tokens,
        tool_schema_tokens,
        history_tokens,
        dynamic_tail_tokens,
        continuation_overhead_tokens: continuation_overhead,
        provider_margin,
        total_estimated_prompt,
        prompt_with_provider_overhead: total_estimated_prompt.saturating_add(provider_margin),
        projected_message_tokens,
        metadata_tokens,
        completion_reserve: budget.completion_reserve as usize,
        soft_context_target: budget.soft_context_target as usize,
        hard_effective_limit: budget.hard_effective_limit as usize,
        context_window: budget.context_window as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_projection_reports_an_actionable_over_budget_checkpoint() {
        let profile = crate::config::ModelProfile {
            name: "budget-test".to_owned(),
            model: "budget-test".to_owned(),
            context_window: Some(256),
            max_tokens: Some(64),
            ..Default::default()
        };
        let budget = profile.context_budget();
        let projection = vec![serde_json::json!({
            "role": "user",
            "content": "oversized ".repeat(2_000),
        })];
        let preflight = calculate_preflight_budget_for_projection(&projection, &[], 0, &budget);

        assert!(!preflight.fits_hard_limit());
        let notice = crate::network::context_preflight_checkpoint_notice(&preflight);
        assert!(notice.contains("Context checkpoint"));
        assert!(notice.contains("/compact"));
        assert!(notice.contains("was not sent"));
    }
}
