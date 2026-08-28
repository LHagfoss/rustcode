use super::*;
use crate::app::ToolConfirmation;
use crate::inline_terminal::InlineTerminal as Terminal;
use ratatui::{backend::TestBackend, layout::Rect};

#[test]
fn single_command_confirmation_uses_codex_command_prompt() {
    let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
    let mut state = AppState::new();
    state.config.theme = "default".to_owned();
    let panel = crate::ui::theme::get_palette(&state.config.theme).panel;
    crate::ui::theme::set_active_theme("nord");
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "run_command".to_string(),
        path: "git commit --message \"hello\"".to_string(),
        content_preview: String::new(),
        content_bytes: 0,
    }]);

    let input_area = Rect::new(0, 2, 100, 10);
    terminal
        .draw(|frame| render_tool_confirmation_modal(frame, &state.render_snapshot(), input_area))
        .unwrap();

    let rendered = (0..14)
        .map(|y| {
            (0..100)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Would you like to run the following command?"));
    assert!(
        rendered.contains("$ git commit --message \"hello\""),
        "rendered modal:\n{rendered}"
    );
    assert!(rendered.contains("› 1. Yes, proceed"));
    assert!(rendered.contains("2. No, cancel this tool call"));

    let buffer = terminal.backend().buffer();
    let command_row = (2..12)
        .find(|y| {
            (0..100)
                .map(|x| buffer[(x, *y)].symbol())
                .collect::<String>()
                .contains("$ git commit --message \"hello\"")
        })
        .expect("dynamic command row");
    assert!((0..100).all(|x| buffer[(x, command_row)].bg == panel));
    let mut command_foregrounds = Vec::new();
    for foreground in (0..100)
        .filter(|x| !buffer[(*x, command_row)].symbol().trim().is_empty())
        .map(|x| buffer[(x, command_row)].fg)
    {
        if !command_foregrounds.contains(&foreground) {
            command_foregrounds.push(foreground);
        }
    }
    assert!(
        command_foregrounds.len() > 1,
        "command should contain syntax colors: {command_foregrounds:?}"
    );
    assert!((0..100).all(|x| buffer[(x, 2)].bg == panel));
    assert!((0..100).all(|x| buffer[(x, 11)].bg == panel));
}

#[test]
fn long_approval_rows_are_clipped_and_keep_the_panel_background() {
    let mut terminal = Terminal::new(TestBackend::new(72, 16)).unwrap();
    let mut state = AppState::new();
    let command = "git log v0.17.0..HEAD --oneline --no-merges; echo ---; git log -3 --oneline; echo ---; git tag --sort=-v:refname | head -5";
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "run_command".to_owned(),
        path: command.to_owned(),
        content_preview: format!(
            "resolved command: {command}\nscope: unclassified or potentially mutating shell command"
        ),
        content_bytes: 0,
    }]);

    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &state.render_snapshot(), Rect::new(0, 1, 72, 14))
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rows = (0..16)
        .map(|y| (0..72).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>();
    assert!(
        rows.iter()
            .any(|row| row.contains("$ git log") && row.contains('…'))
    );
    assert!(!rows.iter().any(|row| row.contains(command)));

    let preview_rows = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains("resolved command:") || row.contains("scope:"))
        .map(|(row, _)| row as u16)
        .collect::<Vec<_>>();
    assert_eq!(preview_rows.len(), 2, "approval rows: {rows:#?}");
    for row in preview_rows {
        let painted_panel = buffer[(71, row)].bg;
        assert!((0..72).all(|x| buffer[(x, row)].bg == painted_panel));
    }
}

#[test]
fn middle_truncation_keeps_command_start_and_tail() {
    assert_eq!(
        truncate_middle_to_width("cargo check --tests", 40),
        "cargo check --tests"
    );
    let clipped = truncate_middle_to_width("git log --oneline; dangerous-command --force", 24);
    assert!(clipped.starts_with("git log"), "clipped command: {clipped}");
    assert!(clipped.ends_with("--force"), "clipped command: {clipped}");
    assert_eq!(clipped.width(), 24);
}

#[test]
fn compact_approval_keeps_heading_and_actions_visible() {
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    let mut state = AppState::new();
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "write_to_file".to_owned(),
        path: "src/main.rs".to_owned(),
        content_preview: "+new line".to_owned(),
        content_bytes: 9,
    }]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &state.render_snapshot(), Rect::new(0, 2, 80, 5))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("Would you like to make the following change?"));
    assert!(rendered.contains("1. Yes, proceed"));
    assert!(rendered.contains("2. No, cancel"));
}

#[test]
fn approval_selection_visibly_moves_to_deny() {
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    let mut state = AppState::new();
    state.tool_confirmation_selected = 1;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "run_command".to_owned(),
        path: "cargo test".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
    }]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &state.render_snapshot(), Rect::new(0, 1, 80, 10))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("› 2. No, cancel this tool call"));
    assert!(!rendered.contains("› 1. Yes, proceed"));
}

#[test]
fn batch_approval_lists_each_tool_in_the_bottom_pane() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = AppState::new();
    state.pending_tool_confirmation = Some(vec![
        ToolConfirmation {
            tool_name: "write_to_file".to_owned(),
            path: "src/one.rs".to_owned(),
            content_preview: String::new(),
            content_bytes: 1,
        },
        ToolConfirmation {
            tool_name: "run_command".to_owned(),
            path: "cargo check".to_owned(),
            content_preview: String::new(),
            content_bytes: 11,
        },
    ]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(
                frame,
                &state.render_snapshot(),
                Rect::new(0, 2, 100, 12),
            )
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("approve these 2 tool calls"));
    assert!(rendered.contains("write_to_file src/one.rs"));
    assert!(rendered.contains("run_command $ cargo check"));
}

#[test]
fn approval_keys_emit_typed_decisions() {
    assert!(matches!(
        approval_event_for_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE), 1),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::Approve))
    ));
    assert!(matches!(
        approval_event_for_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE), 1),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::ApproveAll))
    ));
    assert!(matches!(
        approval_event_for_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 1),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::Deny))
    ));
    assert!(matches!(
        approval_event_for_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 0),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::Deny))
    ));
}

#[test]
fn question_answers_are_typed_without_mutating_the_question() {
    let mut question = PendingQuestion::new(
        "Where?".to_owned(),
        vec!["Here".to_owned(), "There".to_owned()],
        false,
    );
    question.selected = 1;
    assert!(matches!(
        question_answer_event(&question),
        Some(AppEvent::AnswerQuestion(QuestionAnswer::Selected(answer)))
            if answer == "There"
    ));

    question.selected = question.options.len();
    question.activate_custom_input();
    question.insert_str("somewhere");
    assert!(matches!(
        question_custom_answer_event(&question),
        AppEvent::AnswerQuestion(QuestionAnswer::Custom(answer)) if answer == "somewhere"
    ));
    assert_eq!(question.selected, question.options.len());
}

#[test]
fn multi_select_question_answer_joins_selected_options() {
    let mut question = PendingQuestion::new(
        "Which?".to_owned(),
        vec!["one".to_owned(), "two".to_owned()],
        true,
    );
    question.chosen[0] = true;
    question.chosen[1] = true;
    assert!(matches!(
        question_answer_event(&question),
        Some(AppEvent::AnswerQuestion(QuestionAnswer::Selected(answer)))
            if answer == "one, two"
    ));
}

#[test]
fn settings_picker_uses_unified_modal_picker_style() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = AppState::new();
    state.modal_picker_index = 1;
    terminal
        .draw(|frame| {
            render_verbosity_picker_modal(frame, &state.render_snapshot(), Rect::new(0, 12, 100, 3))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Output verbosity"));
    assert!(rendered.contains("● High"));
    assert!(rendered.contains("Pure model text output"));
}

#[test]
fn yolo_picker_renders_options() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = AppState::new();
    state.modal_picker_index = 0;
    terminal
        .draw(|frame| {
            render_yolo_picker_modal(frame, &state.render_snapshot(), Rect::new(0, 12, 100, 3))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Automatic tool confirmation"));
    assert!(rendered.contains("● On"));
    assert!(rendered.contains("Auto-confirm tool executions"));
    assert!(rendered.contains("Off"));
}

#[test]
fn effort_picker_renders_options() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = AppState::new();
    state.modal_picker_index = 0;
    terminal
        .draw(|frame| {
            render_effort_picker_modal(frame, &state.render_snapshot(), Rect::new(0, 12, 100, 3))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Reasoning effort"));
    assert!(rendered.contains("● Low"));
    assert!(rendered.contains("Medium"));
    assert!(rendered.contains("High"));
    assert!(rendered.contains("Off"));
}

#[test]
fn history_picker_renders_borderless_full_width_options() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = AppState::new();
    state.show_history_picker = true;
    state.history_picker_sessions = vec![crate::config::SessionMeta {
        path: std::path::PathBuf::from("/tmp/test-1.json"),
        title: "Build a polished browser tower-defense game with canvas".to_string(),
        message_count: 6,
        when: "17:35".to_string(),
    }];
    state.history_picker_index = 0;
    terminal
        .draw(|frame| {
            render_history_picker_modal(frame, &state.render_snapshot(), Rect::new(0, 12, 100, 3))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Resume session"));
    assert!(rendered.contains("●"));
    assert!(rendered.contains("6 msgs"));
    assert!(rendered.contains("17:35"));
}
