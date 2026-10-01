use super::*;
#[cfg(test)]
use crate::ui::render_snapshot::render_snapshot;
use std::sync::Arc;

pub(super) fn conversation_area_height(content_height: u16, available_height: u16) -> u16 {
    if available_height == 0 {
        return 0;
    }
    content_height.min(available_height)
}

/// Render only the mutable portion of the current turn. Completed history is
/// deliberately excluded: the projection above already includes it.
#[cfg(test)]
pub(super) fn render_live_tail_snapshot(
    state: &RenderSnapshot,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let mut transcript = TranscriptState::default();
    render_live_tail_with_transcript(state, width, height, &mut transcript)
}

#[cfg(test)]
pub(crate) fn render_live_tail(view: &RenderState, width: u16, height: u16) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(view);
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
    render_live_tail_mode(state, width, height, transcript, false)
}

fn render_live_tail_mode(
    state: &RenderSnapshot,
    width: u16,
    height: u16,
    transcript: &mut TranscriptState,
    full_viewport: bool,
) -> Vec<Line<'static>> {
    if state.selected_subagent().is_some() {
        return render_selected_subagent_context(state, width, height);
    }

    if welcome_is_live(state) {
        return build_claude_startup_banner_snapshot(state, width as usize, height as usize);
    }

    let tail = if full_viewport {
        state.current_response().to_owned()
    } else {
        scrollback::mutable_stream_text(&state.current_response())
    };
    let mut lines = Vec::new();

    let mut has_visible_active_cell = false;
    let mut model_live_text = "";
    let visible_live_tool_calls = state
        .live_tool_calls()
        .iter()
        .filter(|call| is_live_tool_call_visible(call))
        .cloned()
        .collect::<Vec<_>>();
    if !tail.is_empty() {
        let parsed_tool =
            rustcode_tool_protocol::parse_tool_call(&tail, state.active_tool_protocol());
        let is_tool_syntax = rustcode_tool_protocol::is_tool_call_start(&tail);
        let should_hide_stream = match parsed_tool {
            Some(ref tool_call) => !rustcode_tool_protocol::is_code_editing_tool(&tool_call.name),
            None => is_tool_syntax,
        };

        if !should_hide_stream {
            model_live_text = &tail;
        }
    }

    // A tool may start after the assistant's thought has already entered
    // committed history while current_response still holds the same text.
    // Keep the committed thought and the live tool row, but do not paint the
    // stale thought a second time below the tool.
    if !visible_live_tool_calls.is_empty()
        && state.history().last().is_some_and(|message| {
            message.role == "assistant" && message.content.trim() == model_live_text.trim()
        })
    {
        model_live_text = "";
    }

    if visible_live_tool_calls.is_empty() {
        transcript.clear_tools();
    } else {
        transcript.set_tools_with_verbosity(&visible_live_tool_calls, &state.verbosity());
        has_visible_active_cell = true;
    }
    if model_live_text.is_empty() {
        transcript.clear_assistant();
    } else {
        has_visible_active_cell = true;
    }

    transcript.sync_model(&state.history(), model_live_text);
    let model_tail = transcript
        .model()
        .live_text()
        .unwrap_or_default()
        .to_owned();

    if !model_live_text.is_empty() {
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
            !full_viewport && scrollback::mutable_stream_is_continuation(&state.current_response()),
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
        || !state.running_tools().is_empty()
        || !state.background_tasks().is_empty();
    if activity_visible && !full_viewport {
        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::from(""));
        }
        lines.push(activity_status_line(state, false, width as usize));
        lines.extend(background_command_lines(state));
        lines.push(Line::from(""));
    }

    if state.recap_loading() {
        lines.extend(render_conversation_recap(
            "Generating conversation recap…",
            width,
        ));
    }

    if height == 0 && full_viewport {
        return Vec::new();
    }
    if height > 0 && lines.len() > height as usize {
        let visible_start = lines.len() - height as usize;
        lines = lines.split_off(visible_start);
    }

    lines.into_iter().map(|line| own_line(&line)).collect()
}

fn welcome_is_live(state: &RenderSnapshot) -> bool {
    (state.history().is_empty() || state.history_display_start() >= state.history().len())
        && state.current_response().is_empty()
        && matches!(state.status(), AppStatus::Idle)
        && state.running_tools().is_empty()
        && state.live_tool_calls().is_empty()
        && state.background_tasks().is_empty()
}

/// Keep recent committed messages visible while the composer owns the full
/// terminal viewport. The terminal still records each message in scrollback;
/// this projection supplies the on-screen chat that would otherwise disappear
/// behind a full-height mutable viewport.
pub(crate) fn render_visible_conversation_with_transcript(
    state: &RenderSnapshot,
    width: u16,
    height: u16,
    transcript: &mut TranscriptState,
) -> Vec<Line<'static>> {
    let history_len = state.history().len();
    let history_revision = state.history().revision();
    // Anything that can add rows below the reader: committed history and the
    // live stream both land at the tail of the same projection.
    let content_mark = (
        history_revision,
        state
            .current_response()
            .len()
            .saturating_mul(2)
            .saturating_add(usize::from(state.recap_loading())),
    );
    let content_changed = transcript.last_content() != Some(content_mark);
    let display_start = state.history_display_start().min(history_len);
    let mut measured_tail = None;
    if height > 0
        && transcript.scroll_rows() > 0
        && let Some(anchor) = transcript.reading_anchor
        && anchor.width == width
        && anchor.display_start == display_start
        && anchor.history_len <= history_len
        && anchor.tail_start >= display_start
    {
        let tail_rows = if anchor.history_revision == history_revision {
            anchor.tail_rows
        } else {
            committed_suffix_rows(state, width, transcript, anchor.tail_start)
        };
        measured_tail = Some((anchor.tail_start, tail_rows));
        let added_rows = tail_rows as isize - anchor.tail_rows as isize;
        let height_change = anchor.height as isize - height as isize;
        transcript.shift_reading_offset(added_rows.saturating_add(height_change));
    }
    let live_height = if transcript.scroll_rows() > 0 && !state.history().is_empty() {
        0
    } else {
        height
    };
    let live = render_live_tail_mode(state, width, live_height, transcript, true);
    if height == 0 || state.selected_subagent().is_some() {
        return live;
    }

    if transcript.selection.is_active() {
        return render_selected_history_projection(
            state,
            width,
            height,
            transcript,
            live,
            display_start,
            measured_tail,
            content_changed,
            content_mark,
        );
    }

    let capacity = height as usize;
    let target_rows = capacity
        .saturating_add(transcript.scroll_rows())
        .saturating_add(1);
    let mut blocks = Vec::new();
    let mut rows = live.len();
    let mut index = state.history().len();
    while index > state.history_display_start() && rows < target_rows {
        let last = index - 1;
        let (block, next_index) = if state.history()[last].role == "tool" {
            let mut first = last;
            while first > state.history_display_start() && state.history()[first - 1].role == "tool"
            {
                first -= 1;
            }
            let indices = (first..index).collect::<Vec<_>>();
            let mut block =
                render_committed_tool_result_group_snapshot(state, &indices, width, false);
            if !block.is_empty() {
                block.push(Line::from(""));
            }
            (Arc::new(block), first)
        } else {
            (transcript.committed_block(state, last, width), last)
        };
        rows += block.len();
        blocks.push(block);
        index = next_index;
    }
    // The welcome cell is the first item in the projected transcript, and the
    // full-height viewport must include it so a notice or turn cannot make it
    // disappear.
    if index == state.history_display_start() && rows < target_rows && !welcome_is_live(state) {
        let banner = build_claude_startup_banner_snapshot(state, width as usize, height as usize);
        rows += banner.len();
        blocks.push(Arc::new(banner));
    }
    let max_scroll = rows.saturating_sub(capacity);
    let scroll = transcript.clamp_scroll_rows(max_scroll);
    if scroll > 0 {
        let tail_start = committed_tail_start(state, display_start);
        let tail_rows = measured_tail
            .filter(|(start, _)| *start == tail_start)
            .map(|(_, rows)| rows)
            .unwrap_or_else(|| committed_suffix_rows(state, width, transcript, tail_start));
        transcript.reading_anchor = Some(super::history_cell::ReadingAnchor {
            width,
            height,
            display_start,
            history_revision,
            history_len,
            tail_start,
            tail_rows,
        });
    }
    // Tail visibility is a fact about the offset this frame clamped to, so it
    // is recomputed here rather than tracked; a revision change while the user
    // is reading only raises the "new activity" flag, it never moves the
    // viewport (#1595).
    transcript.note_projection(scroll == 0, content_changed, content_mark);
    let end = rows.saturating_sub(scroll);
    let start = end.saturating_sub(capacity);
    let mut lines = Vec::with_capacity(capacity);
    let mut offset = 0;
    for block in blocks.into_iter().rev() {
        let block_end = offset + block.len();
        let from = start.saturating_sub(offset).min(block.len());
        let through = end.saturating_sub(offset).min(block.len());
        if from < through {
            lines.extend_from_slice(&block[from..through]);
        }
        offset = block_end;
        if offset >= end {
            return lines;
        }
    }
    let from = start.saturating_sub(offset).min(live.len());
    let through = end.saturating_sub(offset).min(live.len());
    if from < through {
        lines.extend_from_slice(&live[from..through]);
    }
    lines
}

/// Reuse the immutable history projection pinned at selection start. As an
/// edge drag or wheel gesture moves into older content, render only the newly
/// exposed blocks; prefix row counts locate the viewport without rescanning or
/// rebuilding the already selected history span on each frame.
fn render_selected_history_projection(
    state: &RenderSnapshot,
    width: u16,
    height: u16,
    transcript: &mut TranscriptState,
    live: Vec<Line<'static>>,
    display_start: usize,
    measured_tail: Option<(usize, usize)>,
    content_changed: bool,
    content_mark: (u64, usize),
) -> Vec<Line<'static>> {
    let history = state.history();
    let capacity = height as usize;
    let target_rows = capacity
        .saturating_add(transcript.scroll_rows())
        .saturating_add(1);
    transcript
        .selection
        .ensure_selected_projection(history.len(), display_start, width, height);

    loop {
        let Some((cached_rows, next_index, projection_start, finished)) = transcript
            .selection
            .selected_projection()
            .map(|projection| {
                (
                    projection.total_rows(),
                    projection.next_index(),
                    projection.display_start(),
                    projection.finished(),
                )
            })
        else {
            return Vec::new();
        };
        let reached_oldest_history = next_index <= projection_start;
        let needs_welcome = reached_oldest_history && !welcome_is_live(state);
        if (live.len().saturating_add(cached_rows) >= target_rows && !needs_welcome) || finished {
            break;
        }

        let next = if next_index > projection_start {
            let last = next_index - 1;
            if history[last].role == "tool" {
                let mut first = last;
                while first > projection_start && history[first - 1].role == "tool" {
                    first -= 1;
                }
                let indices = (first..next_index).collect::<Vec<_>>();
                let mut block =
                    render_committed_tool_result_group_snapshot(state, &indices, width, false);
                if !block.is_empty() {
                    block.push(Line::from(""));
                }
                Some((Arc::new(block), first, false))
            } else {
                Some((
                    Arc::clone(&transcript.committed_block(state, last, width)),
                    last,
                    false,
                ))
            }
        } else if !welcome_is_live(state) {
            let banner =
                build_claude_startup_banner_snapshot(state, width as usize, height as usize);
            Some((Arc::new(banner), projection_start, true))
        } else {
            None
        };

        if let Some((block, next_index, finish_projection)) = next {
            transcript
                .selection
                .selected_projection_mut()
                .expect("selection projection initialized")
                .append(block, next_index);
            if finish_projection {
                transcript
                    .selection
                    .selected_projection_mut()
                    .expect("selection projection initialized")
                    .finish();
            }
        } else {
            transcript
                .selection
                .selected_projection_mut()
                .expect("selection projection initialized")
                .finish();
        }
    }

    let history_rows = transcript
        .selection
        .selected_projection()
        .map_or(0, |projection| projection.total_rows());
    let total_rows = history_rows.saturating_add(live.len());
    let max_scroll = total_rows.saturating_sub(capacity);
    let scroll = transcript.clamp_scroll_rows(max_scroll);
    if scroll > 0 {
        let tail_start = committed_tail_start(state, display_start);
        let tail_rows = measured_tail
            .filter(|(start, _)| *start == tail_start)
            .map(|(_, rows)| rows)
            .unwrap_or_else(|| committed_suffix_rows(state, width, transcript, tail_start));
        transcript.reading_anchor = Some(super::history_cell::ReadingAnchor {
            width,
            height,
            display_start,
            history_revision: state.history().revision(),
            history_len: history.len(),
            tail_start,
            tail_rows,
        });
    }

    // Keep the "new activity" affordance tied to the offset this frame
    // clamped to, exactly as the uncached projection path does.
    transcript.note_projection(scroll == 0, content_changed, content_mark);
    let end = total_rows.saturating_sub(scroll);
    let start = end.saturating_sub(capacity);
    let history_end = end.min(history_rows);
    let history_start = start.min(history_end);
    let mut lines = if history_start < history_end {
        let from_tail = history_rows - history_end;
        let through_tail = history_rows - history_start;
        transcript
            .selection
            .selected_projection()
            .map_or_else(Vec::new, |projection| {
                projection.rows_from_tail_range(from_tail, through_tail)
            })
    } else {
        Vec::new()
    };
    let live_start = start.saturating_sub(history_rows);
    let live_end = end.saturating_sub(history_rows).min(live.len());
    if live_start < live_end {
        lines.extend_from_slice(&live[live_start..live_end]);
    }
    lines
}

fn committed_tail_start(state: &RenderSnapshot, display_start: usize) -> usize {
    let history = state.history();
    let mut start = history.len();
    if start > display_start {
        start -= 1;
        if history[start].role == "tool" {
            while start > display_start && history[start - 1].role == "tool" {
                start -= 1;
            }
        }
    }
    start
}

fn committed_suffix_rows(
    state: &RenderSnapshot,
    width: u16,
    transcript: &mut TranscriptState,
    start: usize,
) -> usize {
    let history = state.history();
    let mut rows = 0;
    let mut index = start;
    while index < history.len() {
        if history[index].role == "tool" {
            let first = index;
            while index < history.len() && history[index].role == "tool" {
                index += 1;
            }
            let indices = (first..index).collect::<Vec<_>>();
            let block = render_committed_tool_result_group_snapshot(state, &indices, width, false);
            rows += block.len() + usize::from(!block.is_empty());
        } else {
            rows += transcript.committed_block(state, index, width).len();
            index += 1;
        }
    }
    rows
}

pub(super) fn render_selected_subagent_context(
    state: &RenderSnapshot,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let Some(agent) = state.selected_subagent() else {
        return Vec::new();
    };
    let status = match agent.status() {
        rustcode::controller::SubAgentStatus::Running => "running",
        rustcode::controller::SubAgentStatus::Completed => "completed",
        rustcode::controller::SubAgentStatus::Failed => "failed",
        rustcode::controller::SubAgentStatus::Cancelled => "cancelled",
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

    if message.conversation_recap {
        return render_conversation_recap(&message.content, width);
    }

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
        "system" => {
            if let Some(content) = system_notice_for_display(&message.content) {
                render_status_panel(content, width, show_picker, &mut lines);
                lines.push(Line::from(""));
            }
        }
        _ => {}
    }

    lines.into_iter().map(|line| own_line(&line)).collect()
}

fn render_conversation_recap(content: &str, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let style = get_themed_style(
        COLOR_MUTED(),
        COLOR_BG(),
        Modifier::ITALIC | Modifier::DIM,
        false,
    );
    if content == "Generating conversation recap…" {
        return wrap_recap_spans(
            vec![Span::styled(content.to_owned(), style)],
            width as usize,
            None,
        );
    }
    let generated = serde_json::from_str::<serde_json::Value>(content).ok();
    let summary = generated
        .as_ref()
        .and_then(|value| value["summary"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| rustcode::controller::sanitize_recap_content(content));
    let next = generated
        .as_ref()
        .and_then(|value| value["next_action"].as_str());
    let wrap_width = width.saturating_sub(2).max(1) as usize;
    let prefix = "  ↳ Recap: ";
    let indent = if wrap_width > prefix.width() {
        prefix.width()
    } else {
        0
    };
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    if indent == 0 {
        lines.extend(wrap_recap_spans(
            vec![Span::styled("↳ Recap:", style.add_modifier(Modifier::BOLD))],
            wrap_width,
            None,
        ));
    } else {
        spans.push(Span::styled("  ↳ ", style));
        spans.push(Span::styled("Recap: ", style.add_modifier(Modifier::BOLD)));
    }
    spans.push(Span::styled(summary, style));
    let continuation = (indent > 0).then(|| Span::styled(" ".repeat(indent), style));
    lines.extend(wrap_recap_spans(spans, wrap_width, continuation.clone()));
    if let Some(next) = next {
        lines.extend(wrap_recap_spans(
            vec![
                Span::styled(" ".repeat(indent), style),
                Span::styled("Next: ", style.add_modifier(Modifier::BOLD)),
                Span::styled(next.to_owned(), style),
            ],
            wrap_width,
            continuation,
        ));
    }
    lines
}

fn wrap_recap_spans(
    spans: Vec<Span<'static>>,
    width: usize,
    continuation: Option<Span<'static>>,
) -> Vec<Line<'static>> {
    use unicode_segmentation::UnicodeSegmentation;
    let indent_width = continuation
        .as_ref()
        .map_or(0, Span::width)
        .min(width.saturating_sub(1));
    let mut lines = Vec::new();
    let mut row = Vec::new();
    let mut used = 0;
    for span in spans {
        for word in span.content.split_inclusive(char::is_whitespace) {
            let word_width = word.width();
            if used > indent_width
                && used + word_width > width
                && word_width <= width - indent_width
            {
                lines.push(Line::from(std::mem::take(&mut row)));
                if let Some(indent) = &continuation {
                    row.push(indent.clone());
                }
                used = indent_width;
            }
            for grapheme in word.graphemes(true) {
                let columns = grapheme.width();
                if used > indent_width && used + columns > width {
                    lines.push(Line::from(std::mem::take(&mut row)));
                    if let Some(indent) = &continuation {
                        row.push(indent.clone());
                    }
                    used = indent_width;
                }
                row.push(Span::styled(grapheme.to_owned(), span.style));
                used += columns;
            }
        }
    }
    if !row.is_empty() {
        lines.push(Line::from(row));
    }
    lines
}

#[cfg(test)]
pub(crate) fn render_committed_tool_result_group(
    view: &RenderState,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(view);
    render_committed_tool_result_group_snapshot(&snapshot, message_indices, width, show_picker)
}

#[cfg(test)]
pub(crate) fn render_work_separator_before_assistant(
    view: &RenderState,
    assistant_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(view);
    render_work_separator_before_assistant_snapshot(&snapshot, assistant_index, width)
}

#[cfg(test)]
pub(crate) fn build_claude_startup_banner(
    view: &RenderState,
    total_width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(view);
    build_claude_startup_banner_snapshot(&snapshot, total_width, max_height)
}

#[cfg(test)]
pub(crate) fn render_committed_history_block(
    view: &RenderState,
    message_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(view);
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

#[cfg(test)]
pub(super) fn render_committed_assistant_text_snapshot(
    _state: &RenderSnapshot,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    render_committed_assistant_text_with_metrics(content, width, None, None, None, None)
}

#[cfg(test)]
pub(crate) fn render_committed_assistant_chunk(
    view: &RenderState,
    content: &str,
    width: u16,
    is_continuation: bool,
) -> Vec<Line<'static>> {
    render_committed_assistant_chunk_snapshot(
        &RenderSnapshot::new(view),
        content,
        width,
        is_continuation,
    )
}

#[cfg(test)]
pub(crate) fn render_committed_assistant_text(
    view: &RenderState,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    render_committed_assistant_text_snapshot(&RenderSnapshot::new(view), content, width)
}

#[cfg(test)]
pub(super) fn render_committed_assistant_text_with_metrics(
    content: &str,
    width: u16,
    token_usage: Option<rustcode::controller::TokenUsage>,
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

#[cfg(test)]
mod projection_tests {
    use super::*;
    use crate::ui::render_snapshot::set_current_response;
    use rustcode::controller::ChatMessage;

    #[test]
    fn visible_slice_matches_full_projection_across_blocks_welcome_and_live_tail() {
        // Both projections are rendered with the ambient theme, so a test that
        // changes `ACTIVE_THEME` mid-run would make the two renders disagree.
        let _theme_guard = crate::ui::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let mut state = RenderState::new();
        state
            .history
            .push(ChatMessage::new("user", "first request"));
        state.history.push(ChatMessage::new(
            "assistant",
            (0..30)
                .map(|row| format!("first answer row {row:02}"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        ));
        state
            .history
            .push(ChatMessage::new("user", "second request"));
        state
            .history
            .push(ChatMessage::new("assistant", "last answer"));
        set_current_response(&mut state, "live first line\nlive second line");
        let snapshot = render_snapshot(&state);
        let width = 42;
        let height = 14;

        for requested_scroll in [0, 1, 8, 25, 60, 200] {
            let mut transcript = TranscriptState::default();
            transcript.scroll_up(requested_scroll);
            let actual = render_visible_conversation_with_transcript(
                &snapshot,
                width,
                height,
                &mut transcript,
            );

            let mut reference_transcript = TranscriptState::default();
            reference_transcript.scroll_up(requested_scroll);
            let live_height = if requested_scroll > 0 { 0 } else { height };
            let live = render_live_tail_mode(
                &snapshot,
                width,
                live_height,
                &mut reference_transcript,
                true,
            );
            let mut full =
                build_claude_startup_banner_snapshot(&snapshot, width as usize, height as usize);
            for index in 0..snapshot.history().len() {
                full.extend(render_committed_history_block_snapshot(
                    &snapshot, index, width,
                ));
            }
            full.extend(live);
            let max_scroll = full.len().saturating_sub(height as usize);
            let scroll = requested_scroll.min(max_scroll);
            let end = full.len() - scroll;
            let start = end.saturating_sub(height as usize);
            let expected = full[start..end].to_vec();

            assert_eq!(actual, expected, "scroll={requested_scroll}");
        }
    }
}
