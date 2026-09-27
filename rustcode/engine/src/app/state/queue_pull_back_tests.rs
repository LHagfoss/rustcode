use super::{AppState, AppStatus, DraftSubmitMode};

#[test]
fn pop_queued_prompt_pulls_latest_user_prompt_skipping_wakeups() {
    let mut s = AppState::new();
    s.pending_queue = vec![
        "first prompt".to_string(),
        "second prompt".to_string(),
        "__task_wakeup__:abc123".to_string(),
    ];

    assert!(s.pop_queued_prompt());
    assert_eq!(s.input_buffer, "second prompt");
    assert_eq!(s.cursor_position, "second prompt".len());
    // The wakeup entry and the older prompt stay queued.
    assert_eq!(s.pending_queue.len(), 2);

    assert!(s.pop_queued_prompt());
    assert_eq!(s.input_buffer, "first prompt");
    // Only the wakeup entry remains — nothing more to pull.
    assert!(!s.pop_queued_prompt());
    assert_eq!(s.pending_queue, vec!["__task_wakeup__:abc123"]);
}

#[test]
fn observed_background_wakeups_coalesce_without_removing_user_prompts() {
    let mut s = AppState::new();
    s.pending_queue = vec![
        "__task_wakeup__:first".to_string(),
        "queued user prompt".to_string(),
        "__task_wakeup__:second".to_string(),
    ];

    assert_eq!(s.consume_observed_background_wakeups(), 2);
    assert_eq!(s.pending_queue, ["queued user prompt"]);
}

#[test]
fn pulling_back_promoted_steers_updates_the_tracked_prefix() {
    let mut s = AppState::new();
    s.pending_queue = vec![
        "first correction".to_string(),
        "second correction".to_string(),
        "__task_wakeup__:productive_segment".to_string(),
        "follow-up".to_string(),
    ];
    s.promoted_steer_prefix_count = 2;

    assert!(s.pop_queued_prompt());
    assert_eq!(s.input_buffer, "follow-up");
    assert_eq!(s.promoted_steer_prefix_count, 2);

    assert!(s.pop_queued_prompt());
    assert_eq!(s.input_buffer, "second correction");
    assert_eq!(s.promoted_steer_prefix_count, 1);
    assert_eq!(
        s.pending_queue,
        ["first correction", "__task_wakeup__:productive_segment"]
    );
}

#[test]
fn exact_pending_queue_item_can_be_removed_without_confusing_duplicates() {
    let mut state = AppState::new();
    state.pending_queue = vec![
        "same".to_owned(),
        "__task_wakeup__:task".to_owned(),
        "same".to_owned(),
    ];
    state.promoted_steer_prefix_count = 1;

    assert_eq!(
        state.remove_pending_user_prompt(DraftSubmitMode::Queue, 2, "same"),
        Some("same".to_owned())
    );
    assert_eq!(state.pending_queue, ["same", "__task_wakeup__:task"]);
    assert_eq!(state.promoted_steer_prefix_count, 1);
    assert_eq!(
        state.remove_pending_user_prompt(DraftSubmitMode::Queue, 0, "stale"),
        None
    );
}

#[test]
fn exact_pending_steer_can_be_removed_for_editing() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.active_turn_steerable_session = Some(state.active_session_id.clone());
    assert!(state.queue_steer("first".to_owned()));
    assert!(state.queue_steer("second".to_owned()));

    assert_eq!(
        state.remove_pending_user_prompt(DraftSubmitMode::Steer, 0, "first"),
        Some("first".to_owned())
    );
    assert_eq!(state.pending_steers.len(), 1);
    assert_eq!(state.pending_steers[0].text, "second");
}
