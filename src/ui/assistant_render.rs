use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CollapsedMarker {
    Image,
    PastedText,
}

pub(super) fn collapsed_marker_segments(text: &str) -> Vec<(String, Option<CollapsedMarker>)> {
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

pub(super) fn collapse_image_markers(text: &str) -> String {
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

pub(super) fn collapsed_marker_lines(text: &str) -> Vec<Vec<(String, Option<CollapsedMarker>)>> {
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

pub(super) fn model_label(state: &RenderSnapshot) -> String {
    // Only show the main (big) model — hide the small model entirely.
    state.config().default.big().to_string()
}

pub(super) struct AssistantRenderOptions {
    pub(super) token_usage: Option<crate::app::TokenUsage>,
    pub(super) response_time_ms: Option<u64>,
    pub(super) thought_time_ms: Option<u64>,
    pub(super) thought_tokens: Option<u32>,
    pub(super) is_generating: bool,
    pub(super) viewport_width: u16,
    pub(super) show_picker: bool,
    pub(super) last_copy_text: Option<(String, std::time::Instant)>,
}

pub(super) fn truncate_thought_preview(text: &str, max_width: usize) -> String {
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

pub(super) fn is_reasoning_preamble(text: &str) -> bool {
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

pub(super) fn split_thought_blocks(content: &str) -> (String, Option<String>) {
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

pub(super) fn strip_rendered_tool_blocks(content: &str) -> String {
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

pub(super) fn push_assistant_content_line<'a>(
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

pub(super) fn demote_assistant_bullet(lines: &mut [Line<'_>]) {
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
pub(super) fn assistant_fence_transition(
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

pub(super) fn render_assistant_message<'a>(
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
