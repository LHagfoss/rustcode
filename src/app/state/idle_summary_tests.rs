use super::{AppState, AppStatus, ChatMessage};
use std::time::{Duration, Instant};

#[test]
fn idle_summary_requires_a_quiet_session_with_new_history() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "old request"));
    state
        .history
        .push(ChatMessage::new("assistant", "old answer"));
    state.last_turn_had_model_final_response = true;
    state.idle_since = Instant::now() - Duration::from_secs(601);

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

    state.set_notice("YOLO mode enabled");
    state.idle_since = Instant::now() - Duration::from_secs(601);
    assert!(!state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));

    state.history.push(ChatMessage::new("user", "new request"));
    state.last_turn_had_model_final_response = false;
    state.idle_since = Instant::now() - Duration::from_secs(601);
    assert!(!state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));

    state
        .history
        .push(ChatMessage::new("assistant", "new answer"));
    state.last_turn_had_model_final_response = true;
    assert!(state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));
}

#[test]
fn idle_summary_waits_while_work_or_draft_is_present() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "request"));
    state.history.push(ChatMessage::new("assistant", "answer"));
    state.last_turn_had_model_final_response = true;
    state.idle_since = Instant::now() - Duration::from_secs(601);

    state.status = AppStatus::Streaming;
    assert!(!state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));

    state.status = AppStatus::Idle;
    state.input_buffer = "unfinished draft".to_owned();
    assert!(!state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));

    state.input_buffer.clear();
    assert!(!state.should_start_idle_summary(Instant::now(), true, Duration::from_secs(600),));
}

#[test]
fn idle_summary_requires_a_completed_model_response() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "request"));
    state
        .history
        .push(ChatMessage::new("tool", "tool output"));
    state.idle_since = Instant::now() - Duration::from_secs(601);

    assert!(!state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));
}

#[test]
fn idle_summary_uses_idle_entry_time_not_old_user_activity() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "request"));
    state.history.push(ChatMessage::new("assistant", "answer"));
    state.last_turn_had_model_final_response = true;
    state.idle_since = Instant::now();

    assert!(!state.should_start_idle_summary(Instant::now(), false, Duration::from_secs(600),));
}
