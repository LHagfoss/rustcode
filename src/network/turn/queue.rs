use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, AppStatus, OrchestratorLease, StreamTracker};

use super::super::lifecycle;
use super::super::policy;
use super::super::stream::StreamBuffer;
use super::super::title::record_prompt_to_history;
use super::{
    run_agent_turn_with_context_for_session, save_turn_context_after_run,
    take_turn_context_for_prompt_with_limits,
};

fn configure_turn_steerability(
    state: &mut AppState,
    turn_session_id: &str,
    is_wakeup: bool,
    policy_supports_steering: bool,
) {
    state.active_turn_steerable_session =
        (policy_supports_steering && !is_wakeup && state.active_session_id == turn_session_id)
            .then(|| turn_session_id.to_owned());
}

fn promote_turn_end_steers(state: &mut AppState, turn_session_id: &str) -> usize {
    if state.active_session_id != turn_session_id {
        return 0;
    }
    let pending_before = state.pending_steers.len();
    state.promote_pending_steers_to_queue(turn_session_id);
    pending_before.saturating_sub(state.pending_steers.len())
}

fn enqueue_productive_continuation(state: &mut AppState, promoted_steer_count: usize) {
    let position = state
        .promoted_steer_prefix_count
        .max(promoted_steer_count)
        .min(state.pending_queue.len());
    state
        .pending_queue
        .insert(position, "__task_wakeup__:productive_segment".to_string());
}

fn take_turn_context_for_queued_prompt(
    state: &mut AppState,
    is_wakeup: bool,
    is_promoted_steer: bool,
    max_tool_rounds: usize,
    max_total_tool_rounds: usize,
) -> super::TurnContext {
    if is_promoted_steer {
        super::TurnContext::with_budgets(max_tool_rounds, max_total_tool_rounds)
    } else {
        take_turn_context_for_prompt_with_limits(
            state,
            is_wakeup,
            max_tool_rounds,
            max_total_tool_rounds,
        )
    }
}

fn save_turn_context_for_queued_prompt(
    state: &mut AppState,
    context: super::TurnContext,
    preserve_for_wakeup: bool,
    is_promoted_steer: bool,
) {
    if !is_promoted_steer {
        save_turn_context_after_run(state, context, preserve_for_wakeup);
    }
}

/// Releases an orchestrator lease when its task dies without reaching the
/// normal loop exit (panic, abort). The explicit release at the end of the
/// loop disarms the guard; a stale guard release is a no-op because the
/// lease generations no longer match.
struct OrchestratorLeaseGuard {
    state: Arc<Mutex<AppState>>,
    lease: Option<OrchestratorLease>,
}

impl OrchestratorLeaseGuard {
    fn disarm(&mut self) {
        self.lease = None;
    }
}

impl Drop for OrchestratorLeaseGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take()
            && let Ok(mut state) = self.state.try_lock()
        {
            state.release_orchestrator(&lease);
        }
    }
}

pub(crate) async fn process_queue_orchestrator<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
    lease: OrchestratorLease,
) {
    process_queue_orchestrator_inner(client, state, cancel_token, policy, None, lease).await;
}

pub(crate) async fn process_queue_orchestrator_with_ui_events<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
    ui_events: super::super::ui_adapter::AgentUiEventSender,
    lease: OrchestratorLease,
) {
    process_queue_orchestrator_inner(client, state, cancel_token, policy, Some(ui_events), lease)
        .await;
}

async fn process_queue_orchestrator_inner<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
    ui_events: Option<super::super::ui_adapter::AgentUiEventSender>,
    lease: OrchestratorLease,
) {
    dbg_log!("Orchestrator started");
    // Guard the spawn slot against task death (panic/abort) that skips the
    // release at the end of the loop. Without this, one dead task wedges
    // every future spawn until the watchdog invalidates the generation.
    let mut lease_guard = OrchestratorLeaseGuard {
        state: state.clone(),
        lease: Some(lease.clone()),
    };
    loop {
        let (next_prompt, is_wakeup, is_promoted_steer, turn_context, turn_session_id) = {
            let mut s = state.lock().await;
            // Cancellation owns the current turn until it reaches this
            // boundary. Do not dequeue the next item while the token is
            // cancelled: doing so loses queued user prompts when Esc races
            // with the end of the active stream.
            if cancel_token.is_cancelled() {
                s.release_orchestrator(&lease);
                break;
            }
            if s.pending_queue.is_empty() {
                dbg_log!("Pending queue empty, setting status to Idle");
                s.enter_idle();
                s.delegation_active = false;
                s.release_orchestrator(&lease);
                break;
            }
            s.status = AppStatus::Streaming;
            s.generation_start_time = Some(std::time::Instant::now());
            s.stream_tracker = Some(StreamTracker::new());
            s.recent_read_calls.clear();
            s.recent_read_outputs.clear();
            s.read_file_mtimes.clear();
            let prompt = s.pending_queue.remove(0);
            let is_promoted_steer = s.take_promoted_steer_prefix_prompt();
            // Background completions withheld during the previous turn join
            // history here, at a turn boundary, never mid-turn.
            crate::flush_pending_background_outputs(&mut s);
            s.last_turn_had_model_final_response = false;
            let is_wakeup = prompt.starts_with("__task_wakeup__:");
            let is_first_prompt = !is_wakeup && !crate::config::session_has_content(&s.history);
            s.session_title_tool_available = is_first_prompt;
            let max_tool_rounds = s.config.max_tool_rounds;
            let max_total_tool_rounds = s.config.max_total_tool_rounds;
            let turn_context = take_turn_context_for_queued_prompt(
                &mut s,
                is_wakeup,
                is_promoted_steer,
                max_tool_rounds,
                max_total_tool_rounds,
            );
            let turn_session_id = s.active_session_id.clone();
            configure_turn_steerability(
                &mut s,
                &turn_session_id,
                is_wakeup,
                policy.supports_live_turn_steering(),
            );
            dbg_log!("Popped prompt from queue: '{}'", prompt);
            (
                prompt,
                is_wakeup,
                is_promoted_steer,
                turn_context,
                turn_session_id,
            )
        };

        let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
        if !record_prompt_to_history(&state, is_wakeup, &next_prompt, &turn_session_id).await {
            let mut s = state.lock().await;
            super::clear_turn_steerability_for_session(&mut s, &turn_session_id);
            break;
        }
        crate::logger::operational_event(
            "turn.start",
            serde_json::json!({
                "session_id": turn_session_id,
                "wakeup": is_wakeup,
                "tool_rounds": turn_context.budget.tool_rounds,
                "segment_rounds": turn_context.segment_rounds(),
                "segment_limit": (turn_context.budget.max_tool_rounds != usize::MAX)
                    .then_some(turn_context.budget.max_tool_rounds),
                "total_round_limit": (turn_context.budget.max_total_tool_rounds != usize::MAX)
                    .then_some(turn_context.budget.max_total_tool_rounds),
            }),
        );

        let completed_context = if let Some(sender) = ui_events.clone() {
            super::super::ui_adapter::run_agent_turn_with_events_and_context_for_session(
                &client,
                &state,
                &cancel_token,
                &policy,
                &stream_buffer,
                next_prompt.clone(),
                sender,
                turn_context,
                turn_session_id.clone(),
            )
            .await
        } else {
            run_agent_turn_with_context_for_session(
                &client,
                &state,
                &cancel_token,
                &policy,
                &stream_buffer,
                turn_context,
                turn_session_id.clone(),
            )
            .await
        };

        let mut s = state.lock().await;
        if s.active_session_id != turn_session_id {
            // The user switched sessions while this cancelled turn was
            // unwinding. Do not carry its turn context into the replacement.
            super::clear_turn_steerability_for_session(&mut s, &turn_session_id);
            break;
        }
        let promoted_steer_count = promote_turn_end_steers(&mut s, &turn_session_id);
        let cancelled = cancel_token.is_cancelled();
        let schedule_continuation =
            !cancelled && !is_promoted_steer && completed_context.budget.continuation_pending;
        let preserve_for_wakeup = !cancelled
            && !is_promoted_steer
            && (is_wakeup
                || schedule_continuation
                || matches!(
                    completed_context.lifecycle.stop_reason,
                    Some(lifecycle::StopReason::BackgroundPending)
                ));
        save_turn_context_for_queued_prompt(
            &mut s,
            completed_context,
            preserve_for_wakeup,
            is_promoted_steer,
        );
        if schedule_continuation && s.background_turn_context.is_some() {
            enqueue_productive_continuation(&mut s, promoted_steer_count);
        }
        drop(s);

        if cancel_token.is_cancelled() {
            dbg_log!("Cancel token is cancelled, exiting orchestrator loop");
            break;
        }
    }
    {
        let s = state.lock().await;
        // Forensic context for the next stall investigation: a frozen turn
        // leaves status/queue evidence instead of a bare "finished" line.
        dbg_log!(
            "Orchestrator finished (queue={}, status={:?})",
            s.pending_queue.len(),
            s.status
        );
    }
    state.lock().await.release_orchestrator(&lease);
    lease_guard.disarm();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_end_fallback_puts_pending_steers_before_followups_once() {
        let mut state = AppState::new();
        let session_id = state.active_session_id.clone();
        state.status = AppStatus::Streaming;
        state.active_turn_steerable_session = Some(session_id.clone());
        assert!(state.queue_steer("First correction".to_owned()));
        assert!(state.queue_steer("Second correction".to_owned()));
        state.pending_queue = vec!["first follow-up".into(), "second follow-up".into()];

        // The turn finalizer clears steerability before the queue boundary.
        super::super::clear_turn_steerability_for_session(&mut state, &session_id);
        promote_turn_end_steers(&mut state, &session_id);
        promote_turn_end_steers(&mut state, &session_id);

        assert_eq!(
            state.pending_queue,
            [
                "First correction",
                "Second correction",
                "first follow-up",
                "second follow-up"
            ]
        );
        assert!(state.pending_steers.is_empty());
    }

    #[test]
    fn turn_end_fallback_does_not_leak_steers_after_session_replacement() {
        let mut state = AppState::new();
        let old_session_id = state.active_session_id.clone();
        state.status = AppStatus::Streaming;
        state.active_turn_steerable_session = Some(old_session_id.clone());
        assert!(state.queue_steer("Old session correction".to_owned()));
        state.pending_queue = vec!["replacement follow-up".into()];
        state.active_session_id = "replacement-session".into();

        promote_turn_end_steers(&mut state, &old_session_id);

        assert_eq!(state.pending_steers.len(), 1);
        assert_eq!(state.pending_queue, ["replacement follow-up"]);
    }

    #[test]
    fn productive_continuation_follows_promoted_steers_and_precedes_followups() {
        let mut state = AppState::new();
        let session_id = state.active_session_id.clone();
        state.status = AppStatus::Streaming;
        state.active_turn_steerable_session = Some(session_id.clone());
        assert!(state.queue_steer("Correct the implementation".to_owned()));
        assert!(state.queue_steer("Keep the tests focused".to_owned()));
        state.pending_queue = vec!["first follow-up".into(), "second follow-up".into()];

        let promoted = promote_turn_end_steers(&mut state, &session_id);
        enqueue_productive_continuation(&mut state, promoted);

        assert_eq!(
            state.pending_queue,
            [
                "Correct the implementation",
                "Keep the tests focused",
                "__task_wakeup__:productive_segment",
                "first follow-up",
                "second follow-up"
            ]
        );
    }

    #[test]
    fn productive_continuation_without_steers_stays_at_queue_head() {
        let mut state = AppState::new();
        state.pending_queue = vec!["first follow-up".into(), "second follow-up".into()];

        enqueue_productive_continuation(&mut state, 0);

        assert_eq!(
            state.pending_queue,
            [
                "__task_wakeup__:productive_segment",
                "first follow-up",
                "second follow-up"
            ]
        );
    }

    #[test]
    fn productive_segment_context_survives_promoted_corrections_until_its_wakeup() {
        let mut state = AppState::new();
        let session_id = state.active_session_id.clone();
        state.status = AppStatus::Streaming;
        state.active_turn_steerable_session = Some(session_id.clone());
        assert!(state.queue_steer("First correction".to_owned()));
        assert!(state.queue_steer("Second correction".to_owned()));
        state.pending_queue = vec!["first follow-up".into(), "second follow-up".into()];

        let mut original_context = crate::network::TurnContext::with_budgets(11, 90);
        original_context.budget.tool_rounds = 37;
        original_context.budget.segment_count = 4;
        original_context.budget.continuation_pending = true;
        state.background_turn_context = Some(Box::new(original_context));

        let promoted = promote_turn_end_steers(&mut state, &session_id);
        enqueue_productive_continuation(&mut state, promoted);
        assert_eq!(promoted, 2);

        for expected in ["First correction", "Second correction"] {
            assert_eq!(state.pending_queue.remove(0), expected);
            assert!(state.take_promoted_steer_prefix_prompt());
            let correction_context =
                take_turn_context_for_queued_prompt(&mut state, false, true, 99, 999);
            save_turn_context_for_queued_prompt(&mut state, correction_context, false, true);
            let saved = state
                .background_turn_context
                .as_ref()
                .expect("the original productive segment remains saved");
            assert_eq!(saved.budget.tool_rounds, 37);
            assert_eq!(saved.budget.max_tool_rounds, 11);
            assert_eq!(saved.budget.segment_count, 4);
            assert!(saved.budget.continuation_pending);
        }

        assert_eq!(
            state.pending_queue,
            [
                "__task_wakeup__:productive_segment",
                "first follow-up",
                "second follow-up"
            ]
        );
        assert_eq!(
            state.pending_queue.remove(0),
            "__task_wakeup__:productive_segment"
        );
        let resumed = take_turn_context_for_queued_prompt(&mut state, true, false, 11, 90);
        assert_eq!(resumed.budget.tool_rounds, 37);
        assert_eq!(resumed.budget.max_tool_rounds, 11);
        assert_eq!(resumed.budget.segment_count, 5);
        assert!(!resumed.budget.continuation_pending);
        assert_eq!(state.pending_queue, ["first follow-up", "second follow-up"]);
    }

    #[test]
    fn ordinary_interactive_prompt_is_marked_with_its_claimed_session() {
        let mut state = AppState::new();
        let session_id = state.active_session_id.clone();
        state.status = AppStatus::Streaming;

        configure_turn_steerability(&mut state, &session_id, false, true);

        assert_eq!(
            state.active_turn_steerable_session.as_deref(),
            Some(session_id.as_str())
        );
        assert!(state.can_accept_steer());
    }

    #[test]
    fn wakeups_and_policies_without_interactive_steering_do_not_mark_the_turn() {
        let mut state = AppState::new();
        let session_id = state.active_session_id.clone();

        configure_turn_steerability(&mut state, &session_id, true, true);
        assert_eq!(state.active_turn_steerable_session, None);

        configure_turn_steerability(&mut state, &session_id, false, false);
        assert_eq!(state.active_turn_steerable_session, None);
        assert!(!state.can_accept_steer());
    }

    #[test]
    fn question_and_confirmation_waits_block_then_restore_the_same_turn() {
        let mut state = AppState::new();
        let session_id = state.active_session_id.clone();
        state.status = AppStatus::Streaming;
        configure_turn_steerability(&mut state, &session_id, false, true);

        state.status = AppStatus::AwaitingToolConfirmation;
        assert!(!state.can_accept_steer());
        state.status = AppStatus::Streaming;
        assert!(state.can_accept_steer());

        state.status = AppStatus::AwaitingQuestion;
        assert!(!state.can_accept_steer());
        state.status = AppStatus::Streaming;
        assert!(state.can_accept_steer());
        assert_eq!(
            state.active_turn_steerable_session.as_deref(),
            Some(session_id.as_str())
        );
    }

    #[test]
    fn a_claimed_turn_from_another_session_cannot_mark_the_active_session() {
        let mut state = AppState::new();
        let claimed_session_id = "claimed-session";

        configure_turn_steerability(&mut state, claimed_session_id, false, true);

        assert_eq!(state.active_turn_steerable_session, None);
        assert!(!state.can_accept_steer());
    }

    #[test]
    fn lease_guard_releases_abandoned_claim_on_drop() {
        let state = Arc::new(Mutex::new(AppState::new()));
        let lease = state.blocking_lock().claim_orchestrator().expect("claim");
        assert!(state.blocking_lock().orchestrator_running);
        {
            let _guard = OrchestratorLeaseGuard {
                state: state.clone(),
                lease: Some(lease.clone()),
            };
            // Dropped without disarm: simulates panic/abort mid-loop.
        }
        assert!(!state.blocking_lock().orchestrator_running);

        // A disarmed guard after the explicit end-of-loop release changes
        // nothing.
        let lease = state.blocking_lock().claim_orchestrator().expect("reclaim");
        {
            let mut guard = OrchestratorLeaseGuard {
                state: state.clone(),
                lease: Some(lease.clone()),
            };
            assert!(state.blocking_lock().release_orchestrator(&lease));
            guard.disarm();
        }
        assert!(!state.blocking_lock().orchestrator_running);
    }
}
