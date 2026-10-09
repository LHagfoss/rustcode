use super::{AppState, AppStatus};
use std::time::{Duration, Instant};

fn stale_active_state() -> AppState {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time =
        Some(Instant::now() - Duration::from_secs(super::STALL_WATCHDOG_TIMEOUT_SECS + 60));
    state
}

#[test]
fn dead_turn_with_nothing_running_is_a_stall() {
    let state = stale_active_state();
    let recovery = state.check_stall_watchdog(false, Instant::now());
    assert!(recovery.is_some(), "silent 6-minute Streaming must trip");
}

#[test]
fn fresh_or_idle_work_is_not_a_stall() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time = Some(Instant::now());
    assert!(state.check_stall_watchdog(false, Instant::now()).is_none());

    let mut idle = AppState::new();
    idle.status = AppStatus::Idle;
    assert!(idle.check_stall_watchdog(false, Instant::now()).is_none());
}

#[test]
fn background_tasks_and_running_tools_suppress_the_watchdog() {
    let busy = stale_active_state();
    assert!(busy.check_stall_watchdog(true, Instant::now()).is_none());

    let mut tools = stale_active_state();
    tools.running_tools.push("run_command".to_string());
    assert!(tools.check_stall_watchdog(false, Instant::now()).is_none());
}

#[test]
fn stuck_orchestrator_flag_with_queued_prompts_resets() {
    let mut state = stale_active_state();
    state.status = AppStatus::Queued;
    state.pending_queue.push("user follow-up".to_string());
    state.orchestrator_running = true;
    let recovery = state
        .check_stall_watchdog(false, Instant::now())
        .expect("wedged queue must trip");
    assert!(recovery.reset_orchestrator);

    // Same queue with a live orchestrator is normal: the loop spawns.
    let mut healthy = stale_active_state();
    healthy.status = AppStatus::Queued;
    healthy.pending_queue.push("user follow-up".to_string());
    healthy.orchestrator_running = false;
    healthy.generation_start_time = Some(Instant::now());
    assert!(
        healthy
            .check_stall_watchdog(false, Instant::now())
            .is_none()
    );
}

#[test]
fn a_recent_stream_update_prevents_recovery_of_a_queued_follow_up() {
    let mut state = stale_active_state();
    state.status = AppStatus::Streaming;
    state.pending_queue.push("user follow-up".to_string());
    state.orchestrator_running = true;
    let mut tracker = super::super::StreamTracker::new();
    tracker.last_update = Instant::now();
    state.stream_tracker = Some(tracker);

    assert!(
        state.check_stall_watchdog(false, Instant::now()).is_none(),
        "an old turn start is not evidence of a stall while the stream recently progressed"
    );
}

#[test]
fn stale_orchestrator_release_cannot_clear_a_new_session_claim() {
    let mut state = AppState::new();
    let old = state.claim_orchestrator().expect("first claim");

    state.active_session_id = "replacement-session".to_owned();
    state.invalidate_orchestrator();
    let current = state.claim_orchestrator().expect("replacement claim");

    assert_ne!(old, current);
    assert!(!state.release_orchestrator(&old));
    assert!(state.orchestrator_running);
    assert_eq!(state.orchestrator_owner.as_ref(), Some(&current));
    assert!(state.release_orchestrator(&current));
    assert!(!state.orchestrator_running);
}

#[test]
fn watchdog_recovery_clears_the_complete_active_projection() {
    let mut state = stale_active_state();
    state.current_token_usage = Some(crate::app::TokenUsage {
        prompt_tokens: 10,
        completion_tokens: 2,
        total_tokens: 12,
        ..Default::default()
    });
    state.replace_current_response("partial response");
    state.begin_live_tool_call(None, "run_command", &serde_json::json!({}));
    // Keep the projection populated without marking a tool as actively
    // running; an actively running tool intentionally suppresses watchdog
    // recovery while it may still be making progress.
    state.running_tools.clear();
    let mut tracker = super::super::StreamTracker::new();
    tracker.last_update =
        Instant::now() - Duration::from_secs(super::STALL_WATCHDOG_TIMEOUT_SECS + 60);
    state.stream_tracker = Some(tracker);
    state.orchestrator_running = true;

    let recovery = state
        .check_stall_watchdog(false, Instant::now())
        .expect("stale active projection must trip");
    if recovery.reset_orchestrator {
        state.invalidate_orchestrator();
    }
    state.clear_active_turn_projection();

    assert!(state.current_response.is_empty());
    assert!(state.live_tool_calls.is_empty());
    assert!(state.running_tools.is_empty());
    assert!(state.stream_tracker.is_none());
    assert!(state.generation_start_time.is_none());
    assert!(state.current_token_usage.is_none());
    assert!(!state.orchestrator_running);
}

#[test]
fn orphaned_queue_with_no_owner_is_recovered_without_dropping_prompts() {
    // Invalidated (or never spawned) while prompts remained: the spawn slot
    // is free but nothing will ever drain the queue.
    let mut state = stale_active_state();
    state.status = AppStatus::Queued;
    state.pending_queue.push("user follow-up".to_string());
    state.orchestrator_running = false;
    let recovery = state
        .check_stall_watchdog(false, Instant::now())
        .expect("ownerless queue must trip");
    assert!(!recovery.reset_orchestrator);
    assert!(recovery.queue_preserved);
}

#[test]
fn fresh_submission_with_free_slot_is_not_a_stall() {
    // A just-submitted prompt (fresh generation, no loop yet) must reach the
    // spawn block instead of tripping the watchdog.
    let mut state = AppState::new();
    state.status = AppStatus::Queued;
    state.pending_queue.push("brand new".to_string());
    state.orchestrator_running = false;
    state.generation_start_time = Some(Instant::now());
    assert!(state.check_stall_watchdog(false, Instant::now()).is_none());
}

#[test]
fn queued_prompt_after_idle_has_no_stale_clocks_to_trip_on() {
    // Regression: turn-end left the previous turn's timestamps behind, so a
    // prompt submitted after >5min idle looked stalled the instant it was
    // queued (session 01a0fbf3). `enter_idle` must drop per-turn clocks.
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time =
        Some(Instant::now() - Duration::from_secs(super::STALL_WATCHDOG_TIMEOUT_SECS + 60));
    let mut tracker = super::super::StreamTracker::new();
    tracker.last_update =
        Instant::now() - Duration::from_secs(super::STALL_WATCHDOG_TIMEOUT_SECS + 60);
    state.stream_tracker = Some(tracker);
    state.enter_idle();
    assert!(state.generation_start_time.is_none());
    assert!(state.stream_tracker.is_none());

    // The resulting shape — queued prompt, free slot, no clocks — is idle
    // aftermath, not an orphaned queue.
    state.status = AppStatus::Queued;
    state
        .pending_queue
        .push("after ten idle minutes".to_string());
    state.orchestrator_running = false;
    assert!(state.check_stall_watchdog(false, Instant::now()).is_none());
}

fn past_the_limit() -> Duration {
    Duration::from_secs(super::STALL_WATCHDOG_TIMEOUT_SECS + 60)
}

#[tokio::test]
async fn answering_a_long_pending_question_is_not_a_stall() {
    // Session 01a11ffc: a question open for 17 minutes, then the watchdog
    // reset the live turn 16 ms after the answer (#1885).
    let mut state = stale_active_state();
    let mut tracker = super::super::StreamTracker::new();
    tracker.last_update = Instant::now() - past_the_limit();
    state.stream_tracker = Some(tracker);
    state.orchestrator_running = true;
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(state));

    let cancel = tokio_util::sync::CancellationToken::new();
    let question = tokio::spawn({
        let state = std::sync::Arc::clone(&state);
        async move {
            let args = serde_json::json!({ "question": "Continue?", "options": ["Yes", "No"] });
            crate::network::tool_exec::ask_user_question(&state, &cancel, &args).await
        }
    });
    let answer = loop {
        if let Some(answer) = state.lock().await.question_response.take() {
            break answer;
        }
        tokio::task::yield_now().await;
    };

    // Pending: no amount of waiting on the user is a stall.
    {
        let s = state.lock().await;
        assert!(s.check_stall_watchdog(false, Instant::now()).is_none());
        assert!(
            s.check_stall_watchdog(false, Instant::now() + past_the_limit())
                .is_none()
        );
    }

    answer.send("User selected: Yes".to_owned()).unwrap();
    let (output, _) = question.await.unwrap();
    assert!(output.success);

    // Answered: the turn is Streaming again with a turn start and a stream
    // older than the limit, and nothing running yet.
    let s = state.lock().await;
    assert_eq!(s.status, AppStatus::Streaming);
    assert!(s.pending_question.is_none());
    assert!(s.running_tools.is_empty());
    assert!(
        s.check_stall_watchdog(false, Instant::now()).is_none(),
        "the answer is progress; the turn must not be reset"
    );
    // #1226: a turn that then stays silent for the whole limit is still dead.
    assert!(
        s.check_stall_watchdog(false, Instant::now() + past_the_limit())
            .is_some()
    );
}

#[test]
fn a_pending_approval_is_not_a_stall() {
    let mut state = stale_active_state();
    state.pending_tool_confirmation = Some(Vec::new());
    assert!(state.check_stall_watchdog(false, Instant::now()).is_none());
}

#[test]
fn recent_turn_progress_suppresses_the_watchdog_until_it_goes_stale() {
    // A tool that ran past the limit and just finished: old turn start, old
    // stream, nothing running, next request not sent yet.
    let mut state = stale_active_state();
    state.orchestrator_running = true;
    state.note_turn_progress();
    assert!(state.check_stall_watchdog(false, Instant::now()).is_none());

    state.turn_progress_at = Some(Instant::now() - past_the_limit());
    let recovery = state
        .check_stall_watchdog(false, Instant::now())
        .expect("a turn silent past the limit after its last progress is dead");
    assert!(recovery.reset_orchestrator);
}
