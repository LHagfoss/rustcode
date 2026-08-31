use std::fmt;

/// Failure phases for a streaming provider request.  Keeping these separate
/// from the human-facing stop reason lets callers decide whether a request is
/// safe to retry without having to parse log text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamFailureKind {
    ConnectTimeout,
    HeaderTimeout,
    FirstEventTimeout,
    StreamIdleTimeout,
    PrematureEof,
    MalformedSse,
    ProviderError,
    Cancelled,
}

impl fmt::Display for StreamFailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ConnectTimeout => "connect_timeout",
            Self::HeaderTimeout => "header_timeout",
            Self::FirstEventTimeout => "first_event_timeout",
            Self::StreamIdleTimeout => "stream_idle_timeout",
            Self::PrematureEof => "premature_eof",
            Self::MalformedSse => "malformed_sse",
            Self::ProviderError => "provider_error",
            Self::Cancelled => "cancelled",
        })
    }
}

pub(crate) fn stream_failure_kind_from_message(message: &str) -> Option<StreamFailureKind> {
    let name = message
        .strip_prefix("stream_failure:")?
        .split_whitespace()
        .next()?;
    Some(match name {
        "connect_timeout" => StreamFailureKind::ConnectTimeout,
        "header_timeout" => StreamFailureKind::HeaderTimeout,
        "first_event_timeout" => StreamFailureKind::FirstEventTimeout,
        "stream_idle_timeout" => StreamFailureKind::StreamIdleTimeout,
        "premature_eof" => StreamFailureKind::PrematureEof,
        "malformed_sse" => StreamFailureKind::MalformedSse,
        "provider_error" => StreamFailureKind::ProviderError,
        "cancelled" => StreamFailureKind::Cancelled,
        _ => return None,
    })
}

/// Structured stream failure metadata.  The byte/event counters are useful
/// when a provider stalls after emitting partial output: an idle stream with
/// no bytes is materially different from one that stopped halfway through an
/// SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamFailure {
    pub(crate) kind: StreamFailureKind,
    pub(crate) status: Option<u16>,
    pub(crate) detail: Option<String>,
    pub(crate) bytes_received: usize,
    pub(crate) events_received: usize,
    pub(crate) partial_event_bytes: usize,
}

pub(crate) fn stop_reason_for_stream_failure(
    task_completed: bool,
    kind: StreamFailureKind,
) -> StopReason {
    if task_completed {
        StopReason::CompletedWithWarning(kind)
    } else if kind == StreamFailureKind::Cancelled {
        StopReason::Cancelled
    } else if kind == StreamFailureKind::ProviderError {
        StopReason::ProviderError(None)
    } else {
        StopReason::TransportFailure(kind)
    }
}

impl StreamFailure {
    pub(crate) fn new(kind: StreamFailureKind) -> Self {
        Self {
            kind,
            status: None,
            detail: None,
            bytes_received: 0,
            events_received: 0,
            partial_event_bytes: 0,
        }
    }
}

impl fmt::Display for StreamFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stream_failure:{} status={} bytes_received={} events_received={} partial_event_bytes={}{}",
            self.kind,
            self.status
                .map_or_else(|| "none".to_string(), |status| status.to_string()),
            self.bytes_received,
            self.events_received,
            self.partial_event_bytes,
            self.detail
                .as_deref()
                .map_or_else(String::new, |detail| format!(" detail={detail}"))
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StopReason {
    Completed,
    CompletedWithWarning(StreamFailureKind),
    TransportFailure(StreamFailureKind),
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
            Self::CompletedWithWarning(kind) => write!(f, "completed_with_warning:{kind}"),
            Self::TransportFailure(kind) => write!(f, "{kind}"),
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
        assert_eq!(
            StopReason::CompletedWithWarning(StreamFailureKind::StreamIdleTimeout).to_string(),
            "completed_with_warning:stream_idle_timeout"
        );
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
    fn stream_failure_display_keeps_transport_evidence() {
        let failure = StreamFailure {
            kind: StreamFailureKind::StreamIdleTimeout,
            status: None,
            detail: None,
            bytes_received: 42,
            events_received: 3,
            partial_event_bytes: 7,
        };
        assert_eq!(
            failure.to_string(),
            "stream_failure:stream_idle_timeout status=none bytes_received=42 events_received=3 partial_event_bytes=7"
        );
    }

    #[test]
    fn stream_failure_kind_can_be_recovered_from_runner_error() {
        assert_eq!(
            stream_failure_kind_from_message(
                "stream_failure:first_event_timeout status=none bytes_received=0 events_received=0 partial_event_bytes=0"
            ),
            Some(StreamFailureKind::FirstEventTimeout)
        );
        assert_eq!(stream_failure_kind_from_message("provider exploded"), None);
    }

    #[test]
    fn verified_completion_wins_over_a_later_transport_failure() {
        assert_eq!(
            stop_reason_for_stream_failure(true, StreamFailureKind::StreamIdleTimeout),
            StopReason::CompletedWithWarning(StreamFailureKind::StreamIdleTimeout)
        );
        assert_eq!(
            stop_reason_for_stream_failure(false, StreamFailureKind::StreamIdleTimeout),
            StopReason::TransportFailure(StreamFailureKind::StreamIdleTimeout)
        );
    }

    #[test]
    fn finalization_guard_opens_once() {
        let mut lifecycle = TurnLifecycle::new();
        assert!(lifecycle.mark_finalized());
        assert!(!lifecycle.mark_finalized());
    }
}
