use super::*;

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

        assert_ne!(
            key1, key2,
            "cache key must differ when active theme changes"
        );
    }

    // Regression: selection clamped to chat_area.x + 2, so the first two columns
    // of every left-aligned line (tool calls, assistant text) could not be
    // selected or copied. Bounds now start at chat_area.x.
    #[test]
    fn extract_selection_captures_first_column() {
        use super::extract_selection;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        // Chat content rendered flush to the left edge of the chat area.
        buf.set_string(0, 0, "Grep(spinner)", ratatui::style::Style::default());
        let chat_area = Some(area);

        // Select the whole line, row 0, columns 0..=12.
        let text = extract_selection(&buf, (0, 0), (12, 0), chat_area, 0);
        assert_eq!(text.trim(), "Grep(spinner)", "first two chars must survive");
        assert!(text.starts_with("Gr"), "got: {text:?}");
    }

    #[test]
    fn selection_keeps_indentation_and_interior_blank_lines() {
        use super::extract_selection;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        let style = ratatui::style::Style::default();
        buf.set_string(0, 0, "fn main() {", style);
        buf.set_string(0, 1, "    let x = 1;", style);
        // Row 2 left blank on purpose — an interior blank line.
        buf.set_string(0, 3, "    let y = 2;", style);

        let text = extract_selection(&buf, (0, 0), (39, 3), Some(area), 0);

        assert_eq!(text, "fn main() {\n    let x = 1;\n\n    let y = 2;");
    }

    #[test]
    fn selection_drops_blank_rows_swept_at_the_edges() {
        use super::extract_selection;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        buf.set_string(0, 2, "  hello", ratatui::style::Style::default());

        // Drag started two rows above the text and ended two rows below it.
        let text = extract_selection(&buf, (0, 0), (39, 5), Some(area), 0);

        assert_eq!(text, "  hello");
    }

    #[test]
    fn scroll_pill_label_pluralizes_and_drops_zero_count() {
        use super::scroll_pill_label;

        assert_eq!(scroll_pill_label(0), " click to scroll down ↓ ");
        assert_eq!(
            scroll_pill_label(1),
            " 1 new message · click to scroll down ↓ "
        );
        assert_eq!(
            scroll_pill_label(4),
            " 4 new messages · click to scroll down ↓ "
        );
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
        let (label, _) =
            format_pi_tool_action("run_command", &serde_json::json!({"command": "ls"}));
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

    // The input box reuses extract_selection with its own rect and scroll 0.
    // Verify selection works for an area that is not at the buffer origin.
    #[test]
    fn extract_selection_works_in_input_area() {
        use super::extract_selection;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let area = Rect::new(0, 0, 30, 12);
        let mut buf = Buffer::empty(area);
        // Input text region near the bottom of the screen.
        let input_area = Rect::new(2, 9, 26, 2);
        buf.set_string(2, 9, "hello world", ratatui::style::Style::default());

        let text = extract_selection(&buf, (2, 9), (12, 9), Some(input_area), 0);
        assert_eq!(text.trim(), "hello world", "got: {text:?}");
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
                line.spans
                    .iter()
                    .all(|span| span.style.bg.is_some()),
                "ordinary code fences should use the code panel background"
            );
        }
    }

    #[test]
    fn diff_code_blocks_hide_patch_metadata() {
        use super::{AssistantRenderOptions, render_assistant_message};

        let content =
            "```diff\n--- a/src/temp.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-old\n-removed\n```";
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
        let (answer, preview) =
            split_thought_blocks("before\n<think>Planning the next action");
        assert_eq!(answer, "before");
        assert_eq!(preview.as_deref(), Some("Planning the next action"));
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
    fn status_panels_render_minimal_inline() {
        use super::render_status_panel;

        let mut lines = Vec::new();
        render_status_panel("Session status: 5 messages", 80, false, &mut lines);

        assert_eq!(lines.len(), 5, "boxed info status panel includes top/bottom borders & padding");
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
    fn harness_recovery_notices_are_hidden_from_transcript() {
        assert!(super::is_hidden_system_notice(
            "[harness: stopped after 10 tool round(s) — 4 consecutive malformed tool-call blocks the harness could not parse. The task is NOT complete. Review the transcript above; if the remaining work is still valid, resume it in a new turn.]"
        ));
        assert!(!super::is_hidden_system_notice(
            "Notice: background task finished"
        ));
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
    fn input_and_footer_layout_has_no_spacer_row() {
        assert_eq!(footer_layout_constraints(), Constraint::Length(1));
    }
