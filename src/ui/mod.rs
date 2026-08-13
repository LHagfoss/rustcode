mod highlight;
mod lru;
mod markdown;
mod modals;
pub(crate) mod scrollback;
mod tool_result;

use highlight::{
    highlight_code_block, highlight_code_line, highlight_diff_line, render_unified_diff,
    wrap_code_spans,
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
use tool_result::render_tool_result;

use crate::app::activity::{ActivityKind, ActivitySnapshot, classify_activity};
use crate::app::{AppState, AppStatus, ChatMessage, NoticeKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
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
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
    is_generating: bool,
    viewport_width: u16,
    show_picker: bool,
    last_copy_text: Option<(String, std::time::Instant)>,
}

fn truncate_thought_preview(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let content_width = max_width - 1;
    let mut result = String::new();
    let mut width = 0;
    for c in text.chars() {
        let char_width = c.width().unwrap_or(0);
        if width + char_width > content_width {
            break;
        }
        result.push(c);
        width += char_width;
    }
    result.push('…');
    result
}

fn is_reasoning_preamble(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    const PREAMBLE_STARTS: &[&str] = &[
        "Okay,",
        "First,",
        "The project",
        "Thinking:",
        "Thought:",
        "thought",
        "Thought",
        "Let's",
        "I should",
        "I need to",
        "Reasoning:",
        "Plan:",
        "Step 1",
        "• Okay",
        "• First",
        "• The project",
    ];
    PREAMBLE_STARTS.iter().any(|prefix| trimmed.starts_with(prefix))
}

fn split_thought_blocks(content: &str) -> (String, Option<String>) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";

    let mut answer = String::new();
    let mut thought_preview = None;
    let mut rest = content;
    let mut thoughts_collected = Vec::new();
    let mut is_first_segment = true;

    loop {
        let open_idx = rest.find(OPEN);
        let close_idx = rest.find(CLOSE);

        match (open_idx, close_idx) {
            (Some(o_idx), Some(c_idx)) if o_idx < c_idx => {
                let preamble = &rest[..o_idx];
                if !preamble.trim().is_empty() {
                    if is_first_segment && is_reasoning_preamble(preamble) {
                        thoughts_collected.push(preamble);
                    } else {
                        answer.push_str(preamble);
                    }
                }
                let thought = &rest[o_idx + OPEN.len()..c_idx];
                thoughts_collected.push(thought);
                rest = &rest[c_idx + CLOSE.len()..];
                is_first_segment = false;
            }
            (Some(o_idx), None) => {
                let preamble = &rest[..o_idx];
                if !preamble.trim().is_empty() {
                    if is_first_segment && is_reasoning_preamble(preamble) {
                        thoughts_collected.push(preamble);
                    } else {
                        answer.push_str(preamble);
                    }
                }
                let thought = &rest[o_idx + OPEN.len()..];
                thoughts_collected.push(thought);
                rest = "";
                break;
            }
            (None, Some(c_idx)) => {
                let thought = &rest[..c_idx];
                if is_first_segment || is_reasoning_preamble(thought) {
                    thoughts_collected.push(thought);
                } else {
                    answer.push_str(thought);
                }
                rest = &rest[c_idx + CLOSE.len()..];
                is_first_segment = false;
            }
            (Some(_), Some(c_idx)) => {
                let thought = &rest[..c_idx];
                if is_first_segment || is_reasoning_preamble(thought) {
                    thoughts_collected.push(thought);
                } else {
                    answer.push_str(thought);
                }
                rest = &rest[c_idx + CLOSE.len()..];
                is_first_segment = false;
            }
            (None, None) => {
                break;
            }
        }
    }

    if !rest.is_empty() {
        answer.push_str(rest);
    }

    for thought_str in thoughts_collected {
        if thought_preview.is_none() {
            thought_preview = thought_str
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with("<think>"))
                .map(str::to_owned);
        }
    }

    (answer.trim().to_string(), thought_preview)
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
    copy_registry: &mut Vec<(usize, String)>,
    options: AssistantRenderOptions,
) {
    let AssistantRenderOptions {
        token_usage,
        response_time_ms,
        thought_time_ms,
        thought_tokens,
        is_generating,
        viewport_width,
        show_picker,
        last_copy_text,
    } = options;
    let display_content = if let Some(idx) = content.find("\n\n[harness verification:") {
        &content[..idx]
    } else if let Some(idx) = content.find("[harness verification:") {
        &content[..idx]
    } else {
        content
    };

    let (main_content_owned, thought_preview) = split_thought_blocks(display_content);
    let main_content = main_content_owned.as_str();

    if let Some(first_line) = thought_preview {
        let time_str = thought_time_ms.or(response_time_ms).map(|ms| {
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
        let tokens_str = thought_tokens
            .or_else(|| token_usage.as_ref().map(|u| u.total_tokens))
            .map(|tokens| {
                if tokens >= 1000 {
                    format!("{:.1}k tokens", tokens as f32 / 1000.0)
                } else {
                    format!("{} tokens", tokens)
                }
            });

        let thought_meta = match (time_str, tokens_str) {
            (Some(t), Some(k)) => format!("Thought for {t}, {k}"),
            (Some(t), None) => format!("Thought for {t}"),
            (None, Some(k)) => format!("Thought for {k}"),
            (None, None) => "Thought".to_string(),
        };

        let preview_width = (viewport_width as usize).saturating_sub(2).min(64);
        let preview = truncate_thought_preview(&first_line, preview_width);

        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::from(""));
        }

        lines.push(Line::from(vec![
            Span::styled(
                "▸ ",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                thought_meta,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
        ]));

        if !preview.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                format!("  {preview}"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            )]));
        }
    }

    let main_content = strip_rendered_tool_blocks(main_content);
    if !main_content.trim().is_empty() || is_generating {
        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::from(""));
        }
        let content_width = (viewport_width as usize).saturating_sub(2).max(10);
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
                processed_lines.push((false, raw_line.to_string()));
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
        let mut emitted_assistant_gutter = false;
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
                let markdown_lines = render_markdown(
                    &normal_text,
                    content_width,
                    show_picker,
                    !is_generating,
                );
                for markdown_line in markdown_lines {
                    if markdown_line.spans.is_empty() {
                        lines.push(markdown_line);
                        continue;
                    }

                    let prefix = if emitted_assistant_gutter { "  " } else { "• " };
                    emitted_assistant_gutter = true;
                    let mut spans = vec![Span::styled(
                        prefix,
                        get_themed_style(
                            COLOR_PRIMARY(),
                            COLOR_BG(),
                            Modifier::BOLD,
                            show_picker,
                        ),
                    )];
                    spans.extend(markdown_line.spans);
                    lines.push(Line::from(spans));
                }
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

fn current_tps(state: &AppState) -> f64 {
    if state.status == AppStatus::Streaming
        && let Some(ref tracker) = state.stream_tracker
    {
        return tracker.snapshot().0;
    }
    0.0
}

fn format_tps_value(tps: f64) -> String {
    format!("{tps:.1}")
}

fn format_tps_info(tps: f64) -> String {
    format!("Tps: {}", format_tps_value(tps))
}

fn format_token_count(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}K", tokens as f32 / 1000.0)
    } else {
        tokens.to_string()
    }
}

fn format_context_info(
    total_tokens: u32,
    context_window: u32,
    cached_tokens: Option<u32>,
) -> String {
    let pct = if context_window == 0 {
        0.0
    } else {
        ((total_tokens as f32 / context_window as f32) * 100.0).min(100.0)
    };
    let cached = cached_tokens
        .filter(|cached| *cached > 0)
        .map(|cached| format!(" ({} cached)", format_token_count(cached)))
        .unwrap_or_default();
    format!(
        "Context: {}{} ({pct:.0}%)",
        format_token_count(total_tokens),
        cached,
    )
}

fn input_footer_hint_text() -> &'static str {
    "Ctrl+P commands"
}

fn context_usage(state: &AppState) -> (u32, Option<u32>) {
    if let Some(usage) = &state.current_token_usage {
        return (usage.total_tokens, usage.cached_tokens);
    }

    if let Some(usage) = state
        .history
        .iter()
        .rev()
        .find_map(|message| message.token_usage.as_ref())
    {
        return (usage.total_tokens, usage.cached_tokens);
    }

    let chars: usize = state
        .history
        .iter()
        .map(|message| message.content.len())
        .sum();
    ((chars / 4) as u32, None)
}

fn format_input_status_text(state: &AppState) -> String {
    let (total_tokens, cached_tokens) = context_usage(state);
    let mut status = vec![
        format!("Auto-Confirm: {}", state.auto_confirm_status_text()),
        format_context_info(total_tokens, state.active_context_window(), cached_tokens),
        format_tps_info(current_tps(state)),
    ];

    if let Some(quota) = state.model_quota_remaining {
        status.push(format!("Quota: {quota:.0}%"));
    }

    status.push(input_footer_hint_text().to_string());
    status.join("  ")
}

fn activity_status_label(state: &AppState) -> String {
    let activity = classify_activity(&state.status, &state.running_tools);
    if activity.kind == ActivityKind::Working {
        return "Working".to_string();
    }
    activity.label
}

fn blend_rgb(c1: (u8, u8, u8), c2: (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let f = factor.clamp(0.0, 1.0);
    let r = (c1.0 as f32 * f + c2.0 as f32 * (1.0 - f)) as u8;
    let g = (c1.1 as f32 * f + c2.1 as f32 * (1.0 - f)) as u8;
    let b = (c1.2 as f32 * f + c2.2 as f32 * (1.0 - f)) as u8;
    (r, g, b)
}

static SHIMMER_START: OnceLock<Instant> = OnceLock::new();

fn shimmer_rgb(color: Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    }
}

fn shimmer_spans_at(text: &str, elapsed: Duration) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos = ((elapsed.as_secs_f32() % sweep_seconds) / sweep_seconds * period as f32) as isize;
    let band_half_width = 5.0f32;

    let base_rgb = shimmer_rgb(COLOR_MUTED(), (128, 128, 128));
    let highlight_rgb = shimmer_rgb(COLOR_TEXT(), (255, 255, 255));

    chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let i_pos = i as isize + padding as isize;
            let dist = (i_pos - pos).abs() as f32;
            let t = if dist <= band_half_width {
                0.5 * (1.0 + (std::f32::consts::PI * (dist / band_half_width)).cos())
            } else {
                0.0
            };
            let (r, g, b) = blend_rgb(highlight_rgb, base_rgb, t * 0.9);
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

fn shimmer_spans(text: &str, _show_picker: bool) -> Vec<Span<'static>> {
    let elapsed = SHIMMER_START.get_or_init(Instant::now).elapsed();
    shimmer_spans_at(text, elapsed)
}

fn fmt_elapsed_compact(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        format!("{elapsed_secs}s")
    } else if elapsed_secs < 3600 {
        let mins = elapsed_secs / 60;
        let secs = elapsed_secs % 60;
        format!("{mins}m {secs:02}s")
    } else {
        let hours = elapsed_secs / 3600;
        let mins = (elapsed_secs % 3600) / 60;
        let secs = elapsed_secs % 60;
        format!("{hours}h {mins:02}m {secs:02}s")
    }
}

fn activity_status_line(state: &AppState, show_picker: bool) -> Line<'static> {
    let activity = if state.working_status_pending {
        ActivitySnapshot {
            kind: ActivityKind::Working,
            label: "Working".to_string(),
            detail: None,
            animated: true,
        }
    } else {
        classify_activity(&state.status, &state.running_tools)
    };
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

    let mut spans = vec![Span::raw(" ")];

    let bullet_symbol = match activity.kind {
        ActivityKind::ActionRequired => "!",
        ActivityKind::Ready => "◦",
        _ => "•",
    };
    let bullet_color = match activity.kind {
        ActivityKind::ActionRequired => Color::Yellow,
        ActivityKind::Ready => COLOR_MUTED(),
        _ => COLOR_PRIMARY(),
    };
    spans.push(Span::styled(
        bullet_symbol,
        get_themed_style(bullet_color, COLOR_BG(), Modifier::BOLD, show_picker),
    ));
    spans.push(Span::raw(" "));

    let label_text = if state.working_status_pending {
        "Working".to_string()
    } else {
        activity_status_label(state)
    };
    if matches!(
        activity.kind,
        ActivityKind::Working | ActivityKind::RunningTool
    ) {
        spans.extend(shimmer_spans(&label_text, show_picker));
    } else {
        spans.push(Span::styled(
            label_text,
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
    }

    let detail = if activity.kind == ActivityKind::ActionRequired {
        action_detail
    } else {
        activity.detail.clone()
    };
    if let Some(detail) = detail {
        spans.push(Span::styled(
            format!(" · {detail}"),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }

    if matches!(
        activity.kind,
        ActivityKind::Working | ActivityKind::RunningTool
    ) && let Some(started) = state.generation_start_time
    {
        spans.push(Span::styled(
            format!(" ({})", fmt_elapsed_compact(started.elapsed().as_secs())),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }

    if matches!(
        activity.kind,
        ActivityKind::Queued | ActivityKind::Working | ActivityKind::RunningTool
    ) {
        spans.push(Span::styled(
            " · esc ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        spans.push(Span::styled(
            "interrupt",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
    }

    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Maximum queued user prompts previewed above the composer.
const MAX_QUEUE_PREVIEW_ROWS: usize = 3;

fn queued_user_prompts(state: &AppState) -> Vec<&str> {
    state
        .pending_queue
        .iter()
        .filter(|prompt| !prompt.starts_with("__task_wakeup__:"))
        .rev()
        .take(MAX_QUEUE_PREVIEW_ROWS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn queue_preview_height(state: &AppState) -> u16 {
    let rows = queued_user_prompts(state).len();
    if rows == 0 { 0 } else { rows as u16 + 1 }
}

fn truncate_queue_prompt(prompt: &str, max_width: usize) -> String {
    if prompt.width() <= max_width {
        return prompt.to_owned();
    }
    let ellipsis_width = "…".width();
    let mut text = String::new();
    let mut width = 0;
    for ch in prompt.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width + ellipsis_width > max_width {
            break;
        }
        text.push(ch);
        width += ch_width;
    }
    text.push('…');
    text
}

/// Shows the most recent queued user prompts directly above the input box.
/// Internal wakeups stay queued but never consume composer space or leak into
/// this transcript-like preview.
fn render_queue_line(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) {
    let prompts = queued_user_prompts(state);
    if prompts.is_empty() {
        return;
    }
    let block = chunks[1];
    if block.height == 0 {
        return;
    }
    let show_picker = state.modal_open();
    let queued_count = state
        .pending_queue
        .iter()
        .filter(|prompt| !prompt.starts_with("__task_wakeup__:"))
        .count();
    let header = Line::from(Span::styled(
        format!("queued ({queued_count}) · ↑ edit last"),
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    ));
    f.render_widget(
        Paragraph::new(header).style(Style::default().bg(COLOR_BG())),
        ratatui::layout::Rect::new(block.x, block.y, block.width, 1),
    );

    for (row, prompt) in prompts.into_iter().enumerate() {
        let prefix = "  › ";
        let preview = truncate_queue_prompt(
            prompt,
            (block.width as usize).saturating_sub(prefix.width()),
        );
        let line = Line::from(vec![
            Span::styled(
                prefix,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                preview,
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(COLOR_BG())),
            ratatui::layout::Rect::new(block.x, block.y + row as u16 + 1, block.width, 1),
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
        Span::styled(
            " ✦ ",
            Style::default()
                .fg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "rustcode",
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]);

    let title_right = Line::from(vec![
        Span::styled(
            format!(" {mode_label_str} "),
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{model_str}] "),
            Style::default().fg(COLOR_MUTED()),
        ),
    ]);

    let status_line = Line::from(vec![Span::styled(
        format!(" {} ", format_input_status_text(state)),
        Style::default().fg(COLOR_MUTED()),
    )]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(title_left)
        .title(title_right.alignment(ratatui::layout::Alignment::Right))
        .title_bottom(status_line.alignment(ratatui::layout::Alignment::Right));

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
            let placeholder_text =
                "Ask a question, request code changes, or type / for commands...";
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
    copy_wrapped_rows: Vec<(u16, String)>,
    msg_wrapped_rows: Vec<u16>,
    total_wrapped_lines: u16,
}

type RenderedConversation = (Vec<Line<'static>>, Vec<(u16, String)>, Vec<u16>, u16);

#[derive(PartialEq, Clone)]
struct ChatKey {
    hist_len: usize,
    total_len: usize,
    last_len: usize,
    history_display_start: usize,
    width: u16,
    show_picker: bool,
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
        history_display_start: state.history_display_start,
        width,
        show_picker,
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

fn tool_result_is_hidden(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "use_skill"
            | "set_goal"
            | "todo_write"
            | "spawn_agent"
            | "send_agent"
            | "complete_task"
            | "ask_question"
    )
}

fn tool_result_action(state: &AppState, message_index: usize, tool_name: &str) -> (String, String) {
    let tool_call_id = state
        .history
        .get(message_index)
        .and_then(|message| message.tool_call_id.as_deref());
    let structured_call = state.history[..message_index]
        .iter()
        .rev()
        .filter(|message| message.role == "assistant")
        .flat_map(|message| message.tool_calls.iter().rev())
        .find(|call| match tool_call_id {
            Some(id) => call.id == id,
            None => call.name == tool_name,
        });

    if let Some(call) = structured_call {
        let args = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
        return format_pi_tool_action(&call.name, &args);
    }

    let parsed_call = state.history[..message_index]
        .iter()
        .rev()
        .filter(|message| message.role == "assistant")
        .flat_map(|message| {
            message
                .resolved_tool_calls(state.active_tool_protocol())
                .into_iter()
                .rev()
        })
        .find(|call| call.name == tool_name);
    if let Some(call) = parsed_call {
        return format_pi_tool_action(&call.name, &call.arguments);
    }

    format_pi_tool_action(tool_name, &serde_json::Value::Null)
}

fn tool_result_status(message: &ChatMessage, tool_name: &str, result: &str) -> (bool, String) {
    if let Some(record) = &message.tool_result {
        return match record.exit_code {
            Some(code) => (record.success, format!("exit {code}")),
            None if record.success => (true, "completed".to_owned()),
            None => (false, "failed".to_owned()),
        };
    }

    if tool_name == "run_command" {
        if let Some(code) = result.lines().find_map(|line| {
            line.strip_prefix("exit code: ")
                .and_then(|code| code.trim().parse::<i32>().ok())
        }) {
            return (code == 0, format!("exit {code}"));
        }
    }

    let failed = result
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| {
            let line = line.trim_start().to_ascii_lowercase();
            line.starts_with("error") || line.starts_with('✗')
        });
    if failed {
        (false, "failed".to_owned())
    } else {
        (true, "completed".to_owned())
    }
}

fn indent_tool_result_body(lines: Vec<Line<'static>>, tool_name: &str) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .filter(|line| {
            tool_name != "run_command"
                || !line
                    .spans
                    .iter()
                    .any(|span| span.content.trim_start().starts_with('✗'))
        })
        .map(|line| {
            if line.spans.is_empty() {
                return line;
            }
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::raw("  "));
            spans.extend(line.spans);
            let mut indented = Line::from(spans);
            indented.style = line.style;
            indented.alignment = line.alignment;
            indented
        })
        .collect()
}

fn render_committed_tool_result(
    state: &AppState,
    message_index: usize,
    tool_name: &str,
    result: &str,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    if matches!(state.verbosity, crate::app::Verbosity::High) || tool_result_is_hidden(tool_name) {
        return Vec::new();
    }

    let message = &state.history[message_index];
    let (action, target) = tool_result_action(state, message_index, tool_name);
    let (success, status) = tool_result_status(message, tool_name, result);
    let status_color = if success {
        COLOR_GREEN()
    } else {
        Color::Rgb(229, 123, 123)
    };

    let mut header = vec![
        Span::styled(
            "• ",
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            action,
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
    ];
    if !target.is_empty() && target != "?" {
        header.push(Span::styled(
            format!(" · {target}"),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }

    let icon = if success { "✓" } else { "✗" };
    let mut lines = vec![
        Line::from(header),
        Line::from(Span::styled(
            format!("  └ {icon} {status}"),
            get_themed_style(status_color, COLOR_BG(), Modifier::BOLD, show_picker),
        )),
    ];
    let body = cached_tool_result(
        tool_name,
        result,
        width as usize,
        &state.verbosity,
        show_picker,
    );
    lines.extend(indent_tool_result_body(body, tool_name));
    lines
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

fn push_new_chat_separator<'a>(lines: &mut Vec<Line<'a>>, width: u16, show_picker: bool) {
    let label = " ✨ NEW CHAT ";
    let remaining = (width as usize).saturating_sub(label.width());
    let left = remaining / 2;
    let right = remaining - left;
    let style = get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
    lines.push(Line::from(vec![
        Span::styled("─".repeat(left), style),
        Span::styled(label, style),
        Span::styled("─".repeat(right), style),
    ]));
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
    next_visible_message(history, assistant_index).is_some_and(|message| message.role == "tool")
}

fn next_visible_message(history: &[ChatMessage], index: usize) -> Option<&ChatMessage> {
    history
        .iter()
        .skip(index + 1)
        .find(|message| {
            !((message.role == "system" || message.role == "assistant")
                && is_hidden_system_notice(&message.content))
        })
}

fn tool_result_needs_assistant_gap(history: &[ChatMessage], tool_index: usize) -> bool {
    next_visible_message(history, tool_index).is_some_and(|message| message.role == "assistant")
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
    lines.push(Line::from(vec![Span::styled(
        top_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));

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
                Span::styled(
                    padded_header,
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else if trimmed.starts_with('/') {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let cmd_name = parts.first().copied().unwrap_or("");
            let cmd_desc = if parts.len() > 1 {
                parts[1..].join(" ")
            } else {
                String::new()
            };
            let left_sp = format!("  {:<18}", cmd_name);
            let right_len = content_w.saturating_sub(left_sp.chars().count());
            let right_sp = fit_to_width(&cmd_desc, right_len);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(
                    left_sp,
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    right_sp,
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
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
                Span::styled(
                    left_sp,
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    right_sp,
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else if trimmed.starts_with('•') || trimmed.starts_with('-') {
            let bullet_text = trimmed
                .trim_start_matches('•')
                .trim_start_matches('-')
                .trim();
            let full_str = format!("  • {bullet_text}");
            let padded_str = fit_to_width(&full_str, content_w);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(
                    padded_str,
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else {
            let full_str = format!("  {trimmed}");
            let padded_str = fit_to_width(&full_str, content_w);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(
                    padded_str,
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
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
    lines.push(Line::from(vec![Span::styled(
        bot_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));
}



pub(crate) fn build_claude_startup_banner(
    state: &AppState,
    total_width: usize,
    _max_height: usize,
) -> Vec<Line<'static>> {
    let mut banner = Vec::new();
    let version = env!("CARGO_PKG_VERSION");
    let model_name = model_label(state);

    let box_w = total_width.saturating_sub(2).max(65);
    let inner_w = box_w.saturating_sub(2);
    let left_w = if inner_w >= 90 {
        50
    } else {
        (inner_w * 48 / 100).max(30)
    };
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
    banner.push(Line::from(vec![Span::styled(
        top_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));

    let make_row = |left_str: String,
                    left_style: Style,
                    right_str: String,
                    right_style: Style|
     -> Line<'static> {
        let l_cell = fit_to_width(&left_str, left_w);
        let r_cell = fit_to_width(&right_str, right_w);
        Line::from(vec![
            Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
            Span::styled(l_cell, left_style),
            Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
            Span::styled(r_cell, right_style),
            Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
        ])
    };

    let mode_str = match state.agent_mode {
        crate::config::AgentMode::Build => "build",
        crate::config::AgentMode::Plan => "plan",
    };
    let info_txt = format!("{model_name} · {mode_str}");

    // Row 1: Left: Centered "Welcome back!" | Right: "  Tips for getting started"
    let welcome_txt = "Welcome back!";
    let welcome_pad = left_w.saturating_sub(welcome_txt.len()) / 2;
    let left1 = format!("{}{}", " ".repeat(welcome_pad), welcome_txt);
    banner.push(make_row(
        left1,
        Style::default()
            .fg(text_c)
            .bg(reset_bg)
            .add_modifier(Modifier::BOLD),
        "  Tips for getting started".to_string(),
        Style::default()
            .fg(primary)
            .bg(reset_bg)
            .add_modifier(Modifier::BOLD),
    ));

    // Row 2: Left: Centered "<model> · <mode>" | Right: "  Run /help to view all slash commands"
    let info_pad = left_w.saturating_sub(info_txt.len()) / 2;
    let left_info = format!("{}{}", " ".repeat(info_pad), info_txt);
    banner.push(make_row(
        left_info,
        Style::default().fg(muted_c).bg(reset_bg),
        "  Run /help to view all slash commands".to_string(),
        Style::default().fg(text_c).bg(reset_bg),
    ));

    // Bottom border
    let bot_border = format!("╰{}╯", "─".repeat(inner_w));
    banner.push(Line::from(vec![Span::styled(
        bot_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));

    // Padding below welcome message
    banner.push(Line::from(""));

    banner
}

fn conversation_area_height(content_height: u16, available_height: u16) -> u16 {
    if available_height == 0 {
        return 0;
    }
    content_height.min(available_height)
}

/// Render only the mutable portion of the current turn. Completed history is
/// deliberately excluded: it will be committed to terminal scrollback.
pub(crate) fn render_live_tail(
    state: &AppState,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let has_conversation = state
        .history
        .iter()
        .any(|message| matches!(message.role.as_str(), "user" | "assistant"));
    if !has_conversation
        && state.current_response.is_empty()
        && matches!(state.status, AppStatus::Idle)
        && !state.working_status_pending
    {
        return build_claude_startup_banner(state, width as usize, height as usize);
    }

    let tail = scrollback::mutable_stream_text(&state.current_response);
    let mut lines = Vec::new();
    let mut copy_clicks = Vec::new();

    if !tail.is_empty() {
        let parsed_tool = crate::tools::parse_tool_call(&tail, state.active_tool_protocol());
        let is_tool_syntax = crate::tools::is_tool_call_start(&tail);
        let should_hide_stream = match parsed_tool {
            Some(ref tool_call) => !crate::tools::is_code_editing_tool(&tool_call.name),
            None => is_tool_syntax,
        };

        if !should_hide_stream {
            render_assistant_message(
                &tail,
                &mut lines,
                &mut copy_clicks,
                AssistantRenderOptions {
                    token_usage: None,
                    response_time_ms: state
                        .generation_start_time
                        .map(|started| started.elapsed().as_millis() as u64),
                    thought_time_ms: None,
                    thought_tokens: None,
                    is_generating: true,
                    viewport_width: width,
                    show_picker: false,
                    last_copy_text: None,
                },
            );
        }
    }

    if matches!(state.status, AppStatus::Streaming | AppStatus::Queued)
        || state.working_status_pending
        || !state.running_tools.is_empty()
    {
        lines.push(activity_status_line(state, false));
        lines.push(Line::from(""));
        lines.push(Line::from(""));
    }

    lines.into_iter().map(|line| own_line(&line)).collect()
}

/// Render one finalized history entry for insertion into terminal scrollback.
pub(crate) fn render_committed_history_block(
    state: &AppState,
    message_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(message) = state.history.get(message_index) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let show_picker = false;

    match message.role.as_str() {
        "user" => {
            let content = collapse_image_markers(&message.content);
            for (index, text) in content.lines().enumerate() {
                let prefix = if index == 0 { "❯ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(
                        prefix,
                        get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                    ),
                    Span::styled(
                        text.to_owned(),
                        get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }
        "assistant" => {
            if is_hidden_system_notice(&message.content) {
                return Vec::new();
            }
            return render_committed_assistant_text_with_metrics(
                &message.content,
                width,
                message.token_usage.clone(),
                message.response_time_ms,
                message.thought_time_ms,
                message.thought_tokens,
            );
        }
        "tool" => {
            let tool_name = resolve_tool_result_name(
                None,
                message
                    .tool_result
                    .as_ref()
                    .map(|result| result.tool_name.as_str()),
                &message.content,
            )
            .unwrap_or_else(|| "Tool".to_owned());
            let result = message
                .content
                .split_once(": ")
                .map(|(_, result)| result)
                .unwrap_or(&message.content);
            let tool_lines = render_committed_tool_result(
                state,
                message_index,
                &tool_name,
                result,
                width,
                show_picker,
            );
            if !tool_lines.is_empty() {
                lines.extend(tool_lines);
                lines.push(Line::from(""));
            }
        }
        "system" if !is_hidden_system_notice(&message.content) => {
            render_status_panel(&message.content, width, show_picker, &mut lines);
            lines.push(Line::from(""));
        }
        _ => {}
    }

    lines.into_iter().map(|line| own_line(&line)).collect()
}

pub(crate) fn render_committed_assistant_text(
    _state: &AppState,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    render_committed_assistant_text_with_metrics(content, width, None, None, None, None)
}

fn render_committed_assistant_text_with_metrics(
    content: &str,
    width: u16,
    token_usage: Option<crate::app::TokenUsage>,
    response_time_ms: Option<u64>,
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut copy_clicks = Vec::new();
    render_assistant_message(
        content,
        &mut lines,
        &mut copy_clicks,
        AssistantRenderOptions {
            token_usage,
            response_time_ms,
            thought_time_ms,
            thought_tokens,
            is_generating: false,
            viewport_width: width,
            show_picker: false,
            last_copy_text: None,
        },
    );
    lines.into_iter().map(|line| own_line(&line)).collect()
}

fn render_live_conversation(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &mut AppState) {
    let area = chunks[0].inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let lines = render_live_tail(state, area.width, area.height);
    state.conversation_content_height = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(area.width) as u16;
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(COLOR_BG())),
        area,
    );
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
    let raw_input_lines = count_input_lines(&state.input_buffer, inner_width as usize);
    let input_lines = raw_input_lines.min(8);
    let input_height = input_lines + 2;
    let queue_block_height = queue_preview_height(state);

    let max_chat_height = f
        .area()
        .height
        .saturating_sub(2)
        .saturating_sub(queue_block_height)
        .saturating_sub(input_height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .horizontal_margin(0)
        .vertical_margin(1)
        .constraints([
            Constraint::Length(max_chat_height),
            Constraint::Length(queue_block_height),
            Constraint::Length(input_height),
        ])
        .split(f.area());

    render_live_conversation(f, &chunks, state);

    render_queue_line(f, &chunks, state);
    let input_margin = render_input(f, &chunks, state);

    let (_, at_query) = crate::app::get_at_word_query(&state.input_buffer, state.cursor_position)
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

    render_notice(f, state);
    // The final Working row is only needed for the frame that paints the
    // finalized reply. New turns clear this at queue start if another prompt
    // is already waiting.
    state.working_status_pending = false;
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

#[cfg(test)]
mod tests;
