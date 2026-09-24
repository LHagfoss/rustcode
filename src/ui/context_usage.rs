use crate::app::TokenUsage;
use crate::ui::render_snapshot::RenderSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextUsageSource {
    ProviderPrompt,
    HistoryEstimate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextUsage {
    pub(crate) used_tokens: u32,
    pub(crate) source: ContextUsageSource,
}

/// The footer and `/context` headline use usage from the current request when
/// available. Completion tokens do not occupy the context window. When there
/// is no current-request usage, estimate from active history instead of
/// reusing a previous request's saved usage.
pub(crate) fn context_usage(state: &RenderSnapshot) -> ContextUsage {
    if state.selected_subagent_id().is_none()
        && let Some(usage) = state.current_token_usage()
    {
        return provider_prompt_usage(usage);
    }

    let chars: usize = state
        .active_history()
        .iter()
        .map(|message| message.content.len())
        .sum();
    ContextUsage {
        used_tokens: (chars / 4) as u32,
        source: ContextUsageSource::HistoryEstimate,
    }
}

fn provider_prompt_usage(usage: &TokenUsage) -> ContextUsage {
    ContextUsage {
        used_tokens: usage.prompt_tokens,
        source: ContextUsageSource::ProviderPrompt,
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextUsage, ContextUsageSource, context_usage};
    use crate::app::{AppState, ChatMessage, TokenUsage};

    #[test]
    fn cleared_current_usage_does_not_reuse_a_previous_provider_prompt() {
        let mut state = AppState::new();
        assert_eq!(
            context_usage(&state.render_snapshot()),
            ContextUsage {
                used_tokens: 0,
                source: ContextUsageSource::HistoryEstimate,
            }
        );

        state.history.push(ChatMessage::new("user", "abcd"));
        assert_eq!(
            context_usage(&state.render_snapshot()),
            ContextUsage {
                used_tokens: 1,
                source: ContextUsageSource::HistoryEstimate,
            }
        );

        let mut message = ChatMessage::new("assistant", "reply");
        message.token_usage = Some(TokenUsage {
            prompt_tokens: 900,
            completion_tokens: 99,
            total_tokens: 999,
            ..Default::default()
        });
        state.history.push(message);
        state.history.push(ChatMessage::new("user", "new"));

        assert_eq!(
            context_usage(&state.render_snapshot()),
            ContextUsage {
                used_tokens: 3,
                source: ContextUsageSource::HistoryEstimate,
            }
        );
    }

    #[test]
    fn selected_subagent_usage_uses_history_estimate_without_current_usage() {
        let mut state = AppState::new();
        state.current_token_usage = Some(TokenUsage {
            prompt_tokens: 12,
            completion_tokens: 99,
            total_tokens: 111,
            ..Default::default()
        });
        let mut child_message = ChatMessage::new("assistant", "child reply");
        child_message.token_usage = Some(TokenUsage {
            prompt_tokens: 5,
            completion_tokens: 2,
            total_tokens: 7,
            ..Default::default()
        });
        state.subagents.push(crate::app::SubAgent {
            id: 7,
            name: "reviewer".to_owned(),
            task: "review".to_owned(),
            model: None,
            history: std::sync::Arc::new(vec![child_message]),
            status: crate::app::SubAgentStatus::Completed,
            active_turn: false,
            parent_id: None,
            write_access: false,
            allowed_paths: Vec::new(),
            verification_command: None,
            workspace_root: None,
            review_manifest: None,
        });
        state.selected_subagent_id = Some(7);

        assert_eq!(
            context_usage(&state.render_snapshot()),
            ContextUsage {
                used_tokens: 2,
                source: ContextUsageSource::HistoryEstimate,
            }
        );
    }
}
