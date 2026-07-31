//! Structured rendering for native tool results.

use ratatui::{
    style::{Color, Modifier},
    text::{Line, Span},
};
use std::path::Path;

use super::{
    COLOR_BG, COLOR_MUTED, COLOR_TEXT, get_themed_style, highlight_code_block,
    highlight_code_line, render_unified_diff, wrap_code_spans,
};

const MAX_RENDERED_TOOL_LINES: usize = 300;

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
    let mut lines = match tool_name {
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
        "use_skill"
        | "set_goal"
        | "todo_write"
        | "spawn_agent"
        | "send_agent"
        | "complete_task"
        | "ask_question"
        | "manage_task" => Vec::new(),
        _ => render_generic_result(result, show_picker),
    };

    if lines.len() > MAX_RENDERED_TOOL_LINES {
        let hidden = lines.len() - MAX_RENDERED_TOOL_LINES;
        lines.truncate(MAX_RENDERED_TOOL_LINES);
        lines.push(Line::from(Span::styled(
            format!("  … {hidden} more lines hidden; use the tool result or rerun a narrower query"),
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::ITALIC, show_picker),
        )));
    }
    lines
}

fn render_mutation_result<'a>(result: &str, width: usize, show_picker: bool) -> Vec<Line<'a>> {
    let Some(summary) = result.lines().find(|line| !line.trim().is_empty()) else {
        return Vec::new();
    };
    let failed = summary.starts_with("error:") || summary.starts_with("Error:");
    let (icon, color) = if failed {
        ("✗", Color::Rgb(229, 123, 123))
    } else {
        ("✓", super::COLOR_SECONDARY)
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
                ("· ", super::COLOR_TEXT)
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
    result
        .lines()
        .filter(|raw| !raw.trim().is_empty())
        .map(|raw| {
            let is_error = raw.trim_start().to_ascii_lowercase().starts_with("error")
                || raw.trim_start().starts_with('✗');
            let color = if is_error {
                Color::Rgb(229, 123, 123)
            } else {
                COLOR_MUTED
            };
            Line::from(Span::styled(
                format!("  │ {raw}"),
                get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
            ))
        })
        .collect()
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
    let status_icon = if succeeded { "✓" } else { "✗" };
    let status_color = if succeeded {
        super::COLOR_SECONDARY
    } else {
        Color::Rgb(229, 123, 123)
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("  {status_icon} exit {code}"),
            get_themed_style(status_color, COLOR_BG, Modifier::BOLD, show_picker),
        ),
    ])];

    for (kind, raw) in output {
        let (prefix, color) = if kind == "stderr" {
            ("  ! ", Color::Rgb(229, 192, 123))
        } else {
            ("  │ ", COLOR_TEXT)
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{raw}"),
            get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
        )));
    }

    lines
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
                raw.strip_prefix("[File: ").and_then(|header| header.strip_suffix(']'))
                    .map(|header| header.replace(", ", " · "))
                    .unwrap_or_else(|| raw.to_string()),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
            )));
            continue;
        }
        let Some((number, code)) = raw.split_once(": ") else {
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::empty(), show_picker),
            )));
            continue;
        };
        let Ok(number) = number.parse::<usize>() else {
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::empty(), show_picker),
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
                    get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::empty(), show_picker),
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{COLOR_MUTED, MAX_RENDERED_TOOL_LINES, render_file_preview, render_tool_result};
    use ratatui::style::Color;

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
        assert!(lines[0].spans[0].content.contains("✓ exit 0"));
        assert!(lines[1].spans[0].content.contains("│ cargo test"));
        assert_eq!(lines.len(), 2);
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
        assert!(lines[0].spans[0].content.contains("✓ successfully replaced"));
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
            let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
            text.contains("new")
        }));
        assert!(!lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("@@"))
        }));
    }

    #[test]
    fn control_plane_results_are_hidden() {
        assert!(render_tool_result("use_skill", "loaded skill", 80, false).is_empty());
        assert!(render_tool_result("spawn_agent", "agent done", 80, false).is_empty());
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
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(Color::Rgb(229, 123, 123))
        );
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
            line.spans.iter().any(|span| span.content.contains("println!"))
        }));
    }

    #[test]
    fn large_results_are_collapsed_for_transcript_rendering() {
        let result = (0..350)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_tool_result("run_command", &result, 80, false);

        assert_eq!(lines.len(), MAX_RENDERED_TOOL_LINES + 1);
        assert!(lines.last().unwrap().spans[0].content.contains("51 more lines"));
    }
}
