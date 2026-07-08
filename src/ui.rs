use crate::app::{AppState, AppStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

fn pad_to_width(s: &str, width: usize) -> String {
    let current = s.width();
    if current < width {
        format!("{}{}", s, " ".repeat(width - current))
    } else {
        s.to_string()
    }
}

const COLOR_BG: Color = Color::Rgb(21, 23, 26);
const COLOR_PANEL: Color = Color::Rgb(26, 29, 32);
const COLOR_ELEMENT: Color = Color::Rgb(34, 38, 42);
const COLOR_TEXT: Color = Color::Rgb(240, 229, 222);
const COLOR_MUTED: Color = Color::Rgb(136, 146, 154);
const COLOR_PRIMARY: Color = Color::Rgb(236, 110, 93);
const COLOR_SECONDARY: Color = Color::Rgb(60, 88, 101);
const COLOR_GREEN: Color = Color::Rgb(127, 216, 143);
const COLOR_BORDER: Color = Color::Rgb(72, 85, 89);
const COLOR_TIP: Color = Color::Rgb(224, 169, 109);

const LOGO: &[&str] = &[
    "                  ▄                   █      ",
    "▄▀▀▀ █   █ ▄▀▀▀▀ ▀█▀▀ ▄▀▀▀▀ ▄▀▀▀▄ ▄▀▀▀█ ▄▀▀▀▄",
    "█    █   █  ▀▀▀▄  █   █     █   █ █   █ █▀▀▀▀",
    "▀     ▀▀▀  ▀▀▀▀    ▀▀  ▀▀▀▀  ▀▀▀   ▀▀▀▀  ▀▀▀▀",
];

pub use crate::app::suggestion::{COMMANDS, CommandInfo};

fn get_themed_style(fg: Color, bg: Color, modifier: Modifier, show_picker: bool) -> Style {
    if show_picker {
        Style::default().fg(Color::Rgb(60, 68, 72)).bg(COLOR_BG)
    } else {
        Style::default().fg(fg).bg(bg).add_modifier(modifier)
    }
}

fn model_label(state: &AppState) -> String {
    state.config.default.clone()
}

fn active_context_window(state: &AppState) -> u32 {
    state
        .config
        .models
        .iter()
        .find(|m| m.name == state.config.default)
        .and_then(|p| p.context_window)
        .unwrap_or(crate::config::DEFAULT_CONTEXT_WINDOW)
}

fn render_assistant_message<'a>(
    content: &str,
    response_time_ms: Option<u64>,
    model_name: &str,
    lines: &mut Vec<Line<'a>>,
    is_generating: bool,
    viewport_width: u16,
    show_picker: bool,
) {
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
        lines.push(Line::from(""));
    }

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
        lines.push(Line::from(""));
    }

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
    let show_picker =
        state.show_model_picker || state.show_command_picker || state.show_history_picker;

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

        let window = active_context_window(state);
        let pct = if window == 0 {
            0.0
        } else {
            ((total_tokens as f32 / window as f32) * 100.0).min(999.0)
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
            Span::styled(
                format!(" ({:.0}%)", pct),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
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

fn render_input(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) -> Margin {
    let show_picker =
        state.show_model_picker || state.show_command_picker || state.show_history_picker;

    let input_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
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
        let mut styled_chars: Vec<(char, Style)> = state
            .input_buffer
            .chars()
            .map(|c| (c, text_style))
            .collect();

        if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        let cursor_char_index = state.input_buffer
            [..state.cursor_position.min(state.input_buffer.len())]
            .chars()
            .count();

        let mut current_line_spans = Vec::new();
        let mut current_run: Option<(Style, String)> = None;

        let mut col = 0;
        let mut row = 0;

        let total_chars = styled_chars.len();
        for (i, &(c, style)) in styled_chars.iter().enumerate() {
            if i == cursor_char_index {
                cursor_dx = col as u16;
                cursor_dy = row as u16;
            }

            if c == '\n' {
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

        if cursor_char_index == total_chars {
            cursor_dx = col as u16;
            cursor_dy = row as u16;
        }

        if let Some((st, s)) = current_run {
            current_line_spans.push(Span::styled(s, st));
        }
        lines.push(Line::from(current_line_spans));
    }

    let text_area_height = input_inner.height.saturating_sub(1);
    let text_area = ratatui::layout::Rect::new(
        input_inner.x,
        input_inner.y,
        input_inner.width,
        text_area_height,
    );
    let paragraph = Paragraph::new(lines).style(Style::default().bg(COLOR_PANEL));
    f.render_widget(paragraph, text_area);

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
            model_label(state),
            get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
        ),
    ]);
    f.render_widget(Paragraph::new(build_line), build_area);

    if inner_width > 0 && !show_picker {
        f.set_cursor_position((input_inner.x + cursor_dx, input_inner.y + cursor_dy));
    }

    input_margin
}

fn render_conversation(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &mut AppState) {
    let inner_area = chunks[0].inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let show_picker =
        state.show_model_picker || state.show_command_picker || state.show_history_picker;

    let mut lines: Vec<Line> = Vec::new();

    for msg in &state.history {
        if msg.role == "system" {
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
            lines.push(Line::from(""));
        } else if msg.role == "tool" {
            for (i, raw_line) in msg.content.lines().enumerate() {
                let prefix = if i == 0 { "⚙ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(
                        prefix,
                        get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::BOLD, show_picker),
                    ),
                    Span::styled(
                        raw_line,
                        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        } else if msg.role == "user" {
            lines.push(Line::from(""));
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
            lines.push(Line::from(""));
        } else if msg.role == "assistant" {
            if let Some((name, args)) = crate::tools::parse_tool_call(&msg.content) {
                lines.push(Line::from(vec![
                    Span::styled(
                        "→ ",
                        get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::BOLD, show_picker),
                    ),
                    Span::styled(
                        format!("calling {name} {args}"),
                        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::ITALIC, show_picker),
                    ),
                ]));
                lines.push(Line::from(""));
                continue;
            }
            render_assistant_message(
                &msg.content,
                msg.response_time_ms,
                &model_label(state),
                &mut lines,
                false,
                inner_area.width,
                show_picker,
            );
            lines.push(Line::from(""));
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
                    model_label(state),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ),
            ]));
        } else {
            render_assistant_message(
                &state.current_response,
                None,
                &model_label(state),
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
                    model_label(state),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ),
            ]));
        }

        lines.push(Line::from(""));
    }

    // breathing room between the last line and the input box when
    // scrolled to the bottom
    lines.push(Line::from(""));

    let conversation_paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .style(Style::default().bg(COLOR_BG));

    // exact rendered height — the paragraph word-wraps, so estimating
    // rows from character counts undershoots and cuts off the bottom
    let total_wrapped_lines = conversation_paragraph.line_count(inner_area.width) as u16;
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

    let conversation_paragraph = conversation_paragraph.scroll((scroll_offset, 0));

    f.render_widget(conversation_paragraph, inner_area);

    let conv = chunks[0];
    let view_h = inner_area.height;
    let content_h = total_wrapped_lines.max(1);
    if content_h > view_h && max_scroll > 0 {
        let sb_x = conv.x + conv.width.saturating_sub(1);
        let sb_area = ratatui::layout::Rect::new(sb_x, conv.y, 1, view_h);
        let thumb_len = ((view_h as u32 * view_h as u32) / content_h as u32).max(1) as u16;
        let track = view_h.saturating_sub(thumb_len);
        let pos = if max_scroll == 0 {
            0
        } else {
            ((scroll_offset as u64 * track as u64) / max_scroll as u64) as u16
        };
        let mut rows = Vec::with_capacity(view_h as usize);
        for i in 0..view_h {
            let (ch, color) = if i >= pos && i < pos + thumb_len {
                ('█', COLOR_PRIMARY)
            } else {
                ('│', COLOR_BORDER)
            };
            rows.push(Line::from(Span::styled(
                ch.to_string(),
                Style::default().fg(color).bg(COLOR_BG),
            )));
        }
        f.render_widget(
            Paragraph::new(rows).style(Style::default().bg(COLOR_BG)),
            sb_area,
        );
    }
}

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

fn render_welcome_screen(
    f: &mut Frame,
    state: &AppState,
) -> (ratatui::layout::Rect, ratatui::layout::Rect) {
    let width = f.area().width;
    let height = f.area().height;

    let show_picker =
        state.show_model_picker || state.show_command_picker || state.show_history_picker;

    let box_width = 80u16.min(width.saturating_sub(6));
    let inner_width = box_width.saturating_sub(5).max(1);

    let input_lines = if state.input_buffer.is_empty() {
        1
    } else {
        count_input_lines(&state.input_buffer, inner_width as usize)
    };
    let prompt_box_height = input_lines + 4;

    let logo_start_y = height.saturating_sub(17).saturating_sub(input_lines - 1) / 2;

    let welcome_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(logo_start_y),
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Length(prompt_box_height),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(f.area());

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

    let prompt_box_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(prompt_box_area);

    let line_chars = "▌\n".repeat(prompt_box_area.height as usize);
    let vertical_line_widget = Paragraph::new(line_chars).style(get_themed_style(
        COLOR_SECONDARY,
        COLOR_BG,
        Modifier::empty(),
        show_picker,
    ));
    f.render_widget(vertical_line_widget, prompt_box_split[0]);

    let solid_panel = Block::default().style(Style::default().bg(COLOR_PANEL));

    let mut box_lines = Vec::new();
    let mut cursor_dx = 0u16;
    let mut cursor_dy = 0u16;

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

        let mut styled_chars: Vec<(char, Style)> = state
            .input_buffer
            .chars()
            .map(|c| (c, text_style))
            .collect();

        if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        let cursor_char_index = state.input_buffer
            [..state.cursor_position.min(state.input_buffer.len())]
            .chars()
            .count();

        let mut current_line_spans = Vec::new();
        let mut current_run: Option<(Style, String)> = None;

        let mut col = 0;
        let mut row = 0;

        let total_chars = styled_chars.len();
        for (i, &(c, style)) in styled_chars.iter().enumerate() {
            if i == cursor_char_index {
                cursor_dx = col as u16;
                cursor_dy = row as u16;
            }

            if c == '\n' {
                if let Some((st, s)) = current_run.take() {
                    current_line_spans.push(Span::styled(s, st));
                }
                box_lines.push(Line::from(current_line_spans.clone()));
                current_line_spans.clear();
                row += 1;
                col = 0;
            } else {
                if col >= inner_width as usize {
                    if let Some((st, s)) = current_run.take() {
                        current_line_spans.push(Span::styled(s, st));
                    }
                    box_lines.push(Line::from(current_line_spans.clone()));
                    current_line_spans.clear();
                    row += 1;
                    col = 0;
                }

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

        if cursor_char_index == total_chars {
            cursor_dx = col as u16;
            cursor_dy = row as u16;
        }

        if let Some((st, s)) = current_run.take() {
            current_line_spans.push(Span::styled(s, st));
        }
        box_lines.push(Line::from(current_line_spans));
    }

    box_lines.push(Line::from(""));

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
            model_label(state),
            get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
        ),
    ]));

    let inner = prompt_box_split[1].inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    f.render_widget(solid_panel, prompt_box_split[1]);
    f.render_widget(
        Paragraph::new(box_lines).style(Style::default().bg(COLOR_PANEL)),
        inner,
    );

    if inner.width > 0 && !show_picker {
        f.set_cursor_position(ratatui::layout::Position {
            x: inner.x + cursor_dx.min(inner.width.saturating_sub(1)),
            y: inner.y + cursor_dy,
        });
    }

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

    let tip_area = welcome_chunks[6];
    let tip_text = crate::app::TIPS[state.tip_index % crate::app::TIPS.len()];
    let tip_full = format!("{tip_text}");
    let tip_prefix = "● ";
    let prefix_w = tip_prefix.width();
    let tip_w = tip_full.width();
    let total_w = prefix_w + tip_w + 4;
    let tip_padding = (width.saturating_sub(total_w as u16) / 2) as usize;
    let centered_spans = vec![
        Span::styled(" ".repeat(tip_padding), Style::default()),
        Span::styled(
            "● ",
            get_themed_style(COLOR_TIP, COLOR_BG, Modifier::empty(), show_picker),
        ),
        Span::styled(
            "Tip ",
            get_themed_style(COLOR_TIP, COLOR_BG, Modifier::BOLD, show_picker),
        ),
        Span::styled(
            tip_full,
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ),
    ];
    f.render_widget(
        Paragraph::new(Line::from(centered_spans)).style(Style::default().bg(COLOR_BG)),
        tip_area,
    );

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

fn centered_rect_fixed(width: u16, height: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let x = r.x + r.width.saturating_sub(width) / 2;
    let y = r.y + r.height.saturating_sub(height) / 2;
    ratatui::layout::Rect::new(x, y, width.min(r.width), height.min(r.height))
}

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

/// Render the session history picker modal overlay (/history).
fn render_history_picker_modal(f: &mut Frame, state: &AppState) {
    let sessions = &state.history_picker_sessions;
    let selected_idx = state
        .history_picker_index
        .min(sessions.len().saturating_sub(1));

    let modal_area = centered_rect_fixed(65, 18, f.area());
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL)),
        modal_area,
    );

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let header_line = Line::from(vec![
        Span::styled(
            "Resume session",
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ".repeat(inner_area.width.saturating_sub(17) as usize),
            Style::default(),
        ),
        Span::styled("esc", Style::default().fg(COLOR_MUTED)),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[0],
    );

    let mut list_lines = Vec::new();
    for (idx, session) in sessions.iter().enumerate() {
        let desc = format!("{} msgs  {}", session.message_count, session.when);
        let is_selected = selected_idx == idx;
        let line = if is_selected {
            let left_text = format!(" ● {}", session.title);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.len() + desc.len());
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
                Span::styled(desc, Style::default().fg(COLOR_BG).bg(COLOR_PRIMARY)),
            ])
        } else {
            let left_text = format!("   {}", session.title);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.len() + desc.len());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT)),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(desc, Style::default().fg(COLOR_MUTED)),
            ])
        };
        list_lines.push(line);
    }

    let scroll_y = selected_idx.saturating_sub(3) as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL));
    f.render_widget(list_paragraph, modal_chunks[2]);

    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT)),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED)),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT)),
        Span::styled("enter", Style::default().fg(COLOR_MUTED)),
    ]);
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[3],
    );
}

#[derive(Clone)]
pub struct PaletteItem {
    pub group: &'static str,
    pub name: &'static str,
    pub shortcut: &'static str,
}

pub const PALETTE_ITEMS: &[PaletteItem] = &[
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
    PaletteItem {
        group: "Agent",
        name: "Switch model",
        shortcut: "/model",
    },
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

    let modal_area = centered_rect_fixed(65, 20, f.area());

    f.render_widget(Clear, modal_area);

    let modal_block = Block::default().style(Style::default().bg(COLOR_PANEL));

    f.render_widget(modal_block, modal_area);

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
        ])
        .split(inner_area);

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

    let mut list_lines = Vec::new();
    let mut current_group = String::new();

    for (idx, item) in filtered_items.iter().enumerate() {
        if item.group != current_group {
            current_group = item.group.to_string();
            list_lines.push(Line::from(""));
            list_lines.push(Line::from(Span::styled(
                current_group.clone(),
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let is_selected = selected_idx == idx;
        let line = if is_selected {
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

    let scroll_y = selected_idx.saturating_sub(4) as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL));
    f.render_widget(list_paragraph, modal_chunks[4]);
}

pub fn render(f: &mut Frame, state: &mut AppState) {
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_BG)),
        f.area(),
    );

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

        if !filtered_cmds.is_empty() {
            let popup_height = filtered_cmds.len() as u16;
            let popup_y = prompt_box_area.y.saturating_sub(popup_height);
            let popup_area =
                ratatui::layout::Rect::new(inner_area.x, popup_y, inner_area.width, popup_height);
            render_popup_menu(f, state, &filtered_cmds, popup_area);
        }
    } else {
        let inner_width = f.area().width.saturating_sub(6).max(1);
        let input_lines = count_input_lines(&state.input_buffer, inner_width as usize) + 3;
        let input_height = input_lines + 2;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(input_height),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(f.area());

        render_conversation(f, &chunks, state);
        let input_margin = render_input(f, &chunks, state);
        render_footer(f, &chunks, state);

        if !filtered_cmds.is_empty() {
            let input_inner = chunks[1].inner(input_margin);
            let popup_height = filtered_cmds.len() as u16;
            let popup_y = chunks[1].y.saturating_sub(popup_height);
            let popup_area =
                ratatui::layout::Rect::new(input_inner.x, popup_y, input_inner.width, popup_height);
            render_popup_menu(f, state, &filtered_cmds, popup_area);
        }
    }

    if state.show_model_picker {
        render_model_picker_modal(f, state);
    }

    if state.show_command_picker {
        render_command_picker_modal(f, state);
    }

    if state.show_history_picker {
        render_history_picker_modal(f, state);
    }

    if state.status == AppStatus::AwaitingToolConfirmation {
        render_tool_confirmation_modal(f, state);
    }
}

fn render_tool_confirmation_modal(f: &mut Frame, state: &AppState) {
    let confirmation = match &state.pending_tool_confirmation {
        Some(c) => c,
        None => return,
    };

    let modal_area = centered_rect_fixed(60, 16, f.area());

    f.render_widget(Clear, modal_area);

    let modal_block = Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(COLOR_BORDER))
        .style(Style::default().bg(COLOR_PANEL));
    f.render_widget(modal_block, modal_area);

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(1),
        ])
        .split(inner_area);

    let action_label = match confirmation.tool_name.as_str() {
        "create_file" => "Create file",
        "write_file" => "Overwrite file",
        "delete_file" => "Delete file",
        "move_file" => "Move file",
        "copy_file" => "Copy file",
        "run_command" => "Run command",
        _ => "Execute tool",
    };
    let header_line = Line::from(vec![Span::styled(
        format!("⚠ {action_label}?"),
        Style::default().fg(COLOR_TIP).add_modifier(Modifier::BOLD),
    )]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[0],
    );

    let tool_line = Line::from(vec![
        Span::styled("  tool  ", Style::default().fg(COLOR_MUTED)),
        Span::styled(
            &confirmation.tool_name,
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(
        Paragraph::new(tool_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[2],
    );

    let path_display = if confirmation.path.len() > inner_area.width as usize - 10 {
        let cut = inner_area.width as usize - 13;
        format!("…{}", &confirmation.path[confirmation.path.len() - cut..])
    } else {
        confirmation.path.clone()
    };
    let path_title = match confirmation.tool_name.as_str() {
        "run_command" => "  cmd   ",
        _ => "  path  ",
    };
    let path_line = Line::from(vec![
        Span::styled(path_title, Style::default().fg(COLOR_MUTED)),
        Span::styled(path_display, Style::default().fg(COLOR_PRIMARY)),
    ]);
    f.render_widget(
        Paragraph::new(path_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[3],
    );

    let size_line = Line::from(vec![
        Span::styled("  size  ", Style::default().fg(COLOR_MUTED)),
        Span::styled(
            format!("{} bytes", confirmation.content_bytes),
            Style::default().fg(COLOR_TEXT),
        ),
    ]);
    f.render_widget(
        Paragraph::new(size_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[4],
    );

    let auto_confirm_status = if state.auto_confirm {
        "[x] Auto-confirm future tool calls"
    } else {
        "[ ] Auto-confirm future tool calls"
    };
    let auto_confirm_line = Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            auto_confirm_status,
            Style::default().fg(COLOR_TIP).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (Tab to toggle)", Style::default().fg(COLOR_MUTED)),
    ]);
    f.render_widget(
        Paragraph::new(auto_confirm_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[5],
    );

    if !confirmation.content_preview.is_empty() {
        let preview_lines: Vec<Line> = confirmation
            .content_preview
            .lines()
            .take(modal_chunks[7].height as usize)
            .map(|l| {
                let display: String = l.chars().take(inner_area.width as usize - 4).collect();
                Line::from(Span::styled(
                    format!("  {display}"),
                    Style::default()
                        .fg(COLOR_MUTED)
                        .add_modifier(Modifier::ITALIC),
                ))
            })
            .collect();
        f.render_widget(
            Paragraph::new(preview_lines)
                .style(Style::default().bg(COLOR_ELEMENT))
                .wrap(Wrap { trim: false }),
            modal_chunks[7],
        );
    }

    let footer_line = Line::from(vec![
        Span::styled(
            "  y / enter",
            Style::default()
                .fg(COLOR_GREEN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" approve  ", Style::default().fg(COLOR_MUTED)),
        Span::styled(
            "n",
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" deny  ", Style::default().fg(COLOR_MUTED)),
        Span::styled(
            "tab",
            Style::default().fg(COLOR_TIP).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" toggle auto-confirm  ", Style::default().fg(COLOR_MUTED)),
        Span::styled(
            "esc",
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cancel", Style::default().fg(COLOR_MUTED)),
    ]);
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL)),
        modal_chunks[8],
    );
}
