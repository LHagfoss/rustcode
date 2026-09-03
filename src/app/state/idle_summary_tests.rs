use super::{AppState, AppStatus, ChatMessage};
use std::time::{Duration, Instant};

#[test]
fn idle_summary_requires_a_quiet_session_with_new_history() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "old request"));
    state
        .history
        .push(ChatMessage::new("assistant", "old answer"));
    state.last_user_activity_at = Instant::now() - Duration::from_secs(601);

    assert!(state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));

    assert!(state.claim_summary());
    assert!(!state.should_start_idle_summary(
        Instant::now() + Duration::from_secs(1),
        false,
        Duration::from_secs(600),
    ));

    state.finish_summary();
    assert!(!state.should_start_idle_summary(
        Instant::now() + Duration::from_secs(601),
        false,
        Duration::from_secs(600),
    ));

    state.history.push(ChatMessage::new("user", "new request"));
    state.last_user_activity_at = Instant::now() - Duration::from_secs(601);
    assert!(state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));
}

#[test]
fn idle_summary_waits_while_work_or_draft_is_present() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "request"));
    state.history.push(ChatMessage::new("assistant", "answer"));
    state.last_user_activity_at = Instant::now() - Duration::from_secs(601);

    state.status = AppStatus::Streaming;
    assert!(!state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));

    state.status = AppStatus::Idle;
    state.input_buffer = "unfinished draft".to_owned();
    assert!(!state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));

    state.input_buffer.clear();
    assert!(!state.should_start_idle_summary(Instant::now(), true, Duration::from_secs(600),));
}
