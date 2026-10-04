//! Bounded inspection futures stay owned by the batch until all have settled.
use futures_util::{StreamExt, stream};
use std::future::Future;

pub(crate) async fn ordered_bounded<I, F, T>(
    inputs: Vec<I>,
    limit: usize,
    execute: impl Fn(I) -> F,
) -> Vec<T>
where
    F: Future<Output = T>,
{
    let mut results = stream::iter(inputs.into_iter().enumerate().map(|(index, input)| {
        let future = execute(input);
        async move { (index, future.await) }
    }))
    .buffer_unordered(limit.max(1))
    .collect::<Vec<_>>()
    .await;
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

pub(crate) fn parallel_inspection(call: &crate::tools::ToolCall) -> bool {
    matches!(
        call.name.as_str(),
        "view_file"
            | "grep"
            | "glob"
            | "list_directory"
            | "find_symbol"
            | "project_map"
            | "codebase_map"
            | "get_project_map"
    ) && crate::tools::is_read_only_call(call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[tokio::test(start_paused = true)]
    async fn reads_overlap_with_bounded_admission_and_announcement_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let start = tokio::time::Instant::now();
        let results = ordered_bounded(vec![80u64, 20, 25, 120], 2, |delay| {
            let active = active.clone();
            let peak = peak.clone();
            async move {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                delay
            }
        })
        .await;
        assert_eq!(results, vec![80, 20, 25, 120]);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(start.elapsed() < std::time::Duration::from_millis(245));
        println!(
            "inspection delay workload: serial=245ms parallel={}ms",
            start.elapsed().as_millis()
        );
    }
    #[test]
    fn delegation_shell_and_control_calls_are_barriers() {
        for name in [
            "run_command",
            "write_to_file",
            "use_skill",
            "spawn_agent",
            "unknown_mcp",
        ] {
            assert!(!parallel_inspection(&crate::tools::ToolCall {
                name: name.into(),
                arguments: serde_json::json!({}),
                call_id: None
            }));
        }
    }
}
