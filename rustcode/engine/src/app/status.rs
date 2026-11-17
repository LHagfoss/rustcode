use crate::app::state::{AppState, AppStatus};

pub fn should_notify_response_finished(
    response_just_finished: bool,
    terminal_focused: bool,
) -> bool {
    response_just_finished && !terminal_focused
}

pub struct StatusState<'a> {
    status: &'a AppStatus,
}

impl<'a> StatusState<'a> {
    pub fn new(state: &'a mut AppState) -> Self {
        Self {
            status: &state.status,
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
