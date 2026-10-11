use crate::ui::render_snapshot::RenderSnapshot;
use rustcode::controller::TokenUsage;

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

/// Tokens this session has spent, summed from the usage recorded on its
/// transcript.
///
/// Every provider response is stored with the usage of that one request, so
/// the sum over assistant messages is the session total. `prompt_tokens`
/// re-counts the conversation on each request, which is what a provider
/// bills, and is therefore much larger than the context window in use.
/// `cached_tokens` is the part of `prompt_tokens` the provider served from its
/// cache: it is reported beside the total, never subtracted from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionTokenTotal {
    pub(crate) prompt_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) total_tokens: u64,
    pub(crate) cached_tokens: u64,
    /// Provider responses that contributed usage, the running one included.
    pub(crate) responses: usize,
    /// The total includes locally estimated tokens of the request that is
    /// still running, so it is not purely provider-counted yet.
    pub(crate) estimated: bool,
}

impl SessionTokenTotal {
    fn add(&mut self, usage: &TokenUsage) {
        self.prompt_tokens += u64::from(usage.prompt_tokens);
        self.completion_tokens += u64::from(usage.completion_tokens);
        self.total_tokens += u64::from(usage.total_tokens);
        self.cached_tokens += u64::from(usage.cached_tokens.unwrap_or(0));
        self.responses += 1;
    }
}

/// Sum the session's recorded usage. Subagents keep their own transcripts and
/// are not part of this figure. A request that is still running has no
/// transcript message yet, so its live usage is added on top.
pub(crate) fn session_token_total(state: &RenderSnapshot) -> SessionTokenTotal {
    let mut total = SessionTokenTotal::default();
    for usage in state
        .history()
        .iter()
        .filter(|message| message.role == "assistant")
        .filter_map(|message| message.token_usage.as_ref())
    {
        total.add(usage);
    }
    if state.token_usage_in_flight()
        && let Some(usage) = state.current_round_token_usage()
    {
        total.add(usage);
        total.estimated = state.current_turn_token_usage_is_estimated();
    }
    total
}

#[cfg(test)]
mod tests {
    use super::{ContextUsage, ContextUsageSource, context_usage, session_token_total};
    use crate::ui::render_snapshot::render_snapshot;
    use rustcode::controller::{ChatMessage, RenderState, TokenUsage};

    #[test]
    fn cleared_current_usage_does_not_reuse_a_previous_provider_prompt() {
        let mut state = RenderState::new();
        assert_eq!(
            context_usage(&render_snapshot(&state)),
            ContextUsage {
                used_tokens: 0,
                source: ContextUsageSource::HistoryEstimate,
            }
        );

        state.history.push(ChatMessage::new("user", "abcd"));
        assert_eq!(
            context_usage(&render_snapshot(&state)),
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
            context_usage(&render_snapshot(&state)),
            ContextUsage {
                used_tokens: 3,
                source: ContextUsageSource::HistoryEstimate,
            }
        );
    }

    #[test]
    fn selected_subagent_usage_uses_history_estimate_without_current_usage() {
        let mut state = RenderState::new();
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
        let child = rustcode::controller::SubAgentView {
            id: 7,
            name: "reviewer".to_owned(),
            task: "review".to_owned(),
            history: std::sync::Arc::new(vec![child_message]),
            status: rustcode::controller::SubAgentStatus::Completed,
            active_turn: false,
            parent_id: None,
            model: None,
            elapsed_ms: 0,
        };
        state.subagents.push(child.clone());
        state.selected_subagent = Some(child);
        state.selected_subagent_id = Some(7);

        assert_eq!(
            context_usage(&render_snapshot(&state)),
            ContextUsage {
                used_tokens: 2,
                source: ContextUsageSource::HistoryEstimate,
            }
        );
    }

    fn usage(prompt: u32, completion: u32, cached: Option<u32>) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_tokens: cached,
            ..Default::default()
        }
    }

    #[test]
    fn session_total_sums_every_recorded_response_and_reports_cached_separately() {
        let mut state = RenderState::new();
        assert_eq!(session_token_total(&render_snapshot(&state)).responses, 0);

        state.history.push(ChatMessage::new("user", "question"));
        let mut first = ChatMessage::new("assistant", "calling a tool");
        first.token_usage = Some(usage(1_000, 50, Some(400)));
        state.history.push(first);
        // A tool result carries no provider usage of its own.
        state.history.push(ChatMessage::new("tool", "output"));
        let mut second = ChatMessage::new("assistant", "answer");
        second.token_usage = Some(usage(1_200, 80, None));
        state.history.push(second);
        state
            .history
            .push(ChatMessage::new("assistant", "no usage"));

        let total = session_token_total(&render_snapshot(&state));
        assert_eq!(total.prompt_tokens, 2_200);
        assert_eq!(total.completion_tokens, 130);
        assert_eq!(total.total_tokens, 2_330);
        assert_eq!(total.cached_tokens, 400);
        assert_eq!(total.responses, 2);
        assert!(!total.estimated);
    }

    #[test]
    fn session_total_adds_the_running_request_only_while_it_is_in_flight() {
        let mut state = RenderState::new();
        let mut done = ChatMessage::new("assistant", "answer");
        done.token_usage = Some(usage(100, 10, None));
        state.history.push(done);
        state.current_round_token_usage = Some(usage(300, 7, None));

        // Not in flight: the round is already (or about to be) on a message.
        assert_eq!(
            session_token_total(&render_snapshot(&state)).total_tokens,
            110
        );

        state.token_usage_in_flight = true;
        state.current_turn_token_usage_is_estimated = true;
        let total = session_token_total(&render_snapshot(&state));
        assert_eq!(total.total_tokens, 417);
        assert_eq!(total.responses, 2);
        assert!(total.estimated);
    }
}
