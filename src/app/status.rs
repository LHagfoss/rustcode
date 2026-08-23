use crate::app::state::{AppState, AppStatus, LiveToolCall, StreamTracker, TokenUsage};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) fn format_elapsed_compact(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        format!("{elapsed_secs}s")
    } else if elapsed_secs < 3600 {
        format!("{}m {:02}s", elapsed_secs / 60, elapsed_secs % 60)
    } else {
        format!(
            "{}h {:02}m {:02}s",
            elapsed_secs / 3600,
            (elapsed_secs % 3600) / 60,
            elapsed_secs % 60
        )
    }
}

pub(crate) fn context_remaining_percent(used_tokens: u32, context_window: u32) -> u32 {
    if context_window == 0 {
        return 0;
    }
    100u32.saturating_sub(
        ((used_tokens as f64 / context_window as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u32,
    )
}

pub(crate) fn should_notify_response_finished(
    response_just_finished: bool,
    terminal_focused: bool,
) -> bool {
    response_just_finished && !terminal_focused
}

#[allow(dead_code)]
pub(crate) struct StatusState<'a> {
    status: &'a mut AppStatus,
    response_time: &'a mut Option<Duration>,
    generation_start_time: &'a mut Option<Instant>,
    current_token_usage: &'a mut Option<TokenUsage>,
    current_thought_time_ms: &'a mut u64,
    current_thought_tokens: &'a mut u32,
    stream_tracker: &'a mut Option<StreamTracker>,
    running_tools: &'a mut Vec<String>,
    live_tool_calls: &'a Arc<Vec<LiveToolCall>>,
    current_terminal_title: &'a mut Option<String>,
}

impl<'a> StatusState<'a> {
    pub(crate) fn new(state: &'a mut AppState) -> Self {
        Self {
            status: &mut state.status,
            response_time: &mut state.response_time,
            generation_start_time: &mut state.generation_start_time,
            current_token_usage: &mut state.current_token_usage,
            current_thought_time_ms: &mut state.current_thought_time_ms,
            current_thought_tokens: &mut state.current_thought_tokens,
            stream_tracker: &mut state.stream_tracker,
            running_tools: &mut state.running_tools,
            live_tool_calls: &state.live_tool_calls,
            current_terminal_title: &mut state.current_terminal_title,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        *self.status != AppStatus::Idle
    }

    #[allow(dead_code)]
    pub(crate) fn status(&self) -> &AppStatus {
        self.status
    }

    #[allow(dead_code)]
    pub(crate) fn set_idle(&mut self) {
        *self.status = AppStatus::Idle;
    }

    #[allow(dead_code)]
    pub(crate) fn active_tool_count(&self) -> usize {
        self.running_tools.len().max(self.live_tool_calls.len())
    }
}

impl AppState {
    pub(crate) fn status_state(&mut self) -> StatusState<'_> {
        StatusState::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        StatusState, context_remaining_percent, format_elapsed_compact,
        should_notify_response_finished,
    };
    use crate::app::{AppState, AppStatus};

    #[test]
    fn status_view_reports_active_work() {
        let mut state = AppState::new();
        state.status = AppStatus::Streaming;

        let status = StatusState::new(&mut state);
        assert!(status.is_active());
    }

    #[test]
    fn status_formatting_stays_compact_for_footer_and_live_work() {
        assert_eq!(format_elapsed_compact(0), "0s");
        assert_eq!(format_elapsed_compact(61), "1m 01s");
        assert_eq!(format_elapsed_compact(3_723), "1h 02m 03s");
        assert_eq!(context_remaining_percent(25, 100), 75);
        assert_eq!(context_remaining_percent(200, 100), 0);
        assert!(should_notify_response_finished(true, false));
        assert!(!should_notify_response_finished(true, true));
        assert!(!should_notify_response_finished(false, false));
    }
}
