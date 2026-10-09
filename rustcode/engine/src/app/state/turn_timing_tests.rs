use super::*;
use std::time::{Duration, Instant};

#[test]
fn turn_timing_excludes_open_and_closed_user_waits_and_survives_cleanup() {
    let mut state = AppState::new();
    state.current_turn_started_at = Some(Instant::now() - Duration::from_secs(60));
    let id = state.begin_turn_identity();
    state.history.push(ChatMessage::new("user", "do work"));
    state
        .history
        .push(ChatMessage::new("assistant", "phase one"));
    let actual_start = state
        .active_turn_timing
        .as_ref()
        .unwrap()
        .started_at
        .clone();
    state.begin_user_wait(Instant::now() - Duration::from_secs(28));
    let live = state.measured_turn_timing().unwrap();
    assert!((32000..33000).contains(&live.elapsed_work_ms.unwrap()));
    state.exclude_user_wait(Duration::from_secs(28));
    state
        .history
        .push(ChatMessage::new("assistant", "phase two"));
    let finished = state
        .freeze_turn_timing(Some(TurnOutcome::Completed))
        .unwrap();
    assert_eq!(finished.turn_id, id);
    assert_eq!(finished.started_at, actual_start);
    assert_eq!(finished.outcome, Some(TurnOutcome::Completed));
    assert!((32000..33000).contains(&finished.elapsed_work_ms.unwrap()));
    chrono::DateTime::parse_from_rfc3339(finished.ended_at.as_ref().unwrap()).unwrap();
    state.enter_idle();
    state.end_turn_identity(&id);
    let saved = serde_json::to_string(state.history.as_slice()).unwrap();
    let restored: Vec<ChatMessage> = serde_json::from_str(&saved).unwrap();
    assert!(
        restored
            .iter()
            .all(|message| message.turn.as_ref() == Some(&finished))
    );
    assert!(state.active_turn_timing.is_none());
}

#[test]
fn cancellation_is_frozen_once_and_cannot_be_reclassified_as_failure() {
    let mut state = AppState::new();
    state.current_turn_started_at = Some(Instant::now() - Duration::from_secs(3));
    let id = state.begin_turn_identity();
    state.history.push(ChatMessage::new("user", "cancel me"));
    let cancelled = state
        .freeze_turn_timing(Some(TurnOutcome::Cancelled))
        .unwrap();
    state.clear_active_turn_projection();
    state
        .history
        .as_mut_vec()
        .push(ChatMessage::new("tool", "late cancelled-batch result"));
    assert_eq!(
        state.freeze_turn_timing(Some(TurnOutcome::Failed)),
        Some(cancelled.clone())
    );
    state.end_turn_identity(&id);
    assert_eq!(state.history[0].turn.as_ref(), Some(&cancelled));
    assert_eq!(state.history[1].turn.as_ref(), Some(&cancelled));
}

#[test]
fn discarding_suspended_work_records_cancellation_without_counting_idle_time() {
    let mut state = AppState::new();
    state.current_turn_started_at = Some(Instant::now() - Duration::from_secs(32));
    let id = state.begin_turn_identity();
    state.history.push(ChatMessage::new("user", "suspend"));
    let frozen = state.freeze_turn_timing(None).unwrap();
    let mut context = crate::network::TurnContext::new();
    context.lifecycle.turn_timing = Some(frozen.clone());
    state.background_turn_context = Some(Box::new(context));
    state.enter_idle();
    state.end_turn_identity(&id);
    state.finish_pending_turn_timing(TurnOutcome::Cancelled);
    let terminal = state.history[0].turn.as_ref().unwrap();
    assert_eq!(terminal.elapsed_work_ms, frozen.elapsed_work_ms);
    assert_eq!(terminal.started_at, frozen.started_at);
    assert_eq!(terminal.outcome, Some(TurnOutcome::Cancelled));
    assert!(terminal.ended_at.is_some());
}

#[test]
fn a_suspended_turn_keeps_identity_and_measured_work_across_checkpoint_restore() {
    let mut state = AppState::new();
    state.current_turn_started_at = Some(Instant::now() - Duration::from_secs(32));
    let id = state.begin_turn_identity();
    state.history.push(ChatMessage::new("user", "long work"));
    let mut context = crate::network::TurnContext::new();
    context.lifecycle.turn_timing = state.freeze_turn_timing(None);
    context.lifecycle.prior_run_duration = Duration::from_secs(32);
    let checkpoint = context.segment_checkpoint(&state.active_session_id, true, false);
    let bytes = serde_json::to_vec(&checkpoint).unwrap();
    let restored_checkpoint = serde_json::from_slice(&bytes).unwrap();
    state.enter_idle();
    state.end_turn_identity(&id);
    let mut restored = crate::network::TurnContext::new();
    assert!(restored.restore_segment(&restored_checkpoint, &state.active_session_id));
    let first_start = restored
        .lifecycle
        .turn_timing
        .as_ref()
        .unwrap()
        .started_at
        .clone();
    state.background_turn_context = Some(Box::new(restored));
    let resumed = crate::network::turn_engine::take_turn_context_for_prompt(&mut state, true, 40);
    let resumed_id = state.resume_turn_identity(resumed.lifecycle.turn_timing.unwrap());
    assert_eq!(resumed_id, id);
    let timing = state.measured_turn_timing().unwrap();
    assert_eq!(timing.started_at, first_start);
    assert!((32000..33000).contains(&timing.elapsed_work_ms.unwrap()));
    assert!(timing.ended_at.is_none() && timing.outcome.is_none());
}

#[test]
fn old_history_never_invents_a_turn_or_timing_from_a_timestamp() {
    let message: ChatMessage = serde_json::from_str(
        r#"{"role":"assistant","content":"old answer","timestamp":"2026-01-01T10:00:00Z"}"#,
    )
    .unwrap();
    assert_eq!(message.turn, None);
    let encoded = serde_json::to_value(message).unwrap();
    for field in [
        "turn",
        "response_time_ms",
        "thought_time_ms",
        "completed_at",
    ] {
        assert!(encoded.get(field).is_none(), "{field} must remain absent");
    }
}

#[test]
fn resuming_the_current_session_preserves_its_newly_finalized_timing() {
    let dir = tempfile::tempdir().unwrap();
    let id = format!("timing-test-{}", uuid::Uuid::new_v4());
    let sessions = dir.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    let path = sessions.join(format!("{id}.json"));
    let mut state = AppState::new();
    state.active_session_id = id;
    state.current_turn_started_at = Some(Instant::now() - Duration::from_secs(2));
    state.begin_turn_identity();
    state
        .history
        .push(ChatMessage::new("user", "unfinished stored prompt"));
    std::fs::write(&path, serde_json::to_vec(state.history.as_slice()).unwrap()).unwrap();
    let meta = crate::config::SessionMeta {
        path,
        title: "current".to_owned(),
        when: String::new(),
        message_count: 1,
        workspace_cwd: None,
    };
    assert!(crate::app::load_session_into(&mut state, &meta));
    let turn = state
        .history
        .iter()
        .find_map(|message| message.turn.as_ref())
        .unwrap();
    assert_eq!(turn.outcome, Some(TurnOutcome::Cancelled));
    assert!(turn.ended_at.is_some());
}

#[tokio::test]
async fn esc_and_new_session_finalize_suspended_turns_before_discarding_them() {
    for new_session in [false, true] {
        let mut state = AppState::new();
        state.current_turn_started_at = Some(Instant::now() - Duration::from_secs(2));
        let id = state.begin_turn_identity();
        state
            .history
            .push(ChatMessage::new("user", "suspended prompt"));
        let mut context = crate::network::TurnContext::new();
        context.lifecycle.turn_timing = state.freeze_turn_timing(None);
        state.background_turn_context = Some(Box::new(context));
        state.enter_idle();
        state.end_turn_identity(&id);
        let history;
        if new_session {
            let outgoing = state.active_session_id.clone();
            crate::app::start_new_session(&mut state);
            crate::config::flush_history();
            let store =
                rustcode_session::SessionStore::new(crate::config::get_config_dir().unwrap());
            history = store.load_session_history_direct(&outgoing);
        } else {
            let shared = Arc::new(tokio::sync::Mutex::new(state));
            crate::app::stop_turn_keeping_draft(
                &shared,
                &mut tokio_util::sync::CancellationToken::new(),
            )
            .await;
            state = Arc::try_unwrap(shared).ok().unwrap().into_inner();
            history = state.history.as_slice().to_vec();
        }
        assert!(state.background_turn_context.is_none());
        assert_eq!(
            history[0].turn.as_ref().unwrap().outcome,
            Some(TurnOutcome::Cancelled)
        );
    }
}

#[tokio::test]
async fn cancelled_suspended_checkpoint_cannot_resume_after_history_reload() {
    let mut state = AppState::new();
    state.active_session_id = format!("timing-checkpoint-{}", uuid::Uuid::new_v4());
    state.current_turn_started_at = Some(Instant::now() - Duration::from_secs(2));
    let id = state.begin_turn_identity();
    state
        .history
        .push(ChatMessage::new("user", "suspended durable prompt"));
    state.history.push(ChatMessage::new(
        "assistant",
        "Work continues after the tool.",
    ));
    let mut context = crate::network::TurnContext::new();
    context.lifecycle.turn_timing = state.freeze_turn_timing(None);
    let checkpoint = context.segment_checkpoint(&state.active_session_id, true, false);
    crate::config::save_segment_checkpoint(&state.active_session_id, &checkpoint);
    assert!(crate::config::load_segment_checkpoint(&state.active_session_id).is_some());
    state.background_turn_context = Some(Box::new(context));
    state.enter_idle();
    state.end_turn_identity(&id);
    let shared = Arc::new(tokio::sync::Mutex::new(state));
    crate::app::stop_turn_keeping_draft(&shared, &mut tokio_util::sync::CancellationToken::new())
        .await;
    let mut state = Arc::try_unwrap(shared).ok().unwrap().into_inner();
    crate::config::flush_history();
    assert!(crate::config::load_segment_checkpoint(&state.active_session_id).is_none());
    let store = rustcode_session::SessionStore::new(crate::config::get_config_dir().unwrap());
    let meta = store.session_meta_by_id(&state.active_session_id).unwrap();
    assert!(crate::app::load_session_into(&mut state, &meta));
    assert!(!crate::app::queue_restored_segment(&mut state));
    assert!(state.background_turn_context.is_none());
    assert_eq!(
        state.history[0].turn.as_ref().unwrap().outcome,
        Some(TurnOutcome::Cancelled)
    );
}
