//! Structured rendering for native tool results.

use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use std::path::Path;

use super::{
    COLOR_BG, COLOR_ELEMENT, COLOR_MUTED, COLOR_TEXT, get_themed_style, highlight_code_line,
};

fn language_for_path(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("text")
}

fn line_number<'a>(old: &str, _width: usize, show_picker: bool) -> Span<'a> {
    Span::styled(
        format!("{old:>5} "),
        get_themed_style(COLOR_MUTED, COLOR_ELEMENT, Modifier::empty(), show_picker),
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
    match tool_name {
        "view_file" => render_read_result(result, width, show_picker),
        "grep" => render_search_result(result, width, show_picker),
        _ => result
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_string(),
                    get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::empty(), show_picker),
                ))
            })
            .collect(),
    }
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
                raw.to_string(),
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
    result
        .lines()
        .map(|raw| {
            let is_path = raw.ends_with(':') && !raw.starts_with("  ");
            if is_path {
                Line::from(Span::styled(
                    raw.to_string(),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
                ))
            } else if let Some((number, text)) = raw.trim_start().split_once(": ") {
                let mut row = vec![line_number(number, 0, show_picker)];
                row.push(Span::styled(
                    text.to_string(),
                    get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::empty(), show_picker),
                ));
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
    use super::render_tool_result;

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
        assert!(text.starts_with("    4 "));
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
}
