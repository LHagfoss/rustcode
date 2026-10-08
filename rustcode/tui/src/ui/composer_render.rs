use super::*;

pub(super) fn wrap_input_chars(
    styled_chars: &[(char, Style)],
    inner_width: usize,
    cursor_char_index: usize,
    prompt_style: Style,
) -> (Vec<Line<'static>>, u16, u16) {
    let (lines, x, y, _) = wrap_input_chars_with_hits(
        styled_chars,
        inner_width,
        cursor_char_index,
        prompt_style,
        false,
    );
    (lines, x, y)
}

type InputHits = Vec<Vec<(u16, usize)>>;

fn wrap_input_chars_with_hits(
    styled_chars: &[(char, Style)],
    inner_width: usize,
    cursor_char_index: usize,
    prompt_style: Style,
    collect_hits: bool,
) -> (Vec<Line<'static>>, u16, u16, InputHits) {
    if inner_width == 0 {
        return (vec![Line::default()], 0, 0, vec![vec![(0, 0)]]);
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
    let mut hit_rows = Vec::with_capacity(wrapped.len());
    for (row, (characters, start)) in wrapped.into_iter().enumerate() {
        let mut spans = vec![Span::styled(
            if row == 0 { "› " } else { "  " },
            prompt_style,
        )];
        let mut current_run: Option<(Style, String)> = None;
        let mut column = indent;
        let mut hits = if collect_hits {
            vec![(column as u16, start)]
        } else {
            Vec::new()
        };
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
            if collect_hits {
                hits.push((column as u16, index + 1));
            }
            cursor_positions[index + 1] = Some((column as u16, row as u16));
        }
        if let Some((run_style, text)) = current_run {
            spans.push(Span::styled(text, run_style));
        }
        lines.push(Line::from(spans));
        if collect_hits {
            hit_rows.push(hits);
        }
    }

    let cursor = cursor_positions
        .get(cursor_char_index.min(styled_chars.len()))
        .copied()
        .flatten()
        .unwrap_or((indent as u16, 0));
    (lines, cursor.0, cursor.1, hit_rows)
}

/// Resolve a click against the same wrapping, prompt indent, and vertical scroll
/// used by the composer renderer. The returned position is a UTF-8 byte offset.
pub(crate) fn composer_cursor_from_mouse(
    input: &str,
    cursor_byte: usize,
    suggestion: Option<&str>,
    area: ratatui::layout::Rect,
    column: u16,
    row: u16,
) -> Option<usize> {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 0,
    });
    if !inner.contains(ratatui::layout::Position::new(column, row)) {
        return None;
    }

    let displayed = collapsed_marker_segments(input)
        .into_iter()
        .map(|(segment, _)| segment)
        .collect::<String>();
    let editable_chars = displayed.chars().count();
    let mut rendered = displayed;
    if input.is_empty() && suggestion.is_none() {
        rendered.push_str("Ask RustCode to do anything");
    } else if let Some(suffix) = suggestion {
        rendered.push_str(suffix);
    }
    let styled = rendered
        .chars()
        .map(|character| (character, Style::default()))
        .collect::<Vec<_>>();
    let safe_cursor = safe_byte_index(input, cursor_byte);
    let cursor_index = collapse_image_markers(&input[..safe_cursor])
        .chars()
        .count();
    let (_, _, cursor_row, hit_rows) = wrap_input_chars_with_hits(
        &styled,
        inner.width as usize,
        cursor_index,
        Style::default(),
        true,
    );
    let scroll_start = usize::from(cursor_row).saturating_sub(usize::from(inner.height) - 1);
    let clicked_row = usize::from(row - inner.y) + scroll_start;
    let hits = hit_rows.get(clicked_row)?;
    let clicked_column = column - inner.x;
    let display_index = hits
        .iter()
        .take_while(|(x, _)| *x <= clicked_column)
        .last()
        .unwrap_or(&hits[0])
        .1
        .min(editable_chars);
    Some(display_index_to_byte(input, display_index))
}

fn display_index_to_byte(input: &str, index: usize) -> usize {
    let mut raw_offset = 0;
    let mut display_offset = 0;
    for (segment, marker) in collapsed_marker_segments(input) {
        let displayed_len = segment.chars().count();
        let raw_end = match marker {
            None => raw_offset + segment.len(),
            Some(CollapsedMarker::Image) => {
                let after_prefix = &input[raw_offset + "![image](file://".len()..];
                raw_offset + "![image](file://".len() + after_prefix.find(')').unwrap_or(0) + 1
            }
            Some(CollapsedMarker::PastedText) => rustcode_core::paste::parse_at(input, raw_offset)
                .map_or(input.len(), |marker| marker.end),
        };
        if index <= display_offset + displayed_len {
            return match marker {
                None => {
                    let char_index = index - display_offset;
                    segment
                        .char_indices()
                        .nth(char_index)
                        .map_or(raw_end, |(offset, _)| raw_offset + offset)
                }
                Some(_) if index - display_offset <= displayed_len / 2 => raw_offset,
                Some(_) => raw_end,
            };
        }
        display_offset += displayed_len;
        raw_offset = raw_end;
    }
    input.len()
}

#[cfg(test)]
mod mouse_tests {
    use super::*;

    #[test]
    fn click_places_cursor_on_wrapped_unicode_and_empty_lines() {
        let area = ratatui::layout::Rect::new(3, 4, 8, 5);
        let input = "ab界cde\n\nx";
        assert_eq!(
            composer_cursor_from_mouse(input, 0, None, area, 7, 5),
            Some(2)
        );
        assert_eq!(
            composer_cursor_from_mouse(input, 0, None, area, 8, 5),
            Some(2)
        );
        assert_eq!(
            composer_cursor_from_mouse(input, 0, None, area, 5, 6),
            Some(7)
        );
        assert_eq!(
            composer_cursor_from_mouse(input, 0, None, area, 5, 7),
            Some(9)
        );
        assert_eq!(
            composer_cursor_from_mouse(input, 0, None, area, 4, 5),
            Some(0)
        );
        assert_eq!(composer_cursor_from_mouse(input, 0, None, area, 5, 4), None);
    }

    #[test]
    fn click_treats_collapsed_markers_as_atomic_text() {
        let area = ratatui::layout::Rect::new(0, 0, 40, 3);
        let input = "a![image](file:///tmp/a.png)b";
        assert_eq!(
            composer_cursor_from_mouse(input, 0, None, area, 3, 1),
            Some(1)
        );
        assert_eq!(
            composer_cursor_from_mouse(input, 0, None, area, 13, 1),
            Some(input.len() - 1)
        );
        let pasted = "a<!--PASTE:5:hello-->b";
        assert_eq!(
            composer_cursor_from_mouse(pasted, 0, None, area, 3, 1),
            Some(1)
        );
        assert_eq!(
            composer_cursor_from_mouse(pasted, 0, None, area, 30, 1),
            Some(pasted.len())
        );
    }

    #[test]
    fn click_uses_the_visible_rows_when_composer_is_scrolled() {
        let area = ratatui::layout::Rect::new(0, 0, 8, 4);
        let input = "one\ntwo\nthree";
        assert_eq!(
            composer_cursor_from_mouse(input, input.len(), None, area, 2, 1),
            Some(4)
        );
        assert_eq!(
            composer_cursor_from_mouse(input, input.len(), None, area, 2, 2),
            Some(8)
        );
    }

    #[test]
    fn byte_range_maps_plain_text_char_precisely() {
        let input = "héllo world";
        // "h" (1) + "é" (2 bytes) => byte 3 is after "hé".
        assert_eq!(composer_byte_range_to_display(input, 0, 1), Some((0, 1)));
        assert_eq!(composer_byte_range_to_display(input, 0, 3), Some((0, 2)));
        assert_eq!(composer_byte_range_to_display(input, 3, 4), Some((2, 3)));
        assert_eq!(composer_byte_range_to_display(input, 5, 5), None);
    }

    #[test]
    fn byte_range_treats_collapsed_markers_as_atomic() {
        let input = "a![image](file:///tmp/a.png)b";
        let marker_start = 1;
        // Partial overlap of the marker selects the whole "[Image #1]" label.
        let (display_start, display_end) =
            composer_byte_range_to_display(input, marker_start, marker_start + 1)
                .expect("marker overlap");
        assert!(display_end - display_start >= "[Image #1]".len());
        // Full input range covers leading char, marker label, and trailing char.
        let full = composer_byte_range_to_display(input, 0, input.len()).expect("full");
        assert!(full.1 > full.0);
    }
}

#[cfg(test)]
pub(super) fn count_input_lines(input_buffer: &str, inner_width: usize) -> u16 {
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

/// Map a byte range in `input` to display char indices in the collapsed
/// view (#1493). Collapsed image/paste markers are atomic: any overlap
/// selects the whole displayed label. Plain text maps char-precisely,
/// preserving grapheme/char boundaries established by mouse/keyboard.
pub(crate) fn composer_byte_range_to_display(
    input: &str,
    start: usize,
    end: usize,
) -> Option<(usize, usize)> {
    if start >= end || input.is_empty() {
        return None;
    }
    let mut raw_offset = 0usize;
    let mut display_offset = 0usize;
    let mut display_start: Option<usize> = None;
    let mut display_end: Option<usize> = None;
    for (segment, marker) in collapsed_marker_segments(input) {
        let displayed_len = segment.chars().count();
        let raw_end = match marker {
            None => raw_offset + segment.len(),
            Some(CollapsedMarker::Image) => {
                let after_prefix = &input[raw_offset + "![image](file://".len()..];
                raw_offset + "![image](file://".len() + after_prefix.find(')').unwrap_or(0) + 1
            }
            Some(CollapsedMarker::PastedText) => {
                rustcode_core::paste::parse_at(input, raw_offset).map_or(input.len(), |m| m.end)
            }
        };
        let overlaps = raw_offset < end && raw_end > start;
        if overlaps {
            match marker {
                Some(_) => {
                    // Atomic placeholder: include the whole label.
                    display_start = Some(display_start.unwrap_or(display_offset));
                    display_end = Some(display_offset + displayed_len);
                }
                None => {
                    // Char-precise slice within plain text.
                    let mut char_raw = raw_offset;
                    for (char_idx, ch) in segment.chars().enumerate() {
                        let char_end = char_raw + ch.len_utf8();
                        if char_raw < end && char_end > start {
                            display_start =
                                Some(display_start.unwrap_or(display_offset + char_idx));
                            display_end = Some(display_offset + char_idx + 1);
                        }
                        char_raw = char_end;
                    }
                }
            }
        }
        display_offset += displayed_len;
        raw_offset = raw_end;
    }
    match (display_start, display_end) {
        (Some(s), Some(e)) if s < e => Some((s, e)),
        _ => None,
    }
}

/// What the composer row carries while a slash-command panel is open. Panels
/// that take typed input borrow the composer for it instead of drawing their
/// own field; every other panel leaves the row as plain panel background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PanelComposer<'a> {
    Search { query: &'a str, cursor: usize },
    Hidden,
}

pub(super) fn panel_composer(state: &RenderSnapshot) -> Option<PanelComposer<'_>> {
    if !state.modal_open() {
        None
    } else if state.show_model_picker() {
        Some(PanelComposer::Search {
            query: state.model_picker_search(),
            cursor: state.model_picker_search_cursor(),
        })
    } else if state.show_command_picker() {
        Some(PanelComposer::Search {
            query: state.command_picker_search(),
            cursor: state.command_picker_search_cursor(),
        })
    } else {
        Some(PanelComposer::Hidden)
    }
}

const PANEL_SEARCH_PLACEHOLDER: &str = "Type to filter";

fn panel_search_styled_chars(query: &str) -> Vec<(char, Style)> {
    let (text, style) = if query.is_empty() {
        (
            PANEL_SEARCH_PLACEHOLDER,
            get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, false),
        )
    } else {
        (
            query,
            get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), false),
        )
    };
    text.chars().map(|character| (character, style)).collect()
}

fn input_styled_chars(state: &RenderSnapshot, show_picker: bool) -> Vec<(char, Style)> {
    match panel_composer(state) {
        Some(PanelComposer::Search { query, .. }) => return panel_search_styled_chars(query),
        Some(PanelComposer::Hidden) => return Vec::new(),
        None => {}
    }
    let text_style = get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker);
    let marker_style =
        get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker);
    let mut styled_chars = Vec::new();
    for (segment, marker) in collapsed_marker_segments(&state.input_buffer()) {
        let style = if marker.is_some() {
            marker_style
        } else {
            text_style
        };
        styled_chars.extend(segment.chars().map(|character| (character, style)));
    }
    // Highlight the composer selection (#1493). REVERSED mirrors transcript
    // selection; it applies only to input text, never placeholder/suggestion.
    if let Some((start, end)) = state
        .composer_selection_range()
        .and_then(|(s, e)| composer_byte_range_to_display(&state.input_buffer(), s, e))
    {
        let len = styled_chars.len();
        let (start, end) = (start.min(len), end.min(len));
        for (_, style) in styled_chars.iter_mut().take(end).skip(start) {
            *style = style.add_modifier(Modifier::REVERSED);
        }
    }

    if state.input_buffer().is_empty() && state.get_command_suggestion().is_none() {
        let placeholder_style =
            get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
        styled_chars.extend(
            "Ask RustCode to do anything"
                .chars()
                .map(|character| (character, placeholder_style)),
        );
    } else if let Some(suffix) = state.get_command_suggestion() {
        let suggestion_style =
            get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
        styled_chars.extend(
            suffix
                .chars()
                .map(|character| (character, suggestion_style)),
        );
    }
    styled_chars
}

pub(super) fn input_line_count(state: &RenderSnapshot, inner_width: usize) -> u16 {
    if inner_width == 0 {
        return 1;
    }
    wrap_input_chars(
        &input_styled_chars(state, false),
        inner_width,
        0,
        Style::default(),
    )
    .0
    .len() as u16
}

pub(super) fn format_token_count(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}K", tokens as f32 / 1000.0)
    } else {
        tokens.to_string()
    }
}

/// Human-sized model context window shared by the welcome banner and the
/// `/context` panel: the nearest thousand below a million (`128_000` →
/// `128k`, `262_144` → `262k`), tenths of a million above (`1_000_000` → `1M`,
/// `1_500_000` → `1.5M`), and never a spurious `.0` (#1771).
pub(in crate::ui) fn format_context_window(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    if tokens < 999_500 {
        return format!("{}k", (tokens + 500) / 1_000);
    }
    let tenths = (tokens + 50_000) / 100_000;
    if tenths % 10 == 0 {
        format!("{}M", tenths / 10)
    } else {
        format!("{}.{}M", tenths / 10, tenths % 10)
    }
}

#[derive(Clone, Copy)]
enum ActiveWorkState {
    Idle,
    Generating,
    Thinking,
    Working,
    Queued,
    Approval,
    Input,
}

impl ActiveWorkState {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Generating => "Generating",
            Self::Thinking => "Thinking",
            Self::Working => "Working",
            // A turn waiting to start is in flight like any other.
            Self::Queued => "Working",
            Self::Approval => "Awaiting approval",
            Self::Input => "Awaiting input",
        }
    }
}

/// What the turn is doing, as far as the model is concerned. Which tools are
/// running, and for how long, is the transcript's `Running` block; background
/// tasks are counted in the footer.
fn active_work_state(state: &RenderSnapshot) -> ActiveWorkState {
    match state.status() {
        AppStatus::AwaitingToolConfirmation => return ActiveWorkState::Approval,
        AppStatus::AwaitingQuestion => return ActiveWorkState::Input,
        _ => {}
    }
    if state
        .live_tool_calls()
        .iter()
        .any(|call| call.execution_started)
        || !state.running_tools().is_empty()
    {
        return ActiveWorkState::Working;
    }
    // A call that has not started is still being written by the model.
    if *state.status() == AppStatus::Streaming || !state.live_tool_calls().is_empty() {
        return if state.current_thought_started_at().is_some() {
            ActiveWorkState::Thinking
        } else {
            ActiveWorkState::Generating
        };
    }
    // The turn itself is waiting to start.
    if *state.status() == AppStatus::Queued {
        return ActiveWorkState::Queued;
    }
    ActiveWorkState::Idle
}

#[cfg(test)]
pub(super) fn activity_status_label(state: &RenderSnapshot) -> String {
    active_work_state(state).label().to_owned()
}

/// The row under the transcript: the state of the turn, the model and the
/// tokens it has produced. It never names a tool.
pub(super) fn active_work_indicator(
    state: &RenderSnapshot,
    width: u16,
    suffix: Option<Span<'static>>,
) -> Option<Line<'static>> {
    let work = active_work_state(state);
    let marker = match work {
        ActiveWorkState::Idle => return None,
        ActiveWorkState::Approval | ActiveWorkState::Input => '!',
        _ => running_spinner_char(state),
    };
    let detail = match work {
        ActiveWorkState::Approval => state
            .pending_tool_confirmation()
            .and_then(|items| items.first())
            .map(|item| format!(" · {}", item.tool_name))
            .unwrap_or_default(),
        ActiveWorkState::Input => " · answer question".to_owned(),
        _ => format!(" · {}", state.model_name()),
    };
    // The row owns exactly one terminal row and the caller appends the token
    // suffix afterwards, so head + detail + suffix must fit together. Shrink
    // the droppable detail first, then the state word; a clipped row must never
    // eat the cumulative token total (#1725).
    let muted = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false);
    let marker_span = Span::styled(
        format!("{marker} "),
        get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, false),
    );
    let head_span = Span::styled(work.label(), muted);
    let detail_span = Span::styled(detail, muted);
    Some(Line::from(fit_indicator_row(
        vec![marker_span, head_span, detail_span],
        suffix,
        usize::from(width),
    )))
}

/// Compose the reserved indicator row within `width` display columns.
///
/// `spans` are the state word followed by increasingly droppable detail;
/// `suffix` (token accounting) is reserved first and only truncated when even
/// the state word does not fit beside it.
pub(super) fn fit_indicator_row(
    mut spans: Vec<Span<'static>>,
    mut suffix: Option<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let suffix_width = suffix.as_ref().map_or(0, |span| span.content.width());
    let budget = width.saturating_sub(suffix_width);
    // Shrink the least important span (the last detail) until the row fits.
    for _ in 0..spans.len() + 1 {
        let total: usize = spans.iter().map(|span| span.content.width()).sum();
        if total <= budget {
            break;
        }
        let Some(index) = spans.iter().rposition(|span| !span.content.is_empty()) else {
            break;
        };
        let others: usize = spans
            .iter()
            .enumerate()
            .filter(|(position, span)| *position != index && !span.content.is_empty())
            .map(|(_, span)| span.content.width())
            .sum();
        let target = budget.saturating_sub(others);
        spans[index].content = if target == 0 {
            String::new().into()
        } else {
            truncate_to_display_width(&spans[index].content, target).into()
        };
    }
    if let Some(span) = &mut suffix {
        let total: usize = spans.iter().map(|part| part.content.width()).sum();
        let available = width.saturating_sub(total);
        if span.content.width() > available {
            span.content = truncate_to_display_width(&span.content, available).into();
        }
    }
    spans.extend(suffix);
    spans
}

/// Width-aware truncation with an ellipsis that never exceeds `max_width`
/// display columns (unlike char-count truncation, which overflows on wide
/// glyphs and shreds at display wrap).
pub(super) fn truncate_to_display_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    let budget = max_width.saturating_sub(1);
    let mut output = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > budget {
            break;
        }
        used += ch_width;
        output.push(ch);
    }
    output.push('…');
    output
}

/// Spinner glyph for the live running indicator at the bottom of the chat.
/// Honours reduced motion and shares the engine's frame cadence so the chat
/// row and the terminal tab title advance in step.
pub(super) fn running_spinner_char(state: &RenderSnapshot) -> char {
    if state.config().reduced_motion {
        return '•';
    }
    #[cfg(test)]
    let elapsed = Duration::ZERO;
    // Read the shared timeline rather than a local clock so this row and the
    // terminal tab title advance in step (see SPINNER_FRAME_MS).
    #[cfg(not(test))]
    let elapsed = rustcode::controller::spinner_elapsed();
    rustcode::controller::spinner_frame(elapsed)
}

/// The footer's task counter: how many background tasks are running, and how
/// many finished without the model having read the result yet. `None` when
/// there is nothing to count. `/tasks`, or a click on it, lists them.
pub(super) fn tasks_chip_label(state: &RenderSnapshot) -> Option<String> {
    let running = state.background_tasks().len();
    let done = state
        .pending_background_results()
        .iter()
        .filter(|result| result.unread)
        .count();
    let tasks = |count: usize| {
        if count == 1 {
            "1 task".to_owned()
        } else {
            format!("{count} tasks")
        }
    };
    match (running, done) {
        (0, 0) => None,
        (running, 0) => Some(tasks(running)),
        (0, done) => Some(format!("{done} done")),
        (running, done) => Some(format!("{} · {done} done", tasks(running))),
    }
}

pub(super) fn fmt_elapsed_compact(elapsed_secs: u64) -> String {
    rustcode_core::status::format_elapsed_compact(elapsed_secs)
}

fn decode_speed_label(state: &RenderSnapshot) -> Option<String> {
    if *state.status() != AppStatus::Streaming {
        return None;
    }
    let (tokens_per_second, _) = state.stream_tracker()?.snapshot();
    (tokens_per_second >= 0.05).then(|| format!("Tokens/s: {tokens_per_second:.1}"))
}

/// Maximum queued user prompts previewed above the composer.
pub(super) const MAX_QUEUE_PREVIEW_ROWS: usize = 3;
const MAX_QUEUE_PROMPT_PREVIEW_LINES: usize = 2;

pub(super) fn pending_steer_prompts(state: &RenderSnapshot) -> Vec<&str> {
    state
        .pending_steers()
        .iter()
        .rev()
        .take(MAX_QUEUE_PREVIEW_ROWS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

pub(super) fn queued_user_prompts(state: &RenderSnapshot) -> Vec<&str> {
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

pub(super) fn queue_preview_height(state: &RenderSnapshot, width: usize) -> u16 {
    let steers = pending_steer_prompts(state);
    let prompts = queued_user_prompts(state);
    let prompt_width = width.saturating_sub("  › ".width()).max(1);
    let steer_height = if steers.is_empty() {
        0
    } else {
        1 + steers
            .iter()
            .map(|prompt| queue_prompt_preview_lines(prompt, prompt_width).len() as u16)
            .sum::<u16>()
    };
    let queue_height = if prompts.is_empty() {
        0
    } else {
        1 + prompts
            .iter()
            .map(|prompt| queue_prompt_preview_lines(prompt, prompt_width).len() as u16)
            .sum::<u16>()
    };
    steer_height + queue_height
}

pub(super) fn queue_prompt_preview_lines(prompt: &str, max_width: usize) -> Vec<String> {
    let max_width = max_width.max(1);
    let prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if prompt.is_empty() {
        return vec![String::new()];
    }

    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut truncated = false;
    for character in prompt.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if current_width + character_width > max_width {
            if rows.len() + 1 == MAX_QUEUE_PROMPT_PREVIEW_LINES {
                truncated = true;
                break;
            }
            rows.push(std::mem::take(&mut current));
            current_width = 0;
            if character.is_whitespace() {
                continue;
            }
        }
        current.push(character);
        current_width += character_width;
    }
    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
    }
    if truncated {
        if let Some(last) = rows.last_mut() {
            *last = format!(
                "{}…",
                truncate_queue_prompt(last, max_width.saturating_sub(1))
            );
        }
    }
    rows
}

pub(super) fn truncate_queue_prompt(prompt: &str, max_width: usize) -> String {
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

/// Shows pending steers and the most recent queued user prompts directly
/// above the input box. Internal wakeups stay queued but never consume
/// composer space or leak into this transcript-like preview.
pub(super) fn render_queue_line(
    f: &mut Frame,
    chunks: &[ratatui::layout::Rect],
    state: &RenderSnapshot,
) {
    let steers = pending_steer_prompts(state);
    let prompts = queued_user_prompts(state);
    if steers.is_empty() && prompts.is_empty() {
        return;
    }
    let block = chunks[1];
    if block.height == 0 {
        return;
    }
    let show_picker = state.modal_open();
    let mut row_offset = 0u16;
    if !steers.is_empty() {
        let header = Line::from(Span::styled(
            truncate_queue_prompt(
                "pending steering · applies after a result or turn ends",
                block.width as usize,
            ),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        f.render_widget(
            Paragraph::new(header).style(Style::default().bg(COLOR_BG())),
            ratatui::layout::Rect::new(block.x, block.y, block.width, 1),
        );
        for steer in steers {
            row_offset += render_preview_prompt(f, block, row_offset + 1, steer, show_picker);
        }
        row_offset += 1;
    }
    if prompts.is_empty() {
        return;
    }
    let queued_count = state
        .pending_queue()
        .iter()
        .filter(|prompt| !prompt.starts_with("__task_wakeup__:"))
        .count();
    let header = Line::from(Span::styled(
        format!("queued follow-ups ({queued_count}) · ↑ edit last"),
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    ));
    f.render_widget(
        Paragraph::new(header).style(Style::default().bg(COLOR_BG())),
        ratatui::layout::Rect::new(block.x, block.y + row_offset, block.width, 1),
    );

    row_offset += 1;
    for prompt in prompts {
        row_offset += render_preview_prompt(f, block, row_offset, prompt, show_picker);
    }
}

fn render_preview_prompt(
    f: &mut Frame,
    block: ratatui::layout::Rect,
    row: u16,
    prompt: &str,
    show_picker: bool,
) -> u16 {
    let prefix = "  › ";
    let continuation = "    ";
    let rows = queue_prompt_preview_lines(
        prompt,
        (block.width as usize).saturating_sub(prefix.width()).max(1),
    );
    for (index, preview) in rows.iter().enumerate() {
        let row_prefix = if index == 0 { prefix } else { continuation };
        let line = Line::from(vec![
            Span::styled(
                row_prefix,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                preview.as_str(),
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(COLOR_BG())),
            ratatui::layout::Rect::new(block.x, block.y + row + index as u16, block.width, 1),
        );
    }
    rows.len() as u16
}

pub(crate) fn render_input(
    f: &mut Frame,
    chunks: &[ratatui::layout::Rect],
    state: &RenderSnapshot,
) -> Margin {
    let show_picker = state.modal_open();
    let area = chunks[3];
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

    let panel = panel_composer(state);
    if panel == Some(PanelComposer::Hidden) {
        return input_margin;
    }
    let panel_search = match panel {
        Some(PanelComposer::Search { query, cursor }) => Some((query, cursor)),
        _ => None,
    };
    let show_picker = show_picker && panel_search.is_none();

    let inner_width = input_inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_dx = 0u16;
    let mut cursor_dy = 0u16;

    if let Some((query, cursor_byte)) = panel_search.filter(|_| inner_width > 0) {
        let cursor_byte = safe_byte_index(query, cursor_byte);
        let cursor_char = query[..cursor_byte].chars().count();
        let prompt_style = get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, false);
        (lines, cursor_dx, cursor_dy) = wrap_input_chars(
            &panel_search_styled_chars(query),
            inner_width,
            cursor_char,
            prompt_style,
        );
    } else if inner_width > 0 {
        let styled_chars = input_styled_chars(state, show_picker);

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
    let visible_height = text_area_height as usize;
    let cursor_row = cursor_dy as usize;
    let scroll_start = if visible_height == 0 {
        0
    } else {
        cursor_row.saturating_sub(visible_height.saturating_sub(1))
    };
    let visible_lines = if visible_height == 0 {
        Vec::new()
    } else {
        lines
            .into_iter()
            .skip(scroll_start)
            .take(visible_height)
            .collect::<Vec<_>>()
    };
    let cursor_dy = cursor_dy.saturating_sub(scroll_start as u16);
    let text_area = input_inner;
    let paragraph = Paragraph::new(visible_lines).style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(paragraph, text_area);

    if inner_width > 0 && !show_picker {
        f.set_cursor_position((
            input_inner.x + cursor_dx.min(input_inner.width.saturating_sub(1)),
            input_inner.y + cursor_dy.min(text_area_height.saturating_sub(1)),
        ));
    }

    input_margin
}

pub(super) fn composer_footer_visible(state: &RenderSnapshot) -> bool {
    // Panels hide the standing session metadata, but never a feedback row:
    // a one-shot notice (e.g. "Copied selection to clipboard") is the answer
    // to the key just pressed and must stay visible while a slash-command
    // panel is open.
    !state.modal_open() || state.transient_notice().is_some()
}

/// Hint shown under the composer while an inline completion popup is open. The
/// clauses are ordered by how much the user needs them: the keys that move and
/// select the suggestion come first, the trailing affordances come last. The
/// footer row is reserved even when every clause is dropped, so the composer
/// does not jump as the popup opens and closes.
pub(super) const COMPLETION_HINT_CLAUSES: [&str; 3] =
    ["↑/↓ navigate", "enter select", "esc dismiss"];
pub(super) const COMMAND_COMPLETION_HINT_CLAUSES: [&str; 4] = [
    "↑/↓ navigate",
    "enter select",
    "tab complete",
    "esc dismiss",
];

/// Separator between hint clauses, matching the `·` the footer used to render.
const HINT_SEPARATOR: &str = " · ";

pub(super) fn completion_footer_hint_clauses(
    has_command_completions: bool,
) -> &'static [&'static str] {
    if has_command_completions {
        &COMMAND_COMPLETION_HINT_CLAUSES
    } else {
        &COMPLETION_HINT_CLAUSES
    }
}

/// Clauses shown in the footer while a transcript selection is active.
///
/// The selection is the modal gesture here: it claims the copy chord before any
/// other handler sees the key (`selection_owns_key` in the runtime), so the key
/// that copies it is the footer's *leading* clause and therefore the one clause
/// that survives the narrowest row. `or right-click` follows it as the second
/// copy path (`selection.rs`: copy needs Ctrl+C or a right-click on the
/// selection -- terminals such as Apple Terminal reserve Cmd+C for native
/// selection and do not forward it, though other terminals may, see #1566),
/// so the shortest still-truthful form -- the key alone -- is what a narrow
/// row keeps. Both are whole clauses of `fit_hint_clauses`, so nothing is
/// ever clipped mid-affordance (#1529).
pub(super) fn selection_hint_clauses() -> [&'static str; 2] {
    [
        rustcode::controller::copy_selection_binding(),
        "or right-click",
    ]
}

/// The footer's key hints in drop order, or `None` when the row has no hint.
///
/// A live selection leads. Its copy chord is claimed outright -- pressing it can
/// only ever reach the selection -- so it outranks both the completion popup's
/// clauses and the passive session metadata the hint replaces. The popup's own
/// clauses follow and degrade first. One `fit_hint_clauses` prefix covers both
/// families, so the order of this list *is* the footer's drop order.
pub(super) fn footer_hint_clauses(
    popup_hint: Option<&'static [&'static str]>,
    selection_active: bool,
) -> Option<Vec<&'static str>> {
    let copy = selection_active.then(selection_hint_clauses);
    let popup = popup_hint.unwrap_or_default();
    if copy.is_none() && popup.is_empty() {
        return None;
    }
    Some(
        copy.into_iter()
            .flatten()
            .chain(popup.iter().copied())
            .collect(),
    )
}

/// Longest prefix of `clauses` whose text, prefixed by `prefix`, fits `width`,
/// or `None` when not even the first clause fits.
///
/// Hints degrade by content, never by character position: a clause that does
/// not fit is dropped whole instead of being clipped mid-affordance, which is
/// the rule the welcome banner already follows (`#1529`).
pub(super) fn fit_hint_clauses(
    prefix: &str,
    clauses: &[&'static str],
    width: usize,
) -> Option<String> {
    let mut kept = 0;
    let mut used = prefix.width();
    for clause in clauses {
        let extra = clause.width() + if kept == 0 { 0 } else { HINT_SEPARATOR.width() };
        if used + extra > width {
            break;
        }
        used += extra;
        kept += 1;
    }
    (kept > 0).then(|| format!("{prefix}{}", clauses[..kept].join(HINT_SEPARATOR)))
}

pub(super) fn footer_location(state: &RenderSnapshot) -> String {
    let (path, branch) = state
        .cwd_and_branch()
        .rsplit_once(':')
        .unwrap_or((&state.cwd_and_branch(), "unknown"));
    let branch = if branch.is_empty() { "unknown" } else { branch };
    let branch = fit_to_width(branch, 24).trim_end().to_string();
    let path = if path.is_empty() { "~" } else { path };
    format!("{branch} · {path}")
}

/// Paint the footer. Returns where the task counter was drawn, so the pointer
/// can hover and click it.
pub(super) fn render_composer_footer(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: &RenderSnapshot,
    popup_hint: Option<&'static [&'static str]>,
    selection_active: bool,
    tasks_hovered: bool,
) -> Option<ratatui::layout::Rect> {
    if area.height == 0 || area.width == 0 {
        return None;
    }

    let used = super::context_usage::context_usage(state).used_tokens;
    let window = state.active_context_window().max(1);
    let remaining = rustcode_core::status::context_remaining_percent(used, window);
    let location = footer_location(state);
    let hint_clauses = footer_hint_clauses(popup_hint, selection_active);
    let (left_content, left_style, hint_clauses) = if state.ctrl_c_exit_armed() {
        // A second Ctrl+C is a pending exit, which outranks every hint. The
        // same press may also have copied a selection, so keep that answer
        // visible rather than letting the exit hint swallow it.
        let copied = state
            .transient_notice()
            .is_some_and(|notice| notice.starts_with("Copied") || notice.starts_with("Copy "));
        (
            if copied {
                format!(
                    "  {} · ⚠ Press Ctrl+C again to exit",
                    state.transient_notice().unwrap_or_default()
                )
            } else {
                "  ⚠ Press Ctrl+C again to exit".to_owned()
            },
            get_themed_style(Color::Yellow, COLOR_BG(), Modifier::BOLD, false),
            None,
        )
    } else if let Some(notice) = state.transient_notice() {
        // A notice is the answer to the key that was just pressed, so it outranks
        // the standing hints: right after a copy the footer must read "Copied
        // selection to clipboard", not the copy key sitting next to it.
        (
            format!("  {notice}"),
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, false),
            None,
        )
    } else if let Some(clauses) = hint_clauses {
        // A hint names a key the user can press, so it replaces the session
        // metadata rather than competing with it for the same row.
        (
            String::new(),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
            Some(clauses),
        )
    } else {
        let mut metadata = Vec::new();
        if let Some(agent) = state.selected_subagent() {
            metadata.push(agent.name().to_string());
        }
        // The running indicator lives at the bottom of the chat now, so the
        // footer keeps only the model and workspace.
        metadata.push(state.model_name().to_string());
        metadata.push(location);
        (
            format!("  {}", metadata.join(" · ")),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
            None,
        )
    };
    let row_width = area.width as usize;
    let speed = decode_speed_label(state)
        .map(|speed| format!("{speed}  "))
        .unwrap_or_default();
    let context_right = format!("{remaining}% context left  ");
    let speed_and_context = format!("{speed}{context_right}");
    let right = if speed_and_context.width() <= row_width {
        speed_and_context
    } else {
        context_right
    };
    let right_style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false);
    let right_width = right.width();
    // The hint is the actionable content on this row, so it claims the width
    // first and the context percentage yields when the two compete: degrade the
    // hint against the remaining space, and only drop the percentage when even
    // the leading clause no longer fits beside it (#1529).
    let chip_label = tasks_chip_label(state).map(|label| format!(" {label} "));
    let chip_reserve = chip_label
        .as_ref()
        .map_or(0, |chip| chip.width() + 2)
        .min(row_width.saturating_sub(right_width) / 2);
    let (left, keep_right) = match hint_clauses {
        Some(clauses) => {
            let beside_right =
                fit_hint_clauses("  ", &clauses, row_width.saturating_sub(right_width));
            match beside_right {
                Some(hint) => (hint, true),
                None => (
                    fit_hint_clauses("  ", &clauses, row_width).unwrap_or_default(),
                    false,
                ),
            }
        }
        // Session metadata and one-shot notices stay clipped: they name the
        // model and workspace rather than offering a key the user can press.
        // The task counter is something to click, so it keeps its columns.
        None => (
            fit_to_width(
                &left_content,
                row_width.saturating_sub(right_width + chip_reserve),
            ),
            true,
        ),
    };
    let (right, right_width) = if keep_right {
        (right, right_width)
    } else {
        (String::new(), 0)
    };
    // The task counter sits left of the context figure and yields first.
    let chip = chip_label
        .filter(|chip| keep_right && left.width() + chip.width() + 2 + right_width <= row_width);
    let chip_width = chip.as_ref().map_or(0, |chip| chip.width() + 2);
    let padding = row_width.saturating_sub(left.width() + chip_width + right_width);
    let mut spans = vec![
        Span::styled(left, left_style),
        Span::styled(" ".repeat(padding), Style::default().bg(COLOR_BG())),
    ];
    let mut chip_area = None;
    if let Some(chip) = chip {
        let x = area.x + (row_width - right_width - chip_width) as u16;
        chip_area = Some(ratatui::layout::Rect::new(
            x,
            area.y,
            chip.width() as u16,
            1,
        ));
        spans.push(Span::styled(
            chip,
            get_themed_style(
                COLOR_PRIMARY(),
                if tasks_hovered {
                    COLOR_HOVER_BG()
                } else {
                    COLOR_BG()
                },
                Modifier::BOLD,
                false,
            ),
        ));
        spans.push(Span::styled("  ", Style::default().bg(COLOR_BG())));
    }
    spans.push(Span::styled(right, right_style));
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(COLOR_BG())),
        area,
    );
    chip_area
}
