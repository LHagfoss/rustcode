#[test]
fn a_validation_rejected_call_is_reported_only_by_its_tool_result() {
    let describe = |indices: &[usize]| format!("{indices:?}");
    // Session 01a11ffc (#1886): two calls, the edit ran, the write was
    // rejected by validation and answered by its own tool result.
    assert_eq!(
        super::unexecuted_calls_notice(2, 1, &[], &[], &[], |_| true, describe),
        None
    );

    let notice = super::unexecuted_calls_notice(3, 1, &[], &[2], &[2], |_| true, describe)
        .expect("an unscheduled call still needs the notice");
    assert!(
        notice.contains("did not schedule 1 call(s) ([2])"),
        "{notice}"
    );
    assert!(notice.contains("The remaining 1 call(s) ([2])"), "{notice}");
}

// Session 01a130ea (#1970): an unannotated MCP lookup held over the limit
// was told it exceeded the workspace-change limit.
#[test]
fn a_held_call_that_is_not_a_known_mutation_does_not_claim_a_workspace_change() {
    let names = ["solidtime_whoami", "write_to_file", "run_command"];
    let describe = |indices: &[usize]| {
        indices
            .iter()
            .map(|index| names[*index])
            .collect::<Vec<_>>()
            .join(", ")
    };
    let known = |index: usize| super::is_known_mutation_call(names[index]);

    let notice = super::unexecuted_calls_notice(3, 1, &[0], &[], &[], known, describe)
        .expect("a held call needs the notice");
    assert!(!notice.contains("workspace-change"), "{notice}");
    assert!(notice.contains("may have side effects"), "{notice}");
    assert!(notice.contains("(solidtime_whoami)"), "{notice}");

    let notice = super::unexecuted_calls_notice(3, 1, &[0, 1, 2], &[], &[], known, describe)
        .expect("held calls need the notice");
    assert!(
        notice.contains("held 2 over the per-response workspace-change limit"),
        "{notice}"
    );
    assert!(notice.contains("(write_to_file, run_command)"), "{notice}");
    assert!(
        notice.contains("held 1 call(s) that may have side effects"),
        "{notice}"
    );
}

use super::super::{GroundedArtifactEvidence, TurnContext};
use super::{
    BatchOutcomeCounts, REVERIFICATION_ACTION, append_finalized_tool_batch, append_pending_steers,
    apply_round_stagnation, apply_stale_round, batch_invalidates_read_recovery,
    benign_shell_wrapper_failure, bounded_malformed_tool_history,
    content_bearing_inspection_status, grounded_artifact_recovery_message, incomplete_tool_result,
    loop_signal_event, mutation_batch_guidance, record_round_output, scheduled_tool_calls,
    selected_tool_call_indices, should_apply_loop_recovery, take_deferred_batch,
    targeted_no_progress_guidance, tool_changes_workspace,
};
use crate::app::{AppState, AppStatus, ChatMessage, ToolCallRef, ToolResultRecord};
use crate::network::events::ToolResultMetadata;
use crate::network::loop_detect;
use crate::network::policy::TurnPolicy;
use crate::network::{call_refs_for, unanswered_call_results_with_kind};
use crate::tools::ToolCall;
use rustcode_core::{InspectionRange, InspectionResultMetadata, ToolResultCompleteness};
use std::sync::Arc;
use tokio::sync::Mutex;

#[test]
fn readonly_subagent_spawn_does_not_count_as_workspace_edit() {
    assert!(!tool_changes_workspace(
        "spawn_agent",
        Some(&serde_json::json!({"write_access": false}))
    ));
    assert!(!tool_changes_workspace(
        "spawn_agent",
        Some(&serde_json::json!({}))
    ));
    assert!(tool_changes_workspace(
        "spawn_agent",
        Some(&serde_json::json!({"write_access": true}))
    ));
    // A role that presets write access counts the same as asking for it.
    assert!(tool_changes_workspace(
        "spawn_agent",
        Some(&serde_json::json!({"agent_type": "worker"}))
    ));
    assert!(!tool_changes_workspace(
        "spawn_agent",
        Some(&serde_json::json!({"agent_type": "explorer"}))
    ));
    assert!(tool_changes_workspace("write_to_file", None));
}

struct CancelAfterApproval(tokio_util::sync::CancellationToken);

impl TurnPolicy for CancelAfterApproval {
    fn should_approve(
        &self,
        _state: &Arc<Mutex<AppState>>,
        _tool_calls: &[ToolCall],
    ) -> impl std::future::Future<Output = bool> + Send {
        let cancel_token = self.0.clone();
        async move {
            cancel_token.cancel();
            true
        }
    }

    fn should_verify_completion(&self) -> bool {
        false
    }
}

struct ApproveAll;

impl TurnPolicy for ApproveAll {
    fn should_approve(
        &self,
        _state: &Arc<Mutex<AppState>>,
        _tool_calls: &[ToolCall],
    ) -> impl std::future::Future<Output = bool> + Send {
        async { true }
    }

    fn should_verify_completion(&self) -> bool {
        false
    }
}

fn write_call(path: &std::path::Path, content: &str) -> ToolCall {
    ToolCall {
        name: "write_to_file".to_string(),
        arguments: serde_json::json!({"path": path, "content": content}),
        call_id: None,
    }
}

fn inspection_metadata(complete: bool) -> InspectionResultMetadata {
    InspectionResultMetadata {
        requested_path: Some("src/app.ts".to_string()),
        requested_range: Some(InspectionRange {
            start: Some(1),
            end: Some(20),
        }),
        returned_path: Some("src/app.ts".to_string()),
        returned_range: Some(InspectionRange {
            start: Some(1),
            end: Some(20),
        }),
        complete,
        fingerprint: "read:src/app.ts:1:20".to_string(),
        ..Default::default()
    }
}

fn read_call(command: &str) -> ToolCall {
    ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": command}),
        call_id: None,
    }
}

fn steerable_state() -> (AppState, String) {
    let mut state = AppState::new();
    let session_id = state.active_session_id.clone();
    state.status = AppStatus::Streaming;
    state.active_turn_steerable_session = Some(session_id.clone());
    assert!(state.queue_steer("Use Teams".to_owned()));
    assert!(state.queue_steer("Keep the same channel".to_owned()));
    (state, session_id)
}

fn paired_tool_result(call_id: &str, error_kind: Option<&str>) -> ChatMessage {
    ChatMessage::new("tool", format!("result for {call_id}"))
        .answering(Some(call_id.to_owned()))
        .with_tool_result(ToolResultRecord {
            workspace_generation: None,
            workspace_epoch: None,
            evidence_hash: None,
            tool_name: "get_time".to_owned(),
            success: error_kind.is_none(),
            error_kind: error_kind.map(str::to_owned),
            ..Default::default()
        })
}

#[test]
fn batch_outcomes_count_background_starts_as_pending() {
    let result = |success: bool, pending: bool| crate::network::events::ToolResult {
        tool_name: "run_command".to_owned(),
        content: String::new(),
        diff: None,
        file_preview: None,
        metadata: crate::network::events::ToolResultMetadata {
            success,
            pending,
            ..Default::default()
        },
    };
    // A notifying background start reports `success: false, pending: true`.
    let results = vec![
        result(true, false),
        result(false, true),
        result(false, false),
    ];

    assert_eq!(
        BatchOutcomeCounts::of(&results),
        BatchOutcomeCounts {
            successes: 1,
            failed: 1,
            pending: 1,
        }
    );
}

#[test]
fn delivered_steer_keeps_its_structured_origin_in_history() {
    let (mut state, session_id) = steerable_state();
    assert!(state.queue_steer("review this".to_owned()));
    let origin = serde_json::json!({"kind": "agent", "threads": [{"sessionId": "sender"}]});
    state.pending_steers.last_mut().unwrap().origin = Some(origin.clone());
    append_pending_steers(&mut state, &session_id);
    assert_eq!(state.history.last().unwrap().origin.as_ref(), Some(&origin));
}

#[test]
fn finalized_batch_appends_steers_after_all_ordered_tool_results() {
    let (mut state, session_id) = steerable_state();
    state.history.push(
        ChatMessage::new("assistant", "checking channels").with_tool_calls(
            ["call-completed", "call-cancelled", "call-deferred"]
                .into_iter()
                .map(|id| ToolCallRef {
                    id: id.to_owned(),
                    name: "get_time".to_owned(),
                    arguments: "{}".to_owned(),
                })
                .collect(),
        ),
    );
    let results = vec![
        paired_tool_result("call-completed", None),
        paired_tool_result("call-cancelled", Some("cancelled")),
        paired_tool_result("call-deferred", Some("deferred")),
    ];

    append_finalized_tool_batch(&mut state, &session_id, results);

    let history = state.history.as_slice();
    let call_ids = history
        .iter()
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(
        call_ids,
        ["call-completed", "call-cancelled", "call-deferred"]
    );
    for call_id in ["call-completed", "call-cancelled", "call-deferred"] {
        assert_eq!(
            history
                .iter()
                .filter(|message| message.tool_call_id.as_deref() == Some(call_id))
                .count(),
            1,
            "each native call must have exactly one result: {call_id}"
        );
    }
    let result_positions = history
        .iter()
        .enumerate()
        .filter(|(_, message)| message.tool_result.is_some())
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let steer_positions = history
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.role == "user"
                && ["Use Teams", "Keep the same channel"].contains(&message.content.as_str())
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(steer_positions.len(), 2);
    assert!(result_positions.iter().max().unwrap() < &steer_positions[0]);
    assert!(steer_positions[0] < steer_positions[1]);
    assert!(state.pending_steers.is_empty());
}

#[test]
fn batch_handoff_and_queue_promotion_are_single_winner_transitions() {
    let (mut esc_first, session_id) = steerable_state();
    esc_first.promote_pending_steers_to_queue(&session_id);
    append_finalized_tool_batch(
        &mut esc_first,
        &session_id,
        vec![paired_tool_result("call-1", None)],
    );
    assert_eq!(
        esc_first.pending_queue,
        ["Use Teams", "Keep the same channel"]
    );
    assert!(!esc_first.history.iter().any(|message| {
        message.role == "user"
            && ["Use Teams", "Keep the same channel"].contains(&message.content.as_str())
    }));

    let (mut batch_first, session_id) = steerable_state();
    append_finalized_tool_batch(
        &mut batch_first,
        &session_id,
        vec![paired_tool_result("call-1", None)],
    );
    batch_first.promote_pending_steers_to_queue(&session_id);
    assert_eq!(batch_first.pending_queue.len(), 0);
    assert_eq!(
        batch_first
            .history
            .iter()
            .filter(|message| {
                message.role == "user"
                    && ["Use Teams", "Keep the same channel"].contains(&message.content.as_str())
            })
            .count(),
        2
    );
}

#[tokio::test]
async fn esc_and_finalized_batch_race_through_the_state_mutex_exactly_once() {
    async fn run_race(esc_first: bool) {
        let (mut app_state, session_id) = steerable_state();
        app_state.pending_queue = vec!["existing follow-up".to_owned()];
        let state = Arc::new(Mutex::new(app_state));

        // Hold the state lock while enqueueing both contenders. Tokio's
        // mutex is FIFO, so the first task is guaranteed to own the next
        // transition once this guard is released.
        let guard = state.lock().await;
        let (esc_started_tx, esc_started_rx) = tokio::sync::oneshot::channel();
        let (batch_started_tx, batch_started_rx) = tokio::sync::oneshot::channel();
        let (esc_go_tx, esc_go_rx) = tokio::sync::oneshot::channel();
        let (batch_go_tx, batch_go_rx) = tokio::sync::oneshot::channel();
        let esc_state = state.clone();
        let esc_task = tokio::spawn(async move {
            let mut cancel_token = tokio_util::sync::CancellationToken::new();
            let cancelled_token = cancel_token.clone();
            let _ = esc_go_rx.await;
            let _ = esc_started_tx.send(());
            crate::app::actions::handle_escape(&esc_state, &mut cancel_token).await;
            cancelled_token
        });
        let batch_state = state.clone();
        let batch_task = tokio::spawn(async move {
            let _ = batch_go_rx.await;
            let _ = batch_started_tx.send(());
            let mut app_state = batch_state.lock().await;
            append_finalized_tool_batch(
                &mut app_state,
                &session_id,
                vec![paired_tool_result("call-race", None)],
            );
        });

        if esc_first {
            let _ = esc_go_tx.send(());
            esc_started_rx.await.expect("Esc task starts");
            tokio::task::yield_now().await;
            let _ = batch_go_tx.send(());
            batch_started_rx.await.expect("batch task starts");
            tokio::task::yield_now().await;
        } else {
            let _ = batch_go_tx.send(());
            batch_started_rx.await.expect("batch task starts");
            tokio::task::yield_now().await;
            let _ = esc_go_tx.send(());
            esc_started_rx.await.expect("Esc task starts");
            tokio::task::yield_now().await;
        }
        drop(guard);
        let cancel_token = esc_task.await.expect("Esc task completes");
        batch_task.await.expect("batch task completes");
        assert!(cancel_token.is_cancelled());

        let state = state.lock().await;
        let queued_steers = state
            .pending_queue
            .iter()
            .filter(|text| ["Use Teams", "Keep the same channel"].contains(&text.as_str()))
            .map(String::as_str)
            .collect::<Vec<_>>();
        let historical_steers = state
            .history
            .iter()
            .filter(|message| {
                message.role == "user"
                    && ["Use Teams", "Keep the same channel"].contains(&message.content.as_str())
            })
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert!(state.pending_steers.is_empty());
        assert_eq!(
            [queued_steers.as_slice(), historical_steers.as_slice()].concat(),
            ["Use Teams", "Keep the same channel"]
        );
        assert_eq!(
            queued_steers.len() + historical_steers.len(),
            2,
            "each accepted steer must be owned by exactly one transition"
        );
        assert_eq!(
            state
                .pending_queue
                .iter()
                .filter(|text| text.as_str() == "existing follow-up")
                .count(),
            1
        );
        if esc_first {
            assert_eq!(queued_steers, ["Use Teams", "Keep the same channel"]);
            assert!(historical_steers.is_empty());
        } else {
            assert!(queued_steers.is_empty());
            assert_eq!(historical_steers, ["Use Teams", "Keep the same channel"]);
            let last_result = state
                .history
                .iter()
                .rposition(|message| message.tool_result.is_some())
                .expect("the finalized tool result appears");
            let first_steer = state
                .history
                .iter()
                .position(|message| message.content == "Use Teams")
                .expect("first steer appears");
            assert!(
                last_result < first_steer,
                "the finalized result must precede the handed-off steers"
            );
        }
    }

    run_race(true).await;
    run_race(false).await;
}

#[tokio::test]
async fn cancelled_native_batch_closes_calls_before_steers_and_persists_them() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let mut app_state = AppState::new();
    let session_id = app_state.active_session_id.clone();
    app_state
        .history
        .push(ChatMessage::new("user", "Start the task"));
    app_state.status = AppStatus::Streaming;
    app_state.active_turn_steerable_session = Some(session_id.clone());
    assert!(app_state.queue_steer("Use Teams".to_owned()));
    assert!(app_state.queue_steer("Keep the same channel".to_owned()));
    one_mutation_per_response(&mut app_state);
    app_state.auto_confirm = true;
    let api_base_url = app_state.api_base_url.clone();
    app_state.record_function_calling_support(&api_base_url, true);

    let state = Arc::new(Mutex::new(app_state));
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let policy = Arc::new(CancelAfterApproval(cancel_token.clone()));
    let mut ctx = TurnContext::new();
    ctx.response.final_content = "I will make a bounded update.".to_owned();
    // Seed an earlier round's measurement so the assertions below prove the
    // cancelled round does not present it as its own (#1592).
    ctx.progress.last_reason = Some(loop_detect::ProgressReason::NewInformation);
    ctx.metrics.no_progress_results = 4;
    let calls = ["call-first", "call-second", "call-third"]
        .into_iter()
        .enumerate()
        .map(|(index, call_id)| crate::tools::ToolCallEnvelope {
            call_id: call_id.to_owned(),
            tool_name: "write_to_file".to_owned(),
            arguments: serde_json::json!({
                "path": temp.path().join(format!("output-{index}.txt")),
                "content": "must not be written after cancellation",
            }),
        })
        .collect();

    let outcome = super::handle_tool_response(
        &reqwest::Client::new(),
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("tool_calls"),
        0,
        None,
        None,
        None,
        calls,
        &session_id,
    )
    .await;

    assert_eq!(outcome, super::ToolHandlingOutcome::Stop);
    let history = state.lock().await.history.as_slice().to_vec();
    assert!(cancel_token.is_cancelled());
    let tool_results = history
        .iter()
        .filter(|message| message.tool_result.is_some())
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 3, "all requested calls are closed");
    assert_eq!(
        tool_results
            .iter()
            .map(|message| message.tool_call_id.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["call-first", "call-second", "call-third"]
    );
    assert!(tool_results.iter().all(|message| {
        message
            .tool_result
            .as_ref()
            .is_some_and(|result| result.error_kind.as_deref() == Some("Cancelled"))
    }));
    // #1590: the calls the mutation budget could not take are held by the
    // harness, not dropped. Cancellation stops the turn before they can
    // run, so Esc/turn end is what releases them.
    assert_eq!(state.lock().await.deferred_tool_calls.len(), 2);
    let last_result = history
        .iter()
        .rposition(|message| message.tool_result.is_some())
        .expect("cancelled and unselected results");
    let steer_positions = history
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.role == "user"
                && ["Use Teams", "Keep the same channel"].contains(&message.content.as_str())
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(steer_positions.len(), 2);
    assert!(last_result < steer_positions[0]);
    assert!(steer_positions[0] < steer_positions[1]);
    assert!(state.lock().await.pending_steers.is_empty());
    // #1592: the cancelled turn reports what it cancelled, never a stale
    // progress reason carried over from an earlier round of the same turn.
    assert_eq!(
        ctx.lifecycle.stop_reason,
        Some(crate::network::lifecycle::StopReason::Cancelled)
    );
    assert_eq!(ctx.progress.last_reason, None);
    let summary = ctx.benchmark_summary();
    assert_eq!(summary["last_progress_reason"], serde_json::Value::Null);
    assert_eq!(summary["cancelled_tool_calls"], 1);
    assert_eq!(
        summary["no_progress_results"], 4,
        "cancelled results are counted separately, never judged for progress"
    );

    crate::sessions::flush_history();
    let persisted = crate::sessions::load_session_history_direct(&session_id);
    let persisted_result_ids = persisted
        .iter()
        .filter(|message| message.tool_result.is_some())
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(
        persisted_result_ids,
        ["call-first", "call-second", "call-third"],
        "saved history closes every native call"
    );
    let persisted_last_result = persisted
        .iter()
        .rposition(|message| message.tool_result.is_some())
        .expect("persisted results");
    let persisted_steer_positions = persisted
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.role == "user"
                && ["Use Teams", "Keep the same channel"].contains(&message.content.as_str())
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(persisted_steer_positions.len(), 2);
    assert!(persisted_last_result < persisted_steer_positions[0]);
    assert!(persisted_steer_positions[0] < persisted_steer_positions[1]);
    assert!(temp.path().read_dir().unwrap().next().is_none());
}

/// Restrict every profile to one workspace-changing call per response,
/// the policy the queue tests exercise.
fn one_mutation_per_response(app_state: &mut AppState) {
    for profile in &mut app_state.config.models {
        profile.allow_tool_batching = Some(false);
    }
}

fn approving_state() -> (Arc<Mutex<AppState>>, String) {
    let mut app_state = AppState::new();
    one_mutation_per_response(&mut app_state);
    let session_id = app_state.active_session_id.clone();
    app_state
        .history
        .push(ChatMessage::new("user", "Apply both edits"));
    app_state.status = AppStatus::Streaming;
    app_state.auto_confirm = true;
    let api_base_url = app_state.api_base_url.clone();
    app_state.record_function_calling_support(&api_base_url, true);
    (Arc::new(Mutex::new(app_state)), session_id)
}

fn envelope(call_id: &str, call: &ToolCall) -> crate::tools::ToolCallEnvelope {
    crate::tools::ToolCallEnvelope {
        call_id: call_id.to_owned(),
        tool_name: call.name.clone(),
        arguments: call.arguments.clone(),
    }
}

async fn run_round(
    state: &Arc<Mutex<AppState>>,
    policy: &Arc<ApproveAll>,
    ctx: &mut TurnContext,
    content: &str,
    calls: Vec<crate::tools::ToolCallEnvelope>,
    session_id: &str,
) -> super::ToolHandlingOutcome {
    ctx.response.final_content = content.to_owned();
    ctx.response.streamed_call_ids = calls.iter().map(|call| call.call_id.clone()).collect();
    super::handle_tool_response(
        &reqwest::Client::new(),
        state,
        &tokio_util::sync::CancellationToken::new(),
        policy,
        ctx,
        Some("tool_calls"),
        0,
        None,
        None,
        None,
        calls,
        session_id,
    )
    .await
}

// #1590: an over-budget call is queued and answered, not dropped and
// bounced back to the model. The harness runs it on a later round.
#[tokio::test]
async fn over_budget_calls_are_queued_and_executed_without_a_model_reissue() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let (state, session_id) = approving_state();
    let policy = Arc::new(ApproveAll);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut ctx = TurnContext::new();
    let first = write_call(&temp.path().join("first.txt"), "one");
    let second = write_call(&temp.path().join("second.txt"), "two");

    let outcome = super::handle_tool_response(
        &reqwest::Client::new(),
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("tool_calls"),
        0,
        None,
        None,
        None,
        vec![
            envelope("call-first", &first),
            envelope("call-second", &second),
        ],
        &session_id,
    )
    .await;

    assert_eq!(outcome, super::ToolHandlingOutcome::Continue);
    assert_eq!(state.lock().await.deferred_tool_calls.len(), 1);
    assert_eq!(
        state.lock().await.deferred_tool_calls[0].arguments["content"],
        "two",
        "the over-budget call is held intact"
    );
    let round_one = state.lock().await.history.as_slice().to_vec();
    let deferred_result = round_one
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-second"))
        .expect("the queued call is still answered");
    assert!(
        deferred_result.content.contains("do not reissue"),
        "{}",
        deferred_result.content
    );
    let notice = round_one
        .iter()
        .find(|message| message.role == "system" && message.content.contains("queued"))
        .expect("the scheduling notice");
    assert!(
        notice.content.contains("do not reissue"),
        "{}",
        notice.content
    );
    assert!(
        !notice.content.contains("Reissue deferred calls"),
        "the model must not be told to reissue held calls"
    );
    assert!(temp.path().join("first.txt").exists());
    assert!(
        !temp.path().join("second.txt").exists(),
        "the queued call must not run in the same round as its budget peer"
    );

    // Second round: no reissue, one new call. The harness runs the queued
    // call alongside it under its own budget.
    let third = ToolCall {
        name: "get_time".to_string(),
        arguments: serde_json::json!({}),
        call_id: None,
    };
    let outcome = run_round(
        &state,
        &policy,
        &mut ctx,
        "checking the clock",
        vec![envelope("call-third", &third)],
        &session_id,
    )
    .await;

    assert_eq!(outcome, super::ToolHandlingOutcome::Continue);
    assert!(
        state.lock().await.deferred_tool_calls.is_empty(),
        "the queue drains once its call runs"
    );
    let history = state.lock().await.history.as_slice().to_vec();
    let auto_result = history
        .iter()
        .find(|message| {
            message.role == "tool"
                && message.tool_call_id.is_none()
                && message.content.starts_with("write_to_file:")
        })
        .expect("the harness-authored result for the queued call");
    assert!(
        auto_result
            .tool_result
            .as_ref()
            .is_some_and(|record| record.success),
        "{}",
        auto_result.content
    );
    assert!(
        temp.path().join("second.txt").exists(),
        "the queued call ran without the model reissuing it"
    );
    // The queued call already owns an answer under its provider id, so the
    // auto-executed result must not claim that id again (#1590 pairing).
    assert_eq!(ctx.metrics.tool_calls, 3, "two writes and one clock read");
}

// Several workspace-changing calls in one response run in order under the
// default profile; nothing is held for a later round.
#[tokio::test]
async fn default_profile_runs_several_mutations_from_one_response_in_order() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let mut app_state = AppState::new();
    let session_id = app_state.active_session_id.clone();
    app_state
        .history
        .push(ChatMessage::new("user", "Apply both edits"));
    app_state.status = AppStatus::Streaming;
    app_state.auto_confirm = true;
    let api_base_url = app_state.api_base_url.clone();
    app_state.record_function_calling_support(&api_base_url, true);
    let state = Arc::new(Mutex::new(app_state));
    let policy = Arc::new(ApproveAll);
    let mut ctx = TurnContext::new();
    let first = write_call(&temp.path().join("first.txt"), "one");
    let second = write_call(&temp.path().join("second.txt"), "two");

    run_round(
        &state,
        &policy,
        &mut ctx,
        "",
        vec![
            envelope("call-first", &first),
            envelope("call-second", &second),
        ],
        &session_id,
    )
    .await;

    assert!(state.lock().await.deferred_tool_calls.is_empty());
    assert!(temp.path().join("first.txt").exists());
    assert!(temp.path().join("second.txt").exists());
}

// #1857: the model may answer in text right after being told its held
// calls will run. The turn must run them instead of dropping them.
#[tokio::test]
async fn held_calls_run_when_the_model_answers_without_another_call() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let (state, session_id) = approving_state();
    let policy = Arc::new(ApproveAll);
    let mut ctx = TurnContext::new();
    let first = write_call(&temp.path().join("first.txt"), "one");
    let second = write_call(&temp.path().join("second.txt"), "two");

    run_round(
        &state,
        &policy,
        &mut ctx,
        "",
        vec![
            envelope("call-first", &first),
            envelope("call-second", &second),
        ],
        &session_id,
    )
    .await;
    assert_eq!(state.lock().await.deferred_tool_calls.len(), 1);

    let outcome = run_round(
        &state,
        &policy,
        &mut ctx,
        "Both files are written.",
        Vec::new(),
        &session_id,
    )
    .await;

    assert_eq!(outcome, super::ToolHandlingOutcome::Continue);
    assert!(state.lock().await.deferred_tool_calls.is_empty());
    assert!(
        temp.path().join("second.txt").exists(),
        "the held call ran although the model asked for nothing more"
    );
}

#[tokio::test]
async fn a_turn_that_stops_releases_the_calls_it_was_holding() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let (state, _session_id) = approving_state();
    let held = || write_call(&temp.path().join("held.txt"), "never written");

    state.lock().await.deferred_tool_calls.push(held());
    let mut guard = state.lock().await;
    super::super::take_turn_context_for_prompt(&mut guard, false, 40);
    drop(guard);
    assert!(
        state.lock().await.deferred_tool_calls.is_empty(),
        "a new user prompt releases the previous task's held calls"
    );

    state.lock().await.deferred_tool_calls.push(held());
    crate::app::actions::handle_escape(&state, &mut tokio_util::sync::CancellationToken::new())
        .await;
    assert!(
        state.lock().await.deferred_tool_calls.is_empty(),
        "Esc must not leave calls for the next turn to run"
    );
    assert!(!temp.path().join("held.txt").exists());
}

// The drain is a strict prefix bounded by a fresh mutation allowance: a
// non-empty queue always shrinks, so it can never re-enter the round's
// budget or spin (#1590).
#[test]
fn draining_the_queue_shrinks_it_by_a_bounded_prefix() {
    let policy = crate::config::ToolSchedulingPolicy::default();
    let queued = vec![
        write_call(std::path::Path::new("a"), "a"),
        write_call(std::path::Path::new("b"), "b"),
        write_call(std::path::Path::new("c"), "c"),
    ];

    let (batch, prefix) = take_deferred_batch(&queued, policy, &[]);

    assert_eq!(batch.len(), 1, "one mutation per round");
    assert_eq!(prefix, 1);
    assert!(prefix < queued.len(), "the queue keeps making progress");

    let total = queued.len();
    let mut remaining = queued;
    let mut drains = 0;
    while !remaining.is_empty() {
        let (batch, prefix) = take_deferred_batch(&remaining, policy, &[]);
        assert!(!batch.is_empty(), "a non-empty queue always drains");
        remaining.drain(..prefix);
        drains += 1;
        assert!(drains <= total, "the drain terminates");
    }
    assert_eq!(drains, total);
}

#[test]
fn draining_never_reruns_a_call_the_model_already_reissued() {
    let policy = crate::config::ToolSchedulingPolicy::default();
    let queued = vec![
        write_call(std::path::Path::new("a"), "a"),
        read_call("git status"),
    ];

    let (batch, prefix) = take_deferred_batch(
        &queued,
        policy,
        &[write_call(std::path::Path::new("a"), "a")],
    );

    assert_eq!(prefix, 2, "the reissued call is covered by this round");
    assert_eq!(
        batch.len(),
        1,
        "only the call the model did not reissue is executed again"
    );
    assert_eq!(batch[0].name, "run_command");
}

#[test]
fn over_budget_calls_are_reported_as_queued_not_deferred_for_reissue() {
    let calls = vec![
        write_call(std::path::Path::new("a"), "a"),
        write_call(std::path::Path::new("b"), "b"),
        read_call("git status"),
    ];

    let scheduled = scheduled_tool_calls(&calls, &[None, None, None], Default::default());

    assert_eq!(scheduled.selected, vec![0, 2]);
    assert_eq!(scheduled.queued, vec![1]);
    assert!(scheduled.reissue.is_empty());
}

// A completion request is not workspace work and must stay with the model:
// queueing it would park the end of the turn behind a queued edit and, with
// the "do not reissue" notice, leave the task unable to finish (#1590).
#[test]
fn a_completion_request_over_budget_is_left_to_the_model() {
    let calls = vec![
        write_call(std::path::Path::new("a"), "a"),
        ToolCall {
            name: "complete_task".to_string(),
            arguments: serde_json::json!({"result": "done"}),
            call_id: None,
        },
    ];

    let scheduled = scheduled_tool_calls(&calls, &[None, None], Default::default());

    assert_eq!(scheduled.selected, vec![0]);
    assert!(scheduled.queued.is_empty());
    assert_eq!(scheduled.reissue, vec![1]);
}

#[test]
fn completion_request_reaches_finish_gates_before_loop_recovery() {
    assert!(!should_apply_loop_recovery(true, true, false, true));
    assert!(!should_apply_loop_recovery(true, false, false, true));
}

#[test]
fn ordinary_tool_rounds_still_apply_loop_recovery() {
    assert!(should_apply_loop_recovery(false, true, false, false));
    assert!(should_apply_loop_recovery(false, false, false, true));
    assert!(!should_apply_loop_recovery(false, false, false, false));
}

#[test]
fn output_abort_does_not_recover_after_meaningful_progress() {
    assert!(!should_apply_loop_recovery(false, true, true, false));
}

#[test]
fn independent_recovery_still_fires_after_meaningful_progress() {
    assert!(should_apply_loop_recovery(false, true, true, true));
}

#[test]
fn output_abort_still_recovers_without_meaningful_progress() {
    assert!(should_apply_loop_recovery(false, true, false, false));
}

#[test]
fn mixed_batch_progress_suppresses_output_only_recovery_in_both_orders() {
    for batch in [
        [(true, false), (false, true)],
        [(false, true), (true, false)],
    ] {
        let mut output_abort = false;
        let mut round_had_meaningful = false;
        for (is_abort, is_meaningful) in batch {
            output_abort |= is_abort;
            round_had_meaningful |= is_meaningful;
        }
        assert!(!should_apply_loop_recovery(
            false,
            output_abort,
            round_had_meaningful,
            false,
        ));
    }
}

#[test]
fn loop_signal_event_preserves_the_raw_abort_and_suppression() {
    let event = loop_signal_event(
        Some(&loop_detect::LoopStatus::Abort(4)),
        Some(true),
        None,
        false,
        Some("same_round_meaningful_progress"),
        false,
    );
    assert_eq!(event["evidence_recovery"], serde_json::Value::Null);
    assert_eq!(event["output_stagnation"]["status"], "abort");
    assert_eq!(event["output_stagnation"]["repeats"], 4);
    assert_eq!(event["round_had_meaningful"], true);
    assert_eq!(event["recovery_taken"], false);
    assert_eq!(
        event["recovery_suppressed_reason"],
        "same_round_meaningful_progress"
    );
}

#[test]
fn loop_signal_event_records_all_repeated_recovery() {
    let event = loop_signal_event(
        Some(&loop_detect::LoopStatus::Abort(4)),
        Some(false),
        None,
        true,
        None,
        false,
    );
    assert_eq!(event["output_stagnation"]["status"], "abort");
    assert_eq!(event["output_stagnation"]["repeats"], 4);
    assert_eq!(event["round_had_meaningful"], false);
    assert_eq!(event["evidence_recovery"], serde_json::Value::Null);
    assert_eq!(event["recovery_taken"], true);
    assert_eq!(event["recovery_suppressed_reason"], serde_json::Value::Null);
}

#[test]
fn loop_signal_event_reports_cancelled_batch_as_not_evaluated() {
    let event = loop_signal_event(None, None, None, false, None, true);

    assert_eq!(event["output_stagnation"]["status"], "not_evaluated");
    assert_eq!(
        event["output_stagnation"]["repeats"],
        serde_json::Value::Null
    );
    assert_eq!(event["round_had_meaningful"], serde_json::Value::Null);
    assert_eq!(event["evidence_recovery"], serde_json::Value::Null);
    assert_eq!(event["recovery_taken"], false);
    assert_eq!(event["recovery_suppressed_reason"], serde_json::Value::Null);
    assert_eq!(event["cancelled"], true);
}

#[test]
fn loop_signal_event_keeps_independent_churn_recovery_separate() {
    let event = loop_signal_event(
        Some(&loop_detect::LoopStatus::Abort(4)),
        Some(true),
        Some(loop_detect::ProgressReason::Churn),
        true,
        None,
        false,
    );
    assert_eq!(event["output_stagnation"]["status"], "abort");
    assert_eq!(event["output_stagnation"]["repeats"], 4);
    assert_eq!(event["round_had_meaningful"], true);
    assert_eq!(event["evidence_recovery"], "edit_test_revert_churn");
    assert_eq!(event["recovery_taken"], true);
    assert_eq!(event["recovery_suppressed_reason"], serde_json::Value::Null);
}

#[test]
fn mixed_progress_batch_suppresses_and_reports_output_only_recovery() {
    let stagnation = loop_detect::LoopStatus::Abort(4);
    let recovery_taken = should_apply_loop_recovery(false, true, true, false);
    let event = loop_signal_event(
        Some(&stagnation),
        Some(true),
        None,
        recovery_taken,
        Some("same_round_meaningful_progress"),
        false,
    );

    assert!(!recovery_taken);
    assert_eq!(event["output_stagnation"]["status"], "abort");
    assert_eq!(event["round_had_meaningful"], true);
    assert_eq!(event["recovery_taken"], false);
    assert_eq!(
        event["recovery_suppressed_reason"],
        "same_round_meaningful_progress"
    );
}

#[test]
fn no_progress_batch_keeps_and_reports_output_only_recovery() {
    let stagnation = loop_detect::LoopStatus::Abort(4);
    let recovery_taken = should_apply_loop_recovery(false, true, false, false);
    let event = loop_signal_event(
        Some(&stagnation),
        Some(false),
        None,
        recovery_taken,
        None,
        false,
    );

    assert!(recovery_taken);
    assert_eq!(event["output_stagnation"]["status"], "abort");
    assert_eq!(event["round_had_meaningful"], false);
    assert_eq!(event["recovery_taken"], true);
    assert_eq!(event["recovery_suppressed_reason"], serde_json::Value::Null);
}

#[test]
fn grounded_artifact_recovery_is_bounded_and_targeted() {
    let evidence = GroundedArtifactEvidence {
        path: "src/index.html".to_string(),
        write_lines: Some(699),
        write_bytes: Some(23_785),
        read_range: Some((1, 699)),
        repair_attempts: 0,
    };
    let message = grounded_artifact_recovery_message(
        &evidence,
        &format!("error: {}", "x".repeat(10_000)),
        false,
    );

    assert!(message.contains("src/index.html"));
    assert!(message.contains("699 lines / 23785 bytes"));
    assert!(message.contains("complete read of lines 1-699"));
    assert!(message.contains("last repair attempt made no change"));
    assert!(message.contains("exactly one narrow replace_file_content"));
    assert!(message.contains("Do not reread the whole file"));
    assert!(message.len() < 2_000, "recovery notice was not bounded");
}

#[test]
fn single_call_guidance_preserves_mutation_policy() {
    for limit in [1, 3] {
        let guidance = mutation_batch_guidance(&crate::config::ToolSchedulingPolicy {
            max_mutating_calls: limit,
            ..Default::default()
        });
        assert!(guidance.contains("Batch independent read-only calls freely"));
        assert!(guidance.contains(&format!(
            "Up to {limit} workspace-changing calls per response run in the order given"
        )));
        assert!(guidance.contains("Read-only inspection never consumes the mutation budget"));
    }
}

#[test]
fn identical_outputs_in_one_round_count_once_toward_the_loop_warning() {
    let mut detector = loop_detect::LoopDetector::new(6);
    // Four different `gh issue list --search` calls in one parallel
    // batch, each correctly returning `[]`.
    let mut round = std::collections::HashSet::new();
    for _ in 0..4 {
        assert_eq!(
            record_round_output(&mut detector, &mut round, "[]"),
            loop_detect::LoopStatus::Ok
        );
    }
    // The same empty answer over successive rounds still warns.
    let mut status = loop_detect::LoopStatus::Ok;
    for _ in 0..2 {
        let mut round = std::collections::HashSet::new();
        status = record_round_output(&mut detector, &mut round, "[]");
    }
    assert_eq!(status, loop_detect::LoopStatus::Warning(3));
}

#[test]
fn round_stagnation_counts_once_per_batch_and_clears_on_progress() {
    assert_eq!(apply_round_stagnation(2, false, 6), 3);
    assert_eq!(apply_round_stagnation(2, true, 5), 0);
    assert_eq!(apply_round_stagnation(2, false, 0), 2);
    assert_eq!(apply_round_stagnation(usize::MAX, false, 3), usize::MAX);
}

#[test]
fn mostly_stale_rounds_arm_reverification_recovery_despite_one_fresh_read() {
    // Session 01a14e34cc01 turn 3, rounds 26-28 and 35-37: each round
    // re-read 2-3 unchanged files plus one fresh range, with no edits.
    // `apply_round_stagnation` clears on that single fresh read, so the
    // turn burned its token budget with zero recoveries (#2114).
    let streak = crate::config::DEFAULT_EVIDENCE_RECOVERY_STREAK;
    let mut stale = 0;
    let mut no_progress = 0;
    for (hits, assessed) in [(3, 4), (3, 4), (3, 4)] {
        stale = apply_stale_round(stale, hits, assessed, false);
        no_progress = apply_round_stagnation(no_progress, true, hits);
    }
    assert_eq!(no_progress, 0, "the existing round guard never trips");
    assert!(stale >= streak, "three stale rounds arm recovery");

    // An edit, a fresh verification, or a round that is mostly new
    // evidence restarts the count.
    assert_eq!(apply_stale_round(2, 3, 4, true), 0);
    assert_eq!(apply_stale_round(2, 1, 5, false), 0);
    assert_eq!(apply_stale_round(2, 0, 4, false), 0);
    assert_eq!(apply_stale_round(2, 2, 4, false), 3);
}

#[test]
fn failing_discovery_guidance_points_at_the_error() {
    let guidance = targeted_no_progress_guidance(
        loop_detect::ProgressReason::RepeatedFailure,
        3,
        "list_mcp_tools",
    );
    assert!(guidance.contains("no tools were listed"), "{guidance}");
    assert!(!guidance.contains("already-known tools"), "{guidance}");
    let guidance = targeted_no_progress_guidance(
        loop_detect::ProgressReason::NoNewInformation,
        3,
        "list_mcp_tools",
    );
    assert!(guidance.contains("already-known tools"), "{guidance}");
}

#[test]
fn reverification_guidance_asks_to_finish_instead_of_rereading() {
    let guidance = targeted_no_progress_guidance(
        loop_detect::ProgressReason::NoNewInformation,
        3,
        REVERIFICATION_ACTION,
    );
    assert!(guidance.contains("3 consecutive rounds"), "{guidance}");
    assert!(guidance.contains("Stop re-verifying"), "{guidance}");
    assert!(guidance.contains("final summary"), "{guidance}");
}

#[test]
fn default_scheduler_batches_all_valid_reads() {
    let calls = vec![read_call("git status"), read_call("git diff")];
    let errors = vec![None, None];
    let selected = selected_tool_call_indices(
        &calls,
        &errors,
        crate::config::ToolSchedulingPolicy::default(),
    );
    assert_eq!(selected, vec![0, 1]);
}

#[test]
fn default_scheduler_caps_mutations_at_one() {
    let calls = vec![
        ToolCall {
            name: "write_to_file".to_string(),
            arguments: serde_json::json!({"path": "a", "content": "a"}),
            call_id: None,
        },
        ToolCall {
            name: "write_to_file".to_string(),
            arguments: serde_json::json!({"path": "b", "content": "b"}),
            call_id: None,
        },
        read_call("git status"),
    ];
    let errors = vec![None, None, None];
    let selected = selected_tool_call_indices(
        &calls,
        &errors,
        crate::config::ToolSchedulingPolicy::default(),
    );
    assert_eq!(selected, vec![0, 2]);
}

#[test]
fn scheduler_deferred_mutations_are_not_retryable_failures() {
    let calls = vec![
        ToolCall {
            name: "write_to_file".to_string(),
            arguments: serde_json::json!({"path": "a", "content": "a"}),
            call_id: None,
        },
        ToolCall {
            name: "write_to_file".to_string(),
            arguments: serde_json::json!({"path": "b", "content": "b"}),
            call_id: None,
        },
    ];
    let selected = selected_tool_call_indices(
        &calls,
        &[None, None],
        crate::config::ToolSchedulingPolicy::default(),
    );
    let refs = call_refs_for(&calls, &["call_a".into(), "call_b".into()]);
    let deferred_refs: Vec<_> = refs
        .iter()
        .enumerate()
        .filter(|(index, _)| !selected.contains(index))
        .map(|(_, call)| call.clone())
        .collect();
    let results = unanswered_call_results_with_kind(
        &deferred_refs,
        "intentionally deferred by the scheduler",
        crate::tools::ToolErrorKind::Deferred,
    );

    assert_eq!(selected, vec![0]);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().parsed_error_kind(),
        Some(crate::tools::ToolErrorKind::Deferred)
    );
    assert!(!results[0].tool_result.as_ref().unwrap().retryable);
}

#[test]
fn trusted_scheduler_applies_mutation_limit_while_reads_batch_freely() {
    let calls = vec![
        read_call("git status"),
        read_call("git diff"),
        ToolCall {
            name: "write_to_file".to_string(),
            arguments: serde_json::json!({"path": "a", "content": "a"}),
            call_id: None,
        },
        ToolCall {
            name: "write_to_file".to_string(),
            arguments: serde_json::json!({"path": "b", "content": "b"}),
            call_id: None,
        },
    ];
    let errors = vec![None, None, None, None];
    let selected = selected_tool_call_indices(
        &calls,
        &errors,
        crate::config::ToolSchedulingPolicy {
            allow_batching: true,
            max_read_only_calls: 1,
            max_mutating_calls: 1,
            ..Default::default()
        },
    );
    assert_eq!(selected, vec![0, 1, 2]);
}

#[test]
fn scheduler_runs_control_first_with_reads_following() {
    let calls = vec![
        read_call("git status"),
        ToolCall {
            name: "use_skill".to_string(),
            arguments: serde_json::json!({"name": "rust"}),
            call_id: None,
        },
        read_call("git diff"),
        ToolCall {
            name: "write_to_file".to_string(),
            arguments: serde_json::json!({"path": "a", "content": "a"}),
            call_id: None,
        },
    ];
    let errors = vec![None, None, None, None];
    let scheduled = scheduled_tool_calls(
        &calls,
        &errors,
        crate::config::ToolSchedulingPolicy {
            allow_batching: true,
            ..Default::default()
        },
    );
    // Control first for ordering; valid reads ride along in the same
    // round (issue #1230). The mutating companion still waits — for the
    // harness, not the model (#1590).
    assert_eq!(scheduled.selected, vec![1, 0, 2]);
    assert_eq!(scheduled.queued, vec![3]);
}

#[test]
fn read_only_inspection_stays_executable_while_loop_recovery_is_pending() {
    // #984 must not weaken #983: read-only classification is evaluated
    // before the mutating cap in every batch path, including recovery, so
    // inspection is never dropped or reprimanded for budget reasons while
    // the harness nudges the model back on track.
    use crate::tools::{is_read_only_call, partition_tool_batch, validate_tool_calls};

    let shell = |command: &str| ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": command}),
        call_id: None,
    };
    let batch = vec![
        ToolCall {
            name: "replace_file_content".to_string(),
            arguments: serde_json::json!({
                "path": "src/a.ts",
                "edits": [{"old_string": "a", "new_string": "b"}]
            }),
            call_id: None,
        },
        shell("git status --short"),
        shell("ls src"),
        shell("cat src/app.ts"),
    ];
    assert!(!is_read_only_call(&batch[0]));
    assert!(batch.iter().skip(1).all(is_read_only_call));
    let limit = crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE;
    let (kept, dropped) = partition_tool_batch(batch, limit);
    assert!(
        dropped.is_empty(),
        "recovery must not drop inspection: {dropped:?}"
    );
    assert_eq!(kept.len(), 4);
    assert!(validate_tool_calls(&kept, limit).is_ok());
}

fn read_observation(action: &str) -> super::loop_detect::ProgressObservation {
    super::loop_detect::ProgressObservation {
        action: action.to_owned(),
        output_fingerprint: super::loop_detect::stable_hash(action),
        state_fingerprint: None,
        failure_fingerprint: None,
        changed_workspace: false,
        fresh_read: true,
        search_result: false,
        no_result: false,
        verification: false,
        read_only: true,
        replayed: false,
        success: true,
    }
}

#[test]
fn fresh_evidence_invalidates_repeated_read_recovery_in_either_batch_order() {
    use super::loop_detect::{ProgressLedger, ProgressReason};

    let old = read_observation("read:macros.rs:1:80");
    let fresh = read_observation("read:service.rs:1:100");
    for batch in [[&old, &old, &old, &fresh], [&fresh, &old, &old, &old]] {
        let mut ledger = ProgressLedger::default();
        ledger.observe(&old);
        let mut made_progress = false;
        let mut recovery = None;
        for observation in batch {
            let assessment = ledger.observe(observation);
            made_progress |= assessment.meaningful;
            if assessment.streak >= ProgressLedger::RECOVERY_STREAK {
                recovery = Some((
                    assessment.reason,
                    assessment.streak,
                    observation.action.clone(),
                ));
            }
        }
        assert_eq!(
            recovery.as_ref().unwrap().0,
            ProgressReason::NoNewInformation
        );
        assert!(batch_invalidates_read_recovery(
            made_progress,
            recovery.as_ref()
        ));
    }
}

#[test]
fn no_progress_recovery_names_the_repeated_action_and_preserves_resumability() {
    let guidance = targeted_no_progress_guidance(
        super::loop_detect::ProgressReason::NoNewInformation,
        3,
        "read:src/main.rs:1:80",
    );
    assert!(guidance.contains("no_new_information"));
    assert!(guidance.contains("read:src/main.rs:1:80"));
    assert!(guidance.contains("Do not replay"));
    assert!(guidance.contains("preserve completed tool results"));
}

#[test]
fn all_repeated_reads_still_request_recovery() {
    use super::loop_detect::ProgressLedger;

    let old = read_observation("read:macros.rs:1:80");
    let mut ledger = ProgressLedger::default();
    ledger.observe(&old);
    let mut made_progress = false;
    let mut recovery = None;
    for _ in 0..ProgressLedger::RECOVERY_STREAK {
        let assessment = ledger.observe(&old);
        made_progress |= assessment.meaningful;
        recovery = Some((assessment.reason, assessment.streak, old.action.clone()));
    }
    assert!(!batch_invalidates_read_recovery(
        made_progress,
        recovery.as_ref()
    ));
    assert!(should_apply_loop_recovery(
        false,
        false,
        false,
        recovery.is_some()
    ));
}

#[test]
fn fresh_reads_do_not_clear_failure_or_churn_recovery() {
    use super::loop_detect::ProgressReason;

    for reason in [
        ProgressReason::RepeatedFailure,
        ProgressReason::Churn,
        ProgressReason::RepeatedVerification,
    ] {
        let recovery = (reason, 3, "run_command".to_owned());
        assert!(!batch_invalidates_read_recovery(true, Some(&recovery)));
    }
}

#[test]
fn expected_shell_wrapper_statuses_do_not_become_recovery_failures() {
    let no_match = read_call("rg missing_symbol src | head -20");
    let no_match_metadata = ToolResultMetadata {
        success: false,
        exit_code: Some(1),
        ..Default::default()
    };
    assert!(benign_shell_wrapper_failure(
        Some(&no_match),
        &no_match_metadata,
        "exit code: 1\n(no output)"
    ));

    let sigpipe = read_call("rg symbol src | head -20");
    let sigpipe_metadata = ToolResultMetadata {
        success: false,
        exit_code: Some(141),
        ..Default::default()
    };
    assert!(benign_shell_wrapper_failure(
        Some(&sigpipe),
        &sigpipe_metadata,
        "exit code: 141\n(no output)"
    ));

    let real_failure = read_call("rg symbol src && cargo test");
    assert!(!benign_shell_wrapper_failure(
        Some(&real_failure),
        &no_match_metadata,
        "exit code: 1\nerror: cargo test failed"
    ));
}

#[test]
fn search_no_match_stays_benign_when_output_mentions_error() {
    // A no-match report for a pattern like "error" still carries exit 1
    // from a search stage: structural benign-ness must not be vetoed by
    // the word "error" appearing in the quoted pattern or harness text.
    let no_match = read_call("rg error src");
    let no_match_metadata = ToolResultMetadata {
        success: false,
        exit_code: Some(1),
        ..Default::default()
    };
    assert!(benign_shell_wrapper_failure(
        Some(&no_match),
        &no_match_metadata,
        "no matches for 'error'"
    ));
}

#[test]
fn truncated_results_block_same_batch_completion() {
    let metadata = ToolResultMetadata {
        completeness: ToolResultCompleteness::LineTruncated,
        truncated: true,
        ..Default::default()
    };
    assert!(incomplete_tool_result(&metadata));
}

#[test]
fn complete_and_user_limited_results_do_not_block_completion() {
    assert!(!incomplete_tool_result(&ToolResultMetadata::default()));
    assert!(!incomplete_tool_result(&ToolResultMetadata {
        completeness: ToolResultCompleteness::UserLimited,
        ..Default::default()
    }));
}

#[test]
fn malformed_tool_history_is_bounded_and_keeps_a_diagnostic_marker() {
    let content = format!("<tool_call>\n{}", "x".repeat(20_000));
    let bounded = bounded_malformed_tool_history(&content);

    assert!(bounded.len() <= super::MAX_MALFORMED_TOOL_HISTORY_BYTES);
    assert!(bounded.starts_with("<tool_call>"));
    assert!(bounded.contains("malformed tool response truncated by harness"));
    assert!(bounded.contains("bytes omitted"));
}

#[test]
fn malformed_tool_history_does_not_split_utf8() {
    let content = format!("<tool_call>{}", "🙂".repeat(4_000));
    let bounded = bounded_malformed_tool_history(&content);

    assert!(bounded.len() <= super::MAX_MALFORMED_TOOL_HISTORY_BYTES);
    assert!(std::str::from_utf8(bounded.as_bytes()).is_ok());
}

#[test]
fn content_bearing_complete_inspection_requires_typed_metadata() {
    let call = read_call("cat src/app.ts");
    let mut metadata = ToolResultMetadata {
        success: true,
        inspection: Some(inspection_metadata(true)),
        ..Default::default()
    };
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "1: export const app = 1;"),
        Some(true)
    );

    metadata.inspection = None;
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "source"),
        None
    );
}

#[test]
fn completed_read_only_tool_result_supports_headless_synthesis_without_source_metadata() {
    let call = ToolCall {
        name: "read_email".to_string(),
        arguments: serde_json::json!({"uid": 56}),
        call_id: None,
    };
    let mut metadata = ToolResultMetadata {
        success: true,
        ..Default::default()
    };
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "Subject: test\nBody: hello"),
        Some(true),
        "a successful read-only tool result should count without file inspection metadata"
    );

    metadata.success = false;
    metadata.error_kind = Some(crate::tools::ToolErrorKind::Deferred);
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "intentionally deferred"),
        None,
        "a deferred call did not run and must not count as evidence"
    );

    metadata.success = true;
    metadata.error_kind = None;
    metadata.truncated = true;
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "partial email"),
        Some(false),
        "a truncated MCP result cannot support a complete synthesis"
    );

    let send = ToolCall {
        name: "send_email".to_string(),
        arguments: serde_json::json!({"to": "example@example.com"}),
        call_id: None,
    };
    metadata.truncated = false;
    assert_eq!(
        content_bearing_inspection_status(Some(&send), &metadata, "sent"),
        None,
        "a successful external action is not read-only inspection evidence"
    );
}

#[test]
fn wc_and_od_are_not_content_bearing_source_inspections() {
    for command in ["wc -l src/app.ts", "od -An -tx1 src/app.ts"] {
        let call = read_call(command);
        let metadata = ToolResultMetadata {
            success: true,
            inspection: Some(inspection_metadata(true)),
            ..Default::default()
        };
        assert_eq!(
            content_bearing_inspection_status(Some(&call), &metadata, "20"),
            None,
            "{command} must not count as source content"
        );
    }
}

#[test]
fn incomplete_typed_content_inspection_cannot_support_completion() {
    let call = read_call("cat src/app.ts");
    let metadata = ToolResultMetadata {
        success: true,
        completeness: ToolResultCompleteness::ByteTruncated,
        truncated: true,
        inspection: Some(inspection_metadata(false)),
        ..Default::default()
    };
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "partial source"),
        Some(false)
    );
}

#[test]
fn archived_message_reads_support_completion_but_inventories_do_not() {
    let mut call = crate::tools::ToolCall {
        name: "zoom_context".into(),
        arguments: serde_json::json!({"message":1}),
        call_id: None,
    };
    let mut metadata = ToolResultMetadata {
        success: true,
        ..Default::default()
    };
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "archived message"),
        Some(true)
    );
    metadata.completeness = ToolResultCompleteness::ByteTruncated;
    metadata.truncated = true;
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "first page"),
        Some(false)
    );
    metadata.truncated = false;
    metadata.completeness = ToolResultCompleteness::UserLimited;
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "final requested page"),
        Some(true)
    );
    call.arguments = serde_json::json!({});
    assert_eq!(
        content_bearing_inspection_status(Some(&call), &metadata, "inventory previews"),
        None
    );
}
