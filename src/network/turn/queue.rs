use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, AppStatus, StreamTracker};

use super::super::lifecycle;
use super::super::policy;
use super::super::stream::StreamBuffer;
use super::super::title::record_prompt_to_history;
use super::{
    run_agent_turn_with_context, save_turn_context_after_run,
    take_turn_context_for_prompt_with_limits,
};

pub async fn process_queue_orchestrator<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
) {
    process_queue_orchestrator_inner(client, state, cancel_token, policy, None).await;
}

pub(crate) async fn process_queue_orchestrator_with_ui_events<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
    ui_events: super::super::ui_adapter::AgentUiEventSender,
) {
    process_queue_orchestrator_inner(client, state, cancel_token, policy, Some(ui_events)).await;
}

async fn process_queue_orchestrator_inner<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
    ui_events: Option<super::super::ui_adapter::AgentUiEventSender>,
) {
    dbg_log!("Orchestrator started");
    loop {
        let (next_prompt, is_wakeup, turn_context, turn_session_id) = {
            let mut s = state.lock().await;
            if s.pending_queue.is_empty() {
                dbg_log!("Pending queue empty, setting status to Idle");
                s.enter_idle();
                s.delegation_active = false;
                s.orchestrator_running = false;
                break;
            }
            s.status = AppStatus::Streaming;
            s.generation_start_time = Some(std::time::Instant::now());
            s.stream_tracker = Some(StreamTracker::new());
            s.recent_read_calls.clear();
            s.recent_read_outputs.clear();
            s.read_file_mtimes.clear();
            let prompt = s.pending_queue.remove(0);
            s.last_turn_had_model_final_response = false;
            let is_wakeup = prompt.starts_with("__task_wakeup__:");
            let is_first_prompt = !is_wakeup && !crate::config::session_has_content(&s.history);
            s.session_title_tool_available = is_first_prompt;
            let max_tool_rounds = s.config.max_tool_rounds;
            let max_total_tool_rounds = s.config.max_total_tool_rounds;
            let turn_context = take_turn_context_for_prompt_with_limits(
                &mut s,
                is_wakeup,
                max_tool_rounds,
                max_total_tool_rounds,
            );
            let turn_session_id = s.active_session_id.clone();
            dbg_log!("Popped prompt from queue: '{}'", prompt);
            (prompt, is_wakeup, turn_context, turn_session_id)
        };

        let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
        if !record_prompt_to_history(&state, is_wakeup, &next_prompt, &turn_session_id).await {
            break;
        }
        crate::logger::operational_event(
            "turn.start",
            serde_json::json!({
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
            super::super::ui_adapter::run_agent_turn_with_events_and_context(
                &client,
                &state,
                &cancel_token,
                &policy,
                &stream_buffer,
                next_prompt.clone(),
                sender,
                turn_context,
            )
            .await
        } else {
            run_agent_turn_with_context(
                &client,
                &state,
                &cancel_token,
                &policy,
                &stream_buffer,
                turn_context,
            )
            .await
        };

        let mut s = state.lock().await;
        if s.active_session_id != turn_session_id {
            // The user switched sessions while this cancelled turn was
            // unwinding. Do not carry its turn context into the replacement.
            break;
        }
        let schedule_continuation =
            !cancel_token.is_cancelled() && completed_context.budget.continuation_pending;
        let preserve_for_wakeup = is_wakeup
            || schedule_continuation
            || matches!(
                completed_context.lifecycle.stop_reason,
                Some(lifecycle::StopReason::BackgroundPending)
            );
        save_turn_context_after_run(&mut s, completed_context, preserve_for_wakeup);
        if schedule_continuation && s.background_turn_context.is_some() {
            s.pending_queue
                .insert(0, "__task_wakeup__:productive_segment".to_string());
        }
        drop(s);

        if cancel_token.is_cancelled() {
            dbg_log!("Cancel token is cancelled, exiting orchestrator loop");
            break;
        }
    }
    state.lock().await.orchestrator_running = false;
    dbg_log!("Orchestrator finished");
}
