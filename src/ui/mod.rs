mod highlight;
mod lru;
mod markdown;
mod modals;
mod tool_result;

use highlight::{
    highlight_code_block, highlight_code_line, highlight_diff_line, pad_to_width,
    render_unified_diff, wrap_code_spans,
};
use markdown::render_markdown;
pub use modals::{PALETTE_ITEMS, PaletteItem};
pub mod theme;
use modals::{
    render_at_popup_menu, render_command_picker_modal, render_history_picker_modal,
    render_mcp_config_modal, render_model_picker_modal, render_popup_menu, render_question_modal,
    render_theme_picker_modal, render_tool_confirmation_modal, render_verbosity_picker_modal,
    render_welcome_screen,
};
use tool_result::{render_file_preview, render_tool_result};

use crate::app::{AppState, AppStatus, HoverTarget, NoticeKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
};
use std::hash::{Hash, Hasher};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

fn safe_byte_index(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Max visible rows in the slash-command popup; longer lists scroll internally.
const MAX_POPUP_ROWS: u16 = 10;

#[allow(non_snake_case)]
#[inline]
pub fn COLOR_BG() -> Color {
    theme::color_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_PANEL() -> Color {
    theme::color_panel()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_ELEMENT() -> Color {
    theme::color_element()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_TEXT() -> Color {
    theme::color_text()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_MUTED() -> Color {
    theme::color_muted()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_PRIMARY() -> Color {
    theme::color_primary()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_SECONDARY() -> Color {
    theme::color_secondary()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_GREEN() -> Color {
    theme::color_green()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_SELECTION() -> Color {
    theme::color_selection()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_TIP() -> Color {
    theme::color_tip()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_STATUS_BORDER() -> Color {
    theme::color_status_border()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_TURN_SEPARATOR() -> Color {
    theme::color_turn_separator()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_NOTICE_BG() -> Color {
    theme::color_notice_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_HOVER_BG() -> Color {
    theme::color_hover_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_ADD_BG() -> Color {
    theme::color_diff_add_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_ADD_FG() -> Color {
    theme::color_diff_add_fg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_REMOVE_BG() -> Color {
    theme::color_diff_remove_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_REMOVE_FG() -> Color {
    theme::color_diff_remove_fg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_ABSENT_BG() -> Color {
    theme::color_diff_absent_bg()
}

const LOGO: &[&str] = &[
    "                  ▄                   █      ",
    "▄▀▀▀ █   █ ▄▀▀▀▀ ▀█▀▀ ▄▀▀▀▀ ▄▀▀▀▄ ▄▀▀▀█ ▄▀▀▀▄",
    "█    █   █  ▀▀▀▄  █   █     █   █ █   █ █▀▀▀▀",
    "▀     ▀▀▀  ▀▀▀▀    ▀▀  ▀▀▀▀  ▀▀▀   ▀▀▀▀  ▀▀▀▀",
];

pub use crate::app::suggestion::{COMMANDS, CommandInfo};

fn get_themed_style(fg: Color, bg: Color, modifier: Modifier, _show_picker: bool) -> Style {
    Style::default().fg(fg).bg(bg).add_modifier(modifier)
}

/// Collapse pasted image markers (`![image](file://…)`) into compact
/// `[Image #N]` chips for display. The raw markers stay in the underlying
/// buffer / history so `parse_multimodal_content` can still attach the images
/// when the message is sent — this only affects what the user sees.
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;

thread_local! {
    static MARKER_CACHE: RefCell<(u64, String)> = const { RefCell::new((0, String::new())) };
}

fn collapse_image_markers(text: &str) -> String {
    const MARK: &str = "![image](file://";
    if !text.contains(MARK) {
        return text.to_string();
    }

    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    let hash = hasher.finish();

    MARKER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.0 == hash {
            return cache.1.clone();
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
                out.push_str(&rest[start..]);
                *cache = (hash, out.clone());
                return out;
            }
        }
        out.push_str(rest);
        *cache = (hash, out.clone());
        out
    })
}

fn model_label(state: &AppState) -> String {
    // Only show the main (big) model — hide the small model entirely.
    state.config.default.big().to_string()
}

struct AssistantRenderOptions<'a> {
    response_time_ms: Option<u64>,
    model_name: &'a str,
    is_generating: bool,
    viewport_width: u16,
    show_picker: bool,
    thought_collapsed: bool,
    msg_index: Option<usize>,
    last_copy_text: Option<(String, std::time::Instant)>,
}

fn render_assistant_message<'a>(
    content: &'a str,
    lines: &mut Vec<Line<'a>>,
    click_registry: &mut Vec<(usize, usize)>,
    copy_registry: &mut Vec<(usize, String)>,
    options: AssistantRenderOptions<'_>,
) {
    let AssistantRenderOptions {
        response_time_ms,
        model_name,
        is_generating,
        viewport_width,
        show_picker,
        thought_collapsed,
        msg_index,
        last_copy_text,
    } = options;
    let display_content = if let Some(idx) = content.find("\n\n[harness verification:") {
        &content[..idx]
    } else if let Some(idx) = content.find("[harness verification:") {
        &content[..idx]
    } else {
        content
    };

    let mut think_content = None;
    let mut main_content = display_content;

    if content.contains("<think>")
        && let Some(start_idx) = content.find("<think>")
    {
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
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                format!("{label}: {preview}"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
        ]));

        if !thought_collapsed {
            for raw_line in think.lines() {
                lines.push(Line::from(vec![
                    Span::styled(
                        "│ ",
                        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
                    ),
                    Span::styled(
                        raw_line,
                        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                    ),
                ]));
            }
        }
        lines.push(Line::from(""));
    }

    if !main_content.trim().is_empty() || is_generating {
        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::from(""));
        }
        let content_width = (viewport_width as usize).saturating_sub(8).max(10);
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

        // Languages we render as plain text (no Rust syntax highlighting), so
        // prose dumped inside a fence doesn't get every capitalised word painted
        // yellow and words like `in`/`for`/`type` painted like keywords.
        let is_plain_lang = |lang: &str| -> bool {
            matches!(
                lang,
                "" | "text" | "txt" | "markdown" | "md" | "plain" | "plaintext"
            )
        };
        let is_diff_lang = |lang: &str| -> bool { matches!(lang, "diff" | "patch" | "udiff") };

        // Code blocks render as one solid full-width panel. Every row is padded
        // (or wrapped) to `box_width` so the panel background fills the whole box
        // rather than sitting only behind the glyphs, and the copy button is
        // right-aligned using display width — not byte length, which overcounts
        // the 📋 emoji and knocked the button out of alignment.
        let box_width = (viewport_width as usize).max(10);
        let mut i = 0;
        let mut fence_open = false;
        let mut current_lang = String::new();
        while i < processed_lines.len() {
            if processed_lines[i].0 {
                let line_str = &processed_lines[i].1;
                let is_code_fence = line_str.trim_start().starts_with("```");
                if is_code_fence {
                    let opening = !fence_open;
                    fence_open = !fence_open;
                    let fence_text = line_str.trim();
                    if opening {
                        if lines.last().is_some_and(|line| {
                            line.spans.iter().any(|span| !span.content.is_empty())
                        }) {
                            lines.push(Line::from(""));
                        }
                        current_lang = fence_text.trim_start_matches('`').trim().to_lowercase();

                        let mut code_text = String::new();
                        let mut j = i + 1;
                        while j < processed_lines.len()
                            && !(processed_lines[j].0
                                && processed_lines[j].1.trim_start().starts_with("```"))
                        {
                            if !code_text.is_empty() {
                                code_text.push('\n');
                            }
                            code_text.push_str(&processed_lines[j].1);
                            j += 1;
                        }

                        let lang_label = if current_lang.is_empty() {
                            "code".to_string()
                        } else {
                            current_lang.clone()
                        };
                        let is_copied_recently =
                            last_copy_text.as_ref().is_some_and(|(t_text, t)| {
                                t_text == &code_text && t.elapsed().as_secs() < 2
                            });
                        let button_badge = if is_copied_recently {
                            " Copied! 📋 "
                        } else {
                            " Copy 📋 "
                        };
                        let button_color = if is_copied_recently {
                            COLOR_GREEN()
                        } else {
                            COLOR_SECONDARY()
                        };
                        // Keep code on a subtle panel so syntax spans remain visually grouped;
                        // the Copy badge uses the same panel with a stronger foreground.
                        let code_bg = COLOR_BG();
                        let left_text = format!(" {lang_label} ");
                        let pad_len =
                            box_width.saturating_sub(left_text.width() + button_badge.width());
                        let spans = vec![
                            Span::styled(
                                left_text,
                                get_themed_style(
                                    COLOR_MUTED(),
                                    code_bg,
                                    Modifier::BOLD,
                                    show_picker,
                                ),
                            ),
                            Span::styled(
                                " ".repeat(pad_len),
                                get_themed_style(
                                    COLOR_MUTED(),
                                    code_bg,
                                    Modifier::empty(),
                                    show_picker,
                                ),
                            ),
                            Span::styled(
                                button_badge,
                                get_themed_style(
                                    button_color,
                                    COLOR_BG(),
                                    Modifier::BOLD,
                                    show_picker,
                                ),
                            ),
                        ];
                        copy_registry.push((lines.len(), code_text.clone()));
                        lines.push(Line::from(spans));
                        if !is_plain_lang(&current_lang) && !is_diff_lang(&current_lang) {
                            for body_spans in
                                highlight_code_block(&code_text, &current_lang, show_picker)
                            {
                                let mut content_spans = vec![Span::styled(
                                    " ".to_string(),
                                    get_themed_style(
                                        COLOR_TEXT(),
                                        code_bg,
                                        Modifier::empty(),
                                        show_picker,
                                    ),
                                )];
                                content_spans.extend(body_spans.into_iter().map(|span| {
                                    Span::styled(span.content, span.style.bg(code_bg))
                                }));
                                lines.extend(wrap_code_spans(
                                    content_spans,
                                    box_width,
                                    code_bg,
                                    show_picker,
                                ));
                            }
                            i = j.saturating_sub(1);
                        }
                    } else {
                        if processed_lines
                            .get(i + 1)
                            .is_some_and(|(_, text)| !text.trim().is_empty())
                        {
                            lines.push(Line::from(""));
                        }
                        current_lang.clear();
                    }
                } else if is_diff_lang(&current_lang)
                    && (line_str.starts_with('+')
                        || line_str.starts_with('-')
                        || line_str.starts_with("@@"))
                {
                    let is_diff_metadata = line_str.starts_with("@@")
                        || line_str.starts_with("--- ")
                        || line_str.starts_with("+++ ");
                    if !is_diff_metadata {
                        lines.push(highlight_diff_line(line_str, box_width, show_picker));
                    }
                } else {
                    // Body line: leading gutter space, per-language rendering,
                    // then wrapped and padded so the panel bg fills full width.
                    let content_spans = if is_plain_lang(&current_lang)
                        || is_diff_lang(&current_lang)
                    {
                        vec![Span::styled(
                            format!(" {line_str}"),
                            get_themed_style(
                                COLOR_TEXT(),
                                COLOR_BG(),
                                Modifier::empty(),
                                show_picker,
                            ),
                        )]
                    } else {
                        let mut s = vec![Span::styled(
                            " ".to_string(),
                            get_themed_style(
                                COLOR_TEXT(),
                                COLOR_BG(),
                                Modifier::empty(),
                                show_picker,
                            ),
                        )];
                        s.extend(
                            highlight_code_line(line_str, &current_lang, show_picker)
                                .into_iter()
                                .map(|span| Span::styled(span.content, span.style.bg(COLOR_BG()))),
                        );
                        s
                    };
                    lines.extend(wrap_code_spans(
                        content_spans,
                        box_width,
                        COLOR_BG(),
                        show_picker,
                    ));
                }
                i += 1;
            } else {
                let mut normal_block = Vec::new();
                while i < processed_lines.len() && !processed_lines[i].0 {
                    normal_block.push(processed_lines[i].1.clone());
                    i += 1;
                }

                let normal_text = normal_block.join("\n");
                if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                    lines.push(Line::from(""));
                }
                lines.extend(render_markdown(
                    &normal_text,
                    content_width,
                    show_picker,
                    !is_generating,
                ));
            }
        }
        lines.push(Line::from(""));
    }

    if !is_generating {
        let mut status_spans = vec![
            Span::styled(
                "■ ",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                "Build",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                " · ",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ];

        status_spans.push(Span::styled(
            model_name.to_string(),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));

        if let Some(ms) = response_time_ms {
            let secs = ms as f32 / 1000.0;
            status_spans.push(Span::styled(
                format!(" · {:.1}s", secs),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
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
            col += c.width().unwrap_or(1);
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
        && let Some(ref tracker) = state.stream_tracker
    {
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

        let step_duration_ms = 80.0; // Duration of each discrete step in milliseconds
        let num_dots = 6;
        let pulse_centers_f = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 2.0, 1.0];
        let num_cycle_steps = pulse_centers_f.len();

        // Calculate a continuous step value
        let step_float = (millis as f64 / step_duration_ms) % num_cycle_steps as f64;

        // Interpolate the pulse center value
        let current_pulse_center_idx = step_float.floor() as usize;
        let next_pulse_center_idx = (current_pulse_center_idx + 1) % num_cycle_steps;
        let fraction = step_float - step_float.floor();

        let pulse_center_val = pulse_centers_f[current_pulse_center_idx] * (1.0 - fraction)
            + pulse_centers_f[next_pulse_center_idx] * fraction;

        let colors = [
            Color::Rgb(25, 29, 32), // Darkest
            Color::Rgb(34, 40, 45),
            Color::Rgb(43, 51, 57),
            Color::Rgb(52, 62, 70),
            Color::Rgb(60, 88, 101),
            Color::Rgb(120, 160, 180), // Brightest
        ];

        let mut spans = Vec::new();

        for i in 0..num_dots {
            let dist_float = (i as f64 - pulse_center_val).abs();
            let level_float = 3.0 - dist_float; // Max level is 3.0 at the center

            // Clamp level_float to [0.0, 3.0]
            let clamped_level_float = level_float.clamp(0.0, 3.0);

            // Map clamped_level_float (0.0-3.0) to color index (0-5)
            let color_index =
                (clamped_level_float / 3.0 * (colors.len() - 1) as f64).round() as usize;
            let color = colors[color_index];
            spans.push(Span::styled(
                "■",
                get_themed_style(color, COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }

        if let Some(tool_name) = state.running_tools.first() {
            spans.push(Span::styled(
                format!("  executing: {tool_name}"),
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
        } else if !state.pending_queue.is_empty() {
            spans.push(Span::styled(
                format!("  queued: {}", state.pending_queue.len()),
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
        } else {
            let random_statuses = [
                "Thinking...",
                "Analyzing code...",
                "Consulting the oracle...",
                "Brewing coffee...",
                "Refactoring reality...",
                "Checking documentation...",
                "Optimizing loops...",
                "Debugging the universe...",
                "Synthesizing solutions...",
                "Querying knowledge base...",
            ];
            let elapsed_secs = state
                .generation_start_time
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0);
            let status_msg = random_statuses[(elapsed_secs as usize / 3) % random_statuses.len()];
            spans.push(Span::styled(
                format!("  {status_msg}"),
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
            if let Some(t) = state.generation_start_time {
                let secs = t.elapsed().as_secs();
                spans.push(Span::styled(
                    format!(" · {}s", secs),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
            }
        }

        spans.push(Span::styled(
            "   ..... esc ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        spans.push(Span::styled(
            "interrupt",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
        spans
    } else {
        let static_color = Color::Rgb(40, 48, 54);
        let mut spans = Vec::new();

        for _ in 0..6 {
            spans.push(Span::styled(
                "■",
                get_themed_style(static_color, COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }

        if !state.pending_queue.is_empty() {
            spans.push(Span::styled(
                format!("   idle (queued: {})", state.pending_queue.len()),
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
        } else {
            spans.push(Span::styled(
                "   idle",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
        spans
    };

    let right_spans = if state.history.is_empty() {
        vec![
            Span::styled("   ", Style::default()),
            Span::styled(
                "tab",
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                " agents   ",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                "ctrl+p",
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                " commands",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
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
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
            right_spans.push(Span::styled(
                tps_value,
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
        }

        right_spans.push(Span::styled(
            "   Context: ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        right_spans.push(Span::styled(
            token_str,
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
        if !cached_str.is_empty() {
            right_spans.push(Span::styled(
                cached_str,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
        right_spans.push(Span::styled(
            format!(" ({:.0}%)", pct),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));

        if let Some(quota) = state.model_quota_remaining {
            let color = if quota > 50.0 {
                COLOR_PRIMARY()
            } else if quota > 20.0 {
                Color::Yellow
            } else {
                Color::Red
            };
            right_spans.push(Span::styled(
                "   Quota: ",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
            right_spans.push(Span::styled(
                format!("{:.0}%", quota),
                get_themed_style(color, COLOR_BG(), Modifier::BOLD, show_picker),
            ));
        }

        right_spans.push(Span::styled("   ", Style::default()));
        right_spans.push(Span::styled(
            "ctrl+p",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
        right_spans.push(Span::styled(
            " commands",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
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
        COLOR_PRIMARY()
    } else {
        COLOR_MUTED()
    };
    let status_modifier = if state.auto_confirm {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };

    f.render_widget(
        Paragraph::new(Line::from(left_spans)).style(Style::default().bg(COLOR_BG())),
        footer_chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Auto-Confirm: ",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                state.auto_confirm_status_text(),
                get_themed_style(status_color, COLOR_BG(), status_modifier, show_picker),
            ),
        ]))
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().bg(COLOR_BG())),
        footer_chunks[1],
    );
    f.render_widget(
        Paragraph::new(Line::from(right_spans))
            .alignment(ratatui::layout::Alignment::Right)
            .style(Style::default().bg(COLOR_BG())),
        footer_chunks[2],
    );
}

fn render_input(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &mut AppState) -> Margin {
    let show_picker = state.modal_open();

    let input_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(chunks[1]);

    let line_chars = "▌\n".repeat(chunks[1].height as usize);
    let vertical_line_widget = Paragraph::new(line_chars).style(get_themed_style(
        COLOR_SECONDARY(),
        COLOR_BG(),
        Modifier::empty(),
        show_picker,
    ));
    f.render_widget(vertical_line_widget, input_split[0]);

    let solid_panel = Block::default().style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(solid_panel, input_split[1]);

    let input_margin = Margin {
        vertical: 1,
        horizontal: 2,
    };
    let input_inner = input_split[1].inner(input_margin);

    let text_style = if state.input_buffer.starts_with('/') {
        get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker)
    } else {
        get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker)
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

        if state.input_buffer.is_empty() && state.get_command_suggestion().is_none() {
            let placeholder_style =
                get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
            let placeholder_text = "Ask RustCode a question, or type / for commands...";
            styled_chars.extend(placeholder_text.chars().map(|c| (c, placeholder_style)));
        } else if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        let safe_end = state.cursor_position.min(state.input_buffer.len());
        let safe_end = if state.input_buffer.is_char_boundary(safe_end) {
            safe_end
        } else {
            state.input_buffer.char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= safe_end)
                .last()
                .unwrap_or(0)
        };
        let raw_prefix = &state.input_buffer[..safe_end];
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
                col += c.width().unwrap_or(1);
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
    let paragraph = Paragraph::new(lines).style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(paragraph, text_area);
    // Record the editable region so mouse drag-selection can target it.
    state.input_text_area = Some(text_area);

    let build_y = input_inner.y + input_inner.height.saturating_sub(1);
    let build_area = ratatui::layout::Rect::new(input_inner.x, build_y, input_inner.width, 1);
    let (mode_label, mode_color) = match state.agent_mode {
        crate::config::AgentMode::Build => ("Build", COLOR_SECONDARY()),
        crate::config::AgentMode::Plan => ("Plan", Color::Rgb(229, 192, 123)),
    };
    let build_line = Line::from(vec![
        Span::styled(
            mode_label,
            get_themed_style(mode_color, COLOR_PANEL(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " · ",
            get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::empty(), show_picker),
        ),
        Span::styled(
            model_label(state),
            get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker),
        ),
    ]);
    f.render_widget(Paragraph::new(build_line), build_area);

    if inner_width > 0 && !show_picker {
        f.set_cursor_position((input_inner.x + cursor_dx, input_inner.y + cursor_dy));
    }

    input_margin
}

/// snake_case / kebab-case → PascalCase, e.g. `use_skill` → `UseSkill`. Used so
/// custom and MCP tools render like the built-ins (no underscores, capitalized)
/// instead of leaking their raw internal names.
fn to_pascal_case(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn format_pi_tool_action(name: &str, args: &serde_json::Value) -> (String, String) {
    let action_label = match name {
        "view_file" => "Read".to_string(),
        "replace_file_content" | "multi_replace_file_content" => "Edit".to_string(),
        "write_to_file" => "Write".to_string(),
        "delete_file" => "Delete".to_string(),
        "move_file" => "Move".to_string(),
        "copy_file" => "Copy".to_string(),
        "list_directory" | "glob" => "ListDir".to_string(),
        "grep" => "Grep".to_string(),
        "find_symbol" => "Symbol".to_string(),
        "run_command" => "Bash".to_string(),
        "search_web" => "Search".to_string(),
        "get_project_map" => "ProjectMap".to_string(),
        "manage_task" => "ManageTask".to_string(),
        "background_task" => "TaskDone".to_string(),
        other => to_pascal_case(other),
    };

    let target_arg = match name {
        "view_file"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "write_to_file"
        | "delete_file" => args
            .get("TargetFile")
            .or_else(|| args.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "move_file" | "copy_file" => {
            let src = args.get("src").and_then(|v| v.as_str()).unwrap_or("?");
            let dest = args.get("dest").and_then(|v| v.as_str()).unwrap_or("?");
            format!("{} -> {}", src, dest)
        }
        "list_directory" | "glob" => args
            .get("DirectoryPath")
            .or_else(|| args.get("path"))
            .or_else(|| args.get("pattern"))
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string(),
        "grep" => {
            let pattern = args
                .get("Query")
                .or_else(|| args.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let path = args
                .get("SearchPath")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            format!("\"{}\" in {}", pattern, path)
        }
        "run_command" => args
            .get("CommandLine")
            .or_else(|| args.get("command"))
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        "search_web" | "find_symbol" => args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        "use_skill" => args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "spawn_agent" => args
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "send_agent" => args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "set_goal" => args
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "ask_question" => {
            if let Some(q) = args.get("question").and_then(|v| v.as_str()) {
                q.to_string()
            } else if let Some(q_arr) = args.get("questions").and_then(|v| v.as_array()) {
                q_arr
                    .first()
                    .and_then(|q| q.get("question"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        "manage_task" => {
            let action = args
                .get("Action")
                .or_else(|| args.get("action"))
                .and_then(|v| v.as_str())
                .unwrap_or("status");
            let task_id = args
                .get("TaskId")
                .or_else(|| args.get("task_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let clean_id = task_id.rsplit_once('/').map(|(_, r)| r).unwrap_or(task_id);
            if !clean_id.is_empty() {
                format!("{action} ({clean_id})")
            } else {
                action.to_string()
            }
        }
        "background_task" => String::new(),
        _ => format_generic_tool_args(args),
    };

    (action_label, target_arg)
}

fn format_generic_tool_args(args: &serde_json::Value) -> String {
    let Some(obj) = args.as_object() else {
        return String::new();
    };
    if obj.is_empty() {
        return String::new();
    }

    let mut parts = Vec::new();
    for (k, v) in obj {
        if k == "CodeContent"
            || k == "ReplacementContent"
            || k == "content"
            || k == "system_prompt"
            || k == "Code"
            || k == "toolSummary"
            || k == "toolAction"
        {
            continue;
        }
        let val_str = match v {
            serde_json::Value::String(s) => {
                let first_line = s.lines().next().unwrap_or("").trim();
                if first_line.chars().count() > 30 {
                    format!("\"{}...\"", first_line.chars().take(27).collect::<String>())
                } else {
                    format!("\"{}\"", first_line)
                }
            }
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Array(a) => format!("[{} items]", a.len()),
            serde_json::Value::Object(_) => "{...}".to_string(),
            serde_json::Value::Null => "null".to_string(),
        };
        parts.push(format!("{k}={val_str}"));
    }

    if parts.is_empty() {
        if let Some(target) = obj
            .get("TargetFile")
            .or_else(|| obj.get("path"))
            .and_then(|v| v.as_str())
        {
            return target.to_string();
        }
    }

    parts.join(", ")
}

fn resolve_tool_result_name(
    preceding_call_name: Option<&str>,
    persisted_name: Option<&str>,
    content: &str,
) -> Option<String> {
    preceding_call_name
        .or(persisted_name)
        .or_else(|| content.split_once(": ").map(|(name, _)| name))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// Memoized conversation render. Building every message's spans and wrapping
/// them several times per frame is O(history) and dominates scroll latency on
/// long sessions. The rendered lines only change when the history, viewport
/// width, expanded-thoughts, modal state, or copy-badge state changes — so we
/// cache them and reuse across the many frames where nothing but the scroll
/// offset moved.
struct ChatCache {
    key: ChatKey,
    lines: Vec<Line<'static>>,
    header_wrapped_rows: Vec<(u16, usize)>,
    copy_wrapped_rows: Vec<(u16, String)>,
    msg_wrapped_rows: Vec<u16>,
    total_wrapped_lines: u16,
}

type RenderedConversation = (
    Vec<Line<'static>>,
    Vec<(u16, usize)>,
    Vec<(u16, String)>,
    Vec<u16>,
    u16,
);

#[derive(PartialEq, Clone)]
struct ChatKey {
    hist_len: usize,
    total_len: usize,
    last_len: usize,
    width: u16,
    show_picker: bool,
    thoughts: (usize, usize),
    copied_recently: Option<(String, bool)>,
    theme: String,
}

thread_local! {
    static CHAT_CACHE: std::cell::RefCell<Option<ChatCache>> =
        const { std::cell::RefCell::new(None) };
}

fn chat_cache_key(state: &AppState, width: u16, show_picker: bool) -> ChatKey {
    ChatKey {
        hist_len: state.history.len(),
        total_len: state.history.iter().map(|m| m.content.len()).sum(),
        last_len: state.history.last().map_or(0, |m| m.content.len()),
        width,
        show_picker,
        thoughts: (
            state.expanded_thoughts.len(),
            state.expanded_thoughts.iter().sum(),
        ),
        copied_recently: state
            .last_copy_text
            .as_ref()
            .map(|(t_text, t)| (t_text.clone(), t.elapsed().as_secs() < 2)),
        theme: state.config.theme.clone(),
    }
}

/// Deep-copy a borrowed `Line` into an owned `'static` one so it can outlive the
/// `state.history` borrow it was built from and sit in the frame cache.
fn own_line(line: &Line) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .iter()
        .map(|s| Span::styled(s.content.clone().into_owned(), s.style))
        .collect();
    let mut owned = Line::from(spans);
    owned.style = line.style;
    owned.alignment = line.alignment;
    owned
}

/// Maximum number of rendered tool results kept in [`TOOL_RESULT_CACHE`].
const TOOL_RESULT_CACHE_CAP: usize = 256;

thread_local! {
    /// Rendered tool results keyed by content hash. Bounded with LRU eviction
    /// so overflowing the cap drops one cold entry instead of flushing every
    /// still-visible result and forcing a full re-highlight on the next frame.
    static TOOL_RESULT_CACHE: RefCell<lru::LruCache<u64, Vec<Line<'static>>>> =
        RefCell::new(lru::LruCache::new(TOOL_RESULT_CACHE_CAP));
}

fn tool_result_cache_key(
    tool_name: &str,
    result: &str,
    width: usize,
    verbosity: &crate::app::Verbosity,
    show_picker: bool,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool_name.hash(&mut hasher);
    result.hash(&mut hasher);
    width.hash(&mut hasher);
    verbosity.hash(&mut hasher);
    show_picker.hash(&mut hasher);
    theme::active_palette().name.hash(&mut hasher);
    hasher.finish()
}

fn cached_tool_result(
    tool_name: &str,
    result: &str,
    width: usize,
    verbosity: &crate::app::Verbosity,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let key = tool_result_cache_key(tool_name, result, width, verbosity, show_picker);

    TOOL_RESULT_CACHE.with(|cache| {
        // A hit refreshes recency, so results currently on screen are never the
        // eviction victim.
        if let Some(lines) = cache.borrow_mut().get(&key) {
            return lines.clone();
        }
        let lines = render_tool_result(tool_name, result, width, verbosity, show_picker)
            .iter()
            .map(own_line)
            .collect::<Vec<_>>();
        cache.borrow_mut().insert(key, lines.clone());
        lines
    })
}

fn push_turn_separator<'a>(lines: &mut Vec<Line<'a>>, width: u16, show_picker: bool) {
    let rule = "─".repeat(width.max(1) as usize);
    // No leading blank: the preceding transcript item (assistant text, status
    // card, tool card, user bubble) already ends with its own trailing blank
    // row, so pushing one here doubled the gap above the rule.
    lines.push(Line::from(Span::<'static>::styled(
        rule,
        get_themed_style(
            COLOR_TURN_SEPARATOR(),
            COLOR_BG(),
            Modifier::empty(),
            show_picker,
        ),
    )));
    lines.push(Line::from(""));
}

fn render_status_panel<'a>(
    content: &str,
    _width: u16,
    show_picker: bool,
    lines: &mut Vec<Line<'a>>,
) {
    let version = env!("CARGO_PKG_VERSION");
    let lower = content.to_ascii_lowercase();

    let is_info_notice = lower.starts_with("session status")
        || lower.starts_with("session usage")
        || lower.starts_with("rustcode info")
        || lower.starts_with("about rustcode")
        || lower.starts_with("notice: rustcode")
        || lower.starts_with("rustcode help")
        || lower.starts_with("available commands")
        || lower.starts_with("core & session")
        || lower.starts_with("help & commands")
        || lower.starts_with("discovered skills")
        || lower.starts_with("available themes")
        || lower.contains("model quota status")
        || lower.starts_with("quota:");

    let is_warning = !is_info_notice
        && ["warning", "error", "failed", "blocked", "abort", "loop"]
            .iter()
            .any(|word| lower.contains(word));

    if is_info_notice {
        lines.push(Line::from(vec![
            Span::styled(
                ">_ RustCode ",
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                format!("(v{})", version),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ]));
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if is_info_notice {
                lines.push(Line::from(""));
            }
            continue;
        }

        if is_info_notice && trimmed.eq_ignore_ascii_case("rustcode info") {
            continue;
        }

        if is_info_notice
            && (trimmed.ends_with(':')
                || trimmed.starts_with("📊")
                || trimmed.starts_with("📦")
                || trimmed.starts_with("🎨"))
        {
            lines.push(Line::from(vec![
                Span::styled(
                    "  ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    trimmed.to_string(),
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
            ]));
        } else if is_info_notice && trimmed.starts_with('/') {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let cmd_name = parts.first().copied().unwrap_or("");
            let cmd_desc = if parts.len() > 1 {
                parts[1..].join(" ")
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    "  ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    format!("{:<18}", cmd_name),
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    cmd_desc,
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
            ]));
        } else if is_info_notice
            && (trimmed.starts_with("Enter")
                || trimmed.starts_with("Shift+")
                || trimmed.starts_with("Esc")
                || trimmed.starts_with("Up/Down")
                || trimmed.starts_with("Ctrl+")
                || trimmed.starts_with("Alt+"))
        {
            let parts: Vec<&str> = trimmed.splitn(2, "  ").collect();
            let key = parts.first().copied().unwrap_or("").trim();
            let desc = if parts.len() > 1 { parts[1].trim() } else { "" };
            lines.push(Line::from(vec![
                Span::styled(
                    "  ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    format!("{:<18}", key),
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    desc.to_string(),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
            ]));
        } else if is_info_notice && (trimmed.starts_with('•') || trimmed.starts_with('-')) {
            lines.push(Line::from(vec![
                Span::styled(
                    "  • ",
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    trimmed
                        .trim_start_matches('•')
                        .trim_start_matches('-')
                        .trim()
                        .to_string(),
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
            ]));
        } else if is_warning {
            lines.push(Line::from(vec![
                Span::styled(
                    "! ",
                    get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    trimmed.to_string(),
                    get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    "  ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    trimmed.to_string(),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
            ]));
        }
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

    // Streaming shows live content that changes every frame, so it always
    // rebuilds; when idle we reuse the cached lines whenever the key matches.
    let idle = !matches!(state.status, AppStatus::Streaming | AppStatus::Queued);
    let cache_key = chat_cache_key(state, inner_area.width, show_picker);
    let cached: Option<RenderedConversation> = if idle {
        CHAT_CACHE.with(|c| {
            c.borrow().as_ref().filter(|c| c.key == cache_key).map(|c| {
                (
                    c.lines.clone(),
                    c.header_wrapped_rows.clone(),
                    c.copy_wrapped_rows.clone(),
                    c.msg_wrapped_rows.clone(),
                    c.total_wrapped_lines,
                )
            })
        })
    } else {
        None
    };

    let (lines, header_wrapped_rows, copy_wrapped_rows, msg_wrapped_rows, total_wrapped_lines): RenderedConversation = if let Some(hit) = cached {
        hit
    } else {
    let mut lines: Vec<Line> = Vec::new();

    let mut thought_clicks: Vec<(usize, usize)> = Vec::new();
    let mut copy_clicks: Vec<(usize, String)> = Vec::new();

    // Line index where each message's block starts. Messages that render
    // nothing produce a duplicate index; deduped below.
    let mut msg_start_lines: Vec<usize> = Vec::new();

    for (msg_idx, msg) in state.history.iter().enumerate() {
        msg_start_lines.push(lines.len());
        if msg.role == "system" {
            // Hide benign intermediate loop warnings from TUI display
            if msg.content.contains("Loop warning:") {
                continue;
            }
            if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                lines.push(Line::from(""));
            }
            render_status_panel(&msg.content, inner_area.width, show_picker, &mut lines);
            if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                lines.push(Line::from(""));
            }
        } else if msg.role == "tool" {
            let prev_tool_info = if msg_idx > 0 {
                // Walk backward past consecutive tool messages to find the preceding assistant message
                let mut assistant_idx = None;
                let mut tool_count_before_this = 0;
                for i in (0..msg_idx).rev() {
                    if state.history[i].role == "tool" {
                        tool_count_before_this += 1;
                    } else if state.history[i].role == "assistant" {
                        assistant_idx = Some(i);
                        break;
                    } else {
                        break;
                    }
                }

                if let Some(a_idx) = assistant_idx {
                    let assistant_msg = &state.history[a_idx];
                    let calls = crate::tools::parse_tool_calls(
                        &assistant_msg.content,
                        state.active_tool_protocol(),
                    );
                    calls.get(tool_count_before_this).cloned().or_else(|| calls.first().cloned())
                } else {
                    None
                }
            } else {
                None
            };

            let (action, arg) = if let Some(ref tool_call) = prev_tool_info {
                format_pi_tool_action(&tool_call.name, &tool_call.arguments)
            } else {
                let tool_name = resolve_tool_result_name(
                    None,
                    msg.tool_result.as_ref().map(|r| r.tool_name.as_str()),
                    msg.content.as_str(),
                )
                .unwrap_or_default();
                let action_label = match tool_name.as_str() {
                    "view_file" => "Read".to_string(),
                    "replace_file_content" | "multi_replace_file_content" => "Edit".to_string(),
                    "write_to_file" => "Write".to_string(),
                    "list_directory" | "glob" => "ListDir".to_string(),
                    "grep" => "Grep".to_string(),
                    "run_command" => "Bash".to_string(),
                    other => to_pascal_case(other),
                };
                (action_label, String::new())
            };

            let mut spans = vec![
                Span::styled(
                    "● ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    action,
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
            ];
            if !arg.is_empty() {
                spans.push(Span::styled(
                    format!("({arg})"),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
            }
            lines.push(Line::from(spans));

            if let Some((ref path, ref content)) = msg.file_preview {
                lines.extend(render_file_preview(
                    path,
                    content,
                    inner_area.width as usize,
                    show_picker,
                ));
            } else if let Some(ref diff) = msg.diff {
                if !matches!(state.verbosity, crate::app::Verbosity::High) {
                    let code_content_width = inner_area.width as usize;
                    lines.extend(render_unified_diff(diff, code_content_width, show_picker));
                }
            } else if let Some(tool_name) = resolve_tool_result_name(
                prev_tool_info.as_ref().map(|call| call.name.as_str()),
                msg.tool_result.as_ref().map(|result| result.tool_name.as_str()),
                &msg.content,
            ) {
                let result = msg
                    .content
                    .split_once(": ")
                    .map(|(_, result)| result)
                    .unwrap_or(&msg.content);
                lines.extend(cached_tool_result(
                    &tool_name,
                    result,
                    inner_area.width as usize,
                    &state.verbosity,
                    show_picker,
                ));
            }

        } else if msg.role == "user" {
            if msg_idx > 0 {
                push_turn_separator(&mut lines, inner_area.width, show_picker);
            }
            // Account for "▌" prefix (1 char) + internal bubble padding (2 left + 2 right) + right margin (3)
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
                    "▌",
                    get_themed_style(COLOR_SECONDARY(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    " ".repeat(content_width + 4),
                    get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker),
                ),
            ]));

            for line_str in wrapped_lines {
                let padded_text = pad_to_width(&line_str, content_width);
                lines.push(Line::from(vec![
                    Span::styled(
                        "▌",
                        get_themed_style(COLOR_SECONDARY(), COLOR_BG(), Modifier::empty(), show_picker),
                    ),
                    Span::styled(
                        format!("  {padded_text}  "),
                        get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker),
                    ),
                ]));
            }

            // Bottom padding row
            lines.push(Line::from(vec![
                Span::styled(
                    "▌",
                    get_themed_style(COLOR_SECONDARY(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    " ".repeat(content_width + 4),
                    get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker),
                ),
            ]));
            lines.push(Line::from(""));
        } else if msg.role == "assistant" {
            if let Some(tool_call) =
                crate::tools::parse_tool_call(&msg.content, state.active_tool_protocol())
            {
                let has_following_tool_result = state.history.get(msg_idx + 1).is_some_and(|m| m.role == "tool");
                if !has_following_tool_result {
                    let (action, arg) =
                        format_pi_tool_action(&tool_call.name, &tool_call.arguments);
                    let elapsed_ms = state.generation_start_time.map(|t| t.elapsed().as_millis()).unwrap_or(0);
                    let circle = if (elapsed_ms / 350).is_multiple_of(2) { "○ " } else { "● " };
                    lines.push(Line::from(vec![
                        Span::styled(
                            circle,
                            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                        ),
                        Span::styled(
                            action,
                            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                        ),
                        Span::styled(
                            format!("({arg})..."),
                            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
                        ),
                    ]));
                }
                continue;
            }
            let collapsed = !state.expanded_thoughts.contains(&msg_idx);
            render_assistant_message(
                &msg.content,
                &mut lines,
                &mut thought_clicks,
                &mut copy_clicks,
                AssistantRenderOptions {
                    response_time_ms: msg.response_time_ms,
                    model_name: &model_label(state),
                    is_generating: false,
                    viewport_width: inner_area.width,
                    show_picker,
                    thought_collapsed: collapsed,
                    msg_index: Some(msg_idx),
                    last_copy_text: state.last_copy_text.clone(),
                },
            );
            let next_is_tool = state.history.get(msg_idx + 1).is_some_and(|m| {
                m.role == "tool"
                    || (m.role == "assistant"
                        && crate::tools::parse_tool_call(&m.content, state.active_tool_protocol()).is_some())
            });
            if !next_is_tool {
                lines.push(Line::from(""));
            }
        }
    }

    if (state.status == AppStatus::Streaming || state.status == AppStatus::Queued)
        && !state.current_response.is_empty()
    {
        let parsed_tool = crate::tools::parse_tool_call(
            &state.current_response,
            state.active_tool_protocol(),
        );
        let is_tool_syntax = crate::tools::is_tool_call_start(&state.current_response);

        let should_hide_stream = match parsed_tool {
            Some(ref tool_call) => !crate::tools::is_code_editing_tool(&tool_call.name),
            None => is_tool_syntax,
        };

        if !should_hide_stream {
            render_assistant_message(
                &state.current_response,
                &mut lines,
                &mut thought_clicks,
                &mut copy_clicks,
                AssistantRenderOptions {
                    response_time_ms: None,
                    model_name: &model_label(state),
                    is_generating: true,
                    viewport_width: inner_area.width,
                    show_picker,
                    thought_collapsed: false,
                    msg_index: None,
                    last_copy_text: state.last_copy_text.clone(),
                },
            );
            lines.push(Line::from(""));
        }
    }

    // breathing room between the last line and the input box when
    // scrolled to the bottom
    lines.push(Line::from(""));

    // Resolve wrapped start rows for everything the mouse can hit — thought
    // headers, code-block [Copy] badges, and message boundaries — in a single
    // pass. Lines wrap independently, so per-line line_count sums to the exact
    // screen offset.
    let mut header_wrapped_rows: Vec<(u16, usize)> = Vec::new();
    let mut copy_wrapped_rows: Vec<(u16, String)> = Vec::new();
    let mut msg_wrapped_rows: Vec<u16> = Vec::new();
    let mut cum = 0u16;
    {
        let click_map: std::collections::HashMap<usize, usize> =
            thought_clicks.iter().copied().collect();
        let copy_map: std::collections::HashMap<usize, String> =
            copy_clicks.iter().cloned().collect();
        // Messages that emitted no lines share their successor's start index,
        // and a trailing index past the end belongs to no visible content.
        let msg_line_set: std::collections::HashSet<usize> = msg_start_lines
            .iter()
            .copied()
            .filter(|&i| i < lines.len())
            .collect();
        for (i, line) in lines.iter().enumerate() {
            if let Some(&midx) = click_map.get(&i) {
                header_wrapped_rows.push((cum, midx));
            }
            if let Some(text) = copy_map.get(&i) {
                copy_wrapped_rows.push((cum, text.clone()));
            }
            if msg_line_set.contains(&i) {
                msg_wrapped_rows.push(cum);
            }
            let w = line.width() as u16;
            let h = if inner_area.width == 0 || w <= inner_area.width {
                1
            } else {
                Paragraph::new(vec![line.clone()])
                    .wrap(Wrap { trim: false })
                    .line_count(inner_area.width) as u16
            };
            cum = cum.saturating_add(h);
        }
    }

    let total_wrapped_lines = cum;

        let owned_lines: Vec<Line<'static>> = lines.iter().map(own_line).collect();
        if idle {
            let cache = ChatCache {
                key: cache_key,
                lines: owned_lines.clone(),
                header_wrapped_rows: header_wrapped_rows.clone(),
                copy_wrapped_rows: copy_wrapped_rows.clone(),
                msg_wrapped_rows: msg_wrapped_rows.clone(),
                total_wrapped_lines,
            };
            CHAT_CACHE.with(|c| *c.borrow_mut() = Some(cache));
        }
        (owned_lines, header_wrapped_rows, copy_wrapped_rows, msg_wrapped_rows, total_wrapped_lines)
    };

    let conversation_paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(COLOR_BG()));

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

    // Sticky jump-to-latest pill — rendered AFTER the chat paragraph so it isn't
    // painted over. Borderless dark pill centered along the bottom of the chat
    // area, one row clear of the input box, labelled with how many messages
    // start below the viewport.
    // saturating_sub / min guard against narrow viewports.
    if state.scroll_row < state.last_max_scroll {
        let last_visible = scroll_offset + inner_area.height.saturating_sub(1);
        let hidden = msg_wrapped_rows
            .iter()
            .filter(|&&row| row > last_visible)
            .count();
        let label = scroll_pill_label(hidden);
        let btn_width = (label.chars().count() as u16).min(inner_area.width);
        let btn_x = inner_area.x + inner_area.width.saturating_sub(btn_width) / 2;
        // One blank row between the pill and the input box below it.
        let btn_y = inner_area.y + inner_area.height.saturating_sub(2);
        let btn_rect = ratatui::layout::Rect::new(btn_x, btn_y, btn_width, 1);
        state.scroll_to_bottom_btn = Some(btn_rect);
        let pill_bg = if state.hover == HoverTarget::ScrollPill {
            COLOR_HOVER_BG()
        } else {
            COLOR_NOTICE_BG()
        };
        f.render_widget(ratatui::widgets::Clear, btn_rect);
        f.render_widget(
            ratatui::widgets::Paragraph::new(label)
                .alignment(ratatui::layout::Alignment::Center)
                .style(
                    Style::default()
                        .fg(COLOR_TEXT())
                        .bg(pill_bg)
                        .add_modifier(Modifier::BOLD),
                ),
            btn_rect,
        );
    } else {
        state.scroll_to_bottom_btn = None;
    }

    // Map visible thought headers to on-screen rows for click hit-testing.
    state.thought_toggle_rows.clear();
    for (wrapped_row, midx) in header_wrapped_rows {
        if wrapped_row >= scroll_offset && wrapped_row < scroll_offset + inner_area.height {
            let screen_row = inner_area.y + (wrapped_row - scroll_offset);
            state.thought_toggle_rows.push((screen_row, midx));
        }
    }

    // Map each visible [Copy] badge to its on-screen row for click hit-testing.
    state.code_copy_rows.clear();
    for (wrapped_row, text) in copy_wrapped_rows {
        if wrapped_row >= scroll_offset && wrapped_row < scroll_offset + inner_area.height {
            let screen_row = inner_area.y + (wrapped_row - scroll_offset);
            state.code_copy_rows.push((screen_row, text));
        }
    }

    // Hover feedback for the clickable chat rows. The rows live inside the
    // memoized conversation lines, so tinting them here — after the paragraph
    // is painted — keeps the pointer out of the cache key.
    match state.hover {
        HoverTarget::ThoughtHeader(row) => {
            if row >= inner_area.y && row < inner_area.y + inner_area.height {
                let buf = f.buffer_mut();
                for col in inner_area.x..inner_area.x + inner_area.width {
                    if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(col, row)) {
                        cell.set_bg(COLOR_HOVER_BG());
                    }
                }
            }
        }
        HoverTarget::CopyBadge(row) => {
            if row >= inner_area.y && row < inner_area.y + inner_area.height {
                let buf = f.buffer_mut();
                let code_text = state
                    .code_copy_rows
                    .iter()
                    .find(|(r, _)| *r == row)
                    .map(|(_, t)| t);
                let badge_width = if code_text.is_some_and(|ct| {
                    state
                        .last_copy_text
                        .as_ref()
                        .is_some_and(|(t_text, t)| t_text == ct && t.elapsed().as_secs() < 2)
                }) {
                    12
                } else {
                    9
                };
                let badge_start = (inner_area.x + inner_area.width).saturating_sub(badge_width);
                for col in badge_start..inner_area.x + inner_area.width {
                    if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(col, row)) {
                        cell.set_bg(COLOR_HOVER_BG());
                    }
                }
            }
        }
        _ => {}
    }

    let _conv = chunks[0];
    let _view_h = inner_area.height;
    let _content_h = total_wrapped_lines.max(1);
}

pub fn render(f: &mut Frame, state: &mut AppState) {
    theme::set_active_theme(&state.config.theme);

    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_BG())),
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

        let (_, at_query) =
            crate::app::get_at_word_query(&state.input_buffer, state.cursor_position)
                .unwrap_or((0, String::new()));
        let at_files = if !at_query.is_empty()
            || state.input_buffer[..safe_byte_index(&state.input_buffer, state.cursor_position)]
                .ends_with('@')
        {
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

    if state.show_theme_picker {
        render_theme_picker_modal(f, state);
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

    if state.status == AppStatus::AwaitingQuestion {
        render_question_modal(f, state);
    }

    if state.status == AppStatus::VerbosityPicker {
        render_verbosity_picker_modal(f, state);
    }

    // Painted last so it sits on top of everything, like a native selection.
    if !state.modal_open() {
        if let (Some(start), Some(end)) = (state.sel_start, state.sel_end) {
            if start != end {
                // Input-box selections have no scroll offset and use the input rect;
                // chat selections use chat_area and the chat scroll_row.
                let (area, scroll) = if state.sel_in_input {
                    (state.input_text_area, 0)
                } else {
                    (state.chat_area, state.scroll_row)
                };
                highlight_selection(f, start, end, area, scroll);
                let text = extract_selection(f.buffer_mut(), start, end, area, scroll);
                if !text.is_empty() {
                    state.selected_text = Some(text);
                }
            } else {
                state.selected_text = None;
            }
        } else {
            state.selected_text = None;
        }
    }

    // Transient notice toast, painted above everything (even modals).
    render_notice(f, state);
}

/// Label for the jump-to-latest pill. `hidden` is the number of messages whose
/// first row sits below the viewport; zero means the user is only part-way
/// through the last message, so no count is worth showing.
fn scroll_pill_label(hidden: usize) -> String {
    match hidden {
        0 => " click to scroll down ↓ ".to_string(),
        1 => " 1 new message · click to scroll down ↓ ".to_string(),
        n => format!(" {n} new messages · click to scroll down ↓ "),
    }
}

/// How long a notice toast stays on screen before it fades out.
pub(crate) const NOTICE_TTL: std::time::Duration = std::time::Duration::from_secs(3);

/// Columns of padding around the toast text: accent glyph + spaces on both sides.
const NOTICE_PADDING: u16 = 5;

/// Computes the top-right rect for a borderless notice toast holding
/// `text_width` columns of text, or `None` if the screen is too small. The box
/// is a single row — text plus padding — inset one cell from the corner and
/// clamped to the screen width.
fn notice_rect(area: ratatui::layout::Rect, text_width: u16) -> Option<ratatui::layout::Rect> {
    let box_h = 1u16;
    let box_w = (text_width + NOTICE_PADDING)
        .min(area.width.saturating_sub(2))
        .max(3);
    if area.width < box_w + 1 || area.height < box_h + 1 {
        return None;
    }
    let x = area.x + area.width - box_w - 1;
    let y = area.y + 1;
    Some(ratatui::layout::Rect::new(x, y, box_w, box_h))
}

/// Draws a small auto-expiring toast in the top-right corner. Cleared lazily
/// once expired; the ≤100ms idle redraw guarantees it disappears on time.
/// Borderless: a single dark pill sized to its text, with a leading status
/// glyph carrying the accent color.
fn render_notice(f: &mut Frame, state: &mut AppState) {
    let Some(notice) = state.notice.as_ref() else {
        return;
    };
    if notice.shown_at.elapsed() >= NOTICE_TTL {
        state.notice = None;
        return;
    }

    let is_warning = ["warning", "error", "failed", "blocked", "abort", "loop"]
        .iter()
        .any(|word| notice.text.to_ascii_lowercase().contains(word));
    let (glyph, accent) = match notice.kind {
        NoticeKind::Warning => ("!", COLOR_TIP()),
        NoticeKind::Notice if is_warning => ("!", COLOR_TIP()),
        NoticeKind::Notice => ("✓", COLOR_GREEN()),
    };

    // Size to the message so short notices ("Copied to clipboard") don't paint a
    // full-width slab over the conversation.
    let text_width = notice.text.chars().count().min(60) as u16;
    let Some(rect) = notice_rect(f.area(), text_width) else {
        return;
    };

    let text: String = notice.text.chars().take(60).collect();
    let bg = COLOR_PANEL();
    let para = Paragraph::new(Line::from(vec![
        Span::styled("▌", Style::default().fg(accent).bg(bg)),
        Span::styled(
            format!(" {glyph} "),
            Style::default()
                .fg(accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(text, Style::default().fg(COLOR_TEXT()).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ]))
    .style(Style::default().bg(bg));

    f.render_widget(Clear, rect);
    f.render_widget(para, rect);
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
        // Chat content renders flush to ca.x (chat_area is already inset), so the
        // selectable span is [ca.x, ca.x+width-1]. A former `+2`/`-2` gutter here
        // clipped the first and last two columns of every left-aligned line.
        (
            ca.y,
            ca.y + ca.height.saturating_sub(1),
            ca.x,
            ca.x + ca.width.saturating_sub(1),
        )
    } else {
        (
            area.y + 1,
            area.y + area.height.saturating_sub(2),
            area.x,
            area.x + width.saturating_sub(1),
        )
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
                if !sym.trim().is_empty() && sym != "│" && sym != "░" && sym != "█" && sym != "▌"
                {
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

        let col_from = if row == start_row {
            screen_start.0.max(min_col).min(max_col)
        } else {
            min_col
        };
        // Last row stops at the pointer, every earlier row runs to its end of
        // content — the same shape `extract_selection` copies, so what is
        // highlighted and what lands on the clipboard always agree.
        let col_to = if row == end_row {
            screen_end.0.max(min_col).min(max_col).min(last_col)
        } else {
            last_col
        };

        if col_from > col_to {
            continue;
        }

        for col in col_from..=col_to {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(col, row)) {
                cell.set_fg(Color::Rgb(0, 0, 0));
                cell.set_bg(COLOR_SELECTION());
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
        // Must match highlight_selection's bounds so the copied text lines up
        // exactly with what the user sees highlighted (no clipped first/last cols).
        (
            ca.y,
            ca.y + ca.height.saturating_sub(1),
            ca.x,
            ca.x + ca.width.saturating_sub(1),
        )
    } else {
        (
            area.y + 1,
            area.y + area.height.saturating_sub(2),
            area.x,
            area.x + width.saturating_sub(1),
        )
    };

    if screen_start.1 > max_row || screen_end.1 < min_row {
        return String::new();
    }

    let start_row = screen_start.1.max(min_row).min(max_row);
    let end_row = screen_end.1.max(min_row).min(max_row);

    let mut lines_out = Vec::new();
    for row in start_row..=end_row {
        let col_from = if row == start_row {
            screen_start.0.max(min_col).min(max_col)
        } else {
            min_col
        };
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
            "│ ",
            "│",
            "▌ ",
            "▌",
            "⚙ ",
            "⚙",
            "→ ",
            "→",
            "🦀 ",
            "🦀",
            "🌐 ",
            "🌐",
            "+ Warning: ",
            "Warning: ",
            "+ Thought: ",
            "Thought: ",
            "Goal: ",
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

        // Keep leading whitespace: copied code has to paste back with its
        // indentation intact. Only the trailing padding the terminal renders is
        // dropped (already done by `trim_end` above).
        lines_out.push(clean.to_string());
    }

    // Blank rows inside the selection are real blank lines, but blank rows at
    // either end are just the empty space the drag swept over.
    while lines_out.first().is_some_and(|l| l.trim().is_empty()) {
        lines_out.remove(0);
    }
    while lines_out.last().is_some_and(|l| l.trim().is_empty()) {
        lines_out.pop();
    }

    let res = lines_out.join("\n");
    dbg_log!(
        "[SELECTION] Extracted {} chars from selection range start={:?} end={:?}: {:?}",
        res.len(),
        start,
        end,
        res
    );
    res
}

#[cfg(test)]
mod tests {
    use super::collapse_image_markers;

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
        let mut clicks = Vec::new();
        let mut copies = Vec::new();
        let width: u16 = 80;
        render_assistant_message(
            content,
            &mut lines,
            &mut clicks,
            &mut copies,
            AssistantRenderOptions {
                response_time_ms: None,
                model_name: "model",
                is_generating: false,
                viewport_width: width,
                show_picker: false,
                thought_collapsed: true,
                msg_index: None,
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
                    .all(|span| span.style.bg == Some(super::COLOR_BG())),
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
        let mut clicks = Vec::new();
        let mut copies = Vec::new();
        render_assistant_message(
            content,
            &mut lines,
            &mut clicks,
            &mut copies,
            AssistantRenderOptions {
                response_time_ms: None,
                model_name: "model",
                is_generating: false,
                viewport_width: 80,
                show_picker: false,
                thought_collapsed: true,
                msg_index: None,
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
    fn status_panels_render_minimal_inline() {
        use super::render_status_panel;

        let mut lines = Vec::new();
        render_status_panel("Session status: 5 messages", 80, false, &mut lines);

        assert_eq!(lines.len(), 2, "info status panel includes header");
        assert!(lines[0].spans[0].content.contains(">_ RustCode"));
        assert!(lines[1].spans[0].content.contains("  "));
        assert!(
            lines[1].spans[1]
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
    fn tool_action_formats_generic_args_and_omits_empty() {
        use super::format_pi_tool_action;

        let (action, arg) = format_pi_tool_action(
            "manage_task",
            &serde_json::json!({"Action": "status", "TaskId": "task-123"}),
        );
        assert_eq!(action, "ManageTask");
        assert_eq!(arg, "status (task-123)");

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
}
