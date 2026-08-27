use super::AppState;

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
