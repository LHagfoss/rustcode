use crate::app::state::{AppState, AppStatus, LiveToolCall, StreamTracker, TokenUsage};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn should_notify_response_finished(
    response_just_finished: bool,
    terminal_focused: bool,
) -> bool {
    response_just_finished && !terminal_focused
}

#[allow(dead_code)]
pub struct StatusState<'a> {
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
    pub fn new(state: &'a mut AppState) -> Self {
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

    pub fn is_active(&self) -> bool {
        *self.status != AppStatus::Idle
    }
}

impl AppState {
    pub fn status_state(&mut self) -> StatusState<'_> {
        StatusState::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{StatusState, should_notify_response_finished};
    use crate::app::{AppState, AppStatus};

    #[test]
    fn status_view_reports_active_work() {
        let mut state = AppState::new();
        state.status = AppStatus::Streaming;

        let status = StatusState::new(&mut state);
        assert!(status.is_active());
    }

    #[test]
    fn notifies_response_finished_only_when_unfocused() {
        assert!(should_notify_response_finished(true, false));
        assert!(!should_notify_response_finished(true, true));
        assert!(!should_notify_response_finished(false, false));
    }
}
