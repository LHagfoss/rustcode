use super::{AppState, AppStatus, DraftSubmitMode, PendingQuestion, ToolConfirmation};

fn steerable_state() -> AppState {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.active_turn_steerable_session = Some(state.active_session_id.clone());
    state
}

#[test]
fn accepts_multiple_nonempty_steers_as_distinct_ordered_items() {
    let mut state = steerable_state();

    assert!(state.can_accept_steer());
    assert!(state.queue_steer("Use Teams".to_owned()));
    assert!(state.queue_steer("Keep the same channel".to_owned()));
    assert_eq!(
        state
            .pending_steers
            .iter()
            .map(|steer| steer.text.as_str())
            .collect::<Vec<_>>(),
        ["Use Teams", "Keep the same channel"]
    );
    assert!(state.pending_queue.is_empty());
}

#[test]
fn rejects_steers_without_a_regular_active_streaming_turn() {
    let mut state = steerable_state();
    for status in [AppStatus::Idle, AppStatus::Queued] {
        state.status = status;
        assert!(!state.queue_steer("no".to_owned()));
    }

    state.status = AppStatus::Streaming;
    state.active_turn_steerable_session = Some("different-session".to_owned());
    assert!(!state.queue_steer("no".to_owned()));

    state.active_turn_steerable_session = Some(state.active_session_id.clone());
    state.status = AppStatus::AwaitingToolConfirmation;
    assert!(!state.queue_steer("no".to_owned()));

    state.status = AppStatus::Streaming;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "write_file".to_owned(),
        path: "file.txt".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
        rememberable_prefix: None,
    }]);
    assert!(!state.queue_steer("no".to_owned()));
    state.pending_tool_confirmation = None;

    state.status = AppStatus::AwaitingQuestion;
    assert!(!state.queue_steer("no".to_owned()));
    state.status = AppStatus::Streaming;
    state.pending_question = Some(PendingQuestion::new(
        "Choose".to_owned(),
        vec!["A".to_owned()],
        false,
    ));
    assert!(!state.queue_steer("no".to_owned()));
    state.pending_question = None;
    state.pending_question_queue.push(PendingQuestion::new(
        "Next question".to_owned(),
        vec!["B".to_owned()],
        false,
    ));
    assert!(!state.queue_steer("no".to_owned()));
}

#[test]
fn rejects_empty_or_whitespace_only_steers() {
    let mut state = steerable_state();
    assert!(!state.queue_steer(String::new()));
    assert!(!state.queue_steer(" \n\t ".to_owned()));
    assert!(state.pending_steers.is_empty());
}

#[test]
fn takes_pending_steers_only_for_the_matching_turn_session() {
    let mut state = steerable_state();
    let session_id = state.active_session_id.clone();
    state.queue_steer("first".to_owned());
    state.queue_steer("second".to_owned());

    assert!(state.take_steers_for_history("other-session").is_empty());
    assert_eq!(state.pending_steers.len(), 2);
    assert_eq!(
        state.take_steers_for_history(&session_id),
        ["first", "second"]
    );
    assert!(state.pending_steers.is_empty());
}

#[test]
fn promotes_matching_pending_steers_ahead_of_followups_in_order() {
    let mut state = steerable_state();
    state.queue_steer("first".to_owned());
    state.queue_steer("second".to_owned());
    state.pending_queue = vec!["follow-up".to_owned(), "wake-up".to_owned()];

    state.promote_pending_steers_to_queue(&state.active_session_id.clone());

    assert!(state.pending_steers.is_empty());
    assert_eq!(state.active_turn_steerable_session, None);
    assert_eq!(
        state.pending_queue,
        ["first", "second", "follow-up", "wake-up"]
    );
}

#[test]
fn promotes_pending_steers_after_turn_finalization_clears_steerability() {
    let mut state = steerable_state();
    let session_id = state.active_session_id.clone();
    state.queue_steer("final-boundary steer".to_owned());
    state.pending_queue = vec!["existing follow-up".to_owned()];
    // Turn finalization clears input eligibility before the queue orchestrator
    // receives the completed context and performs its FIFO fallback.
    state.active_turn_steerable_session = None;

    state.promote_pending_steers_to_queue(&session_id);

    assert!(state.pending_steers.is_empty());
    assert_eq!(
        state.pending_queue,
        ["final-boundary steer", "existing follow-up"]
    );
}

#[test]
fn later_promoted_steers_follow_the_remaining_prefix_before_its_wakeup() {
    let mut state = steerable_state();
    let session_id = state.active_session_id.clone();
    state.queue_steer("old first".to_owned());
    state.queue_steer("old second".to_owned());
    state.pending_queue = vec!["follow-up".to_owned()];

    state.promote_pending_steers_to_queue(&session_id);
    let wakeup_position = state.promoted_steer_prefix_count;
    state.pending_queue.insert(
        wakeup_position,
        "__task_wakeup__:productive_segment".to_owned(),
    );

    assert_eq!(state.pending_queue.remove(0), "old first");
    assert!(state.take_promoted_steer_prefix_prompt());

    state.status = AppStatus::Streaming;
    state.active_turn_steerable_session = Some(session_id.clone());
    assert!(state.queue_steer("new steer".to_owned()));
    state.promote_pending_steers_to_queue(&session_id);

    assert_eq!(
        state.pending_queue,
        [
            "old second",
            "new steer",
            "__task_wakeup__:productive_segment",
            "follow-up"
        ]
    );
    assert_eq!(state.promoted_steer_prefix_count, 2);
}

#[test]
fn stale_session_transitions_leave_other_session_steers_untouched() {
    let mut state = steerable_state();
    state.draft_submit_mode = DraftSubmitMode::Queue;
    state.queue_steer("keep me".to_owned());
    let pending = state.pending_steers[0].text.clone();
    let marker = state.active_turn_steerable_session.clone();
    let mode = state.draft_submit_mode;

    assert!(state.take_steers_for_history("other-session").is_empty());
    state.promote_pending_steers_to_queue("other-session");
    assert_eq!(state.pending_steers[0].text, pending);
    assert_eq!(state.active_turn_steerable_session, marker);
    assert_eq!(state.draft_submit_mode, mode);
    assert!(state.pending_queue.is_empty());
}
