use crate::app::{AppState, AppStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
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

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "fn" | "let"
            | "mut"
            | "pub"
            | "use"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "match"
            | "if"
            | "else"
            | "return"
            | "loop"
            | "for"
            | "in"
            | "while"
            | "async"
            | "await"
            | "mod"
            | "crate"
            | "self"
            | "Self"
            | "true"
            | "false"
            | "const"
            | "static"
            | "type"
            | "where"
            | "dyn"
            | "as"
            | "ref"
            | "move"
            | "unsafe"
    )
}

fn is_type(s: &str) -> bool {
    matches!(
        s,
        "Option"
            | "Result"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
            | "String"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "bool"
            | "Vec"
            | "Arc"
            | "Rc"
            | "Mutex"
            | "Box"
            | "Pin"
            | "Future"
            | "Instant"
            | "Duration"
    ) || (!s.is_empty() && s.chars().next().unwrap().is_uppercase())
}

fn highlight_rust_line_with_colors<'a>(
    line: &str,
    default_fg: Color,
    bg_color: Color,
    show_picker: bool,
) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    let color_keyword = Color::Rgb(198, 120, 221); // Purple
    let color_type = Color::Rgb(229, 192, 123); // Yellow
    let color_string = Color::Rgb(152, 195, 121); // Green
    let color_comment = Color::Rgb(92, 99, 112); // Gray (muted)
    let color_number = Color::Rgb(209, 154, 102); // Orange
    let color_macro = Color::Rgb(97, 175, 239); // Blue
    let color_fn = Color::Rgb(97, 175, 239); // Blue

    while i < chars.len() {
        // Comments
        if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
            let comment_text: String = chars[i..].iter().collect();
            spans.push(Span::styled(
                comment_text,
                get_themed_style(color_comment, bg_color, Modifier::empty(), show_picker),
            ));
            break;
        }

        // Strings
        if chars[i] == '"' {
            let mut s = String::new();
            s.push('"');
            i += 1;
            let mut escaped = false;
            while i < chars.len() {
                let c = chars[i];
                s.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(
                s,
                get_themed_style(color_string, bg_color, Modifier::empty(), show_picker),
            ));
            continue;
        }

        // Characters
        if chars[i] == '\'' {
            let mut s = String::new();
            s.push('\'');
            i += 1;
            let mut escaped = false;
            while i < chars.len() {
                let c = chars[i];
                s.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '\'' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(
                s,
                get_themed_style(color_string, bg_color, Modifier::empty(), show_picker),
            ));
            continue;
        }

        // Numbers
        if chars[i].is_ascii_digit() {
            let mut num = String::new();
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || chars[i] == '_'
                    || chars[i].is_ascii_alphabetic())
            {
                num.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(
                num,
                get_themed_style(color_number, bg_color, Modifier::empty(), show_picker),
            ));
            continue;
        }

        // Identifiers
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let mut ident = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                ident.push(chars[i]);
                i += 1;
            }

            let is_macro = i < chars.len() && chars[i] == '!';
            let is_fn = !is_macro
                && ((i < chars.len() && chars[i] == '(')
                    || (i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] == ':'));

            let style = if is_macro {
                ident.push('!');
                i += 1;
                get_themed_style(color_macro, bg_color, Modifier::BOLD, show_picker)
            } else if is_keyword(&ident) {
                get_themed_style(color_keyword, bg_color, Modifier::BOLD, show_picker)
            } else if is_type(&ident) {
                get_themed_style(color_type, bg_color, Modifier::empty(), show_picker)
            } else if is_fn {
                get_themed_style(color_fn, bg_color, Modifier::empty(), show_picker)
            } else {
                get_themed_style(default_fg, bg_color, Modifier::empty(), show_picker)
            };

            spans.push(Span::styled(ident, style));
            continue;
        }

        // Symbols
        let mut symbol = String::new();
        symbol.push(chars[i]);
        i += 1;
        spans.push(Span::styled(
            symbol,
            get_themed_style(default_fg, bg_color, Modifier::empty(), show_picker),
        ));
    }

    spans
}

fn highlight_rust_line<'a>(line: &str, show_picker: bool) -> Vec<Span<'a>> {
    highlight_rust_line_with_colors(line, COLOR_TEXT, COLOR_ELEMENT, show_picker)
}

fn highlight_diff_line<'a>(line: &str, width: usize, show_picker: bool) -> Line<'a> {
    let (prefix, code) = if line.is_empty() {
        (' ', "")
    } else {
        let mut chars = line.chars();
        let first = chars.next().unwrap();
        if first == '+' || first == '-' || first == ' ' {
            (first, chars.as_str())
        } else {
            (' ', line)
        }
    };

    let bg_color = match prefix {
        '+' => Color::Rgb(24, 40, 24), // Dark Green
        '-' => Color::Rgb(48, 20, 20), // Dark Red
        _ => COLOR_ELEMENT,            // Dark Gray
    };

    let default_fg = match prefix {
        '+' => Color::Rgb(160, 240, 160), // Light Green
        '-' => Color::Rgb(240, 150, 150), // Light Red
        _ => COLOR_TEXT,                  // Default text color
    };

    let spans = highlight_rust_line_with_colors(code, default_fg, bg_color, show_picker);

    let prefix_str = format!("{} ", prefix);
    let mut final_spans = vec![Span::styled(
        prefix_str,
        get_themed_style(default_fg, bg_color, Modifier::BOLD, show_picker),
    )];
    final_spans.extend(spans);

    let current_width: usize = final_spans.iter().map(|s| s.content.width()).sum();
    if current_width < width {
        final_spans.push(Span::styled(
            " ".repeat(width - current_width),
            get_themed_style(default_fg, bg_color, Modifier::empty(), show_picker),
        ));
    }

    Line::from(final_spans)
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
/// Uniform text-selection background — a touch lighter than the panel.
const COLOR_SELECTION: Color = Color::Rgb(45, 50, 56);
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
    // Only show the main (big) model — hide the small model entirely.
    state.config.default.big().to_string()
}

fn render_assistant_message<'a>(
    content: &str,
    response_time_ms: Option<u64>,
    model_name: &str,
    lines: &mut Vec<Line<'a>>,
    is_generating: bool,
    viewport_width: u16,
    show_picker: bool,
    thought_collapsed: bool,
    msg_index: Option<usize>,
    click_registry: &mut Vec<(usize, usize)>,
) {
    let mut think_content = None;
    let mut main_content = content;

    if content.contains("<think>") {
        if let Some(start_idx) = content.find("<think>") {
            if let Some(real_end_idx) = content[start_idx..].find("</think>") {
                let end_idx = start_idx + real_end_idx;
                let think_part = &content[start_idx + 7..end_idx];
                let main_part = &content[end_idx + 8..];
                think_content = Some(think_part.trim());
                main_content = main_part.trim();
            } else {
                let think_part = &content[start_idx + 7..];
                think_content = Some(think_part.trim());
                main_content = "";
            }
        }
    }

    if let Some(think) = think_content {
        let base = if let Some(ms) = response_time_ms {
            if ms >= 1000 {
                format!("Thought: {:.1}s", ms as f32 / 1000.0)
            } else {
                format!("Thought: {}ms", ms)
            }
        } else {
            "Thought:".to_string()
        };
        // Streaming (no msg_index) shows thoughts live with no toggle chip.
        let toggle = match msg_index {
            Some(_) if thought_collapsed => "+ ",
            Some(_) => "− ",
            None => "",
        };
        if let Some(idx) = msg_index {
            click_registry.push((lines.len(), idx));
        }
        lines.push(Line::from(Span::styled(
            format!("{}{}", toggle, base),
            get_themed_style(
                Color::Rgb(229, 192, 123),
                COLOR_BG,
                Modifier::empty(),
                show_picker,
            ),
        )));

        if !thought_collapsed {
            for raw_line in think.lines() {
                lines.push(Line::from(Span::styled(
                    raw_line.to_string(),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                )));
            }
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
                let mut line_spans = highlight_rust_line(raw_line, show_picker);
                let current_width: usize = line_spans.iter().map(|s| s.content.width()).sum();
                if current_width < content_width {
                    line_spans.push(Span::styled(
                        " ".repeat(content_width - current_width),
                        get_themed_style(COLOR_TEXT, COLOR_ELEMENT, Modifier::empty(), show_picker),
                    ));
                }
                spans.extend(line_spans);
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
        ];

        status_spans.push(Span::styled(
            model_name.to_string(),
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ));

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

/// Returns ("Tokens/s: ", "N.N") with the live rate when streaming, or "0.0" when not.
fn format_tokens_info(state: &AppState) -> (String, String) {
    if state.status == AppStatus::Streaming {
        if let Some(ref tracker) = state.stream_tracker {
            let (tps, _) = tracker.snapshot();
            return ("Tps: ".to_string(), format!("{:.1}", tps));
        }
    }
    ("Tps: ".to_string(), "0.0".to_string())
}

fn render_footer(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) {
    let footer_area = chunks[3];
    let show_picker = state.modal_open();

    let left_spans = if state.status == AppStatus::Streaming
        || state.status == AppStatus::Queued
        || !state.running_tools.is_empty()
    {
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

        if let Some(tool_name) = state.running_tools.first() {
            spans.push(Span::styled(
                format!("  executing: {tool_name}"),
                get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::BOLD, show_picker),
            ));
        } else if !state.pending_queue.is_empty() {
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
            Span::styled("   ", Style::default()),
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
        let (total_tokens, cached_tokens) = if let Some(usage) = &state.current_token_usage {
            (usage.total_tokens, usage.cached_tokens)
        } else {
            let last_usage = state
                .history
                .iter()
                .rev()
                .find_map(|m| m.token_usage.as_ref());
            if let Some(u) = last_usage {
                (u.total_tokens, u.cached_tokens)
            } else {
                let chars: usize = state.history.iter().map(|m| m.content.len()).sum();
                ((chars / 4) as u32, None)
            }
        };

        let token_str = if total_tokens >= 1000 {
            format!("{:.1}K", total_tokens as f32 / 1000.0)
        } else {
            format!("{}", total_tokens)
        };

        let cached_str = if let Some(cached) = cached_tokens {
            if cached > 0 {
                let cached_formatted = if cached >= 1000 {
                    format!("{:.1}K", cached as f32 / 1000.0)
                } else {
                    format!("{}", cached)
                };
                format!(" ({} cached)", cached_formatted)
            } else {
                "".to_string()
            }
        } else {
            "".to_string()
        };

        let window = state.active_context_window();
        let pct = if window == 0 {
            0.0
        } else {
            ((total_tokens as f32 / window as f32) * 100.0).min(100.0)
        };

        let mut right_spans = Vec::new();

        // Add leading padding for visual spacing at start
        right_spans.push(Span::styled("   ", Style::default()));

        let tps_label = format_tokens_info(state).0;
        let tps_value = format_tokens_info(state).1;
        if !tps_label.is_empty() {
            right_spans.push(Span::styled(
                tps_label,
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ));
            right_spans.push(Span::styled(
                tps_value,
                get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::BOLD, show_picker),
            ));
        }

        right_spans.push(Span::styled(
            "   Context: ",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ));
        right_spans.push(Span::styled(
            token_str,
            get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::BOLD, show_picker),
        ));
        if !cached_str.is_empty() {
            right_spans.push(Span::styled(
                cached_str,
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ));
        }
        right_spans.push(Span::styled(
            format!(" ({:.0}%)", pct),
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ));

        right_spans.push(Span::styled("   ", Style::default()));
        right_spans.push(Span::styled(
            "ctrl+p",
            get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
        ));
        right_spans.push(Span::styled(
            " commands",
            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
        ));

        right_spans
    };

    let footer_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(22),
            Constraint::Fill(1),
        ])
        .split(footer_area);

    let status_color = if state.auto_confirm {
        COLOR_PRIMARY
    } else {
        COLOR_MUTED
    };
    let status_modifier = if state.auto_confirm {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };

    f.render_widget(
        Paragraph::new(Line::from(left_spans)).style(Style::default().bg(COLOR_BG)),
        footer_chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Auto-Confirm: ",
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ),
            Span::styled(
                state.auto_confirm_status_text(),
                get_themed_style(status_color, COLOR_BG, status_modifier, show_picker),
            ),
        ]))
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().bg(COLOR_BG)),
        footer_chunks[1],
    );
    f.render_widget(
        Paragraph::new(Line::from(right_spans))
            .alignment(ratatui::layout::Alignment::Right)
            .style(Style::default().bg(COLOR_BG)),
        footer_chunks[2],
    );
}

fn render_input(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) -> Margin {
    let show_picker = state.modal_open();

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

fn format_tool_call_brief(name: &str, args: &serde_json::Value) -> String {
    match name {
        "view_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            let start = args.get("start_line").and_then(|v| v.as_i64()).unwrap_or(1);
            let end = args.get("end_line").and_then(|v| v.as_i64());
            if let Some(e) = end {
                format!("view_file: view {} lines {}-{}", path, start, e)
            } else {
                format!("view_file: view {} starting at line {}", path, start)
            }
        }
        "replace_file_content" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            let start = args.get("start_line").and_then(|v| v.as_i64()).unwrap_or(0);
            let end = args.get("end_line").and_then(|v| v.as_i64()).unwrap_or(0);
            format!("replace_file_content: replace {} lines {}-{}", path, start, end)
        }
        "multi_replace_file_content" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            let count = args.get("replacements").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0);
            format!("multi_replace_file_content: apply {} edits to {}", count, path)
        }
        "write_to_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            let overwrite = args.get("overwrite").and_then(|o| o.as_bool()).unwrap_or(false);
            if overwrite {
                format!("write_to_file: overwrite {}", path)
            } else {
                format!("write_to_file: create {}", path)
            }
        }
        "delete_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("delete_file: delete {}", path)
        }
        "move_file" => {
            let src = args.get("src").and_then(|v| v.as_str()).unwrap_or("?");
            let dest = args.get("dest").and_then(|v| v.as_str()).unwrap_or("?");
            format!("move_file: {} -> {}", src, dest)
        }
        "copy_file" => {
            let src = args.get("src").and_then(|v| v.as_str()).unwrap_or("?");
            let dest = args.get("dest").and_then(|v| v.as_str()).unwrap_or("?");
            format!("copy_file: {} -> {}", src, dest)
        }
        "run_command" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("?");
            format!("run_command: {}", cmd)
        }
        "search_web" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("?");
            format!("search_web: \"{}\"", query)
        }
        "find_symbol" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("?");
            format!("find_symbol: \"{}\"", query)
        }
        "grep" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("grep: \"{}\" in {}", pattern, path)
        }
        "glob" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("glob: \"{}\" in {}", pattern, path)
        }
        "list_directory" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("list_directory: {}", path)
        }
        _ => format!("{}: {}", name, args),
    }
}

fn render_conversation(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &mut AppState) {
    let inner_area = chunks[0].inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let show_picker = state.modal_open();
    state.viewport_height = inner_area.height;

    let mut lines: Vec<Line> = Vec::new();

    let mut thought_clicks: Vec<(usize, usize)> = Vec::new();

    for (msg_idx, msg) in state.history.iter().enumerate() {
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
            let (tool_name, tool_result) = if let Some(pos) = msg.content.find(": ") {
                (&msg.content[..pos], &msg.content[pos + 2..])
            } else {
                ("", msg.content.as_str())
            };

            let line_count = tool_result.lines().count();
            let byte_count = tool_result.len();
            // Default is a compact one-liner. We never dump full raw output into the
            // chat — file/command bodies are noise unless they're a diff (handled via
            // msg.diff below) or a short command result (previewed a few lines down).
            let summary = match tool_name {
                "read_file" | "view_file" => {
                    format!(
                        "completed (read {} lines, {} bytes)",
                        line_count, byte_count
                    )
                }
                "grep" => format!("completed ({} matching lines)", line_count),
                "glob" => format!("completed ({} files found)", line_count),
                "list_directory" => format!("completed ({} entries listed)", line_count),
                "find_symbol" => format!("completed ({} symbols found)", line_count),
                "get_project_map" => format!("completed ({} bytes of map generated)", byte_count),
                "search_web" => format!("completed ({} bytes of search results)", byte_count),
                _ => {
                    let trimmed = tool_result.trim();
                    if trimmed.is_empty() {
                        "completed".to_string()
                    } else if line_count <= 1 && trimmed.width() <= 80 {
                        format!("completed · {}", trimmed)
                    } else {
                        format!("completed ({} lines)", line_count)
                    }
                }
            };

            lines.push(Line::from(vec![
                Span::styled(
                    "⚙ ",
                    get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    format!("{}: ", tool_name),
                    get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    summary,
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ),
            ]));

            // Command output is useful (test results, errors) but capped so it never
            // floods the chat. Longer output is summarised by the line count above.
            if tool_name == "run_command" && !tool_result.trim().is_empty() {
                const MAX_PREVIEW: usize = 20;
                let preview: Vec<&str> = tool_result.lines().take(MAX_PREVIEW).collect();
                for line_str in preview {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "  ",
                            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                        ),
                        Span::styled(
                            line_str.to_string(),
                            get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                        ),
                    ]));
                }
                if line_count > MAX_PREVIEW {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  … {} more lines", line_count - MAX_PREVIEW),
                        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::ITALIC, show_picker),
                    )]));
                }
            }

            if let Some(ref diff) = msg.diff {
                let content_width = (inner_area.width as usize).saturating_sub(6);
                for line_str in diff.lines() {
                    let line = highlight_diff_line(line_str, content_width, show_picker);
                    let mut new_spans = vec![Span::styled(
                        "  ",
                        get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::empty(), show_picker),
                    )];
                    new_spans.extend(line.spans);
                    lines.push(Line::from(new_spans));
                }
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
            if let Some((name, args)) =
                crate::tools::parse_tool_call(&msg.content, state.config.tool_protocol)
            {
                let brief = format_tool_call_brief(&name, &args);
                lines.push(Line::from(vec![
                    Span::styled(
                        "→ ",
                        get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::BOLD, show_picker),
                    ),
                    Span::styled(
                        brief,
                        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::ITALIC, show_picker),
                    ),
                ]));
                lines.push(Line::from(""));
                continue;
            }
            let collapsed = !state.expanded_thoughts.contains(&msg_idx);
            render_assistant_message(
                &msg.content,
                msg.response_time_ms,
                &model_label(state),
                &mut lines,
                false,
                inner_area.width,
                show_picker,
                collapsed,
                Some(msg_idx),
                &mut thought_clicks,
            );
            lines.push(Line::from(""));
        }
    }

    if state.status == AppStatus::Streaming || state.status == AppStatus::Queued {
        let label = if let Some(tool_name) = state.running_tools.first() {
            format!("Executing {tool_name}")
        } else {
            "Build".to_string()
        };

        if state.current_response.is_empty() {
            let mut status_spans: Vec<Span> = vec![
                Span::styled(
                    "■ ",
                    get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::empty(), show_picker),
                ),
                Span::styled(
                    label,
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    " · ",
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ),
            ];

            status_spans.push(Span::styled(
                model_label(state),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
            ));

            lines.push(Line::from(status_spans));
        } else {
            render_assistant_message(
                &state.current_response,
                None,
                &model_label(state),
                &mut lines,
                true,
                inner_area.width,
                show_picker,
                false,
                None,
                &mut thought_clicks,
            );

            lines.push(Line::from(vec![
                Span::styled(
                    "■ ",
                    get_themed_style(COLOR_PRIMARY, COLOR_BG, Modifier::empty(), show_picker),
                ),
                Span::styled(
                    label,
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

    // Resolve each clickable thought header's wrapped start row. Lines wrap
    // independently, so per-line line_count sums to the exact screen offset.
    let mut header_wrapped_rows: Vec<(u16, usize)> = Vec::new();
    if let Some(&(last_line, _)) = thought_clicks.last() {
        let click_map: std::collections::HashMap<usize, usize> =
            thought_clicks.iter().copied().collect();
        let mut cum = 0u16;
        for (i, line) in lines.iter().enumerate() {
            if let Some(&midx) = click_map.get(&i) {
                header_wrapped_rows.push((cum, midx));
            }
            let h = Paragraph::new(vec![line.clone()])
                .wrap(Wrap { trim: false })
                .line_count(inner_area.width) as u16;
            cum = cum.saturating_add(h);
            if i >= last_line {
                break;
            }
        }
    }

    let conversation_paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
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

    // Map visible thought headers to on-screen rows for click hit-testing.
    state.thought_toggle_rows.clear();
    for (wrapped_row, midx) in header_wrapped_rows {
        if wrapped_row >= scroll_offset && wrapped_row < scroll_offset + inner_area.height {
            let screen_row = inner_area.y + (wrapped_row - scroll_offset);
            state.thought_toggle_rows.push((screen_row, midx));
        }
    }

    let conv = chunks[0];
    let view_h = inner_area.height;
    let content_h = total_wrapped_lines.max(1);
    state.scrollbar_height = 0;
    if content_h > view_h && max_scroll > 0 {
        let sb_x = conv.x + conv.width.saturating_sub(1);
        let sb_area = ratatui::layout::Rect::new(sb_x, conv.y, 1, view_h);
        state.scrollbar_col = sb_x;
        state.scrollbar_top = conv.y;
        state.scrollbar_height = view_h;
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

    let show_picker = state.modal_open();

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
        format!("v{}", env!("CARGO_PKG_VERSION")),
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

    // Scrollable widget viewport
    let list_height = modal_chunks[4].height as usize;
    // Find the actual line index in list_lines for the selected item (accounting for group headers)
    let mut list_line_idx = 0;
    let mut target_list_idx: usize = 0;
    for (i, item) in filtered_items.iter().enumerate() {
        if i == 0 || item.group != filtered_items[i - 1].group {
            list_line_idx += 2; // blank line + group header
        }
        if i == selected_idx {
            target_list_idx = list_line_idx;
            break;
        }
        list_line_idx += 1;
    }
    let total_lines = list_lines.len();
    let scroll_y: u16 = if total_lines <= list_height {
        0
    } else {
        let ideal = target_list_idx.saturating_sub(list_height / 3);
        let lo = target_list_idx.saturating_sub(list_height - 1).max(0);
        let hi = target_list_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
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
    // Confirmation overlay for delete (Ctrl+D)
    if let Some(del_idx) = state.pending_delete_session_idx {
        let modal_area = centered_rect_fixed(50, 14, f.area());
        f.render_widget(Clear, modal_area);
        f.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_PRIMARY))
                .style(Style::default().bg(COLOR_PANEL)),
            modal_area,
        );

        let inner = modal_area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Length(1), // Spacer
                Constraint::Length(1), // Body text
                Constraint::Length(1), // Spacer
                Constraint::Length(1), // Confirm buttons
            ])
            .split(inner);

        let title = format!("Delete session {}?", del_idx + 1);
        f.render_widget(
            Paragraph::new(title).style(Style::default().fg(COLOR_PRIMARY).add_modifier(Modifier::BOLD)),
            chunks[0],
        );
        // Spacer
        f.render_widget(Clear, chunks[1]);
        if let Some(meta) = state.history_picker_sessions.get(del_idx) {
            f.render_widget(
                Paragraph::new(meta.title.clone()).style(Style::default().fg(COLOR_TEXT)),
                chunks[2],
            );
        }
        // Spacer
        f.render_widget(Clear, chunks[3]);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("y", Style::default().fg(COLOR_PRIMARY).add_modifier(Modifier::BOLD)),
                Span::styled(":delete    ", Style::default().fg(COLOR_MUTED)),
                Span::styled("n/esc", Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD)),
                Span::styled(":cancel  ", Style::default().fg(COLOR_MUTED)),
            ])).style(Style::default()),
            chunks[4],
        );

        return;
    }

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

    let list_height = modal_chunks[2].height as usize;
    let total_lines = list_lines.len();
    let scroll_y: u16 = if total_lines <= list_height {
        0
    } else {
        let ideal = selected_idx.saturating_sub(list_height / 3);
        let lo = selected_idx.saturating_sub(list_height - 1).max(0);
        let hi = selected_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
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

fn render_mcp_config_modal(f: &mut Frame, state: &AppState) {
    let servers = &state.config.mcp_servers;
    let selected_idx = state.mcp_picker_index;

    let modal_area = centered_rect_fixed(70, 18, f.area());
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
            Constraint::Min(3),    // Content area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    if let Some(ref edit_state) = state.mcp_edit_state {
        // --- ADD / EDIT MODE ---
        let title = if edit_state.is_add {
            "Add MCP Server"
        } else {
            "Edit MCP Server"
        };
        let header_line = Line::from(vec![Span::styled(
            title,
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(Paragraph::new(header_line), modal_chunks[0]);

        // Draw 3 input fields
        let form_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Name input
                Constraint::Length(3), // Command input
                Constraint::Length(3), // Args input
            ])
            .split(modal_chunks[2]);

        for field_idx in 0..3 {
            let label = match field_idx {
                0 => "Server Name",
                1 => "Executable Command",
                _ => "Arguments (space-separated)",
            };
            let val = match field_idx {
                0 => &edit_state.name_input,
                1 => &edit_state.command_input,
                _ => &edit_state.args_input,
            };

            let is_active = edit_state.active_field == field_idx;
            let display_val = if is_active {
                format!("{val}_")
            } else {
                val.clone()
            };

            let border_style = if is_active {
                Style::default().fg(COLOR_TEXT)
            } else {
                Style::default().fg(COLOR_MUTED)
            };

            f.render_widget(
                Paragraph::new(display_val).block(
                    Block::default()
                        .title(Span::styled(label, Style::default().fg(COLOR_MUTED)))
                        .borders(Borders::ALL)
                        .border_style(border_style),
                ),
                form_chunks[field_idx],
            );
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "enter",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Save    ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "esc",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Cancel    ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "tab / arrows",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Switch Field", Style::default().fg(COLOR_MUTED)),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[3]);
    } else {
        // --- LIST MODE ---
        let header_line = Line::from(vec![
            Span::styled(
                "MCP Servers Configuration",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " ".repeat(inner_area.width.saturating_sub(29) as usize),
                Style::default(),
            ),
            Span::styled("esc", Style::default().fg(COLOR_MUTED)),
        ]);
        f.render_widget(Paragraph::new(header_line), modal_chunks[0]);

        let mut list_lines = Vec::new();
        for (idx, srv) in servers.iter().enumerate() {
            let is_selected = selected_idx == idx;
            let status = if srv.enabled { "Enabled" } else { "Disabled" };
            let status_style = if srv.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(COLOR_MUTED)
            };

            let cmd_text = format!("{} {}", srv.command, srv.args.join(" "));

            let line = if is_selected {
                let left_text = format!(" ● {}", srv.name);
                let right_text = format!(" [{}] {}", status, cmd_text);
                let padding_len =
                    (inner_area.width as usize).saturating_sub(left_text.len() + right_text.len());

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
                    Span::styled(format!(" [{}]", status), status_style.bg(COLOR_PRIMARY)),
                    Span::styled(
                        format!(" {}", cmd_text),
                        Style::default()
                            .fg(COLOR_BG)
                            .bg(COLOR_PRIMARY)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        format!("{:<20}", srv.name),
                        Style::default().fg(COLOR_MUTED),
                    ),
                    Span::styled(" [", Style::default().fg(COLOR_MUTED)),
                    Span::styled(status, status_style),
                    Span::styled("] ", Style::default().fg(COLOR_MUTED)),
                    Span::styled(cmd_text, Style::default().fg(COLOR_MUTED)),
                ])
            };
            list_lines.push(line);
        }

        if list_lines.is_empty() {
            f.render_widget(
                Paragraph::new("No MCP servers configured.\nPress 'a' to add a new server.")
                    .style(Style::default().fg(COLOR_MUTED)),
                modal_chunks[2],
            );
        } else {
            f.render_widget(Paragraph::new(list_lines), modal_chunks[2]);
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "a",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Add    ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "e",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Edit    ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "d",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Delete    ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "enter",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Toggle Enabled", Style::default().fg(COLOR_MUTED)),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[3]);
    }
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
        group: "Agent",
        name: "Set context window",
        shortcut: "/context",
    },
    PaletteItem {
        group: "Agent",
        name: "Set parser/tool protocol",
        shortcut: "/parser",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure provider profile",
        shortcut: "/provider",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure Ollama models",
        shortcut: "/ollama",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure MCP servers",
        shortcut: "/mcp",
    },
    PaletteItem {
        group: "Session",
        name: "Change session title",
        shortcut: "/change_title",
    },
    PaletteItem {
        group: "Session",
        name: "Clear conversation",
        shortcut: "/clear",
    },
    PaletteItem {
        group: "Session",
        name: "Cancel active stream",
        shortcut: "/cancel",
    },
    PaletteItem {
        group: "System",
        name: "Show help",
        shortcut: "/help",
    },
    PaletteItem {
        group: "System",
        name: "Show token usage stats",
        shortcut: "/stats",
    },
    PaletteItem {
        group: "System",
        name: "Show token usage (alias)",
        shortcut: "/usage",
    },
    PaletteItem {
        group: "System",
        name: "Show RAM usage",
        shortcut: "/memory",
    },
    PaletteItem {
        group: "System",
        name: "List available tools",
        shortcut: "/tools",
    },
    PaletteItem {
        group: "System",
        name: "Exit the app",
        shortcut: "ctrl+c",
    },
    PaletteItem {
        group: "System",
        name: "Quit the app",
        shortcut: "/quit",
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

    let list_height = modal_chunks[4].height as usize;
    // Find the actual line index in list_lines for the selected item (accounting for group headers)
    let mut list_line_idx = 0;
    let mut target_list_idx: usize = 0;
    for (i, item) in filtered_items.iter().enumerate() {
        if i == 0 || item.group != filtered_items[i - 1].group {
            list_line_idx += 2; // blank line + group header
        }
        if i == selected_idx {
            target_list_idx = list_line_idx;
            break;
        }
        list_line_idx += 1;
    }
    let total_lines = list_lines.len();
    let scroll_y: u16 = if total_lines <= list_height {
        0
    } else {
        let ideal = target_list_idx.saturating_sub(list_height / 3);
        let lo = target_list_idx.saturating_sub(list_height - 1).max(0);
        let hi = target_list_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
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
            .horizontal_margin(3)
            .vertical_margin(1)
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

    if state.show_mcp_config {
        render_mcp_config_modal(f, state);
    }

    if state.status == AppStatus::AwaitingToolConfirmation {
        render_tool_confirmation_modal(f, state);
    }

    // Painted last so it sits on top of everything, like a native selection.
    if !state.modal_open() {
        if let (Some(start), Some(end)) = (state.sel_start, state.sel_end) {
            highlight_selection(f, start, end);
        }
    }
}

/// Inverts the cells covered by a text selection (screen coords), row-major, so it
/// reads like selecting a paragraph on a web page rather than a rectangular block.
fn highlight_selection(f: &mut Frame, start: (u16, u16), end: (u16, u16)) {
    let (start, end) = if (start.1, start.0) <= (end.1, end.0) {
        (start, end)
    } else {
        (end, start)
    };
    let buf = f.buffer_mut();
    let area = buf.area;
    let width = area.width;
    if width == 0 {
        return;
    }
    for row in start.1..=end.1 {
        if row < area.y || row >= area.y + area.height {
            continue;
        }
        let col_from = if row == start.1 { start.0 } else { area.x };
        let col_to = if row == end.1 {
            end.0
        } else {
            area.x + width - 1
        };
        for col in col_from..=col_to {
            if col < area.x || col >= area.x + width {
                continue;
            }
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(col, row)) {
                cell.set_fg(Color::Rgb(255, 255, 255));
                cell.set_bg(COLOR_SELECTION);
            }
        }
    }
}

/// Reconstructs selected text from the last rendered buffer, row-major with trailing
/// whitespace trimmed per line — matches what the highlight shows on screen.
pub fn extract_selection(
    buf: &ratatui::buffer::Buffer,
    start: (u16, u16),
    end: (u16, u16),
) -> String {
    let (start, end) = if (start.1, start.0) <= (end.1, end.0) {
        (start, end)
    } else {
        (end, start)
    };
    let area = buf.area;
    let width = area.width;
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for row in start.1..=end.1 {
        if row < area.y || row >= area.y + area.height {
            continue;
        }
        let col_from = if row == start.1 { start.0 } else { area.x };
        let col_to = if row == end.1 {
            end.0.min(area.x + width - 1)
        } else {
            area.x + width - 1
        };
        let mut line = String::new();
        for col in col_from..=col_to {
            if let Some(cell) = buf.cell(ratatui::layout::Position::new(col, row)) {
                let sym = cell.symbol();
                let filtered: String = sym
                    .chars()
                    .filter(|&c| c != '\0' && !c.is_control())
                    .collect();
                line.push_str(&filtered);
            }
        }
        let mut clean = line.trim_end();

        // Strip leading UI border prefixes
        for prefix in &[
            "│ ", "│", "▌ ", "▌", "⚙ ", "⚙", "→ ", "→", "🦀 ", "🦀", "🌐 ", "🌐",
        ] {
            if clean.starts_with(prefix) {
                clean = &clean[prefix.len()..];
                break;
            }
        }

        // Strip trailing scrollbar blocks
        for suffix in &[" █", "█", " ░", "░", " ▒", "▒", " ▓", "▓"] {
            if clean.ends_with(suffix) {
                clean = &clean[..clean.len() - suffix.len()];
                break;
            }
        }

        out.push_str(clean.trim_end());
        if row != end.1 {
            out.push('\n');
        }
    }
    out
}

fn render_tool_confirmation_modal(f: &mut Frame, state: &AppState) {
    let show_picker = state.modal_open();
    let confirmations = match &state.pending_tool_confirmation {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    if confirmations.len() == 1 {
        let confirmation = &confirmations[0];
        let modal_area = centered_rect_fixed(60, 16, f.area());

        f.render_widget(Clear, modal_area);

        let modal_block = Block::default().style(Style::default().bg(COLOR_PANEL));
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
                Constraint::Length(1),
            ])
            .split(inner_area);

        let action_label = match confirmation.tool_name.as_str() {
            "write_to_file" => "Write to file",
            "replace_file_content" => "Replace file content",
            "multi_replace_file_content" => "Apply multi-replace",
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
                    let width = (inner_area.width as usize).saturating_sub(4);
                    highlight_diff_line(l, width, show_picker)
                })
                .collect();
            f.render_widget(
                Paragraph::new(preview_lines).wrap(Wrap { trim: false }),
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
    } else {
        // Render batch confirmation modal
        let modal_area = centered_rect_fixed(70, 16, f.area());
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
                Constraint::Length(1), // Header
                Constraint::Length(1), // Spacer
                Constraint::Min(5),    // List of tools
                Constraint::Length(1), // Auto-confirm option
                Constraint::Length(1), // Spacer
                Constraint::Length(1), // Footer/Actions
            ])
            .split(inner_area);

        let header_line = Line::from(vec![Span::styled(
            format!("⚠ Approve {} tool calls in parallel?", confirmations.len()),
            Style::default().fg(COLOR_TIP).add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(Paragraph::new(header_line), modal_chunks[0]);

        let mut tool_lines = Vec::new();
        for (i, c) in confirmations.iter().enumerate() {
            let action = match c.tool_name.as_str() {
                "write_to_file" => "Write to file",
                "replace_file_content" => "Replace file content",
                "multi_replace_file_content" => "Apply multi-replace",
                "create_file" => "Create file",
                "write_file" => "Overwrite file",
                "delete_file" => "Delete file",
                "move_file" => "Move file",
                "copy_file" => "Copy file",
                "run_command" => "Run command",
                _ => "Execute tool",
            };

            let path_display = if c.path.len() > inner_area.width as usize - 25 {
                let cut = inner_area.width as usize - 28;
                format!("…{}", &c.path[c.path.len() - cut..])
            } else {
                c.path.clone()
            };

            let line = Line::from(vec![
                Span::styled(format!("  {}. ", i + 1), Style::default().fg(COLOR_MUTED)),
                Span::styled(
                    format!("{:<15}", action),
                    Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", path_display),
                    Style::default().fg(COLOR_PRIMARY),
                ),
            ]);
            tool_lines.push(line);
        }

        f.render_widget(Paragraph::new(tool_lines), modal_chunks[2]);

        let auto_confirm_status = if state.auto_confirm {
            "[x] Auto-confirm future tool calls"
        } else {
            "[ ] Auto-confirm future tool calls"
        };
        let auto_confirm_line = Line::from(vec![
            Span::styled(
                auto_confirm_status,
                Style::default().fg(COLOR_TIP).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" (Tab to toggle)", Style::default().fg(COLOR_MUTED)),
        ]);
        f.render_widget(Paragraph::new(auto_confirm_line), modal_chunks[3]);

        let footer_line = Line::from(vec![
            Span::styled(
                "  y / enter",
                Style::default()
                    .fg(COLOR_GREEN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" approve all  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "n / esc",
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" deny all", Style::default().fg(COLOR_MUTED)),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[5]);
    }
}
