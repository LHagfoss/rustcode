use super::AppState;

#[test]
fn composer_selection_range_orders_anchor_and_cursor() {
    let mut state = AppState::new();
    state.input_buffer = "hello".to_owned();
    state.cursor_position = 5;
    state.composer_selection_anchor = Some(1);
    assert_eq!(state.composer_selection_range(), Some((1, 5)));
    assert_eq!(state.composer_selected_text().as_deref(), Some("ello"));

    state.cursor_position = 0;
    assert_eq!(state.composer_selection_range(), Some((0, 1)));
    assert_eq!(state.composer_selected_text().as_deref(), Some("h"));
}

#[test]
fn composer_selection_is_empty_when_anchor_equals_cursor() {
    let mut state = AppState::new();
    state.input_buffer = "hello".to_owned();
    state.cursor_position = 2;
    state.composer_selection_anchor = Some(2);
    assert!(!state.has_composer_selection());
    assert_eq!(state.composer_selection_range(), None);
}

#[test]
fn typing_replaces_composer_selection_once() {
    let mut state = AppState::new();
    state.input_buffer = "hello world".to_owned();
    state.cursor_position = 5;
    state.composer_selection_anchor = Some(0);
    state.insert_char('H');
    assert_eq!(state.input_buffer, "H world");
    assert_eq!(state.cursor_position, 1);
    assert!(!state.has_composer_selection());
}

#[test]
fn backspace_deletes_composer_selection_once() {
    let mut state = AppState::new();
    state.input_buffer = "hello world".to_owned();
    state.cursor_position = 5;
    state.composer_selection_anchor = Some(0);
    state.delete_char_backspace();
    assert_eq!(state.input_buffer, " world");
    assert_eq!(state.cursor_position, 0);
    assert!(!state.has_composer_selection());
}

#[test]
fn delete_removes_composer_selection_once() {
    let mut state = AppState::new();
    state.input_buffer = "hello".to_owned();
    state.cursor_position = 0;
    state.composer_selection_anchor = Some(5);
    state.delete_char_delete();
    assert_eq!(state.input_buffer, "");
    assert_eq!(state.cursor_position, 0);
    assert!(!state.has_composer_selection());
}

#[test]
fn composer_selection_clamps_stale_bounds_after_clear() {
    let mut state = AppState::new();
    state.input_buffer = "hello".to_owned();
    state.cursor_position = 5;
    state.composer_selection_anchor = Some(1);
    state.input_buffer.clear();
    state.cursor_position = 0;
    assert_eq!(state.composer_selection_range(), None);
}
