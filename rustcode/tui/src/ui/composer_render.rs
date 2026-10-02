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

fn input_styled_chars(state: &RenderSnapshot, show_picker: bool) -> Vec<(char, Style)> {
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

pub(super) fn activity_status_label(state: &RenderSnapshot) -> String {
    let base_activity =
        rustcode::controller::classify_activity(&state.status(), &state.running_tools());
    let activity = if base_activity.kind == rustcode::controller::ActivityKind::ActionRequired {
        base_activity
    } else {
        rustcode::controller::classify_live_tools(&state.live_tool_calls()).unwrap_or(base_activity)
    };
    if activity.kind == rustcode::controller::ActivityKind::ActionRequired {
        return "Action Required".to_string();
    }
    if activity.kind == rustcode::controller::ActivityKind::Queued {
        return "Queued".to_string();
    }
    if activity.kind == rustcode::controller::ActivityKind::Ready {
        return "Idle".to_string();
    }
    if state.current_thought_started_at().is_some() {
        return "Thinking".to_string();
    }
    "Working".to_string()
}

fn background_spinner_frame() -> char {
    const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    #[cfg(test)]
    let elapsed = Duration::ZERO;
    #[cfg(not(test))]
    let elapsed = BACKGROUND_SPINNER_START.get_or_init(Instant::now).elapsed();
    let frame = (elapsed.as_millis() / 120) as usize % FRAMES.len();
    FRAMES[frame]
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

pub(super) fn background_terminal_summary(state: &RenderSnapshot) -> String {
    const MAX_VISIBLE_COMMANDS: usize = 3;
    const COMMAND_LABEL_CHARS: usize = 36;

    let mut tasks = state.background_tasks().iter().collect::<Vec<_>>();
    tasks.sort_by_key(|task| task.started_at);
    let count = tasks.len();
    let elapsed = tasks
        .iter()
        .map(|task| task.started_at)
        .next()
        .map(|started| fmt_elapsed_compact(started.elapsed().as_secs()))
        .unwrap_or_else(|| "0s".to_string());
    let mut parts = vec![format!("{count} running ({elapsed})")];
    parts.extend(tasks.iter().take(MAX_VISIBLE_COMMANDS).map(|task| {
        let label =
            rustcode::controller::background_command_label(&task.command, COMMAND_LABEL_CHARS);
        if label.is_empty() {
            format!("task {}", task.id)
        } else {
            label
        }
    }));
    if count > MAX_VISIBLE_COMMANDS {
        parts.push(format!("{} more", count - MAX_VISIBLE_COMMANDS));
    }
    parts.extend(["/ps".to_string(), "/stop".to_string()]);
    format!("{} {}", background_spinner_frame(), parts.join(" · "))
}

pub(super) fn background_command_lines(state: &RenderSnapshot) -> Vec<Line<'static>> {
    const MAX_VISIBLE_COMMANDS: usize = 3;
    let style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false);
    let mut lines = state
        .background_tasks()
        .iter()
        .take(MAX_VISIBLE_COMMANDS)
        .map(|task| {
            let command = rustcode::controller::background_command_label(&task.command, 240);
            Line::from(Span::styled(format!("  └ {command}"), style))
        })
        .collect::<Vec<_>>();
    let omitted = state
        .background_tasks()
        .len()
        .saturating_sub(MAX_VISIBLE_COMMANDS);
    if omitted > 0 {
        lines.push(Line::from(Span::styled(
            format!("  └ … {omitted} more (/ps to view)"),
            style,
        )));
    }
    lines
}

pub(super) fn blend_rgb(c1: (u8, u8, u8), c2: (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let f = factor.clamp(0.0, 1.0);
    let r = (c1.0 as f32 * f + c2.0 as f32 * (1.0 - f)) as u8;
    let g = (c1.1 as f32 * f + c2.1 as f32 * (1.0 - f)) as u8;
    let b = (c1.2 as f32 * f + c2.2 as f32 * (1.0 - f)) as u8;
    (r, g, b)
}

#[cfg(not(test))]
static BACKGROUND_SPINNER_START: OnceLock<Instant> = OnceLock::new();

pub(super) fn shimmer_rgb(color: Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    }
}

pub(super) fn shimmer_spans_at(text: &str, elapsed: Duration) -> Vec<Span<'static>> {
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

/// Plain muted label used when the animated sweep is turned off.
pub(super) fn static_spans(text: &str, show_picker: bool) -> Vec<Span<'static>> {
    vec![Span::styled(
        text.to_string(),
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    )]
}

pub(super) fn shimmer_spans(
    text: &str,
    show_picker: bool,
    reduced_motion: bool,
) -> Vec<Span<'static>> {
    if reduced_motion {
        return static_spans(text, show_picker);
    }
    #[cfg(test)]
    let elapsed = Duration::ZERO;
    #[cfg(not(test))]
    let elapsed = {
        // Quantize the sweep to the 120ms spinner cadence so timer-only ticks
        // within the same bucket render identical rows (#1632). Without this
        // every 16ms frame produced new RGB values and invalidated cached
        // frames even with identical chat content. Read the shared timeline so
        // the sweep and the spinner glyph stay in step.
        let raw = rustcode::controller::spinner_elapsed();
        Duration::from_millis(u64::try_from(raw.as_millis() / 120 * 120).unwrap_or(u64::MAX))
    };
    shimmer_spans_at(text, elapsed)
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

pub(super) fn activity_status_line(
    state: &RenderSnapshot,
    show_picker: bool,
    width: usize,
) -> Line<'static> {
    let base_activity =
        rustcode::controller::classify_activity(&state.status(), &state.running_tools());
    let activity = if base_activity.kind == rustcode::controller::ActivityKind::ActionRequired {
        base_activity
    } else {
        rustcode::controller::classify_live_tools(&state.live_tool_calls()).unwrap_or(base_activity)
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
        rustcode::controller::ActivityKind::ActionRequired => "!",
        rustcode::controller::ActivityKind::Ready => "◦",
        _ => "•",
    };
    let bullet_color = match activity.kind {
        rustcode::controller::ActivityKind::ActionRequired => Color::Yellow,
        rustcode::controller::ActivityKind::Ready => COLOR_MUTED(),
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
        rustcode::controller::ActivityKind::Working
            | rustcode::controller::ActivityKind::RunningTool
    ) {
        spans.extend(shimmer_spans(
            &label_text,
            show_picker,
            state.config().reduced_motion,
        ));
    } else {
        spans.push(Span::styled(
            label_text,
            get_themed_style(
                if activity.kind == rustcode::controller::ActivityKind::ActionRequired {
                    Color::Yellow
                } else if activity.kind == rustcode::controller::ActivityKind::Ready {
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

    if activity.kind == rustcode::controller::ActivityKind::ActionRequired {
        if let Some(detail) = action_detail {
            spans.push(Span::styled(
                format!(" · {detail}"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    }

    if matches!(
        activity.kind,
        rustcode::controller::ActivityKind::Working
            | rustcode::controller::ActivityKind::RunningTool
    ) && let Some(started) = state.generation_start_time()
    {
        spans.push(Span::styled(
            format!(" ({})", fmt_elapsed_compact(started.elapsed().as_secs())),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }

    if !state.background_tasks().is_empty() {
        spans.push(Span::styled(
            format!(" · {}", background_terminal_summary(state)),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }

    // The esc and steer-mode hints follow the same drop-don't-clip rule as the
    // footer hint: a clause that does not fit the row is omitted whole, so the
    // line never ends mid-affordance (#1529).
    let hint_style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker);
    let mut used: usize = spans.iter().map(|span| span.content.width()).sum();
    let mut push_hint = |spans: &mut Vec<Span<'static>>, clauses: &[&'static str]| {
        if let Some(hint) = fit_hint_clauses(HINT_SEPARATOR, clauses, width.saturating_sub(used)) {
            used += hint.width();
            spans.push(Span::styled(hint, hint_style));
        }
    };

    if matches!(
        activity.kind,
        rustcode::controller::ActivityKind::Working
            | rustcode::controller::ActivityKind::RunningTool
    ) {
        // Esc only interrupts the model stream; background terminals survive
        // it (issue #1223). Say so when a background job is actually running.
        let clauses: &[&'static str] =
            if state.steering_escape_will_interrupt() && !state.pending_steers().is_empty() {
                &["esc interrupt and apply now"]
            } else if !state.pending_steers().is_empty() {
                &[]
            } else if state.background_tasks().is_empty() {
                &["esc interrupt"]
            } else {
                &["esc interrupts stream only"]
            };
        push_hint(&mut spans, clauses);
    }

    if state.show_steer_mode_hint() {
        // The mode names what a keystroke will do, so it outlives the key that
        // flips it.
        push_hint(
            &mut spans,
            match state.draft_submit_mode() {
                rustcode::controller::DraftSubmitMode::Steer => &["Steer", "Tab switches to Queue"],
                rustcode::controller::DraftSubmitMode::Queue => &["Queue", "Tab switches to Steer"],
            },
        );
    }

    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Maximum queued user prompts previewed above the composer.
pub(super) const MAX_QUEUE_PREVIEW_ROWS: usize = 3;

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

pub(super) fn queue_preview_height(state: &RenderSnapshot) -> u16 {
    let steer_rows = pending_steer_prompts(state).len();
    let queue_rows = queued_user_prompts(state).len();
    let steer_height = if steer_rows == 0 {
        0
    } else {
        steer_rows as u16 + 1
    };
    let queue_height = if queue_rows == 0 {
        0
    } else {
        queue_rows as u16 + 1
    };
    steer_height + queue_height
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
            "pending steers · apply after next tool result or when the turn ends",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        f.render_widget(
            Paragraph::new(header).style(Style::default().bg(COLOR_BG())),
            ratatui::layout::Rect::new(block.x, block.y, block.width, 1),
        );
        for (row, steer) in steers.into_iter().enumerate() {
            render_preview_prompt(f, block, row_offset + row as u16 + 1, steer, show_picker);
        }
        row_offset += pending_steer_prompts(state).len() as u16 + 1;
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

    for (row, prompt) in prompts.into_iter().enumerate() {
        render_preview_prompt(f, block, row_offset + row as u16 + 1, prompt, show_picker);
    }
}

fn render_preview_prompt(
    f: &mut Frame,
    block: ratatui::layout::Rect,
    row: u16,
    prompt: &str,
    show_picker: bool,
) {
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
        ratatui::layout::Rect::new(block.x, block.y + row, block.width, 1),
    );
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

    let inner_width = input_inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_dx = 0u16;
    let mut cursor_dy = 0u16;

    if inner_width > 0 {
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
/// selection -- Cmd+C is the terminal's native selection and never reaches
/// the app on macOS, see #1566), so the shortest still-truthful form -- the
/// key alone -- is what a narrow row keeps. Both are whole clauses of `fit_hint_clauses`, so nothing is
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

pub(super) fn render_composer_footer(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: &RenderSnapshot,
    popup_hint: Option<&'static [&'static str]>,
    selection_active: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let used = super::context_usage::context_usage(state).used_tokens;
    let window = state.active_context_window().max(1);
    let remaining = rustcode_core::status::context_remaining_percent(used, window);
    let location = footer_location(state);
    let hint_clauses = footer_hint_clauses(popup_hint, selection_active);
    let (left_content, left_style, hint_clauses) = if state.ctrl_c_exit_armed() {
        // A second Ctrl+C is a pending exit, which outranks every hint.
        (
            "  ⚠ Press Ctrl+C again to exit".to_owned(),
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
        None => (
            fit_to_width(&left_content, row_width.saturating_sub(right_width)),
            true,
        ),
    };
    let (right, right_width) = if keep_right {
        (right, right_width)
    } else {
        (String::new(), 0)
    };
    let padding = row_width.saturating_sub(left.width() + right_width);
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
