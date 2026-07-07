//! UI layer: terminal layout, rendering widgets, custom prompt prefix, footer.

use crate::app::{AppState, AppStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

/// Pad `s` with trailing spaces up to `width` terminal columns (unicode-aware).
fn pad_to_width(s: &str, width: usize) -> String {
    let current = s.width();
    if current < width {
        format!("{}{}", s, " ".repeat(width - current))
    } else {
        s.to_string()
    }
}

// Cozy Rain Theme Colors
const COLOR_BG: Color = Color::Rgb(21, 23, 26); // #15171A (smoky base background)
const COLOR_PANEL: Color = Color::Rgb(26, 29, 32); // #1A1D20 (translucent/charcoal panel cards)
const COLOR_ELEMENT: Color = Color::Rgb(34, 38, 42); // #22262A (element background / code block panel)
const COLOR_TEXT: Color = Color::Rgb(240, 229, 222); // #F0E5DE (soft cream main text)
const COLOR_MUTED: Color = Color::Rgb(136, 146, 154); // #88929A (muted gray text)
const COLOR_PRIMARY: Color = Color::Rgb(236, 110, 93); // #EC6E5D (vibrant coral active accent)
const COLOR_SECONDARY: Color = Color::Rgb(60, 88, 101); // #3C5865 (deep slate blue)
const COLOR_GREEN: Color = Color::Rgb(127, 216, 143); // #7fd88f (green syntax/success)
const COLOR_BORDER: Color = Color::Rgb(72, 85, 89); // #485559 (border)
const COLOR_TIP: Color = Color::Rgb(224, 169, 109); // #E0A96D (warning/gold Tip dot)

// ASCII Welcome Logo
const LOGO: &[&str] = &[
    "                  ▄                   █      ",
    "▄▀▀▀ █   █ ▄▀▀▀▀ ▀█▀▀ ▄▀▀▀▀ ▄▀▀▀▄ ▄▀▀▀█ ▄▀▀▀▄",
    "█    █   █  ▀▀▀▄  █   █     █   █ █   █ █▀▀▀▀",
    "▀     ▀▀▀  ▀▀▀▀    ▀▀  ▀▀▀▀  ▀▀▀   ▀▀▀▀  ▀▀▀▀",
];

// Command Information for Autocomplete Menu
pub use crate::app::suggestion::{COMMANDS, CommandInfo};

/// Helper to dim colors if the model picker modal is active.
fn get_themed_style(fg: Color, bg: Color, modifier: Modifier, show_picker: bool) -> Style {
    if show_picker {
        Style::default()
            .fg(Color::Rgb(60, 68, 72)) // dimmed gray-blue
            .bg(COLOR_BG)
    } else {
        Style::default().fg(fg).bg(bg).add_modifier(modifier)
    }
}

/// Helper to render markdown and style code blocks for assistant/AI messages.
fn render_assistant_message<'a>(
    content: &str,
    response_time_ms: Option<u64>,
    model_name: &str,
    lines: &mut Vec<Line<'a>>,
    is_generating: bool,
    viewport_width: u16,
    show_picker: bool,
) {
    // Check for think tags
    let mut think_content = None;
    let mut main_content = content;

    if content.contains("<think>") {
        if let Some(end_idx) = content.find("</think>") {
            let start_idx = content.find("<think>").unwrap();
            let think_part = &content[start_idx + 7..end_idx];
            let main_part = &content[end_idx + 8..];
            think_content = Some(think_part.trim());
            main_content = main_part.trim();
        } else {
            let start_idx = content.find("<think>").unwrap();
            let think_part = &content[start_idx + 7..];
            think_content = Some(think_part.trim());
            main_content = "";
        }
    }

    // Render Think Block if present
    if let Some(think) = think_content {
        let thought_header = if let Some(ms) = response_time_ms {
            format!("Thought: {}ms", ms)
        } else {
            "Thought:".to_string()
        };
        lines.push(Line::from(Span::styled(
            thought_header,
            get_themed_style(
                Color::Rgb(229, 192, 123),
                COLOR_BG,
                Modifier::empty(),
                show_picker,
            ),
        )));

        for raw_line in think.lines() {
            lines.push(Line::from(Span::styled(
                raw_line.to_string(),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            )));
        }
        lines.push(Line::from("")); // spacer
    }

    // Render Main Content with markdown
    if !main_content.is_empty() {
        let mut in_code_block = false;
        let display_lines: Vec<&str> = main_content.lines().collect();
        for raw_line in &display_lines {
            let is_code_fence = raw_line.trim_start().starts_with("```");
            let mut spans = Vec::new();

            if is_code_fence {
                let content_width = (viewport_width as usize).saturating_sub(6);
                spans.push(Span::styled(
                    pad_to_width(raw_line, content_width),
                    get_themed_style(COLOR_MUTED, COLOR_ELEMENT, Modifier::empty(), show_picker),
                ));
                in_code_block = !in_code_block;
            } else if in_code_block {
                let content_width = (viewport_width as usize).saturating_sub(6);
                spans.push(Span::styled(
                    pad_to_width(raw_line, content_width),
                    get_themed_style(COLOR_GREEN, COLOR_ELEMENT, Modifier::empty(), show_picker),
                ));
            } else {
                let mut chars = raw_line.chars().peekable();
                let mut current = String::new();
                let mut in_inline_code = false;
                let mut in_bold = false;

                while let Some(c) = chars.next() {
                    if c == '`' {
                        if !current.is_empty() {
                            let modifier = if in_bold {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            };
                            let style = if in_inline_code {
                                get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                            } else {
                                get_themed_style(COLOR_TEXT, COLOR_BG, modifier, show_picker)
                            };
                            spans.push(Span::styled(current.clone(), style));
                            current.clear();
                        }
                        in_inline_code = !in_inline_code;
                    } else if c == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        if !current.is_empty() {
                            let modifier = if in_bold {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            };
                            let style = if in_inline_code {
                                get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                            } else {
                                get_themed_style(COLOR_TEXT, COLOR_BG, modifier, show_picker)
                            };
                            spans.push(Span::styled(current.clone(), style));
                            current.clear();
                        }
                        in_bold = !in_bold;
                    } else {
                        current.push(c);
                    }
                }

                if !current.is_empty() {
                    let modifier = if in_bold {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    };
                    let style = if in_inline_code {
                        get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                    } else {
                        get_themed_style(COLOR_TEXT, COLOR_BG, modifier, show_picker)
                    };
                    spans.push(Span::styled(current, style));
                }
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from("")); // spacer
    }

    // Render Status Line (unless it's streaming, which is handled dynamically)
    if !is_generating {
        let mut status_spans = vec![
            Span::styled(
                "■ ",
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ),
            Span::styled(
                "Build",
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
            ),
            Span::styled(
                " · ",
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ),
            Span::styled(
                model_name.to_string(),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ),
        ];

        if let Some(ms) = response_time_ms {
            let secs = ms as f32 / 1000.0;
            status_spans.push(Span::styled(
                format!(" · {:.1}s", secs),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ));
        }

        lines.push(Line::from(status_spans));
    }
}

/// Helper function to count wrapped lines in the input buffer.
fn count_input_lines(input_buffer: &str, inner_width: usize) -> u16 {
    if inner_width == 0 {
        return 1;
    }
    let mut lines_count = 1;
    let mut col = 0;

    for c in input_buffer.chars() {
        if c == '\n' {
            lines_count += 1;
            col = 0;
        } else {
            col += 1;
            if col == inner_width {
                lines_count += 1;
                col = 0;
            }
        }
    }
    lines_count
}

fn render_footer(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) {
    let footer_area = chunks[3];
    let show_picker = state.show_model_picker || state.show_command_picker;

    let left_spans = if state.status == AppStatus::Streaming || state.status == AppStatus::Queued {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let step = ((millis / 80) % 10) as usize;
        let pulse_center = if step < 5 { step } else { 9 - step };

        let colors = [
            Color::Rgb(25, 29, 32),
            Color::Rgb(34, 40, 45),
            Color::Rgb(43, 51, 57),
            Color::Rgb(52, 62, 70),
            Color::Rgb(60, 88, 101),
            Color::Rgb(120, 160, 180),
        ];

        let mut spans = Vec::new();

        for i in 0..6 {
            let dist = (i as isize - pulse_center as isize).abs() as usize;
            let level = if dist >= 5 { 0 } else { 5 - dist };
            let color = colors[level];
            spans.push(Span::styled(
                "■",
                get_themed_style(color, COLOR_BG, Modifier::empty(), show_picker),
            ));
        }

        if !state.pending_queue.is_empty() {
            spans.push(Span::styled(
                format!("  queued: {}", state.pending_queue.len()),
                get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::BOLD, show_picker),
            ));
        }

        spans.push(Span::styled(
            "   ..... esc ",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ));
        spans.push(Span::styled(
            "interrupt",
            get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
        ));
        spans
    } else {
        let static_color = Color::Rgb(40, 48, 54);
        let mut spans = Vec::new();

        for _ in 0..6 {
            spans.push(Span::styled(
                "■",
                get_themed_style(static_color, COLOR_BG, Modifier::empty(), show_picker),
            ));
        }

        spans.push(Span::styled(
            "   idle",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ));
        spans
    };

    let right_spans = if state.history.is_empty() {
        vec![
            Span::styled(
                "tab",
                get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
            ),
            Span::styled(
                " agents   ",
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ),
            Span::styled(
                "ctrl+p",
                get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
            ),
            Span::styled(
                " commands",
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ),
        ]
    } else {
        let total_tokens = if let Some(usage) = &state.current_token_usage {
            usage.total_tokens
        } else {
            let last_usage = state
                .history
                .iter()
                .rev()
                .find_map(|m| m.token_usage.as_ref());
            if let Some(u) = last_usage {
                u.total_tokens
            } else {
                let chars: usize = state.history.iter().map(|m| m.content.len()).sum();
                (chars / 4) as u32
            }
        };

        let token_str = if total_tokens >= 1000 {
            format!("{:.1}K", total_tokens as f32 / 1000.0)
        } else {
            format!("{}", total_tokens)
        };

        vec![
            Span::styled(
                "context used: ",
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ),
            Span::styled(
                token_str,
                get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::BOLD, show_picker),
            ),
            Span::styled("   ", Style::default()),
            Span::styled(
                "ctrl+p",
                get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
            ),
            Span::styled(
                " commands",
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ),
        ]
    };

    let footer_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(footer_area);

    f.render_widget(
        Paragraph::new(Line::from(left_spans)).style(Style::default().bg(COLOR_BG)),
        footer_chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(right_spans))
            .alignment(ratatui::layout::Alignment::Right)
            .style(Style::default().bg(COLOR_BG)),
        footer_chunks[1],
    );
}

/// Render the input area (block + cursor + optional completion suffix).
fn render_input(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) -> Margin {
    let show_picker = state.show_model_picker || state.show_command_picker;

    // Split input block horizontally: left vertical block line (▌) and solid content box
    let input_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1), // Thick vertical block line ▌
            Constraint::Min(0),    // Solid content box
        ])
        .split(chunks[1]);

    let line_chars = "▌\n".repeat(chunks[1].height as usize);
    let vertical_line_widget = Paragraph::new(line_chars).style(get_themed_style(
        COLOR_SECONDARY,
        COLOR_BG,
        Modifier::empty(),
        show_picker,
    ));
    f.render_widget(vertical_line_widget, input_split[0]);

    let solid_panel = Block::default().style(Style::default().bg(COLOR_PANEL));
    f.render_widget(solid_panel, input_split[1]);

    let input_margin = Margin {
        vertical: 1,
        horizontal: 2,
    };
    let input_inner = input_split[1].inner(input_margin);

    let text_style = if state.input_buffer.starts_with('/') {
        get_themed_style(COLOR_PRIMARY, COLOR_PANEL, Modifier::BOLD, show_picker)
    } else {
        get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker)
    };

    let inner_width = input_inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_dx = 0u16;
    let mut cursor_dy = 0u16;

    if inner_width > 0 {
        // Collect input characters
        let mut styled_chars: Vec<(char, Style)> = state
            .input_buffer
            .chars()
            .map(|c| (c, text_style))
            .collect();

        // Collect suggestion characters (only if suggestion is active)
        if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        // cursor_position is a byte offset; rendering iterates chars.
        let cursor_char_index = state.input_buffer
            [..state.cursor_position.min(state.input_buffer.len())]
            .chars()
            .count();

        // Split styled_chars into lines, respecting '\n' and wrapping at inner_width
        let mut current_line_spans = Vec::new();
        let mut current_run: Option<(Style, String)> = None;

        let mut col = 0;
        let mut row = 0;

        let total_chars = styled_chars.len();
        for (i, &(c, style)) in styled_chars.iter().enumerate() {
            // Track cursor coordinates *before* appending character
            if i == cursor_char_index {
                cursor_dx = col as u16;
                cursor_dy = row as u16;
            }

            if c == '\n' {
                // Flush run
                if let Some((st, s)) = current_run.take() {
                    current_line_spans.push(Span::styled(s, st));
                }
                lines.push(Line::from(current_line_spans.clone()));
                current_line_spans.clear();
                row += 1;
                col = 0;
            } else {
                if col >= inner_width {
                    if let Some((st, s)) = current_run.take() {
                        current_line_spans.push(Span::styled(s, st));
                    }
                    lines.push(Line::from(current_line_spans.clone()));
                    current_line_spans.clear();
                    row += 1;
                    col = 0;
                }

                // Group character runs by styling for optimal spans
                match current_run.as_mut() {
                    Some((st, s)) if *st == style => {
                        s.push(c);
                    }
                    _ => {
                        if let Some((st, s)) = current_run.take() {
                            current_line_spans.push(Span::styled(s, st));
                        }
                        current_run = Some((style, c.to_string()));
                    }
                }
                col += 1;
            }
        }

        // Final cursor placement matching length
        if cursor_char_index == total_chars {
            cursor_dx = col as u16;
            cursor_dy = row as u16;
        }

        // Flush any remaining styling runs
        if let Some((st, s)) = current_run {
            current_line_spans.push(Span::styled(s, st));
        }
        lines.push(Line::from(current_line_spans));
    }

    // Inside the input block, render text box and leave padding spacer at the bottom
    let text_area_height = input_inner.height.saturating_sub(1);
    let text_area = ratatui::layout::Rect::new(
        input_inner.x,
        input_inner.y,
        input_inner.width,
        text_area_height,
    );
    let paragraph = Paragraph::new(lines).style(Style::default().bg(COLOR_PANEL));
    f.render_widget(paragraph, text_area);

    // Render Build info inside input box at bottom (colored with secondary accent color #3C5865)
    let build_y = input_inner.y + input_inner.height.saturating_sub(1);
    let build_area = ratatui::layout::Rect::new(input_inner.x, build_y, input_inner.width, 1);
    let build_line = Line::from(vec![
        Span::styled(
            "Build",
            get_themed_style(COLOR_SECONDARY, COLOR_PANEL, Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " · ",
            get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::empty(), show_picker),
        ),
        Span::styled(
            state.model_name.clone(),
            get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
        ),
        Span::styled(" ", Style::default().bg(COLOR_PANEL)),
        Span::styled(
            state.config.default.clone(),
            get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::empty(), show_picker),
        ),
    ]);
    f.render_widget(Paragraph::new(build_line), build_area);

    // Place cursor relative to mapped cursor_dx and cursor_dy
    if inner_width > 0 && !show_picker {
        f.set_cursor_position((input_inner.x + cursor_dx, input_inner.y + cursor_dy));
    }

    input_margin
}

/// Render the main conversation area with proper line wrapping and auto-scroll.
fn render_conversation(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &mut AppState) {
    let inner_area = chunks[0].inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let show_picker = state.show_model_picker || state.show_command_picker;

    // Build wrapped display lines.
    let mut lines: Vec<Line> = Vec::new();

    for msg in &state.history {
        if msg.role == "system" {
            // Local helper messages (e.g. /help output).
            for raw_line in msg.content.lines() {
                lines.push(Line::from(vec![
                    Span::styled(
                        "│ ",
                        get_themed_style(
                            Color::Rgb(229, 192, 123),
                            COLOR_BG,
                            Modifier::BOLD,
                            show_picker,
                        ),
                    ),
                    Span::styled(
                        raw_line,
                        get_themed_style(
                            Color::Rgb(229, 192, 123),
                            COLOR_BG,
                            Modifier::empty(),
                            show_picker,
                        ),
                    ),
                ]));
            }
            lines.push(Line::from("")); // spacer
        } else if msg.role == "user" {
            lines.push(Line::from("")); // spacer above box
            let content_width = (inner_area.width as usize).saturating_sub(4);
            let mut wrapped_lines = Vec::new();
            for raw_line in msg.content.lines() {
                if raw_line.is_empty() {
                    wrapped_lines.push("".to_string());
                } else {
                    let mut current = String::new();
                    for word in raw_line.split_whitespace() {
                        if current.is_empty() {
                            current.push_str(word);
                        } else if current.width() + 1 + word.width() <= content_width {
                            current.push(' ');
                            current.push_str(word);
                        } else {
                            wrapped_lines.push(current);
                            current = word.to_string();
                        }
                    }
                    if !current.is_empty() {
                        wrapped_lines.push(current);
                    }
                }
            }

            for line_str in wrapped_lines {
                let line_str = pad_to_width(&line_str, content_width);
                lines.push(Line::from(vec![
                    Span::styled(
                        "▌ ",
                        get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::empty(), show_picker),
                    ),
                    Span::styled(
                        line_str,
                        get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                    ),
                ]));
            }
            lines.push(Line::from("")); // spacer below box
        } else if msg.role == "assistant" {
            render_assistant_message(
                &msg.content,
                msg.response_time_ms,
                &state.model_name,
                &mut lines,
                false,
                inner_area.width,
                show_picker,
            );
            lines.push(Line::from("")); // spacer
        }
    }

    if state.status == AppStatus::Streaming || state.status == AppStatus::Queued {
        if state.current_response.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(
                    "■ ",
                    get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::empty(), show_picker),
                ),
                Span::styled(
                    "Build",
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    " · ",
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ),
                Span::styled(
                    state.model_name.clone(),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ),
            ]));
        } else {
            render_assistant_message(
                &state.current_response,
                None,
                &state.model_name,
                &mut lines,
                true,
                inner_area.width,
                show_picker,
            );

            lines.push(Line::from(vec![
                Span::styled(
                    "■ ",
                    get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::empty(), show_picker),
                ),
                Span::styled(
                    "Build",
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    " · ",
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ),
                Span::styled(
                    state.model_name.clone(),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ),
            ]));
        }
    }

    // Calculate auto-scroll offset based on wrapped line heights.
    let mut total_wrapped_lines = 0u16;
    for line in &lines {
        match line.width() {
            0 => total_wrapped_lines += 1,
            w => total_wrapped_lines += (w as u16).div_ceil(inner_area.width),
        }
    }
    let max_scroll = total_wrapped_lines.saturating_sub(inner_area.height);
    state.last_max_scroll = max_scroll;

    let scroll_offset = if state.is_scroll_locked_to_bottom {
        state.scroll_row = max_scroll;
        max_scroll
    } else {
        if state.scroll_row > max_scroll {
            state.scroll_row = max_scroll;
            max_scroll
        } else {
            state.scroll_row
        }
    };

    let conversation_paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .scroll((scroll_offset, 0))
        .style(Style::default().bg(COLOR_BG));

    f.render_widget(conversation_paragraph, inner_area);
}

/// Render the autocomplete suggestion menu popup overlay.
fn render_popup_menu(
    f: &mut Frame,
    state: &AppState,
    filtered_cmds: &[&CommandInfo],
    area: ratatui::layout::Rect,
) {
    let mut popup_lines = Vec::new();
    for (idx, cmd) in filtered_cmds.iter().enumerate() {
        let is_selected = state
            .active_suggestion_index
            .map(|i| i == idx)
            .unwrap_or(false);

        let line = if is_selected {
            let left_text = format!("{:<12}   {}", cmd.name, cmd.desc);
            let total_len = left_text.len();
            let padding_len = (area.width as usize).saturating_sub(total_len);
            let full_text = format!("{}{}", left_text, " ".repeat(padding_len));

            Line::from(Span::styled(
                full_text,
                Style::default()
                    .fg(COLOR_BG)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            let left_text = format!("{:<12}   ", cmd.name);
            let desc_text = cmd.desc.to_string();
            let total_len = left_text.len() + desc_text.len();
            let padding_len = (area.width as usize).saturating_sub(total_len);

            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT).bg(COLOR_PANEL)),
                Span::styled(desc_text, Style::default().fg(COLOR_MUTED).bg(COLOR_PANEL)),
                Span::styled(" ".repeat(padding_len), Style::default().bg(COLOR_PANEL)),
            ])
        };
        popup_lines.push(line);
    }
    f.render_widget(
        Paragraph::new(popup_lines).style(Style::default().bg(COLOR_PANEL)),
        area,
    );
}

/// Render the welcome splash screen centered vertically and horizontally.
fn render_welcome_screen(
    f: &mut Frame,
    state: &AppState,
) -> (ratatui::layout::Rect, ratatui::layout::Rect) {
    let width = f.area().width;
    let height = f.area().height;

    // Total vertical lines: Logo (4) + Spacer (3) + Prompt Box (5) + Spacer (1) + Hints (1) + Spacer (2) + Tip (1) = 17 lines
    let logo_start_y = height.saturating_sub(17) / 2;

    let show_picker = state.show_model_picker || state.show_command_picker;

    let welcome_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(logo_start_y),
            Constraint::Length(4), // Logo
            Constraint::Length(3), // Spacer
            Constraint::Length(5), // Prompt Box (5 lines high)
            Constraint::Length(2), // Spacer between box and hints
            Constraint::Length(1), // Hints (tab agents...)
            Constraint::Length(2), // Spacer
            Constraint::Length(1), // Tip
            Constraint::Min(0),    // Spacer to bottom
        ])
        .split(f.area());

    // 1. Logo: Color Split ("rust" in COLOR_SECONDARY, "code" in COLOR_TEXT)
    let logo_area = welcome_chunks[1];
    let padding_left = (logo_area.width.saturating_sub(45) / 2) as usize;
    let mut logo_lines = Vec::new();

    for line in LOGO {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() >= 22 {
            let part1: String = chars[0..22].iter().collect();
            let part2: String = chars[22..].iter().collect();

            logo_lines.push(Line::from(vec![
                Span::styled(
                    format!("{}{}", " ".repeat(padding_left), part1),
                    get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    part2,
                    get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
                ),
            ]));
        } else {
            logo_lines.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(padding_left), line),
                get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
            )));
        }
    }
    f.render_widget(
        Paragraph::new(logo_lines).style(Style::default().bg(COLOR_BG)),
        logo_area,
    );

    // 2. Prompt Box Layout (80 columns or full screen minus margins)
    let box_width = 80u16.min(width.saturating_sub(6));
    let box_padding = (width.saturating_sub(box_width) / 2) as u16;
    let box_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(box_padding),
            Constraint::Length(box_width),
            Constraint::Min(0),
        ])
        .split(welcome_chunks[3]);

    let prompt_box_area = box_chunks[1];

    // Split prompt box horizontally: left vertical block line (▌) and solid content box
    let prompt_box_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1), // Thick vertical block line ▌
            Constraint::Min(0),    // Solid content box
        ])
        .split(prompt_box_area);

    // Render the vertical thick block bar (▌)
    let line_chars = "▌\n".repeat(prompt_box_area.height as usize);
    let vertical_line_widget = Paragraph::new(line_chars).style(get_themed_style(
        COLOR_SECONDARY,
        COLOR_BG,
        Modifier::empty(),
        show_picker,
    ));
    f.render_widget(vertical_line_widget, prompt_box_split[0]);

    // Render solid content panel with solid COLOR_PANEL background (no border lines)
    let solid_panel = Block::default().style(Style::default().bg(COLOR_PANEL));

    let mut box_lines = Vec::new();

    // Input Line or Placeholder
    if state.input_buffer.is_empty() {
        box_lines.push(Line::from(Span::styled(
            "Ask anything... \"Fix a TODO in the codebase\"",
            get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::empty(), show_picker),
        )));
    } else {
        let text_style = if state.input_buffer.starts_with('/') {
            get_themed_style(COLOR_PRIMARY, COLOR_PANEL, Modifier::BOLD, show_picker)
        } else {
            get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker)
        };
        let mut spans = vec![Span::styled(state.input_buffer.clone(), text_style)];
        if let Some(suffix) = state.get_command_suggestion() {
            spans.push(Span::styled(
                suffix,
                get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::ITALIC, show_picker),
            ));
        }
        box_lines.push(Line::from(spans));
    }

    // Spacer empty line inside welcome input block to push Build down to the bottom
    box_lines.push(Line::from(""));

    // Provider Build Status Line (accented #3C5865 on Build)
    box_lines.push(Line::from(vec![
        Span::styled(
            "Build",
            get_themed_style(COLOR_SECONDARY, COLOR_PANEL, Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " · ",
            get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::empty(), show_picker),
        ),
        Span::styled(
            state.model_name.clone(),
            get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
        ),
        Span::styled(" ", Style::default().bg(COLOR_PANEL)),
        Span::styled(
            state.config.default.clone(),
            get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::empty(), show_picker),
        ),
    ]));

    // Margin inside the prompt block panel (horizontal: 2 is perfect since it is shifted by gap)
    let inner = prompt_box_split[1].inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    f.render_widget(solid_panel, prompt_box_split[1]);
    f.render_widget(
        Paragraph::new(box_lines).style(Style::default().bg(COLOR_PANEL)),
        inner,
    );

    // Position cursor inside prompt box (cursor_position is a byte offset)
    if inner.width > 0 && !show_picker {
        let cursor_col = state.input_buffer[..state.cursor_position.min(state.input_buffer.len())]
            .chars()
            .count() as u16;
        f.set_cursor_position((inner.x + cursor_col, inner.y));
    }

    // 3. Hint Row right below prompt box
    let hint_area = welcome_chunks[4];
    let hint_box_width_area =
        ratatui::layout::Rect::new(prompt_box_area.x, hint_area.y, prompt_box_area.width, 1);
    let hint_text = Paragraph::new(Line::from(vec![
        Span::styled(
            "tab",
            get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " agents   ",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ),
        Span::styled(
            "ctrl+p",
            get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " commands",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ),
    ]))
    .alignment(ratatui::layout::Alignment::Right)
    .style(Style::default().bg(COLOR_BG));
    f.render_widget(hint_text, hint_box_width_area);

    // 4. Centered Tip Row
    let tip_area = welcome_chunks[6];
    let tip_lines = vec![
        Span::styled(
            "● ",
            get_themed_style(COLOR_TIP, COLOR_BG, Modifier::empty(), show_picker),
        ),
        Span::styled(
            "Tip ",
            get_themed_style(COLOR_TIP, COLOR_BG, Modifier::BOLD, show_picker),
        ),
        Span::styled(
            "Use ",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ),
        Span::styled(
            "/status",
            get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " or ",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ),
        Span::styled(
            "ctrl+x s",
            get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " to see system status info",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ),
    ];
    let tip_text_len = 54; // approximate length of string characters
    let tip_padding = (width.saturating_sub(tip_text_len) / 2) as usize;
    let mut centered_spans = vec![Span::styled(" ".repeat(tip_padding), Style::default())];
    centered_spans.extend(tip_lines);
    f.render_widget(
        Paragraph::new(Line::from(centered_spans)).style(Style::default().bg(COLOR_BG)),
        tip_area,
    );

    // 5. Welcome Screen Bottom Metadata - shifted 2 lines up from bottom edge for perfect padding
    let bottom_y = height.saturating_sub(2);
    let metadata_area = ratatui::layout::Rect::new(2, bottom_y, width.saturating_sub(4), 1);

    let meta_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(metadata_area);

    let left_meta = Paragraph::new(Span::styled(
        &state.cwd_and_branch,
        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
    ))
    .style(Style::default().bg(COLOR_BG));
    let right_meta = Paragraph::new(Span::styled(
        env!("CARGO_PKG_VERSION"),
        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
    ))
    .alignment(ratatui::layout::Alignment::Right)
    .style(Style::default().bg(COLOR_BG));

    f.render_widget(left_meta, meta_chunks[0]);
    f.render_widget(right_meta, meta_chunks[1]);

    (prompt_box_area, prompt_box_split[1])
}

/// Helper function to create centered Rect for modals.
fn centered_rect_fixed(width: u16, height: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let x = r.x + r.width.saturating_sub(width) / 2;
    let y = r.y + r.height.saturating_sub(height) / 2;
    ratatui::layout::Rect::new(x, y, width.min(r.width), height.min(r.height))
}
/// One row in the model picker: a model profile from the config.
#[derive(Clone)]
pub struct PickerItem {
    pub group: String,
    pub name: String,
    pub desc: String,
}

fn picker_group_for_url(url: &str) -> &'static str {
    if url.contains(":11434") {
        "ollama"
    } else if url.contains(":1976") {
        "Apple Foundation Models"
    } else {
        "custom providers"
    }
}

/// Model picker rows for the current config profiles, filtered by the
/// active search string. Shared by rendering (ui) and selection (main).
pub fn get_filtered_picker_items(state: &AppState) -> Vec<PickerItem> {
    let search = state.model_picker_search.to_lowercase();
    state
        .config
        .models
        .iter()
        .map(|p| PickerItem {
            group: picker_group_for_url(&p.url).to_string(),
            name: p.name.clone(),
            desc: p.model.clone(),
        })
        .filter(|item| {
            item.name.to_lowercase().contains(&search)
                || item.group.to_lowercase().contains(&search)
                || item.desc.to_lowercase().contains(&search)
        })
        .collect()
}

/// Render the model picker modal overlay.
fn render_model_picker_modal(f: &mut Frame, state: &AppState) {
    let filtered_items = get_filtered_picker_items(state);

    let selected_idx = state
        .model_picker_index
        .min(filtered_items.len().saturating_sub(1));

    // Fixed modal box in center of terminal
    let modal_area = centered_rect_fixed(65, 18, f.area());

    // Clear the background to prevent text bleed-through
    f.render_widget(Clear, modal_area);

    // Borderless solid background panel
    let modal_block = Block::default().style(Style::default().bg(COLOR_PANEL));

    f.render_widget(modal_block, modal_area);

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
    });

    // Layout constraints inside modal: Header (1), Search (1), List (Min), Footer (1)
    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Search
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    // 1. Modal Header
    let header_line = Line::from(vec![
        Span::styled(
            "Select model",
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ".repeat(inner_area.width.saturating_sub(15) as usize),
            Style::default(),
        ),
        Span::styled("esc", Style::default().fg(COLOR_MUTED)),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[0],
    );

    // 2. Search Box with cursor (flashing peach block)
    let search_line = if state.model_picker_search.is_empty() {
        Line::from(vec![
            Span::styled("█", Style::default().fg(COLOR_PRIMARY)),
            Span::styled("Search", Style::default().fg(COLOR_MUTED)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                state.model_picker_search.clone(),
                Style::default().fg(COLOR_TEXT),
            ),
            Span::styled("█", Style::default().fg(COLOR_PRIMARY)),
        ])
    };
    f.render_widget(
        Paragraph::new(search_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[2],
    );

    // 3. Models List
    let mut list_lines = Vec::new();
    let mut current_group = String::new();

    for (idx, item) in filtered_items.iter().enumerate() {
        if item.group != current_group {
            current_group = item.group.clone();
            list_lines.push(Line::from("")); // spacer
            list_lines.push(Line::from(Span::styled(
                current_group.clone(),
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let is_selected = selected_idx == idx;
        let line = if is_selected {
            // Selected row: solid Peach background block
            let left_text = format!(" ● {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.len() + item.desc.len());
            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default()
                        .fg(COLOR_BG)
                        .bg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ".repeat(padding_len),
                    Style::default().fg(COLOR_BG).bg(COLOR_PRIMARY),
                ),
                Span::styled(
                    item.desc.clone(),
                    Style::default().fg(COLOR_BG).bg(COLOR_PRIMARY),
                ),
            ])
        } else {
            let left_text = format!("   {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.len() + item.desc.len());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT)),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(item.desc.clone(), Style::default().fg(COLOR_MUTED)),
            ])
        };
        list_lines.push(line);
    }

    // Scrollable widget viewport viewport
    let scroll_y = selected_idx.saturating_sub(3) as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL));
    f.render_widget(list_paragraph, modal_chunks[4]);

    // 4. Modal Footer
    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT)),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED)),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT)),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED)),
        Span::styled("search ", Style::default().fg(COLOR_TEXT)),
        Span::styled("type", Style::default().fg(COLOR_MUTED)),
    ]);
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[5],
    );
}

#[derive(Clone)]
pub struct PaletteItem {
    pub group: &'static str,
    pub name: &'static str,
    pub shortcut: &'static str,
}

/// Every palette item maps to a real, implemented action (the shortcut
/// column shows the equivalent slash command).
pub const PALETTE_ITEMS: &[PaletteItem] = &[
    // Session Group
    PaletteItem {
        group: "Session",
        name: "New session",
        shortcut: "/new",
    },
    PaletteItem {
        group: "Session",
        name: "Resume session",
        shortcut: "/resume",
    },
    PaletteItem {
        group: "Session",
        name: "Copy last reply",
        shortcut: "/copy",
    },
    // Agent Group
    PaletteItem {
        group: "Agent",
        name: "Switch model",
        shortcut: "/model",
    },
    // System Group
    PaletteItem {
        group: "System",
        name: "Help",
        shortcut: "/help",
    },
    PaletteItem {
        group: "System",
        name: "Exit the app",
        shortcut: "ctrl+c",
    },
];

fn render_command_picker_modal(f: &mut Frame, state: &AppState) {
    let search = state.command_picker_search.to_lowercase();
    let filtered_items: Vec<&PaletteItem> = PALETTE_ITEMS
        .iter()
        .filter(|item| {
            item.name.to_lowercase().contains(&search)
                || item.group.to_lowercase().contains(&search)
        })
        .collect();

    let selected_idx = state
        .command_picker_index
        .min(filtered_items.len().saturating_sub(1));

    // Fixed modal box in center of terminal
    let modal_area = centered_rect_fixed(65, 20, f.area());

    // Clear the background to prevent text bleed-through
    f.render_widget(Clear, modal_area);

    // Borderless solid background panel
    let modal_block = Block::default().style(Style::default().bg(COLOR_PANEL));

    f.render_widget(modal_block, modal_area);

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
    });

    // Layout constraints inside modal: Header (1), Search (1), List (Min)
    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Search
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // List area
        ])
        .split(inner_area);

    // 1. Modal Header
    let header_line = Line::from(vec![
        Span::styled(
            "Commands",
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ".repeat(inner_area.width.saturating_sub(12) as usize),
            Style::default(),
        ),
        Span::styled("esc", Style::default().fg(COLOR_MUTED)),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[0],
    );

    // 2. Search Box with cursor (flashing peach block)
    let search_line = if state.command_picker_search.is_empty() {
        Line::from(vec![
            Span::styled("█", Style::default().fg(COLOR_PRIMARY)),
            Span::styled("Search", Style::default().fg(COLOR_MUTED)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                state.command_picker_search.clone(),
                Style::default().fg(COLOR_TEXT),
            ),
            Span::styled("█", Style::default().fg(COLOR_PRIMARY)),
        ])
    };
    f.render_widget(
        Paragraph::new(search_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[2],
    );

    // 3. Commands List
    let mut list_lines = Vec::new();
    let mut current_group = String::new();

    for (idx, item) in filtered_items.iter().enumerate() {
        if item.group != current_group {
            current_group = item.group.to_string();
            list_lines.push(Line::from("")); // spacer
            list_lines.push(Line::from(Span::styled(
                current_group.clone(),
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let is_selected = selected_idx == idx;
        let line = if is_selected {
            // Selected row: solid Peach background block
            let name_part = format!(" {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(name_part.len() + item.shortcut.len());
            Line::from(vec![
                Span::styled(
                    name_part,
                    Style::default()
                        .fg(COLOR_BG)
                        .bg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ".repeat(padding_len),
                    Style::default().fg(COLOR_BG).bg(COLOR_PRIMARY),
                ),
                Span::styled(
                    item.shortcut.to_string(),
                    Style::default().fg(COLOR_BG).bg(COLOR_PRIMARY),
                ),
            ])
        } else {
            let name_part = format!("  {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(name_part.len() + item.shortcut.len());
            Line::from(vec![
                Span::styled(name_part, Style::default().fg(COLOR_TEXT)),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(item.shortcut.to_string(), Style::default().fg(COLOR_MUTED)),
            ])
        };
        list_lines.push(line);
    }

    // Scrollable viewport
    let scroll_y = selected_idx.saturating_sub(4) as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL));
    f.render_widget(list_paragraph, modal_chunks[4]);
}

/// Main render function called by the TUI event loop.
pub fn render(f: &mut Frame, state: &mut AppState) {
    // Fill the screen with solid slate background
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_BG)),
        f.area(),
    );

    // Compute active autocomplete suggestions
    let filtered_cmds: Vec<&CommandInfo> =
        if state.input_buffer.starts_with('/') && !state.input_buffer.contains(' ') {
            COMMANDS
                .iter()
                .filter(|c| c.name.starts_with(&state.input_buffer))
                .collect()
        } else {
            Vec::new()
        };

    if state.history.is_empty() {
        let (prompt_box_area, inner_area) = render_welcome_screen(f, state);

        // Draw popup menu overlay if active
        if !filtered_cmds.is_empty() {
            let popup_height = filtered_cmds.len() as u16;
            let popup_y = prompt_box_area.y.saturating_sub(popup_height);
            let popup_area =
                ratatui::layout::Rect::new(inner_area.x, popup_y, inner_area.width, popup_height);
            render_popup_menu(f, state, &filtered_cmds, popup_area);
        }
    } else {
        // Determine dynamic input area height based on input buffer length and terminal width.
        // Inner input width is terminal width minus margins and borders (6 characters).
        let inner_width = f.area().width.saturating_sub(6).max(1);
        let input_lines = count_input_lines(&state.input_buffer, inner_width as usize) + 3; // input text + 1 blank line + 1 build line + spacer
        let input_height = input_lines + 2;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Min(3),               // conversation area (grows/shrinks dynamically).
                Constraint::Length(input_height), // input area.
                Constraint::Length(1),            // spacer padding between chat input and footer!
                Constraint::Length(1),            // footer.
            ])
            .split(f.area());

        render_conversation(f, &chunks, state);
        let input_margin = render_input(f, &chunks, state);
        render_footer(f, &chunks, state);

        // Draw popup menu overlay if active
        if !filtered_cmds.is_empty() {
            let input_inner = chunks[1].inner(input_margin);
            let popup_height = filtered_cmds.len() as u16;
            let popup_y = chunks[1].y.saturating_sub(popup_height);
            let popup_area =
                ratatui::layout::Rect::new(input_inner.x, popup_y, input_inner.width, popup_height);
            render_popup_menu(f, state, &filtered_cmds, popup_area);
        }
    }

    // Draw Model Picker Modal on top of everything if toggled active
    if state.show_model_picker {
        render_model_picker_modal(f, state);
    }

    // Draw Command Picker Modal on top of everything if toggled active
    if state.show_command_picker {
        render_command_picker_modal(f, state);
    }
}
