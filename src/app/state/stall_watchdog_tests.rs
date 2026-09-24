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
