mod composer;
mod events;
mod frame_requester;
mod highlight;
mod history_cell;
mod keymap;
mod lru;
mod markdown;
mod modals;
pub(crate) mod render_snapshot;
mod terminal_runtime;
mod transcript;

pub(crate) use composer::{Composer, ComposerAction};
pub(crate) use events::{TuiEvent, TuiEventStream};
pub(crate) use frame_requester::{FrameRequester, FrameStream};
pub(crate) use history_cell::TranscriptState;
use history_cell::{HistoryCell, is_live_tool_call_visible};
pub(crate) use transcript::TranscriptModel;
pub(crate) mod scrollback;
mod tool_result;
pub(crate) use terminal_runtime::TerminalRuntime;

use highlight::{
    highlight_code_block, highlight_code_line, highlight_diff_line, highlight_shell_command,
    render_unified_diff, wrap_code_spans,
};
use markdown::{
    push_wrapped_with_continuation, render_markdown, unwrap_markdown_table_fences,
    wrap_styled_spans,
};
pub use modals::{PALETTE_ITEMS, PaletteItem};
pub mod theme;
pub(crate) use modals::{
    approval_event_for_key, question_answer_event, question_cancel_event,
    question_custom_answer_event,
};
use modals::{
    question_height, render_at_popup_menu, render_command_picker_modal, render_context_modal,
    render_effort_picker_modal, render_history_picker_modal, render_mcp_config_modal,
    render_model_picker_modal, render_popup_menu, render_protocol_picker_modal,
    render_question_modal, render_subagent_picker_modal, render_theme_picker_modal,
    render_thinking_picker_modal, render_tool_confirmation_modal, render_update_prompt_modal,
    render_verbosity_picker_modal, render_yolo_picker_modal, tool_confirmation_height,
};
use tool_result::render_tool_result;

use crate::app::activity::{ActivityKind, classify_activity, classify_live_tools};
use crate::app::{AppState, AppStatus, ChatMessage};
use crate::inline_terminal::Frame;
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
};
use render_snapshot::RenderSnapshot;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

fn safe_byte_index(s: &str, byte_pos: usize) -> usize {
    let mut position = byte_pos.min(s.len());
    while !s.is_char_boundary(position) {
        position = position.saturating_sub(1);
    }
    position
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
#[allow(non_snake_case, dead_code)]
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
#[allow(non_snake_case, dead_code)]
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

pub use crate::app::suggestion::CommandInfo;

fn get_themed_style(fg: Color, bg: Color, modifier: Modifier, _show_picker: bool) -> Style {
    Style::default().fg(fg).bg(bg).add_modifier(modifier)
}

/// Collapse pasted image and long-text markers into compact display chips.
/// The raw markers stay in the underlying buffer / history so
/// `parse_multimodal_content` can still attach the content when the message is
/// sent — this only affects what the user sees.
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;

thread_local! {
    static MARKER_CACHE: RefCell<(u64, String)> = const { RefCell::new((0, String::new())) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CollapsedMarker {
    Image,
    PastedText,
}

fn collapsed_marker_segments(text: &str) -> Vec<(String, Option<CollapsedMarker>)> {
    const MARK_IMG: &str = "![image](file://";
    const MARK_PASTE: &str = "<!--PASTE:";
    let mut segments = Vec::new();
    let mut rest = text;
    let mut img_n = 0;
    let mut paste_n = 0;

    while !rest.is_empty() {
        let next_img = rest.find(MARK_IMG);
        let next_paste = rest.find(MARK_PASTE);
        let (idx, is_image) = match (next_img, next_paste) {
            (None, None) => {
                segments.push((rest.to_owned(), None));
                break;
            }
            (Some(idx), None) => (idx, true),
            (None, Some(idx)) => (idx, false),
            (Some(img_idx), Some(paste_idx)) => (img_idx, img_idx < paste_idx),
        };
        if idx > 0 {
            segments.push((rest[..idx].to_owned(), None));
        }

        if is_image {
            let after = &rest[idx + MARK_IMG.len()..];
            let Some(close) = after.find(')') else {
                segments.push((rest[idx..].to_owned(), None));
                break;
            };
            img_n += 1;
            segments.push((format!("[Image #{img_n}]"), Some(CollapsedMarker::Image)));
            rest = &after[close + 1..];
        } else {
            let after = &rest[idx + MARK_PASTE.len()..];
            let Some(end) = after.find("-->") else {
                segments.push((rest[idx..].to_owned(), None));
                break;
            };
            let payload = &after[..end];
            paste_n += 1;
            let label = if let Some((len_str, body)) = payload.split_once(':') {
                let len_num: usize = len_str.parse().unwrap_or(body.len());
                format!("[Pasted Text #{paste_n} ({len_num} chars)]")
            } else {
                format!("[Pasted Text #{paste_n}]")
            };
            segments.push((label, Some(CollapsedMarker::PastedText)));
            rest = &after[end + 3..];
        }
    }
    segments
}

fn collapse_image_markers(text: &str) -> String {
    if !text.contains("![image](file://") && !text.contains("<!--PASTE:") {
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
        let collapsed = collapsed_marker_segments(text)
            .into_iter()
            .map(|(segment, _)| segment)
            .collect::<String>();
        *cache = (hash, collapsed.clone());
        collapsed
    })
}

fn collapsed_marker_lines(text: &str) -> Vec<Vec<(String, Option<CollapsedMarker>)>> {
    let mut lines = vec![Vec::new()];
    for (segment, marker) in collapsed_marker_segments(text) {
        let mut rest = segment.as_str();
        while let Some(newline) = rest.find('\n') {
            if newline > 0 {
                lines
                    .last_mut()
                    .expect("marker line exists")
                    .push((rest[..newline].to_owned(), marker));
            }
            lines.push(Vec::new());
            rest = &rest[newline + 1..];
        }
        if !rest.is_empty() {
            lines
                .last_mut()
                .expect("marker line exists")
                .push((rest.to_owned(), marker));
        }
    }
    lines
}

fn model_label(state: &RenderSnapshot) -> String {
    // Only show the main (big) model — hide the small model entirely.
    state.config().default.big().to_string()
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
    PREAMBLE_STARTS
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
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
            let after_tag = &output[block_start..];
            let (rel_end, next_rel) = crate::tools::find_closing_tool_fence(after_tag);
            if rel_end == after_tag.len() && !after_tag.is_empty() {
                break;
            }
            let end = block_start + next_rel;
            let block = &after_tag[..rel_end];
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

fn push_assistant_content_line<'a>(
    lines: &mut Vec<Line<'a>>,
    mut line: Line<'a>,
    emitted_gutter: &mut bool,
    show_picker: bool,
) {
    if line.spans.is_empty() {
        lines.push(line);
        return;
    }

    let prefix = if *emitted_gutter { "  " } else { "• " };
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(
        prefix,
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    ));
    spans.append(&mut line.spans);
    line.spans = spans;
    *emitted_gutter = true;
    lines.push(line);
}

fn demote_assistant_bullet(lines: &mut [Line<'_>]) {
    if let Some(prefix) = lines
        .iter_mut()
        .filter_map(|line| line.spans.first_mut())
        .find(|span| span.content == "• ")
    {
        prefix.content = "  ".into();
    }
}

/// Apply one Markdown fence transition to the mutable assistant stream. The
/// opener length is part of the state: a three-backtick line cannot close a
/// four-backtick block, and fence-like content inside a longer block remains
/// code rather than toggling the renderer.
fn assistant_fence_transition(
    open: Option<(u8, usize)>,
    line: &str,
) -> (bool, Option<(u8, usize)>, Option<String>) {
    let Some((marker, marker_length, rest)) = scrollback::fence_line_info(line) else {
        return (false, open, None);
    };
    if let Some((open_marker, open_length)) = open {
        if marker == open_marker && marker_length >= open_length && rest.trim().is_empty() {
            return (true, None, None);
        }
        return (false, open, None);
    }
    (
        true,
        Some((marker, marker_length)),
        Some(rest.trim().to_owned()),
    )
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
        let time_str = thought_time_ms
            .or_else(|| {
                if is_generating {
                    None
                } else {
                    response_time_ms
                }
            })
            .map(|ms| {
                if ms >= 1000 {
                    let sec = ms as f32 / 1000.0;
                    if sec.fract().abs() < 0.001 || sec >= 10.0 {
                        format!("{:.0}s", sec)
                    } else {
                        format!("{:.1}s", sec)
                    }
                } else {
                    format!("{}ms", ms)
                }
            });
        let tokens_str = thought_tokens
            .or_else(|| {
                if is_generating {
                    None
                } else {
                    token_usage.as_ref().map(|u| u.total_tokens)
                }
            })
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
    let normalized_main_content = unwrap_markdown_table_fences(&main_content);
    let main_content = normalized_main_content.as_ref();
    if !main_content.trim().is_empty() || is_generating {
        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::from(""));
        }
        let content_width = (viewport_width as usize).saturating_sub(2).max(10);
        let mut processed_lines: Vec<(bool, String)> = Vec::new();
        let mut stream_fence = None;

        for raw_line in main_content.lines() {
            let (is_fence_boundary, next_fence, _) =
                assistant_fence_transition(stream_fence, raw_line);
            processed_lines.push((
                is_fence_boundary || stream_fence.is_some(),
                raw_line.to_string(),
            ));
            stream_fence = next_fence;
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

        // Keep fenced code source-backed while streaming, but render the body
        // directly in the transcript like Codex. A language/copy header and a
        // full-width panel add visual weight that ordinary coding replies do not
        // need; syntax colour and the existing copy registry remain available.
        let box_width = content_width;
        let mut i = 0;
        let mut emitted_assistant_gutter = false;
        let mut fence_open = None;
        let mut current_lang = String::new();
        while i < processed_lines.len() {
            if processed_lines[i].0 {
                let line_str = &processed_lines[i].1;
                let (is_code_fence, next_fence, language) =
                    assistant_fence_transition(fence_open, line_str);
                if is_code_fence {
                    let opening = fence_open.is_none();
                    if opening {
                        if lines.last().is_some_and(|line| {
                            line.spans.iter().any(|span| !span.content.is_empty())
                        }) {
                            lines.push(Line::from(""));
                        }
                        current_lang = language.unwrap_or_default();

                        let mut code_text = String::new();
                        let mut j = i + 1;
                        let open_fence = next_fence.expect("opening fence has state");
                        while j < processed_lines.len()
                            && !assistant_fence_transition(Some(open_fence), &processed_lines[j].1)
                                .0
                        {
                            if !code_text.is_empty() {
                                code_text.push('\n');
                            }
                            code_text.push_str(&processed_lines[j].1);
                            j += 1;
                        }

                        if !code_text.is_empty() {
                            let _copied_recently =
                                last_copy_text.as_ref().is_some_and(|(text, at)| {
                                    text == &code_text && at.elapsed().as_secs() < 2
                                });
                            copy_registry.push((lines.len(), code_text.clone()));

                            let rendered = if is_plain_lang(&current_lang) {
                                code_text
                                    .lines()
                                    .map(|line| {
                                        vec![Span::styled(
                                            line.to_owned(),
                                            get_themed_style(
                                                COLOR_TEXT(),
                                                COLOR_BG(),
                                                Modifier::empty(),
                                                show_picker,
                                            ),
                                        )]
                                    })
                                    .collect::<Vec<_>>()
                            } else if is_diff_lang(&current_lang) {
                                code_text
                                    .lines()
                                    .map(|line| {
                                        highlight_diff_line(line, box_width, show_picker).spans
                                    })
                                    .collect::<Vec<_>>()
                            } else {
                                highlight_code_block(&code_text, &current_lang, show_picker)
                            };

                            for body_spans in rendered {
                                for line in wrap_styled_spans(body_spans, box_width) {
                                    push_assistant_content_line(
                                        lines,
                                        line,
                                        &mut emitted_assistant_gutter,
                                        show_picker,
                                    );
                                }
                            }
                        }
                        i = j.saturating_sub(1);
                    } else {
                        if processed_lines
                            .get(i + 1)
                            .is_some_and(|(_, text)| !text.trim().is_empty())
                        {
                            lines.push(Line::from(""));
                        }
                        current_lang.clear();
                    }
                    fence_open = next_fence;
                } else if is_diff_lang(&current_lang)
                    && (line_str.starts_with('+')
                        || line_str.starts_with('-')
                        || line_str.starts_with("@@"))
                {
                    let is_diff_metadata = line_str.starts_with("@@")
                        || line_str.starts_with("--- ")
                        || line_str.starts_with("+++ ");
                    if !is_diff_metadata {
                        push_assistant_content_line(
                            lines,
                            highlight_diff_line(line_str, box_width, show_picker),
                            &mut emitted_assistant_gutter,
                            show_picker,
                        );
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
                    for line in wrap_code_spans(content_spans, box_width, COLOR_BG(), show_picker) {
                        push_assistant_content_line(
                            lines,
                            line,
                            &mut emitted_assistant_gutter,
                            show_picker,
                        );
                    }
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
                let markdown_lines =
                    render_markdown(&normal_text, content_width, show_picker, !is_generating);
                for markdown_line in markdown_lines {
                    if markdown_line.spans.is_empty() {
                        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                            lines.push(markdown_line);
                        }
                        continue;
                    }

                    push_assistant_content_line(
                        lines,
                        markdown_line,
                        &mut emitted_assistant_gutter,
                        show_picker,
                    );
                }
            }
        }
        if !is_generating {
            lines.push(Line::from(""));
        }
    }

    // A finalized thought-only response can hand off directly to a tool cell.
    // Keep the same single separator that finalized prose receives so the
    // thought preview does not run into the following Explored/Ran heading.
    if !is_generating && lines.last().is_some_and(|line| !line.spans.is_empty()) {
        lines.push(Line::from(""));
    }
}

fn wrap_input_chars(
    styled_chars: &[(char, Style)],
    inner_width: usize,
    cursor_char_index: usize,
    prompt_style: Style,
) -> (Vec<Line<'static>>, u16, u16) {
    if inner_width == 0 {
        return (vec![Line::default()], 0, 0);
    }

    type InputChar = (usize, char, Style);
    type InputLine = (Vec<InputChar>, usize);

    let indent = 2.min(inner_width);
    let mut wrapped: Vec<InputLine> = Vec::new();
    let mut current: Vec<InputChar> = Vec::new();
    let mut current_start = 0;
    let mut current_width = indent;

    for (index, &(character, style)) in styled_chars.iter().enumerate() {
        if character == '\n' {
            wrapped.push((std::mem::take(&mut current), current_start));
            current_start = index + 1;
            current_width = indent;
            continue;
        }

        let character_width = character.width().unwrap_or(1);
        if current_width + character_width > inner_width && !current.is_empty() {
            let split_at = current
                .iter()
                .rposition(|(_, character, _)| character.is_whitespace())
                .filter(|&index| index + 1 < current.len());
            let remainder = split_at.map(|index| current.split_off(index + 1));

            wrapped.push((std::mem::take(&mut current), current_start));
            current = remainder.unwrap_or_default();
            current_start = current.first().map(|(index, _, _)| *index).unwrap_or(index);
            current_width = indent
                + current
                    .iter()
                    .map(|(_, character, _)| character.width().unwrap_or(1))
                    .sum::<usize>();
        }

        current.push((index, character, style));
        current_width += character_width;
    }
    wrapped.push((current, current_start));

    let mut cursor_positions = vec![None; styled_chars.len() + 1];
    let mut lines = Vec::with_capacity(wrapped.len());
    for (row, (characters, start)) in wrapped.into_iter().enumerate() {
        let mut spans = vec![Span::styled(
            if row == 0 { "› " } else { "  " },
            prompt_style,
        )];
        let mut current_run: Option<(Style, String)> = None;
        let mut column = indent;
        cursor_positions[start] = Some((column as u16, row as u16));

        for (index, character, style) in characters {
            cursor_positions[index] = Some((column as u16, row as u16));
            match current_run.as_mut() {
                Some((run_style, text)) if *run_style == style => text.push(character),
                _ => {
                    if let Some((run_style, text)) = current_run.take() {
                        spans.push(Span::styled(text, run_style));
                    }
                    current_run = Some((style, character.to_string()));
                }
            }
            column += character.width().unwrap_or(1);
            cursor_positions[index + 1] = Some((column as u16, row as u16));
        }
        if let Some((run_style, text)) = current_run {
            spans.push(Span::styled(text, run_style));
        }
        lines.push(Line::from(spans));
    }

    let cursor = cursor_positions
        .get(cursor_char_index.min(styled_chars.len()))
        .copied()
        .flatten()
        .unwrap_or((indent as u16, 0));
    (lines, cursor.0, cursor.1)
}

fn count_input_lines(input_buffer: &str, inner_width: usize) -> u16 {
    if inner_width == 0 {
        return 1;
    }

    let collapsed = collapse_image_markers(input_buffer);
    let styled_chars = collapsed
        .chars()
        .map(|character| (character, Style::default()))
        .collect::<Vec<_>>();
    wrap_input_chars(&styled_chars, inner_width, 0, Style::default())
        .0
        .len() as u16
}

fn format_token_count(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}K", tokens as f32 / 1000.0)
    } else {
        tokens.to_string()
    }
}

fn context_usage(state: &RenderSnapshot) -> (u32, Option<u32>) {
    if let Some(usage) = &state.current_token_usage() {
        return (usage.total_tokens, usage.cached_tokens);
    }

    if let Some(usage) = state
        .active_history()
        .iter()
        .rev()
        .find_map(|message| message.token_usage.as_ref())
    {
        return (usage.total_tokens, usage.cached_tokens);
    }

    let chars: usize = state
        .active_history()
        .iter()
        .map(|message| message.content.len())
        .sum();
    ((chars / 4) as u32, None)
}

fn activity_status_label(state: &RenderSnapshot) -> String {
    let base_activity = classify_activity(&state.status(), &state.running_tools());
    let activity = if base_activity.kind == ActivityKind::ActionRequired {
        base_activity
    } else {
        classify_live_tools(&state.live_tool_calls()).unwrap_or(base_activity)
    };
    if activity.kind == ActivityKind::ActionRequired {
        return "Action Required".to_string();
    }
    if activity.kind == ActivityKind::Queued {
        return "Queued".to_string();
    }
    if activity.kind == ActivityKind::Ready {
        return "Idle".to_string();
    }
    if state.current_thought_started_at().is_some() {
        return "Thinking".to_string();
    }
    "Working".to_string()
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
    crate::app::status::format_elapsed_compact(elapsed_secs)
}

fn activity_status_line(state: &RenderSnapshot, show_picker: bool) -> Line<'static> {
    let base_activity = classify_activity(&state.status(), &state.running_tools());
    let activity = if base_activity.kind == ActivityKind::ActionRequired {
        base_activity
    } else {
        classify_live_tools(&state.live_tool_calls()).unwrap_or(base_activity)
    };
    let action_detail = state
        .pending_tool_confirmation()
        .as_ref()
        .and_then(|confirmations| confirmations.first())
        .map(|confirmation| format!("approve {}", confirmation.tool_name))
        .or_else(|| {
            state
                .pending_question()
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

    let label_text = activity_status_label(state);
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

    if activity.kind == ActivityKind::ActionRequired {
        if let Some(detail) = action_detail {
            spans.push(Span::styled(
                format!(" · {detail}"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    }

    if matches!(
        activity.kind,
        ActivityKind::Working | ActivityKind::RunningTool
    ) && let Some(started) = state.generation_start_time()
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

fn queued_user_prompts(state: &RenderSnapshot) -> Vec<&str> {
    state
        .pending_queue()
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

fn queue_preview_height(state: &RenderSnapshot) -> u16 {
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
fn render_queue_line(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &RenderSnapshot) {
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
        .pending_queue()
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

fn render_input(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &RenderSnapshot) -> Margin {
    let show_picker = state.modal_open();
    let area = chunks[2];
    f.render_widget(Clear, area);
    f.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(COLOR_PANEL())),
        area,
    );
    let input_margin = Margin {
        vertical: 1,
        horizontal: 0,
    };
    let input_inner = area.inner(input_margin);

    let text_style = if state.input_buffer().starts_with('/') {
        get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker)
    } else {
        get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker)
    };

    let inner_width = input_inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_dx = 0u16;
    let mut cursor_dy = 0u16;

    if inner_width > 0 {
        let marker_style =
            get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker);
        let mut styled_chars = Vec::new();
        for (segment, marker) in collapsed_marker_segments(&state.input_buffer()) {
            let style = if marker.is_some() {
                marker_style
            } else {
                text_style
            };
            styled_chars.extend(segment.chars().map(|c| (c, style)));
        }

        if state.input_buffer().is_empty() && state.get_command_suggestion().is_none() {
            let placeholder_style =
                get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
            let placeholder_text = "Ask RustCode to do anything";
            styled_chars.extend(placeholder_text.chars().map(|c| (c, placeholder_style)));
        } else if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        let safe_end = state.cursor_position().min(state.input_buffer().len());
        let safe_end = if state.input_buffer().is_char_boundary(safe_end) {
            safe_end
        } else {
            state
                .input_buffer()
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= safe_end)
                .last()
                .unwrap_or(0)
        };
        let raw_prefix = &state.input_buffer()[..safe_end];
        let cursor_char_index = collapse_image_markers(raw_prefix).chars().count();

        let prompt_style =
            get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker);
        (lines, cursor_dx, cursor_dy) =
            wrap_input_chars(&styled_chars, inner_width, cursor_char_index, prompt_style);
    }

    let text_area_height = input_inner.height;
    let text_area = input_inner;
    let paragraph = Paragraph::new(lines).style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(paragraph, text_area);

    if inner_width > 0 && !show_picker {
        f.set_cursor_position((
            input_inner.x + cursor_dx.min(input_inner.width.saturating_sub(1)),
            input_inner.y + cursor_dy.min(text_area_height.saturating_sub(1)),
        ));
    }

    input_margin
}

fn composer_footer_visible(
    state: &RenderSnapshot,
    has_command_completions: bool,
    has_file_completions: bool,
) -> bool {
    !state.modal_open() && !has_command_completions && !has_file_completions
}

fn footer_location(state: &RenderSnapshot) -> String {
    let (path, branch) = state
        .cwd_and_branch()
        .rsplit_once(':')
        .unwrap_or((&state.cwd_and_branch(), "unknown"));
    let branch = if branch.is_empty() { "unknown" } else { branch };
    let branch = fit_to_width(branch, 24).trim_end().to_string();
    let path = if path.is_empty() { "~" } else { path };
    format!("{branch} · {path}")
}

fn render_composer_footer(f: &mut Frame, area: ratatui::layout::Rect, state: &RenderSnapshot) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let (used, _) = context_usage(state);
    let window = state.active_context_window().max(1);
    let remaining = crate::app::status::context_remaining_percent(used, window);
    let location = footer_location(state);
    let left_content = if let Some(agent) = state.selected_subagent() {
        format!("  {} · {} · {}", agent.name(), state.model_name(), location)
    } else {
        format!("  {} · {}", state.model_name(), location)
    };
    let (right, right_style) = if state.ctrl_c_exit_armed() {
        (
            "⚠ Press Ctrl+C again to exit  ".to_owned(),
            get_themed_style(Color::Yellow, COLOR_BG(), Modifier::BOLD, false),
        )
    } else {
        (
            format!("{remaining}% context left  "),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        )
    };
    let left = fit_to_width(
        &left_content,
        (area.width as usize).saturating_sub(right.width()),
    );
    let padding = (area.width as usize).saturating_sub(left.width() + right.width());
    let left_style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(left, left_style),
            Span::styled(" ".repeat(padding), Style::default().bg(COLOR_BG())),
            Span::styled(right, right_style),
        ]))
        .style(Style::default().bg(COLOR_BG())),
        area,
    );
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

fn contract_home_path(path: &str, home_path: Option<&str>) -> String {
    if path.is_empty() {
        return String::new();
    }
    if let Some(home) = home_path {
        if path.starts_with(&home) {
            return format!("~{}", &path[home.len()..]);
        }
    }
    path.to_string()
}

fn format_pi_tool_action(
    name: &str,
    args: &serde_json::Value,
    home_path: Option<&str>,
) -> (String, String) {
    let name_lower = name.to_ascii_lowercase();
    let action_label = match name_lower.as_str() {
        "view_file" | "viewfile" | "read_file" | "readfile" => "Read".to_string(),
        "replace_file_content"
        | "replacefilecontent"
        | "multi_replace_file_content"
        | "multireplacefilecontent"
        | "edit_file"
        | "editfile"
        | "patch_file"
        | "patchfile" => "Edit".to_string(),
        "write_to_file" | "writetofile" | "write_file" | "writefile" | "create_file"
        | "createfile" => "Write".to_string(),
        "delete_file" | "deletefile" => "Delete".to_string(),
        "move_file" | "movefile" => "Move".to_string(),
        "copy_file" | "copyfile" => "Copy".to_string(),
        "list_directory" | "list_dir" | "listdir" | "glob" => "ListDir".to_string(),
        "grep" | "grep_search" | "grepsearch" => "Search".to_string(),
        "find_symbol" | "findsymbol" | "codebase_symbol" | "codebasesymbol" => "Symbol".to_string(),
        "run_command" | "runcommand" | "execute_command" | "bash" => "Bash".to_string(),
        "search_web" | "searchweb" | "codebase_search" | "codebasesearch" => "Search".to_string(),
        "get_project_map" | "getprojectmap" => "ProjectMap".to_string(),
        "manage_task" | "managetask" => "ManageTask".to_string(),
        "background_task" | "backgroundtask" => "TaskDone".to_string(),
        "remember" => "Remember".to_string(),
        "recall_memory" | "recallmemory" => "Recall".to_string(),
        "forget_memory" | "forgetmemory" => "Forget".to_string(),
        _ => to_pascal_case(name),
    };

    let target_arg = match name_lower.as_str() {
        "view_file"
        | "viewfile"
        | "read_file"
        | "readfile"
        | "replace_file_content"
        | "replacefilecontent"
        | "multi_replace_file_content"
        | "multireplacefilecontent"
        | "write_to_file"
        | "writetofile"
        | "write_file"
        | "writefile"
        | "edit_file"
        | "editfile"
        | "create_file"
        | "createfile"
        | "patch_file"
        | "patchfile"
        | "delete_file"
        | "deletefile" => {
            let path = args
                .get("TargetFile")
                .or_else(|| args.get("target_file"))
                .or_else(|| args.get("AbsolutePath"))
                .or_else(|| args.get("absolute_path"))
                .or_else(|| args.get("path"))
                .or_else(|| args.get("file"))
                .or_else(|| args.get("filePath"))
                .or_else(|| args.get("filepath"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            contract_home_path(path, home_path)
        }
        "move_file" | "movefile" | "copy_file" | "copyfile" => {
            let src = args
                .get("src")
                .or_else(|| args.get("source"))
                .or_else(|| args.get("from"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let dest = args
                .get("dest")
                .or_else(|| args.get("destination"))
                .or_else(|| args.get("to"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("{} -> {}", src, dest)
        }
        "list_directory" | "list_dir" | "glob" => {
            let path = args
                .get("DirectoryPath")
                .or_else(|| args.get("SearchPath"))
                .or_else(|| args.get("path"))
                .or_else(|| args.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            contract_home_path(path, home_path)
        }
        "grep" | "grep_search" => {
            let query = args
                .get("Query")
                .or_else(|| args.get("query"))
                .or_else(|| args.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("Grep {query}")
        }
        "run_command" => args
            .get("CommandLine")
            .or_else(|| args.get("command"))
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        "search_web" | "codebase_search" | "find_symbol" | "codebase_symbol" | "recall_memory" => {
            args.get("query")
                .or_else(|| args.get("Query"))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string()
        }
        "remember" | "forget_memory" => args
            .get("key")
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
        "wait_agent" | "cancel_agent" => args
            .get("id")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            })
            .unwrap_or_default(),
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
#[allow(dead_code)]
struct ChatCache {
    key: ChatKey,
    lines: Vec<Line<'static>>,
    copy_wrapped_rows: Vec<(u16, String)>,
    msg_wrapped_rows: Vec<u16>,
    total_wrapped_lines: u16,
}

#[allow(dead_code)]
type RenderedConversation = (Vec<Line<'static>>, Vec<(u16, String)>, Vec<u16>, u16);

#[allow(dead_code)]
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

#[allow(dead_code)]
fn chat_cache_key(state: &RenderSnapshot, width: u16, show_picker: bool) -> ChatKey {
    let history = state.active_history();
    ChatKey {
        hist_len: history.len(),
        total_len: history.iter().map(|m| m.content.len()).sum(),
        last_len: history.last().map_or(0, |m| m.content.len()),
        history_display_start: state.active_history_display_start(),
        width,
        show_picker,
        copied_recently: state
            .last_copy_text()
            .as_ref()
            .map(|(t_text, t)| (t_text.clone(), t.elapsed().as_secs() < 2)),
        theme: state.config().theme.clone(),
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
        "set_goal" | "todo_write" | "complete_task" | "ask_question"
    )
}

fn tool_result_action(
    state: &RenderSnapshot,
    message_index: usize,
    tool_name: &str,
) -> (String, String) {
    format_pi_tool_action(
        tool_name,
        &tool_call_arguments(state, message_index, tool_name),
        state.home_path(),
    )
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

fn indent_tool_result_body(
    lines: Vec<Line<'static>>,
    tool_name: &str,
    verbosity: &crate::app::Verbosity,
    width: u16,
) -> Vec<Line<'static>> {
    if matches!(verbosity, crate::app::Verbosity::High) {
        return Vec::new();
    }

    let filtered = lines
        .into_iter()
        .filter(|line| {
            tool_name != "run_command"
                || !line
                    .spans
                    .iter()
                    .any(|span| span.content.trim_start().starts_with('✗'))
        })
        .collect::<Vec<_>>();
    let max_visible = 6;
    let omitted = filtered.len().saturating_sub(max_visible);
    let head_count = max_visible / 2;
    let tail_count = max_visible - head_count;
    let visible = if omitted == 0 {
        filtered
    } else {
        filtered[..head_count]
            .iter()
            .chain(&filtered[filtered.len() - tail_count..])
            .cloned()
            .collect()
    };
    let max_w = (width as usize).max(10);
    let mut indented = Vec::new();
    for (index, line) in visible.into_iter().enumerate() {
        if line.spans.is_empty() {
            indented.push(line);
            continue;
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::styled(
            if index == 0 { "  └ " } else { "    " },
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        ));
        spans.extend(line.spans);
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        );
        push_wrapped_with_continuation(&mut indented, spans, max_w, Some(continuation));
    }
    if omitted > 0 {
        indented.insert(
            head_count,
            Line::from(Span::styled(
                format!("    … +{omitted} lines"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, false),
            )),
        );
    }
    indented
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolTranscriptKind {
    Explored,
    Command,
    Edit,
    Tool,
}

fn tool_transcript_kind(tool_name: &str) -> ToolTranscriptKind {
    if crate::app::activity::is_exploration_tool(tool_name) {
        ToolTranscriptKind::Explored
    } else if tool_name == "run_command" || tool_name.eq_ignore_ascii_case("bash") {
        ToolTranscriptKind::Command
    } else if crate::app::activity::is_editing_tool(tool_name) {
        ToolTranscriptKind::Edit
    } else {
        ToolTranscriptKind::Tool
    }
}

fn format_exploration_action(
    name: &str,
    args: &serde_json::Value,
    home_path: Option<&str>,
) -> (String, String) {
    match name {
        "view_file" => {
            let path = args
                .get("TargetFile")
                .or_else(|| args.get("AbsolutePath"))
                .or_else(|| args.get("path"))
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            ("Read".to_string(), contract_home_path(path, home_path))
        }
        "list_directory" | "list_dir" | "glob" => {
            let path = args
                .get("DirectoryPath")
                .or_else(|| args.get("SearchPath"))
                .or_else(|| args.get("path"))
                .or_else(|| args.get("pattern"))
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            ("List".to_string(), contract_home_path(path, home_path))
        }
        "grep" | "grep_search" => {
            let query = args
                .get("Query")
                .or_else(|| args.get("query"))
                .or_else(|| args.get("pattern"))
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let path = args
                .get("SearchPath")
                .or_else(|| args.get("path"))
                .and_then(|value| value.as_str())
                .filter(|path| !path.is_empty() && *path != ".");
            let target = path
                .map(|path| format!("{query} in {}", contract_home_path(path, home_path)))
                .unwrap_or_else(|| query.to_string());
            ("Search".to_string(), target)
        }
        "find_symbol" | "codebase_search" | "codebase_symbol" => {
            let query = args
                .get("query")
                .or_else(|| args.get("Query"))
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            ("Search".to_string(), query.to_string())
        }
        "get_project_map" => ("Read".to_string(), "project map".to_string()),
        _ => format_pi_tool_action(name, args, home_path),
    }
}

struct ToolTranscriptEntry {
    message_index: usize,
    tool_name: String,
    action: String,
    target: String,
    success: bool,
    status: String,
    body: Vec<Line<'static>>,
    kind: ToolTranscriptKind,
}

fn tool_call_arguments(
    state: &RenderSnapshot,
    message_index: usize,
    tool_name: &str,
) -> serde_json::Value {
    let history = state.active_history();
    let message = &history[message_index];
    if let Some(call_id) = message.tool_call_id.as_deref() {
        return history[..message_index]
            .iter()
            .rev()
            .filter(|message| message.role == "assistant")
            .flat_map(|message| message.tool_calls.iter().rev())
            .find(|call| call.id == call_id)
            .and_then(|call| serde_json::from_str(&call.arguments).ok())
            .unwrap_or(serde_json::Value::Null);
    }

    for (assistant_index, assistant) in history[..message_index].iter().enumerate().rev() {
        if assistant.role != "assistant" {
            continue;
        }
        let calls = assistant.resolved_tool_calls(state.active_tool_protocol());
        if !calls.iter().any(|call| call.name == tool_name) {
            continue;
        }
        let prior_same_name_results = history[assistant_index + 1..message_index]
            .iter()
            .filter(|message| {
                message.role == "tool"
                    && resolve_tool_result_name(
                        None,
                        message
                            .tool_result
                            .as_ref()
                            .map(|result| result.tool_name.as_str()),
                        &message.content,
                    )
                    .as_deref()
                        == Some(tool_name)
            })
            .count();
        if let Some(call) = calls
            .into_iter()
            .filter(|call| call.name == tool_name)
            .nth(prior_same_name_results)
        {
            return call.arguments;
        }
    }

    serde_json::Value::Null
}

fn tool_transcript_entry(
    state: &RenderSnapshot,
    message_index: usize,
    width: u16,
    show_picker: bool,
) -> Option<ToolTranscriptEntry> {
    let message = state.active_history().get(message_index)?;
    if message.role != "tool" {
        return None;
    }
    let tool_name = resolve_tool_result_name(
        None,
        message
            .tool_result
            .as_ref()
            .map(|result| result.tool_name.as_str()),
        &message.content,
    )
    .unwrap_or_else(|| "Tool".to_owned());
    if tool_result_is_hidden(&tool_name) {
        return None;
    }

    let result = message
        .content
        .split_once(": ")
        .map(|(_, result)| result)
        .unwrap_or(&message.content);
    let kind = tool_transcript_kind(&tool_name);
    let (action, target) = if kind == ToolTranscriptKind::Explored {
        let args = tool_call_arguments(state, message_index, &tool_name);
        format_exploration_action(&tool_name, &args, state.home_path())
    } else {
        tool_result_action(state, message_index, &tool_name)
    };
    let (success, status) = tool_result_status(message, &tool_name, result);
    let body = cached_tool_result(
        &tool_name,
        result,
        width as usize,
        &state.verbosity(),
        show_picker,
    );

    Some(ToolTranscriptEntry {
        message_index,
        tool_name,
        action,
        target,
        success,
        status,
        body,
        kind,
    })
}

fn tool_group_header(title: &str, success: bool, show_picker: bool) -> Line<'static> {
    let bullet_color = if success {
        COLOR_GREEN()
    } else {
        Color::Rgb(229, 123, 123)
    };
    Line::from(vec![
        Span::styled(
            "• ",
            get_themed_style(bullet_color, COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            title.to_owned(),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
    ])
}

fn tool_child_line(
    entry: &ToolTranscriptEntry,
    first: bool,
    show_hint: bool,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let mut spans = vec![Span::styled(
        if first { "  └ " } else { "    " },
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    )];
    if entry.kind == ToolTranscriptKind::Edit {
        if !entry.target.is_empty() && entry.target != "?" {
            spans.push(Span::styled(
                entry.target.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        } else {
            spans.push(Span::styled(
                entry.action.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
        }
    } else {
        spans.push(Span::styled(
            entry.action.clone(),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
        if !entry.target.is_empty() && entry.target != "?" {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                entry.target.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    }
    if show_hint {
        spans.push(Span::styled(
            " (ctrl+o to expand)",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
        ));
    }
    let mut lines = Vec::new();
    let continuation = Span::styled(
        "    ",
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    );
    push_wrapped_with_continuation(
        &mut lines,
        spans,
        (width as usize).max(10),
        Some(continuation),
    );
    lines
}

fn command_child_lines(
    entry: &ToolTranscriptEntry,
    first: bool,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let mut commands = highlight_shell_command(&entry.target, COLOR_BG(), show_picker);
    if commands.is_empty() {
        commands.push(Line::default());
    }
    let mut lines = Vec::with_capacity(commands.len());
    let max_w = (width as usize).max(10);
    for (command_index, command) in commands.into_iter().enumerate() {
        let mut spans = vec![Span::styled(
            if first && command_index == 0 {
                "  └ "
            } else {
                "    "
            },
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        )];
        if command_index == 0 {
            spans.push(Span::styled(
                entry.action.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
            if !entry.target.is_empty() && entry.target != "?" {
                spans.push(Span::styled(
                    " ",
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
            }
        }
        if entry.target != "?" {
            spans.extend(command.spans);
        }
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        );
        push_wrapped_with_continuation(&mut lines, spans, max_w, Some(continuation));
    }
    if !entry.success {
        if let Some(line) = lines.last_mut() {
            line.spans.push(Span::styled(
                format!(" · {}", entry.status),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    }
    lines
}

fn command_summary_lines(
    entry: &ToolTranscriptEntry,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let bullet_color = if entry.success {
        COLOR_GREEN()
    } else {
        Color::Rgb(229, 123, 123)
    };
    let has_command = !entry.target.is_empty() && entry.target != "?";
    let mut commands = highlight_shell_command(&entry.target, COLOR_BG(), show_picker);
    if commands.is_empty() {
        commands.push(Line::default());
    }
    let last = commands.len().saturating_sub(1);
    let max_w = (width as usize).max(10);
    let mut lines = Vec::new();
    for (index, command) in commands.into_iter().enumerate() {
        let mut spans = if index == 0 {
            vec![
                Span::styled(
                    "• ",
                    get_themed_style(bullet_color, COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    if has_command { "Ran $ " } else { "Ran Bash" },
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
            ]
        } else {
            vec![Span::styled(
                "    ",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            )]
        };
        if has_command {
            spans.extend(command.spans);
        }
        if index == last {
            spans.push(Span::styled(
                format!(" · {}", entry.status),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        );
        push_wrapped_with_continuation(&mut lines, spans, max_w, Some(continuation));
    }
    lines
}

fn indent_generic_tool_body(
    lines: Vec<Line<'static>>,
    verbosity: &crate::app::Verbosity,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    if matches!(verbosity, crate::app::Verbosity::High) {
        return Vec::new();
    }

    let max_visible = 6;
    let omitted = lines.len().saturating_sub(max_visible);
    let head_count = max_visible / 2;
    let tail_count = max_visible - head_count;
    let visible = if omitted == 0 {
        lines
    } else {
        lines[..head_count]
            .iter()
            .chain(&lines[lines.len() - tail_count..])
            .cloned()
            .collect()
    };
    let max_w = (width as usize).max(10);
    let mut indented = Vec::new();
    for line in visible {
        if line.spans.is_empty() {
            indented.push(line);
            continue;
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        spans.extend(line.spans);
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        );
        push_wrapped_with_continuation(&mut indented, spans, max_w, Some(continuation));
    }
    if omitted > 0 {
        indented.insert(
            head_count,
            Line::from(Span::styled(
                format!("    … +{omitted} lines"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
            )),
        );
    }
    indented
}

pub(crate) fn render_committed_tool_result_group_snapshot(
    state: &RenderSnapshot,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let entries = message_indices
        .iter()
        .filter_map(|&index| tool_transcript_entry(state, index, width, show_picker))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        let kind = entries[index].kind;
        let group_end = if kind == ToolTranscriptKind::Command
            && matches!(state.verbosity(), crate::app::Verbosity::Low)
        {
            index + 1
        } else {
            (index + 1..entries.len())
                .find(|&next| entries[next].kind != kind)
                .unwrap_or(entries.len())
        };
        let group = &entries[index..group_end];
        let success = group.iter().all(|entry| entry.success);

        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        if kind == ToolTranscriptKind::Command {
            if matches!(state.verbosity(), crate::app::Verbosity::High) {
                lines.push(tool_group_header("Ran", success, show_picker));
                for (child_index, entry) in group.iter().enumerate() {
                    lines.extend(command_child_lines(
                        entry,
                        child_index == 0,
                        width,
                        show_picker,
                    ));
                }
            } else {
                let entry = &group[0];
                lines.extend(command_summary_lines(entry, width, show_picker));
                lines.extend(indent_tool_result_body(
                    entry.body.clone(),
                    &entry.tool_name,
                    &state.verbosity(),
                    width,
                ));
            }
        } else {
            let title = if kind == ToolTranscriptKind::Explored {
                "Explored"
            } else if kind == ToolTranscriptKind::Edit {
                "Edited"
            } else if kind == ToolTranscriptKind::Tool {
                "Ran"
            } else {
                "Called"
            };
            lines.push(tool_group_header(title, success, show_picker));
            let mut seen = std::collections::HashSet::new();
            let mut first_child = true;
            for entry in group {
                let identity = format!("{}\0{}", entry.action, entry.target);
                if kind != ToolTranscriptKind::Explored || seen.insert(identity) {
                    let is_expanded = state.expanded_thoughts().contains(&entry.message_index);
                    let show_hint = kind == ToolTranscriptKind::Tool
                        && !entry.body.is_empty()
                        && !is_expanded
                        && matches!(state.verbosity(), crate::app::Verbosity::Low);
                    lines.extend(tool_child_line(
                        entry,
                        first_child,
                        show_hint,
                        width,
                        show_picker,
                    ));
                    first_child = false;
                    if kind == ToolTranscriptKind::Tool
                        && is_expanded
                        && matches!(state.verbosity(), crate::app::Verbosity::Low)
                    {
                        lines.extend(indent_generic_tool_body(
                            entry.body.clone(),
                            &state.verbosity(),
                            width,
                            show_picker,
                        ));
                    }
                }
            }
        }

        index = group_end;
    }
    lines
}

fn render_committed_tool_result(
    state: &RenderSnapshot,
    message_index: usize,
    _tool_name: &str,
    _result: &str,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    render_committed_tool_result_group_snapshot(state, &[message_index], width, show_picker)
}

fn format_elapsed_compact(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

pub(crate) fn render_work_separator_before_assistant_snapshot(
    state: &RenderSnapshot,
    assistant_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let history = state.active_history();
    let Some(message) = history.get(assistant_index) else {
        return Vec::new();
    };
    if message.role != "assistant" || message.content.trim().is_empty() {
        return Vec::new();
    }
    let follows_work = history[..assistant_index]
        .iter()
        .rev()
        .find(|candidate| {
            !((candidate.role == "system" || candidate.role == "assistant")
                && is_hidden_system_notice(&candidate.content))
        })
        .is_some_and(|candidate| candidate.role == "tool");
    if !follows_work {
        return Vec::new();
    }

    let label = message
        .response_time_ms
        .filter(|milliseconds| *milliseconds > 60_000)
        .map(|milliseconds| format!("─ Worked for {} ─", format_elapsed_compact(milliseconds)));
    let text = if let Some(label) = label {
        let label_width = label.width();
        format!(
            "{label}{}",
            "─".repeat((width as usize).saturating_sub(label_width))
        )
    } else {
        "─".repeat(width.max(1) as usize)
    };
    vec![
        Line::from(Span::styled(
            text,
            get_themed_style(COLOR_TURN_SEPARATOR(), COLOR_BG(), Modifier::empty(), false),
        )),
        Line::from(""),
    ]
}

fn push_centered_separator<'a>(
    lines: &mut Vec<Line<'a>>,
    label_text: &str,
    width: u16,
    show_picker: bool,
) {
    if lines.last().map_or(true, |l| !l.spans.is_empty()) {
        lines.push(Line::from(""));
    }
    let label = format!(" {} ", label_text.trim());
    let remaining = (width as usize).saturating_sub(label.width());
    let left = remaining / 2;
    let right = remaining - left;
    let line_style = get_themed_style(
        COLOR_TURN_SEPARATOR(),
        COLOR_BG(),
        Modifier::empty(),
        show_picker,
    );
    let label_style = get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
    lines.push(Line::from(vec![
        Span::styled("─".repeat(left), line_style),
        Span::styled(label, label_style),
        Span::styled("─".repeat(right), line_style),
    ]));
}

fn push_new_chat_separator<'a>(lines: &mut Vec<Line<'a>>, width: u16, show_picker: bool) {
    push_centered_separator(lines, "✨ NEW CHAT", width, show_picker);
    lines.push(Line::from(""));
}

fn is_hidden_system_notice(content: &str) -> bool {
    content.contains("Loop warning:")
        || content.contains("tool calls in that response were dropped")
        || content.contains("Oversized response:")
        || content.starts_with(crate::network::compaction::SUMMARY_MARKER)
        || content.starts_with("[harness: stopped after ")
        || content.contains("Your reasoning became repetitive")
        || content.contains("reasoning loop")
}

fn tool_result_follows(history: &[ChatMessage], assistant_index: usize) -> bool {
    next_visible_message(history, assistant_index).is_some_and(|message| message.role == "tool")
}

fn next_visible_message(history: &[ChatMessage], index: usize) -> Option<&ChatMessage> {
    history.iter().skip(index + 1).find(|message| {
        !((message.role == "system" || message.role == "assistant")
            && is_hidden_system_notice(&message.content))
    })
}

pub(crate) fn tool_result_needs_assistant_gap(history: &[ChatMessage], tool_index: usize) -> bool {
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

    if lower.starts_with("resumed session") {
        push_centered_separator(lines, "Resumed Session", width, show_picker);
        return;
    }
    if lower.contains("new chat started") {
        push_centered_separator(lines, "New Chat Started", width, show_picker);
        return;
    }

    // Convert verbose internal agent-steering prompts into concise, human-friendly status lines in the UI.
    let human_summary = if content.contains("stuck in a loop")
        || content.contains("CRITICAL — you are stuck in a loop")
    {
        Some("Repetitive tool loop detected — stopping tools and requesting final response")
    } else if content.contains("Your reasoning became repetitive")
        || content.contains("reasoning loop")
    {
        Some("Reasoning loop detected — continuing turn to take concrete action")
    } else if content.contains("Evidence-based recovery:")
        || content.contains("previous tool action repeated without making progress")
    {
        Some("Repetitive tool actions detected — nudging agent to make progress")
    } else if content.starts_with("[harness: failure replan") {
        Some("Repeated tool execution failures — requesting alternative strategy")
    } else {
        None
    };

    if let Some(summary) = human_summary {
        lines.push(Line::from(vec![
            Span::styled(
                "! ",
                get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                summary.to_string(),
                get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ]));
        return;
    }

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
    let content_w = inner_w.saturating_sub(2);

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
            || trimmed.starts_with('?')
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

pub(crate) fn build_claude_startup_banner_snapshot(
    state: &RenderSnapshot,
    total_width: usize,
    _max_height: usize,
) -> Vec<Line<'static>> {
    let mut banner = Vec::new();
    let version = env!("CARGO_PKG_VERSION");
    let model_name = model_label(state);

    let box_w = total_width.saturating_sub(2).min(66).max(45);
    let inner_w = box_w.saturating_sub(2);

    let border_c = COLOR_PRIMARY();
    let primary = COLOR_PRIMARY();
    let text_c = COLOR_TEXT();
    let muted_c = COLOR_MUTED();
    let reset_bg = COLOR_BG();

    // Top border
    let title_str = format!(">_ RustCode v{version}");
    let top_pad = inner_w.saturating_sub(title_str.chars().count() + 3);
    let top_border = format!("╭─ {title_str} {}╮", "─".repeat(top_pad));
    banner.push(Line::from(vec![Span::styled(
        top_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));

    let make_row = |spans: Vec<Span<'static>>| -> Line<'static> {
        let mut line_spans = Vec::new();
        line_spans.push(Span::styled(
            "│",
            Style::default().fg(border_c).bg(reset_bg),
        ));

        let mut used = 0;
        for s in &spans {
            used += s.content.chars().count();
        }
        line_spans.extend(spans);

        let pad = inner_w.saturating_sub(used);
        if pad > 0 {
            line_spans.push(Span::styled(" ".repeat(pad), Style::default().bg(reset_bg)));
        }
        line_spans.push(Span::styled(
            "│",
            Style::default().fg(border_c).bg(reset_bg),
        ));
        Line::from(line_spans)
    };

    // Blank line after title
    banner.push(make_row(vec![]));

    // Row 1: model
    let label_w = 15;
    let mut model_spans = vec![
        Span::styled(
            fit_to_width("  model:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            model_name.clone(),
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let used_for_model = label_w + model_name.chars().count();
    if inner_w >= used_for_model + 22 {
        model_spans.push(Span::styled("    ", Style::default().bg(reset_bg)));
        model_spans.push(Span::styled(
            "/model",
            Style::default().fg(primary).bg(reset_bg),
        ));
        model_spans.push(Span::styled(
            " to change",
            Style::default().fg(muted_c).bg(reset_bg),
        ));
    }
    banner.push(make_row(model_spans));

    // Row 2: reasoning effort
    let effort = state
        .active_model_profile()
        .and_then(|profile| profile.reasoning_effort.clone())
        .unwrap_or_else(|| "default".to_string());
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  effort:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            effort,
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("    ", Style::default().bg(reset_bg)),
        Span::styled("/effort", Style::default().fg(primary).bg(reset_bg)),
        Span::styled(" to change", Style::default().fg(muted_c).bg(reset_bg)),
    ]));

    // Row 3: context window
    let context_window = format!(
        "{} tokens",
        format_token_count(state.active_context_window())
    );
    let mut context_spans = vec![
        Span::styled(
            fit_to_width("  context:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            context_window.clone(),
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let used_for_context = label_w + context_window.chars().count();
    if inner_w >= used_for_context + 22 {
        context_spans.push(Span::styled("    ", Style::default().bg(reset_bg)));
        context_spans.push(Span::styled(
            "/context",
            Style::default().fg(primary).bg(reset_bg),
        ));
        context_spans.push(Span::styled(
            " to change",
            Style::default().fg(muted_c).bg(reset_bg),
        ));
    }
    banner.push(make_row(context_spans));

    // Row 4: directory
    let (dir_display, _) = state
        .cwd_and_branch()
        .rsplit_once(':')
        .unwrap_or((state.cwd_and_branch(), ""));
    let dir_display = if dir_display.is_empty() {
        "~"
    } else {
        dir_display
    };

    let max_dir_len = inner_w.saturating_sub(label_w + 1);
    let dir_fitted = fit_to_width(&dir_display, max_dir_len);
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  directory:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(dir_fitted, Style::default().fg(text_c).bg(reset_bg)),
    ]));

    // Row 5: branch
    let branch_name = state
        .cwd_and_branch()
        .rsplit_once(':')
        .map(|(_, branch)| branch)
        .filter(|branch| !branch.is_empty())
        .unwrap_or("unknown");
    let branch_fitted = fit_to_width(branch_name, inner_w.saturating_sub(label_w));
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  branch:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(branch_fitted, Style::default().fg(text_c).bg(reset_bg)),
    ]));

    // Row 6: permissions
    let (perm_text, perm_style) = if state.auto_confirm() {
        (
            "YOLO mode",
            Style::default()
                .fg(Color::Rgb(255, 125, 155))
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            "Interactive",
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        )
    };
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  permissions:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(perm_text, perm_style),
    ]));

    // Help shortcut
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  help:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled("/help", Style::default().fg(primary).bg(reset_bg)),
        Span::styled(" for commands", Style::default().fg(muted_c).bg(reset_bg)),
    ]));

    // Blank line before the bottom border
    banner.push(make_row(vec![]));

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
fn render_live_tail_snapshot(
    state: &RenderSnapshot,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let mut transcript = TranscriptState::default();
    render_live_tail_with_transcript(state, width, height, &mut transcript)
}

pub(crate) fn render_live_tail(state: &AppState, width: u16, height: u16) -> Vec<Line<'static>> {
    let snapshot = state.render_snapshot();
    render_live_tail_snapshot(&snapshot, width, height)
}

/// Render the mutable end of the transcript using a persistent presentation
/// cell owned by the terminal loop. The compatibility wrapper above keeps
/// snapshot/unit callers simple; the interactive TUI passes the same state
/// across frames so deltas replace one active cell instead of constructing a
/// new terminal block on every redraw.
pub(crate) fn render_live_tail_with_transcript(
    state: &RenderSnapshot,
    width: u16,
    height: u16,
    transcript: &mut TranscriptState,
) -> Vec<Line<'static>> {
    if state.selected_subagent().is_some() {
        return render_selected_subagent_context(state, width, height);
    }

    let visible_history_is_empty =
        state.history().is_empty() || state.history_display_start() >= state.history().len();
    if visible_history_is_empty
        && state.current_response().is_empty()
        && matches!(state.status(), AppStatus::Idle)
        && state.running_tools().is_empty()
        && state.live_tool_calls().is_empty()
    {
        return build_claude_startup_banner_snapshot(state, width as usize, height as usize);
    }

    let tail = scrollback::mutable_stream_text(&state.current_response());
    let mut lines = Vec::new();

    let mut has_visible_active_cell = false;
    let mut model_live_text = "";
    let visible_live_tool_calls = state
        .live_tool_calls()
        .iter()
        .filter(|call| is_live_tool_call_visible(call))
        .cloned()
        .collect::<Vec<_>>();
    if !visible_live_tool_calls.is_empty() {
        transcript.set_tools_with_verbosity(&visible_live_tool_calls, &state.verbosity());
        has_visible_active_cell = true;
    } else if !tail.is_empty() {
        let parsed_tool = crate::tools::parse_tool_call(&tail, state.active_tool_protocol());
        let is_tool_syntax = crate::tools::is_tool_call_start(&tail);
        let should_hide_stream = match parsed_tool {
            Some(ref tool_call) => !crate::tools::is_code_editing_tool(&tool_call.name),
            None => is_tool_syntax,
        };

        if !should_hide_stream {
            model_live_text = &tail;
            has_visible_active_cell = true;
        } else {
            transcript.clear();
        }
    } else {
        transcript.clear();
    }

    transcript.sync_model(&state.history(), model_live_text);
    let model_tail = transcript
        .model()
        .live_text()
        .unwrap_or_default()
        .to_owned();

    if has_visible_active_cell && state.live_tool_calls().is_empty() {
        let live_thought_time_ms = if state.current_thought_started_at().is_some()
            || state.current_thought_time_ms() > 0
        {
            let elapsed_current = state
                .current_thought_started_at()
                .map(|started| started.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let total_ms = state
                .current_thought_time_ms()
                .saturating_add(elapsed_current);
            (total_ms > 0).then_some(total_ms)
        } else {
            None
        };
        let live_thought_tokens =
            (state.current_thought_tokens() > 0).then_some(state.current_thought_tokens());

        transcript.set_assistant(
            &model_tail,
            scrollback::mutable_stream_is_continuation(&state.current_response()),
            state
                .generation_start_time()
                .map(|started| started.elapsed().as_millis() as u64),
            live_thought_time_ms,
            live_thought_tokens,
        );
    }

    if has_visible_active_cell {
        lines.extend(transcript.display_lines(width));
    }

    let activity_visible = matches!(state.status(), AppStatus::Streaming | AppStatus::Queued)
        || !state.running_tools().is_empty();
    if activity_visible {
        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::from(""));
        }
        lines.push(activity_status_line(state, false));
        lines.push(Line::from(""));
    }

    if height > 0 && lines.len() > height as usize {
        let visible_start = lines.len() - height as usize;
        lines = lines.split_off(visible_start);
    }

    lines.into_iter().map(|line| own_line(&line)).collect()
}

fn render_selected_subagent_context(
    state: &RenderSnapshot,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let Some(agent) = state.selected_subagent() else {
        return Vec::new();
    };
    let status = match agent.status() {
        crate::app::SubAgentStatus::Running => "running",
        crate::app::SubAgentStatus::Completed => "completed",
        crate::app::SubAgentStatus::Failed => "failed",
        crate::app::SubAgentStatus::Cancelled => "cancelled",
    };
    let parent = agent
        .parent_id()
        .map(|id| format!("agent-{id}"))
        .unwrap_or_else(|| "main".to_owned());
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("↳ {}", agent.name()),
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, false),
        ),
        Span::styled(
            format!(" · {status} · parent {parent}"),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        ),
    ])];
    lines.push(Line::from(Span::styled(
        "  agent context · use /agents to navigate · main history preserved",
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
    )));

    let history = state.active_history();
    let start = history.len().saturating_sub(8);
    for index in start..history.len() {
        lines.extend(render_committed_history_block_snapshot(state, index, width));
    }
    if agent.active_turn() {
        lines.push(Line::from(Span::styled(
            "• Working",
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, false),
        )));
    }
    if lines.len() > height as usize {
        lines = lines.split_off(lines.len() - height as usize);
    }
    lines.into_iter().map(|line| own_line(&line)).collect()
}

/// Render one finalized history entry for insertion into terminal scrollback.
pub(crate) fn render_committed_history_block_snapshot(
    state: &RenderSnapshot,
    message_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let history = state.active_history();
    let Some(message) = history.get(message_index) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let show_picker = false;

    match message.role.as_str() {
        "user" => {
            let prefix_style =
                get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker);
            let marker_style =
                get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker);
            let text_style =
                get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker);
            let continuation = Span::styled("  ", prefix_style);
            let mut user_lines = Vec::new();
            for (index, segments) in
                collapsed_marker_lines(message.content.trim_end_matches(['\r', '\n']))
                    .into_iter()
                    .enumerate()
            {
                let prefix = if index == 0 {
                    Span::styled("› ", prefix_style)
                } else {
                    continuation.clone()
                };
                let mut spans = vec![prefix];
                for (segment, marker) in segments {
                    spans.push(Span::styled(
                        segment,
                        if marker.is_some() {
                            marker_style
                        } else {
                            text_style
                        },
                    ));
                }
                push_wrapped_with_continuation(
                    &mut user_lines,
                    spans,
                    width as usize,
                    Some(continuation.clone()),
                );
            }
            for line in &mut user_lines {
                for span in &mut line.spans {
                    span.style = span.style.bg(COLOR_PANEL());
                }
                let padding = (width as usize).saturating_sub(line.width());
                if padding > 0 {
                    line.spans.push(Span::styled(
                        " ".repeat(padding),
                        Style::default().bg(COLOR_PANEL()),
                    ));
                }
            }
            let panel_padding = || {
                Line::from(Span::styled(
                    " ".repeat(width as usize),
                    Style::default().bg(COLOR_PANEL()),
                ))
            };
            lines.push(panel_padding());
            lines.extend(user_lines);
            lines.push(panel_padding());
            lines.push(Line::from(""));
        }
        "assistant" => {
            if is_hidden_system_notice(&message.content) {
                return Vec::new();
            }
            return history_cell::AssistantMarkdownCell::committed(
                &message.content,
                message.token_usage.clone(),
                message.response_time_ms,
                message.thought_time_ms,
                message.thought_tokens,
            )
            .display_lines(width);
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
                let next_is_tool = state
                    .active_history()
                    .get(message_index + 1)
                    .is_some_and(|m| m.role == "tool");
                if !next_is_tool {
                    lines.push(Line::from(""));
                }
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

pub(crate) fn render_committed_tool_result_group(
    state: &AppState,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let snapshot = state.render_snapshot();
    render_committed_tool_result_group_snapshot(&snapshot, message_indices, width, show_picker)
}

pub(crate) fn render_work_separator_before_assistant(
    state: &AppState,
    assistant_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let snapshot = state.render_snapshot();
    render_work_separator_before_assistant_snapshot(&snapshot, assistant_index, width)
}

pub(crate) fn build_claude_startup_banner(
    state: &AppState,
    total_width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    let snapshot = state.render_snapshot();
    build_claude_startup_banner_snapshot(&snapshot, total_width, max_height)
}

pub(crate) fn render_committed_history_block(
    state: &AppState,
    message_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let snapshot = state.render_snapshot();
    render_committed_history_block_snapshot(&snapshot, message_index, width)
}

pub(crate) fn render_committed_assistant_chunk_snapshot(
    _state: &RenderSnapshot,
    content: &str,
    width: u16,
    is_continuation: bool,
) -> Vec<Line<'static>> {
    history_cell::AssistantMarkdownCell::streaming(content, is_continuation, None, None, None)
        .display_lines(width)
}

fn render_committed_assistant_text_snapshot(
    _state: &RenderSnapshot,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    render_committed_assistant_text_with_metrics(content, width, None, None, None, None)
}

pub(crate) fn render_committed_assistant_chunk(
    _state: &AppState,
    content: &str,
    width: u16,
    is_continuation: bool,
) -> Vec<Line<'static>> {
    render_committed_assistant_chunk_snapshot(
        &RenderSnapshot::new(_state),
        content,
        width,
        is_continuation,
    )
}

pub(crate) fn render_committed_assistant_text(
    _state: &AppState,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    render_committed_assistant_text_snapshot(&RenderSnapshot::new(_state), content, width)
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

fn render_live_conversation(f: &mut Frame, area: ratatui::layout::Rect, lines: Vec<Line<'static>>) {
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(COLOR_BG())),
        area,
    );
}

pub fn render(f: &mut Frame, state: &mut AppState) {
    let mut transcript = TranscriptState::default();
    let snapshot = state.render_snapshot();
    let revision = snapshot.revision();
    let (content_height, input_area) =
        render_with_transcript_snapshot(f, &snapshot, &mut transcript);
    state.publish_render_metrics(revision, content_height, input_area);
}

fn live_surface_padding(state: &RenderSnapshot) -> (u16, u16) {
    let active = matches!(state.status(), AppStatus::Streaming | AppStatus::Queued)
        || !state.running_tools().is_empty();
    (u16::from(!active), 1)
}

fn inset_vertical(area: ratatui::layout::Rect, top: u16, bottom: u16) -> ratatui::layout::Rect {
    ratatui::layout::Rect::new(
        area.x,
        area.y.saturating_add(top),
        area.width,
        area.height.saturating_sub(top.saturating_add(bottom)),
    )
}

/// Height of the mutable inline surface for the next frame. Finalized history
/// is rendered above this area into terminal scrollback.
pub(crate) fn desired_height_snapshot(
    state: &RenderSnapshot,
    transcript: &mut TranscriptState,
    width: u16,
    terminal_height: u16,
) -> u16 {
    let available = terminal_height.max(1);
    let inner_width = width.saturating_sub(2).max(1);
    let completion_dismissed =
        state.dismissed_completion() == state.completion_identity().as_deref();
    let filtered_cmds = if completion_dismissed {
        Vec::new()
    } else {
        crate::app::suggestion::filtered_commands(&state.input_buffer())
    };
    let (_, at_query) =
        crate::app::get_at_word_query(&state.input_buffer(), state.cursor_position())
            .unwrap_or((0, String::new()));
    let at_files = if !completion_dismissed
        && (!at_query.is_empty()
            || state.input_buffer()
                [..safe_byte_index(&state.input_buffer(), state.cursor_position())]
                .ends_with('@'))
    {
        crate::app::list_project_file_paths(&at_query)
    } else {
        Vec::new()
    };

    let approval_active = *state.status() == AppStatus::AwaitingToolConfirmation;
    let question_active = *state.status() == AppStatus::AwaitingQuestion;
    let input_height = if approval_active {
        tool_confirmation_height(state, available.saturating_sub(2))
    } else if question_active {
        question_height(state, width, available.saturating_sub(2))
    } else {
        count_input_lines(&state.input_buffer(), inner_width as usize).min(8) + 2
    };
    let queue_height = queue_preview_height(state);
    let popup_height = if approval_active || question_active {
        0
    } else if !filtered_cmds.is_empty() {
        (filtered_cmds.len() as u16).min(MAX_POPUP_ROWS)
    } else if !at_files.is_empty() {
        (at_files.len() as u16).min(8)
    } else {
        0
    };
    let footer_height = u16::from(composer_footer_visible(
        state,
        !filtered_cmds.is_empty(),
        !at_files.is_empty(),
    ));

    let live_lines = render_live_tail_with_transcript(state, width, available, transcript);
    let mut chat_height = Paragraph::new(live_lines)
        .wrap(Wrap { trim: false })
        .line_count(width) as u16;
    if state.history().is_empty() {
        chat_height = chat_height.max(15);
    }
    // Inline pickers are anchored above the composer and replace this portion
    // of the live tail, so reserve their tallest existing panel.
    if state.modal_open() {
        chat_height = chat_height.max(14);
    }

    let (top_padding, bottom_padding) = live_surface_padding(state);
    top_padding
        .saturating_add(bottom_padding)
        .saturating_add(chat_height)
        .saturating_add(queue_height)
        .saturating_add(input_height)
        .saturating_add(footer_height)
        .saturating_add(popup_height)
        .min(available)
        .max(1)
}

pub(crate) fn desired_height(
    state: &AppState,
    transcript: &mut TranscriptState,
    width: u16,
    terminal_height: u16,
) -> u16 {
    let snapshot = state.render_snapshot();
    desired_height_snapshot(&snapshot, transcript, width, terminal_height)
}

/// Interactive TUI entry point. `transcript` is terminal-only mutable state;
/// it must never be persisted with `ChatMessage` history or included in a
/// provider request.
pub(crate) fn render_with_transcript_snapshot(
    f: &mut Frame,
    state: &RenderSnapshot,
    transcript: &mut TranscriptState,
) -> (u16, ratatui::layout::Rect) {
    theme::set_active_theme(&state.config().theme);

    let completion_dismissed =
        state.dismissed_completion() == state.completion_identity().as_deref();
    let filtered_cmds: Vec<&CommandInfo> = if completion_dismissed {
        Vec::new()
    } else {
        crate::app::suggestion::filtered_commands(&state.input_buffer())
    };

    let inner_width = f.area().width.saturating_sub(2).max(1);
    let chat_width = f.area().width.max(1);
    let raw_input_lines = count_input_lines(&state.input_buffer(), inner_width as usize);
    let input_lines = raw_input_lines.min(8);
    let approval_active = *state.status() == AppStatus::AwaitingToolConfirmation;
    let question_active = *state.status() == AppStatus::AwaitingQuestion;
    let input_height = if approval_active {
        tool_confirmation_height(state, f.area().height.saturating_sub(2))
    } else if question_active {
        question_height(state, f.area().width, f.area().height.saturating_sub(2))
    } else {
        input_lines + 2
    };
    let queue_block_height = queue_preview_height(state);

    let (_, at_query) =
        crate::app::get_at_word_query(&state.input_buffer(), state.cursor_position())
            .unwrap_or((0, String::new()));
    let at_files = if !completion_dismissed
        && (!at_query.is_empty()
            || state.input_buffer()
                [..safe_byte_index(&state.input_buffer(), state.cursor_position())]
                .ends_with('@'))
    {
        crate::app::list_project_file_paths(&at_query)
    } else {
        Vec::new()
    };
    let popup_rows = if approval_active || question_active {
        0
    } else if !filtered_cmds.is_empty() {
        (filtered_cmds.len() as u16).min(MAX_POPUP_ROWS)
    } else if !at_files.is_empty() {
        (at_files.len() as u16).min(8)
    } else {
        0
    };
    let footer_visible =
        composer_footer_visible(state, !filtered_cmds.is_empty(), !at_files.is_empty());
    let footer_height = u16::from(footer_visible);
    let (top_padding, bottom_padding) = live_surface_padding(state);
    let vertical_padding = top_padding.saturating_add(bottom_padding);
    // Keep completion rows below the composer, matching Codex's bottom-pane
    // layout. Reserve the space before sizing the conversation so the popup
    // never overwrites transcript or the input bar.
    let popup_height = popup_rows.min(
        f.area()
            .height
            .saturating_sub(vertical_padding)
            .saturating_sub(queue_block_height)
            .saturating_sub(input_height)
            .saturating_sub(footer_height),
    );

    let max_chat_height = f
        .area()
        .height
        .saturating_sub(vertical_padding)
        .saturating_sub(queue_block_height)
        .saturating_sub(input_height)
        .saturating_sub(footer_height)
        .saturating_sub(popup_height);
    let layout_area = inset_vertical(f.area(), top_padding, bottom_padding);

    let lines = render_live_tail_with_transcript(state, chat_width, max_chat_height, transcript);
    let conversation_content_height = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(chat_width) as u16;

    let min_welcome_height = if state.history().is_empty() { 15 } else { 0 };
    let mut chat_height = conversation_area_height(conversation_content_height, max_chat_height)
        .max(min_welcome_height)
        .min(max_chat_height);
    if state.modal_open() {
        chat_height = chat_height.max(14.min(max_chat_height));
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .horizontal_margin(0)
        .constraints([
            Constraint::Length(chat_height),
            Constraint::Length(queue_block_height),
            Constraint::Length(input_height),
            Constraint::Length(footer_height),
            Constraint::Length(popup_height),
        ])
        .split(layout_area);

    render_live_conversation(f, chunks[0], lines);

    render_queue_line(f, &chunks, state);
    let input_margin = if approval_active {
        render_tool_confirmation_modal(f, state, chunks[2]);
        Margin {
            vertical: 0,
            horizontal: 0,
        }
    } else if question_active {
        render_question_modal(f, state, chunks[2]);
        Margin {
            vertical: 0,
            horizontal: 0,
        }
    } else {
        Composer::default().render(f, &chunks, state)
    };
    if footer_visible {
        render_composer_footer(f, chunks[3], state);
    }

    if !filtered_cmds.is_empty() {
        let input_inner = chunks[2].inner(input_margin);
        let popup_area = ratatui::layout::Rect::new(
            input_inner.x,
            chunks[4].y,
            input_inner.width,
            chunks[4].height,
        );
        render_popup_menu(f, state, &filtered_cmds, popup_area);
    } else if !at_files.is_empty() {
        let input_inner = chunks[2].inner(input_margin);
        let popup_area = ratatui::layout::Rect::new(
            input_inner.x,
            chunks[4].y,
            input_inner.width,
            chunks[4].height,
        );
        render_at_popup_menu(f, state, &at_files, popup_area);
    }

    let input_box_area = chunks[2];

    if state.show_model_picker() {
        render_model_picker_modal(f, state, input_box_area);
    }

    if state.show_theme_picker() {
        render_theme_picker_modal(f, state, input_box_area);
    }

    if state.show_command_picker() {
        render_command_picker_modal(f, state, input_box_area);
    }

    if state.show_history_picker() {
        render_history_picker_modal(f, state, input_box_area);
    }

    if state.show_subagent_picker() {
        render_subagent_picker_modal(f, state, input_box_area);
    }

    if state.show_context_modal() {
        render_context_modal(f, state, input_box_area);
    }

    if state.show_update_prompt() {
        render_update_prompt_modal(f, state, input_box_area);
    }

    if state.show_mcp_config() {
        render_mcp_config_modal(f, state, input_box_area);
    }

    if *state.status() == AppStatus::VerbosityPicker {
        render_verbosity_picker_modal(f, state, input_box_area);
    }

    if *state.status() == AppStatus::ThinkingPicker {
        render_thinking_picker_modal(f, state, input_box_area);
    }

    if *state.status() == AppStatus::EffortPicker {
        render_effort_picker_modal(f, state, input_box_area);
    }

    if *state.status() == AppStatus::ProtocolPicker {
        render_protocol_picker_modal(f, state, input_box_area);
    }

    if *state.status() == AppStatus::YoloPicker {
        render_yolo_picker_modal(f, state, input_box_area);
    }

    (conversation_content_height, input_box_area)
}

pub fn render_with_transcript(
    f: &mut Frame,
    state: &mut AppState,
    transcript: &mut TranscriptState,
) {
    let snapshot = state.render_snapshot();
    let revision = snapshot.revision();
    let (content_height, input_area) = render_with_transcript_snapshot(f, &snapshot, transcript);
    state.publish_render_metrics(revision, content_height, input_area);
}

#[cfg(test)]
mod tests;
