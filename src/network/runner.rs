use crate::network::loop_detect::{LoopDetector, LoopStatus};
use serde_json::Value;

/// Shared continuation and loop policy for every model-facing turn loop.
pub(crate) struct TurnRunner {
    continuation_count: usize,
    max_continuations: usize,
    loop_detector: LoopDetector,
}

impl TurnRunner {
    pub(crate) fn new() -> Self {
        Self {
            continuation_count: 0,
            max_continuations: 5,
            loop_detector: LoopDetector::new(6),
        }
    }

    pub(crate) fn allow_continuation(&mut self, response_is_cut_off: bool) -> bool {
        if !response_is_cut_off || self.continuation_count >= self.max_continuations {
            return false;
        }
        self.continuation_count += 1;
        true
    }

    pub(crate) fn continuation_count(&self) -> usize {
        self.continuation_count
    }

    pub(crate) fn check_tool(&mut self, name: &str, arguments: &Value) -> LoopStatus {
        let (exact, category) = crate::network::loop_detect::signatures(name, arguments);
        self.loop_detector.check_tool(name, &exact, &category)
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
        assert_eq!(runner.continuation_count(), 5);
    }
}
