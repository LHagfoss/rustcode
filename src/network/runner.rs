use std::future::Future;

use crate::app::TokenUsage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResponseError {
    pub(crate) message: String,
    /// Response text emitted by the failed request. This is kept separate from
    /// the diagnostic so a stream failure can preserve a safe checkpoint
    /// without replaying it as a successful tool response.
    pub(crate) partial_content: String,
    /// Bounded native call state from a failed stream. These are diagnostics
    /// only and must not be converted into executable tool calls.
    pub(crate) partial_native_tool_calls: Vec<super::stream::NativeToolCallCheckpoint>,
}

impl ResponseError {
    pub(crate) fn with_partial(message: impl Into<String>, partial_content: String) -> Self {
        Self {
            message: message.into(),
            partial_content,
            partial_native_tool_calls: Vec::new(),
        }
    }

    pub(crate) fn with_partial_native(
        message: impl Into<String>,
        partial_content: String,
        partial_native_tool_calls: Vec<super::stream::NativeToolCallCheckpoint>,
    ) -> Self {
        Self {
            message: message.into(),
            partial_content,
            partial_native_tool_calls,
        }
    }
}

impl From<String> for ResponseError {
    fn from(message: String) -> Self {
        Self::with_partial(message, String::new())
    }
}

impl From<&str> for ResponseError {
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}

impl std::fmt::Display for ResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Shared continuation and loop policy for every model-facing turn loop.
pub(crate) struct TurnRunner {
    continuation_count: usize,
    adaptive_continuation_count: usize,
    replay_tokens: u32,
    max_continuations: usize,
}

/// Every continuation resends the accumulated response prefix. Keep that
/// replay work bounded independently of the number of continuations and the
/// logical output ceiling.
const MAX_CONTINUATION_REPLAY_TOKENS: u32 = 65_536;

#[derive(Debug, Clone)]
pub(crate) struct ContinuationPolicy {
    /// Higher ceiling available only for a verified profile after an
    /// actionable textual tool call is proven truncated.
    pub(crate) adaptive_tool_output_limit: Option<u32>,
    /// Prompt-plus-output budget for the logical response. This is already
    /// reduced by the caller's base prompt estimate and conservative schema
    /// reserve.
    pub(crate) context_output_limit: Option<u32>,
    /// Prevent repeated continuation requests from creating an unbounded
    /// logical response even when the provider keeps stopping at length.
    pub(crate) max_total_output_tokens: u32,
    /// Provider/profile-specific continuation ceiling. The default remains
    /// intentionally small because every continuation replays the prefix.
    pub(crate) max_continuations: usize,
}

impl Default for ContinuationPolicy {
    fn default() -> Self {
        Self {
            adaptive_tool_output_limit: None,
            context_output_limit: None,
            max_total_output_tokens: 32_768,
            max_continuations: crate::config::DEFAULT_MAX_TOOL_CONTINUATIONS,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ContinuationRequest {
    pub(crate) previous: String,
    pub(crate) output_token_limit: Option<u32>,
}

pub(crate) struct ResponseChunk {
    pub(crate) content: String,
    pub(crate) final_answer_boundary: super::stream::FinalAnswerBoundary,
    pub(crate) provider_final_answer_state: super::stream::ProviderFinalAnswerState,
    pub(crate) finish_reason: Option<String>,
    pub(crate) has_native_tool_calls: bool,
    pub(crate) output_token_limit: Option<u32>,
    pub(crate) thought_time_ms: u64,
    pub(crate) thought_tokens: u32,
    pub(crate) token_usage: Option<TokenUsage>,
}

#[derive(Debug)]
pub(crate) struct CollectedResponse {
    pub(crate) content: String,
    pub(crate) final_answer_boundary: super::stream::FinalAnswerBoundary,
    pub(crate) provider_final_answer_state: super::stream::ProviderFinalAnswerState,
    pub(crate) finish_reason: Option<String>,
    pub(crate) thought_time_ms: u64,
    pub(crate) thought_tokens: u32,
    pub(crate) token_usage: Option<TokenUsage>,
}

fn add_usage(total: &mut Option<TokenUsage>, usage: Option<TokenUsage>) {
    let Some(usage) = usage else {
        return;
    };
    let target = total.get_or_insert_with(TokenUsage::default);
    target.prompt_tokens = target.prompt_tokens.saturating_add(usage.prompt_tokens);
    target.completion_tokens = target
        .completion_tokens
        .saturating_add(usage.completion_tokens);
    target.total_tokens = target.total_tokens.saturating_add(usage.total_tokens);
    target.cached_tokens = Some(
        target
            .cached_tokens
            .unwrap_or_default()
            .saturating_add(usage.cached_tokens.unwrap_or_default()),
    );
    target.cache_write_tokens = Some(
        target
            .cache_write_tokens
            .unwrap_or_default()
            .saturating_add(usage.cache_write_tokens.unwrap_or_default()),
    );
    target.cache_discount = usage.cache_discount.or(target.cache_discount);
}

impl TurnRunner {
    pub(crate) fn new() -> Self {
        Self::with_max_continuations(crate::config::DEFAULT_MAX_TOOL_CONTINUATIONS)
    }

    pub(crate) fn with_max_continuations(max_continuations: usize) -> Self {
        Self {
            continuation_count: 0,
            adaptive_continuation_count: 0,
            replay_tokens: 0,
            // Replaying a provider prefix can amplify an incomplete
            // structured tool call. In particular, local models often
            // restart a large write from byte zero instead of resuming its
            // JSON arguments, growing context without making progress.
            max_continuations: max_continuations
                .max(1)
                .min(crate::config::MAX_CONFIGURED_TOOL_CONTINUATIONS),
        }
    }

    fn reserve_continuation(&mut self, accumulated: &str) -> bool {
        let prefix_tokens = crate::network::count_tokens(accumulated);
        let replay_tokens = self.replay_tokens.saturating_add(prefix_tokens);
        if replay_tokens > MAX_CONTINUATION_REPLAY_TOKENS {
            return false;
        }
        self.replay_tokens = replay_tokens;
        self.continuation_count += 1;
        true
    }

    pub(crate) fn allow_continuation(
        &mut self,
        response_is_cut_off: bool,
        accumulated: &str,
    ) -> bool {
        if !response_is_cut_off || self.continuation_count >= self.max_continuations {
            return false;
        }
        self.reserve_continuation(accumulated)
    }

    fn adaptive_output_limit(
        &mut self,
        current_limit: Option<u32>,
        accumulated: &str,
        policy: &ContinuationPolicy,
    ) -> Option<u32> {
        if self.continuation_count >= self.max_continuations
            || self.adaptive_continuation_count >= 1
        {
            return None;
        }
        let current_limit = current_limit?;
        let configured_limit = policy.adaptive_tool_output_limit?;
        let current_tokens = crate::network::count_tokens(accumulated);
        let mut next_limit = configured_limit.min(current_limit.saturating_mul(2));
        if let Some(context_limit) = policy.context_output_limit {
            next_limit = next_limit.min(context_limit.saturating_sub(current_tokens));
        }
        next_limit = next_limit.min(
            policy
                .max_total_output_tokens
                .saturating_sub(current_tokens),
        );
        if next_limit <= current_limit || !self.reserve_continuation(accumulated) {
            return None;
        }
        self.adaptive_continuation_count += 1;
        Some(next_limit)
    }
}

/// Collect one model response, transparently continuing responses cut off by
/// the provider. The callback owns request construction, allowing TUI, CLI,
/// and subagent adapters to share exactly one continuation policy.
pub(crate) async fn collect_response<F, Fut, E>(
    policy: ContinuationPolicy,
    mut request: F,
) -> Result<CollectedResponse, ResponseError>
where
    F: FnMut(ContinuationRequest) -> Fut,
    Fut: Future<Output = Result<ResponseChunk, E>>,
    E: Into<ResponseError>,
{
    let mut accumulated = String::new();
    let mut has_native_tool_calls = false;
    let mut thought_time_ms: u64 = 0;
    let mut thought_tokens: u32 = 0;
    let mut token_usage = None;
    let mut runner = TurnRunner::with_max_continuations(policy.max_continuations);
    let mut next_output_token_limit = None;
    loop {
        let output_token_limit = next_output_token_limit.take();
        let chunk = request(ContinuationRequest {
            previous: accumulated.clone(),
            output_token_limit,
        })
        .await
        .map_err(Into::into)
        .map_err(|mut error| {
            // A failed continuation only reports the bytes from that request;
            // join them to the successful prefix before handing the error back
            // to the turn layer. This is the checkpoint that must survive a
            // provider/device failure.
            if !accumulated.is_empty() {
                let mut content = accumulated.clone();
                content.push_str(&error.partial_content);
                error.partial_content = content;
            }
            error
        })?;
        accumulated.push_str(&chunk.content);
        // This describes the segment that ended the collected response, not
        // any earlier segment. A later reasoning-only continuation must clear
        // an earlier content boundary before recovery evaluates the result.
        let final_answer_boundary = chunk.final_answer_boundary;
        let provider_final_answer_state = chunk.provider_final_answer_state;
        has_native_tool_calls |= chunk.has_native_tool_calls;
        thought_time_ms = thought_time_ms.saturating_add(chunk.thought_time_ms);
        thought_tokens = thought_tokens.saturating_add(chunk.thought_tokens);
        add_usage(&mut token_usage, chunk.token_usage);
        if !has_native_tool_calls {
            let cut_off = crate::network::is_cut_off(&accumulated, chunk.finish_reason.as_deref());
            let adaptive_candidate = crate::network::text::is_adaptive_tool_continuation_candidate(
                &accumulated,
                chunk.finish_reason.as_deref(),
            );
            let adaptive_limit = if adaptive_candidate {
                runner.adaptive_output_limit(chunk.output_token_limit, &accumulated, &policy)
            } else {
                None
            };
            if adaptive_candidate {
                crate::logger::operational_event(
                    "turn.adaptive_continuation",
                    serde_json::json!({
                        "finish_reason": chunk.finish_reason.as_deref(),
                        "current_output_limit": chunk.output_token_limit,
                        "next_output_limit": adaptive_limit,
                        "current_output_tokens": crate::network::count_tokens(&accumulated),
                        "context_output_limit": policy.context_output_limit,
                        "max_total_output_tokens": policy.max_total_output_tokens,
                        "outcome": if adaptive_limit.is_some() { "escalate" } else { "blocked" },
                    }),
                );
            }
            let should_continue = if adaptive_candidate {
                adaptive_limit.is_some()
                    || (policy.adaptive_tool_output_limit.is_none()
                        && runner.allow_continuation(cut_off, &accumulated))
            } else {
                runner.allow_continuation(cut_off, &accumulated)
            };
            if should_continue {
                next_output_token_limit = adaptive_limit;
                continue;
            }
        }
        let (content, finish_reason) = if let Some(prefix) =
            crate::network::text::complete_native_tool_call_prefix(&accumulated)
        {
            // A complete native call is actionable now. Drop any later
            // incomplete call so tolerant JSON repair cannot execute a
            // truncated mutation, and treat the safe prefix as a completed
            // tool round rather than an output-limit failure.
            (prefix.to_string(), Some("stop".to_string()))
        } else {
            (accumulated, chunk.finish_reason)
        };
        return Ok(CollectedResponse {
            content,
            final_answer_boundary,
            provider_final_answer_state,
            finish_reason,
            thought_time_ms,
            thought_tokens,
            token_usage,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::stream::{FinalAnswerBoundary, ProviderFinalAnswerState};

    #[test]
    fn continuation_policy_is_bounded_and_reusable() {
        let mut runner = TurnRunner::new();
        assert!(!runner.allow_continuation(false, ""));
        for _ in 0..2 {
            assert!(runner.allow_continuation(true, "small prefix"));
        }
        assert!(!runner.allow_continuation(true, "small prefix"));
    }

    #[test]
    fn continuation_replay_budget_blocks_large_prefix_growth() {
        let mut runner = TurnRunner::with_max_continuations(4);
        let prefix = "token ".repeat(MAX_CONTINUATION_REPLAY_TOKENS as usize * 2);
        assert!(!runner.allow_continuation(true, &prefix));
    }

    #[tokio::test]
    async fn collect_response_retries_cut_off_chunks() {
        let mut calls = 0;
        let mut previous_args = Vec::new();
        let result = collect_response(ContinuationPolicy::default(), |request| {
            calls += 1;
            previous_args.push(request.previous);
            let chunk = if calls == 1 { "partial" } else { " finish" };
            let reason = if calls == 1 {
                Some("length".to_string())
            } else {
                Some("stop".to_string())
            };
            async move {
                Ok::<_, ResponseError>(ResponseChunk {
                    content: chunk.to_string(),
                    final_answer_boundary: FinalAnswerBoundary::None,
                    provider_final_answer_state: ProviderFinalAnswerState::None,
                    finish_reason: reason,
                    has_native_tool_calls: false,
                    output_token_limit: None,
                    thought_time_ms: 0,
                    thought_tokens: 0,
                    token_usage: None,
                })
            }
        })
        .await
        .expect("response should collect");

        assert_eq!(result.content, "partial finish");
        assert_eq!(calls, 2);
        assert_eq!(previous_args, ["", "partial"]);
    }

    #[tokio::test]
    async fn collect_response_stops_after_complete_textual_call_before_incomplete_call() {
        let mut calls = 0;
        let result = collect_response(
            ContinuationPolicy {
                adaptive_tool_output_limit: Some(16_000),
                context_output_limit: Some(100_000),
                max_total_output_tokens: 32_768,
                max_continuations: 2,
            },
            |_request| {
                calls += 1;
                async move {
                    Ok::<_, ResponseError>(ResponseChunk {
                        content: concat!(
                            "[TOOL_CALLS]list_directory[ARGS]{\"path\":\"/tmp\"}\n",
                            "[TOOL_CALLS]write_to_file[ARGS]{\"path\":\"x\",",
                            "\"content\":\"partial"
                        )
                        .into(),
                        final_answer_boundary: FinalAnswerBoundary::None,
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some("length".into()),
                        has_native_tool_calls: false,
                        output_token_limit: Some(8_192),
                        thought_time_ms: 0,
                        thought_tokens: 0,
                        token_usage: None,
                    })
                }
            },
        )
        .await
        .expect("complete textual tool call should be collected");

        assert_eq!(calls, 1);
        assert_eq!(
            result.content,
            "[TOOL_CALLS]list_directory[ARGS]{\"path\":\"/tmp\"}\n"
        );
        assert_eq!(result.finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn collect_response_stops_on_native_tool_call_with_reasoning() {
        let mut calls = 0;
        let result = collect_response(ContinuationPolicy::default(), |request| {
            calls += 1;
            async move {
                Ok::<_, ResponseError>(ResponseChunk {
                    content: if request.previous.is_empty() {
                        "<think>plan</think>".into()
                    } else {
                        "unexpected continuation".into()
                    },
                    final_answer_boundary: FinalAnswerBoundary::None,
                    provider_final_answer_state: ProviderFinalAnswerState::None,
                    finish_reason: Some("stop".into()),
                    has_native_tool_calls: true,
                    output_token_limit: None,
                    thought_time_ms: 0,
                    thought_tokens: 0,
                    token_usage: None,
                })
            }
        })
        .await
        .expect("native tool response should collect");

        assert_eq!(calls, 1);
        assert_eq!(result.content, "<think>plan</think>");
    }

    #[tokio::test]
    async fn collect_response_continues_reasoning_only_without_native_tool_call() {
        let mut calls = 0;
        let result = collect_response(ContinuationPolicy::default(), |request| {
            calls += 1;
            async move {
                Ok::<_, ResponseError>(ResponseChunk {
                    content: if request.previous.is_empty() {
                        "<think>plan</think>".into()
                    } else {
                        "answer".into()
                    },
                    final_answer_boundary: if request.previous.is_empty() {
                        FinalAnswerBoundary::None
                    } else {
                        FinalAnswerBoundary::ReasoningClosed
                    },
                    provider_final_answer_state: if request.previous.is_empty() {
                        ProviderFinalAnswerState::None
                    } else {
                        ProviderFinalAnswerState::Terminal
                    },
                    finish_reason: Some("stop".into()),
                    has_native_tool_calls: false,
                    output_token_limit: None,
                    thought_time_ms: 0,
                    thought_tokens: 0,
                    token_usage: None,
                })
            }
        })
        .await
        .expect("reasoning-only response should continue");

        assert_eq!(calls, 2);
        assert_eq!(result.content, "<think>plan</think>answer");
        assert_eq!(
            result.final_answer_boundary,
            FinalAnswerBoundary::ReasoningClosed
        );
        assert_eq!(
            result.provider_final_answer_state,
            ProviderFinalAnswerState::Terminal
        );
    }

    #[tokio::test]
    async fn later_reasoning_only_continuation_invalidates_earlier_content_boundary() {
        let mut calls = 0;
        let result = collect_response(ContinuationPolicy::default(), |_| {
            calls += 1;
            let (content, final_answer_boundary, finish_reason) = match calls {
                1 => (
                    "<think>completed inspection</think>".to_string(),
                    FinalAnswerBoundary::None,
                    Some("length".to_string()),
                ),
                2 => (
                    "Findings: src/app.ts has a validated export boundary.".to_string(),
                    FinalAnswerBoundary::ReasoningClosed,
                    Some("length".to_string()),
                ),
                3 => (
                    "<think>I got stuck reconsidering the same review.</think>".to_string(),
                    FinalAnswerBoundary::None,
                    Some("reasoning_loop".to_string()),
                ),
                _ => panic!("unexpected continuation"),
            };
            async move {
                Ok::<_, ResponseError>(ResponseChunk {
                    content,
                    final_answer_boundary,
                    provider_final_answer_state: ProviderFinalAnswerState::None,
                    finish_reason,
                    has_native_tool_calls: false,
                    output_token_limit: None,
                    thought_time_ms: 0,
                    thought_tokens: 0,
                    token_usage: None,
                })
            }
        })
        .await
        .expect("reasoning loop response should collect");

        assert_eq!(calls, 3);
        assert_eq!(result.final_answer_boundary, FinalAnswerBoundary::None);
        assert_eq!(result.finish_reason.as_deref(), Some("reasoning_loop"));
    }

    #[tokio::test]
    async fn collect_response_accumulates_thought_stats_across_segments() {
        let mut calls = 0;
        let result = collect_response(ContinuationPolicy::default(), |request| {
            calls += 1;
            async move {
                Ok::<_, ResponseError>(ResponseChunk {
                    content: if request.previous.is_empty() {
                        "<think>first</think>".into()
                    } else {
                        "answer".into()
                    },
                    final_answer_boundary: if request.previous.is_empty() {
                        FinalAnswerBoundary::None
                    } else {
                        FinalAnswerBoundary::ReasoningClosed
                    },
                    provider_final_answer_state: ProviderFinalAnswerState::None,
                    finish_reason: Some("stop".into()),
                    has_native_tool_calls: false,
                    output_token_limit: None,
                    thought_time_ms: if request.previous.is_empty() {
                        250
                    } else {
                        400
                    },
                    thought_tokens: if request.previous.is_empty() { 12 } else { 8 },
                    token_usage: None,
                })
            }
        })
        .await
        .expect("thought stats should collect");

        assert_eq!(calls, 2);
        assert_eq!(result.thought_time_ms, 650);
        assert_eq!(result.thought_tokens, 20);
    }

    #[tokio::test]
    async fn adaptive_continuation_raises_only_an_incomplete_tool_call() {
        let mut calls = 0;
        let mut requested_limits = Vec::new();
        let result = collect_response(
            ContinuationPolicy {
                adaptive_tool_output_limit: Some(16_000),
                context_output_limit: Some(100_000),
                max_total_output_tokens: 32_768,
                max_continuations: 2,
            },
            |request| {
                calls += 1;
                requested_limits.push(request.output_token_limit);
                let content = if calls == 1 {
                    "<tool_call><function=write_to_file>{\"path\":\"x\",\"content\":\"partial"
                } else {
                    "}</tool_call>"
                };
                async move {
                    Ok::<_, ResponseError>(ResponseChunk {
                        content: content.to_string(),
                        final_answer_boundary: FinalAnswerBoundary::None,
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some(if calls == 1 {
                            "length".to_string()
                        } else {
                            "stop".to_string()
                        }),
                        has_native_tool_calls: false,
                        output_token_limit: Some(8_192),
                        thought_time_ms: 0,
                        thought_tokens: 0,
                        token_usage: None,
                    })
                }
            },
        )
        .await
        .expect("adaptive continuation should collect");

        assert_eq!(calls, 2);
        assert_eq!(requested_limits, [None, Some(16_000)]);
        assert!(result.content.ends_with("</tool_call>"));
    }

    #[tokio::test]
    async fn adaptive_continuation_does_not_raise_reasoning_or_complete_calls() {
        let mut calls = 0;
        let mut requested_limits = Vec::new();
        let _ = collect_response(
            ContinuationPolicy {
                adaptive_tool_output_limit: Some(16_000),
                context_output_limit: Some(100_000),
                max_total_output_tokens: 32_768,
                max_continuations: 2,
            },
            |request| {
                calls += 1;
                requested_limits.push(request.output_token_limit);
                async move {
                    Ok::<_, ResponseError>(ResponseChunk {
                        content: if calls == 1 {
                            "<think>still planning</think>".into()
                        } else {
                            "answer".into()
                        },
                        final_answer_boundary: if calls == 1 {
                            FinalAnswerBoundary::None
                        } else {
                            FinalAnswerBoundary::ReasoningClosed
                        },
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some(if calls == 1 {
                            "length".into()
                        } else {
                            "stop".into()
                        }),
                        has_native_tool_calls: false,
                        output_token_limit: Some(8_192),
                        thought_time_ms: 0,
                        thought_tokens: 0,
                        token_usage: None,
                    })
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(requested_limits, [None, None]);

        calls = 0;
        requested_limits.clear();
        let _ = collect_response(
            ContinuationPolicy {
                adaptive_tool_output_limit: Some(16_000),
                context_output_limit: Some(100_000),
                max_total_output_tokens: 32_768,
                max_continuations: 2,
            },
            |request| {
                calls += 1;
                requested_limits.push(request.output_token_limit);
                async move {
                    Ok::<_, ResponseError>(ResponseChunk {
                        content: "```tool\n{\"name\":\"get_time\",\"arguments\":{}}\n```".into(),
                        final_answer_boundary: FinalAnswerBoundary::None,
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some("length".into()),
                        has_native_tool_calls: false,
                        output_token_limit: Some(8_192),
                        thought_time_ms: 0,
                        thought_tokens: 0,
                        token_usage: None,
                    })
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(requested_limits, [None]);
    }

    #[tokio::test]
    async fn adaptive_continuation_stops_when_context_has_no_room() {
        let mut calls = 0;
        let mut requested_limits = Vec::new();
        let _ = collect_response(
            ContinuationPolicy {
                adaptive_tool_output_limit: Some(16_000),
                context_output_limit: Some(1),
                max_total_output_tokens: 32_768,
                max_continuations: 2,
            },
            |request| {
                calls += 1;
                requested_limits.push(request.output_token_limit);
                async move {
                    Ok::<_, ResponseError>(ResponseChunk {
                        content: "<tool_call><function=write_to_file>{\"path\":\"x\",\"content\":\"partial".into(),
                        final_answer_boundary: FinalAnswerBoundary::None,
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some("length".into()),
                        has_native_tool_calls: false,
                        output_token_limit: Some(8_192),
                        thought_time_ms: 0,
                    thought_tokens: 0,
                    token_usage: None,
                    })
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(requested_limits, [None]);
    }

    #[tokio::test]
    async fn adaptive_continuation_stops_safely_at_total_output_ceiling() {
        let mut calls = 0;
        let mut requested_limits = Vec::new();
        let result = collect_response(
            ContinuationPolicy {
                adaptive_tool_output_limit: Some(16_000),
                context_output_limit: Some(100_000),
                max_total_output_tokens: 8_192,
                max_continuations: 2,
            },
            |request| {
                calls += 1;
                requested_limits.push(request.output_token_limit);
                async move {
                    Ok::<_, ResponseError>(ResponseChunk {
                        content: "<tool_call><function=write_to_file>{\"path\":\"x\",\"content\":\"partial"
                            .into(),
                        final_answer_boundary: FinalAnswerBoundary::None,
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some("length".into()),
                        has_native_tool_calls: false,
                        output_token_limit: Some(8_192),
                        thought_time_ms: 0,
                    thought_tokens: 0,
                    token_usage: None,
                    })
                }
            },
        )
        .await
        .expect("bounded truncation should return the unexecuted response");

        assert_eq!(calls, 1);
        assert_eq!(requested_limits, [None]);
        assert!(crate::network::is_cut_off(
            &result.content,
            result.finish_reason.as_deref()
        ));
        assert!(
            crate::tools::parse_tool_calls(&result.content, crate::config::ToolProtocol::Native)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_continuation_preserves_prefix_and_failed_stream_bytes() {
        let mut calls = 0;
        let result = collect_response(ContinuationPolicy::default(), |request| {
            calls += 1;
            async move {
                if request.previous.is_empty() {
                    Ok::<_, ResponseError>(ResponseChunk {
                        content:
                            "[TOOL_CALLS]write_to_file[ARGS]{\"path\":\"x\",\"content\":\"partial"
                                .into(),
                        final_answer_boundary: FinalAnswerBoundary::None,
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some("length".into()),
                        has_native_tool_calls: false,
                        output_token_limit: None,
                        thought_time_ms: 0,
                        thought_tokens: 0,
                        token_usage: None,
                    })
                } else {
                    Err(ResponseError::with_partial(
                        "stream_failure:provider_error status=200 events_received=2",
                        " still incomplete".into(),
                    ))
                }
            }
        })
        .await
        .expect_err("the injected provider failure must reach the caller");

        assert_eq!(calls, 2);
        assert_eq!(
            result.partial_content,
            "[TOOL_CALLS]write_to_file[ARGS]{\"path\":\"x\",\"content\":\"partial still incomplete"
        );
        assert!(!result.partial_content.is_empty());
    }

    #[tokio::test]
    async fn failed_native_continuation_preserves_non_executable_checkpoint() {
        let checkpoint = super::super::stream::NativeToolCallCheckpoint {
            index: Some(0),
            call_id: Some("call-1".to_owned()),
            tool_name: "write_to_file".to_owned(),
            argument_bytes: 12,
            arguments_complete: false,
            arguments_overflowed: false,
            argument_fingerprint: "abc".to_owned(),
            diagnostic: "unexpected end".to_owned(),
        };
        let result = collect_response(ContinuationPolicy::default(), |_request| async {
            Err::<ResponseChunk, _>(ResponseError::with_partial_native(
                "stream_failure:provider_error status=200 events_received=1",
                String::new(),
                vec![checkpoint.clone()],
            ))
        })
        .await
        .expect_err("the injected provider failure must reach the caller");

        assert!(result.partial_content.is_empty());
        assert_eq!(result.partial_native_tool_calls, vec![checkpoint]);
    }
}
