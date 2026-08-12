use std::future::Future;

/// Shared continuation and loop policy for every model-facing turn loop.
pub(crate) struct TurnRunner {
    continuation_count: usize,
    max_continuations: usize,
}

pub(crate) struct ResponseChunk {
    pub(crate) content: String,
    pub(crate) finish_reason: Option<String>,
    pub(crate) has_native_tool_calls: bool,
}

pub(crate) struct CollectedResponse {
    pub(crate) content: String,
    pub(crate) finish_reason: Option<String>,
}

impl TurnRunner {
    pub(crate) fn new() -> Self {
        Self {
            continuation_count: 0,
            max_continuations: 5,
        }
    }

    pub(crate) fn allow_continuation(&mut self, response_is_cut_off: bool) -> bool {
        if !response_is_cut_off || self.continuation_count >= self.max_continuations {
            return false;
        }
        self.continuation_count += 1;
        true
    }
}

/// Collect one model response, transparently continuing responses cut off by
/// the provider. The callback owns request construction, allowing TUI, CLI,
/// and subagent adapters to share exactly one continuation policy.
pub(crate) async fn collect_response<F, Fut>(
    mut request: F,
) -> Result<CollectedResponse, String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<ResponseChunk, String>>,
{
    let mut accumulated = String::new();
    let mut has_native_tool_calls = false;
    let mut runner = TurnRunner::new();
    loop {
        let chunk = request(accumulated.clone()).await?;
        accumulated.push_str(&chunk.content);
        has_native_tool_calls |= chunk.has_native_tool_calls;
        if !has_native_tool_calls
            && runner.allow_continuation(crate::network::is_cut_off(
                &accumulated,
                chunk.finish_reason.as_deref(),
            ))
        {
            continue;
        }
        return Ok(CollectedResponse {
            content: accumulated,
            finish_reason: chunk.finish_reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_policy_is_bounded_and_reusable() {
        let mut runner = TurnRunner::new();
        assert!(!runner.allow_continuation(false));
        for _ in 0..5 {
            assert!(runner.allow_continuation(true));
        }
        assert!(!runner.allow_continuation(true));
    }

    #[tokio::test]
    async fn collect_response_retries_cut_off_chunks() {
        let mut calls = 0;
        let mut previous_args = Vec::new();
        let result = collect_response(|previous| {
            calls += 1;
            previous_args.push(previous);
            let chunk = if calls == 1 { "partial" } else { " finish" };
            let reason = if calls == 1 {
                Some("length".to_string())
            } else {
                Some("stop".to_string())
            };
            async move {
                Ok(ResponseChunk {
                    content: chunk.to_string(),
                    finish_reason: reason,
                    has_native_tool_calls: false,
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
        let result = collect_response(|previous| {
            calls += 1;
            async move {
                Ok(ResponseChunk {
                    content: if previous.is_empty() {
                        "<think>plan</think>".into()
                    } else {
                        "unexpected continuation".into()
                    },
                    finish_reason: Some("stop".into()),
                    has_native_tool_calls: true,
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
        let result = collect_response(|previous| {
            calls += 1;
            async move {
                Ok(ResponseChunk {
                    content: if previous.is_empty() {
                        "<think>plan</think>".into()
                    } else {
                        "answer".into()
                    },
                    finish_reason: Some("stop".into()),
                    has_native_tool_calls: false,
                })
            }
        })
        .await
        .expect("reasoning-only response should continue");

        assert_eq!(calls, 2);
        assert_eq!(result.content, "<think>plan</think>answer");
    }
}
