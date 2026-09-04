use std::future::Future;

/// Shared continuation and loop policy for every model-facing turn loop.
pub(crate) struct TurnRunner {
    continuation_count: usize,
    adaptive_continuation_count: usize,
    max_continuations: usize,
}

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
}

impl Default for ContinuationPolicy {
    fn default() -> Self {
        Self {
            adaptive_tool_output_limit: None,
            context_output_limit: None,
            max_total_output_tokens: 32_768,
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
}

pub(crate) struct CollectedResponse {
    pub(crate) content: String,
    pub(crate) final_answer_boundary: super::stream::FinalAnswerBoundary,
    pub(crate) provider_final_answer_state: super::stream::ProviderFinalAnswerState,
    pub(crate) finish_reason: Option<String>,
    pub(crate) thought_time_ms: u64,
    pub(crate) thought_tokens: u32,
}

impl TurnRunner {
    pub(crate) fn new() -> Self {
        Self {
            continuation_count: 0,
            adaptive_continuation_count: 0,
            // More than two provider continuations tends to amplify an
            // incomplete structured tool call. In particular, local models
            // often restart a large write from byte zero instead of resuming
            // its JSON arguments, growing context without making progress.
            max_continuations: 2,
        }
    }

    pub(crate) fn allow_continuation(&mut self, response_is_cut_off: bool) -> bool {
        if !response_is_cut_off || self.continuation_count >= self.max_continuations {
            return false;
        }
        self.continuation_count += 1;
        true
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
        if next_limit <= current_limit {
            return None;
        }
        self.continuation_count += 1;
        self.adaptive_continuation_count += 1;
        Some(next_limit)
    }
}

/// Collect one model response, transparently continuing responses cut off by
/// the provider. The callback owns request construction, allowing TUI, CLI,
/// and subagent adapters to share exactly one continuation policy.
pub(crate) async fn collect_response<F, Fut>(
    policy: ContinuationPolicy,
    mut request: F,
) -> Result<CollectedResponse, String>
where
    F: FnMut(ContinuationRequest) -> Fut,
    Fut: Future<Output = Result<ResponseChunk, String>>,
{
    let mut accumulated = String::new();
    let mut has_native_tool_calls = false;
    let mut thought_time_ms: u64 = 0;
    let mut thought_tokens: u32 = 0;
    let mut runner = TurnRunner::new();
    let mut next_output_token_limit = None;
    loop {
        let output_token_limit = next_output_token_limit.take();
        let chunk = request(ContinuationRequest {
            previous: accumulated.clone(),
            output_token_limit,
        })
        .await?;
        accumulated.push_str(&chunk.content);
        // This describes the segment that ended the collected response, not
        // any earlier segment. A later reasoning-only continuation must clear
        // an earlier content boundary before recovery evaluates the result.
        let final_answer_boundary = chunk.final_answer_boundary;
        let provider_final_answer_state = chunk.provider_final_answer_state;
        has_native_tool_calls |= chunk.has_native_tool_calls;
        thought_time_ms = thought_time_ms.saturating_add(chunk.thought_time_ms);
        thought_tokens = thought_tokens.saturating_add(chunk.thought_tokens);
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
                        && runner.allow_continuation(cut_off))
            } else {
                runner.allow_continuation(cut_off)
            };
            if should_continue {
                next_output_token_limit = adaptive_limit;
                continue;
            }
        }
        return Ok(CollectedResponse {
            content: accumulated,
            final_answer_boundary,
            provider_final_answer_state,
            finish_reason: chunk.finish_reason,
            thought_time_ms,
            thought_tokens,
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
        assert!(!runner.allow_continuation(false));
        for _ in 0..2 {
            assert!(runner.allow_continuation(true));
        }
        assert!(!runner.allow_continuation(true));
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
                Ok(ResponseChunk {
                    content: chunk.to_string(),
                    final_answer_boundary: FinalAnswerBoundary::None,
                    provider_final_answer_state: ProviderFinalAnswerState::None,
                    finish_reason: reason,
                    has_native_tool_calls: false,
                    output_token_limit: None,
                    thought_time_ms: 0,
                    thought_tokens: 0,
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
    async fn collect_response_stops_on_native_tool_call_with_reasoning() {
        let mut calls = 0;
        let result = collect_response(ContinuationPolicy::default(), |request| {
            calls += 1;
            async move {
                Ok(ResponseChunk {
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
                Ok(ResponseChunk {
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
                Ok(ResponseChunk {
                    content,
                    final_answer_boundary,
                    provider_final_answer_state: ProviderFinalAnswerState::None,
                    finish_reason,
                    has_native_tool_calls: false,
                    output_token_limit: None,
                    thought_time_ms: 0,
                    thought_tokens: 0,
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
                Ok(ResponseChunk {
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
                    Ok(ResponseChunk {
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
            },
            |request| {
                calls += 1;
                requested_limits.push(request.output_token_limit);
                async move {
                    Ok(ResponseChunk {
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
            },
            |request| {
                calls += 1;
                requested_limits.push(request.output_token_limit);
                async move {
                    Ok(ResponseChunk {
                        content: "```tool\n{\"name\":\"get_time\",\"arguments\":{}}\n```".into(),
                        final_answer_boundary: FinalAnswerBoundary::None,
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some("length".into()),
                        has_native_tool_calls: false,
                        output_token_limit: Some(8_192),
                        thought_time_ms: 0,
                        thought_tokens: 0,
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
            },
            |request| {
                calls += 1;
                requested_limits.push(request.output_token_limit);
                async move {
                    Ok(ResponseChunk {
                        content: "<tool_call><function=write_to_file>{\"path\":\"x\",\"content\":\"partial".into(),
                        final_answer_boundary: FinalAnswerBoundary::None,
                        provider_final_answer_state: ProviderFinalAnswerState::None,
                        finish_reason: Some("length".into()),
                        has_native_tool_calls: false,
                        output_token_limit: Some(8_192),
                        thought_time_ms: 0,
                        thought_tokens: 0,
                    })
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(requested_limits, [None]);
    }
}
