//! Structured rendering for native tool results.

use ratatui::{
    style::{Color, Modifier},
    text::{Line, Span},
};
use std::path::Path;

use super::{
    COLOR_BG, COLOR_MUTED, COLOR_TEXT, get_themed_style, highlight_code_block, highlight_code_line,
    render_unified_diff, wrap_code_spans,
};

/// Read, search and generic results are chatty and easy to re-request, so they
/// stay tightly capped in the transcript.
const MAX_RENDERED_TOOL_LINES: usize = 5;
/// Command output is the one result the user usually has to audit themselves
/// (a build log, a test run), so it gets a much larger budget.
const MAX_RENDERED_COMMAND_LINES: usize = 24;

fn language_for_path(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("text")
}

pub(super) fn render_file_preview<'a>(
    path: &str,
    content: &str,
    width: usize,
    show_picker: bool,
) -> Vec<Line<'a>> {
    let language = language_for_path(path);
    let mut lines = vec![Line::from(Span::styled(
        format!("  {} · {}", path, language),
        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
    ))];
    for spans in highlight_code_block(content, language, show_picker) {
        let mut row = vec![Span::styled(
            "  ",
            get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::empty(), show_picker),
        )];
        row.extend(spans);
        lines.extend(wrap_code_spans(row, width.max(10), COLOR_BG, show_picker));
    }
    lines
}

fn line_number<'a>(old: &str, _width: usize, show_picker: bool) -> Span<'a> {
    Span::styled(
        format!("{old:>5} │ "),
        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
    )
}

/// Render a tool result as a compact transcript cell instead of one unstyled
/// paragraph. Read results become source snippets; grep results get match
/// gutters; other results remain readable plain output.
pub(super) fn render_tool_result<'a>(
    tool_name: &str,
    result: &str,
    width: usize,
    show_picker: bool,
) -> Vec<Line<'a>> {
    let lines = match tool_name {
        "view_file" => render_read_result(result, width, show_picker),
        "grep" => render_search_result(result, width, show_picker),
        "glob" | "list_directory" => render_directory_result(result, show_picker),
        "run_command" => render_command_result(result, show_picker),
        "replace_file_content"
        | "multi_replace_file_content"
        | "write_to_file"
        | "delete_file"
        | "move_file"
        | "copy_file" => render_mutation_result(result, width, show_picker),
        // The action line already communicates control-plane lifecycle. Their
        // raw acknowledgement is implementation noise in the transcript.
        "use_skill" | "set_goal" | "todo_write" | "spawn_agent" | "send_agent"
        | "complete_task" | "ask_question" | "manage_task" => Vec::new(),
        _ => render_generic_result(result, show_picker),
    };

    let is_diff_result = matches!(
        tool_name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
    ) && result.contains("```diff");
    if is_diff_result {
        return lines;
    }

    if tool_name == "run_command" {
        // A failed command puts the useful part (the compiler error, the failing
        // assertion) at the bottom, so keep the tail and pin the status row.
        let failed = command_exit_code(result).is_some_and(|code| code != 0);
        let pinned = usize::from(failed && !lines.is_empty());
        collapse(
            lines,
            MAX_RENDERED_COMMAND_LINES,
            pinned,
            failed,
            show_picker,
        )
    } else {
        collapse(lines, MAX_RENDERED_TOOL_LINES, 0, false, show_picker)
    }
}

/// Trim `lines` to `cap` rows plus an elision notice. The first `pinned` rows
/// always survive; `keep_tail` decides whether the surviving remainder is taken
/// from the start or the end of the body.
fn collapse<'a>(
    mut lines: Vec<Line<'a>>,
    cap: usize,
    pinned: usize,
    keep_tail: bool,
    show_picker: bool,
) -> Vec<Line<'a>> {
    if lines.len() <= cap {
        return lines;
    }
    let body_budget = cap.saturating_sub(pinned);
    let hidden = lines.len() - pinned - body_budget;
    let notice = Line::from(Span::styled(
        format!("  … {hidden} more lines"),
        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::ITALIC, show_picker),
    ));

    if keep_tail {
        let body = lines.split_off(lines.len() - body_budget);
        lines.truncate(pinned);
        lines.push(notice);
        lines.extend(body);
        lines
    } else {
        lines.truncate(pinned + body_budget);
        lines.push(notice);
        lines
    }
}

fn command_exit_code(result: &str) -> Option<i32> {
    result
        .lines()
        .find_map(|raw| raw.strip_prefix("exit code: "))
        .and_then(|code| code.trim().parse::<i32>().ok())
}

fn render_mutation_result<'a>(result: &str, width: usize, show_picker: bool) -> Vec<Line<'a>> {
    let Some(summary) = result.lines().find(|line| !line.trim().is_empty()) else {
        return Vec::new();
    };
    let failed = summary.starts_with("error:") || summary.starts_with("Error:");
    let (icon, color) = if failed {
        ("●", Color::Rgb(229, 123, 123))
    } else {
        ("●", super::COLOR_GREEN)
    };
    let diffs: Vec<&str> = result
        .split("```diff")
        .skip(1)
        .filter_map(|block| block.split_once("```").map(|(diff, _)| diff.trim()))
        .filter(|diff| !diff.is_empty())
        .collect();

    let mut lines = Vec::new();
    if !diffs.is_empty() {
        for diff in diffs {
            // The Edit heading already identifies this as a patch. Hunk metadata
            // is useful to a patch parser but adds visual noise in the transcript.
            let diff_body = diff
                .lines()
                .filter(|line| !line.trim_start().starts_with("@@"))
                .collect::<Vec<_>>()
                .join("\n");
            lines.extend(render_unified_diff(&diff_body, width, show_picker));
        }
    } else {
        lines.push(Line::from(Span::styled(
            format!("  {icon} {summary}"),
            get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
        )));
    }
    lines
}

fn render_directory_result<'a>(result: &str, show_picker: bool) -> Vec<Line<'a>> {
    result
        .lines()
        .map(|raw| {
            let (marker, color) = if raw.ends_with('/') {
                ("▸ ", super::COLOR_PRIMARY)
            } else if raw.contains(" file(s) matched") || raw.starts_with("no files") {
                ("", super::COLOR_MUTED)
            } else {
                ("· ", super::COLOR_MUTED)
            };
            Line::from(vec![
                Span::styled(
                    marker,
                    get_themed_style(color, COLOR_BG, Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    raw.to_string(),
                    get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
                ),
            ])
        })
        .collect()
}

/// Render an unclassified tool result as a quiet transcript block. The action
/// line above already identifies the tool, so the body only needs a muted
/// gutter and enough error contrast to remain actionable.
fn render_generic_result<'a>(result: &str, show_picker: bool) -> Vec<Line<'a>> {
    // The harness knows nothing about this tool's formatting, so interior blank
    // lines are the only paragraph structure it has. Keep them, but collapse
    // long runs so a padded result cannot eat the whole line budget.
    let body: Vec<&str> = result.lines().collect();
    let start = body.iter().position(|raw| !raw.trim().is_empty());
    let Some(start) = start else {
        return Vec::new();
    };
    let end = body
        .iter()
        .rposition(|raw| !raw.trim().is_empty())
        .unwrap_or(start);

    let mut lines = Vec::new();
    let mut blank_run = 0usize;
    for raw in &body[start..=end] {
        if raw.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
            lines.push(Line::from(Span::styled(
                "  │".to_string(),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            )));
            continue;
        }
        blank_run = 0;
        let is_error = raw.trim_start().to_ascii_lowercase().starts_with("error")
            || raw.trim_start().starts_with('✗');
        let color = if is_error {
            Color::Rgb(229, 123, 123)
        } else {
            COLOR_MUTED
        };
        lines.push(Line::from(Span::styled(
            format!("  │ {raw}"),
            get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
        )));
    }
    lines
}

fn render_command_result<'a>(result: &str, show_picker: bool) -> Vec<Line<'a>> {
    let mut exit_code = None;
    let mut section = "stdout";
    let mut output = Vec::new();

    for raw in result.lines() {
        if let Some(code) = raw.strip_prefix("exit code: ") {
            exit_code = code.trim().parse::<i32>().ok();
        } else if raw == "stdout:" {
            section = "stdout";
        } else if raw == "stderr:" {
            section = "stderr";
        } else if raw != "(no output)" && !raw.is_empty() {
            output.push((section, raw));
        }
    }

    let code = exit_code.unwrap_or(0);
    let succeeded = code == 0;
    let mut lines = Vec::new();
    if !succeeded {
        lines.push(Line::from(Span::styled(
            format!("  ✗ exit {code}"),
            get_themed_style(
                Color::Rgb(229, 123, 123),
                COLOR_BG,
                Modifier::BOLD,
                show_picker,
            ),
        )));
    }

    for (kind, raw) in output {
        let (prefix, color) = if kind == "stderr" {
            ("  ! ", Color::Rgb(229, 192, 123))
        } else {
            ("  │ ", COLOR_MUTED)
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{raw}"),
            get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
        )));
    }

    lines
}

/// Turn `[File: path, Lines X to Y of Z, Bytes offset: N]` into a readable
/// header. The byte offset only exists so the agent can resume a read; it is
/// harness bookkeeping and means nothing in the human transcript.
fn format_read_header(raw: &str) -> String {
    let Some(header) = raw
        .strip_prefix("[File: ")
        .and_then(|header| header.strip_suffix(']'))
    else {
        return raw.to_string();
    };
    header
        .split(", ")
        .filter(|segment| !segment.starts_with("Bytes offset:"))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn render_read_result<'a>(result: &str, width: usize, show_picker: bool) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let mut language = "text";
    for (index, raw) in result.lines().enumerate() {
        if index == 0 {
            if let Some(path) = raw
                .strip_prefix("[File: ")
                .and_then(|header| header.split_once(',').map(|(path, _)| path))
            {
                language = language_for_path(path);
            }
            lines.push(Line::from(Span::styled(
                format_read_header(raw),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
            )));
            continue;
        }
        let Some((number, code)) = raw.split_once(": ") else {
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            )));
            continue;
        };
        let Ok(number) = number.parse::<usize>() else {
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            )));
            continue;
        };
        let mut row = vec![line_number(&number.to_string(), width, show_picker)];
        row.extend(highlight_code_line(code, language, show_picker));
        lines.push(Line::from(row));
    }
    lines
}

fn render_search_result<'a>(result: &str, _width: usize, show_picker: bool) -> Vec<Line<'a>> {
    let mut language = "text";
    result
        .lines()
        .map(|raw| {
            let is_path = raw.ends_with(':') && !raw.starts_with("  ");
            if is_path {
                language = language_for_path(raw.trim_end_matches(':'));
                Line::from(Span::styled(
                    raw.to_string(),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
                ))
            } else if let Some((number, text)) = raw.trim_start().split_once(": ") {
                let mut row = vec![line_number(number, 0, show_picker)];
                row.extend(highlight_code_line(text, language, show_picker));
                Line::from(row)
            } else {
                Line::from(Span::styled(
                    raw.to_string(),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        COLOR_MUTED, MAX_RENDERED_COMMAND_LINES, MAX_RENDERED_TOOL_LINES, render_file_preview,
        render_tool_result,
    };
    use ratatui::style::Color;

    fn text_of(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn read_results_have_header_and_line_numbered_code() {
        let lines = render_tool_result(
            "view_file",
            "[File: src/main.rs, Lines 4 to 5 of 5]\n4: fn main() {}",
            80,
            false,
        );
        assert_eq!(lines.len(), 2);
        let text: String = lines[1]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.starts_with("    4 │ "));
        assert!(text.contains("fn main"));
    }

    #[test]
    fn grep_results_distinguish_file_headers_and_matches() {
        let lines = render_tool_result("grep", "src/main.rs:\n  12: fn main() {}", 80, false);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].spans[0].content.contains("src/main.rs"));
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|span| span.content.contains("12"))
        );
    }

    #[test]
    fn directory_results_get_tree_markers() {
        let lines = render_tool_result("list_directory", "src/\nmain.rs", 80, false);
        assert!(lines[0].spans[0].content.contains('▸'));
        assert!(lines[1].spans[0].content.contains('·'));
    }

    #[test]
    fn command_results_have_compact_status_and_output() {
        let lines = render_tool_result(
            "run_command",
            "exit code: 0\nstdout:\ncargo test\nstderr:\n",
            80,
            false,
        );
        assert!(!lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("exit 0"))
        }));
        assert!(lines[0].spans[0].content.contains("│ cargo test"));
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn failed_commands_use_error_status_and_stderr_marker() {
        let lines = render_tool_result(
            "run_command",
            "exit code: 1\nstderr:\npermission denied",
            80,
            false,
        );
        assert!(lines[0].spans[0].content.contains("✗ exit 1"));
        assert!(lines[1].spans[0].content.contains("! permission denied"));
    }

    #[test]
    fn edit_results_show_only_a_compact_success_summary() {
        let lines = render_tool_result(
            "replace_file_content",
            "successfully replaced target_content in 'src/main.rs'",
            80,
            false,
        );
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].spans[0]
                .content
                .contains("● successfully replaced")
        );
    }

    #[test]
    fn edit_results_preserve_embedded_diffs() {
        let lines = render_tool_result(
            "replace_file_content",
            "successfully replaced target_content in 'src/main.rs'\n\n```diff\n@@\n-old\n+new\n```",
            80,
            false,
        );
        assert!(lines.len() > 1);
        assert!(lines.iter().any(|line| {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            text.contains("new")
        }));
        assert!(
            !lines
                .iter()
                .any(|line| { line.spans.iter().any(|span| span.content.contains("@@")) })
        );
    }

    #[test]
    fn control_plane_results_are_hidden() {
        assert!(render_tool_result("use_skill", "loaded skill", 80, false).is_empty());
        assert!(render_tool_result("spawn_agent", "agent done", 80, false).is_empty());
    }

    #[test]
    fn tool_output_uses_darker_muted_color() {
        let lines = render_tool_result(
            "run_command",
            "exit code: 0\nstdout:\nhello world",
            80,
            false,
        );
        assert!(!lines.is_empty());
        let last = lines.last().unwrap();
        assert_eq!(last.spans[0].style.fg, Some(COLOR_MUTED));
    }

    #[test]
    fn generic_results_are_muted_and_keep_errors_visible() {
        let lines = render_tool_result(
            "mcp_custom_tool",
            "completed\nerror: remote service failed",
            80,
            false,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[0].spans[0].content.starts_with("  │ completed"));
        assert_eq!(lines[0].spans[0].style.fg, Some(COLOR_MUTED));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Rgb(229, 123, 123)));
    }

    #[test]
    fn write_previews_render_as_normal_highlighted_code() {
        let lines = render_file_preview(
            "src/temp.rs",
            "fn greet() {\n    println!(\"hello\");\n}",
            80,
            false,
        );
        assert!(lines[0].spans[0].content.contains("src/temp.rs"));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("println!"))
        }));
    }

    #[test]
    fn large_results_are_collapsed_for_transcript_rendering() {
        let result = (0..350)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_tool_result("mcp_custom_tool", &result, 80, false);

        assert_eq!(lines.len(), MAX_RENDERED_TOOL_LINES + 1);
        assert!(
            lines.last().unwrap().spans[0]
                .content
                .contains("345 more lines")
        );
    }

    #[test]
    fn command_results_get_a_larger_line_budget_than_read_results() {
        let result = (0..350)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let command = render_tool_result("run_command", &result, 80, false);
        let generic = render_tool_result("mcp_custom_tool", &result, 80, false);

        assert_eq!(command.len(), MAX_RENDERED_COMMAND_LINES + 1);
        assert!(command.len() > generic.len());
        assert!(
            command.last().unwrap().spans[0]
                .content
                .contains("326 more lines")
        );
    }

    #[test]
    fn failed_commands_keep_the_status_line_and_the_output_tail() {
        let body = (0..200)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = format!("exit code: 101\nstderr:\n{body}\nerror: build failed");
        let lines = render_tool_result("run_command", &result, 80, false);

        assert_eq!(lines.len(), MAX_RENDERED_COMMAND_LINES + 1);
        assert!(text_of(&lines[0]).contains("✗ exit 101"));
        assert!(text_of(&lines[1]).contains("more lines"));
        // The tail carries the failure reason; the head is boilerplate.
        assert!(text_of(lines.last().unwrap()).contains("error: build failed"));
        assert!(!lines.iter().any(|line| text_of(line).contains("line 0")));
    }

    #[test]
    fn truncation_notice_has_no_model_directed_instructions() {
        let result = (0..40)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_tool_result("grep", &result, 80, false);
        let notice = text_of(lines.last().unwrap());

        assert_eq!(notice.trim(), "… 35 more lines");
        assert!(!notice.contains("rerun"));
        assert!(!notice.contains("tool result"));
    }

    #[test]
    fn generic_results_preserve_interior_blank_lines() {
        let lines = render_tool_result("mcp_custom_tool", "first\n\nsecond", 80, false);

        assert_eq!(lines.len(), 3);
        assert!(text_of(&lines[0]).contains("first"));
        assert_eq!(text_of(&lines[1]).trim_end(), "  │");
        assert!(text_of(&lines[2]).contains("second"));
    }

    #[test]
    fn generic_results_collapse_blank_runs_and_trim_edges() {
        let lines = render_tool_result("mcp_custom_tool", "\n\nfirst\n\n\n\nsecond\n\n", 80, false);

        assert_eq!(lines.len(), 3);
        assert!(text_of(&lines[0]).contains("first"));
        assert!(text_of(&lines[2]).contains("second"));
    }

    #[test]
    fn read_headers_hide_the_byte_offset() {
        let lines = render_tool_result(
            "view_file",
            "[File: src/main.rs, Lines 1 to 2 of 9, Bytes offset: 0]\n1: fn main() {}",
            80,
            false,
        );
        let header = text_of(&lines[0]);

        assert!(!header.contains("Bytes offset"));
        assert_eq!(header, "src/main.rs · Lines 1 to 2 of 9");
    }

    #[test]
    fn embedded_diffs_are_not_truncated() {
        let diff = (0..8)
            .map(|index| format!("-removed line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = format!("successfully edited file\n\n```diff\n{diff}\n```");
        let lines = render_tool_result("replace_file_content", &result, 80, false);

        assert!(lines.len() > MAX_RENDERED_TOOL_LINES);
        assert!(
            !lines
                .iter()
                .any(|line| text_of(line).contains("more lines"))
        );
    }
}
