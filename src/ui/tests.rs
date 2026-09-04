use super::*;

static THEME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn render_state_to_text(state: &mut AppState, width: u16, height: u16) -> String {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, state)).unwrap();

    (0..height)
        .map(|row| {
            (0..width)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_context_modal_to_text(state: &AppState, width: u16, height: u16) -> String {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::{backend::TestBackend, layout::Rect};

    let snapshot = state.render_snapshot();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            super::modals::render_context_modal(
                frame,
                &snapshot,
                Rect::new(0, height.saturating_sub(3), width, 3),
            );
        })
        .unwrap();

    (0..height)
        .map(|row| {
            (0..width)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_snapshot_to_text(state: &AppState, width: u16, height: u16) -> String {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct CellAppearance {
        foreground: ratatui::style::Color,
        background: ratatui::style::Color,
        modifiers: ratatui::style::Modifier,
    }

    impl From<&ratatui::buffer::Cell> for CellAppearance {
        fn from(cell: &ratatui::buffer::Cell) -> Self {
            Self {
                foreground: cell.fg,
                background: cell.bg,
                modifiers: cell.modifier,
            }
        }
    }

    let snapshot = state.render_snapshot();
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let mut appearances = Vec::new();
    let mut symbol_rows = Vec::with_capacity(height as usize);
    let mut appearance_rows = Vec::with_capacity(height as usize);

    for row in 0..height {
        let mut symbols = String::new();
        let mut row_appearances = Vec::with_capacity(width as usize);

        for column in 0..width {
            let cell = &buffer[(column, row)];
            symbols.push_str(cell.symbol());

            let appearance = CellAppearance::from(cell);
            let appearance_id = appearances
                .iter()
                .position(|existing| *existing == appearance)
                .unwrap_or_else(|| {
                    appearances.push(appearance);
                    appearances.len() - 1
                });
            row_appearances.push(format!("{appearance_id:02}"));
        }

        symbol_rows.push(symbols);
        appearance_rows.push(row_appearances.join(" "));
    }

    let appearance_palette = appearances
        .iter()
        .enumerate()
        .map(|(id, appearance)| {
            format!(
                "{id:02}: fg={:?}, bg={:?}, modifiers={:?}",
                appearance.foreground, appearance.background, appearance.modifiers
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "{}\n\ncell appearances:\n{}\n\nappearance palette:\n{}",
        symbol_rows.join("\n"),
        appearance_rows.join("\n"),
        appearance_palette
    )
}

#[test]
fn render_snapshot_preserves_existing_ui_output() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut states = Vec::new();

    states.push(AppState::new());

    let mut streaming = AppState::new();
    streaming.history.push(ChatMessage::new("user", "hello"));
    streaming.status = AppStatus::Streaming;
    streaming.replace_current_response("streamed output");
    states.push(streaming);

    let mut approval = AppState::new();
    approval.status = AppStatus::AwaitingToolConfirmation;
    approval.pending_tool_confirmation = Some(vec![crate::app::ToolConfirmation {
        tool_name: "run_command".to_owned(),
        path: "cargo test".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
    }]);
    states.push(approval);

    let mut question = AppState::new();
    question.status = AppStatus::AwaitingQuestion;
    question.pending_question = Some(crate::app::PendingQuestion::new(
        "Proceed?".to_owned(),
        vec!["Yes".to_owned(), "No".to_owned()],
        false,
    ));
    states.push(question);

    let mut picker = AppState::new();
    picker.show_model_picker = true;
    states.push(picker);

    let mut selected_subagent = AppState::new();
    selected_subagent.subagents.push(crate::app::SubAgent {
        id: 7,
        name: "reviewer".to_owned(),
        task: "review the patch".to_owned(),
        model: Some("test-model".to_owned()),
        history: std::sync::Arc::new(vec![ChatMessage::new("assistant", "subagent response")]),
        status: crate::app::SubAgentStatus::Running,
        active_turn: true,
        parent_id: Some(3),
        write_access: false,
        allowed_paths: Vec::new(),
        verification_command: None,
        workspace_root: None,
        review_manifest: None,
    });
    selected_subagent.selected_subagent_id = Some(7);
    states.push(selected_subagent);

    for state in &mut states {
        state.config = crate::config::AppConfig::default();
        state.model_name = "gemini-3.6-flash".to_owned();
        state.api_base_url = "http://localhost:3000/v1/chat/completions".to_owned();
        state.cwd_and_branch = "/repo:main".to_owned();
    }
    theme::set_active_theme("default");

    let appearance_oracle = render_snapshot_to_text(&states[0], 1, 1);
    assert!(
        appearance_oracle.contains("fg=")
            && appearance_oracle.contains("bg=")
            && appearance_oracle.contains("modifiers="),
        "render oracle must include complete cell appearance"
    );

    // These fixtures are the independent rendering oracle: changing the
    // snapshot renderer changes a terminal cell and fails this test.
    fn expand_golden_fixture(fixture: &str) -> String {
        fixture
            .trim_end()
            .replace('␠', " ")
            .replace("v0.00.0", concat!("v", env!("CARGO_PKG_VERSION")))
    }

    let golden_outputs = [
        expand_golden_fixture(include_str!("fixtures/render_snapshot_0.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_1.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_2.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_3.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_4.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_5.txt")),
    ];

    for (index, state) in states.into_iter().enumerate() {
        let actual = render_snapshot_to_text(&state, 60, 16);
        assert_eq!(
            actual, golden_outputs[index],
            "render case {index} diverged"
        );
    }
}

#[test]
fn acceptance_empty_session_has_welcome_and_composer() {
    let mut state = AppState::new();
    let rendered = render_state_to_text(&mut state, 100, 28);

    assert!(
        rendered.contains(">_ RustCode") && rendered.contains("model:"),
        "rendered: {rendered:?}"
    );
    assert!(
        rendered.contains("Ask RustCode to do anything"),
        "rendered: {rendered:?}"
    );
}

#[test]
fn acceptance_streaming_session_has_working_surface_and_live_text() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    state.status = AppStatus::Streaming;
    state.replace_current_response("streamed output");

    let rendered = render_state_to_text(&mut state, 100, 20);

    assert!(rendered.contains("Working"), "rendered: {rendered:?}");
    assert!(
        rendered.contains("streamed output"),
        "rendered: {rendered:?}"
    );
}

#[test]
fn acceptance_tool_confirmation_replaces_composer_with_actions() {
    use crate::app::ToolConfirmation;

    let mut state = AppState::new();
    state.status = AppStatus::AwaitingToolConfirmation;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "run_command".to_owned(),
        path: "cargo test".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
    }]);

    let rendered = render_state_to_text(&mut state, 100, 14);

    assert!(
        rendered.contains("Would you like to run the following command?"),
        "rendered: {rendered:?}"
    );
    assert!(rendered.contains("$ cargo test"), "rendered: {rendered:?}");
    assert!(
        rendered.contains("Press enter to confirm"),
        "rendered: {rendered:?}"
    );
}

#[test]
fn acceptance_narrow_terminal_keeps_the_composer_visible() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    let rendered = render_state_to_text(&mut state, 48, 8);

    assert!(rendered.contains("Ask RustCode"), "rendered: {rendered:?}");
}

#[test]
fn desired_height_keeps_an_idle_conversation_compact() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    let mut transcript = TranscriptState::default();

    assert_eq!(super::desired_height(&state, &mut transcript, 100, 40), 6);
}

#[test]
fn desired_height_grows_with_streaming_text_and_clamps_to_terminal() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    state.status = AppStatus::Streaming;
    state.replace_current_response(
        (0..50)
            .map(|line| format!("streamed line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let mut transcript = TranscriptState::default();

    let height = super::desired_height(&state, &mut transcript, 40, 18);
    assert!(height > 6);
    assert_eq!(height, 18);
}

#[test]
fn model_picker_keeps_multiple_models_visible_above_the_composer() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.config.models = (1..=5)
        .map(|number| crate::config::ModelProfile {
            name: format!("model-{number}"),
            url: format!("http://localhost/{number}"),
            model: format!("model-{number}"),
            context_window: None,
            engine: Some("Local".to_owned()),
            api_key: None,
            env_key: None,
            tool_protocol: None,
            enable_thinking: None,
            reasoning_effort: None,
            max_tokens: None,
            supports_vision: None,
            ..Default::default()
        })
        .collect();
    state.show_model_picker = true;

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal.draw(|frame| render(frame, &mut state)).unwrap();

    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    let visible_models = (1..=5)
        .filter(|number| rendered.contains(&format!("model-{number}")))
        .count();

    assert!(
        visible_models >= 3,
        "the inline picker must show several choices, got {visible_models}: {rendered:?}"
    );
}

#[test]
fn command_picker_keeps_multiple_commands_visible_above_the_composer() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.show_command_picker = true;

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal.draw(|frame| render(frame, &mut state)).unwrap();

    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    let visible_commands = ["New session", "Fork session", "Archive session"]
        .iter()
        .filter(|command| rendered.contains(**command))
        .count();

    assert_eq!(
        visible_commands, 3,
        "the inline picker must show its first three commands: {rendered:?}"
    );
}

#[test]
fn inline_command_suggestions_render_below_the_composer() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.input_buffer = "/".to_owned();
    state.cursor_position = 1;
    state.active_suggestion_index = Some(0);

    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal.draw(|frame| render(frame, &mut state)).unwrap();

    let buffer = terminal.backend().buffer();
    let row_text = |row: u16| {
        (0..100)
            .map(|column| buffer[(column, row)].symbol())
            .collect::<String>()
    };
    let composer_row = (0..20)
        .find(|row| row_text(*row).contains("/"))
        .expect("composer input row should be visible");
    let popup_row = (0..20)
        .find(|row| row_text(*row).contains("/cancel"))
        .expect("inline command popup should be visible");

    assert!(
        popup_row > composer_row,
        "popup should be below the composer: composer={composer_row}, popup={popup_row}"
    );
    assert!(
        !(0..20).any(|row| row_text(row).contains("context left")),
        "the footer should be hidden while completions are visible"
    );
}

#[test]
fn welcome_banner_renders_without_a_conversation() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut state)).unwrap();

    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(
        rendered.contains("model:")
            && rendered.contains("effort:")
            && rendered.contains("context:")
            && rendered.contains("directory:"),
        "the empty chat must display its welcome banner: {rendered:?}"
    );
    assert!(
        rendered.contains(">_ RustCode"),
        "the welcome banner header must include '>_ RustCode': {rendered:?}"
    );
    assert!(
        rendered.contains("branch:") && rendered.contains("help:") && rendered.contains("/help"),
        "the welcome banner must include branch and help rows: {rendered:?}"
    );
}

#[test]
fn welcome_banner_shows_active_model_effort_and_context_window() {
    let mut state = AppState::new();
    state.api_base_url = "http://localhost/test".to_string();
    state.model_name = "test-model".to_string();
    state.config.models = vec![crate::config::ModelProfile {
        name: "test-profile".to_string(),
        url: state.api_base_url.clone(),
        model: state.model_name.clone(),
        context_window: Some(128_000),
        reasoning_effort: Some("high".to_string()),
        ..Default::default()
    }];

    let lines = super::build_claude_startup_banner(&state, 100, 28);
    let rendered = lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("effort:      high"),
        "rendered: {rendered:?}"
    );
    assert!(
        rendered.contains("context:     128.0K tokens"),
        "rendered: {rendered:?}"
    );
    assert!(rendered.contains("/context to change"));
}

#[test]
fn welcome_banner_includes_padding_below() {
    let state = AppState::new();
    let lines = super::render_live_tail(&state, 100, 28);
    assert!(!lines.is_empty());
    // The last line should be empty padding below the banner box
    let last = &lines[lines.len() - 1];
    assert!(
        last.spans.is_empty() || last.spans.iter().all(|s| s.content.trim().is_empty()),
        "welcome banner must end with a blank padding line"
    );
}

#[test]
fn welcome_banner_includes_padding_before_bottom_border() {
    let state = AppState::new();
    let lines = super::render_live_tail(&state, 100, 28);
    let bottom_border = lines
        .iter()
        .position(|line| line.to_string().contains('╰'))
        .expect("welcome banner must include a bottom border");
    assert!(bottom_border > 0);
    assert!(
        lines[bottom_border - 1]
            .to_string()
            .trim_matches('│')
            .trim()
            .is_empty(),
        "welcome banner must have a blank line before its bottom border"
    );
}

#[test]
fn welcome_banner_adapts_to_small_viewports_without_truncating_box() {
    let state = AppState::new();
    // Test with small height = 6
    let lines = super::render_live_tail(&state, 100, 6);
    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("model:"));
    assert!(
        text.contains("╰"),
        "banner must end cleanly with a bottom border"
    );
}

#[test]
fn inline_notice_finishes_the_welcome_cell_and_compacts_the_viewport() {
    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("system", "YOLO mode enabled"));

    let lines = super::render_live_tail(&state, 100, 28);
    assert!(
        !lines
            .iter()
            .any(|line| line.to_string().contains("directory:")),
        "the welcome banner must not be rendered again after a transcript notice"
    );

    let mut transcript = TranscriptState::default();
    assert_eq!(super::desired_height(&state, &mut transcript, 100, 40), 6);
}

#[test]
fn queue_preview_shows_recent_user_prompts_without_wakeups() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.pending_queue = vec![
        "first prompt".to_owned(),
        "second prompt".to_owned(),
        "third prompt".to_owned(),
        "fourth prompt".to_owned(),
        "__task_wakeup__:task-123".to_owned(),
    ];

    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    terminal.draw(|frame| render(frame, &mut state)).unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(rendered.contains("queued (4) · ↑ edit last"));
    assert!(rendered.contains("second prompt"));
    assert!(rendered.contains("third prompt"));
    assert!(rendered.contains("fourth prompt"));
    assert!(!rendered.contains("first prompt"));
    assert!(!rendered.contains("__task_wakeup__"));
}

// Regression: the tool-result cache used to `clear()` the whole map at the
// cap, throwing away every still-visible result and forcing a full
// re-render on the next frame. It now drops a single cold entry.
#[test]
fn tool_result_cache_evicts_one_lru_entry_at_cap() {
    use super::{
        TOOL_RESULT_CACHE, TOOL_RESULT_CACHE_CAP, cached_tool_result, tool_result_cache_key,
    };

    let cap = TOOL_RESULT_CACHE_CAP;
    let verbosity = crate::app::Verbosity::Low;
    for i in 0..cap {
        cached_tool_result("Bash", &format!("result {i}"), 80, &verbosity, false);
    }
    TOOL_RESULT_CACHE.with(|cache| assert_eq!(cache.borrow().entries.len(), cap));

    // Read the oldest entry so it becomes the most recently used one; a hit
    // must refresh recency.
    let oldest = tool_result_cache_key("Bash", "result 0", 80, &verbosity, false);
    cached_tool_result("Bash", "result 0", 80, &verbosity, false);

    // Exceed the cap by one: exactly one entry is evicted, and it is the
    // least recently used one rather than the entry just read.
    cached_tool_result("Bash", "overflow", 80, &verbosity, false);
    TOOL_RESULT_CACHE.with(|cache| {
        let cache = cache.borrow();
        assert_eq!(cache.entries.len(), cap, "cap must hold after overflow");
        assert!(
            cache.entries.contains_key(&oldest),
            "entry read just before the insert must survive"
        );
        assert!(
            !cache.entries.contains_key(&tool_result_cache_key(
                "Bash", "result 1", 80, &verbosity, false
            )),
            "the least recently used entry is the eviction victim"
        );
    });
}

#[test]
fn theme_change_changes_cache_keys() {
    use super::{theme, tool_result_cache_key};

    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let verbosity = crate::app::Verbosity::Low;
    theme::set_active_theme("default");
    let key1 = tool_result_cache_key("Bash", "result 0", 80, &verbosity, false);
    theme::set_active_theme("nord");
    let key2 = tool_result_cache_key("Bash", "result 0", 80, &verbosity, false);

    assert_ne!(
        key1, key2,
        "cache key must differ when active theme changes"
    );
    theme::set_active_theme("default");
}

#[test]
fn sky_theme_loads_and_updates_syntax_highlighting() {
    use super::theme;

    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let sky = theme::get_palette("sky");
    assert_eq!(sky.name, "sky");
    assert_eq!(sky.primary, ratatui::style::Color::Rgb(56, 148, 240));
    assert_eq!(sky.secondary, ratatui::style::Color::Rgb(136, 196, 56));
    assert_eq!(sky.panel, ratatui::style::Color::Rgb(22, 32, 50));

    theme::set_active_theme("sky");
    assert_eq!(
        super::COLOR_PRIMARY(),
        ratatui::style::Color::Rgb(56, 148, 240)
    );
    assert_eq!(
        super::COLOR_SECONDARY(),
        ratatui::style::Color::Rgb(136, 196, 56)
    );

    let spans = super::highlight_code_line("let x = 42;", "rust", false);
    assert!(!spans.is_empty());

    // Restore default theme
    theme::set_active_theme("default");
}

#[test]
fn custom_tools_render_pascalcase_with_param() {
    use super::{format_pi_tool_action, to_pascal_case};

    assert_eq!(to_pascal_case("use_skill"), "UseSkill");
    assert_eq!(to_pascal_case("complete_task"), "CompleteTask");
    assert_eq!(to_pascal_case("git-feature-workflow"), "GitFeatureWorkflow");

    let (label, arg) = format_pi_tool_action(
        "use_skill",
        &serde_json::json!({"name": "git-feature-workflow"}),
        None,
    );
    assert_eq!(label, "UseSkill");
    assert_eq!(arg, "git-feature-workflow");

    let (label, arg) = format_pi_tool_action(
        "complete_task",
        &serde_json::json!({"result": "done"}),
        None,
    );
    assert_eq!(label, "CompleteTask");
    assert_eq!(arg, "result=\"done\"");

    let (label, arg) = format_pi_tool_action("complete_task", &serde_json::json!({}), None);
    assert_eq!(label, "CompleteTask");
    assert_eq!(arg, "");

    // Built-in aliases are unchanged.
    let (label, _) =
        format_pi_tool_action("run_command", &serde_json::json!({"command": "ls"}), None);
    assert_eq!(label, "Bash");
}

#[test]
fn tool_path_formatting_uses_captured_snapshot_home() {
    let state = AppState::new();
    let snapshot = state.render_snapshot();
    let Some(home) = snapshot.home_path() else {
        return;
    };
    let path = format!("{home}/project/file.rs");

    let (_, rendered) = format_pi_tool_action(
        "view_file",
        &serde_json::json!({"path": path}),
        snapshot.home_path(),
    );

    assert_eq!(rendered, "~/project/file.rs");
}

#[test]
fn persisted_edit_result_resolves_tool_name_without_previous_call() {
    let result = "replace_file_content: successfully replaced target_content\n\n```diff\n@@ -1 +1 @@\n-old\n+new\n```";
    let tool_name = super::resolve_tool_result_name(None, Some("replace_file_content"), result);

    assert_eq!(tool_name.as_deref(), Some("replace_file_content"));
    assert!(
        super::render_tool_result(
            tool_name.as_deref().unwrap(),
            result.strip_prefix("replace_file_content: ").unwrap(),
            80,
            &crate::app::Verbosity::Low,
            false,
        )
        .iter()
        .any(|line| line.spans.iter().any(|span| span.content.contains("new")))
    );
}

#[test]
fn committed_tool_result_shows_action_status_and_indented_output() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"cargo test"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0\n504 passed")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: Some(0),
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("• Ran $ cargo test · exit 0"))
    );
    assert!(
        rendered.iter().any(|line| line.contains("504 passed")),
        "command output must be rendered beneath its header: {rendered:?}"
    );
}

#[test]
fn committed_tool_result_shows_failure_status() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"cargo test"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "run_command: exit code: 1\nstderr:\npermission denied",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            arguments_hash: String::new(),
            success: false,
            exit_code: Some(1),
            changed_paths: Vec::new(),
            truncated: false,
            full_output_artifact: None,
            ..Default::default()
        }),
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("• Ran $ cargo test · exit 1"))
    );
}

#[test]
fn use_skill_renders_in_committed_history() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "use_skill".to_owned(),
            arguments: r#"{"name":"clockify"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "use_skill: <skill_content>...</skill_content>")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "use_skill".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Ran");
    assert!(rendered.iter().any(|line| line == "  └ UseSkill clockify"));
}

#[test]
fn high_verbosity_keeps_tool_call_summaries_visible() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::High;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "use_skill".to_owned(),
            arguments: r#"{"name":"clockify"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "use_skill: loaded clockify")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "use_skill".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Ran", "  └ UseSkill clockify"]);
}

#[test]
fn completed_generic_tool_uses_ran_heading_and_indented_child() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "get_time".to_owned(),
            arguments: "{}".to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "get_time: Thursday, 08:30")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "get_time".to_owned(),
                success: true,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Ran", "  └ GetTime"]);
}

#[test]
fn high_verbosity_batches_consecutive_commands_under_one_heading() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::High;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"git status --short"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"cargo check --tests"}"#.to_owned(),
            },
        ]));
    for (id, command) in [
        ("call-1", "git status --short"),
        ("call-2", "cargo check --tests"),
    ] {
        state.history.push(
            ChatMessage::new(
                "tool",
                format!("run_command: exit code: 0\n{command} output"),
            )
            .answering(Some(id.to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }),
        );
    }

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "• Ran",
            "  └ Bash git status --short",
            "    Bash cargo check --tests"
        ]
    );
    assert!(!rendered.iter().any(|line| line.contains("output")));
}

#[test]
fn high_verbosity_keeps_mixed_provider_batch_under_one_ran_heading() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::High;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "get_time".to_owned(),
                arguments: "{}".to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"git status --short"}"#.to_owned(),
            },
        ]));
    state.history.push(
        ChatMessage::new("tool", "get_time: Thursday, 08:30")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "get_time".to_owned(),
                success: true,
                ..Default::default()
            }),
    );
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        ["• Ran", "  └ GetTime", "    Bash git status --short"]
    );
}

#[test]
fn worked_separator_only_labels_concrete_work_over_one_minute() {
    use crate::app::{ChatMessage, ToolResultRecord};

    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "fix it"));
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0").with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            success: true,
            ..Default::default()
        }),
    );
    let mut assistant = ChatMessage::new("assistant", "Done.");
    assistant.response_time_ms = Some(125_000);
    state.history.push(assistant);

    let separator = super::render_work_separator_before_assistant(&state, 2, 80);
    assert_eq!(separator.len(), 2);
    assert!(
        separator[0]
            .to_string()
            .starts_with("─ Worked for 2m 05s ─")
    );
    assert!(separator[1].to_string().is_empty());

    state.history[2].response_time_ms = Some(12_000);
    assert_eq!(
        super::render_work_separator_before_assistant(&state, 2, 12)[0].to_string(),
        "────────────"
    );
    assert!(super::render_work_separator_before_assistant(&state, 0, 80).is_empty());
}

#[test]
fn work_separator_follows_tool_with_padding_gap() {
    use crate::app::{ChatMessage, ToolResultRecord};

    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "explore"));
    state.history.push(
        ChatMessage::new("tool", "view_file: read main.rs").with_tool_result(ToolResultRecord {
            tool_name: "view_file".to_owned(),
            success: true,
            ..Default::default()
        }),
    );
    let mut assistant = ChatMessage::new("assistant", "Found it.");
    assistant.response_time_ms = Some(254_000);
    state.history.push(assistant);

    let separator = super::render_work_separator_before_assistant(&state, 2, 80);
    assert_eq!(separator.len(), 2);
    assert!(
        separator[0]
            .to_string()
            .starts_with("─ Worked for 4m 14s ─")
    );
    assert_eq!(separator[1].to_string(), "");
}

#[test]
fn high_verbosity_hides_generic_tool_details() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::High;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "mcp_custom_tool".to_owned(),
            arguments: r#"{"path":"src"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "mcp_custom_tool: completed\nline 1\nline 2")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "mcp_custom_tool".to_owned(),
                arguments_hash: String::new(),
                success: true,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Ran");
    assert!(
        rendered
            .iter()
            .any(|line| line.starts_with("  └ McpCustomTool"))
    );
    assert!(rendered.iter().any(|line| line.contains("McpCustomTool")));
    assert!(!rendered.iter().any(|line| line.contains("completed")));
    assert!(!rendered.iter().any(|line| line.contains("line 2")));
    assert!(!rendered.iter().any(|line| line.contains("ctrl+o")));
}

#[test]
fn high_verbosity_collapses_tool_output_without_mutating_history() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "mcp_custom_tool".to_owned(),
            arguments: r#"{"path":"src"}"#.to_owned(),
        }]),
    );
    let body = (0..50)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    state.history.push(
        ChatMessage::new("tool", format!("mcp_custom_tool: completed\n{body}"))
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "mcp_custom_tool".to_owned(),
                success: true,
                ..Default::default()
            }),
    );
    let history = state.history.clone();

    state.verbosity = Verbosity::Low;
    let low = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    state.verbosity = Verbosity::High;
    let high = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(!low.iter().any(|line| line.contains("line 25")));
    assert!(!low.iter().any(|line| line.contains("line 49")));
    assert!(!high.iter().any(|line| line.contains("line 49")));
    assert!(!high.iter().any(|line| line.contains("… +31 lines")));
    assert!(!high.iter().any(|line| line.contains("line 25")));
    assert!(low.iter().any(|line| line.contains("ctrl+o to expand")));
    assert!(!high.iter().any(|line| line.contains("ctrl+o to expand")));
    assert!(state.history == history);
}

#[test]
fn default_verbosity_is_high() {
    assert_eq!(
        crate::app::Verbosity::default(),
        crate::app::Verbosity::High
    );
}

#[test]
fn completed_edits_have_a_distinct_transcript_heading() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "replace_file_content".to_owned(),
            arguments: r#"{"path":"src/main.rs"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "replace_file_content: successfully edited")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "replace_file_content".to_owned(),
                arguments_hash: String::new(),
                success: true,
                changed_paths: vec!["src/main.rs".to_owned()],
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Edited");
    assert_eq!(rendered[1], "  └ src/main.rs");
}

#[test]
fn committed_batched_edits_with_casing_aliases_group_under_edited() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "replace_file_content".to_owned(),
                arguments: r#"{"TargetFile":"src/game/engine.ts"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "WriteFile".to_owned(),
                arguments: r#"{"path":"src/App.tsx"}"#.to_owned(),
            },
        ]));
    state.history.push(
        ChatMessage::new("tool", "replace_file_content: ok")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "replace_file_content".to_owned(),
                success: true,
                changed_paths: vec!["src/game/engine.ts".to_owned()],
                ..Default::default()
            }),
    );
    state.history.push(
        ChatMessage::new("tool", "WriteFile: ok")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "WriteFile".to_owned(),
                success: true,
                changed_paths: vec!["src/App.tsx".to_owned()],
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Edited");
    assert_eq!(rendered[1], "  └ src/game/engine.ts");
    assert_eq!(rendered[2], "    src/App.tsx");
}

#[test]
fn exploration_results_group_and_deduplicate_child_rows() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-list-1".to_owned(),
                name: "list_directory".to_owned(),
                arguments: r#"{"path":"src"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-search".to_owned(),
                name: "grep".to_owned(),
                arguments: r#"{"pattern":"renderer","path":"src"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-list-2".to_owned(),
                name: "list_directory".to_owned(),
                arguments: r#"{"path":"src"}"#.to_owned(),
            },
        ]));
    for (id, name, content) in [
        ("call-list-1", "list_directory", "list_directory: ui/"),
        ("call-search", "grep", "grep: src/ui/mod.rs:1"),
        ("call-list-2", "list_directory", "list_directory: ui/"),
    ] {
        state.history.push(
            ChatMessage::new("tool", content)
                .answering(Some(id.to_owned()))
                .with_tool_result(ToolResultRecord {
                    tool_name: name.to_owned(),
                    arguments_hash: String::new(),
                    success: true,
                    exit_code: None,
                    changed_paths: Vec::new(),
                    truncated: false,
                    full_output_artifact: None,
                    ..Default::default()
                }),
        );
    }

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2, 3], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Explored");
    assert_eq!(
        rendered
            .iter()
            .filter(|line| *line == "  └ List src")
            .count(),
        1
    );
    assert!(
        rendered
            .iter()
            .any(|line| line == "    Search renderer in src")
    );
}

#[test]
fn exploration_results_match_repeated_calls_without_ids_in_order() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "unused-1".to_owned(),
                name: "list_directory".to_owned(),
                arguments: r#"{"path":"src"}"#.to_owned(),
            },
            ToolCallRef {
                id: "unused-2".to_owned(),
                name: "list_directory".to_owned(),
                arguments: r#"{"path":"tests"}"#.to_owned(),
            },
        ]));
    for content in ["list_directory: ui/", "list_directory: fixtures/"] {
        state.history.push(
            ChatMessage::new("tool", content).with_tool_result(ToolResultRecord {
                tool_name: "list_directory".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
        );
    }

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line == "  └ List src"));
    assert!(rendered.iter().any(|line| line == "    List tests"));
}

#[test]
fn command_preview_preserves_the_output_tail() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"cargo test"}"#.to_owned(),
        }]),
    );
    let body = (0..20)
        .map(|index| format!("line {index}"))
        .chain(std::iter::once("error: build failed".to_owned()))
        .collect::<Vec<_>>()
        .join("\n");
    state.history.push(
        ChatMessage::new(
            "tool",
            format!("run_command: exit code: 1\nstderr:\n{body}"),
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            arguments_hash: String::new(),
            success: false,
            exit_code: Some(1),
            changed_paths: Vec::new(),
            truncated: false,
            full_output_artifact: None,
            ..Default::default()
        }),
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("… +")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("error: build failed"))
    );
}

#[test]
fn expanded_generic_tool_preserves_its_result_body() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "custom_lookup".to_owned(),
            arguments: r#"{"query":"renderer"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "custom_lookup: first result\nsecond result")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "custom_lookup".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );
    state.expanded_thoughts.insert(1);

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("first result")));
    assert!(rendered.iter().any(|line| line.contains("second result")));
}

#[test]
fn collapses_image_markers_to_chips() {
    // Plain text is untouched.
    assert_eq!(collapse_image_markers("hello world"), "hello world");

    // A single marker becomes a numbered chip, surrounding text preserved.
    assert_eq!(
        collapse_image_markers("look ![image](file:///tmp/a.png) here"),
        "look [Image #1] here"
    );

    // Multiple markers increment.
    assert_eq!(
        collapse_image_markers("![image](file:///tmp/a.png)![image](file:///tmp/b.png)"),
        "[Image #1][Image #2]"
    );

    // Unclosed marker (mid-paste) is left as-is from the marker onward.
    let unclosed = "text ![image](file:///tmp/a";
    assert_eq!(collapse_image_markers(unclosed), unclosed);
}

#[test]
fn pasted_image_and_text_chips_use_accent_text() {
    let mut state = AppState::new();
    state.history.push(ChatMessage::new(
        "user",
        "see ![image](file:///tmp/a.png) and <!--PASTE:12:pasted text-->",
    ));

    let block = super::render_committed_history_block(&state, 0, 100);
    let marker_text = block
        .iter()
        .flat_map(|line| line.spans.iter())
        .filter(|span| span.style.fg == Some(super::COLOR_PRIMARY()))
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(marker_text.contains("[Image #1]"));
    assert!(marker_text.contains("[Pasted Text #1 (12 chars)]"));
    assert!(
        block
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| {
                span.style.fg == Some(super::COLOR_PRIMARY())
                    && span.style.add_modifier.contains(Modifier::BOLD)
            })
            .count()
            > 1
    );
}

#[test]
fn code_blocks_render_as_lightweight_transcript_rows() {
    use super::{AssistantRenderOptions, render_assistant_message};
    let content = "```text\nWhy Rust Outshines C#\n\nA short line\n```";
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    let width: u16 = 80;
    render_assistant_message(
        content,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: width,
            show_picker: false,
            last_copy_text: None,
        },
    );

    // The body remains copyable without a language/copy header or full-width panel.
    assert_eq!(copies.len(), 1);
    let rendered = lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Why Rust Outshines C#"));
    assert!(rendered.contains("A short line"));
    assert!(!rendered.contains("Copy 📋"));
    assert!(lines.iter().all(|line| line.width() < width as usize));
}

#[test]
fn streamed_markdown_fences_keep_adversarial_content_in_the_code_cell() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let streaming = concat!(
        "Before\n\n",
        "````rust\n",
        "let marker = \"```\";\n",
        "```\n",
        "let still_code = true;\n"
    );
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        streaming,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: true,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );
    let streaming_text: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(streaming_text.contains("let marker = \"```\";"));
    assert!(streaming_text.contains("let still_code = true;"));

    let completed = concat!(
        "Before\n\n",
        "~~~text\nfirst\n~~~\n\n",
        "```rust\nsecond\n```\n\n",
        "After"
    );
    lines.clear();
    copies.clear();
    render_assistant_message(
        completed,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );
    let completed_text: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(completed_text.contains("first"));
    assert!(completed_text.contains("second"));
    assert!(completed_text.contains("After"));
    assert_eq!(copies.len(), 2, "both completed fences need copy targets");
}

#[test]
fn diff_code_blocks_preserve_patch_context_like_codex() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let content = "```diff\n--- a/src/temp.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-old\n-removed\n```";
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        content,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    let rendered: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(rendered.contains("a/src/temp.rs"));
    assert!(rendered.contains("/dev/null"));
    assert!(rendered.contains("@@ -1,2"));
    assert!(rendered.contains("removed"));
}

#[test]
fn thinking_with_tool_calls_hides_serialized_tool_blocks() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let content = concat!(
        "<think>Planning the next command.</think>\n\n",
        "```tool\n",
        r#"{"name":"run_command","arguments":{"command":"git status"}}"#,
        "\n```"
    );
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        content,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    let rendered: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(rendered.contains("Thought"));
    assert!(rendered.contains("Planning the next command."));
    assert!(!rendered.contains("run_command"));
    assert!(!rendered.contains("git status"));
    assert!(!rendered.contains("Build"));
}

#[test]
fn thought_parser_collapses_multiple_blocks() {
    let (answer, preview) = split_thought_blocks(
        "<think>First useful thought\nmore detail</think>answer\n<think>Second thought</think>",
    );
    assert_eq!(answer, "answer");
    assert_eq!(preview.as_deref(), Some("First useful thought"));
}

#[test]
fn thought_parser_drops_unclosed_block_from_answer() {
    let (answer, preview) = split_thought_blocks("before\n<think>Planning the next action");
    assert_eq!(answer, "before");
    assert_eq!(preview.as_deref(), Some("Planning the next action"));
}

#[test]
fn thought_parser_handles_missing_open_tag() {
    let (answer, preview) =
        split_thought_blocks("Reasoning about user request.\n</think>\n\nFinal response");
    assert_eq!(answer, "Final response");
    assert_eq!(preview.as_deref(), Some("Reasoning about user request."));
}

#[test]
fn thought_parser_captures_preamble_before_think_tag() {
    let raw = "Okay, the user is asking hello how are you, which I should respond to politely.\n\nFirst, I must check skills.\n\n<think>\nI will provide a standard friendly response.\n</think>\n\nHello! I am doing well, thank you for asking.";
    let (answer, preview) = split_thought_blocks(raw);
    assert_eq!(answer, "Hello! I am doing well, thank you for asking.");
    assert_eq!(
        preview.as_deref(),
        Some("Okay, the user is asking hello how are you, which I should respond to politely.")
    );
}

#[test]
fn thought_preview_keeps_short_text_unchanged() {
    assert_eq!(
        truncate_thought_preview("Analyzing Paste Events", 24),
        "Analyzing Paste Events"
    );
}

#[test]
fn thought_preview_truncates_to_one_display_line() {
    assert_eq!(
        truncate_thought_preview(
            "The user has made a request with contradictory instructions.",
            24
        ),
        "The user has made a req…"
    );
}

#[test]
fn thought_preview_does_not_split_wide_or_multibyte_characters() {
    let result = truncate_thought_preview("分析しています 🚀", 10);
    assert!(result.width() <= 10);
    assert!(result.is_char_boundary(result.len()));
}

#[test]
fn test_thinking_renders_metadata_and_summary() {
    use super::{AssistantRenderOptions, render_assistant_message};
    use crate::app::TokenUsage;

    let content =
        "<think>\nUnderstanding the history issue.\nTracing line by line.\n</think>\nDone";
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        content,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: Some(TokenUsage {
                prompt_tokens: 1000,
                completion_tokens: 400,
                total_tokens: 1400,
                cached_tokens: None,
            }),
            response_time_ms: Some(3000),
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    assert_eq!(lines[0].spans[1].content, "Thought for 3s, 1.4k tokens");
    assert_eq!(lines[0].spans[0].content, "▸ ");
    assert_eq!(
        lines[1].spans[0].content,
        "  Understanding the history issue."
    );
    let rendered: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(!rendered.contains("Tracing line by line."));
}

#[test]
fn thinking_metadata_uses_thought_stats_not_full_response_stats() {
    use super::{AssistantRenderOptions, render_assistant_message};
    use crate::app::TokenUsage;

    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        "<think>Planning the answer.</think>Final answer.",
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: Some(TokenUsage {
                prompt_tokens: 1000,
                completion_tokens: 900,
                total_tokens: 1900,
                cached_tokens: None,
            }),
            response_time_ms: Some(9000),
            thought_time_ms: Some(1250),
            thought_tokens: Some(42),
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    assert_eq!(lines[0].spans[1].content, "Thought for 1.2s, 42 tokens");
}

#[test]
fn test_tool_result_follows_skips_hidden_notices() {
    use super::tool_result_follows;
    use crate::app::ChatMessage;

    let history = vec![
        ChatMessage::new("assistant", "calling tool"),
        ChatMessage::new("system", "[harness: stopped after 13 tool round(s)]"),
        ChatMessage::new("tool", "tool output"),
    ];
    assert!(tool_result_follows(&history, 0));

    let history_direct = vec![
        ChatMessage::new("assistant", "calling tool"),
        ChatMessage::new("tool", "tool output"),
    ];
    assert!(tool_result_follows(&history_direct, 0));

    let history_no_tool = vec![
        ChatMessage::new("assistant", "calling tool"),
        ChatMessage::new("user", "hello"),
    ];
    assert!(!tool_result_follows(&history_no_tool, 0));
}

#[test]
fn tool_result_spacing_targets_next_assistant() {
    use super::tool_result_needs_assistant_gap;
    use crate::app::ChatMessage;

    let direct_assistant = vec![
        ChatMessage::new("tool", "tool output"),
        ChatMessage::new("assistant", "<think>planning</think>answer"),
    ];
    assert!(tool_result_needs_assistant_gap(&direct_assistant, 0));

    let hidden_notice_then_assistant = vec![
        ChatMessage::new("tool", "tool output"),
        ChatMessage::new("system", "[harness: stopped after 1 tool round(s)]"),
        ChatMessage::new("assistant", "<think>planning</think>answer"),
    ];
    assert!(tool_result_needs_assistant_gap(
        &hidden_notice_then_assistant,
        0
    ));

    let user_follows = vec![
        ChatMessage::new("tool", "tool output"),
        ChatMessage::new("user", "next prompt"),
    ];
    assert!(!tool_result_needs_assistant_gap(&user_follows, 0));

    let consecutive_tools = vec![
        ChatMessage::new("tool", "first output"),
        ChatMessage::new("tool", "second output"),
    ];
    assert!(!tool_result_needs_assistant_gap(&consecutive_tools, 0));
}

#[test]
fn status_panels_render_minimal_inline() {
    use super::render_status_panel;

    let mut lines = Vec::new();
    render_status_panel("Session status: 5 messages", 80, false, &mut lines);

    assert_eq!(
        lines.len(),
        5,
        "boxed info status panel includes top/bottom borders & padding"
    );
    assert!(lines[0].spans[0].content.contains(">_ RustCode"));
    assert!(
        lines[2].spans[1]
            .content
            .contains("Session status: 5 messages")
    );

    let mut notice_lines = Vec::new();
    render_status_panel(
        "Notice: background task finished",
        80,
        false,
        &mut notice_lines,
    );

    assert_eq!(notice_lines.len(), 1, "ordinary notice panel skips header");
    assert!(notice_lines[0].spans[0].content.contains("  "));

    let mut loop_recovery_lines = Vec::new();
    render_status_panel(
        "[Evidence-based recovery: signal=no_new_information streak=4 action=view_file]. Use a different, evidence-producing next step; do not repeat the same unchanged read, no-result search, no-op edit, or failed command.\nThe previous tool action repeated without making progress. Tools remain enabled for one recovery attempt.",
        80,
        false,
        &mut loop_recovery_lines,
    );
    assert_eq!(loop_recovery_lines.len(), 1);
    assert_eq!(loop_recovery_lines[0].spans[0].content, "! ");
    assert_eq!(
        loop_recovery_lines[0].spans[1].content,
        "Repetitive tool actions detected — nudging agent to make progress"
    );

    let mut loop_abort_lines = Vec::new();
    render_status_panel(
        "[Evidence-based recovery: signal=no_new_information streak=6 action=view_file]. Use a different, evidence-producing next step.\nCRITICAL — you are stuck in a loop. Tools are now DISABLED for this turn. Do NOT emit any tool calls.",
        80,
        false,
        &mut loop_abort_lines,
    );
    assert_eq!(loop_abort_lines.len(), 1);
    assert_eq!(loop_abort_lines[0].spans[0].content, "! ");
    assert_eq!(
        loop_abort_lines[0].spans[1].content,
        "Repetitive tool loop detected — stopping tools and requesting final response"
    );

    let mut cancelled_lines = Vec::new();
    render_status_panel(
        "[harness: turn stopped — cancelled]",
        80,
        false,
        &mut cancelled_lines,
    );
    assert_eq!(cancelled_lines.len(), 2);
    assert_eq!(cancelled_lines[1].spans[1].content, " ✕ Turn cancelled ");
}

#[test]
fn status_panel_help_box_lines_have_uniform_width() {
    use super::render_status_panel;

    let help_text = crate::app::actions::build_help_text();
    let mut lines = Vec::new();
    let total_width = 100u16;
    render_status_panel(&help_text, total_width, false, &mut lines);

    assert!(lines.len() > 10, "help text should render a full card");

    let expected_box_width = lines[0].width();
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(
            line.width(),
            expected_box_width,
            "line {i} ({:?}) must match box width {expected_box_width}",
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        );
    }
}

#[test]
fn new_chat_separator_spans_width_and_centers_label() {
    use super::push_new_chat_separator;
    use unicode_width::UnicodeWidthStr;

    let mut lines = Vec::new();
    push_new_chat_separator(&mut lines, 40, false);

    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].width(), 0);
    assert_eq!(lines[1].width(), 40);
    assert_eq!(lines[1].spans[1].content, " ✨ NEW CHAT ");
    assert_eq!(lines[2].width(), 0);

    let left = lines[1].spans[0].content.width();
    let right = lines[1].spans[2].content.width();
    assert!((left as isize - right as isize).abs() <= 1);
}

#[test]
fn resumed_session_separator_spans_width_and_centers_label() {
    use unicode_width::UnicodeWidthStr;

    let mut lines = Vec::new();
    super::render_status_panel("Resumed session \"My Test Session\"", 60, false, &mut lines);

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].width(), 0);
    assert_eq!(lines[1].width(), 60);
    assert_eq!(lines[1].spans[1].content, " Resumed Session ");

    let left = lines[1].spans[0].content.width();
    let right = lines[1].spans[2].content.width();
    assert!((left as isize - right as isize).abs() <= 1);
}

#[test]
fn new_chat_started_separator_spans_width_without_emoji() {
    use unicode_width::UnicodeWidthStr;

    let mut lines = Vec::new();
    super::render_status_panel("New chat started", 60, false, &mut lines);

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].width(), 0);
    assert_eq!(lines[1].width(), 60);
    assert_eq!(lines[1].spans[1].content, " New Chat Started ");

    let left = lines[1].spans[0].content.width();
    let right = lines[1].spans[2].content.width();
    assert!((left as isize - right as isize).abs() <= 1);
}

#[test]
fn resumed_session_committed_block_has_top_and_bottom_padding() {
    let mut state = AppState::new();
    state.history.push(crate::app::ChatMessage::new(
        "system",
        "Resumed session \"My Test Session\"",
    ));

    let block = super::render_committed_history_block(&state, 0, 60);
    assert_eq!(block.len(), 3);
    assert!(block[0].to_string().is_empty());
    assert!(block[1].to_string().contains("Resumed Session"));
    assert!(block[2].to_string().is_empty());
}

// Regression: a short transcript used to receive the entire remaining frame,
// pinning the input box to the bottom and leaving a large empty gap.
#[test]
fn conversation_area_height_fits_short_transcripts_and_caps_long_ones() {
    assert_eq!(conversation_area_height(8, 36), 8);
    assert_eq!(conversation_area_height(64, 36), 36);
    assert_eq!(conversation_area_height(0, 36), 0);
}

#[test]
fn harness_recovery_notices_are_hidden_from_transcript() {
    assert!(super::is_hidden_system_notice(
        "[harness: stopped after 10 tool round(s) — 4 consecutive malformed tool-call blocks the harness could not parse. The task is NOT complete. Review the transcript above; if the remaining work is still valid, resume it in a new turn.]"
    ));
    assert!(super::is_hidden_system_notice(
        "[Oversized response: only the first 1 tool calls were kept (use_skill); 1 more were dropped. Anything the response claimed about their results was imagined — continue from the real results below.]"
    ));
    assert!(!super::is_hidden_system_notice(
        "Notice: background task finished"
    ));
    assert!(!super::is_hidden_system_notice(
        "[harness: turn stopped — cancelled]"
    ));
}

#[test]
fn cancelled_turn_renders_as_a_human_status_separator() {
    let mut state = crate::app::AppState::new();
    state.history.push(crate::app::ChatMessage::new(
        "system",
        "[harness: turn stopped — cancelled]",
    ));

    let rendered = super::render_committed_history_block(&state, 0, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("✕ Turn cancelled"))
    );
    assert!(!rendered.iter().any(|line| line.contains("[harness:")));
}

#[test]
fn assistant_oversized_response_notice_renders_empty_block() {
    let mut state = crate::app::AppState::new();
    state.history.push(crate::app::ChatMessage::new(
        "assistant",
        "[Oversized response: only the first 1 tool calls were kept (use_skill); 1 more were dropped. Anything the response claimed about their results was imagined — continue from the real results below.]",
    ));
    let block = super::render_committed_history_block(&state, 0, 80);
    assert!(block.is_empty());
}

#[test]
fn tool_action_formats_generic_args_and_omits_empty() {
    use super::format_pi_tool_action;

    let (action, arg) = format_pi_tool_action(
        "manage_task",
        &serde_json::json!({"Action": "status", "TaskId": "task-123"}),
        None,
    );
    assert_eq!(action, "ManageTask");
    assert_eq!(arg, "status task-123");

    let (action_list, arg_list) =
        format_pi_tool_action("manage_task", &serde_json::json!({"Action": "list"}), None);
    assert_eq!(action_list, "ManageTask");
    assert_eq!(arg_list, "list");

    let (action_bg, arg_bg) = format_pi_tool_action(
        "background_task",
        &serde_json::json!({"TaskId": "task-456"}),
        None,
    );
    assert_eq!(action_bg, "TaskDone");
    assert_eq!(arg_bg, "task-456");

    let (action2, arg2) = format_pi_tool_action("get_date", &serde_json::json!({}), None);
    assert_eq!(action2, "GetDate");
    assert_eq!(arg2, "");
}

#[test]
fn line_height_fast_path_matches_paragraph_wrap() {
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Wrap};

    let width = 80u16;
    let short_line = Line::from("Short text fits in viewport");
    let long_line = Line::from("A ".repeat(100));

    let short_w = short_line.width() as u16;
    let short_fast_h = if width == 0 || short_w <= width {
        1
    } else {
        Paragraph::new(vec![short_line.clone()])
            .wrap(Wrap { trim: false })
            .line_count(width) as u16
    };
    let short_expected_h = Paragraph::new(vec![short_line])
        .wrap(Wrap { trim: false })
        .line_count(width) as u16;
    assert_eq!(short_fast_h, short_expected_h);

    let long_w = long_line.width() as u16;
    let long_fast_h = if width == 0 || long_w <= width {
        1
    } else {
        Paragraph::new(vec![long_line.clone()])
            .wrap(Wrap { trim: false })
            .line_count(width) as u16
    };
    let long_expected_h = Paragraph::new(vec![long_line])
        .wrap(Wrap { trim: false })
        .line_count(width) as u16;
    assert_eq!(long_fast_h, long_expected_h);
}

#[test]
fn footer_animation_pulse_center_reaches_both_edges() {
    let num_dots = 6;
    let pulse_centers_f = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 2.0, 1.0];
    assert_eq!(pulse_centers_f.first(), Some(&0.0));
    assert!(pulse_centers_f.contains(&(num_dots as f64 - 1.0)));
    assert_eq!(pulse_centers_f[5], 5.0);
}

#[test]
fn activity_status_labels_idle_and_working_states() {
    let state = AppState::new();
    assert_eq!(activity_status_label(&state.render_snapshot()), "Idle");
    assert_eq!(
        activity_status_line(&state.render_snapshot(), false)
            .spans
            .last()
            .unwrap()
            .content,
        " "
    );

    let mut streaming_state = AppState::new();
    streaming_state.status = AppStatus::Streaming;
    assert_eq!(
        activity_status_label(&streaming_state.render_snapshot()),
        "Working"
    );

    streaming_state.current_thought_started_at = Some(std::time::Instant::now());
    assert_eq!(
        activity_status_label(&streaming_state.render_snapshot()),
        "Thinking"
    );
}

#[test]
fn streaming_decode_speed_is_displayed_in_composer_footer_not_activity() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time = Some(std::time::Instant::now());
    let mut tracker = crate::app::StreamTracker::new();
    tracker.tokens_so_far = 8;
    tracker.record_chunk();
    state.stream_tracker = Some(tracker);

    let status = activity_status_line(&state.render_snapshot(), false).to_string();
    let rendered = render_state_to_text(&mut state, 100, 12);
    let footer = rendered
        .lines()
        .find(|line| line.contains("context left"))
        .expect("composer footer should be rendered");

    assert!(!status.contains("Tokens/s"), "{status}");
    assert!(footer.contains("Tokens/s: 80.0"), "{footer}");
}

#[test]
fn background_terminal_activity_shows_management_hints_and_command() {
    let mut state = AppState::new();
    state.background_turn_context = Some(Box::new(
        crate::network::TurnContext::with_max_tool_rounds(1),
    ));
    let session_id = state.active_session_id.clone();
    let task_id = "ui-background-footer";
    let long_command = if cfg!(target_os = "windows") {
        "ping -n 30 127.0.0.1 > NUL"
    } else {
        "sleep 30"
    };
    crate::tools::spawn_background_task_for_test(task_id, &session_id, long_command).unwrap();
    let snapshot = state.render_snapshot();
    state.background_turn_context = None;
    let neutral_snapshot = state.render_snapshot();
    crate::tools::stop_background_tasks(&session_id);

    let status = super::activity_status_line(&snapshot, false).to_string();
    assert!(status.contains("Waiting for background terminal"));
    assert!(status.contains("esc to interrupt"));
    assert!(status.contains("1 background terminal running"));
    assert!(status.contains("/ps to view · /stop to close"));
    let neutral_status = super::activity_status_line(&neutral_snapshot, false).to_string();
    assert!(neutral_status.contains("Idle"));
    assert!(neutral_status.contains("1 background terminal running"));
    assert!(!neutral_status.contains("Waiting for background terminal"));
    assert!(!neutral_status.contains("esc to interrupt"));

    let commands = super::background_command_lines(&snapshot);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].to_string(), format!("  └ {long_command}"));
    let live_tail = super::render_live_tail_snapshot(&snapshot, 120, 10)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(live_tail.contains("Waiting for background terminal"));
    assert!(live_tail.contains(&format!("  └ {long_command}")));
    assert_eq!(
        super::background_terminal_summary(2),
        "2 background terminals running · /ps to view · /stop to close"
    );
    assert_eq!(
        crate::tools::background_command_label("cargo\n test\t--locked", 80),
        "cargo test --locked"
    );
}

#[test]
fn live_tool_activity_is_rendered_without_protocol_text() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time = Some(std::time::Instant::now());
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(crate::app::LiveToolCall::new(
        "call-1",
        None,
        "run_command",
        "Bash",
        "cargo test",
    ));

    let line = super::activity_status_line(&state.render_snapshot(), false).to_string();

    assert!(line.contains("Working"));
    assert!(line.contains("esc interrupt"));
    assert!(!line.contains("tool_calls"));
    assert!(!line.contains("Bash"));
    assert!(!line.contains("cargo test"));
}

#[test]
fn composer_footer_stays_compact_when_busy() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.model_name = "streaming-model".to_string();
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut state))
        .unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("streaming-model"));
    assert!(!rendered.contains("Enter message then press enter to queue"));
}

#[test]
fn live_history_cell_keeps_identical_invocations_visible_separately() {
    let calls = vec![
        crate::app::LiveToolCall::new("local:1", None, "run_command", "Bash", "cargo test"),
        crate::app::LiveToolCall::new("local:2", None, "run_command", "Bash", "cargo test"),
    ];

    let rendered = super::history_cell::render_live_tool_cell(&calls, 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(rendered[0], "• Running");
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("Bash $ cargo test"))
            .count(),
        2,
        "the live cell must not deduplicate distinct invocation identities"
    );
}

#[test]
fn live_tool_cell_is_a_projection_not_history() {
    let mut state = AppState::new();
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(crate::app::LiveToolCall::new(
        "local:1",
        None,
        "view_file",
        "Read",
        "src/main.rs",
    ));

    let text = super::render_live_tail(&state, 80, 24)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Exploring"));
    assert!(state.history.is_empty());
}

#[test]
fn single_live_generic_tool_shows_its_action_without_using_heading() {
    let call = crate::app::LiveToolCall::new(
        "local:1",
        None,
        "use_skill",
        "UseSkill",
        "release-automation",
    );
    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• UseSkill release-automation"]);
}

#[test]
fn speculative_tool_without_target_is_not_rendered() {
    let mut call = crate::app::LiveToolCall::new("local:1", None, "list", "List", "");
    call.execution_started = false;

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false);

    assert!(rendered.is_empty());
}

#[test]
fn live_editing_tool_cell_shows_editing_heading_and_target_child() {
    let call = crate::app::LiveToolCall::new(
        "local:1",
        None,
        "replace_file_content",
        "Edit",
        "src/game/engine.ts",
    );
    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Editing", "  └ src/game/engine.ts"]);
}

#[test]
fn live_audio_generation_cell_shows_editing_heading_and_output_path() {
    let arguments = serde_json::json!({
        "prompt": "a short balloon pop",
        "duration_seconds": 0.4,
        "output_path": "assets/audio/balloon-pop.wav"
    });
    let (action, target) =
        crate::app::activity::summarize_tool_call("generate_sound_effect", &arguments);
    let call =
        crate::app::LiveToolCall::new("local:1", None, "generate_sound_effect", action, target);

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Editing", "  └ assets/audio/balloon-pop.wav"]);
}

#[test]
fn live_video_render_cell_shows_progress() {
    let mut call = crate::app::LiveToolCall::new(
        "local:1",
        None,
        "render_video",
        "RenderVideo",
        "video-project.json",
    );
    call.output.push_back(crate::app::LiveToolOutputChunk {
        stderr: true,
        text: "render progress: 42% (2.1s/5.0s)\n".to_owned(),
    });

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Rendering video-project.json");
    assert!(rendered[1].contains("render progress: 42% (2.1s/5.0s)"));
}

#[test]
fn live_batched_edits_with_casing_aliases_group_under_editing_without_actions() {
    let calls = vec![
        crate::app::LiveToolCall::new(
            "local:1",
            None,
            "replace_file_content",
            "Edit",
            "src/game/engine.ts",
        ),
        crate::app::LiveToolCall::new("local:2", None, "WriteFile", "Write", "src/App.tsx"),
    ];
    let rendered = super::history_cell::render_live_tool_cell(&calls, 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        ["• Editing", "  └ src/game/engine.ts", "    src/App.tsx"]
    );
}

#[test]
fn live_multiple_generic_tools_show_running_heading() {
    let calls = vec![
        crate::app::LiveToolCall::new("local:1", None, "clockify_timer", "ClockifyTimer", "start"),
        crate::app::LiveToolCall::new("local:2", None, "notify_user", "NotifyUser", "done"),
    ];
    let rendered = super::history_cell::render_live_tool_cell(&calls, 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "• Running",
            "  └ ClockifyTimer start",
            "    NotifyUser done"
        ]
    );
}

#[test]
fn live_command_cell_shows_bounded_stdout_stderr_and_omission() {
    let mut call =
        crate::app::LiveToolCall::new("local:1", None, "run_command", "Bash", "cargo test");
    call.output.push_back(crate::app::LiveToolOutputChunk {
        stderr: false,
        text: (0..12).map(|line| format!("stdout {line}\n")).collect(),
    });
    call.output.push_back(crate::app::LiveToolOutputChunk {
        stderr: true,
        text: "compiler error\n".to_owned(),
    });
    call.omitted_output_bytes = 4096;

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• cargo test");
    assert!(rendered.iter().any(|line| line.contains("compiler error")));
    assert!(rendered.iter().any(|line| line.contains("lines")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("4096 earlier bytes omitted"))
    );
    assert!(
        rendered.len() <= 8,
        "live output must remain bounded: {rendered:?}"
    );
}

#[test]
fn high_verbosity_live_command_cell_shows_only_the_invocation() {
    let mut call =
        crate::app::LiveToolCall::new("local:1", None, "run_command", "Bash", "cargo test");
    call.output.push_back(crate::app::LiveToolOutputChunk {
        stderr: false,
        text: "secret command output\n".to_owned(),
    });

    let rendered = super::history_cell::render_live_tool_cell_with_verbosity(
        &[call],
        80,
        &crate::app::Verbosity::High,
        false,
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>();

    assert_eq!(rendered, ["• cargo test"]);
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("secret command output"))
    );
}

#[test]
fn question_replaces_composer_with_borderless_bottom_pane() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.status = AppStatus::AwaitingQuestion;
    state.pending_question = Some(crate::app::PendingQuestion::new(
        "Choose an option.".to_owned(),
        vec!["Option 1".to_owned(), "Option 2".to_owned()],
        false,
    ));
    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    terminal.draw(|frame| render(frame, &mut state)).unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Question 1/1 (1 unanswered)"));
    assert!(rendered.contains("› 1. Option 1"));
    assert!(rendered.contains("enter to submit answer"));
    assert!(!rendered.contains("Ask RustCode to do anything"));
    assert!(!rendered.contains('╭') && !rendered.contains('╰'));
}

#[test]
fn active_transcript_cell_updates_in_place_and_clears_without_history() {
    let mut transcript = super::TranscriptState::default();

    transcript.set_assistant("first paragraph", false, None, None, None);
    let first_revision = transcript.revision();
    assert!(!transcript.display_lines(80).is_empty());

    transcript.set_assistant("first paragraph\n\nsecond", true, None, None, None);
    assert!(transcript.revision() > first_revision);
    assert!(!transcript.display_lines(80).is_empty());

    transcript.set_tools(&[crate::app::LiveToolCall::new(
        "call-1",
        Some("native-1".to_owned()),
        "run_command",
        "Bash",
        "cargo test",
    )]);
    assert!(transcript.revision() > first_revision);
    assert!(
        transcript
            .display_lines(80)
            .iter()
            .any(|line| line.to_string().contains("cargo test"))
    );

    transcript.clear();
    assert!(transcript.display_lines(80).is_empty());
}

#[test]
fn action_required_status_wins_over_a_live_question_tool() {
    let mut state = AppState::new();
    state.status = AppStatus::AwaitingQuestion;
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(crate::app::LiveToolCall::new(
        "question",
        None,
        "ask_question",
        "AskQuestion",
        "continue?",
    ));

    assert_eq!(
        super::activity_status_label(&state.render_snapshot()),
        "Action Required"
    );
}

#[test]
fn split_stable_rows_keeps_only_the_incomplete_suffix_live() {
    let (stable, tail) = super::scrollback::split_stable_rows("first\nsecond\nthird");

    assert_eq!(stable, vec!["first", "second"]);
    assert_eq!(tail, "third");
}

#[test]
fn transcript_cursor_never_recommits_history_or_stream_rows() {
    let mut cursor = super::scrollback::TranscriptCursor::default();

    assert_eq!(cursor.take_history_range(3), 0..3);
    assert_eq!(cursor.take_history_range(3), 3..3);
    assert_eq!(cursor.take_stable_stream("alpha\n\nbeta"), vec!["alpha"]);
    assert!(cursor.take_stable_stream("alpha\n\nbeta").is_empty());
}

#[test]
fn transcript_cursor_retries_pending_content_until_acknowledged() {
    let mut cursor = super::scrollback::TranscriptCursor::default();

    assert_eq!(cursor.pending_history_range(2), 0..2);
    assert_eq!(cursor.pending_history_range(2), 0..2);
    cursor.commit_history_through(2);
    assert_eq!(cursor.pending_history_range(2), 2..2);

    assert_eq!(cursor.pending_stable_stream("line\n\ntail"), vec!["line"]);
    assert_eq!(cursor.pending_stable_stream("line\n\ntail"), vec!["line"]);
    cursor.commit_stable_stream("line\n\n");
    assert!(cursor.pending_stable_stream("line\n\ntail").is_empty());
}

#[test]
fn transcript_cursor_reset_replays_history_after_resize() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    cursor.commit_history_through(4);
    cursor.commit_stable_stream("already rendered\n");

    cursor.reset();

    assert_eq!(cursor.pending_history_range(4), 0..4);
    assert_eq!(
        cursor.pending_stable_stream("already rendered\n\nnext row"),
        vec!["already rendered"]
    );
}

#[test]
fn transcript_cursor_holds_thought_stream_until_finalized() {
    let cursor = super::scrollback::TranscriptCursor::default();

    assert!(
        cursor
            .pending_stable_stream("<think>\nPlanning\n")
            .is_empty()
    );
    assert!(
        cursor
            .pending_stable_stream("thoughtPlanning the response\n")
            .is_empty()
    );
}

#[test]
fn transcript_cursor_keeps_an_incomplete_code_fence_together() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let stream = "intro\n\n```rust\nfn main() {\n";

    assert_eq!(cursor.pending_stable_source(stream), "intro\n\n".to_owned());
    assert_eq!(cursor.pending_stable_stream(stream), vec!["intro"]);
    assert_eq!(
        super::scrollback::mutable_stream_text(stream),
        "```rust\nfn main() {\n".to_owned()
    );

    cursor.commit_stable_stream("intro\n\n");
    assert!(cursor.pending_stable_stream(stream).is_empty());

    let completed = "intro\n\n```rust\nfn main() {}\n```\n";
    assert_eq!(cursor.pending_stable_source(completed), String::new());
}

#[test]
fn transcript_cursor_keeps_streamed_tables_mutable_until_finalization() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let header = "intro\n\n| Name | Value |\n";
    let with_delimiter = "intro\n\n| Name | Value |\n| --- | --- |\n";
    let with_row = "intro\n\n| Name | Value |\n| --- | --- |\n| one | two |\n";

    assert_eq!(cursor.pending_stable_source(header), "intro\n\n");
    assert_eq!(
        super::scrollback::mutable_stream_text(header),
        "| Name | Value |\n"
    );
    cursor.commit_stable_stream("intro\n\n");

    assert!(cursor.pending_stable_stream(with_delimiter).is_empty());
    assert_eq!(
        super::scrollback::mutable_stream_text(with_row),
        "| Name | Value |\n| --- | --- |\n| one | two |\n"
    );
    assert!(cursor.pending_stable_stream(with_row).is_empty());

    let remainder = cursor
        .take_final_stream_remainder(with_row)
        .expect("stream prefix should be acknowledged");
    assert_eq!(remainder, with_row.strip_prefix("intro\n\n").unwrap());

    cursor.reset();
    assert_eq!(cursor.pending_stable_source(with_row), "intro\n\n");
}

#[test]
fn transcript_cursor_does_not_hold_pipe_text_without_a_table_delimiter() {
    let cursor = super::scrollback::TranscriptCursor::default();
    let stream = "A | B\nThis is ordinary prose\n";

    assert!(cursor.pending_stable_stream(stream).is_empty());
}

#[test]
fn transcript_cursor_releases_completed_fence_and_replays_after_resize() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let before_close = "intro\n````rust\nlet text = \"```\";\n";

    assert_eq!(
        cursor.pending_stable_source(before_close),
        "intro\n".to_owned()
    );
    assert_eq!(
        super::scrollback::mutable_stream_text(before_close),
        "````rust\nlet text = \"```\";\n".to_owned()
    );
    cursor.commit_stable_stream("intro\n");

    let after_close = "intro\n````rust\nlet text = \"```\";\n````\nnext row";
    assert_eq!(
        cursor.pending_stable_source(after_close),
        "````rust\nlet text = \"```\";\n````\n".to_owned()
    );
    assert_eq!(
        super::scrollback::mutable_stream_text(after_close),
        "next row".to_owned()
    );

    cursor.commit_stable_stream("````rust\nlet text = \"```\";\n````\n");
    cursor.reset();
    assert_eq!(
        cursor.pending_stable_source(after_close),
        "intro\n````rust\nlet text = \"```\";\n````\n".to_owned()
    );
}

#[test]
fn live_tail_excludes_committed_history() {
    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "old completed answer"));
    state.status = AppStatus::Streaming;
    state.replace_current_response("stable line\nunclosed tail");

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("Working"));
    assert!(text.contains("unclosed tail"));
    assert!(!text.contains("old completed answer"));
}

#[test]
fn reasoning_prefixed_stream_keeps_completed_answer_lines_live() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.replace_current_response(
        "<think>\nPlanning\n</think>\n\nFirst answer line\nSecond answer line",
    );

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(
        text.contains("First answer line"),
        "completed answer rows must remain visible while the next row streams: {text:?}"
    );
    assert!(text.contains("Second answer line"));
}

#[test]
fn bare_thought_stream_stays_in_the_compact_reasoning_preview() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.replace_current_response("thoughtPlanning the response\n");

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("Thought"));
    assert!(text.contains("Planning the response"));
    assert!(!text.contains("thoughtPlanning"));
}

#[test]
fn assistant_messages_use_a_gutter_after_soft_reflow() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        "one two three four five six seven",
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 20,
            show_picker: false,
            last_copy_text: None,
        },
    );

    let prose: Vec<_> = lines.iter().filter(|line| !line.spans.is_empty()).collect();
    assert_eq!(prose[0].spans[0].content, "• ");
    let first_line = prose[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(first_line.contains("one two"));
    assert_eq!(prose[1].spans[0].content, "  ");
}

#[test]
fn streamed_assistant_chunks_only_bullet_the_first_chunk() {
    let state = AppState::new();
    let first = super::render_committed_assistant_chunk(&state, "first line\n", 80, false);
    let continuation = super::render_committed_assistant_chunk(&state, "second line\n", 80, true);

    assert_eq!(first[0].spans[0].content, "• ");
    assert_eq!(continuation[0].spans[0].content, "  ");
}

#[test]
fn assistant_message_uses_one_gutter_across_paragraphs() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        "first paragraph\n\n```text\ncode\n```\n\nsecond paragraph",
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    let prefixes = lines
        .iter()
        .filter(|line| !line.spans.is_empty())
        .filter_map(|line| line.spans.first())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>();

    assert_eq!(prefixes.first(), Some(&"• "));
    assert!(prefixes.iter().skip(1).all(|prefix| *prefix == "  "));
}

#[test]
fn committed_user_messages_keep_regular_body_text() {
    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("user", "inspect the parser"));

    let block = super::render_committed_history_block(&state, 0, 80);

    assert_eq!(block[1].spans[0].content, "› ");
    assert_eq!(block[1].width(), 80);
    assert!(
        block[0]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(super::COLOR_PANEL()))
    );
    assert!(
        block[1]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(super::COLOR_PANEL()))
    );
    assert!(
        block[2]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(super::COLOR_PANEL()))
    );
    assert!(
        !block[1].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn committed_user_message_has_trailing_blank_line() {
    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("user", "check latest 10 commits"));

    let block = super::render_committed_history_block(&state, 0, 80);

    assert_eq!(block.len(), 4);
    assert_eq!(block[0].width(), 80);
    assert_eq!(block[2].width(), 80);
    assert_eq!(
        block[0]
            .spans
            .iter()
            .skip(1)
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        ""
    );
    assert_eq!(
        block[1]
            .spans
            .iter()
            .skip(1)
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        format!("{:<78}", "check latest 10 commits")
    );
    assert!(block[3].spans.is_empty());
}

#[test]
fn committed_assistant_message_has_one_trailing_separator() {
    let state = AppState::new();

    let block = super::render_committed_assistant_text(&state, "Finished.", 80);

    assert_eq!(block.len(), 2);
    assert_eq!(block[0].spans[0].content, "• ");
    assert!(block[1].spans.is_empty());
}

#[test]
fn conversation_recap_renders_as_compact_labeled_block() {
    let mut state = AppState::new();
    state.history.push(
        ChatMessage::new(
            "assistant",
            "The implementation is complete; cargo test passes and the next step is review.",
        )
        .as_conversation_recap(),
    );

    let rendered = super::render_committed_history_block(&state, 0, 80);
    let text = rendered
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();

    assert!(text[0].starts_with("  ─ Conversation recap ─"));
    assert_eq!(text[0].chars().count(), 80);
    assert_eq!(text[1], "");
    assert_eq!(
        text[2],
        "  The implementation is complete; cargo test passes and the next step is review."
    );
    assert_eq!(text[3], "");
    assert!(!text.iter().any(|line| line.contains("• ")));
}

#[test]
fn conversation_recap_wraps_inside_its_message_gutter() {
    let mut state = AppState::new();
    state.history.push(
        ChatMessage::new(
            "assistant",
            "The recap remains aligned while its long message wraps across multiple lines.",
        )
        .as_conversation_recap(),
    );

    let rendered = super::render_committed_history_block(&state, 0, 32);
    let text = rendered
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();

    assert!(text.len() > 5, "recap fixture must wrap: {text:?}");
    assert!(text[0].starts_with("  ─ Conversation recap ─"));
    assert!(
        text[2..]
            .iter()
            .filter(|line| !line.is_empty())
            .all(|line| line.starts_with("  ") && line.chars().count() <= 32)
    );
}

#[test]
fn committed_assistant_message_uses_saved_thought_metrics() {
    let mut state = AppState::new();
    let mut message = ChatMessage::new("assistant", "<think>Planning.</think>Finished.");
    message.thought_time_ms = Some(1250);
    message.thought_tokens = Some(42);
    state.history.push(message);

    let block = super::render_committed_history_block(&state, 0, 80);

    assert_eq!(block[0].spans[1].content, "Thought for 1.2s, 42 tokens");
}

#[test]
fn committed_thought_only_message_has_a_separator_before_tools() {
    let mut state = AppState::new();
    let mut message = ChatMessage::new(
        "assistant",
        "<think>Find the Rust files before reading them.</think>",
    );
    message.thought_time_ms = Some(718);
    message.thought_tokens = Some(31);
    state.history.push(message);
    state
        .history
        .push(ChatMessage::new("tool", "glob: src/main.rs"));

    let thought = super::render_committed_history_block(&state, 0, 80);
    let tool = super::render_committed_history_block(&state, 1, 80);

    assert!(
        thought[0]
            .to_string()
            .contains("Thought for 718ms, 31 tokens")
    );
    assert!(thought.last().is_some_and(|line| line.spans.is_empty()));
    assert!(tool.first().is_some_and(|line| !line.spans.is_empty()));
}

#[test]
fn live_tail_uses_formatted_working_status() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("• Working"));
    assert!(text.contains("esc interrupt"));
    assert!(!text.contains("Working..."));
}

#[test]
fn live_tail_includes_working_status_with_trailing_gap() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;

    let lines = super::render_live_tail(&state, 80, 24);

    assert!(lines.len() >= 2);
    assert!(lines.last().unwrap().spans.is_empty());
    let status_text = lines[lines.len() - 2]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(status_text.contains("Working"));
}

#[test]
fn visible_streaming_text_keeps_working_status_until_completion() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.replace_current_response(
        (1..=10)
            .map(|line| format!("streamed line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let lines = super::render_live_tail(&state, 30, 24);
    let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();
    let text = rendered.join(" ");

    assert!(
        text.contains("streamed line 1"),
        "streaming lines: {rendered:?}"
    );
    assert!(text.contains("line 10"), "streaming lines: {rendered:?}");
    assert!(rendered.len() > 5, "streaming lines: {rendered:?}");
    assert!(rendered.iter().any(|line| line.contains("Working")));
    assert!(lines.last().is_some_and(|line| line.spans.is_empty()));
}

#[test]
fn consecutive_thought_blocks_have_a_blank_line_gap() {
    let mut lines = Vec::new();
    let mut copy_clicks = Vec::new();
    let options = super::AssistantRenderOptions {
        token_usage: None,
        response_time_ms: None,
        thought_time_ms: Some(1500),
        thought_tokens: Some(100),
        is_generating: false,
        viewport_width: 80,
        show_picker: false,
        last_copy_text: None,
    };

    super::render_assistant_message(
        "<think>\nFirst thought\n</think>\nFirst response",
        &mut lines,
        &mut copy_clicks,
        options,
    );

    let options2 = super::AssistantRenderOptions {
        token_usage: None,
        response_time_ms: None,
        thought_time_ms: Some(2000),
        thought_tokens: Some(150),
        is_generating: false,
        viewport_width: 80,
        show_picker: false,
        last_copy_text: None,
    };

    super::render_assistant_message(
        "<think>\nSecond thought\n</think>\nSecond response",
        &mut lines,
        &mut copy_clicks,
        options2,
    );

    let thought_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.spans.iter().any(|s| s.content.contains("Thought for")))
        .map(|(i, _)| i)
        .collect();

    assert_eq!(thought_indices.len(), 2);
    assert!(lines[thought_indices[1] - 1].spans.is_empty());
}

#[test]
fn active_turn_uses_only_the_history_separator_above_working() {
    let mut state = AppState::new();
    assert_eq!(
        super::live_surface_padding(&state.render_snapshot()),
        (1, 1)
    );

    state.status = AppStatus::Streaming;
    assert_eq!(
        super::live_surface_padding(&state.render_snapshot()),
        (0, 1)
    );
}

#[test]
fn empty_composer_has_painted_padding_and_external_model_footer() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut state))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let prompt_row = (0..12).find(|y| {
        (0..100)
            .map(|x| buffer[(x, *y)].symbol())
            .collect::<String>()
            .contains("Ask RustCode to do anything")
    });
    let bottom_border_row = (0..12).find(|y| {
        (0..100)
            .map(|x| buffer[(x, *y)].symbol())
            .collect::<String>()
            .contains("context left")
    });

    let prompt_row = prompt_row.expect("composer prompt should be rendered");
    let footer_row = bottom_border_row.expect("composer footer should be rendered");
    assert_eq!(
        state.input_text_area.map(|area| area.y),
        Some(prompt_row - 1),
        "shutdown should know where the transient composer begins"
    );
    assert_eq!(footer_row, prompt_row + 2);
    let footer = (0..100)
        .map(|x| buffer[(x, footer_row)].symbol())
        .collect::<String>();
    assert!(
        footer.contains(&state.model_name),
        "composer footer: {footer:?}"
    );
    assert!(
        !footer.contains("? for shortcuts"),
        "composer footer: {footer:?}"
    );
    assert!(
        footer.contains(" · "),
        "composer footer should include model/location separators: {footer:?}"
    );
    assert_eq!(buffer[(0, prompt_row - 1)].bg, COLOR_PANEL());
    assert_eq!(buffer[(0, prompt_row + 1)].bg, COLOR_PANEL());
    assert_eq!(buffer[(0, footer_row)].bg, COLOR_BG());
    assert_eq!(buffer[(99, prompt_row)].bg, COLOR_PANEL());
}

#[test]
fn armed_ctrl_c_is_visible_in_the_production_composer_footer() {
    let mut state = AppState::new();
    state.ctrl_c_exit_deadline =
        Some(std::time::Instant::now() + std::time::Duration::from_secs(2));

    let rendered = render_state_to_text(&mut state, 100, 12);

    assert!(rendered.contains("⚠ Press Ctrl+C again to exit"));
    let footer = rendered
        .lines()
        .find(|line| line.contains("Press Ctrl+C again to exit"))
        .expect("exit warning should be rendered in the composer footer");
    assert!(
        footer.starts_with("  ⚠ Press Ctrl+C again to exit"),
        "exit warning should be left-aligned: {footer:?}"
    );
    assert!(
        footer.ends_with("100% context left  "),
        "context usage should remain right-aligned: {footer:?}"
    );

    // The warning is clipped safely, rather than causing a layout overflow,
    // on terminals narrower than the complete message.
    let narrow = render_state_to_text(&mut state, 24, 12);
    assert!(narrow.contains('⚠'));
}

#[test]
fn composer_footer_shows_path_and_truncates_long_branch() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.cwd_and_branch =
        "~/code/rustcode:feature/a-branch-name-that-is-definitely-too-long".to_string();
    let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut state))
        .unwrap();

    let footer_row = (0..12)
        .find(|row| {
            (0..120)
                .map(|column| terminal.backend().buffer()[(column, *row)].symbol())
                .collect::<String>()
                .contains("context left")
        })
        .expect("composer footer should be rendered");
    let footer = (0..120)
        .map(|column| terminal.backend().buffer()[(column, footer_row)].symbol())
        .collect::<String>();
    assert!(footer.contains("~/code/rustcode"));
    assert!(footer.contains("feature/a-branch-name-t…"));
    assert!(
        !footer.contains("definitely-too-long"),
        "footer should not contain the untruncated branch: {footer:?}"
    );
}

#[test]
fn composer_footer_is_hidden_while_a_picker_is_open() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.show_model_picker = true;
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut state))
        .unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(!rendered.contains("context left"));
}

#[test]
fn codex_shimmer_moves_a_visible_gradient_across_working() {
    let early = super::shimmer_spans_at("Working", std::time::Duration::from_millis(850));
    let later = super::shimmer_spans_at("Working", std::time::Duration::from_millis(1100));
    let early_colors = early.iter().map(|span| span.style.fg).collect::<Vec<_>>();
    let later_colors = later.iter().map(|span| span.style.fg).collect::<Vec<_>>();

    assert!(
        early_colors.iter().any(|color| *color != early_colors[0]),
        "a visible frame must not paint the whole word one color: {early_colors:?}"
    );
    assert!(
        later_colors.iter().any(|color| *color != later_colors[0]),
        "a visible frame must not paint the whole word one color: {later_colors:?}"
    );
    assert_ne!(
        early_colors, later_colors,
        "the gradient must travel over time"
    );
}

#[test]
fn transcript_cursor_returns_only_uncommitted_final_stream_tail() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    cursor.commit_stable_stream("stable\n\n");

    assert_eq!(
        cursor.take_final_stream_remainder("stable\n\ntail"),
        Some("tail".to_owned())
    );
    assert_eq!(cursor.take_final_stream_remainder("stable\ntail"), None);
}

#[test]
fn transcript_cursor_keeps_a_committed_prefix_when_the_stream_finalizes() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let final_text = "Opening line\n\nFinal answer";
    let stable = cursor.pending_stable_source(final_text);
    cursor.commit_stable_stream(&stable);

    cursor.begin_stream("");

    assert_eq!(
        cursor.take_final_stream_remainder(final_text),
        Some("Final answer".to_owned())
    );
}

#[test]
fn transcript_cursor_reports_an_empty_tail_when_stream_rows_need_a_separator() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    cursor.commit_stable_stream("table row\n\n");

    // The final history entry can contain exactly the rows already committed
    // during streaming. The draw loop uses this empty remainder as the handoff
    // point to insert one blank row before a follow-up user message.
    assert_eq!(
        cursor.take_final_stream_remainder("table row\n\n"),
        Some(String::new())
    );
}

#[test]
fn transcript_cursor_resets_when_a_new_stream_replaces_the_old_one() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    cursor.commit_stable_stream("first\n\n");
    cursor.begin_stream("second\n\ntail");

    assert_eq!(
        cursor.pending_stable_stream("second\n\ntail"),
        vec!["second"]
    );
}

#[test]
fn subagent_picker_renders_context_status_and_navigation_hint() {
    let mut state = AppState::new();
    crate::app::SubagentController.spawn(
        &mut state,
        "inspect the parser",
        Some("high".to_owned()),
        None,
        false,
        Vec::new(),
        None,
        None,
    );
    state.show_subagent_picker = true;

    let rendered = render_state_to_text(&mut state, 100, 30);

    assert!(rendered.contains("Agent contexts"));
    assert!(rendered.contains("agent-1"));
    assert!(rendered.contains("inspect the parser"));
    assert!(rendered.contains("main"));
}

#[test]
fn selected_subagent_renders_its_transcript_without_replacing_parent_history() {
    let mut state = AppState::new();
    state
        .history
        .push(crate::app::ChatMessage::new("user", "parent task"));
    let id = crate::app::SubagentController.spawn(
        &mut state,
        "child task",
        None,
        None,
        false,
        Vec::new(),
        None,
        None,
    );
    std::sync::Arc::make_mut(&mut state.subagents[0].history)
        .push(crate::app::ChatMessage::new("assistant", "child result"));
    crate::app::SubagentController
        .select(&mut state, id)
        .unwrap();

    let rendered = render_state_to_text(&mut state, 100, 30);

    assert!(rendered.contains("agent-1"));
    assert!(rendered.contains("child result"));
    assert_eq!(state.history[0].content, "parent task");
}

#[test]
fn active_subagent_context_is_named_in_the_composer_footer() {
    let mut state = AppState::new();
    let id = crate::app::SubagentController.spawn(
        &mut state,
        "child task",
        None,
        None,
        false,
        Vec::new(),
        None,
        None,
    );
    crate::app::SubagentController
        .select(&mut state, id)
        .unwrap();

    let rendered = render_state_to_text(&mut state, 100, 30);

    assert!(rendered.contains(&format!("agent-1 · {}", state.model_name)));
}

#[test]
fn model_picker_open_then_close_leaves_no_duplicate_composer_or_stale_rows() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.history.push(ChatMessage::new("user", "test prompt"));
    state.config.models = vec![
        crate::config::ModelProfile {
            name: "model-a".to_string(),
            url: "http://localhost/a".to_string(),
            model: "model-a".to_string(),
            context_window: None,
            engine: Some("Local".to_owned()),
            api_key: None,
            env_key: None,
            tool_protocol: None,
            enable_thinking: None,
            reasoning_effort: None,
            max_tokens: None,
            supports_vision: None,
            ..Default::default()
        },
        crate::config::ModelProfile {
            name: "model-b".to_string(),
            url: "http://localhost/b".to_string(),
            model: "model-b".to_string(),
            context_window: None,
            engine: Some("Local".to_owned()),
            api_key: None,
            env_key: None,
            tool_protocol: None,
            enable_thinking: None,
            reasoning_effort: None,
            max_tokens: None,
            supports_vision: None,
            ..Default::default()
        },
    ];

    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    // Step 1: Open /model picker
    state.show_model_picker = true;
    let h1 = desired_height(&state, &mut transcript, 80, 24);
    assert!(
        h1 >= 14,
        "modal open should request at least 14 rows, got {h1}"
    );
    terminal
        .draw_height(h1, |f| {
            render_with_transcript(f, &mut state, &mut transcript)
        })
        .unwrap();

    // Step 2: Select model and close picker
    state.show_model_picker = false;
    let h2 = desired_height(&state, &mut transcript, 80, 24);
    assert!(
        h2 < h1,
        "viewport should shrink on modal close, h1={h1}, h2={h2}"
    );
    terminal
        .draw_height(h2, |f| {
            render_with_transcript(f, &mut state, &mut transcript)
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rendered = (0..24)
        .map(|r| (0..80).map(|c| buffer[(c, r)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_count = rendered
        .lines()
        .filter(|line| line.contains("Ask RustCode to do anything"))
        .count();
    assert_eq!(
        prompt_count, 1,
        "must have exactly 1 composer prompt, got {prompt_count}:\n{rendered}"
    );
    assert!(
        !rendered.contains("Select model"),
        "picker header must not remain after close"
    );
}

#[test]
fn viewport_expansion_followed_by_shrink_clears_stale_rows() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();

    // Draw tall viewport with some state
    terminal
        .draw_height(16, |f| {
            render_with_transcript(f, &mut state, &mut transcript);
        })
        .unwrap();

    // Draw smaller viewport
    terminal
        .draw_height(6, |f| {
            render_with_transcript(f, &mut state, &mut transcript);
        })
        .unwrap();

    assert_eq!(terminal.area().height, 6);
    let buffer = terminal.backend().buffer();
    // Verify rows below the 6th row are empty
    for row in 6..20 {
        let line: String = (0..60).map(|col| buffer[(col, row)].symbol()).collect();
        assert!(
            line.trim().is_empty(),
            "row {row} below active viewport must be cleared, found: {line:?}"
        );
    }
}

#[test]
fn multiline_input_indentation_aligns_continuation_lines() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = AppState::new();
    state.input_buffer = "first line\nsecond line\nthird line".to_string();
    state.cursor_position = state.input_buffer.len();

    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();

    terminal
        .draw_height(10, |f| {
            render_with_transcript(f, &mut state, &mut transcript);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let mut rendered_lines = Vec::new();
    for row in 0..10 {
        let line: String = (0..80).map(|col| buffer[(col, row)].symbol()).collect();
        if !line.trim().is_empty() {
            rendered_lines.push(line);
        }
    }

    // Check that the first line starts with "› first line" and second line starts with "  second line"
    let first = rendered_lines
        .iter()
        .find(|l| l.contains("first line"))
        .expect("first line rendered");
    let second = rendered_lines
        .iter()
        .find(|l| l.contains("second line"))
        .expect("second line rendered");
    let third = rendered_lines
        .iter()
        .find(|l| l.contains("third line"))
        .expect("third line rendered");

    assert!(
        first.contains("› first line"),
        "first line must start with prompt chevron: {first}"
    );
    assert!(
        second.contains("  second line"),
        "second line must have 2-space padding: {second}"
    );
    assert!(
        third.contains("  third line"),
        "third line must have 2-space padding: {third}"
    );
}

#[test]
fn count_input_lines_accounts_for_prompt_indent() {
    assert_eq!(super::count_input_lines("", 80), 1);
    assert_eq!(super::count_input_lines("hello", 80), 1);
    assert_eq!(super::count_input_lines("hello\nworld", 80), 2);
    assert_eq!(super::count_input_lines("line1\nline2\nline3", 80), 3);

    // With width 10, indent is 2, available is 8 chars per line
    // "12345678" takes 8 chars + 2 indent = 10 -> fits on line 1
    // Next char triggers wrap to line 2
    assert_eq!(super::count_input_lines("12345678", 10), 1);
    assert_eq!(super::count_input_lines("123456789", 10), 2);
}

#[test]
fn input_wraps_at_word_boundaries_before_splitting_long_words() {
    let styled_chars = "alpha beta toggling speed"
        .chars()
        .map(|character| (character, ratatui::style::Style::default()))
        .collect::<Vec<_>>();
    let (lines, cursor_x, cursor_y) = super::wrap_input_chars(
        &styled_chars,
        18,
        styled_chars.len(),
        ratatui::style::Style::default(),
    );
    let rendered = lines
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();

    assert_eq!(rendered.len(), 2);
    assert!(
        !rendered[0].contains("toggling"),
        "word was split: {rendered:?}"
    );
    assert!(
        rendered[1].contains("toggling speed"),
        "rendered: {rendered:?}"
    );
    assert_eq!((cursor_x, cursor_y), (16, 1));

    let long_word = "abcdefghijklmnop";
    let long_word_chars = long_word
        .chars()
        .map(|character| (character, ratatui::style::Style::default()))
        .collect::<Vec<_>>();
    let (long_lines, _, _) = super::wrap_input_chars(
        &long_word_chars,
        10,
        long_word_chars.len(),
        ratatui::style::Style::default(),
    );
    assert_eq!(long_lines.len(), 2, "long words still need hard wrapping");
}

#[test]
fn live_streaming_thinking_block_uses_thought_duration_not_total_generation_time() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(555));
    state.current_thought_started_at =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(2300));
    state.current_thought_tokens = 106;
    state.replace_current_response("<think>\nAnalyzing the project\n");

    let lines = super::render_live_tail(&state, 80, 24);
    let rendered = lines
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();
    let thought_header = rendered
        .iter()
        .find(|l| l.contains("Thought"))
        .expect("Thought header must be rendered");

    assert!(
        thought_header.contains("Thought for 2.3s, 106 tokens"),
        "thought header must show live thought stats: {thought_header}"
    );
    assert!(
        !thought_header.contains("555s"),
        "thought header must not show total generation duration: {thought_header}"
    );
}

#[test]
fn live_streaming_completed_thought_preserves_duration_while_rest_of_response_streams() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(555));
    state.current_thought_started_at = None;
    state.current_thought_time_ms = 43000;
    state.current_thought_tokens = 1400;
    state.replace_current_response(
        "<think>\nAnalyzing the project\n</think>\nHere is the rest of the stream",
    );

    let lines = super::render_live_tail(&state, 80, 24);
    let rendered = lines
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();
    let thought_header = rendered
        .iter()
        .find(|l| l.contains("Thought"))
        .expect("Thought header must be rendered");

    assert!(
        thought_header.contains("Thought for 43s, 1.4k tokens"),
        "thought header must show completed thought stats: {thought_header}"
    );
    assert!(
        !thought_header.contains("555s"),
        "thought header must not show total generation duration: {thought_header}"
    );
}

#[test]
fn command_child_lines_wrap_with_indentation() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.verbosity = Verbosity::High;
    let long_cmd = "curl -sS https://example.com/api/v1/organizations/test -H 'Authorization: Bearer test_token' --data '{\"field\":\"very long content here\"}'";
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: serde_json::json!({"command": long_cmd}).to_string(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "ok")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: Some(0),
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 40, false);
    assert!(
        rendered.len() > 2,
        "long command should wrap across multiple lines: {rendered:?}"
    );
    assert!(rendered[0].to_string().starts_with("• Ran"));
    assert!(rendered[1].to_string().starts_with("  └ Bash"));
    // Continuation lines must have indentation ("    ")
    for line in &rendered[2..] {
        let text = line.to_string();
        assert!(
            text.starts_with("    ") || text.is_empty(),
            "wrapped line must be indented with 4 spaces: {text:?}"
        );
    }
}

#[test]
fn default_turn_separator_is_lighter_color() {
    let default_palette = super::theme::get_palette("default");
    assert_eq!(
        default_palette.turn_separator,
        ratatui::style::Color::Rgb(90, 112, 126)
    );
}

#[test]
fn acceptance_context_modal_renders_usage_and_breakdown() {
    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("user", "Hello assistant"));
    state.history.push(ChatMessage::new(
        "assistant",
        "Hello! How can I help you today?",
    ));
    state.show_context_modal = true;

    let breakdown = modals::calculate_context_breakdown(&state.render_snapshot());
    assert!(breakdown.user_tokens > 0);
    assert!(breakdown.assistant_tokens > 0);
    assert!(breakdown.total_used > 0);

    let rendered = render_context_modal_to_text(&state, 120, 24);
    assert!(rendered.contains("context usage"), "rendered: {rendered:?}");
    assert!(
        rendered.contains("Token usage by category"),
        "rendered: {rendered:?}"
    );
    assert!(rendered.contains("User messages"), "rendered: {rendered:?}");
    assert!(
        rendered.contains("Agent responses"),
        "rendered: {rendered:?}"
    );
    assert!(rendered.contains("Free space"), "rendered: {rendered:?}");

    let lines = rendered.lines().collect::<Vec<_>>();
    let header_row = lines
        .iter()
        .position(|line| line.contains("context usage"))
        .expect("context header should be rendered");
    let summary_row = lines
        .iter()
        .position(|line| line.contains(" tokens (") && line.contains(" · "))
        .expect("context summary should be rendered");
    let first_grid_row = lines
        .iter()
        .position(|line| line.chars().take(60).collect::<String>().contains("● "))
        .expect("context grid should be rendered");
    let category_header_row = lines
        .iter()
        .position(|line| line.contains("Token usage by category"))
        .expect("category header should be rendered");
    assert_eq!(summary_row, header_row + 3);
    assert_eq!(first_grid_row, summary_row);
    assert_eq!(category_header_row, summary_row + 2);
}
