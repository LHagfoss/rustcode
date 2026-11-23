use super::{
    InputFlow, PickerSearchTarget, QuestionEditAction, SubagentPickerAction,
    agent_context_return_target, clear_selection_for_composer_key, filtered_command_picker_items,
    handle_cmd_copy_chord, handle_cmd_copy_chord_with, handle_copy_or_exit_chord,
    handle_picker_search_key, handle_question_custom_key, insert_clipboard_paste,
    insert_question_answer_text, is_cmd_copy_chord, is_copy_or_exit_chord, is_keyboard_range_key,
    is_shift_tab, is_transcript_navigation, open_demo_if_requested, picker_selection_for_key,
    report_selection_copy, return_to_latest_for_key, scroll_command_panel, scroll_panel_selection,
    selection_owns_key, subagent_picker_action,
};
use crate::ui::{Composer, TranscriptState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};
use rustcode::app::AppState;
use rustcode::clipboard::ClipboardCopyStatus;
use rustcode::controller::PendingQuestion;
use std::sync::Arc;
use tokio::sync::Mutex;

#[test]
fn clicking_a_tool_block_opens_then_closes_only_that_block() {
    use rustcode::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
    for round in 0..2 {
        let id = format!("call-{round}");
        state.history.push(
            ChatMessage::new("assistant", format!("round {round}")).with_tool_calls(vec![
                ToolCallRef {
                    id: id.clone(),
                    name: "run_command".to_owned(),
                    arguments: r#"{"command":"cargo test"}"#.to_owned(),
                },
            ]),
        );
        state.history.push(
            ChatMessage::new("tool", "run_command: exit code: 0\nok")
                .answering(Some(id))
                .with_tool_result(ToolResultRecord {
                    tool_name: "run_command".to_owned(),
                    success: true,
                    ..Default::default()
                }),
        );
    }

    // The second block is history index 3.
    super::toggle_tool_block(&mut state, 80, (3, 4));
    assert!(state.expanded_thoughts.contains(&3));
    assert!(!state.expanded_thoughts.contains(&1));

    super::toggle_tool_block(&mut state, 80, (3, 4));
    assert!(state.expanded_thoughts.is_empty());
}

#[test]
fn clicking_a_folded_run_opens_and_folds_it() {
    let mut state = AppState::new();
    state.verbosity = rustcode::app::Verbosity::Low;
    // A summary row stands for the calls from history index 1 up to 4.
    super::toggle_tool_block(&mut state, 80, (1, 4));
    assert!(state.expanded_tool_runs.contains(&1));
    assert!(state.expanded_thoughts.is_empty(), "no call body opens");
    super::toggle_tool_block(&mut state, 80, (1, 4));
    assert!(state.expanded_tool_runs.is_empty());
}

#[test]
fn ask_question_editor_matches_composer_modified_enter_and_word_navigation() {
    let mut question = PendingQuestion::new("Question?".to_owned(), vec![], false);
    question.activate_custom_input();
    question.insert_str("café tools");

    assert_eq!(
        handle_question_custom_key(
            &mut question,
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)
        ),
        QuestionEditAction::Handled
    );
    assert_eq!(question.custom_cursor, "café ".len());
    handle_question_custom_key(
        &mut question,
        KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
    );
    assert_eq!(question.custom_input.as_deref(), Some("café Xtools"));

    handle_question_custom_key(
        &mut question,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
    );
    handle_question_custom_key(
        &mut question,
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
    );
    assert_eq!(question.custom_input.as_deref(), Some("café X\n\ntools"));
    assert_eq!(
        handle_question_custom_key(
            &mut question,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        QuestionEditAction::Submit
    );
}

#[test]
fn ask_question_editor_supports_forward_word_delete_and_line_kill() {
    let mut question = PendingQuestion::new("Question?".to_owned(), vec![], false);
    question.activate_custom_input();
    question.insert_str("one two three");
    question.custom_cursor = "one ".len();

    handle_question_custom_key(
        &mut question,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT),
    );
    // Forward word delete removes the word and leaves the space after it.
    assert_eq!(question.custom_input.as_deref(), Some("one  three"));
    assert_eq!(question.custom_cursor, "one ".len());

    handle_question_custom_key(
        &mut question,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER),
    );
    assert_eq!(question.custom_input.as_deref(), Some(" three"));
    assert_eq!(question.custom_cursor, 0);
}

#[test]
fn ask_question_paste_preserves_normalized_newlines_and_unicode() {
    let mut question = PendingQuestion::new("Question?".to_owned(), vec![], false);
    question.activate_custom_input();
    insert_question_answer_text(&mut question, "først\r\n二つ\rslutt");

    assert_eq!(question.custom_input.as_deref(), Some("først\n二つ\nslutt"));
    assert_eq!(question.custom_cursor, "først\n二つ\nslutt".len());
}

#[test]
fn ask_question_up_from_first_answer_line_returns_to_options_with_draft() {
    let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
    let mut question = PendingQuestion::new(
        "Question?".to_owned(),
        vec!["One".to_owned(), "Two".to_owned()],
        false,
    );
    question.selected = 2;
    question.activate_custom_input();
    question.insert_str("first\nsecond");

    // Up on the second line moves within the answer.
    handle_question_custom_key(&mut question, up);
    assert!(question.custom_input.is_some());
    assert!(question.custom_cursor_on_first_line());

    // Up on the first line returns to the last option and keeps the text.
    handle_question_custom_key(&mut question, up);
    assert!(question.custom_input.is_none());
    assert_eq!(question.selected, 1);
    assert_eq!(question.custom_draft, "first\nsecond");

    question.activate_custom_input();
    assert_eq!(question.custom_input.as_deref(), Some("first\nsecond"));
    assert_eq!(question.custom_cursor, "first\nsecond".len());
}

#[test]
fn scroll_speed_picker_steps_within_range_and_saves_on_enter() {
    let mut state = AppState::new();
    state.settings_picker = Some(rustcode::controller::SettingsPicker::ScrollSpeed);
    state.modal_picker_index = 1;

    assert_eq!(
        super::scroll_speed_picker_key(&mut state, KeyCode::Left),
        None
    );
    assert_eq!(state.modal_picker_index, 1, "never below one line");
    super::scroll_speed_picker_key(&mut state, KeyCode::Right);
    super::scroll_speed_picker_key(&mut state, KeyCode::Up);
    assert_eq!(state.modal_picker_index, 3);
    super::scroll_speed_picker_key(&mut state, KeyCode::End);
    super::scroll_speed_picker_key(&mut state, KeyCode::Right);
    assert_eq!(state.modal_picker_index, 20, "never above the range");
    super::scroll_speed_picker_key(&mut state, KeyCode::Char('5'));
    assert_eq!(
        super::scroll_speed_picker_key(&mut state, KeyCode::Enter),
        Some(5)
    );

    assert_eq!(
        super::scroll_speed_picker_key(&mut state, KeyCode::Esc),
        None
    );
    assert!(state.settings_picker.is_none(), "esc closes without saving");
}

#[test]
fn ask_question_up_without_options_stays_in_the_answer() {
    let mut question = PendingQuestion::new("Question?".to_owned(), vec![], false);
    question.activate_custom_input();
    question.insert_str("text");
    handle_question_custom_key(
        &mut question,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
    );
    assert_eq!(question.custom_input.as_deref(), Some("text"));
}

#[tokio::test]
async fn opening_demo_consumes_only_the_command_and_preserves_live_session() {
    let mut state = AppState::new();
    state.active_session_id = "live-session".to_owned();
    state
        .history
        .push(rustcode::app::ChatMessage::new("user", "existing history"));
    state.input_buffer = "/test".to_owned();
    state.cursor_position = state.input_buffer.len();
    let expected_history = state.history.snapshot();
    let app_state = Arc::new(Mutex::new(state));
    let mut demo_state = None;

    assert!(open_demo_if_requested(&app_state, &mut demo_state).await);
    let live = app_state.lock().await;
    assert_eq!(live.active_session_id, "live-session");
    assert_eq!(live.history.snapshot(), expected_history);
    assert!(live.input_buffer.is_empty());
    let demo = demo_state.expect("demo view is separate from the live session");
    assert_eq!(demo.render_state().active_session_id, "static-visual-demo");
    assert_eq!(demo.render_state().history.len(), 4);
}

#[tokio::test]
async fn demo_command_does_not_match_a_prompt_with_arguments() {
    let mut state = AppState::new();
    state.input_buffer = "/test please".to_owned();
    let app_state = Arc::new(Mutex::new(state));
    let mut demo_state = None;

    assert!(!open_demo_if_requested(&app_state, &mut demo_state).await);
    assert!(demo_state.is_none());
    assert_eq!(app_state.lock().await.input_buffer, "/test please");
}

#[test]
fn picker_navigation_stops_at_both_ends_and_reaches_long_list_tail() {
    let last = 127;
    let mut selected = 0;
    assert_eq!(
        picker_selection_for_key(selected, last + 1, KeyCode::Up),
        Some(0)
    );
    for _ in 0..last {
        selected = picker_selection_for_key(selected, last + 1, KeyCode::Down).unwrap();
    }
    assert_eq!(selected, last);
    assert_eq!(
        picker_selection_for_key(selected, last + 1, KeyCode::Down),
        Some(last)
    );
    assert_eq!(
        picker_selection_for_key(selected, last + 1, KeyCode::Up),
        Some(last - 1)
    );
    assert_eq!(picker_selection_for_key(0, 0, KeyCode::Down), Some(0));
}

#[test]
fn subagent_picker_left_selects_and_right_returns() {
    assert_eq!(
        subagent_picker_action(KeyCode::Left),
        Some(SubagentPickerAction::Select)
    );
    assert_eq!(
        subagent_picker_action(KeyCode::Right),
        Some(SubagentPickerAction::Close)
    );
    assert_eq!(
        subagent_picker_action(KeyCode::Enter),
        Some(SubagentPickerAction::Select)
    );
}

#[test]
fn tasks_panel_owns_navigation_log_stop_and_close_keys() {
    use rustcode::controller::TasksPanelInput;
    for (key, expected) in [
        (KeyCode::Up, Some(TasksPanelInput::Up)),
        (KeyCode::Down, Some(TasksPanelInput::Down)),
        (KeyCode::Enter, Some(TasksPanelInput::Open)),
        (KeyCode::Esc, Some(TasksPanelInput::Back)),
        (KeyCode::Char('x'), Some(TasksPanelInput::Stop)),
        (KeyCode::Char('q'), Some(TasksPanelInput::Close)),
        (KeyCode::Char('a'), None),
        (KeyCode::Backspace, None),
    ] {
        assert_eq!(super::tasks_panel_input_for_key(key), expected, "{key:?}");
    }
}

#[test]
fn right_from_an_empty_child_composer_returns_to_its_parent() {
    let mut state = AppState::new();
    let child = rustcode::app::SubagentController
        .spawn(
            &mut state,
            "child",
            None,
            None,
            false,
            Vec::new(),
            None,
            None,
        )
        .raw();
    state.selected_subagent_id = Some(child);
    assert_eq!(agent_context_return_target(&state, KeyCode::Right), Some(0));

    state.input_buffer = "draft".to_owned();
    assert_eq!(agent_context_return_target(&state, KeyCode::Right), None);
}

#[test]
fn command_picker_filter_includes_visible_fuzzy_group_matches() {
    let expected = crate::ui::PALETTE_ITEMS
        .iter()
        .find(|item| item.group == "System")
        .expect("palette has a System group");
    assert!(
        filtered_command_picker_items("system")
            .iter()
            .any(|item| item.name == expected.name)
    );
}

#[test]
fn picker_search_supports_composer_cursor_editing_without_splitting_unicode() {
    let mut state = AppState::new();
    state.command_picker_search = "café tools".to_owned();
    state.command_picker_search_cursor = state.command_picker_search.len();
    state.command_picker_index = 7;
    let key = |code, modifiers| KeyEvent::new(code, modifiers);

    assert!(handle_picker_search_key(
        &mut state,
        PickerSearchTarget::Command,
        key(KeyCode::Left, KeyModifiers::NONE)
    ));
    assert_eq!(
        state.command_picker_index, 7,
        "cursor motion keeps selection"
    );
    assert!(handle_picker_search_key(
        &mut state,
        PickerSearchTarget::Command,
        key(KeyCode::Left, KeyModifiers::ALT)
    ));
    assert_eq!(
        &state.command_picker_search[..state.command_picker_search_cursor],
        "café "
    );
    assert!(handle_picker_search_key(
        &mut state,
        PickerSearchTarget::Command,
        key(KeyCode::Char('X'), KeyModifiers::NONE)
    ));
    assert_eq!(state.command_picker_search, "café Xtools");
    assert_eq!(
        state.command_picker_index, 0,
        "query changes reset selection"
    );
    assert!(handle_picker_search_key(
        &mut state,
        PickerSearchTarget::Command,
        key(KeyCode::Left, KeyModifiers::NONE)
    ));
    assert!(handle_picker_search_key(
        &mut state,
        PickerSearchTarget::Command,
        key(KeyCode::Delete, KeyModifiers::ALT)
    ));
    assert_eq!(state.command_picker_search, "café ");
    assert_eq!(state.command_picker_search_cursor, "café ".len());
    assert!(handle_picker_search_key(
        &mut state,
        PickerSearchTarget::Command,
        key(KeyCode::Home, KeyModifiers::NONE)
    ));
    assert!(handle_picker_search_key(
        &mut state,
        PickerSearchTarget::Command,
        key(KeyCode::Char('a'), KeyModifiers::CONTROL)
    ));
    assert_eq!(state.command_picker_search_cursor, 0);
    assert!(!handle_picker_search_key(
        &mut state,
        PickerSearchTarget::Command,
        key(KeyCode::Up, KeyModifiers::NONE)
    ));
}

#[test]
fn mcp_editor_paste_uses_active_utf8_cursor_for_key_and_bracketed_paste() {
    use crate::ui::keymap::{KeyAction, KeyMap};

    let keymap = KeyMap::from_environment();
    for modifiers in [
        KeyModifiers::CONTROL,
        KeyModifiers::SUPER,
        KeyModifiers::META,
    ] {
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('v'), modifiers)),
            KeyAction::Paste
        );
    }

    let mut edit_state = rustcode::app::McpEditState {
        is_add: true,
        edit_index: None,
        name_input: "café".to_owned(),
        command_input: String::new(),
        args_input: String::new(),
        active_field: 0,
        cursor_pos: "caf".len(),
    };
    // A pasted line break becomes a space: the field holds one line.
    edit_state.insert_str("X\r\nY");
    assert_eq!(edit_state.name_input, "cafX Yé");
    assert_eq!(edit_state.cursor_pos, "cafX Y".len());
}

#[tokio::test]
async fn selection_copy_feedback_reports_backend_result_without_chat_message() {
    let state = Arc::new(Mutex::new(AppState::new()));
    for (result, expected) in [
        (
            ClipboardCopyStatus::Confirmed,
            "Copied selection to clipboard",
        ),
        (
            ClipboardCopyStatus::Requested,
            "Copy sent to terminal; paste to verify",
        ),
        (ClipboardCopyStatus::Failed, "Copy failed; try again"),
    ] {
        report_selection_copy(&state, "selected text", |text| {
            assert_eq!(text, "selected text");
            result
        })
        .await;
        let state = state.lock().await;
        assert_eq!(state.active_transient_notice(), Some(expected));
        assert!(state.history.is_empty());
    }
}

#[test]
fn shift_tab_is_normalized_from_supported_terminal_events() {
    assert!(is_shift_tab(KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::NONE
    )));
    assert!(is_shift_tab(KeyEvent::new(
        KeyCode::Tab,
        KeyModifiers::SHIFT
    )));
    assert!(!is_shift_tab(KeyEvent::new(
        KeyCode::Tab,
        KeyModifiers::NONE
    )));
}

#[test]
fn keyboard_range_only_owns_unmodified_or_shifted_arrows() {
    assert!(is_keyboard_range_key(KeyEvent::new(
        KeyCode::Left,
        KeyModifiers::NONE
    )));
    assert!(is_keyboard_range_key(KeyEvent::new(
        KeyCode::Down,
        KeyModifiers::SHIFT
    )));
    assert!(!is_keyboard_range_key(KeyEvent::new(
        KeyCode::Left,
        KeyModifiers::ALT
    )));
    assert!(!is_keyboard_range_key(KeyEvent::new(
        KeyCode::Char('x'),
        KeyModifiers::NONE
    )));
}

#[test]
fn ordinary_composer_key_exits_keyboard_range_mode() {
    let mut transcript = TranscriptState::default();
    let area = Rect::new(0, 0, 8, 1);
    transcript
        .selection
        .refresh(area, &Buffer::empty(area), &[false]);
    transcript.selection.begin_keyboard_with_snapshot(
        crate::ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
            &AppState::new(),
        )),
        0,
    );
    assert!(transcript.selection.is_keyboard_mode());
    assert!(selection_owns_key(
        &transcript,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    ));
    assert!(!selection_owns_key(
        &transcript,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    ));
    // Ctrl+C stays out of the selection's hands so it can arm the exit;
    // Cmd+C stays out too, but takes the copy-only path.
    assert!(is_copy_or_exit_chord(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL
    )));
    assert!(is_cmd_copy_chord(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SUPER
    )));
    assert!(!selection_owns_key(
        &transcript,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)
    ));
    clear_selection_for_composer_key(
        &mut transcript,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
    );
    assert!(!transcript.selection.is_keyboard_mode());
    assert!(!transcript.selection.is_active());
}

/// The advertised copy binding reaches the selection handler for both
/// mouse and keyboard selections, and `ctrl+c` without a selection falls
/// through to interrupt/exit (#1566).
#[test]
fn ctrl_c_copies_every_selection_and_still_reaches_the_exit_arm() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    // The hint and the handler share this definition: the footer promises
    // exactly the chord the handler owns.
    assert_eq!(rustcode::controller::copy_selection_binding(), "ctrl+c");
    let copy = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(is_copy_or_exit_chord(copy));
    // Cmd+C is handled as copy-only; Control takes precedence when both
    // modifiers arrive in the same event.
    let cmd_copy = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER);
    assert!(!is_copy_or_exit_chord(cmd_copy));
    assert!(is_cmd_copy_chord(cmd_copy));
    let mixed_copy = KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::SUPER | KeyModifiers::CONTROL,
    );
    assert!(is_copy_or_exit_chord(mixed_copy));
    assert!(!is_cmd_copy_chord(mixed_copy));
    assert!(!is_copy_or_exit_chord(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::NONE
    )));

    // A live selection does not take the chord away from the exit path, so
    // copying and arming happen together.
    let area = Rect::new(0, 0, 8, 1);
    let mut buffer = Buffer::empty(area);
    buffer.set_string(0, 0, "hello", ratatui::style::Style::default());

    // Mouse selection.
    let mut mouse = TranscriptState::default();
    mouse.selection.refresh(area, &buffer, &[false]);
    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    mouse.selection.mouse(down);
    mouse.selection.mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 3,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    mouse.selection.mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 3,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(mouse.selection.has_selection());
    assert!(!mouse.selection.is_keyboard_mode());
    assert_eq!(mouse.selection.selected_text().as_deref(), Some("hell"));

    // Keyboard selection offers the same copyable text.
    let mut keyboard = TranscriptState::default();
    keyboard.selection.refresh(area, &buffer, &[false]);
    keyboard.selection.begin_keyboard_with_snapshot(
        crate::ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
            &AppState::new(),
        )),
        0,
    );
    keyboard.selection.move_keyboard(KeyCode::Right);
    assert!(keyboard.selection.has_selection());

    // Esc is the only key the selection owns outright; the chord is handled
    // before it so the double-press exit stays reachable.
    assert!(!selection_owns_key(&mouse, copy));
    assert!(selection_owns_key(
        &mouse,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    ));

    // No selection: the key is not owned, so the caller falls through to
    // the existing interruption/exit behavior.
    let idle = TranscriptState::default();
    assert!(!idle.selection.has_selection());
    assert!(!selection_owns_key(
        &idle,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    ));
}

/// Two presses of the chord must exit even while a selection is live.
///
/// The selection handlers used to sit above the exit check and returned
/// early, so any selection made the double-press unreachable — and a
/// pinned selection survives a whole turn.
#[tokio::test]
async fn two_ctrl_c_presses_exit_while_a_selection_is_live() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let area = Rect::new(0, 0, 8, 1);
    let mut buffer = Buffer::empty(area);
    buffer.set_string(0, 0, "hello", ratatui::style::Style::default());

    let mut transcript = TranscriptState::default();
    transcript.selection.refresh(area, &buffer, &[false]);
    for event in [
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    ] {
        transcript.selection.mouse(event);
    }
    assert!(transcript.selection.has_selection());

    let app_state = Arc::new(Mutex::new(AppState::new()));

    // First press: copies and arms, without exiting.
    assert!(matches!(
        handle_copy_or_exit_chord(&app_state, &transcript).await,
        InputFlow::ContinueIteration
    ));
    assert!(app_state.lock().await.ctrl_c_exit_armed());

    // Second press inside the window exits.
    assert!(matches!(
        handle_copy_or_exit_chord(&app_state, &transcript).await,
        InputFlow::Exit { update: false }
    ));
}

#[tokio::test]
async fn repeated_cmd_c_copies_selection_without_arming_exit() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let area = Rect::new(0, 0, 8, 1);
    let mut buffer = Buffer::empty(area);
    buffer.set_string(0, 0, "hello", ratatui::style::Style::default());
    let mut transcript = TranscriptState::default();
    // `/session` displays a fixed session panel whose selection is stored
    // separately from the main transcript selection.
    transcript.panel_selection_area = Some(area);
    transcript.panel_selection_scrollable = false;
    transcript.panel_selection.refresh(area, &buffer, &[false]);
    for event in [
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    ] {
        transcript.panel_selection.mouse(event);
    }
    assert_eq!(
        transcript.panel_selection.selected_text().as_deref(),
        Some("hell")
    );

    let app_state = Arc::new(Mutex::new(AppState::new()));
    for _ in 0..2 {
        let copied = std::sync::Arc::new(std::sync::Mutex::new(None));
        let copied_by_backend = copied.clone();
        assert!(matches!(
            handle_cmd_copy_chord_with(&app_state, &transcript, move |text| {
                *copied_by_backend.lock().unwrap() = Some(text.to_owned());
                rustcode::clipboard::ClipboardCopyStatus::Confirmed
            })
            .await,
            InputFlow::ContinueIteration
        ));
        assert_eq!(copied.lock().unwrap().as_deref(), Some("hell"));
        assert!(!app_state.lock().await.ctrl_c_exit_armed());
    }
}

#[tokio::test]
async fn cmd_c_copies_a_composer_selection_without_hanging() {
    let app_state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = app_state.lock().await;
        state.input_buffer = "see ![image](file:///tmp/a.png) here".to_owned();
        state.composer_selection_anchor = Some(0);
        state.cursor_position = 3;
    }
    let copied = std::sync::Arc::new(std::sync::Mutex::new(None));
    let copied_by_backend = copied.clone();
    let flow = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        handle_cmd_copy_chord_with(&app_state, &TranscriptState::default(), move |text| {
            *copied_by_backend.lock().unwrap() = Some(text.to_owned());
            rustcode::clipboard::ClipboardCopyStatus::Confirmed
        }),
    )
    .await
    .expect("copying a composer selection must not deadlock on the state lock");
    assert!(matches!(flow, InputFlow::ContinueIteration));
    assert_eq!(copied.lock().unwrap().as_deref(), Some("see"));
}

#[tokio::test]
async fn cmd_c_without_app_selection_never_arms_exit() {
    let app_state = Arc::new(Mutex::new(AppState::new()));
    let transcript = TranscriptState::default();
    // Cmd+C also breaks a pending Ctrl+C confirmation. A later Ctrl+C
    // starts a fresh sequence instead of exiting unexpectedly.
    assert!(!rustcode::app::handle_ctrl_c(&app_state).await);
    assert!(app_state.lock().await.ctrl_c_exit_armed());
    for _ in 0..2 {
        assert!(matches!(
            handle_cmd_copy_chord(&app_state, &transcript).await,
            InputFlow::ContinueIteration
        ));
        assert!(!app_state.lock().await.ctrl_c_exit_armed());
    }
    assert!(!rustcode::app::handle_ctrl_c(&app_state).await);
    assert!(app_state.lock().await.ctrl_c_exit_armed());
}

/// With no selection the chord is purely the exit affordance.
#[tokio::test]
async fn two_ctrl_c_presses_exit_with_no_selection() {
    let transcript = TranscriptState::default();
    let app_state = Arc::new(Mutex::new(AppState::new()));

    assert!(matches!(
        handle_copy_or_exit_chord(&app_state, &transcript).await,
        InputFlow::ContinueIteration
    ));
    assert!(app_state.lock().await.ctrl_c_exit_armed());
    assert!(matches!(
        handle_copy_or_exit_chord(&app_state, &transcript).await,
        InputFlow::Exit { update: false }
    ));
}

/// Keyboard-select mode owns the chord with a collapsed caret: nothing to
/// copy, but it must still report progress and arm the exit.
#[tokio::test]
async fn collapsed_keyboard_selection_reports_instead_of_failing_silently() {
    let mut transcript = TranscriptState::default();
    let area = Rect::new(0, 0, 8, 1);
    transcript
        .selection
        .refresh(area, &Buffer::empty(area), &[false]);
    transcript.selection.begin_keyboard_with_snapshot(
        crate::ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
            &AppState::new(),
        )),
        0,
    );
    assert!(transcript.selection.is_keyboard_mode());
    assert_eq!(transcript.selection.selected_text(), None);

    let app_state = Arc::new(Mutex::new(AppState::new()));
    assert!(matches!(
        handle_copy_or_exit_chord(&app_state, &transcript).await,
        InputFlow::ContinueIteration
    ));
    assert!(app_state.lock().await.ctrl_c_exit_armed());
}

#[test]
fn command_panel_wheel_stops_at_bottom_and_reverses_immediately() {
    let mut transcript = TranscriptState::default();
    transcript.panel_selection_area = Some(Rect::new(0, 0, 40, 10));
    transcript.panel_selection_scrollable = true;
    transcript.panel_max_scroll_row = 7;
    let mut row = 0;
    scroll_panel_selection(&mut transcript, &mut row, 1);
    assert_eq!(row, 1, "each wheel event moves one row");
    for _ in 0..100 {
        scroll_panel_selection(&mut transcript, &mut row, 1);
    }
    assert_eq!(row, 7);
    scroll_panel_selection(&mut transcript, &mut row, -1);
    assert_eq!(row, 6, "no invisible scroll debt at the bottom");
    transcript.panel_max_scroll_row = 0;
    scroll_panel_selection(&mut transcript, &mut row, 1);
    assert_eq!(row, 0, "short or empty replacement content cannot scroll");
}

#[test]
fn command_panel_key_steps_clamp_before_and_after_movement() {
    for step in [1, 10] {
        let mut row = 0;
        for _ in 0..100 {
            scroll_command_panel(&mut row, 23, step);
        }
        assert_eq!(row, 23);
        scroll_command_panel(&mut row, 23, -step);
        assert_eq!(row, 23 - step as u16);
        row = u16::MAX;
        scroll_command_panel(&mut row, 15, -step);
        assert_eq!(row, 15 - step as u16, "resize clamps before moving up");
        scroll_command_panel(&mut row, 0, step);
        assert_eq!(row, 0);
    }
}

#[test]
fn informational_panel_selection_keeps_its_range_across_wheel_scroll() {
    let area = Rect::new(0, 0, 12, 1);
    let mut buffer = Buffer::empty(area);
    buffer.set_string(0, 0, "panel text", ratatui::style::Style::default());

    let mut transcript = TranscriptState::default();
    transcript.selection.refresh(area, &buffer, &[false]);
    transcript.selection.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    transcript.selection.mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    transcript.selection.mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    let transcript_text = transcript.selection.selected_text();

    transcript.panel_selection_area = Some(area);
    transcript.panel_selection.refresh(area, &buffer, &[false]);
    transcript.panel_selection.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 6,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    transcript.panel_selection.mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 9,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    transcript.panel_selection.mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 9,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(
        transcript.panel_selection.selected_text().as_deref(),
        Some("text")
    );
    assert_eq!(transcript.selection.selected_text(), transcript_text);
    let mut modal_scroll_row = 0;
    assert!(scroll_panel_selection(
        &mut transcript,
        &mut modal_scroll_row,
        1,
    ));
    assert_eq!(modal_scroll_row, 0);
    assert_eq!(
        transcript.panel_selection.selected_text().as_deref(),
        Some("text"),
        "wheel input over a fixed info panel preserves the copyable range"
    );
    clear_selection_for_composer_key(
        &mut transcript,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
    );
    assert!(!transcript.panel_selection.has_selection());
    assert_eq!(transcript.selection.selected_text(), transcript_text);
}

#[test]
fn composer_keys_keep_reading_position_and_escape_returns_to_latest() {
    let mut transcript = TranscriptState::default();
    transcript.scroll_up(3);
    assert!(!return_to_latest_for_key(
        &mut transcript,
        KeyCode::Char('x')
    ));
    assert_eq!(transcript.scroll_rows(), 3);

    transcript.scroll_up(2);
    assert!(return_to_latest_for_key(&mut transcript, KeyCode::Esc));
    assert_eq!(transcript.scroll_rows(), 0);
}

#[test]
fn typing_and_submission_end_pinned_selection_while_transcript_navigation_keeps_it() {
    let mut transcript = TranscriptState::default();
    let area = Rect::new(0, 0, 8, 2);
    let buffer = Buffer::empty(area);
    transcript.selection.refresh(area, &buffer, &[false, false]);
    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    let begin = |transcript: &mut TranscriptState| {
        transcript.selection.begin_with_snapshot(
            down,
            crate::ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
                &AppState::new(),
            )),
            transcript.scroll_rows(),
        );
        assert!(transcript.selection.is_active());
    };
    begin(&mut transcript);
    assert!(is_transcript_navigation(KeyEvent::new(
        KeyCode::PageUp,
        KeyModifiers::NONE
    )));
    clear_selection_for_composer_key(
        &mut transcript,
        KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
    );
    assert!(transcript.selection.is_active());
    clear_selection_for_composer_key(
        &mut transcript,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
    );
    assert!(!transcript.selection.is_active());
    begin(&mut transcript);
    clear_selection_for_composer_key(
        &mut transcript,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(!transcript.selection.is_active());
}

/// The Ctrl/Cmd+V fallback and the keymap paste share one insert helper,
/// so both land the same buffer for the same clipboard text at and just
/// below the large-paste threshold (#1527).
#[test]
fn clipboard_paste_paths_agree_on_threshold_and_newlines() {
    let composer = Composer::default();
    for payload in [
        "y".repeat(299),
        "z".repeat(300),
        "one\r\ntwo\rthree".to_owned(),
    ] {
        let mut fallback = AppState::new();
        insert_clipboard_paste(&composer, &mut fallback, &payload);

        let mut keymap = AppState::new();
        composer.handle_paste(&mut keymap, &payload);

        assert_eq!(
            fallback.input_buffer, keymap.input_buffer,
            "fallback and keymap paste must insert the same buffer for {payload:?}"
        );
    }
}

#[test]
fn clipboard_paste_below_threshold_inserts_verbatim() {
    let mut state = AppState::new();
    insert_clipboard_paste(&Composer::default(), &mut state, &"y".repeat(299));
    assert_eq!(state.input_buffer, "y".repeat(299));

    let mut large = AppState::new();
    insert_clipboard_paste(&Composer::default(), &mut large, &"y".repeat(300));
    assert!(large.input_buffer.starts_with("<!--PASTE:300:"));
    assert!(large.input_buffer.ends_with("-->"));
}
