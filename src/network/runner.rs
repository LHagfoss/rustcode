use std::future::Future;

/// Shared continuation and loop policy for every model-facing turn loop.
pub(crate) struct TurnRunner {
    continuation_count: usize,
    max_continuations: usize,
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
) -> Result<(String, Option<String>), String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<(String, Option<String>), String>>,
{
    let mut accumulated = String::new();
    let mut runner = TurnRunner::new();
    loop {
        let (chunk, finish_reason) = request(accumulated.clone()).await?;
        accumulated.push_str(&chunk);
        if runner.allow_continuation(crate::network::is_cut_off(
            &accumulated,
            finish_reason.as_deref(),
        )) {
            continue;
        }
        return Ok((accumulated, finish_reason));
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
            async move { Ok((chunk.to_string(), reason)) }
        })
        .await
        .expect("response should collect");

        assert_eq!(result.0, "partial finish");
        assert_eq!(calls, 2);
        assert_eq!(previous_args, ["", "partial"]);
    }
}
