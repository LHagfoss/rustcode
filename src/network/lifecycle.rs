use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StopReason {
    Completed,
    BackgroundPending,
    Cancelled,
    RecoveryFailed,
    LoopEscalation,
    ProviderError(Option<u16>),
    UnavailableTool,
    BudgetExceeded(String),
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Completed => f.write_str("completed"),
            Self::BackgroundPending => f.write_str("background_pending"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::RecoveryFailed => f.write_str("recovery_failed"),
            Self::LoopEscalation => f.write_str("loop_escalation"),
            Self::ProviderError(Some(status)) => write!(f, "provider_error:{status}"),
            Self::ProviderError(None) => f.write_str("provider_error"),
            Self::UnavailableTool => f.write_str("unavailable_tool"),
            Self::BudgetExceeded(limit) => write!(f, "budget:{limit}"),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TurnLifecycle {
    finalized: bool,
}

impl TurnLifecycle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn mark_finalized(&mut self) -> bool {
        if self.finalized {
            return false;
        }
        self.finalized = true;
        true
    }
}

pub(crate) fn is_unavailable_tool_error(reason: &str) -> bool {
    reason.contains("unknown or unavailable tool")
}

pub(crate) fn final_transcript_content(
    task_completed: bool,
    content: &str,
    content_already_persisted: bool,
    reason: &StopReason,
) -> Option<String> {
    if task_completed {
        None
    } else if matches!(reason, StopReason::BackgroundPending) {
        None
    } else if content_already_persisted || content.trim().is_empty() {
        Some(format!("[harness: turn stopped — {reason}]"))
    } else {
        Some(content.to_string())
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    #[test]
    fn unavailable_tool_detection_is_narrow() {
        assert!(is_unavailable_tool_error(
            "unknown or unavailable tool 'mystery'; use only tools in the current registry"
        ));
        assert!(!is_unavailable_tool_error(
            "invalid arguments for 'grep': missing required property 'pattern'"
        ));
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    #[test]
    fn completed_turn_does_not_request_a_duplicate_assistant_message() {
        assert_eq!(
            final_transcript_content(true, "already recorded", true, &StopReason::Completed),
            None
        );
    }

    #[test]
    fn empty_terminal_turn_gets_a_durable_stop_marker() {
        assert_eq!(
            final_transcript_content(false, "", false, &StopReason::Cancelled),
            Some("[harness: turn stopped — cancelled]".to_string())
        );
    }

    #[test]
    fn persisted_tool_round_content_is_not_appended_again_on_cancel() {
        assert_eq!(
            final_transcript_content(
                false,
                "assistant response already followed by a tool result",
                true,
                &StopReason::Cancelled,
            ),
            Some("[harness: turn stopped — cancelled]".to_string())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reasons_have_stable_benchmark_values() {
        assert_eq!(StopReason::Completed.to_string(), "completed");
        assert_eq!(StopReason::Cancelled.to_string(), "cancelled");
        assert_eq!(StopReason::RecoveryFailed.to_string(), "recovery_failed");
        assert_eq!(StopReason::LoopEscalation.to_string(), "loop_escalation");
        assert_eq!(
            StopReason::ProviderError(Some(429)).to_string(),
            "provider_error:429"
        );
        assert_eq!(
            StopReason::ProviderError(None).to_string(),
            "provider_error"
        );
        assert_eq!(StopReason::UnavailableTool.to_string(), "unavailable_tool");
        assert_eq!(
            StopReason::BudgetExceeded("tool_rounds=4".into()).to_string(),
            "budget:tool_rounds=4"
        );
    }

    #[test]
    fn finalization_guard_opens_once() {
        let mut lifecycle = TurnLifecycle::new();
        assert!(lifecycle.mark_finalized());
        assert!(!lifecycle.mark_finalized());
    }
}
