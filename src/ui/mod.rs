mod highlight;
mod lru;
mod markdown;
mod modals;
mod tool_result;

use highlight::{
    highlight_code_block, highlight_code_line, highlight_diff_line,
    render_unified_diff, wrap_code_spans,
};
use markdown::render_markdown;
pub use modals::{PALETTE_ITEMS, PaletteItem};
pub mod theme;
use modals::{
    render_at_popup_menu, render_command_picker_modal, render_history_picker_modal,
    render_mcp_config_modal, render_model_picker_modal, render_popup_menu,
    render_protocol_picker_modal, render_question_modal, render_theme_picker_modal,
    render_thinking_picker_modal, render_tool_confirmation_modal, render_verbosity_picker_modal,
};
use tool_result::{render_file_preview, render_tool_result};

use crate::app::activity::{ActivityKind, animation_cells, classify_activity};
use crate::app::{AppState, AppStatus, ChatMessage, HoverTarget, NoticeKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
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
#[allow(non_snake_case, dead_code)]
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
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_DIFF_ADD_BG() -> Color {
    theme::color_diff_add_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_ADD_FG() -> Color {
    theme::color_diff_add_fg()
}
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_DIFF_REMOVE_BG() -> Color {
    theme::color_diff_remove_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_REMOVE_FG() -> Color {
    theme::color_diff_remove_fg()
}
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_DIFF_ABSENT_BG() -> Color {
    theme::color_diff_absent_bg()
}

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
    const MARK_IMG: &str = "![image](file://";
    const MARK_PASTE: &str = "<!--PASTE:";
    if !text.contains(MARK_IMG) && !text.contains(MARK_PASTE) {
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
        let mut img_n = 0;
        let mut paste_n = 0;
        while !rest.is_empty() {
            let next_img = rest.find(MARK_IMG);
            let next_paste = rest.find(MARK_PASTE);

            match (next_img, next_paste) {
                (None, None) => {
                    out.push_str(rest);
                    break;
                }
                (Some(idx), None) => {
                    out.push_str(&rest[..idx]);
                    let after = &rest[idx + MARK_IMG.len()..];
                    if let Some(close) = after.find(')') {
                        img_n += 1;
                        out.push_str(&format!("[Image #{img_n}]"));
                        rest = &after[close + 1..];
                    } else {
                        out.push_str(&rest[idx..]);
                        break;
                    }
                }
                (None, Some(idx)) => {
                    out.push_str(&rest[..idx]);
                    let after = &rest[idx + MARK_PASTE.len()..];
                    if let Some(end) = after.find("-->") {
                        let payload = &after[..end];
                        paste_n += 1;
                        if let Some((len_str, body)) = payload.split_once(':') {
                            let len_num: usize = len_str.parse().unwrap_or(body.len());
                            out.push_str(&format!("[Pasted Text #{paste_n} ({len_num} chars)]"));
                        } else {
                            out.push_str(&format!("[Pasted Text #{paste_n}]"));
                        }
                        rest = &after[end + 3..];
                    } else {
                        out.push_str(&rest[idx..]);
                        break;
                    }
                }
                (Some(img_idx), Some(paste_idx)) => {
                    if img_idx < paste_idx {
                        out.push_str(&rest[..img_idx]);
                        let after = &rest[img_idx + MARK_IMG.len()..];
                        if let Some(close) = after.find(')') {
                            img_n += 1;
                            out.push_str(&format!("[Image #{img_n}]"));
                            rest = &after[close + 1..];
                        } else {
                            out.push_str(&rest[img_idx..]);
                            break;
                        }
                    } else {
                        out.push_str(&rest[..paste_idx]);
                        let after = &rest[paste_idx + MARK_PASTE.len()..];
                        if let Some(end) = after.find("-->") {
                            let payload = &after[..end];
                            paste_n += 1;
                            if let Some((len_str, body)) = payload.split_once(':') {
                                let len_num: usize = len_str.parse().unwrap_or(body.len());
                                out.push_str(&format!(
                                    "[Pasted Text #{paste_n} ({len_num} chars)]"
                                ));
                            } else {
                                out.push_str(&format!("[Pasted Text #{paste_n}]"));
                            }
                            rest = &after[end + 3..];
                        } else {
                            out.push_str(&rest[paste_idx..]);
                            break;
                        }
                    }
                }
            }
        }
        *cache = (hash, out.clone());
        out
    })
}

fn model_label(state: &AppState) -> String {
    // Only show the main (big) model — hide the small model entirely.
    state.config.default.big().to_string()
}

struct AssistantRenderOptions {
    token_usage: Option<crate::app::TokenUsage>,
    response_time_ms: Option<u64>,
    is_generating: bool,
    viewport_width: u16,
    show_picker: bool,
    thought_collapsed: bool,
    msg_index: Option<usize>,
    last_copy_text: Option<(String, std::time::Instant)>,
}

fn strip_rendered_tool_blocks(content: &str) -> String {
    let mut output = content.to_string();

    for fence in ["```tool", "```json"] {
        let mut search_from = 0;
        while let Some(relative_start) = output[search_from..].find(fence) {
            let start = search_from + relative_start;
            let block_start = start + fence.len();
            let Some(relative_end) = output[block_start..].find("```") else {
                break;
            };
            let end = block_start + relative_end + 3;
            let block = &output[block_start..block_start + relative_end];
            let is_tool_call =
                crate::tools::parse_tool_call(block, crate::config::ToolProtocol::Json).is_some();

            if is_tool_call {
                output.replace_range(start..end, "");
                search_from = start;
            } else {
                search_from = end;
            }
        }
    }

    output
}

fn render_assistant_message<'a>(
    content: &'a str,
    lines: &mut Vec<Line<'a>>,
    click_registry: &mut Vec<(usize, usize)>,
    copy_registry: &mut Vec<(usize, String)>,
    options: AssistantRenderOptions,
) {
    let AssistantRenderOptions {
        token_usage,
        response_time_ms,
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
        let time_str = response_time_ms.map(|ms| {
            if ms >= 1000 {
                let sec = ms as f32 / 1000.0;
                if (sec * 10.0).fract() == 0.0 || sec >= 10.0 {
                    format!("{:.0}s", sec)
                } else {
                    format!("{:.1}s", sec)
                }
            } else {
                format!("{}ms", ms)
            }
        });
        let tokens_str = token_usage.as_ref().map(|u| {
            if u.total_tokens >= 1000 {
                format!("{:.1}k tokens", u.total_tokens as f32 / 1000.0)
            } else {
                format!("{} tokens", u.total_tokens)
            }
        });

        let thought_meta = match (time_str, tokens_str) {
            (Some(t), Some(k)) => format!("Thought for {t}, {k}"),
            (Some(t), None) => format!("Thought for {t}"),
            (None, Some(k)) => format!("Thought for {k}"),
            (None, None) => "Thought".to_string(),
        };

        let toggle = if thought_collapsed { "+ " } else { "− " };
        if let Some(idx) = msg_index {
            click_registry.push((lines.len(), idx));
        }

        let first_line = think
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or("");
        let preview = if first_line.chars().count() > 70 {
            format!("{}...", first_line.chars().take(67).collect::<String>())
        } else {
            first_line.to_string()
        };

        lines.push(Line::from(vec![
            Span::styled(
                toggle,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                thought_meta,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
        ]));

        if thought_collapsed {
            if !preview.is_empty() {
                if let Some(idx) = msg_index {
                    click_registry.push((lines.len(), idx));
                }
                lines.push(Line::from(vec![Span::styled(
                    format!("  {preview}"),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
                )]));
            }
        } else {
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
    }

    let main_content = strip_rendered_tool_blocks(main_content);
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
    let footer_area = *chunks.last().unwrap();
    let show_picker = state.modal_open();
    let activity = classify_activity(&state.status, &state.running_tools);
    let animation_frame = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        / 100;
    let cells = animation_cells(animation_frame, 6);
    let action_detail = state
        .pending_tool_confirmation
        .as_ref()
        .and_then(|confirmations| confirmations.first())
        .map(|confirmation| format!("approve {}", confirmation.tool_name))
        .or_else(|| {
            state
                .pending_question
                .as_ref()
                .map(|_| "answer question".to_string())
        });

    let mut left_spans = Vec::new();
    for active in &cells {
        let (symbol, color) = match activity.kind {
            ActivityKind::ActionRequired => ("!", Color::Yellow),
            ActivityKind::Ready => ("◦", COLOR_MUTED()),
            _ if *active => ("●", COLOR_PRIMARY()),
            _ => ("◦", COLOR_MUTED()),
        };
        left_spans.push(Span::styled(
            symbol,
            get_themed_style(color, COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }

    let mut status_text = activity.label.clone();
    let detail = if activity.kind == ActivityKind::ActionRequired {
        action_detail
    } else {
        activity.detail.clone()
    };
    if let Some(detail) = detail {
        status_text.push_str(" · ");
        status_text.push_str(&detail);
    }
    if matches!(
        activity.kind,
        ActivityKind::Working | ActivityKind::RunningTool
    ) && let Some(started) = state.generation_start_time
    {
        status_text.push_str(&format!(" · {}s", started.elapsed().as_secs()));
    }

    left_spans.push(Span::styled(
        format!("  {status_text}"),
        get_themed_style(
            if activity.kind == ActivityKind::ActionRequired {
                Color::Yellow
            } else if activity.kind == ActivityKind::Ready {
                COLOR_MUTED()
            } else {
                COLOR_PRIMARY()
            },
            COLOR_BG(),
            Modifier::BOLD,
            show_picker,
        ),
    ));

    if matches!(
        activity.kind,
        ActivityKind::Queued | ActivityKind::Working | ActivityKind::RunningTool
    ) {
        left_spans.push(Span::styled(
            "   esc ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        left_spans.push(Span::styled(
            "interrupt",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
    }

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
            Constraint::Length(28),
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

/// Shows the most recently queued prompt on a thin line directly above the
/// input box, padded with a blank row above and below, so a prompt typed and
/// enqueued mid-stream doesn't vanish from view until it's actually sent.
/// Renders nothing when the queue is empty — the caller already collapses
/// this block to zero height in that case.
fn render_queue_line(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) {
    let Some(latest) = state.pending_queue.last() else {
        return;
    };
    let block = chunks[1];
    if block.height < 3 {
        return;
    }
    let show_picker = state.modal_open();
    let area = ratatui::layout::Rect::new(block.x, block.y + 1, block.width, 1);

    let label = format!("queued ({}): ", state.pending_queue.len());
    let hint = "press ↑ to edit";
    let max_text_width = (area.width as usize).saturating_sub(label.len() + hint.len() + 2);
    let preview: String = latest.chars().take(max_text_width).collect();
    let truncated = latest.chars().count() > max_text_width;
    let text = if truncated {
        format!("{preview}…")
    } else {
        preview
    };

    let left_spans = vec![
        Span::styled(
            label,
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ),
        Span::styled(
            text,
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::empty(), show_picker),
        ),
    ];
    let left_width: usize = left_spans.iter().map(|s| s.content.chars().count()).sum();

    f.render_widget(
        Paragraph::new(Line::from(left_spans)).style(Style::default().bg(COLOR_BG())),
        area,
    );

    if area.width as usize > left_width + hint.len() {
        let hint_area = ratatui::layout::Rect::new(
            area.x + area.width - hint.len() as u16,
            area.y,
            hint.len() as u16,
            1,
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
            )))
            .style(Style::default().bg(COLOR_BG())),
            hint_area,
        );
    }
}

fn render_input(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &mut AppState) -> Margin {
    let show_picker = state.modal_open();
    let area = chunks[2];

    let border_color = if show_picker {
        COLOR_MUTED()
    } else {
        COLOR_PRIMARY()
    };

    let (mode_label_str, mode_color) = match state.agent_mode {
        crate::config::AgentMode::Build => ("build", COLOR_PRIMARY()),
        crate::config::AgentMode::Plan => ("plan", Color::Rgb(229, 192, 123)),
    };
    let model_str = model_label(state);

    let title_left = Line::from(vec![
        Span::styled(" ✦ ", Style::default().fg(COLOR_PRIMARY()).add_modifier(Modifier::BOLD)),
        Span::styled("rustcode", Style::default().fg(COLOR_TEXT()).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ]);

    let title_right = Line::from(vec![
        Span::styled(format!("[{mode_label_str}] "), Style::default().fg(mode_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("[{model_str}] "), Style::default().fg(COLOR_MUTED())),
    ]);

    let footer_hints = Line::from(vec![
        Span::styled(" Tab ", Style::default().fg(COLOR_PRIMARY()).add_modifier(Modifier::BOLD)),
        Span::styled("autocomplete · ", Style::default().fg(COLOR_MUTED())),
        Span::styled("Shift+Enter ", Style::default().fg(COLOR_PRIMARY()).add_modifier(Modifier::BOLD)),
        Span::styled("newline · ", Style::default().fg(COLOR_MUTED())),
        Span::styled("/ ", Style::default().fg(COLOR_PRIMARY()).add_modifier(Modifier::BOLD)),
        Span::styled("commands · ", Style::default().fg(COLOR_MUTED())),
        Span::styled("Ctrl+O ", Style::default().fg(COLOR_PRIMARY()).add_modifier(Modifier::BOLD)),
        Span::styled("model ", Style::default().fg(COLOR_MUTED())),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(title_left)
        .title(title_right.alignment(ratatui::layout::Alignment::Right))
        .title_bottom(footer_hints.alignment(ratatui::layout::Alignment::Right));

    f.render_widget(block, area);

    let input_margin = Margin {
        vertical: 1,
        horizontal: 2,
    };
    let input_inner = area.inner(input_margin);

    let text_style = if state.input_buffer.starts_with('/') {
        get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker)
    } else {
        get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker)
    };

    let inner_width = input_inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_dx = 0u16;
    let mut cursor_dy = 0u16;

    if inner_width > 0 {
        let display_buffer = collapse_image_markers(&state.input_buffer);
        let mut styled_chars: Vec<(char, Style)> =
            display_buffer.chars().map(|c| (c, text_style)).collect();

        if state.input_buffer.is_empty() && state.get_command_suggestion().is_none() {
            let placeholder_style =
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker);
            let placeholder_text = "Ask a question, request code changes, or type / for commands...";
            styled_chars.extend(placeholder_text.chars().map(|c| (c, placeholder_style)));
        } else if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        let safe_end = state.cursor_position.min(state.input_buffer.len());
        let safe_end = if state.input_buffer.is_char_boundary(safe_end) {
            safe_end
        } else {
            state
                .input_buffer
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= safe_end)
                .last()
                .unwrap_or(0)
        };
        let raw_prefix = &state.input_buffer[..safe_end];
        let cursor_char_index = collapse_image_markers(raw_prefix).chars().count();

        let prompt_span = Span::styled(
            "❯ ",
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
        );
        let mut current_line_spans = vec![prompt_span];
        let mut current_run: Option<(Style, String)> = None;

        let mut col = 2;
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

    let text_area_height = input_inner.height;
    let text_area = ratatui::layout::Rect::new(
        input_inner.x,
        input_inner.y,
        input_inner.width,
        text_area_height,
    );
    let paragraph = Paragraph::new(lines).style(Style::default().bg(COLOR_BG()));
    f.render_widget(paragraph, text_area);
    state.input_text_area = Some(text_area);

    if inner_width > 0 && !show_picker {
        f.set_cursor_position((
            input_inner.x + cursor_dx.min(input_inner.width.saturating_sub(1)),
            input_inner.y + cursor_dy,
        ));
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
            .or_else(|| args.get("skill"))
            .or_else(|| args.get("skill_name"))
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
                format!("{action} {clean_id}")
            } else {
                action.to_string()
            }
        }
        "background_task" => {
            let task_id = args
                .get("TaskId")
                .or_else(|| args.get("task_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let clean_id = task_id.rsplit_once('/').map(|(_, r)| r).unwrap_or(task_id);
            clean_id.to_string()
        }
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

fn is_hidden_system_notice(content: &str) -> bool {
    content.contains("Loop warning:")
        || content.contains("tool calls in that response were dropped")
        || content.contains("Oversized response:")
        || content.starts_with(crate::network::compaction::SUMMARY_MARKER)
        || content.starts_with("[harness: stopped after ")
}

fn tool_result_follows(history: &[ChatMessage], assistant_index: usize) -> bool {
    history
        .iter()
        .skip(assistant_index + 1)
        .find(|message| !(message.role == "system" && is_hidden_system_notice(&message.content)))
        .map_or(false, |message| message.role == "tool")
}

fn fit_to_width(s: &str, target_width: usize) -> String {
    let char_count = s.chars().count();
    if char_count > target_width {
        if target_width > 1 {
            let truncated: String = s.chars().take(target_width - 1).collect();
            format!("{truncated}…")
        } else {
            s.chars().take(target_width).collect()
        }
    } else {
        format!("{:<width$}", s, width = target_width)
    }
}

fn render_status_panel<'a>(
    content: &str,
    width: u16,
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

    if !is_info_notice {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                lines.push(Line::from(""));
                continue;
            }
            if is_warning {
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
        return;
    }

    let border_c = COLOR_PRIMARY();
    let reset_bg = COLOR_BG();

    let box_w = (width as usize).saturating_sub(2).max(40);
    let inner_w = box_w.saturating_sub(2);
    let content_w = inner_w.saturating_sub(4);

    // Top border: ╭─ >_ RustCode v0.17.0 ──────────────────────────────────────────╮
    let title_str = format!(">_ RustCode v{version}");
    let top_pad = inner_w.saturating_sub(title_str.chars().count() + 3);
    let top_border = format!("╭─ {title_str} {}╮", "─".repeat(top_pad));
    lines.push(Line::from(vec![
        Span::styled(top_border, Style::default().fg(border_c).bg(reset_bg)),
    ]));

    // Top blank padding line
    lines.push(Line::from(vec![
        Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
        Span::styled(" ".repeat(inner_w), Style::default().bg(reset_bg)),
        Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
    ]));

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("rustcode info") {
            continue;
        }

        let is_header = trimmed.ends_with(':')
            || trimmed.starts_with("📊")
            || trimmed.starts_with("📦")
            || trimmed.starts_with("🎨")
            || trimmed.starts_with("Core & Session")
            || trimmed.starts_with("Help & Commands")
            || trimmed.starts_with("Discovered Skills");

        if is_header {
            let padded_header = fit_to_width(&format!("  {trimmed}"), content_w);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(padded_header, get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker)),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else if trimmed.starts_with('/') {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let cmd_name = parts.first().copied().unwrap_or("");
            let cmd_desc = if parts.len() > 1 { parts[1..].join(" ") } else { String::new() };
            let left_sp = format!("  {:<18}", cmd_name);
            let right_len = content_w.saturating_sub(left_sp.chars().count());
            let right_sp = fit_to_width(&cmd_desc, right_len);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(left_sp, get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker)),
                Span::styled(right_sp, get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker)),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else if trimmed.starts_with("Enter")
            || trimmed.starts_with("Shift+")
            || trimmed.starts_with("Esc")
            || trimmed.starts_with("Up/Down")
            || trimmed.starts_with("Ctrl+")
            || trimmed.starts_with("Alt+")
        {
            let parts: Vec<&str> = trimmed.splitn(2, "  ").collect();
            let key = parts.first().copied().unwrap_or("").trim();
            let desc = if parts.len() > 1 { parts[1].trim() } else { "" };
            let left_sp = format!("  {:<18}", key);
            let right_len = content_w.saturating_sub(left_sp.chars().count());
            let right_sp = fit_to_width(desc, right_len);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(left_sp, get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker)),
                Span::styled(right_sp, get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker)),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else if trimmed.starts_with('•') || trimmed.starts_with('-') {
            let bullet_text = trimmed.trim_start_matches('•').trim_start_matches('-').trim();
            let full_str = format!("  • {bullet_text}");
            let padded_str = fit_to_width(&full_str, content_w);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(padded_str, get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker)),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else {
            let full_str = format!("  {trimmed}");
            let padded_str = fit_to_width(&full_str, content_w);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(padded_str, get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker)),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        }
    }

    // Bottom blank padding line
    lines.push(Line::from(vec![
        Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
        Span::styled(" ".repeat(inner_w), Style::default().bg(reset_bg)),
        Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
    ]));

    // Bottom border: ╰──────────────────────────────────────────────────────────╯
    let bot_border = format!("╰{}╯", "─".repeat(inner_w));
    lines.push(Line::from(vec![
        Span::styled(bot_border, Style::default().fg(border_c).bg(reset_bg)),
    ]));
}

const RUSTCODE_LOGO: &[&str] = &[
    "                  ▄                   █      ",
    "▄▀▀▀ █   █ ▄▀▀▀▀ ▀█▀▀ ▄▀▀▀▀ ▄▀▀▀▄ ▄▀▀▀█ ▄▀▀▀▄",
    "█    █   █  ▀▀▀▄  █   █     █   █ █   █ █▀▀▀▀",
    "▀     ▀▀▀  ▀▀▀▀    ▀▀  ▀▀▀▀  ▀▀▀   ▀▀▀▀  ▀▀▀▀",
];

fn build_claude_startup_banner(state: &AppState, total_width: usize) -> Vec<Line<'static>> {
    let mut banner = Vec::new();
    let version = env!("CARGO_PKG_VERSION");
    let model_name = model_label(state);
    let cwd_branch = if state.cwd_and_branch.is_empty() {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "rustcode".to_string())
    } else {
        state.cwd_and_branch.clone()
    };

    let box_w = total_width.saturating_sub(2).max(65);
    let inner_w = box_w.saturating_sub(2);
    let left_w = if inner_w >= 90 { 50 } else { (inner_w * 44 / 100).max(30) };
    let right_w = inner_w.saturating_sub(left_w + 1);

    let border_c = COLOR_PRIMARY();
    let primary = COLOR_PRIMARY();
    let text_c = COLOR_TEXT();
    let muted_c = COLOR_MUTED();
    let reset_bg = COLOR_BG();

    // Top border
    let title_str = format!("RustCode v{version}");
    let top_pad = inner_w.saturating_sub(title_str.chars().count() + 3);
    let top_border = format!("╭─ {title_str} {}╮", "─".repeat(top_pad));
    banner.push(Line::from(vec![
        Span::styled(top_border, Style::default().fg(border_c).bg(reset_bg)),
    ]));

    let make_row = |left_str: String, left_style: Style, right_str: String, right_style: Style| -> Line<'static> {
        let l_cell = format!("{:<width$}", left_str, width = left_w);
        let r_cell = format!("{:<width$}", right_str, width = right_w);
        Line::from(vec![
            Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
            Span::styled(l_cell, left_style),
            Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
            Span::styled(r_cell, right_style),
            Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
        ])
    };

    let make_divider_row = |left_str: String, left_style: Style| -> Line<'static> {
        let l_cell = format!("{:<width$}", left_str, width = left_w);
        let r_div = "─".repeat(right_w);
        Line::from(vec![
            Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
            Span::styled(l_cell, left_style),
            Span::styled("├", Style::default().fg(border_c).bg(reset_bg)),
            Span::styled(r_div, Style::default().fg(border_c).bg(reset_bg)),
            Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
        ])
    };

    // Row 0: Blank line top padding
    banner.push(make_row(
        "".to_string(), Style::default().bg(reset_bg),
        "".to_string(), Style::default().bg(reset_bg),
    ));

    // Row 1: Left: Centered "Welcome back!" | Right: "  Tips for getting started"
    let welcome_txt = "Welcome back!";
    let welcome_pad = left_w.saturating_sub(welcome_txt.len()) / 2;
    let left1 = format!("{}{}", " ".repeat(welcome_pad), welcome_txt);
    banner.push(make_row(
        left1, Style::default().fg(text_c).bg(reset_bg).add_modifier(Modifier::BOLD),
        "  Tips for getting started".to_string(), Style::default().fg(primary).bg(reset_bg).add_modifier(Modifier::BOLD),
    ));

    // Row 2: Left: Blank space | Right: "  Run /help to view all slash commands"
    banner.push(make_row(
        "".to_string(), Style::default().bg(reset_bg),
        "  Run /help to view all slash commands".to_string(), Style::default().fg(text_c).bg(reset_bg),
    ));

    // Rows 3..6: 4-line RustCode logo on Left in WHITE
    let logo_width = 45;
    let logo_pad = left_w.saturating_sub(logo_width) / 2;

    // Row 3: Logo line 0 | Right: "  Type @ to mention and link project files"
    let l_line0 = if left_w >= 48 { format!("{}{}", " ".repeat(logo_pad), RUSTCODE_LOGO[0]) } else { "  rustcode".to_string() };
    banner.push(make_row(
        l_line0, Style::default().fg(text_c).bg(reset_bg).add_modifier(Modifier::BOLD),
        "  Type @ to mention and link project files".to_string(), Style::default().fg(text_c).bg(reset_bg),
    ));

    // Row 4: Logo line 1 | Right: Divider Line ────────────────
    let l_line1 = if left_w >= 48 { format!("{}{}", " ".repeat(logo_pad), RUSTCODE_LOGO[1]) } else { "".to_string() };
    banner.push(make_divider_row(
        l_line1, Style::default().fg(text_c).bg(reset_bg).add_modifier(Modifier::BOLD),
    ));

    // Row 5: Logo line 2 | Right: "  Shortcuts & Options"
    let l_line2 = if left_w >= 48 { format!("{}{}", " ".repeat(logo_pad), RUSTCODE_LOGO[2]) } else { "".to_string() };
    banner.push(make_row(
        l_line2, Style::default().fg(text_c).bg(reset_bg).add_modifier(Modifier::BOLD),
        "  Shortcuts & Options".to_string(), Style::default().fg(primary).bg(reset_bg).add_modifier(Modifier::BOLD),
    ));

    // Row 6: Logo line 3 | Right: "  /model select model  ·  /theme switch theme"
    let l_line3 = if left_w >= 48 { format!("{}{}", " ".repeat(logo_pad), RUSTCODE_LOGO[3]) } else { "".to_string() };
    banner.push(make_row(
        l_line3, Style::default().fg(text_c).bg(reset_bg).add_modifier(Modifier::BOLD),
        "  /model select model  ·  /theme switch theme".to_string(), Style::default().fg(muted_c).bg(reset_bg),
    ));

    // Row 7: Left: Blank space | Right: "  Tab autocomplete     ·  Shift+Enter newline"
    banner.push(make_row(
        "".to_string(), Style::default().bg(reset_bg),
        "  Tab autocomplete     ·  Shift+Enter newline".to_string(), Style::default().fg(muted_c).bg(reset_bg),
    ));

    // Row 8: Left: Centered "<model> · <mode>" | Right: "  Built-in MCP tools, search & execution"
    let mode_str = match state.agent_mode {
        crate::config::AgentMode::Build => "build",
        crate::config::AgentMode::Plan => "plan",
    };
    let info_txt = format!("{model_name} · {mode_str}");
    let info_pad = left_w.saturating_sub(info_txt.len()) / 2;
    let left8 = format!("{}{}", " ".repeat(info_pad), info_txt);
    banner.push(make_row(
        left8, Style::default().fg(muted_c).bg(reset_bg),
        "  Built-in MCP tools, search & execution".to_string(), Style::default().fg(muted_c).bg(reset_bg),
    ));

    // Row 9: Left: Centered "<cwd_branch>" | Right: Blank space
    let cwd_pad = left_w.saturating_sub(cwd_branch.len()) / 2;
    let left9 = format!("{}{}", " ".repeat(cwd_pad), cwd_branch);
    banner.push(make_row(
        left9, Style::default().fg(muted_c).bg(reset_bg),
        "".to_string(), Style::default().bg(reset_bg),
    ));

    // Row 10: Blank line bottom padding
    banner.push(make_row(
        "".to_string(), Style::default().bg(reset_bg),
        "".to_string(), Style::default().bg(reset_bg),
    ));

    // Bottom border
    let bot_border = format!("╰{}╯", "─".repeat(inner_w));
    banner.push(Line::from(vec![
        Span::styled(bot_border, Style::default().fg(border_c).bg(reset_bg)),
    ]));

    banner
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

    if state.history.is_empty() {
        lines.push(Line::from(""));
        lines.extend(build_claude_startup_banner(state, inner_area.width as usize));
        lines.push(Line::from(""));
    }

    // Line index where each message's block starts. Messages that render
    // nothing produce a duplicate index; deduped below.
    let mut msg_start_lines: Vec<usize> = Vec::new();

    for (msg_idx, msg) in state.history.iter().enumerate() {
        msg_start_lines.push(lines.len());
        if msg.role == "system" {
            // Hide benign intermediate notices and full compaction summary text from TUI display
            if is_hidden_system_notice(&msg.content) {
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
            let show_tool_details = !matches!(state.verbosity, crate::app::Verbosity::High);
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
                    } else if state.history[i].role == "system"
                        && is_hidden_system_notice(&state.history[i].content)
                    {
                        continue;
                    } else {
                        break;
                    }
                }

                if let Some(a_idx) = assistant_idx {
                    let assistant_msg = &state.history[a_idx];
                    let calls = assistant_msg.resolved_tool_calls(state.active_tool_protocol());
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
                    "manage_task" => "ManageTask".to_string(),
                    "background_task" => "TaskDone".to_string(),
                    other => to_pascal_case(other),
                };
                let arg = if tool_name == "background_task" {
                    msg.content
                        .lines()
                        .next()
                        .and_then(|l| l.strip_prefix("background_task: Task "))
                        .and_then(|l| l.split_whitespace().next())
                        .unwrap_or("")
                        .to_string()
                } else {
                    String::new()
                };
                (action_label, arg)
            };

            let action_len = action.len();
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
            let prefix_len = action_len + 7;
            let max_arg_len = (inner_area.width as usize).saturating_sub(prefix_len);
            let display_arg = if arg.is_empty() {
                String::new()
            } else if arg.chars().count() > max_arg_len {
                let take_len = max_arg_len.saturating_sub(3);
                format!("{}...", arg.chars().take(take_len).collect::<String>())
            } else {
                arg.clone()
            };
            spans.push(Span::styled(
                format!("({display_arg})"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
            lines.push(Line::from(spans));

            if show_tool_details {
                if let Some((ref path, ref content)) = msg.file_preview {
                    lines.extend(render_file_preview(
                        path,
                        content,
                        inner_area.width as usize,
                        show_picker,
                    ));
                } else if let Some(ref diff) = msg.diff {
                    let code_content_width = inner_area.width as usize;
                    lines.extend(render_unified_diff(diff, code_content_width, show_picker));
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
            }
            if state
                .history
                .get(msg_idx + 1)
                .is_some_and(|next| next.role == "user")
            {
                lines.push(Line::from(""));
            }

        } else if msg.role == "user" {
            if msg_idx > 0 {
                push_turn_separator(&mut lines, inner_area.width, show_picker);
            }
            let content_width = (inner_area.width as usize).saturating_sub(4);
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

            for (idx, line_str) in wrapped_lines.into_iter().enumerate() {
                if idx == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "❯ ",
                            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                        ),
                        Span::styled(
                            line_str,
                            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
                        ),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "  ",
                            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                        ),
                        Span::styled(
                            line_str,
                            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                        ),
                    ]));
                }
            }
            lines.push(Line::from(""));
        } else if msg.role == "assistant" {
            let calls = msg.resolved_tool_calls(state.active_tool_protocol());
            let has_following_tool_result = tool_result_follows(&state.history, msg_idx);

            if let Some(tool_call) = calls.first() {
                if !has_following_tool_result {
                    let (action, arg) =
                        format_pi_tool_action(&tool_call.name, &tool_call.arguments);
                    let elapsed_ms = state.generation_start_time.map(|t| t.elapsed().as_millis()).unwrap_or(0);
                    let circle = if (elapsed_ms / 350).is_multiple_of(2) { "○ " } else { "● " };
                    let action_len = action.len();
                    let prefix_len = action_len + 10; // circle (2) + action + "(...)" (5) + margin offset (3)
                    let max_arg_len = (inner_area.width as usize).saturating_sub(prefix_len);
                    let display_arg = if arg.chars().count() > max_arg_len {
                        let take_len = max_arg_len.saturating_sub(3);
                        format!("{}...", arg.chars().take(take_len).collect::<String>())
                    } else {
                        arg.clone()
                    };
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
                            format!("({display_arg})..."),
                            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
                        ),
                    ]));
                }
            }

            let collapsed = !state.expanded_thoughts.contains(&msg_idx);
            render_assistant_message(
                &msg.content,
                &mut lines,
                &mut thought_clicks,
                &mut copy_clicks,
                AssistantRenderOptions {
                    token_usage: msg.token_usage.clone(),
                    response_time_ms: msg.response_time_ms,
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
                        && !m.resolved_tool_calls(state.active_tool_protocol()).is_empty())
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
            let live_ms = state.generation_start_time.map(|t| t.elapsed().as_millis() as u64);
            render_assistant_message(
                &state.current_response,
                &mut lines,
                &mut thought_clicks,
                &mut copy_clicks,
                AssistantRenderOptions {
                    token_usage: None,
                    response_time_ms: live_ms,
                    is_generating: true,
                    viewport_width: inner_area.width,
                    show_picker,
                    thought_collapsed: true,
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

    let filtered_cmds: Vec<&CommandInfo> =
        if state.input_buffer.starts_with('/') && !state.input_buffer.contains(' ') {
            COMMANDS
                .iter()
                .filter(|c| c.name.starts_with(&state.input_buffer))
                .collect()
        } else {
            Vec::new()
        };

    let inner_width = f.area().width.saturating_sub(4).max(1);
    let raw_input_lines = count_input_lines(&state.input_buffer, inner_width as usize) + 3;
    let input_lines = raw_input_lines.min(8);
    let input_height = input_lines + 2;
    let queue_block_height = if state.pending_queue.is_empty() { 0 } else { 3 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .horizontal_margin(0)
        .vertical_margin(1)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(queue_block_height),
            Constraint::Length(input_height),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    render_conversation(f, &chunks, state);
    render_queue_line(f, &chunks, state);
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
        let input_inner = chunks[2].inner(input_margin);
        let popup_height = (filtered_cmds.len() as u16)
            .min(MAX_POPUP_ROWS)
            .min(chunks[2].y);
        let popup_y = chunks[2].y.saturating_sub(popup_height);
        let popup_area =
            ratatui::layout::Rect::new(input_inner.x, popup_y, input_inner.width, popup_height);
        render_popup_menu(f, state, &filtered_cmds, popup_area);
    } else if !at_files.is_empty() {
        let input_inner = chunks[2].inner(input_margin);
        let popup_height = at_files.len().min(8) as u16;
        let popup_y = chunks[2].y.saturating_sub(popup_height);
        let popup_area =
            ratatui::layout::Rect::new(input_inner.x, popup_y, input_inner.width, popup_height);
        render_at_popup_menu(f, state, &at_files, popup_area);
    }

    let input_box_area = chunks[2];

    if state.show_model_picker {
        render_model_picker_modal(f, state, input_box_area);
    }

    if state.show_theme_picker {
        render_theme_picker_modal(f, state, input_box_area);
    }

    if state.show_command_picker {
        render_command_picker_modal(f, state, input_box_area);
    }

    if state.show_history_picker {
        render_history_picker_modal(f, state, input_box_area);
    }

    if state.show_mcp_config {
        render_mcp_config_modal(f, state, input_box_area);
    }

    if state.status == AppStatus::AwaitingToolConfirmation {
        render_tool_confirmation_modal(f, state, input_box_area);
    }

    if state.status == AppStatus::AwaitingQuestion {
        render_question_modal(f, state, input_box_area);
    }

    if state.status == AppStatus::VerbosityPicker {
        render_verbosity_picker_modal(f, state, input_box_area);
    }

    if state.status == AppStatus::ThinkingPicker {
        render_thinking_picker_modal(f, state, input_box_area);
    }

    if state.status == AppStatus::ProtocolPicker {
        render_protocol_picker_modal(f, state, input_box_area);
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
    let bg = COLOR_BG();
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
mod tests;
