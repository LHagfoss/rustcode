use std::io::{self, Write};
use std::thread;
use std::time::Duration;
use rand::seq::IndexedRandom;

pub fn show_spinner(running: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let messages = vec!["Thinking...", "Analyzing code...", "Refactoring...", "Checking logic..."];
    let spinner = vec!['|', '/', '-', '\\'];
    let mut i = 0;
    
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        let msg = messages.choose(&mut rand::rng()).unwrap();
        print!("\r{} {}", spinner[i % 4], msg);
        io::stdout().flush().unwrap();
        i += 1;
        thread::sleep(Duration::from_millis(200));
    }
    print!("\r ");
    io::stdout().flush().unwrap();
}

pub mod theme;
mod highlight;
mod modals;

use highlight::{highlight_diff_line, highlight_rust_line, pad_to_width};
use modals::{
    render_at_popup_menu, render_command_picker_modal, render_history_picker_modal,
    render_mcp_config_modal, render_model_picker_modal, render_popup_menu,
    render_tool_confirmation_modal, render_welcome_screen,
};
pub use modals::{PALETTE_ITEMS, PaletteItem};

use crate::app::{AppState, AppStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

/// Max visible rows in the slash-command popup; longer lists scroll internally.
const MAX_POPUP_ROWS: u16 = 10;

const COLOR_BG: Color = Color::Rgb(21, 23, 26);
const COLOR_PANEL: Color = Color::Rgb(26, 29, 32);
const COLOR_ELEMENT: Color = Color::Rgb(34, 38, 42);
const COLOR_TEXT: Color = Color::Rgb(240, 229, 222);
const COLOR_MUTED: Color = Color::Rgb(136, 146, 154);
const COLOR_PRIMARY: Color = Color::Rgb(236, 110, 93);
const COLOR_SECONDARY: Color = Color::Rgb(60, 88, 101);
const COLOR_GREEN: Color = Color::Rgb(127, 216, 143);
/// Uniform text-selection background — vibrant selection blue for high visibility.
const COLOR_SELECTION: Color = Color::Rgb(60, 95, 150);
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

/// Collapse pasted image markers (`![image](file://…)`) into compact
/// `[Image #N]` chips for display. The raw markers stay in the underlying
/// buffer / history so `parse_multimodal_content` can still attach the images
/// when the message is sent — this only affects what the user sees.
fn collapse_image_markers(text: &str) -> String {
    const MARK: &str = "![image](file://";
    if !text.contains(MARK) {
        return text.to_string();
    }
    let mut out = String::new();
    let mut rest = text;
    let mut n = 0;
    while let Some(start) = rest.find(MARK) {
        out.push_str(&rest[..start]);
        let after = &rest[start + MARK.len()..];
        if let Some(close) = after.find(')') {
            n += 1;
            out.push_str(&format!("[Image #{n}]"));
            rest = &after[close + 1..];
        } else {
            // Unclosed marker (e.g. mid-paste) — leave the remainder untouched.
            out.push_str(&rest[start..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn model_label(state: &AppState) -> String {
    // Only show the main (big) model — hide the small model entirely.
    state.config.default.big().to_string()
}

fn render_assistant_message<'a>(
    content: &'a str,
    response_time_ms: Option<u64>,
    model_name: &str,
    lines: &mut Vec<Line<'a>>,
    is_generating: bool,
    viewport_width: u16,
    show_picker: bool,
    thought_collapsed: bool,
    msg_index: Option<usize>,
    click_registry: &mut Vec<(usize, usize)>,
    is_copied_recently: bool,
) {
    let mut think_content = None;
    let mut main_content = content;

    if content.contains("<think>")
        && let Some(start_idx) = content.find("<think>") {
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

    if let Some(think) = think_content {
        let label = if let Some(ms) = response_time_ms {
            if ms >= 1000 {
                format!("Thinking ({:.1}s)", ms as f32 / 1000.0)
            } else {
                format!("Thinking ({}ms)", ms)
            }
        } else {
            "Thinking".to_string()
        };
        let toggle = if thought_collapsed { "+ " } else { "− " };
        if let Some(idx) = msg_index {
            click_registry.push((lines.len(), idx));
        }

        let first_line = think.lines().next().unwrap_or(&label);
        let preview = if first_line.len() > 65 {
            format!("{}...", &first_line[..65])
        } else {
            first_line.to_string()
        };

        lines.push(Line::from(vec![
            Span::styled(
                toggle,
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
            ),
            Span::styled(
                format!("{label}: {preview}"),
                get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
            ),
        ]));

        if !thought_collapsed {
            for raw_line in think.lines() {
                lines.push(Line::from(vec![
                    Span::styled(
                        "│ ",
                        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker),
                    ),
                    Span::styled(
                        raw_line,
                        get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                    ),
                ]));
            }
        }
        lines.push(Line::from(""));
    }

    if !main_content.trim().is_empty() || is_generating {
        let content_width = (viewport_width as usize).saturating_sub(10).max(10);
        let mut processed_lines: Vec<(bool, String)> = Vec::new();
        let mut in_code_block = false;

        for raw_line in main_content.lines() {
            let is_code_fence = raw_line.trim_start().starts_with("```");
            if is_code_fence {
                in_code_block = !in_code_block;
                processed_lines.push((true, raw_line.to_string()));
            } else if in_code_block {
                processed_lines.push((true, raw_line.to_string()));
            } else {
                if raw_line.trim().is_empty() {
                    processed_lines.push((false, String::new()));
                } else {
                    let mut current = String::new();
                    for word in raw_line.split_whitespace() {
                        if current.is_empty() {
                            current.push_str(word);
                        } else if current.width() + 1 + word.width() <= content_width {
                            current.push(' ');
                            current.push_str(word);
                        } else {
                            processed_lines.push((false, current));
                            current = word.to_string();
                        }
                    }
                    if !current.is_empty() {
                        processed_lines.push((false, current));
                    }
                }
            }
        }

        let mut i = 0;
        while i < processed_lines.len() {
            if processed_lines[i].0 {
                let line_str = &processed_lines[i].1;
                let is_code_fence = line_str.trim_start().starts_with("```");
                let mut spans = Vec::new();
                let code_content_width = (viewport_width as usize).saturating_sub(6);
                if is_code_fence {
                    let button_badge = if is_copied_recently { " 📋 [Copied!] " } else { " 📋 [Copy] " };
                    let button_color = if is_copied_recently { Color::Rgb(152, 195, 121) } else { COLOR_SECONDARY };
                    let fence_text = line_str.trim();
                    let left_len = fence_text.len();
                    let right_len = button_badge.len();
                    let pad_len = code_content_width.saturating_sub(left_len + right_len);

                    spans.push(Span::styled(
                        format!("{}{}", fence_text, " ".repeat(pad_len)),
                        get_themed_style(COLOR_MUTED, COLOR_ELEMENT, Modifier::empty(), show_picker),
                    ));
                    spans.push(Span::styled(
                        button_badge,
                        get_themed_style(button_color, COLOR_ELEMENT, Modifier::BOLD, show_picker),
                    ));
                    lines.push(Line::from(spans));
                } else if line_str.starts_with('+') || line_str.starts_with('-') || line_str.starts_with("@@") {
                    lines.push(highlight_diff_line(line_str, code_content_width, show_picker));
                } else {
                    let mut line_spans = highlight_rust_line(line_str, show_picker);
                    let current_width: usize = line_spans.iter().map(|s| s.content.width()).sum();
                    if current_width < code_content_width {
                        line_spans.push(Span::styled(
                            " ".repeat(code_content_width - current_width),
                            get_themed_style(COLOR_TEXT, COLOR_ELEMENT, Modifier::empty(), show_picker),
                        ));
                    }
                    spans.extend(line_spans);
                    lines.push(Line::from(spans));
                }
                i += 1;
            } else {
                // Gather contiguous normal text lines
                let mut normal_block = Vec::new();
                while i < processed_lines.len() && !processed_lines[i].0 {
                    normal_block.push(processed_lines[i].1.clone());
                    i += 1;
                }

                // Render the normal block as a single padded bubble card!
                // Top padding row
                lines.push(Line::from(vec![
                    Span::styled(
                        "▌ ",
                        get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::empty(), show_picker),
                    ),
                    Span::styled(
                        " ".repeat(content_width + 4),
                        get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                    ),
                ]));

                // Text rows
                for line_str in normal_block {
                    let mut spans = Vec::new();
                    
                    // Add 2 spaces left padding inside the bubble
                    spans.push(Span::styled(
                        "  ",
                        get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                    ));

                    let mut chars = line_str.chars().peekable();
                    let mut current = String::new();
                    let mut in_inline_code = false;
                    let mut in_bold = false;

                    while let Some(c) = chars.next() {
                        if c == '`' {
                            if !current.is_empty() {
                                let modifier = if in_bold { Modifier::BOLD } else { Modifier::empty() };
                                let style = if in_inline_code {
                                    get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                                } else {
                                    get_themed_style(COLOR_TEXT, COLOR_PANEL, modifier, show_picker)
                                };
                                spans.push(Span::styled(current.clone(), style));
                                current.clear();
                            }
                            in_inline_code = !in_inline_code;
                        } else if c == '*' && chars.peek() == Some(&'*') {
                            chars.next();
                            if !current.is_empty() {
                                let modifier = if in_bold { Modifier::BOLD } else { Modifier::empty() };
                                let style = if in_inline_code {
                                    get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                                } else {
                                    get_themed_style(COLOR_TEXT, COLOR_PANEL, modifier, show_picker)
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
                        let modifier = if in_bold { Modifier::BOLD } else { Modifier::empty() };
                        let style = if in_inline_code {
                            get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                        } else {
                            get_themed_style(COLOR_TEXT, COLOR_PANEL, modifier, show_picker)
                        };
                        spans.push(Span::styled(current, style));
                    }

                    // Pad to full content_width so the COLOR_PANEL background fills the block
                    let current_width: usize = spans.iter().map(|s| s.content.width()).sum::<usize>().saturating_sub(2);
                    if current_width < content_width {
                        spans.push(Span::styled(
                            " ".repeat(content_width - current_width),
                            get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                        ));
                    }

                    // Add 2 spaces right padding inside the bubble
                    spans.push(Span::styled(
                        "  ",
                        get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                    ));

                    let mut final_spans = vec![
                        Span::styled(
                            "▌ ",
                            get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::empty(), show_picker),
                        ),
                    ];
                    final_spans.extend(spans);
                    lines.push(Line::from(final_spans));
                }

                // Bottom padding row
                lines.push(Line::from(vec![
                    Span::styled(
                        "▌ ",
                        get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::empty(), show_picker),
                    ),
                    Span::styled(
                        " ".repeat(content_width + 4),
                        get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                    ),
                ]));
            }
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
    if state.status == AppStatus::Streaming
        && let Some(ref tracker) = state.stream_tracker {
            let (tps, _) = tracker.snapshot();
            return ("Tps: ".to_string(), format!("{:.1}", tps));
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
            let dist = (i as isize - pulse_center as isize).unsigned_abs();
            let level = 5_usize.saturating_sub(dist);
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

        if let Some(quota) = state.model_quota_remaining {
            let color = if quota > 50.0 {
                COLOR_PRIMARY
            } else if quota > 20.0 {
                Color::Yellow
            } else {
                Color::Red
            };
            right_spans.push(Span::styled("   Quota: ", get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker)));
            right_spans.push(Span::styled(format!("{:.0}%", quota), get_themed_style(color, COLOR_BG, Modifier::BOLD, show_picker)));
        }

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
        // Show pasted images as compact `[Image #N]` chips instead of the raw
        // `![image](file://…)` marker. The buffer itself is unchanged; the cursor
        // is remapped into this collapsed view so caret placement stays correct.
        let display_buffer = collapse_image_markers(&state.input_buffer);
        let mut styled_chars: Vec<(char, Style)> =
            display_buffer.chars().map(|c| (c, text_style)).collect();

        if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED, COLOR_PANEL, Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        let raw_prefix =
            &state.input_buffer[..state.cursor_position.min(state.input_buffer.len())];
        let cursor_char_index = collapse_image_markers(raw_prefix).chars().count();

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
    let (mode_label, mode_color) = match state.agent_mode {
        crate::config::AgentMode::Build => ("Build", COLOR_SECONDARY),
        crate::config::AgentMode::Plan => ("Plan", Color::Rgb(229, 192, 123)),
    };
    let build_line = Line::from(vec![
        Span::styled(
            mode_label,
            get_themed_style(mode_color, COLOR_PANEL, Modifier::BOLD, show_picker),
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
    state.chat_area = Some(inner_area);

    let mut lines: Vec<Line> = Vec::new();

    let mut thought_clicks: Vec<(usize, usize)> = Vec::new();

    for (msg_idx, msg) in state.history.iter().enumerate() {
        if msg.role == "system" {
            if msg.content.contains("🏁") || msg.content.contains("Goal Accomplished") {
                lines.push(Line::from(vec![
                    Span::styled(
                        " 🏁 ",
                        Style::default().fg(Color::Rgb(152, 195, 121)).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {} ", msg.content),
                        Style::default().fg(Color::Rgb(152, 195, 121)).bg(COLOR_PANEL).add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(""));
                continue;
            }

            let collapsed = !state.expanded_thoughts.contains(&msg_idx);
            let lower = msg.content.to_lowercase();
            let is_prompt_opt = lower.contains("prompt optimized") || lower.contains("activated automatically");
            let is_info_or_help = msg.content.starts_with("Available Commands:") || lower.contains("copied code/reply") || lower.contains("resumed session") || lower.contains("quota status");
            let is_warning = !is_prompt_opt && !is_info_or_help && (lower.contains("warning") || lower.contains("loop") || lower.contains("abort") || lower.contains("error"));
            let label = if is_warning { "Warning" } else { "Notice" };
            let theme_color = if is_warning {
                Color::Rgb(229, 192, 123)
            } else {
                Color::Rgb(100, 175, 235)
            };

            let first_line = msg.content.lines().next().unwrap_or(label);
            let preview = if first_line.len() > 65 {
                format!("{}...", &first_line[..65])
            } else {
                first_line.to_string()
            };

            let toggle = if collapsed { "+ " } else { "− " };
            thought_clicks.push((lines.len(), msg_idx));

            lines.push(Line::from(vec![
                Span::styled(
                    toggle,
                    get_themed_style(
                        theme_color,
                        COLOR_BG,
                        Modifier::BOLD,
                        show_picker,
                    ),
                ),
                Span::styled(
                    format!("{label}: {preview}"),
                    get_themed_style(
                        theme_color,
                        COLOR_BG,
                        Modifier::BOLD,
                        show_picker,
                    ),
                ),
            ]));

            if !collapsed {
                for raw_line in msg.content.lines() {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "│ ",
                            get_themed_style(
                                theme_color,
                                COLOR_BG,
                                Modifier::BOLD,
                                show_picker,
                            ),
                        ),
                        Span::styled(
                            raw_line,
                            get_themed_style(
                                theme_color,
                                COLOR_BG,
                                Modifier::empty(),
                                show_picker,
                            ),
                        ),
                    ]));
                }
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

            if let Some(ref diff) = msg.diff {
                let code_content_width = (inner_area.width as usize).saturating_sub(6);
                for diff_line in diff.lines() {
                    lines.push(highlight_diff_line(diff_line, code_content_width, show_picker));
                }
            }
            lines.push(Line::from(""));
        } else if msg.role == "user" {
            lines.push(Line::from(""));
            // Account for "▌ " prefix (2 characters) plus internal bubble padding (4 characters) plus margins (2 characters)
            let content_width = (inner_area.width as usize).saturating_sub(8);
            let display_content = collapse_image_markers(&msg.content);
            let mut wrapped_lines = Vec::new();
            for raw_line in display_content.lines() {
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

            // Top padding row
            lines.push(Line::from(vec![
                Span::styled(
                    "▌ ",
                    get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::empty(), show_picker),
                ),
                Span::styled(
                    " ".repeat(content_width + 4),
                    get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                ),
            ]));

            for line_str in wrapped_lines {
                let padded_text = pad_to_width(&line_str, content_width);
                lines.push(Line::from(vec![
                    Span::styled(
                        "▌ ",
                        get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::empty(), show_picker),
                    ),
                    Span::styled(
                        format!("  {padded_text}  "),
                        get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                    ),
                ]));
            }

            // Bottom padding row
            lines.push(Line::from(vec![
                Span::styled(
                    "▌ ",
                    get_themed_style(COLOR_SECONDARY, COLOR_BG, Modifier::empty(), show_picker),
                ),
                Span::styled(
                    " ".repeat(content_width + 4),
                    get_themed_style(COLOR_TEXT, COLOR_PANEL, Modifier::empty(), show_picker),
                ),
            ]));
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
            let is_copied_recently = state.last_copy_time.is_some_and(|t| t.elapsed().as_secs() < 2);
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
                is_copied_recently,
            );
            lines.push(Line::from(""));
        }
    }

    if state.status == AppStatus::Streaming || state.status == AppStatus::Queued {
        let label = if let Some(tool_name) = state.running_tools.first() {
            format!("Executing {tool_name}")
        } else {
            match state.agent_mode {
                crate::config::AgentMode::Build => "Build".to_string(),
                crate::config::AgentMode::Plan => "Plan".to_string(),
            }
        };

        if state.current_response.is_empty() {
            let random_statuses = [
                "Thinking...",
                "Analyzing code...",
                "Consulting the oracle...",
                "Brewing coffee...",
                "Refactoring reality...",
                "Checking documentation...",
                "Optimizing loops...",
                "Debugging the universe...",
            ];
            let elapsed_secs = state.generation_start_time.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            let status_msg = random_statuses[(elapsed_secs as usize / 3) % random_statuses.len()];

            let mut status_spans: Vec<Span> = vec![
                Span::styled(
                    format!("{status_msg} "),
                    get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    " · ",
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
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

            if let Some(t) = state.generation_start_time {
                let secs = t.elapsed().as_secs_f32();
                status_spans.push(Span::styled(
                    format!(" · {:.1}s", secs),
                    get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker),
                ));
            }

            lines.push(Line::from(status_spans));
        } else {
            let is_copied_recently = state.last_copy_time.is_some_and(|t| t.elapsed().as_secs() < 2);

            // Check if current streaming response is a tool call syntax
            let parsed_tool = crate::tools::parse_tool_call(&state.current_response, state.config.tool_protocol);
            let is_tool_syntax = crate::tools::is_tool_call_start(&state.current_response);

            let should_hide_stream = match parsed_tool {
                Some((ref tool_name, _)) => !crate::tools::is_code_editing_tool(tool_name),
                None => is_tool_syntax,
            };

            if should_hide_stream {
                let random_statuses = [
                    "Preparing tool action...",
                    "Analyzing query...",
                    "Gathering context...",
                    "Checking codebase...",
                ];
                let elapsed_secs = state.generation_start_time.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                let status_msg = random_statuses[(elapsed_secs as usize / 2) % random_statuses.len()];

                let tool_label = parsed_tool.map(|(n, _)| format!("Executing {n}...")).unwrap_or_else(|| status_msg.to_string());

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{tool_label} "),
                        get_themed_style(COLOR_TIP, COLOR_BG, Modifier::BOLD, show_picker),
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
                    false,
                    None,
                    &mut thought_clicks,
                    is_copied_recently,
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

    let _conv = chunks[0];
    let _view_h = inner_area.height;
    let _content_h = total_wrapped_lines.max(1);

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
            // Cap to the rows available above the prompt so a long command list
            // scrolls inside the popup instead of painting over the input.
            let popup_height = (filtered_cmds.len() as u16)
                .min(MAX_POPUP_ROWS)
                .min(prompt_box_area.y);
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

        let (_, at_query) = crate::app::get_at_word_query(&state.input_buffer, state.cursor_position)
            .unwrap_or((0, String::new()));
        let at_files = if !at_query.is_empty() || state.input_buffer[..state.cursor_position.min(state.input_buffer.len())].ends_with('@') {
            crate::app::list_project_file_paths(&at_query)
        } else {
            Vec::new()
        };

        if !filtered_cmds.is_empty() {
            let input_inner = chunks[1].inner(input_margin);
            // Cap to the rows available above the input box so a long command
            // list scrolls inside the popup instead of overlapping the prompt.
            let popup_height = (filtered_cmds.len() as u16)
                .min(MAX_POPUP_ROWS)
                .min(chunks[1].y);
            let popup_y = chunks[1].y.saturating_sub(popup_height);
            let popup_area =
                ratatui::layout::Rect::new(input_inner.x, popup_y, input_inner.width, popup_height);
            render_popup_menu(f, state, &filtered_cmds, popup_area);
        } else if !at_files.is_empty() {
            let input_inner = chunks[1].inner(input_margin);
            let popup_height = at_files.len().min(8) as u16;
            let popup_y = chunks[1].y.saturating_sub(popup_height);
            let popup_area =
                ratatui::layout::Rect::new(input_inner.x, popup_y, input_inner.width, popup_height);
            render_at_popup_menu(f, state, &at_files, popup_area);
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
            highlight_selection(f, start, end, state.chat_area, state.scroll_row);
            let text = extract_selection(f.buffer_mut(), start, end, state.chat_area, state.scroll_row);
            if !text.is_empty() {
                state.selected_text = Some(text);
            }
        } else {
            state.selected_text = None;
        }
    }
}

fn highlight_selection(
    f: &mut Frame,
    start: (u16, u16),
    end: (u16, u16),
    chat_area: Option<ratatui::layout::Rect>,
    scroll_row: u16,
) {
    let screen_start_y = start.1.saturating_sub(scroll_row);
    let screen_end_y = end.1.saturating_sub(scroll_row);

    let (screen_start, screen_end) = if (screen_start_y, start.0) <= (screen_end_y, end.0) {
        ((start.0, screen_start_y), (end.0, screen_end_y))
    } else {
        ((end.0, screen_end_y), (start.0, screen_start_y))
    };

    let buf = f.buffer_mut();
    let area = buf.area;
    let width = area.width;
    if width == 0 {
        return;
    }

    let (min_row, max_row, min_col, max_col) = if let Some(ca) = chat_area {
        (ca.y, ca.y + ca.height.saturating_sub(1), ca.x + 2, ca.x + ca.width.saturating_sub(2))
    } else {
        (area.y + 1, area.y + area.height.saturating_sub(2), area.x + 2, area.x + width.saturating_sub(2))
    };

    // If the selection is completely scrolled off-screen, don't draw anything
    if screen_start.1 > max_row || screen_end.1 < min_row {
        return;
    }

    let start_row = screen_start.1.max(min_row).min(max_row);
    let end_row = screen_end.1.max(min_row).min(max_row);

    for row in start_row..=end_row {
        let mut last_content_col = None;
        for col in (min_col..=max_col).rev() {
            if let Some(cell) = buf.cell(ratatui::layout::Position::new(col, row)) {
                let sym = cell.symbol();
                if !sym.trim().is_empty() && sym != "│" && sym != "░" && sym != "█" && sym != "▌" {
                    last_content_col = Some(col);
                    break;
                }
            }
        }

        // If this row has no text content at all (empty row, margin, or empty space below chat), skip it entirely!
        let last_col = match last_content_col {
            Some(c) => c,
            None => continue,
        };

        let col_from = if row == start_row { screen_start.0.max(min_col).min(max_col) } else { min_col };
        let is_last_row = row == end_row;
        let is_single_line = start_row == end_row;
        let col_to = if is_last_row && is_single_line {
            screen_end.0.max(min_col).min(max_col).min(last_col)
        } else {
            last_col
        };

        if col_from > col_to {
            continue;
        }

        for col in col_from..=col_to {
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
    chat_area: Option<ratatui::layout::Rect>,
    scroll_row: u16,
) -> String {
    let screen_start_y = start.1.saturating_sub(scroll_row);
    let screen_end_y = end.1.saturating_sub(scroll_row);

    let (screen_start, screen_end) = if (screen_start_y, start.0) <= (screen_end_y, end.0) {
        ((start.0, screen_start_y), (end.0, screen_end_y))
    } else {
        ((end.0, screen_end_y), (start.0, screen_start_y))
    };

    let area = buf.area;
    let width = area.width;
    if width == 0 {
        return String::new();
    }

    let (min_row, max_row, min_col, max_col) = if let Some(ca) = chat_area {
        (ca.y, ca.y + ca.height.saturating_sub(1), ca.x + 2, ca.x + ca.width.saturating_sub(2))
    } else {
        (area.y + 1, area.y + area.height.saturating_sub(2), area.x + 2, area.x + width.saturating_sub(2))
    };

    if screen_start.1 > max_row || screen_end.1 < min_row {
        return String::new();
    }

    let start_row = screen_start.1.max(min_row).min(max_row);
    let end_row = screen_end.1.max(min_row).min(max_row);

    let mut lines_out = Vec::new();
    for row in start_row..=end_row {
        let col_from = if row == start_row { screen_start.0.max(min_col).min(max_col) } else { min_col };
        let col_to = if row == end_row {
            screen_end.0.max(min_col).min(max_col)
        } else {
            max_col
        };
        let mut line = String::new();
        for col in col_from..=col_to {
            if let Some(cell) = buf.cell(ratatui::layout::Position::new(col, row)) {
                let sym = cell.symbol();
                let filtered: String = sym
                    .chars()
                    .filter(|&c| c != '\0' && !c.is_control() && c != '▌')
                    .collect();
                line.push_str(&filtered);
            }
        }
        let mut clean = line.trim_end();

        // Strip leading UI border & header prefixes
        for prefix in &[
            "│ ", "│", "▌ ", "▌", "⚙ ", "⚙", "→ ", "→", "🦀 ", "🦀", "🌐 ", "🌐",
            "+ Warning: ", "Warning: ", "+ Thought: ", "Thought: ", "Goal: ",
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

        let trimmed = clean.trim();
        if !trimmed.is_empty() {
            lines_out.push(trimmed.to_string());
        }
    }
    let res = lines_out.join("\n");
    dbg_log!("[SELECTION] Extracted {} chars from selection range start={:?} end={:?}: {:?}", res.len(), start, end, res);
    res
}


#[cfg(test)]
mod tests {
    use super::collapse_image_markers;

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
}
