//! Structured rendering for native tool results.

use ratatui::{
    style::{Color, Modifier},
    text::{Line, Span},
};
use std::path::Path;

use super::{COLOR_BG, COLOR_MUTED, get_themed_style, highlight_code_line, render_unified_diff};
#[cfg(test)]
use super::{COLOR_TEXT, highlight_code_block, wrap_code_spans};

fn language_for_path(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("text")
}

#[cfg(test)]
pub(super) fn render_file_preview<'a>(
    path: &str,
    content: &str,
    width: usize,
    show_picker: bool,
) -> Vec<Line<'a>> {
    let language = language_for_path(path);
    let mut lines = vec![Line::from(Span::styled(
        format!("  {} · {}", path, language),
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
    ))];
    for spans in highlight_code_block(content, language, show_picker) {
        let mut row = vec![Span::styled(
            "  ",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
        )];
        row.extend(spans);
        lines.extend(wrap_code_spans(row, width.max(10), COLOR_BG(), show_picker));
    }
    lines
}

fn line_number<'a>(old: &str, _width: usize, show_picker: bool) -> Span<'a> {
    Span::styled(
        format!("{old:>5} │ "),
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    )
}

/// Render a tool result as a compact transcript cell instead of one unstyled
/// paragraph. Read results become source snippets; grep results get match
/// gutters; other results remain readable plain output.
pub(super) fn render_tool_result<'a>(
    tool_name: &str,
    result: &str,
    width: usize,
    verbosity: &rustcode::controller::Verbosity,
    show_picker: bool,
) -> Vec<Line<'a>> {
    if matches!(verbosity, rustcode::controller::Verbosity::High) {
        return Vec::new();
    }

    let lower = tool_name.to_ascii_lowercase();
    let lines = match lower.as_str() {
        "view_file" | "viewfile" | "read_file" | "readfile" => {
            render_read_result(result, width, show_picker)
        }
        "grep" | "grep_search" | "grepsearch" => render_search_result(result, width, show_picker),
        "glob" | "list_directory" | "list_dir" | "listdir" => {
            render_directory_result(result, show_picker)
        }
        "run_command" | "bash" => render_command_result(result, show_picker),
        "replace_file_content"
        | "replacefilecontent"
        | "multi_replace_file_content"
        | "multireplacefilecontent"
        | "write_to_file"
        | "writetofile"
        | "write_file"
        | "writefile"
        | "create_file"
        | "createfile"
        | "write_file_chunk"
        | "writefilechunk"
        | "edit_file"
        | "editfile"
        | "patch_file"
        | "patchfile"
        | "delete_file"
        | "deletefile"
        | "move_file"
        | "movefile"
        | "copy_file"
        | "copyfile" => render_mutation_result(result, width, show_picker),
        // The action line already communicates control-plane lifecycle. Their
        // raw acknowledgement is implementation noise in the transcript.
        // `ask_question` is excluded: its result is the user's answer, which
        // must stay visible (question + choice render as the entry headline,
        // the full answer renders here for the expanded view).
        "use_skill" | "set_goal" | "todo_write" | "spawn_agent" | "send_agent" | "cancel_agent"
        | "complete_task" => Vec::new(),
        "ask_question" => render_generic_result(result, show_picker),
        _ => render_generic_result(result, show_picker),
    };

    cap_transcript_lines(lines, show_picker)
}

/// Hard cap on the transcript rows one tool result may claim.
///
/// The engine bounds a payload at `MAX_TOOL_OUTPUT_LINES` (1000) and
/// `MAX_TOOL_OUTPUT_BYTES` (50 KiB), which is a transport limit, not a
/// presentation one: before this cap a single `cat` or a 1000-line diff could
/// take over the viewport, and expanding it was irreversible because the body
/// was written straight into terminal scrollback. Capping here — the one
/// choke point every per-tool renderer already returns through — bounds the
/// body for commands, edits, greps and generic tools alike, expanded or not.
/// The remainder stays in the session log; the marker says so. (#1593)
pub(crate) const TOOL_RESULT_TRANSCRIPT_MAX_LINES: usize = 120;

/// Truncate to [`TOOL_RESULT_TRANSCRIPT_MAX_LINES`], leaving one marker row
/// that names the omitted count so a truncated body never reads as complete.
///
/// The kept rows are the oldest and the newest, with the marker between them.
/// A generic result's first rows are its summary and a failed command's last
/// rows are the error, so dropping the middle is the only cut that cannot
/// discard the part the user opened the result to read (#1593).
fn cap_transcript_lines<'a>(lines: Vec<Line<'a>>, show_picker: bool) -> Vec<Line<'a>> {
    if lines.len() <= TOOL_RESULT_TRANSCRIPT_MAX_LINES {
        return lines;
    }
    let tail = TOOL_RESULT_TRANSCRIPT_MAX_LINES / 4;
    let head = TOOL_RESULT_TRANSCRIPT_MAX_LINES - tail;
    let omitted = lines.len() - TOOL_RESULT_TRANSCRIPT_MAX_LINES;
    let mut capped = Vec::with_capacity(TOOL_RESULT_TRANSCRIPT_MAX_LINES + 1);
    capped.extend_from_slice(&lines[..head]);
    capped.push(Line::from(Span::styled(
        format!("… +{omitted} more lines · full output in the session log"),
        get_themed_style(
            COLOR_MUTED(),
            COLOR_BG(),
            Modifier::ITALIC | Modifier::DIM,
            show_picker,
        ),
    )));
    capped.extend_from_slice(&lines[lines.len() - tail..]);
    capped
}

fn render_mutation_result<'a>(result: &str, width: usize, show_picker: bool) -> Vec<Line<'a>> {
    let Some(summary) = result.lines().find(|line| !line.trim().is_empty()) else {
        return Vec::new();
    };
    let failed = summary.starts_with("error:") || summary.starts_with("Error:");
    let (icon, color) = if failed {
        ("●", Color::Rgb(229, 123, 123))
    } else {
        ("●", super::COLOR_GREEN())
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
            get_themed_style(color, COLOR_BG(), Modifier::empty(), show_picker),
        )));
    }
    lines
}

/// True when an edit result reports a no-op rather than a change.
///
/// No-op and failed changes keep their truthful single-line status; they never
/// synthesize a diff preview.
pub(super) fn edit_result_is_noop(result: &str) -> bool {
    let lower = result.to_ascii_lowercase();
    lower.contains("already applied") || lower.contains("no changes made")
}

/// True when the tool result already carries an embedded unified diff.
pub(super) fn result_has_embedded_diff(result: &str) -> bool {
    result.contains("```diff")
}

fn edit_args_path(args: &serde_json::Value) -> &str {
    args.get("path")
        .or_else(|| args.get("TargetFile"))
        .or_else(|| args.get("target_file"))
        .or_else(|| args.get("AbsolutePath"))
        .or_else(|| args.get("absolute_path"))
        .or_else(|| args.get("file"))
        .or_else(|| args.get("filePath"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn edit_args_content<'a>(tool_name: &str, args: &'a serde_json::Value) -> Option<&'a str> {
    let lower = tool_name.to_ascii_lowercase();
    match lower.as_str() {
        "write_to_file" | "writetofile" | "write_file" | "writefile" | "create_file"
        | "createfile" | "write_file_chunk" | "writefilechunk" => {
            args.get("content").and_then(|v| v.as_str())
        }
        "replace_file_content"
        | "replacefilecontent"
        | "multi_replace_file_content"
        | "multireplacefilecontent"
        | "edit_file"
        | "editfile"
        | "patch_file"
        | "patchfile" => args
            .get("replacement_content")
            .or_else(|| args.get("ReplacementContent"))
            .or_else(|| args.get("new_string"))
            .or_else(|| args.get("newString"))
            .or_else(|| args.get("content"))
            .and_then(|v| v.as_str()),
        _ => None,
    }
}

/// Synthesize a compact added-lines preview for write/edit calls whose result
/// carries no embedded diff (e.g. `write_to_file` reports only `wrote 'path'
/// (N lines, M bytes)`).
///
/// Returns an empty vec when there is nothing meaningful to show: failures,
/// no-ops, results that already embed a diff, or calls without content args.
/// The caller keeps the truthful summary line in those cases.
pub(super) fn synthesized_edit_preview<'a>(
    tool_name: &str,
    args: &serde_json::Value,
    result: &str,
    success: bool,
    width: usize,
    show_picker: bool,
) -> Vec<Line<'a>> {
    if !success || edit_result_is_noop(result) || result_has_embedded_diff(result) {
        return Vec::new();
    }
    let Some(content) = edit_args_content(tool_name, args) else {
        return Vec::new();
    };
    if content.trim().is_empty() {
        return Vec::new();
    }
    let _path = edit_args_path(args);
    // Render the new content as added diff lines so the transcript keeps the
    // same syntax/diff cues as real edit diffs. Line numbers and wrapping are
    // handled by the unified-diff renderer; the transcript layer truncates the
    // preview and expands the full body on Ctrl+O.
    let diff_text = content
        .lines()
        .map(|line| format!("+{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    if diff_text.trim().is_empty() {
        return Vec::new();
    }
    render_unified_diff(&diff_text, width, show_picker)
}

fn render_directory_result<'a>(result: &str, show_picker: bool) -> Vec<Line<'a>> {
    result
        .lines()
        .map(|raw| {
            let (marker, color) = if raw.ends_with('/') {
                ("▸ ", super::COLOR_PRIMARY())
            } else if raw.contains(" file(s) matched") || raw.starts_with("no files") {
                ("", super::COLOR_MUTED())
            } else {
                ("· ", super::COLOR_MUTED())
            };
            Line::from(vec![
                Span::styled(
                    marker,
                    get_themed_style(color, COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    raw.to_string(),
                    get_themed_style(color, COLOR_BG(), Modifier::empty(), show_picker),
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
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, show_picker),
            )));
            continue;
        }
        blank_run = 0;
        let is_error = raw.trim_start().to_ascii_lowercase().starts_with("error")
            || raw.trim_start().starts_with('✗');
        let (color, modifier) = if is_error {
            (Color::Rgb(229, 123, 123), Modifier::empty())
        } else {
            (COLOR_MUTED(), Modifier::DIM)
        };
        lines.push(Line::from(Span::styled(
            format!("  │ {raw}"),
            get_themed_style(color, COLOR_BG(), modifier, show_picker),
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
                COLOR_BG(),
                Modifier::BOLD,
                show_picker,
            ),
        )));
    }

    for (kind, raw) in output {
        let (prefix, color, modifier) = if kind == "stderr" {
            ("  ! ", Color::Rgb(229, 192, 123), Modifier::empty())
        } else {
            ("  │ ", COLOR_MUTED(), Modifier::DIM)
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{raw}"),
            get_themed_style(color, COLOR_BG(), modifier, show_picker),
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
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
            )));
            continue;
        }
        let Some((number, code)) = raw.split_once(": ") else {
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            )));
            continue;
        };
        let Ok(number) = number.parse::<usize>() else {
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
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
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
                ))
            } else if let Some((number, text)) = raw.trim_start().split_once(": ") {
                let mut row = vec![line_number(number, 0, show_picker)];
                row.extend(highlight_code_line(text, language, show_picker));
                Line::from(row)
            } else {
                Line::from(Span::styled(
                    raw.to_string(),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        COLOR_MUTED, TOOL_RESULT_TRANSCRIPT_MAX_LINES, render_file_preview, render_tool_result,
    };
    use crate::ui::tests::THEME_TEST_LOCK;
    use ratatui::style::Color;

    fn text_of(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn read_results_have_header_and_line_numbered_code() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "view_file",
            "[File: src/main.rs, Lines 4 to 5 of 5]\n4: fn main() {}",
            80,
            &rustcode::controller::Verbosity::Low,
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
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "grep",
            "src/main.rs:\n  12: fn main() {}",
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );
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
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "list_directory",
            "src/\nmain.rs",
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );
        assert!(lines[0].spans[0].content.contains('▸'));
        assert!(lines[1].spans[0].content.contains('·'));
    }

    #[test]
    fn command_results_have_compact_status_and_output() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "run_command",
            "exit code: 0\nstdout:\ncargo test\nstderr:\n",
            80,
            &rustcode::controller::Verbosity::Low,
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
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "run_command",
            "exit code: 1\nstderr:\npermission denied",
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );
        assert!(lines[0].spans[0].content.contains("✗ exit 1"));
        assert!(lines[1].spans[0].content.contains("! permission denied"));
    }

    #[test]
    fn edit_results_show_only_a_compact_success_summary() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "replace_file_content",
            "successfully replaced target_content in 'src/main.rs'",
            80,
            &rustcode::controller::Verbosity::Low,
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
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "replace_file_content",
            "successfully replaced target_content in 'src/main.rs'\n\n```diff\n@@\n-old\n+new\n```",
            80,
            &rustcode::controller::Verbosity::Low,
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
    fn write_preview_synthesizes_added_lines_from_content_args() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        use serde_json::json;

        let args = json!({"path": "src/new.rs", "content": "pub fn new() {}\n"});
        let lines = super::synthesized_edit_preview(
            "write_to_file",
            &args,
            "wrote 'src/new.rs' (1 lines, 15 bytes)",
            true,
            80,
            false,
        );
        assert!(!lines.is_empty());
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("pub fn new"), "{text:?}");
    }

    #[test]
    fn write_preview_stays_empty_for_noop_failure_or_embedded_diff() {
        use serde_json::json;

        let args = json!({"path": "src/main.rs", "content": "hi"});
        assert!(
            super::synthesized_edit_preview(
                "write_to_file",
                &args,
                "already applied; no changes made to 'src/main.rs'",
                true,
                80,
                false,
            )
            .is_empty()
        );
        assert!(
            super::synthesized_edit_preview(
                "write_to_file",
                &args,
                "wrote 'src/main.rs'",
                false,
                80,
                false,
            )
            .is_empty()
        );
        assert!(
            super::synthesized_edit_preview(
                "write_to_file",
                &args,
                "ok\n\n```diff\n+hi\n```",
                true,
                80,
                false,
            )
            .is_empty()
        );
        assert!(super::edit_result_is_noop(
            "Already Applied; NO CHANGES made"
        ));
        assert!(super::result_has_embedded_diff("```diff\n+hi\n```"));
    }

    #[test]
    fn control_plane_results_are_hidden() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        assert!(
            render_tool_result(
                "use_skill",
                "loaded skill",
                80,
                &rustcode::controller::Verbosity::Low,
                false
            )
            .is_empty()
        );
        assert!(
            render_tool_result(
                "spawn_agent",
                "agent done",
                80,
                &rustcode::controller::Verbosity::Low,
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn tool_output_uses_darker_muted_color() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "run_command",
            "exit code: 0\nstdout:\nhello world",
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );
        assert!(!lines.is_empty());
        let last = lines.last().unwrap();
        assert_eq!(last.spans[0].style.fg, Some(COLOR_MUTED()));
    }

    #[test]
    fn generic_results_are_muted_and_keep_errors_visible() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

        let lines = render_tool_result(
            "mcp_custom_tool",
            "completed\nerror: remote service failed",
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[0].spans[0].content.starts_with("  │ completed"));
        assert_eq!(lines[0].spans[0].style.fg, Some(COLOR_MUTED()));
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
    fn large_results_are_capped_for_transcript_rendering() {
        let result = (0..350)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_tool_result(
            "mcp_custom_tool",
            &result,
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );

        // #1593: the transcript no longer promises every stored line. A body
        // that outgrows the cap keeps its head and its tail, names the omitted
        // count between them, and points at the session log for the rest.
        assert_eq!(lines.len(), TOOL_RESULT_TRANSCRIPT_MAX_LINES + 1);
        assert!(text_of(&lines[0]).contains("line 0"));
        let head = TOOL_RESULT_TRANSCRIPT_MAX_LINES - TOOL_RESULT_TRANSCRIPT_MAX_LINES / 4;
        let marker = text_of(&lines[head]);
        assert!(marker.contains("more lines"), "{marker}");
        assert!(marker.contains("session log"), "{marker}");
        // The newest rows are the ones a reader opened the result for, so the
        // cap drops the middle rather than the end.
        let last = text_of(lines.last().expect("a body is never empty"));
        assert!(last.contains("line 349"), "{last}");
        assert!(
            !lines[head + 1..lines.len() - TOOL_RESULT_TRANSCRIPT_MAX_LINES / 4]
                .iter()
                .any(|line| text_of(line).contains("line 200"))
        );
    }

    #[test]
    fn command_and_generic_results_share_the_same_cap() {
        let result = (0..350)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let command = render_tool_result(
            "run_command",
            &result,
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );
        let generic = render_tool_result(
            "mcp_custom_tool",
            &result,
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );

        assert_eq!(command.len(), generic.len());
        assert!(command.len() <= TOOL_RESULT_TRANSCRIPT_MAX_LINES + 1);
        assert!(
            command
                .iter()
                .any(|line| text_of(line).contains("more lines"))
        );
        assert!(
            generic
                .iter()
                .any(|line| text_of(line).contains("more lines"))
        );
    }

    #[test]
    fn long_command_results_are_capped_like_every_other_tool() {
        let result = (0..350)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_tool_result(
            "run_command",
            &result,
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );

        assert_eq!(lines.len(), TOOL_RESULT_TRANSCRIPT_MAX_LINES + 1);
        assert!(
            lines
                .iter()
                .any(|line| text_of(line).contains("more lines"))
        );
    }

    #[test]
    fn failed_commands_keep_the_status_line_and_the_output_tail() {
        let body = (0..200)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = format!("exit code: 101\nstderr:\n{body}\nerror: build failed");
        let lines = render_tool_result(
            "run_command",
            &result,
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );

        assert!(text_of(&lines[0]).contains("✗ exit 101"));
        assert!(lines.iter().any(|line| text_of(line).contains("line 0")));
        assert!(lines.iter().any(|line| text_of(line).contains("line 199")));
        assert!(text_of(lines.last().unwrap()).contains("error: build failed"));
    }

    #[test]
    fn generic_results_preserve_interior_blank_lines() {
        let lines = render_tool_result(
            "mcp_custom_tool",
            "first\n\nsecond",
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );

        assert_eq!(lines.len(), 3);
        assert!(text_of(&lines[0]).contains("first"));
        assert_eq!(text_of(&lines[1]).trim_end(), "  │");
        assert!(text_of(&lines[2]).contains("second"));
    }

    #[test]
    fn generic_results_collapse_blank_runs_and_trim_edges() {
        let lines = render_tool_result(
            "mcp_custom_tool",
            "\n\nfirst\n\n\n\nsecond\n\n",
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );

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
            &rustcode::controller::Verbosity::Low,
            false,
        );
        let header = text_of(&lines[0]);

        assert!(!header.contains("Bytes offset"));
        assert_eq!(header, "src/main.rs · Lines 1 to 2 of 9");
    }

    #[test]
    fn embedded_diffs_survive_whole_inside_the_transcript_cap() {
        let diff = (0..8)
            .map(|index| format!("-removed line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = format!("successfully edited file\n\n```diff\n{diff}\n```");
        let lines = render_tool_result(
            "replace_file_content",
            &result,
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );

        assert!(lines.len() > 5);
        assert!(
            !lines
                .iter()
                .any(|line| text_of(line).contains("more lines"))
        );

        // The old assertion was that a diff is *never* truncated. The contract
        // is now bounded rather than absolute: a diff that fits the cap stays
        // whole, and one that does not is cut with a marker (#1593).
        let oversized = (0..TOOL_RESULT_TRANSCRIPT_MAX_LINES + 40)
            .map(|index| format!("-removed line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let capped = render_tool_result(
            "replace_file_content",
            &format!("successfully edited file\n\n```diff\n{oversized}\n```"),
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );
        assert_eq!(capped.len(), TOOL_RESULT_TRANSCRIPT_MAX_LINES + 1);
        assert!(
            capped
                .iter()
                .any(|line| text_of(line).contains("more lines"))
        );
    }

    #[test]
    fn manage_task_renders_under_low_verbosity() {
        let result =
            "TaskId: task-1, Status: RUNNING, PID: 1234, Runtime: 5s, Command: cargo check";
        let lines = render_tool_result(
            "manage_task",
            result,
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        );
        assert!(!lines.is_empty());
    }

    #[test]
    fn high_verbosity_hides_tool_output() {
        let result =
            "TaskId: task-1, Status: RUNNING, PID: 1234, Runtime: 5s, Command: cargo check";
        assert!(
            render_tool_result(
                "manage_task",
                result,
                80,
                &rustcode::controller::Verbosity::High,
                false
            )
            .is_empty()
        );
        assert!(
            render_tool_result(
                "replace_file_content",
                "edited file",
                80,
                &rustcode::controller::Verbosity::High,
                false
            )
            .is_empty()
        );
        assert!(
            render_tool_result(
                "run_command",
                "command output",
                80,
                &rustcode::controller::Verbosity::High,
                false
            )
            .is_empty()
        );
    }
}
