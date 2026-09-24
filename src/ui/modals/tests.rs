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
        rememberable_prefix: None,
        forbidden_prefix: None,
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
        rememberable_prefix: None,
        forbidden_prefix: None,
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
        rememberable_prefix: None,
        forbidden_prefix: None,
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
        rememberable_prefix: Some("cargo test".to_owned()),
        forbidden_prefix: Some("cargo test".to_owned()),
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
    assert!(rendered.contains("3. Always allow this exact command"));
    assert!(rendered.contains("4. Always forbid literal tokens `cargo test…`"));
}

#[test]
fn subagent_command_confirmation_keeps_the_reusable_choice_visible() {
    let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
    let mut state = AppState::new();
    state.tool_confirmation_selected = 2;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "agent-1 · run_command".to_owned(),
        path: "cargo test --lib".to_owned(),
        content_preview: String::new(),
        content_bytes: 14,
        rememberable_prefix: Some("cargo test".to_owned()),
        forbidden_prefix: Some("cargo test".to_owned()),
    }]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &state.render_snapshot(), Rect::new(0, 1, 90, 10))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Would you like to run the following command?"));
    assert!(rendered.contains("3. Always allow this exact command"));
    assert!(rendered.contains("4. Always forbid literal tokens `cargo test…`"));
    assert!(rendered.contains("$ cargo test --lib"));
}

#[test]
fn unsafe_allow_commands_can_still_be_forbidden_from_the_confirmation_panel() {
    let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
    let mut state = AppState::new();
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "run_command".to_owned(),
        path: "curl https://example.com".to_owned(),
        content_preview: String::new(),
        content_bytes: 24,
        rememberable_prefix: None,
        forbidden_prefix: Some("curl".to_owned()),
    }]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &state.render_snapshot(), Rect::new(0, 1, 90, 10))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("3. Always forbid literal tokens `curl…`"));
    assert!(!rendered.contains("Always allow"));

    state.move_tool_confirmation_selection(1);
    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 2);
    assert!(matches!(
        approval_event_for_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            2,
            None,
            Some("curl")
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::ForbidAndRemember(prefix)))
            if prefix == "curl"
    ));
}

#[test]
fn approval_selection_reaches_allow_and_forbid_prefix_choices() {
    let mut state = AppState::new();
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "run_command".to_owned(),
        path: "cargo test --lib".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
        rememberable_prefix: Some("cargo test".to_owned()),
        forbidden_prefix: Some("cargo test".to_owned()),
    }]);
    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 1);
    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 2);
    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 3);
    state.move_tool_confirmation_selection(-1);
    assert_eq!(state.tool_confirmation_selected, 2);
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
            rememberable_prefix: None,
            forbidden_prefix: None,
        },
        ToolConfirmation {
            tool_name: "run_command".to_owned(),
            path: "cargo check".to_owned(),
            content_preview: String::new(),
            content_bytes: 11,
            rememberable_prefix: None,
            forbidden_prefix: None,
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
        approval_event_for_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            1,
            None,
            None,
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::Approve))
    ));
    assert!(matches!(
        approval_event_for_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            3,
            Some("cargo test"),
            Some("cargo test"),
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::ForbidAndRemember(prefix)))
            if prefix == "cargo test"
    ));
    assert!(matches!(
        approval_event_for_key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            0,
            None,
            Some("cargo test"),
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::ForbidAndRemember(prefix)))
            if prefix == "cargo test"
    ));
    assert!(matches!(
        approval_event_for_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            1,
            None,
            None,
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::ApproveAll))
    ));
    assert!(matches!(
        approval_event_for_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            1,
            None,
            None
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::Deny))
    ));
    assert!(matches!(
        approval_event_for_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            0,
            None,
            None
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::Deny))
    ));
    assert!(matches!(
        approval_event_for_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            2,
            Some("cargo test"),
            Some("cargo test"),
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::ApproveAndRemember(prefix)))
            if prefix == "cargo test"
    ));
    assert!(matches!(
        approval_event_for_key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            0,
            Some("cargo test"),
            Some("cargo test"),
        ),
        Some(AppEvent::ApprovalDecision(ApprovalDecision::ApproveAndRemember(prefix)))
            if prefix == "cargo test"
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
fn chained_question_modal_shows_position_descriptions_and_nav_hint() {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let mut state = AppState::new();
    state.begin_question_chain(vec![
        PendingQuestion::new(
            "Where from?".to_owned(),
            vec!["CHANGELOG".to_owned(), "API".to_owned()],
            false,
        )
        .with_header("Source".to_owned())
        .with_descriptions(vec!["curated".to_owned(), String::new()]),
        PendingQuestion::new("How many?".to_owned(), vec!["3".to_owned()], false)
            .with_header("Count".to_owned()),
    ]);
    terminal
        .draw(|frame| {
            render_question_modal(frame, &state.render_snapshot(), Rect::new(0, 0, 100, 20))
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(
        rendered.contains("Source · Question 1/2 (2 unanswered)"),
        "chain header missing: {rendered:?}"
    );
    assert!(rendered.contains("Where from?"), "question missing");
    assert!(rendered.contains("CHANGELOG"), "option missing");
    assert!(rendered.contains("curated"), "description missing");
    assert!(
        rendered.contains("tab next"),
        "chain nav hint missing: {rendered:?}"
    );
}

#[test]
fn question_modal_wraps_long_option_and_description_and_keeps_navigation_visible() {
    let mut terminal = Terminal::new(TestBackend::new(44, 10)).unwrap();
    let mut state = AppState::new();
    state.begin_question_chain(vec![
        PendingQuestion::new(
            "Pick one".to_owned(),
            vec!["A long option label that needs to wrap cleanly".to_owned()],
            false,
        )
        .with_descriptions(vec![
            "A long description that should wrap beneath its option label".to_owned(),
        ]),
        PendingQuestion::new("Next?".to_owned(), vec!["Yes".to_owned()], false),
    ]);

    terminal
        .draw(|frame| {
            render_question_modal(frame, &state.render_snapshot(), Rect::new(0, 0, 44, 10))
        })
        .unwrap();

    let rows = (0..10)
        .map(|y| {
            (0..44)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let option_start = rows
        .iter()
        .position(|row| row.contains("› 1. A long option"))
        .expect("first option row should be rendered");
    let footer_start = rows
        .iter()
        .position(|row| row.contains("enter to submit answer"))
        .expect("submit hint should remain visible");
    let option_rows = footer_start.saturating_sub(option_start);

    assert!(
        option_rows >= 3,
        "option and description should wrap: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("enter to submit answer"))
            && rows.iter().any(|row| row.contains("tab next")),
        "submit and chain navigation hints should remain visible: {rows:?}"
    );
}

#[test]
fn question_height_accounts_for_wrapped_options() {
    let mut state = AppState::new();
    state.begin_question_chain(vec![PendingQuestion::new(
        "Pick one".to_owned(),
        vec![
            "A deliberately long option label that wraps across several rows".to_owned(),
            "x".repeat(50),
        ],
        false,
    )]);

    // Header (1), question (1), gap (1), each wrapped option (3), custom option
    // (1), gap (1), compact footer (1), panel padding (2), and trailing space (1).
    assert_eq!(question_height(&state.render_snapshot(), 30, 20), 15);
}

#[test]
fn question_modal_keeps_selected_option_and_footer_on_narrow_terminal() {
    let mut terminal = Terminal::new(TestBackend::new(24, 11)).unwrap();
    let mut state = AppState::new();
    state.begin_question_chain(vec![
        PendingQuestion::new(
            "Choose an option".to_owned(),
            vec!["The selected answer has a long label".to_owned()],
            false,
        ),
        PendingQuestion::new("Next?".to_owned(), vec!["Yes".to_owned()], false),
    ]);

    terminal
        .draw(|frame| {
            render_question_modal(frame, &state.render_snapshot(), Rect::new(0, 0, 24, 11))
        })
        .unwrap();

    let rows = (0..11)
        .map(|y| {
            (0..24)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert!(
        rows.iter().any(|row| row.contains("› 1.")),
        "selected option missing: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("submit"))
            && rows.iter().any(|row| row.contains("tab next"))
            && rows.iter().any(|row| row.contains("⇧tab"))
            && rows.iter().any(|row| row.contains("back")),
        "footer and chain navigation should remain visible: {rows:?}"
    );
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
