use super::*;

#[test]
fn model_picker_keeps_multiple_models_visible_above_the_composer() {
    use ratatui::{backend::TestBackend, Terminal};

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
            max_tokens: None,
            supports_vision: None,
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
    use ratatui::{backend::TestBackend, Terminal};

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
    let visible_commands = ["New session", "Resume session", "Copy last reply"]
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
    use ratatui::{backend::TestBackend, Terminal};

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
        .find(|row| row_text(*row).contains("Auto-Confirm"))
        .expect("composer status row should be visible");
    let popup_row = (0..20)
        .find(|row| row_text(*row).contains("/cancel"))
        .expect("inline command popup should be visible");

    assert!(
        popup_row > composer_row,
        "popup should be below the composer: composer={composer_row}, popup={popup_row}"
    );
}

#[test]
fn welcome_banner_renders_without_a_conversation() {
    use ratatui::{Terminal, backend::TestBackend};

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
        rendered.contains("Welcome back!"),
        "the empty chat must display its welcome banner: {rendered:?}"
    );
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
fn welcome_banner_adapts_to_small_viewports_without_truncating_box() {
    let state = AppState::new();
    // Test with small height = 6
    let lines = super::render_live_tail(&state, 100, 6);
    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("Welcome back!"));
    assert!(text.contains("╰"), "banner must end cleanly with a bottom border");
}


#[test]
fn queue_preview_shows_recent_user_prompts_without_wakeups() {
    use ratatui::{Terminal, backend::TestBackend};

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

    let verbosity = crate::app::Verbosity::Low;
    theme::set_active_theme("default");
    let key1 = tool_result_cache_key("Bash", "result 0", 80, &verbosity, false);
    theme::set_active_theme("nord");
    let key2 = tool_result_cache_key("Bash", "result 0", 80, &verbosity, false);

    assert_ne!(key1, key2, "cache key must differ when active theme changes");
}

#[test]
fn notice_toast_sits_top_right_and_clamps() {
    use super::notice_rect;
    use ratatui::layout::Rect;

    let screen = Rect::new(0, 0, 100, 40);
    let r = notice_rect(screen, 18).unwrap();
    // 18 text + 5 padding (glyph + spaces) = 23 wide, borderless single row.
    assert_eq!(r.width, 23);
    assert_eq!(r.height, 1);
    // Right-aligned with a one-column gutter, one row down from the top.
    assert_eq!(r.x, 100 - 23 - 1);
    assert_eq!(r.y, 1);
    assert!(r.x + r.width < screen.width, "must stay on screen");

    // Very wide text is clamped to the screen width.
    let wide = notice_rect(screen, 500).unwrap();
    assert!(wide.x + wide.width <= screen.width);

    // Tiny screen → no toast.
    assert!(notice_rect(Rect::new(0, 0, 2, 1), 5).is_none());
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
    );
    assert_eq!(label, "UseSkill");
    assert_eq!(arg, "git-feature-workflow");

    let (label, arg) =
        format_pi_tool_action("complete_task", &serde_json::json!({"result": "done"}));
    assert_eq!(label, "CompleteTask");
    assert_eq!(arg, "result=\"done\"");

    let (label, arg) = format_pi_tool_action("complete_task", &serde_json::json!({}));
    assert_eq!(label, "CompleteTask");
    assert_eq!(arg, "");

    // Built-in aliases are unchanged.
    let (label, _) = format_pi_tool_action("run_command", &serde_json::json!({"command": "ls"}));
    assert_eq!(label, "Bash");
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
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
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

    assert!(rendered
        .iter()
        .any(|line| line.contains("• Ran $ cargo test · exit 0")));
    assert!(
        rendered.iter().any(|line| line.contains("504 passed")),
        "command output must be rendered beneath its header: {rendered:?}"
    );
}

#[test]
fn committed_tool_result_shows_failure_status() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
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

    assert!(rendered
        .iter()
        .any(|line| line.contains("• Ran $ cargo test · exit 1")));
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

    assert!(rendered.iter().any(|line| line == "• Tool"));
    assert!(
        rendered
            .iter()
            .any(|line| line.starts_with("  └ UseSkill clockify"))
    );
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

    assert_eq!(rendered, ["• Tool", "  └ UseSkill clockify"]);
}

#[test]
fn high_verbosity_expands_generic_tool_details() {
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

    assert!(rendered.iter().any(|line| line.contains("completed")));
    assert!(rendered.iter().any(|line| line.contains("line 2")));
    assert!(!rendered.iter().any(|line| line.contains("ctrl+o")));
}

#[test]
fn verbosity_changes_bounded_tool_presentation_without_mutating_history() {
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = AppState::new();
    state.history.push(ChatMessage::new("assistant", "").with_tool_calls(vec![
        ToolCallRef {
            id: "call-1".to_owned(),
            name: "mcp_custom_tool".to_owned(),
            arguments: r#"{"path":"src"}"#.to_owned(),
        },
    ]));
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
    assert!(high.iter().any(|line| line.contains("line 49")));
    assert!(high.iter().any(|line| line.contains("… +31 lines")));
    assert!(!high.iter().any(|line| line.contains("line 25")));
    assert!(low.len() < high.len());
    assert!(state.history == history);
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
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Edit src/main.rs"))
    );
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
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
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
    use crate::app::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = AppState::new();
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
fn code_block_rows_fill_full_width() {
    use super::{AssistantRenderOptions, render_assistant_message};
    use unicode_width::UnicodeWidthStr;

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

    // Exactly one code panel → one copy button, anchored to the header row.
    assert_eq!(copies.len(), 1);
    let header_idx = copies[0].0;

    // Header + 3 body rows (text, blank, text) must each be
    // exactly `width` display columns.
    for line in &lines[header_idx..header_idx + 4] {
        let w: usize = line.spans.iter().map(|s| s.content.width()).sum();
        assert_eq!(w, width as usize, "code panel row must fill full width");
    }
    for line in &lines[header_idx + 1..header_idx + 4] {
        assert!(
            line.spans.iter().all(|span| span.style.bg.is_some()),
            "ordinary code fences should use the code panel background"
        );
    }
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
fn diff_code_blocks_hide_patch_metadata() {
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
    assert!(!rendered.contains("a/src/temp.rs"));
    assert!(!rendered.contains("/dev/null"));
    assert!(!rendered.contains("@@ -1,2"));
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
    assert_eq!(
        preview.as_deref(),
        Some("Reasoning about user request.")
    );
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
}

#[test]
fn new_chat_separator_spans_width_and_centers_label() {
    use super::push_new_chat_separator;
    use unicode_width::UnicodeWidthStr;

    let mut lines = Vec::new();
    push_new_chat_separator(&mut lines, 40, false);

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].width(), 40);
    assert_eq!(lines[0].spans[1].content, " ✨ NEW CHAT ");
    assert_eq!(lines[1].width(), 0);

    let left = lines[0].spans[0].content.width();
    let right = lines[0].spans[2].content.width();
    assert!((left as isize - right as isize).abs() <= 1);
}

#[test]
fn resumed_session_separator_spans_width_and_centers_label() {
    use unicode_width::UnicodeWidthStr;

    let mut lines = Vec::new();
    super::render_status_panel("Resumed session \"My Test Session\"", 60, false, &mut lines);

    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].width(), 60);
    assert_eq!(lines[0].spans[1].content, " Resumed Session ");

    let left = lines[0].spans[0].content.width();
    let right = lines[0].spans[2].content.width();
    assert!((left as isize - right as isize).abs() <= 1);
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
    );
    assert_eq!(action, "ManageTask");
    assert_eq!(arg, "status task-123");

    let (action_list, arg_list) =
        format_pi_tool_action("manage_task", &serde_json::json!({"Action": "list"}));
    assert_eq!(action_list, "ManageTask");
    assert_eq!(arg_list, "list");

    let (action_bg, arg_bg) = format_pi_tool_action(
        "background_task",
        &serde_json::json!({"TaskId": "task-456"}),
    );
    assert_eq!(action_bg, "TaskDone");
    assert_eq!(arg_bg, "task-456");

    let (action2, arg2) = format_pi_tool_action("get_date", &serde_json::json!({}));
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
fn compact_status_formats_empty_context_and_idle_tps() {
    assert_eq!(format_context_info(0, 0, None), "Context: 0 (0%)");
    assert_eq!(format_tps_info(0.0), "Tps: 0.0");
}

#[test]
fn input_footer_only_advertises_command_palette() {
    assert_eq!(input_footer_hint_text(), "Ctrl+P commands");
}

#[test]
fn input_bar_contains_live_status_and_command_hint() {
    let state = AppState::new();
    assert_eq!(
        format_input_status_text(&state),
        "Auto-Confirm: OFF  Context: 0 (0%)  Tps: 0.0  Ctrl+P commands"
    );
    assert_eq!(activity_status_label(&state), "Idle");
    assert_eq!(
        activity_status_line(&state, false)
            .spans
            .last()
            .unwrap()
            .content,
        " "
    );
}

#[test]
fn live_tool_activity_is_rendered_without_protocol_text() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    state.live_tool_calls.push(crate::app::LiveToolCall {
        key: "call-1".to_owned(),
        provider_call_id: None,
        tool_name: "run_command".to_owned(),
        action: "Bash".to_owned(),
        target: "cargo test".to_owned(),
    });

    let line = super::activity_status_line(&state, false).to_string();

    assert!(line.contains("Running"));
    assert!(line.contains("Bash cargo test"));
    assert!(!line.contains("tool_calls"));
}

#[test]
fn live_history_cell_keeps_identical_invocations_visible_separately() {
    let calls = vec![
        crate::app::LiveToolCall {
            key: "local:1".to_owned(),
            provider_call_id: None,
            tool_name: "run_command".to_owned(),
            action: "Bash".to_owned(),
            target: "cargo test".to_owned(),
        },
        crate::app::LiveToolCall {
            key: "local:2".to_owned(),
            provider_call_id: None,
            tool_name: "run_command".to_owned(),
            action: "Bash".to_owned(),
            target: "cargo test".to_owned(),
        },
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
    state.live_tool_calls.push(crate::app::LiveToolCall {
        key: "local:1".to_owned(),
        provider_call_id: None,
        tool_name: "view_file".to_owned(),
        action: "Read".to_owned(),
        target: "src/main.rs".to_owned(),
    });

    let text = super::render_live_tail(&state, 80, 24)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Exploring"));
    assert!(state.history.is_empty());
}

#[test]
fn active_transcript_cell_updates_in_place_and_clears_without_history() {
    let mut transcript = super::TranscriptState::default();

    transcript.set_assistant("first paragraph", false, None);
    let first_revision = transcript.revision();
    assert!(!transcript.display_lines(80).is_empty());

    transcript.set_assistant("first paragraph\n\nsecond", true, None);
    assert!(transcript.revision() > first_revision);
    assert!(!transcript.display_lines(80).is_empty());

    transcript.set_tools(&[crate::app::LiveToolCall {
        key: "call-1".to_owned(),
        provider_call_id: Some("native-1".to_owned()),
        tool_name: "run_command".to_owned(),
        action: "Bash".to_owned(),
        target: "cargo test".to_owned(),
    }]);
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
    state.live_tool_calls.push(crate::app::LiveToolCall {
        key: "question".to_owned(),
        provider_call_id: None,
        tool_name: "ask_question".to_owned(),
        action: "AskQuestion".to_owned(),
        target: "continue?".to_owned(),
    });

    assert_eq!(super::activity_status_label(&state), "Action Required");
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
    assert_eq!(
        cursor.take_stable_stream("alpha\n\nbeta"),
        vec!["alpha"]
    );
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

    assert!(cursor.pending_stable_stream("<think>\nPlanning\n").is_empty());
    assert!(cursor
        .pending_stable_stream("thoughtPlanning the response\n")
        .is_empty());
}

#[test]
fn transcript_cursor_keeps_an_incomplete_code_fence_together() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let stream = "intro\n\n```rust\nfn main() {\n";

    assert_eq!(
        cursor.pending_stable_source(stream),
        "intro\n\n".to_owned()
    );
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
    state.current_response = "stable line\nunclosed tail".to_owned();

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
    state.current_response =
        "<think>\nPlanning\n</think>\n\nFirst answer line\nSecond answer line".to_owned();

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
    state.current_response = "thoughtPlanning the response\n".to_owned();

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

    let prose: Vec<_> = lines
        .iter()
        .filter(|line| !line.spans.is_empty())
        .collect();
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
    let continuation =
        super::render_committed_assistant_chunk(&state, "second line\n", 80, true);

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

    assert_eq!(block[0].spans[0].content, "❯ ");
    assert!(!block[0].spans[1].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn committed_user_message_has_trailing_blank_line() {
    let mut state = AppState::new();
    state
        .history
        .push(ChatMessage::new("user", "check latest 10 commits"));

    let block = super::render_committed_history_block(&state, 0, 80);

    assert_eq!(block.len(), 2);
    assert_eq!(block[0].spans[1].content, "check latest 10 commits");
    assert!(block[1].spans.is_empty());
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

    super::render_assistant_message("<think>\nFirst thought\n</think>\nFirst response", &mut lines, &mut copy_clicks, options);

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

    super::render_assistant_message("<think>\nSecond thought\n</think>\nSecond response", &mut lines, &mut copy_clicks, options2);

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
fn live_tail_keeps_working_for_the_final_painted_frame() {
    let mut state = AppState::new();
    state.status = AppStatus::Idle;
    state.working_status_pending = true;

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("Working"), "rendered live tail: {text:?}");
}

#[test]
fn render_clearing_working_status_pending_requests_follow_up_redraw() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut state = AppState::new();
    state.status = AppStatus::Idle;
    state.working_status_pending = true;
    state.redraw_requested = false;

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut state))
        .unwrap();

    assert!(!state.working_status_pending, "working_status_pending should be reset to false");
    assert!(state.take_redraw_request(), "render must request a follow-up redraw when working_status_pending is cleared");
}

#[test]
fn empty_composer_has_no_extra_blank_rows() {
    use ratatui::{Terminal, backend::TestBackend};

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
            .contains("Ask a question")
    });
    let bottom_border_row = (0..12).find(|y| {
        (0..100)
            .map(|x| buffer[(x, *y)].symbol())
            .collect::<String>()
            .contains("Auto-Confirm")
    });

    let prompt_row = prompt_row.expect("composer prompt should be rendered");
    assert_eq!(bottom_border_row, Some(prompt_row + 1));
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
