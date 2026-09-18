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
    assert!(healthy
        .check_stall_watchdog(false, Instant::now())
        .is_none());
}
