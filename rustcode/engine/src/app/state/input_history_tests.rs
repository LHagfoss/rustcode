use super::AppState;

#[test]
fn recall_uses_typed_inputs_not_chat_history() {
    let mut s = AppState::new();
    // Slash commands never become chat messages; generated user blobs
    // (e.g. /goal's "Goal: ..." text) aren't typed input.
    s.input_history = vec![
        "fix the parser".to_string(),
        "/verbosity toggle".to_string(),
    ];
    s.history.push(crate::app::state::ChatMessage::new(
        "user",
        "Goal: generated blob that should not be recalled",
    ));

    s.history_up();
    assert_eq!(s.input_buffer, "/verbosity toggle");
    s.history_up();
    assert_eq!(s.input_buffer, "fix the parser");
    s.history_down();
    assert_eq!(s.input_buffer, "/verbosity toggle");
    s.history_down();
    assert_eq!(s.input_buffer, "");
    assert!(s.history_index.is_none());
}
