use super::*;
use crate::inline_terminal::InlineTerminal as Terminal;
use crate::ui::render_snapshot::render_snapshot;
use crate::ui::tests::THEME_TEST_LOCK;
use ratatui::{backend::TestBackend, layout::Rect};
use rustcode::controller::ToolConfirmation;

#[test]
fn single_command_confirmation_uses_codex_command_prompt() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
    let mut state = RenderState::new();
    state.config.theme = "default".to_owned();
    let panel = crate::ui::theme::get_palette(&state.config.theme).panel;
    crate::ui::theme::set_active_theme("nord");
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        request_id: None,
        tool_name: "run_command".to_string(),
        path: "git commit --message \"hello\"".to_string(),
        content_preview: String::new(),
        content_bytes: 0,
        rememberable_prefix: None,
        forbidden_prefix: None,
    }]);

    let input_area = Rect::new(0, 2, 100, 10);
    terminal
        .draw(|frame| render_tool_confirmation_modal(frame, &render_snapshot(&state), input_area))
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
    crate::ui::theme::set_active_theme("default");
}

#[test]
fn long_approval_rows_are_clipped_and_keep_the_panel_background() {
    let mut terminal = Terminal::new(TestBackend::new(72, 16)).unwrap();
    let mut state = RenderState::new();
    let command = "git log v0.17.0..HEAD --oneline --no-merges; echo ---; git log -3 --oneline; echo ---; git tag --sort=-v:refname | head -5";
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        request_id: None,
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
            render_tool_confirmation_modal(frame, &render_snapshot(&state), Rect::new(0, 1, 72, 14))
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
    let mut state = RenderState::new();
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        request_id: None,
        tool_name: "write_to_file".to_owned(),
        path: "src/main.rs".to_owned(),
        content_preview: "+new line".to_owned(),
        content_bytes: 9,
        rememberable_prefix: None,
        forbidden_prefix: None,
    }]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &render_snapshot(&state), Rect::new(0, 2, 80, 5))
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
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    let mut state = RenderState::new();
    state.tool_confirmation_selected = 1;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        request_id: None,
        tool_name: "run_command".to_owned(),
        path: "cargo test".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
        rememberable_prefix: Some("cargo test".to_owned()),
        forbidden_prefix: Some("cargo test".to_owned()),
    }]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &render_snapshot(&state), Rect::new(0, 1, 80, 14))
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
    assert!(rendered.contains("3. Always allow plain token prefix `cargo test`"));
    assert!(rendered.contains("4. Always forbid literal tokens `cargo test…`"));
    assert!(
        rendered.contains("Approval controls prompts; OS isolation is separate."),
        "rendered: {rendered:?}"
    );
}

#[test]
fn subagent_command_confirmation_keeps_the_reusable_choice_visible() {
    let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
    let mut state = RenderState::new();
    state.tool_confirmation_selected = 2;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        request_id: None,
        tool_name: "agent-1 · run_command".to_owned(),
        path: "cargo test --lib".to_owned(),
        content_preview: String::new(),
        content_bytes: 14,
        rememberable_prefix: Some("cargo test".to_owned()),
        forbidden_prefix: Some("cargo test".to_owned()),
    }]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &render_snapshot(&state), Rect::new(0, 1, 90, 10))
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
    assert!(rendered.contains("3. Always allow plain token prefix `cargo test`"));
    assert!(rendered.contains("4. Always forbid literal tokens `cargo test…`"));
    assert!(rendered.contains("$ cargo test --lib"));
}

#[test]
fn unsafe_allow_commands_can_still_be_forbidden_from_the_confirmation_panel() {
    let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
    let mut state = RenderState::new();
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        request_id: None,
        tool_name: "run_command".to_owned(),
        path: "curl https://example.com".to_owned(),
        content_preview: String::new(),
        content_bytes: 24,
        rememberable_prefix: None,
        forbidden_prefix: Some("curl".to_owned()),
    }]);
    terminal
        .draw(|frame| {
            render_tool_confirmation_modal(frame, &render_snapshot(&state), Rect::new(0, 1, 90, 10))
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

    state.tool_confirmation_selected = 2;
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
fn batch_approval_lists_each_tool_in_the_bottom_pane() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = RenderState::new();
    state.pending_tool_confirmation = Some(vec![
        ToolConfirmation {
            request_id: None,
            tool_name: "write_to_file".to_owned(),
            path: "src/one.rs".to_owned(),
            content_preview: String::new(),
            content_bytes: 1,
            rememberable_prefix: None,
            forbidden_prefix: None,
        },
        ToolConfirmation {
            request_id: None,
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
                &render_snapshot(&state),
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
    let mut state = RenderState::new();
    state.set_question_chain(vec![
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
            render_question_modal(frame, &render_snapshot(&state), Rect::new(0, 0, 100, 20))
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
    let mut state = RenderState::new();
    state.set_question_chain(vec![
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
            render_question_modal(frame, &render_snapshot(&state), Rect::new(0, 0, 44, 10))
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
    let mut state = RenderState::new();
    state.set_question_chain(vec![PendingQuestion::new(
        "Pick one".to_owned(),
        vec![
            "A deliberately long option label that wraps across several rows".to_owned(),
            "x".repeat(50),
        ],
        false,
    )]);

    // Header (1), question (1), gap (1), each wrapped option (3), custom option
    // (1), gap (1), compact footer (1), panel padding (2), and trailing space (1).
    assert_eq!(question_height(&render_snapshot(&state), 30, 20), 15);
}

#[test]
fn question_modal_keeps_selected_option_and_footer_on_narrow_terminal() {
    let mut terminal = Terminal::new(TestBackend::new(24, 11)).unwrap();
    let mut state = RenderState::new();
    state.set_question_chain(vec![
        PendingQuestion::new(
            "Choose an option".to_owned(),
            vec!["The selected answer has a long label".to_owned()],
            false,
        ),
        PendingQuestion::new("Next?".to_owned(), vec!["Yes".to_owned()], false),
    ]);

    terminal
        .draw(|frame| {
            render_question_modal(frame, &render_snapshot(&state), Rect::new(0, 0, 24, 11))
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
fn question_custom_answer_preserves_spaces_and_explicit_lines() {
    let mut question = PendingQuestion::new("Question?".to_owned(), vec![], false);
    question.activate_custom_input();
    question.custom_input = Some("first  line\n  second line".to_owned());
    question.custom_cursor = question.custom_input.as_ref().unwrap().len();

    let (lines, _, _) = super::question::question_modal_lines(&question, "  Question", 40);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("first  line")));
    assert!(rendered.iter().any(|line| line.contains("  second line")));
}

#[test]
fn question_custom_cursor_wraps_before_a_character_that_moves_to_the_next_row() {
    assert_eq!(
        super::question::question_custom_cursor_position("abc界", 3, 3),
        (1, 0)
    );
}

#[test]
fn settings_picker_uses_unified_modal_picker_style() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = RenderState::new();
    state.modal_picker_index = 1;
    terminal
        .draw(|frame| {
            render_verbosity_picker_modal(frame, &render_snapshot(&state), Rect::new(0, 12, 100, 3))
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
    assert!(rendered.contains("› High"));
    assert!(rendered.contains("Tool calls only"));
}

/// Every inline picker and the scrollable command panels measure themselves with
/// the same rules, so the whole family agrees on width, height floor and column
/// budget (#1528, #1588).
#[test]
fn every_inline_picker_shares_the_measurement_rules() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let panel = COLOR_PANEL();

    // Composer row far enough down that every declared height fits above it.
    const SCREEN_HEIGHT: u16 = 40;
    const INPUT_ROW: u16 = 30;
    // Slack columns to the right of the composer. An inline modal is bounded to
    // the composer width, so a panel that sizes its rows itself would paint into
    // this slack instead of being clipped away by the viewport.
    const SLACK: u16 = 24;

    type Picker = (&'static str, fn(&mut Frame, &RenderSnapshot, Rect), u16);
    let pickers: Vec<Picker> = vec![
        (
            "verbosity",
            render_verbosity_picker_modal,
            VERBOSITY_PICKER_HEIGHT,
        ),
        ("yolo", render_yolo_picker_modal, YOLO_PICKER_HEIGHT),
        ("theme", render_theme_picker_modal, THEME_PICKER_HEIGHT),
        (
            "thinking",
            render_thinking_picker_modal,
            THINKING_PICKER_HEIGHT,
        ),
        ("effort", render_effort_picker_modal, EFFORT_PICKER_HEIGHT),
        (
            "protocol",
            render_protocol_picker_modal,
            PROTOCOL_PICKER_HEIGHT,
        ),
        ("model", render_model_picker_modal, MODEL_PICKER_HEIGHT),
        (
            "history",
            render_history_picker_modal,
            HISTORY_PICKER_HEIGHT,
        ),
        (
            "subagent",
            render_subagent_picker_modal,
            SUBAGENT_PICKER_HEIGHT,
        ),
        (
            "command",
            render_command_picker_modal,
            COMMAND_PICKER_HEIGHT,
        ),
        ("mcp", render_mcp_config_modal, MCP_CONFIG_HEIGHT),
        ("command-panel", render_command_panel, COMMAND_PANEL_HEIGHT),
        ("tasks", render_tasks_panel_modal, TASKS_PANEL_HEIGHT),
    ];

    for (case, composer_width) in [("wide", 100u16), ("narrow", 40u16)] {
        let screen_width = composer_width + SLACK;
        let mut measurements = Vec::new();
        for (name, render, declared_height) in &pickers {
            let mut terminal =
                Terminal::new(TestBackend::new(screen_width, SCREEN_HEIGHT)).unwrap();
            let state = picker_state();
            let snapshot = render_snapshot(&state);
            terminal
                .draw(|frame| render(frame, &snapshot, Rect::new(0, INPUT_ROW, composer_width, 3)))
                .unwrap();

            let buffer = terminal.backend().buffer();
            // The panel background bounds the frame: an inline modal paints its
            // whole rect and nothing outside it.
            let painted = |x: u16, y: u16| buffer[(x, y)].bg == panel;
            let frame_top = (0..INPUT_ROW)
                .find(|y| (0..screen_width).any(|x| painted(x, *y)))
                .unwrap_or_else(|| panic!("{case} {name}: picker painted no frame"));
            let frame_bottom = (frame_top..SCREEN_HEIGHT)
                .rfind(|y| (0..screen_width).any(|x| painted(x, *y)))
                .unwrap_or_else(|| panic!("{case} {name}: picker frame has no bottom row"));
            let left = (0..screen_width)
                .find(|x| (frame_top..=frame_bottom).any(|y| painted(*x, y)))
                .unwrap();
            let right = (0..screen_width)
                .rfind(|x| (frame_top..=frame_bottom).any(|y| painted(*x, y)))
                .unwrap();
            measurements.push((name, right - left + 1, frame_bottom - frame_top + 1));

            assert_eq!(
                left, 0,
                "{case} {name}: picker must be left-aligned with the composer"
            );
            // The composer width bounds every inline modal, so the panels line
            // up with the input box at any viewport width.
            assert_eq!(
                right,
                composer_width - 1,
                "{case} {name}: picker must span the composer width and no more"
            );
            assert!(
                (MIN_MODAL_HEIGHT..=*declared_height).contains(&(frame_bottom - frame_top + 1)),
                "{case} {name}: height must sit in [MIN_MODAL_HEIGHT, {declared_height}]"
            );
            // A picker that sizes its own rows wider than its frame paints past
            // the panel into the slack, which is what the shared column budget
            // and the shared anchor prevent.
            assert!(
                (frame_top..=frame_bottom)
                    .all(|y| (right + 1..screen_width).all(|x| !painted(x, y))),
                "{case} {name}: no row may be wider than the frame"
            );
        }
        let expected_width = measurements[0].1;
        assert!(
            measurements.iter().all(|(_, w, _)| *w == expected_width),
            "{case}: pickers disagree on width: {measurements:?}"
        );
        assert_eq!(
            expected_width, composer_width,
            "{case}: every picker must measure the same width"
        );
    }
}

/// State the pickers need to have something to list. Each picker reads a
/// different slice of the view, so the table seeds all of them.
fn picker_state() -> RenderState {
    let mut state = RenderState::new();
    state.modal_picker_index = 0;
    state.history_picker_index = 0;
    state.subagent_picker_index = 0;
    state.command_picker_index = 0;
    state.mcp_picker_index = 0;
    // A label column wide enough to be truncated at a narrow composer, plus a
    // value long enough to wrap: the command panel must stay inside its frame
    // for both (#1588).
    state.command_panel = Some(rustcode::controller::CommandPanel {
        title: "Memory",
        content: "Keys\n  short  one\n  a-much-longer-label  two\n  mid  a description long enough that it has to wrap somewhere in a narrow frame\n".to_owned(),
    });
    state.tasks_panel = Some(tasks_panel_view(None));
    state.history_picker_sessions = vec![rustcode::controller::SessionMeta {
        path: std::path::PathBuf::from("/tmp/picker-sizing.json"),
        title: "A deliberately long session title that will not fit a narrow frame".to_owned(),
        message_count: 6,
        when: "17:35".to_owned(),
        workspace_cwd: None,
    }];
    state
}

#[test]
fn picker_column_budget_never_exceeds_the_frame() {
    for frame_width in [0usize, 4, 8, 20, 40, 100] {
        for secondary in [0usize, 1, 5, 30, 200] {
            let budget = picker_column_budget(frame_width, secondary);
            assert!(
                budget <= frame_width,
                "budget {budget} exceeds a {frame_width}-column frame"
            );
            assert_eq!(picker_column_budget(frame_width, secondary), budget);
        }
    }
    // `secondary` is the full secondary column, so a frame too narrow for both
    // columns collapses the primary one to zero rather than underflowing.
    assert_eq!(picker_column_budget(4, 200), 0);
}

#[test]
fn picker_list_window_keeps_the_selection_visible() {
    // Everything fits: no scrolling.
    assert_eq!(picker_list_window(3, 2, 10), 0);
    // A window taller than the list still starts at the top.
    assert_eq!(picker_list_window(0, 4, 10), 0);

    for total in [5usize, 20, 100] {
        for list_height in [3usize, 6, 11] {
            for selected in 0..total {
                let scroll = picker_list_window(selected, total, list_height) as usize;
                assert!(
                    scroll <= selected,
                    "the window must not scroll past the selection"
                );
                assert!(
                    selected < scroll + list_height,
                    "the selection must stay inside the {list_height}-row window"
                );
                assert!(
                    scroll + list_height <= total || total <= list_height,
                    "the window must not run past the end of the list"
                );
            }
        }
    }
}

#[test]
fn picker_panel_is_bounded_above_the_composer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let state = RenderState::new();
    terminal
        .draw(|frame| {
            render_verbosity_picker_modal(frame, &render_snapshot(&state), Rect::new(0, 20, 80, 3))
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    // The panel stops at its own max height instead of taking the viewport,
    // so the transcript above it stays visible.
    assert_ne!(buffer[(0, 0)].bg, COLOR_PANEL());
    assert_ne!(buffer[(0, 9)].bg, COLOR_PANEL());
    assert_eq!(buffer[(0, 10)].bg, COLOR_PANEL());
    assert_eq!(buffer[(79, 19)].bg, COLOR_PANEL());
}

#[test]
fn subagent_picker_hints_fit_without_clipping_at_24_40_and_80_columns() {
    let _theme_guard = crate::ui::tests::THEME_TEST_LOCK
        .lock()
        .expect("theme test lock");

    for width in [24u16, 40, 80] {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        let state = picker_state();
        terminal
            .draw(|frame| {
                render_subagent_picker_modal(
                    frame,
                    &render_snapshot(&state),
                    Rect::new(0, 20, width, 3),
                )
            })
            .unwrap();

        let instruction = (0..24)
            .map(|y| {
                (0..width)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
            })
            .find(|line| line.contains("← enter"))
            .expect("picker instruction row");
        assert!(
            unicode_width::UnicodeWidthStr::width(instruction.as_str()) <= usize::from(width),
            "{width}: {instruction:?}"
        );
        if width < 80 {
            assert!(
                !instruction.contains("switch fr"),
                "{width}: {instruction:?}"
            );
        }

        match width {
            24 => {
                assert!(instruction.contains("← enter"), "{instruction:?}");
                assert!(instruction.contains("→ back"), "{instruction:?}");
            }
            40 => assert!(instruction.contains("alt+←/→ switch"), "{instruction:?}"),
            80 => {
                assert!(
                    instruction.contains("switch from the prompt"),
                    "{instruction:?}"
                );
                assert!(
                    instruction.contains("parent history preserved"),
                    "{instruction:?}"
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn picker_panel_never_exceeds_the_space_above_the_composer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let state = RenderState::new();
    // Only six rows sit above the composer, fewer than the panel's max height,
    // so the panel is clamped to what is available instead of overflowing.
    terminal
        .draw(|frame| {
            render_verbosity_picker_modal(frame, &render_snapshot(&state), Rect::new(0, 6, 80, 3))
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].bg, COLOR_PANEL());
    assert_eq!(buffer[(79, 5)].bg, COLOR_PANEL());
}

#[test]
fn yolo_picker_renders_options() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = RenderState::new();
    state.modal_picker_index = 0;
    terminal
        .draw(|frame| {
            render_yolo_picker_modal(frame, &render_snapshot(&state), Rect::new(0, 12, 100, 3))
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
    assert!(rendered.contains("› On"));
    assert!(rendered.contains("Auto-confirm tool executions"));
    assert!(rendered.contains("Off"));
}

#[test]
fn effort_picker_renders_options() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = RenderState::new();
    state.modal_picker_index = 0;
    terminal
        .draw(|frame| {
            render_effort_picker_modal(frame, &render_snapshot(&state), Rect::new(0, 12, 100, 3))
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
    assert!(rendered.contains("› Low"));
    assert!(rendered.contains("Medium"));
    assert!(rendered.contains("High"));
    assert!(rendered.contains("Off"));
}

#[test]
fn history_picker_renders_borderless_full_width_options() {
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let mut state = RenderState::new();
    state.show_history_picker = true;
    state.history_picker_sessions = vec![rustcode::controller::SessionMeta {
        path: std::path::PathBuf::from("/tmp/test-1.json"),
        title: "Build a polished browser tower-defense game with canvas".to_string(),
        message_count: 6,
        when: "17:35".to_string(),
        workspace_cwd: None,
    }];
    state.history_picker_index = 0;
    terminal
        .draw(|frame| {
            render_history_picker_modal(frame, &render_snapshot(&state), Rect::new(0, 12, 100, 3))
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
    assert!(rendered.contains("›"));
    assert!(rendered.contains("6 msgs"));
    assert!(rendered.contains("17:35"));
}

#[test]
fn context_panel_uses_theme_swatches_matching_the_grid() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    crate::ui::theme::set_active_theme("default");
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    let mut state = RenderState::new();
    state.history.push(rustcode::controller::ChatMessage::new(
        "user",
        "Hello assistant",
    ));
    state.history.push(rustcode::controller::ChatMessage::new(
        "assistant",
        "Hello! How can I help you today?",
    ));
    terminal
        .draw(|frame| {
            render_context_modal(frame, &render_snapshot(&state), Rect::new(0, 21, 120, 3))
        })
        .unwrap();
    let rows = (0..24)
        .map(|y| {
            (0..120)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let rendered = rows.join("\n");
    for category in [
        "User messages",
        "Agent responses",
        "Tool calls",
        "System prompt",
        "System tools",
        "Skills",
        "Subagents",
    ] {
        assert!(
            rendered.contains(&format!("● {category}")),
            "legend swatch must match the grid glyph: {rendered:?}"
        );
    }
    assert!(
        !rendered.contains('⛃'),
        "legend must not mix grid glyphs: {rendered:?}"
    );
    assert!(rendered.contains("● "), "grid should render used cells");
    assert!(rendered.contains("□ "), "grid should render free cells");
    crate::ui::theme::set_active_theme("default");
}

#[test]
fn context_panel_emphasizes_over_threshold_categories() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    crate::ui::theme::set_active_theme("default");
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    let mut state = RenderState::new();
    // Tool traffic dominates a small window, pushing "Tool calls" over the
    // over-threshold share while tiny categories stay muted.
    let mut profile = rustcode::controller::ModelProfile::default();
    profile.name = state.model_name.clone();
    profile.model = state.model_name.clone();
    profile.url = state.api_base_url.clone();
    profile.context_window = Some(100_000);
    state.config.models.clear();
    state.config.models.push(profile);
    state.history.push(rustcode::controller::ChatMessage::new(
        "tool",
        "The quick brown fox jumps over the lazy dog. ".repeat(4_400),
    ));
    let breakdown = calculate_context_breakdown(&render_snapshot(&state));
    let tool_pct = breakdown.tool_tokens as f64 / breakdown.context_window.max(1) as f64 * 100.0;
    assert!(
        tool_pct >= super::panel::OVER_THRESHOLD_PCT,
        "tool share should clear the threshold: {tool_pct:.1}%"
    );
    terminal
        .draw(|frame| {
            render_context_modal(frame, &render_snapshot(&state), Rect::new(0, 21, 120, 3))
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let row_of = |needle: &str| {
        (0..24)
            .find(|y| {
                (0..120)
                    .map(|x| buffer[(x, *y)].symbol())
                    .collect::<String>()
                    .contains(needle)
            })
            .unwrap_or_else(|| panic!("missing {needle:?}"))
    };
    let tool_row = row_of("Tool calls");
    assert!(
        (0..120).any(|x| buffer[(x, tool_row)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD)),
        "over-threshold category value should be emphasized"
    );
    let skills_row = row_of("Skills");
    assert!(
        (0..120).all(|x| !buffer[(x, skills_row)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD)),
        "under-threshold category value should stay muted"
    );
    crate::ui::theme::set_active_theme("default");
}

/// The command panel paints its label column through the shared row helper, so
/// the values line up in the frame and not only in the source (#1588).
#[test]
fn command_panel_paints_an_aligned_label_column() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.command_panel = Some(rustcode::controller::CommandPanel {
        title: "Memory",
        content: "  short  one\n  a-much-longer-label  two\n".to_owned(),
    });
    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
    terminal
        .draw(|frame| render_command_panel(frame, &snapshot, Rect::new(0, 12, 60, 2)))
        .unwrap();

    let row = |y: u16| {
        (0..60)
            .map(|column| terminal.backend().buffer()[(column, y)].symbol())
            .collect::<String>()
    };
    let one = row(3).find("one").expect("first value painted");
    let two = row(4).find("two").expect("second value painted");
    assert_eq!(
        one,
        two,
        "both values must start on one column:\n{}\n{}",
        row(3),
        row(4)
    );
}

/// The palette filters with the shared fuzzy rule, so a half-remembered command
/// is still reachable (#1588).
#[test]
fn command_palette_matches_a_mistyped_query() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.command_picker_search = "show ram usge".to_owned();
    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
    terminal
        .draw(|frame| render_command_picker_modal(frame, &snapshot, Rect::new(0, 9, 60, 3)))
        .unwrap();

    let rendered = (0..14)
        .map(|y| {
            (0..60)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Show RAM usage"),
        "the typo must still find the command:\n{rendered}"
    );
}

/// Match marking is per character, so a fuzzy hit explains itself: the query
/// letters stand out and the rest of the row stays in the row's own style
/// (#1588).
#[test]
fn highlight_match_spans_marks_only_the_matched_characters() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    crate::ui::theme::set_active_theme("default");
    let base = Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL());

    let spans = highlight_match_spans("Show RAM usage", "show ram usge", 40, base);
    let marked: String = spans
        .iter()
        .filter(|span| span.style.fg == Some(COLOR_PRIMARY()))
        .map(|span| span.content.as_ref())
        .collect();
    let plain: String = spans.iter().map(|span| span.content.as_ref()).collect();
    assert_eq!(plain, "Show RAM usage", "marking never changes the text");
    assert_eq!(marked, "ShowRAMusge", "only the query letters are marked");
    assert!(
        spans
            .iter()
            .filter(|span| span.style.fg == Some(COLOR_PRIMARY()))
            .all(|span| span.style.add_modifier.contains(Modifier::BOLD)),
        "a matched character is bold"
    );

    // A one-character query would mark the same cell in every row.
    let plain_row = highlight_match_spans("Show RAM usage", "s", 40, base);
    assert!(
        plain_row
            .iter()
            .all(|span| span.style.fg == Some(COLOR_TEXT())),
        "a single character query marks nothing"
    );

    // Truncation keeps a prefix, so the marked positions stay valid.
    let clipped = highlight_match_spans("Show RAM usage", "usge", 6, base);
    let clipped_text: String = clipped.iter().map(|span| span.content.as_ref()).collect();
    assert_eq!(clipped_text, "Show …");
    crate::ui::theme::set_active_theme("default");
}

/// Two running tasks and three finished ones, with the selection on the
/// second running task; `log` swaps the list for that task's output.
fn tasks_panel_view(
    log: Option<rustcode::controller::TaskLogView>,
) -> rustcode::controller::TasksPanelView {
    use rustcode::controller::{TaskOutcome, TaskPanelRow, TaskRowState};
    use std::time::{Duration, Instant};
    let running = |id: &str, command: &str, seconds: u64| TaskPanelRow {
        id: id.to_owned(),
        command: command.to_owned(),
        state: TaskRowState::Running {
            started_at: Instant::now()
                .checked_sub(Duration::from_secs(seconds))
                .expect("monotonic clock is past the test offset"),
        },
    };
    let finished = |id: &str, command: &str, outcome: TaskOutcome| TaskPanelRow {
        id: id.to_owned(),
        command: command.to_owned(),
        state: TaskRowState::Finished {
            outcome,
            ran_for: Duration::from_secs(12),
        },
    };
    rustcode::controller::TasksPanelView {
        rows: vec![
            running(
                "t1",
                "cargo build --release --workspace --all-features --locked",
                125,
            ),
            running("t2", "npm run dev", 5),
            finished("t3", "cargo test", TaskOutcome::Done),
            finished("t4", "make lint", TaskOutcome::Exit(2)),
            finished("t5", "sleep 600", TaskOutcome::Stopped),
        ],
        selected: 1,
        log,
    }
}

fn tasks_panel_text(view: Option<rustcode::controller::TasksPanelView>, width: u16) -> Vec<String> {
    let mut state = RenderState::new();
    state.tasks_panel = view;
    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(width, 22)).unwrap();
    terminal
        .draw(|frame| render_tasks_panel_modal(frame, &snapshot, Rect::new(0, 20, width, 2)))
        .unwrap();
    (0..22)
        .map(|y| {
            (0..width)
                .map(|column| terminal.backend().buffer()[(column, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn tasks_panel_lists_running_then_finished_with_key_hints() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let rows = tasks_panel_text(Some(tasks_panel_view(None)), 80);
    let text = rows.join("\n");
    let row_of = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} is not painted:\n{text}"))
    };

    assert!(text.contains("Tasks · 2 running"), "{text}");
    for hint in ["↑/↓ select", "enter log", "x stop", "esc close"] {
        assert!(text.contains(hint), "missing hint {hint:?}:\n{text}");
    }
    // Running rows carry elapsed time and come before the finished group.
    let build = row_of("cargo build");
    let dev = row_of("npm run dev");
    let heading = row_of("Finished");
    assert!(
        rows[build].contains("• cargo build") && rows[build].contains("2m 05s"),
        "{text}"
    );
    assert!(build < dev && dev < heading, "{text}");
    assert!(
        rows[dev].starts_with("  › • npm run dev"),
        "selection marker: {text}"
    );
    assert!(
        rows[row_of("cargo test")].contains("✓ cargo test · done · 12s"),
        "{text}"
    );
    assert!(
        rows[row_of("make lint")].contains("× make lint · exit 2"),
        "{text}"
    );
    assert!(
        rows[row_of("sleep 600")].contains("− sleep 600 · stopped"),
        "{text}"
    );
    assert!(heading < row_of("cargo test"), "{text}");
    assert!(!text.contains("No tasks are running."), "{text}");

    // Finished rows are dimmed; running rows are not.
    let mut state = RenderState::new();
    state.tasks_panel = Some(tasks_panel_view(None));
    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(80, 22)).unwrap();
    terminal
        .draw(|frame| render_tasks_panel_modal(frame, &snapshot, Rect::new(0, 20, 80, 2)))
        .unwrap();
    let dim = |y: usize| {
        terminal.backend().buffer()[(6, y as u16)]
            .modifier
            .contains(ratatui::style::Modifier::DIM)
    };
    assert!(dim(row_of("cargo test")) && !dim(build), "{text}");
}

#[test]
fn tasks_panel_empty_and_finished_only_states_say_nothing_is_running() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let empty = rustcode::controller::TasksPanelView {
        rows: Vec::new(),
        selected: 0,
        log: None,
    };
    let text = tasks_panel_text(Some(empty), 60).join("\n");
    assert!(text.contains("No tasks are running."), "{text}");
    assert!(text.contains("esc close"), "{text}");
    assert!(!text.contains("Finished"), "{text}");

    let mut finished_only = tasks_panel_view(None);
    finished_only.rows.drain(..2);
    finished_only.selected = 0;
    let text = tasks_panel_text(Some(finished_only), 60).join("\n");
    assert!(text.contains("No tasks are running."), "{text}");
    assert!(text.contains("Finished"), "{text}");
    assert!(text.contains("› ✓ cargo test"), "{text}");

    // Closed: nothing is painted.
    assert!(tasks_panel_text(None, 60).iter().all(String::is_empty));
}

#[test]
fn tasks_panel_keeps_the_selection_in_view_and_inside_a_narrow_frame() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use rustcode::controller::{TaskOutcome, TaskPanelRow, TaskRowState};
    let mut view = tasks_panel_view(None);
    view.rows.extend((0..30).map(|index| TaskPanelRow {
        id: format!("old-{index}"),
        command: format!("job number {index}"),
        state: TaskRowState::Finished {
            outcome: TaskOutcome::Done,
            ran_for: std::time::Duration::from_secs(1),
        },
    }));
    view.selected = view.rows.len() - 1;
    let rows = tasks_panel_text(Some(view), 32);
    let text = rows.join("\n");
    // The last row is the selected one and it is on screen, label shortened
    // to make room for its outcome.
    let last = rows.iter().rfind(|row| !row.is_empty()).expect("rows");
    assert!(last.starts_with("  › ✓ job number"), "{text}");
    assert!(last.ends_with("· done · 1s"), "{text}");
    assert!(
        rows.iter()
            .all(|row| unicode_width::UnicodeWidthStr::width(row.as_str()) <= 32),
        "{text}"
    );
}

#[test]
fn tasks_panel_log_view_shows_the_newest_output_and_how_to_go_back() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let log = |scroll: usize| rustcode::controller::TaskLogView {
        task_id: "t2".to_owned(),
        command: "npm run dev".to_owned(),
        text: (1..=40)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        earlier_omitted: true,
        scroll,
    };
    let text = tasks_panel_text(Some(tasks_panel_view(Some(log(0)))), 80).join("\n");
    assert!(text.contains("Task log · npm run dev"), "{text}");
    for hint in ["↑/↓ scroll", "esc back", "x stop"] {
        assert!(text.contains(hint), "missing hint {hint:?}:\n{text}");
    }
    assert!(text.contains("line 40"), "{text}");
    assert!(!text.contains("line 1\n"), "{text}");
    assert!(
        !text.contains("cargo build"),
        "the list is replaced: {text}"
    );

    // Scrolling up reveals older output and lets go of the newest line.
    let scrolled = tasks_panel_text(Some(tasks_panel_view(Some(log(5)))), 80).join("\n");
    assert!(
        scrolled.contains("line 35") && !scrolled.contains("line 40"),
        "{scrolled}"
    );

    // Scrolled past the top, the omission marker leads the oldest kept line.
    let top = tasks_panel_text(Some(tasks_panel_view(Some(log(1000)))), 80).join("\n");
    assert!(top.contains("… earlier output omitted"), "{top}");
    assert!(top.contains("line 1\n"), "{top}");
}
