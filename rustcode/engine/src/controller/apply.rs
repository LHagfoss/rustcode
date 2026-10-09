//! Turn-control decisions applied to session state.
//!
//! Approval/question answers, background-task event routing, and the observed
//! queue orchestrator. Used by the controller worker and the terminal event
//! loop alike; the loop itself lives in the frontend.

use crate::app::{AppState, AppStatus, ApprovalDecision, QuestionAnswer};
use crate::network::ui_adapter::AgentUiEventSender;
use rustcode_tasks::TaskEvent;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Keep Git discovery off the TUI's event/render loop. The AppState cache
/// deduplicates requests and rejects results after a session or cwd switch.
pub async fn refresh_workspace_location_async(state: &Arc<Mutex<AppState>>) {
    refresh_workspace_location_async_with(state, |cwd| {
        crate::app::workspace::WorkspaceLocation::detect(cwd)
    })
    .await;
}

async fn refresh_workspace_location_async_with<F>(state: &Arc<Mutex<AppState>>, detect: F)
where
    F: FnOnce(&std::path::Path) -> crate::app::workspace::WorkspaceLocation + Send + 'static,
{
    let request = state
        .lock()
        .await
        .claim_workspace_location_refresh(std::time::Instant::now());
    let Some(request) = request else {
        return;
    };

    let worker_state = Arc::clone(state);
    tokio::spawn(async move {
        let cwd = request.cwd.clone();
        let location = tokio::task::spawn_blocking(move || detect(&cwd)).await.ok();
        worker_state
            .lock()
            .await
            .complete_workspace_location_refresh(request, location);
    });
}

#[cfg(test)]
mod workspace_refresh_tests {
    use super::refresh_workspace_location_async_with;
    use crate::app::AppState;
    use crate::app::workspace::WorkspaceLocation;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    use tokio::sync::Mutex;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_git_lookup_does_not_hold_the_state_lock() {
        let state = Arc::new(Mutex::new(AppState::new()));
        // AppState starts with a debounce window; wait for it to become due.
        tokio::time::sleep(Duration::from_millis(510)).await;

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        refresh_workspace_location_async_with(&state, move |_| {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            WorkspaceLocation {
                path: "~/repo".to_owned(),
                branch: "main".to_owned(),
            }
        })
        .await;

        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .expect("blocking lookup started")
            .expect("blocking lookup worker sent its start signal");
        let guard = tokio::time::timeout(Duration::from_millis(100), state.lock())
            .await
            .expect("slow git lookup must not hold the app-state mutex");
        drop(guard);
        release_tx.send(()).expect("worker is still waiting");
    }
}

pub async fn spawn_observed_orchestrator(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    ui_events: AgentUiEventSender,
) -> bool {
    let lease = {
        let mut state = state.lock().await;
        if state.summary_in_flight || state.pending_queue.is_empty() {
            return false;
        }
        let Some(lease) = state.claim_orchestrator() else {
            return false;
        };
        state.status = AppStatus::Queued;
        lease
    };

    let handle = tokio::spawn(async move {
        crate::network::process_queue_orchestrator_with_ui_events(
            client,
            state,
            cancel_token,
            Arc::new(crate::network::policy::InteractivePolicy),
            ui_events,
            lease,
        )
        .await;
    });
    tokio::spawn(async move {
        match handle.await {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {
                crate::dbg_log!("Orchestrator task cancelled");
            }
            Err(error) => {
                crate::dbg_log!("Orchestrator task died: {error}");
                crate::logger::operational_event(
                    "orchestrator.task_died",
                    serde_json::json!({ "error": error.to_string() }),
                );
            }
        }
    });
    true
}

fn record_active_background_task(
    state: &mut crate::app::AppState,
    task_id: &str,
    output: crate::tools::ToolExecutionOutput,
    notify_on_complete: bool,
) -> bool {
    if state.background_wakeup_ids.contains(task_id) {
        return false;
    }
    if crate::tools::background_result_delivered_by_wait(&state.active_session_id, task_id) {
        // `manage_task` `wait` already returned this result to the model.
        state.background_wakeup_ids.insert(task_id.to_owned());
        state.request_redraw();
        return false;
    }
    if state.orchestrator_running {
        // A turn is in flight: withhold the output so it joins history at
        // the next turn boundary instead of derailing the current turn's
        // context mid-stream. The wakeup is still queued now.
        state
            .pending_background_outputs
            .push(crate::PendingBackgroundOutput {
                task_id: task_id.to_owned(),
                output,
            });
        if notify_on_complete {
            crate::queue_background_wakeup(state, task_id);
        } else {
            state.background_wakeup_ids.insert(task_id.to_owned());
            state.request_redraw();
        }
        return true;
    }
    state
        .history
        .push(crate::background_task_history_message(task_id, output));
    if notify_on_complete {
        crate::queue_background_wakeup(state, task_id);
    } else {
        state.background_wakeup_ids.insert(task_id.to_owned());
        state.request_redraw();
    }
    true
}

pub async fn apply_background_task_event(
    app_state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    event: TaskEvent,
) -> bool {
    let notify_on_complete = event.notify_on_complete().unwrap_or(true);
    let Some((task_id, session_id, output)) = crate::tools::task_event_to_tool_output(event) else {
        return false;
    };
    let mut state = app_state.lock().await;
    if state.active_session_id == session_id {
        if record_active_background_task(&mut state, &task_id, output, notify_on_complete) {
            crate::config::save_session_history(&session_id, &state.history);
            true
        } else {
            false
        }
    } else {
        let mut history = crate::config::load_session_history_direct(&session_id);
        history.push(crate::background_task_history_message(&task_id, output));
        crate::config::save_session_history(&session_id, &history);
        false
    }
}

pub async fn apply_approval_decision(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    decision: ApprovalDecision,
) {
    let (approved, remember_prefix) = match decision {
        ApprovalDecision::Approve => (true, None),
        ApprovalDecision::ApproveAll => {
            state.lock().await.auto_confirm = true;
            (true, None)
        }
        ApprovalDecision::ApproveAndRemember(prefix) => (true, Some((prefix, false))),
        ApprovalDecision::ForbidAndRemember(prefix) => (true, Some((prefix, true))),
        ApprovalDecision::Deny => (false, None),
        ApprovalDecision::Custom(reason) => (!reason.trim().is_empty(), None),
    };
    if !approved {
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
    }
    let mut state = state.lock().await;
    if let Some(tx) = state.tool_confirmation_response.take() {
        let response = if !approved {
            crate::app::ToolConfirmationResponse::Deny
        } else if let Some((prefix, forbid)) = remember_prefix {
            let valid_prefix = state
                .pending_tool_confirmation
                .as_ref()
                .filter(|items| {
                    items.len() == 1
                        && if forbid {
                            items[0].forbidden_prefix.is_some()
                        } else {
                            items[0].rememberable_prefix.is_some()
                        }
                })
                .and_then(|items| {
                    if forbid {
                        items[0].forbidden_prefix.clone()
                    } else {
                        items[0].rememberable_prefix.clone()
                    }
                })
                .filter(|actual| actual == &prefix);
            valid_prefix.map_or(crate::app::ToolConfirmationResponse::Approve, |prefix| {
                if forbid {
                    crate::app::ToolConfirmationResponse::ForbidAndRemember(prefix)
                } else {
                    crate::app::ToolConfirmationResponse::ApproveAndRemember(prefix)
                }
            })
        } else {
            crate::app::ToolConfirmationResponse::Approve
        };
        let _ = tx.send(response);
    }
    state.pending_tool_confirmation = None;
    state.pending_approval_details = None;
    state.pending_approval_batch_id = None;
    state.request_redraw();
}

/// Resolves a native approval only while the controller-issued identity still
/// names the batch currently held by the policy. The comparison and channel
/// take happen under the same state lock so a delayed callback cannot resolve
/// a replacement batch.
pub async fn apply_approval_decision_for_batch(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    expected_batch_id: &str,
    decision: ApprovalDecision,
) -> bool {
    let mut state = state.lock().await;
    if state.pending_tool_confirmation.is_none()
        || state.tool_confirmation_response.is_none()
        || state.pending_approval_batch_id.as_deref() != Some(expected_batch_id)
    {
        return false;
    }

    let approved = match decision {
        ApprovalDecision::Approve => true,
        ApprovalDecision::Deny => false,
        _ => return false,
    };
    if !approved {
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
    }
    if let Some(tx) = state.tool_confirmation_response.take() {
        let response = if approved {
            crate::app::ToolConfirmationResponse::Approve
        } else {
            crate::app::ToolConfirmationResponse::Deny
        };
        let _ = tx.send(response);
    }
    state.pending_tool_confirmation = None;
    state.pending_approval_details = None;
    state.pending_approval_batch_id = None;
    state.request_redraw();
    true
}

pub async fn apply_question_answer(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    answer: QuestionAnswer,
) {
    if matches!(answer, QuestionAnswer::Cancelled) {
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
        let mut state = state.lock().await;
        if let Some(tx) = state.question_response.take() {
            let _ = tx.send("User cancelled prompt.".to_owned());
        }
        state.clear_question_chain();
        state.enter_idle();
        state.request_redraw();
        return;
    }
    let current = match answer {
        QuestionAnswer::Selected(answer) | QuestionAnswer::Custom(answer) => answer,
        QuestionAnswer::Cancelled => unreachable!("cancelled handled above"),
    };
    let mut state = state.lock().await;
    record_question_answer(&mut state, current);
}

/// Record `current` for the active question: advance the chain, or resolve
/// the tool call once the last question is answered.
fn record_question_answer(state: &mut AppState, current: String) {
    if !state.pending_question_queue.is_empty() {
        // More questions remain in the chain: record this answer, advance to
        // the next question, and keep waiting — the tool call resolves only
        // once every question is answered (or the chain is cancelled).
        state.advance_question_chain(current);
        state.request_redraw();
        return;
    }
    let answers = state.take_question_chain_answers(Some(current.clone()));
    // A stale event with no active chain (e.g. a double Enter racing the
    // submit) falls back to the raw answer instead of an empty submission.
    let output = if answers.is_empty() {
        format!("User selected: {current}")
    } else {
        crate::app::state::format_question_chain_answers(&answers)
    };
    if let Some(tx) = state.question_response.take() {
        let _ = tx.send(output);
    }
    state.request_redraw();
}

/// A typed answer to one identified question. Unlike [`QuestionAnswer`] it
/// carries the chosen options separately, so they can be checked against the
/// question they were chosen from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionReply {
    /// One or more of the question's own options, verbatim.
    Options(Vec<String>),
    /// Free text, for the always-present "write your own answer" slot.
    Custom(String),
}

/// Why an identity-bound answer was not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionRejection {
    /// The named question is no longer the pending one (answered, cancelled
    /// or replaced).
    Stale,
    /// The question is still pending but the answer does not fit it.
    Invalid(String),
}

/// Answers a question only while `expected_question_id` still names the one
/// that is pending. The identity check, the option validation and the
/// response-channel take happen under the same state lock, so of two racing
/// answers exactly one applies and the other sees [`QuestionRejection::Stale`].
pub async fn apply_question_answer_for_question(
    state: &Arc<Mutex<AppState>>,
    expected_question_id: &str,
    reply: QuestionReply,
) -> Result<(), QuestionRejection> {
    let mut state = state.lock().await;
    let Some(question) = state
        .pending_question
        .as_ref()
        .filter(|question| question.id == expected_question_id)
    else {
        return Err(QuestionRejection::Stale);
    };
    if state.question_response.is_none() {
        return Err(QuestionRejection::Stale);
    }
    let current = match reply {
        QuestionReply::Options(selected) => {
            if selected.is_empty() {
                return Err(QuestionRejection::Invalid(
                    "no option was selected".to_owned(),
                ));
            }
            if selected.len() > 1 && !question.is_multi_select {
                return Err(QuestionRejection::Invalid(
                    "this question accepts exactly one option".to_owned(),
                ));
            }
            for (index, option) in selected.iter().enumerate() {
                if !question.options.contains(option) {
                    return Err(QuestionRejection::Invalid(format!(
                        "`{option}` is not an option of this question"
                    )));
                }
                if selected[..index].contains(option) {
                    return Err(QuestionRejection::Invalid(format!(
                        "`{option}` was selected more than once"
                    )));
                }
            }
            // Same rendering as the terminal modal's multi-select answer.
            selected.join(", ")
        }
        QuestionReply::Custom(text) => {
            let text = text.trim();
            if text.is_empty() {
                return Err(QuestionRejection::Invalid(
                    "a custom answer cannot be empty".to_owned(),
                ));
            }
            text.to_owned()
        }
    };
    record_question_answer(&mut state, current);
    Ok(())
}

/// Requests cancellation only while `expected_turn_id` still names the turn
/// the orchestrator is running. The comparison and the token cancel happen
/// under the state lock that assigns turn identities, so a cancel written for
/// a finished turn can never stop the one that started after it. The caller
/// then runs its normal cancel cleanup (joining the turn, replacing the token).
pub async fn cancel_turn_for_turn(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &CancellationToken,
    expected_turn_id: &str,
) -> bool {
    let mut state = state.lock().await;
    if state.active_turn_id.as_deref() != Some(expected_turn_id) {
        return false;
    }
    state.active_turn_id = None;
    cancel_token.cancel();
    true
}
#[cfg(test)]
mod tests {
    use super::record_active_background_task;
    use crate::app::AppState;
    use crate::tools::ToolExecutionOutput;

    #[test]
    fn active_completion_is_recorded_and_queued_once() {
        let mut state = AppState::new();
        state.active_session_id = "active-completion-session".to_owned();
        let output = ToolExecutionOutput::success("cargo test passed".to_owned());

        assert!(record_active_background_task(
            &mut state,
            "task-completed-once",
            output.clone(),
            true
        ));
        assert!(!record_active_background_task(
            &mut state,
            "task-completed-once",
            output,
            true
        ));
        assert_eq!(state.history.len(), 1);
        assert_eq!(
            state.pending_queue,
            vec!["__task_wakeup__:task-completed-once".to_owned()]
        );
        assert!(state.background_wakeup_ids.contains("task-completed-once"));
    }

    #[test]
    fn silent_completion_is_recorded_without_queuing_a_turn() {
        let mut state = AppState::new();
        state.active_session_id = "silent-completion-session".to_owned();
        let output = ToolExecutionOutput::success("[command status: exit_code=0]".to_owned());

        assert!(record_active_background_task(
            &mut state,
            "silent-task",
            output.clone(),
            false,
        ));
        assert!(!record_active_background_task(
            &mut state,
            "silent-task",
            output,
            false,
        ));
        assert_eq!(state.history.len(), 1);
        assert!(state.pending_queue.is_empty());
        assert!(state.background_wakeup_ids.contains("silent-task"));
    }

    #[test]
    fn completion_during_active_turn_is_withheld_until_boundary() {
        let mut state = AppState::new();
        state.active_session_id = "withheld-session".to_owned();
        state.orchestrator_running = true;
        let output = ToolExecutionOutput::success("done".to_owned());

        assert!(record_active_background_task(
            &mut state,
            "task-withheld",
            output.clone(),
            true
        ));
        assert!(!record_active_background_task(
            &mut state,
            "task-withheld",
            output,
            true
        ));
        assert_eq!(state.history.len(), 0);
        assert_eq!(state.pending_background_outputs.len(), 1);
        assert_eq!(
            state.pending_queue,
            vec!["__task_wakeup__:task-withheld".to_owned()]
        );

        state.orchestrator_running = false;
        assert_eq!(crate::flush_pending_background_outputs(&mut state), 1);
        assert_eq!(state.history.len(), 1);
        assert!(state.history[0].content.contains("done"));
        assert!(state.pending_background_outputs.is_empty());
        assert_eq!(crate::flush_pending_background_outputs(&mut state), 0);
    }

    #[test]
    fn result_read_through_wait_is_not_delivered_again() {
        let mut state = AppState::new();
        state.active_session_id = "wait-read-session".to_owned();
        state.orchestrator_running = true;
        let output = ToolExecutionOutput::success("done".to_owned());

        // Parked before `wait` returned it, then parked for a task the model
        // never waited on.
        assert!(record_active_background_task(
            &mut state,
            "task-read",
            output.clone(),
            true
        ));
        assert!(record_active_background_task(
            &mut state,
            "task-unread",
            output.clone(),
            true
        ));
        crate::tools::exec::note_wait_delivered("wait-read-session", "task-read");

        // The next request keeps only the wakeup whose result is still withheld.
        assert_eq!(state.consume_answered_background_wakeups(), 0);
        assert_eq!(
            state.pending_queue,
            vec!["__task_wakeup__:task-unread".to_owned()]
        );
        assert_eq!(state.pending_background_outputs.len(), 1);
        assert_eq!(state.pending_background_outputs[0].task_id, "task-unread");

        // A completion that arrives after `wait` returned it is not parked.
        crate::tools::exec::note_wait_delivered("wait-read-session", "task-late");
        assert!(!record_active_background_task(
            &mut state,
            "task-late",
            output,
            true
        ));
        assert_eq!(state.pending_background_outputs.len(), 1);

        assert_eq!(crate::flush_pending_background_outputs(&mut state), 1);
        assert!(state.pending_background_outputs.is_empty());
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.consume_answered_background_wakeups(), 1);
        assert!(state.pending_queue.is_empty());
    }

    #[test]
    fn session_subscription_retains_inactive_completion_until_consumed() {
        let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
        let subscription = manager.subscribe_session("inactive-session");
        let task = manager
            .spawn_with_id(
                "inactive-completion-task",
                rustcode_tasks::TaskSpec::new(
                    "inactive-session",
                    rustcode_command::CommandRequest {
                        command: if cfg!(target_os = "windows") {
                            "echo retained".to_owned()
                        } else {
                            "printf retained".to_owned()
                        },
                        status_command: None,
                        sandboxed_shell: false,
                        cwd: None,
                        env: Vec::new(),
                        timeout: std::time::Duration::from_secs(5),
                        process_group: true,
                        inherited_fds: Vec::new(),
                    },
                ),
            )
            .expect("spawn inactive task");

        let mut saw_finished = false;
        while let Ok(event) = subscription.recv() {
            if event.task_id() == task.id() && event.is_terminal() {
                saw_finished = true;
                break;
            }
        }
        assert!(saw_finished, "inactive session completion was retained");
    }
}
