use super::*;
#[cfg(test)]
use crate::ui::render_snapshot::render_snapshot;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[cfg(test)]
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
#[cfg(test)]
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
    // stale thought a second time below the tool. The live call may not be
    // visible yet (an argument-less MCP call has no target until it starts),
    // and the inline tail holds only the unflushed part of the stream, so the
    // check compares the whole response and does not wait for a tool (#1770).
    if !model_live_text.is_empty()
        && state.history().last().is_some_and(|message| {
            let committed = message.content.trim();
            message.role == "assistant"
                && (committed == model_live_text.trim()
                    || committed == state.current_response().trim())
        })
    {
        model_live_text = "";
    }

    if visible_live_tool_calls.is_empty() {
        transcript.clear_tools();
    } else {
        transcript.set_tools_with_verbosity(
            &visible_live_tool_calls,
            state.verbosity(),
            state.home_path(),
        );
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

    let background_lines =
        super::composer_render::background_command_lines_with_width(state, width);
    if !background_lines.is_empty() && !full_viewport {
        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::from(""));
        }
        lines.extend(background_lines);
        lines.push(Line::from(""));
    }

    if state.recap_loading() {
        lines.extend(render_conversation_recap(
            "Generating conversation recap…",
            width,
        ));
    }

    // Full viewport callers slice the combined committed + live projection.
    // Clipping the live cell here would make its older rows unreachable.
    if !full_viewport && height > 0 && lines.len() > height as usize {
        let visible_start = lines.len() - height as usize;
        lines = lines.split_off(visible_start);
    }

    lines.into_iter().map(|line| own_line(&line)).collect()
}

/// Plain running indicator (spinner + model) painted in the reserved row at
/// the bottom of the chat, or `None` when there is nothing to report.
///
/// Deliberately plain — status words, elapsed clocks and token rates belong in
/// the transcript. A turn waiting on the provider or a tool still shows that
/// work is happening instead of an empty chat.
pub(super) fn live_running_indicator(state: &RenderSnapshot, width: u16) -> Option<Line<'static>> {
    if matches!(
        state.status(),
        AppStatus::AwaitingToolConfirmation | AppStatus::AwaitingQuestion
    ) {
        return None;
    }
    // Output only (thinking + answer) for this turn. Prompt tokens are
    // re-sent with every request, so adding them made the figure track
    // the whole conversation instead of the work done this turn.
    let usage_tokens = |usage: Option<&rustcode::controller::TokenUsage>| {
        usage.map_or(0, |usage| u64::from(usage.completion_tokens))
    };
    let completed = usage_tokens(state.current_turn_token_usage());
    let completed_continuations = usage_tokens(state.current_round_token_usage())
        .saturating_add(u64::from(state.current_round_estimated_output_tokens()));
    let active_request = if state.provider_request_in_flight() {
        if let Some(usage) = state.current_token_usage() {
            usage_tokens(Some(usage))
        } else {
            state
                .stream_tracker()
                .map(|tracker| u64::from(tracker.snapshot().1))
                .unwrap_or_default()
        }
    } else {
        0
    };
    let tokens = completed
        .saturating_add(completed_continuations)
        .saturating_add(active_request);
    let provisional = state.token_usage_in_flight()
        || state.current_turn_token_usage_is_estimated()
        || state.current_round_estimated_output_tokens() > 0;
    let token_suffix = (provisional || state.current_turn_token_usage().is_some()).then(|| {
        Span::styled(
            format!(
                " · ↓ {}{} tokens",
                if provisional { "~" } else { "" },
                super::composer_render::format_token_count(tokens.min(u64::from(u32::MAX)) as u32)
            ),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        )
    });
    // Foreground/queued work must not swallow the cumulative turn total: the
    // transcript cell carries identity and elapsed time, while this row keeps
    // the token accounting. Both are composed into the single reserved row so
    // head, detail and total can never overflow together (#1725).
    if let Some(indicator) =
        super::composer_render::active_work_indicator(state, width, token_suffix.clone())
    {
        return Some(indicator);
    }
    let activity = rustcode::controller::classify_live_tools(&state.live_tool_calls()).unwrap_or(
        rustcode::controller::classify_activity(&state.status(), &state.running_tools()),
    );
    match activity.kind {
        // Nothing is running, and waiting on the user is not "running": the
        // approval/question panel is its own signal, so no indicator is added.
        rustcode::controller::ActivityKind::Ready
        | rustcode::controller::ActivityKind::ActionRequired => None,
        _ => {
            let spans = vec![
                Span::styled(
                    format!("{} ", super::composer_render::running_spinner_char(state)),
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, false),
                ),
                Span::styled(
                    format!("Generating · {}", state.model_name()),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
                ),
            ];
            // Long model names truncate; token accounting is reserved first.
            Some(Line::from(super::composer_render::fit_indicator_row(
                spans,
                token_suffix,
                usize::from(width),
            )))
        }
    }
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
    let mut live_content = std::collections::hash_map::DefaultHasher::new();
    state.current_response().len().hash(&mut live_content);
    state.recap_loading().hash(&mut live_content);
    for call in state
        .live_tool_calls()
        .iter()
        .filter(|call| is_live_tool_call_visible(call))
    {
        call.key.hash(&mut live_content);
        call.execution_started.hash(&mut live_content);
        call.omitted_output_bytes.hash(&mut live_content);
        for chunk in &call.output {
            chunk.stderr.hash(&mut live_content);
            chunk.text.hash(&mut live_content);
        }
    }
    let content_mark = (history_revision, live_content.finish() as usize);
    if let Some(agent) = state.selected_subagent() {
        // An agent context has no reading anchor of its own, so it scrolls as
        // a plain offset from the newest row of the agent's whole history.
        let content_mark = (
            agent.history().len() as u64,
            usize::from(agent.active_turn()),
        );
        let content_changed = transcript.last_content() != Some(content_mark);
        let capacity = height as usize;
        let wanted_rows = capacity
            .saturating_add(transcript.scroll_rows())
            .saturating_add(1);
        let lines = selected_subagent_lines(state, width, wanted_rows);
        let scroll = transcript.clamp_scroll_rows(lines.len().saturating_sub(capacity));
        transcript.note_projection(scroll == 0, content_changed, content_mark);
        let end = lines.len() - scroll;
        return lines[end.saturating_sub(capacity)..end].to_vec();
    }
    let content_changed = transcript.last_content() != Some(content_mark);
    let display_start = state.history_display_start().min(history_len);
    if height == 0 {
        return Vec::new();
    }
    let live = render_live_tail_mode(state, width, height, transcript, true);
    // The welcome banner moves to the committed prefix after the first turn;
    // it is not mutable tail growth and must not be subtracted on that handoff.
    let live_rows = if welcome_is_live(state) {
        0
    } else {
        live.len()
    };
    let mut measured_tail = None;
    if transcript.scroll_rows() > 0
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
        let added_rows = tail_rows.saturating_add(live_rows) as isize
            - anchor.tail_rows.saturating_add(anchor.live_rows) as isize;
        let height_change = anchor.height as isize - height as isize;
        transcript.shift_reading_offset(added_rows.saturating_add(height_change));
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
            let first = tool_chain_start(state, last, state.history_display_start());
            (
                transcript.committed_tool_group(state, first, index, width),
                first,
            )
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
            live_rows: if welcome_is_live(state) {
                0
            } else {
                live.len()
            },
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
                let first = tool_chain_start(state, last, projection_start);
                let indices = tool_chain_indices(state, first, next_index);
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
            live_rows: if welcome_is_live(state) {
                0
            } else {
                live.len()
            },
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
            start = tool_chain_start(state, start, display_start);
        }
    }
    start
}

/// An assistant message that shows nothing in the transcript: it only carries
/// the tool calls of a one-tool-per-round step. Thoughts and prose are visible,
/// so they end a chain.
fn is_invisible_tool_step(state: &RenderSnapshot, index: usize) -> bool {
    let Some(message) = state.history().get(index) else {
        return false;
    };
    message.role == "assistant"
        && !message.conversation_recap
        && (!message.tool_calls.is_empty()
            || !rustcode_tool_protocol::resolve_tool_calls(message, state.active_tool_protocol())
                .is_empty())
        // The renderer is the authority on what shows. Whether a block is
        // empty does not depend on the width, and an empty message skips it.
        && (message.content.trim().is_empty()
            || render_committed_history_block_snapshot(state, index, 80).is_empty())
}

/// First index of the tool-result chain ending at `last`, never below `floor`.
///
/// Rounds with nothing visible between them belong to one group, so the chain
/// crosses invisible tool steps and the group renders under a single heading
/// with one closed tree, however many rounds produced it.
fn tool_chain_start(state: &RenderSnapshot, last: usize, floor: usize) -> usize {
    let history = state.history();
    let mut first = last;
    loop {
        while first > floor && history[first - 1].role == "tool" {
            first -= 1;
        }
        if first >= floor + 2
            && is_invisible_tool_step(state, first - 1)
            && history[first - 2].role == "tool"
        {
            first -= 2;
        } else {
            return first;
        }
    }
}

/// Exclusive end of the tool-result chain starting at `first`.
fn tool_chain_end(state: &RenderSnapshot, first: usize) -> usize {
    let history = state.history();
    let mut end = first;
    loop {
        while end < history.len() && history[end].role == "tool" {
            end += 1;
        }
        if end + 1 < history.len()
            && is_invisible_tool_step(state, end)
            && history[end + 1].role == "tool"
        {
            end += 1;
        } else {
            return end;
        }
    }
}

/// The tool results inside a chain, leaving out the invisible steps between.
fn tool_chain_indices(state: &RenderSnapshot, first: usize, end: usize) -> Vec<usize> {
    let history = state.history();
    (first..end)
        .filter(|&index| history[index].role == "tool")
        .collect()
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
            index = tool_chain_end(state, first);
            rows += transcript
                .committed_tool_group(state, first, index, width)
                .len();
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
    let mut lines = selected_subagent_lines(state, width, height as usize);
    if lines.len() > height as usize {
        lines = lines.split_off(lines.len() - height as usize);
    }
    lines
}

/// Project the selected agent's context newest-first until `wanted_rows` are
/// covered, so a long agent transcript costs only the rows the viewport and
/// its scroll offset can reach. The header is the first row of the context
/// and appears once the projection reaches the top.
fn selected_subagent_lines(
    state: &RenderSnapshot,
    width: u16,
    wanted_rows: usize,
) -> Vec<Line<'static>> {
    let Some(agent) = state.selected_subagent() else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    let mut rows = usize::from(agent.active_turn());
    let mut index = state.active_history().len();
    while index > 0 && rows < wanted_rows {
        index -= 1;
        let block = render_committed_history_block_snapshot(state, index, width);
        rows += block.len();
        blocks.push(block);
    }

    let mut lines = Vec::new();
    if index == 0 {
        let status = match agent.status() {
            rustcode::controller::SubAgentStatus::Queued => "Queued",
            rustcode::controller::SubAgentStatus::Interrupted => "Interrupted",
            rustcode::controller::SubAgentStatus::Running => "Running",
            rustcode::controller::SubAgentStatus::Completed => "Completed",
            rustcode::controller::SubAgentStatus::Failed => "Failed",
            rustcode::controller::SubAgentStatus::Cancelled => "Cancelled",
        };
        lines.push(Line::from(vec![Span::styled(
            format!("• {status} {}", agent.name()),
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, false),
        )]));
    }
    lines.extend(blocks.into_iter().rev().flatten());
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
            let mut response = history_cell::AssistantMarkdownCell::committed(
                &message.content,
                message.token_usage.clone(),
                message.response_time_ms,
                message.thought_time_ms,
                message.thought_tokens,
            )
            .display_lines(width);
            let completion = render_turn_completion(message, width);
            if !completion.is_empty() {
                response.extend(completion);
                response.push(Line::from(""));
            }
            return response;
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

fn render_turn_completion(
    message: &rustcode::controller::ChatMessage,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(completed_at) = message
        .completed_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    else {
        return Vec::new();
    };
    if width == 0 {
        return Vec::new();
    }
    use chrono::Datelike;
    let completed_at = completed_at.with_timezone(&chrono::Local);
    let today = chrono::Local::now().date_naive();
    let date_format = if completed_at.date_naive() == today {
        ""
    } else if completed_at.year() == today.year() {
        "%b %-d at "
    } else {
        "%b %-d, %Y at "
    };
    let mut labels = Vec::new();
    if let Some(duration) = message.response_time_ms {
        let seconds = duration / 1_000;
        let elapsed = if seconds >= 3600 {
            format!(
                "{}h {}m {}s",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            )
        } else if seconds >= 60 {
            format!("{}m {}s", seconds / 60, seconds % 60)
        } else if seconds == 0 {
            "<1s".to_string()
        } else {
            format!("{seconds}s")
        };
        labels.push(format!("Worked for {elapsed}"));
    }
    labels.push(format!(
        "{}{}",
        completed_at.format(date_format),
        completed_at.format("%H:%M")
    ));
    let style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, false);
    let indent = if width > 2 { "  " } else { "" };
    if width < 10 {
        let label = labels.join(" • ");
        return label
            .chars()
            .collect::<Vec<_>>()
            .chunks(width as usize)
            .map(|chunk| Line::from(Span::styled(chunk.iter().collect::<String>(), style)))
            .collect();
    }
    let mut lines = Vec::new();
    push_wrapped_with_continuation(
        &mut lines,
        vec![Span::styled(
            format!("{indent}{}", labels.join(" • ")),
            style,
        )],
        width as usize,
        Some(Span::styled(indent, style)),
    );
    lines
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

    fn streaming_state() -> RenderState {
        let mut state = RenderState::new();
        state
            .history
            .push(ChatMessage::new("user", "older request"));
        set_current_response(&mut state, &streamed_rows(0..30));
        state
    }

    fn streamed_rows(rows: std::ops::Range<usize>) -> String {
        rows.map(|row| format!("live row {row:02} 界 é"))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn visible_text(
        state: &RenderState,
        transcript: &mut TranscriptState,
        width: u16,
        height: u16,
    ) -> Vec<String> {
        render_visible_conversation_with_transcript(
            &render_snapshot(state),
            width,
            height,
            transcript,
        )
        .iter()
        .map(ToString::to_string)
        .collect()
    }

    #[test]
    fn scrolling_can_inspect_the_middle_of_a_long_live_response() {
        let _guard = crate::ui::tests::THEME_TEST_LOCK.lock().unwrap();
        let state = streaming_state();
        let mut transcript = TranscriptState::default();
        let bottom = visible_text(&state, &mut transcript, 40, 8).join("\n");
        assert!(bottom.contains("live row 29"), "{bottom}");
        transcript.scroll_up(12);
        let middle = visible_text(&state, &mut transcript, 40, 8).join("\n");
        assert!(middle.contains("live row 23"), "{middle}");
        assert!(!middle.contains("live row 29"), "{middle}");
        assert!(!transcript.is_following());
        transcript.scroll_up(1000);
        let oldest = visible_text(&state, &mut transcript, 40, 8).join("\n");
        assert!(!oldest.contains("live row 29"), "{oldest}");
        transcript.scroll_down(transcript.scroll_rows().saturating_sub(12));
        assert_eq!(
            visible_text(&state, &mut transcript, 40, 8).join("\n"),
            middle
        );
        transcript.jump_to_latest();
        assert!(
            visible_text(&state, &mut transcript, 40, 8)
                .join("\n")
                .contains("live row 29")
        );
        assert!(transcript.is_following());
    }

    #[test]
    fn live_growth_holds_manual_reading_rows_and_jump_resumes_following() {
        let _guard = crate::ui::tests::THEME_TEST_LOCK.lock().unwrap();
        let mut state = streaming_state();
        let mut transcript = TranscriptState::default();
        let _ = visible_text(&state, &mut transcript, 40, 8);
        transcript.scroll_up(12);
        let before = visible_text(&state, &mut transcript, 40, 8);
        assert!(before.join("\n").contains("live row 23"));
        set_current_response(&mut state, &streamed_rows(0..45));
        assert_eq!(visible_text(&state, &mut transcript, 40, 8), before);
        assert!(transcript.unseen_activity());
        transcript.jump_to_latest();
        assert!(
            visible_text(&state, &mut transcript, 40, 8)
                .join("\n")
                .contains("live row 44")
        );
        set_current_response(&mut state, &streamed_rows(0..50));
        assert!(
            visible_text(&state, &mut transcript, 40, 8)
                .join("\n")
                .contains("live row 49")
        );
        assert!(transcript.is_following());
        assert!(!transcript.unseen_activity());
    }

    #[test]
    fn live_reading_survives_completion_cancellation_and_height_resize() {
        let _guard = crate::ui::tests::THEME_TEST_LOCK.lock().unwrap();
        for cancelled in [false, true] {
            let mut state = streaming_state();
            let mut transcript = TranscriptState::default();
            let _ = visible_text(&state, &mut transcript, 16, 8);
            transcript.scroll_up(20);
            let before = visible_text(&state, &mut transcript, 16, 8);
            assert!(before.join("\n").contains("live row"));
            let shorter = visible_text(&state, &mut transcript, 16, 6);
            assert_eq!(shorter, before[..6]);
            state.history.push(ChatMessage::new(
                "assistant",
                state.current_response.to_string(),
            ));
            set_current_response(&mut state, "");
            state.status = AppStatus::Idle;
            if cancelled {
                state
                    .history
                    .push(ChatMessage::new("system", "Turn interrupted by user"));
            }
            assert_eq!(visible_text(&state, &mut transcript, 16, 6), shorter);
            transcript.jump_to_latest();
            assert!(
                visible_text(&state, &mut transcript, 16, 8)
                    .join("\n")
                    .contains("live row 29")
            );
        }
    }

    #[test]
    fn tool_output_announces_unseen_activity_without_timer_only_updates() {
        let _guard = crate::ui::tests::THEME_TEST_LOCK.lock().unwrap();
        let mut state = streaming_state();
        let mut call = rustcode::controller::LiveToolCall::new(
            "call-1",
            None,
            "run_command",
            "Bash",
            "sleep 10",
        );
        call.execution_started = true;
        std::sync::Arc::make_mut(&mut state.live_tool_calls).push(call);
        let mut transcript = TranscriptState::default();
        let _ = visible_text(&state, &mut transcript, 40, 8);
        transcript.scroll_up(12);
        let before = visible_text(&state, &mut transcript, 40, 8);
        assert!(!transcript.unseen_activity());
        let _ = visible_text(&state, &mut transcript, 40, 8);
        assert!(!transcript.unseen_activity());
        std::sync::Arc::make_mut(&mut state.live_tool_calls)[0]
            .output
            .push_back(rustcode::controller::LiveToolOutputChunk {
                stderr: false,
                text: "new output".to_owned(),
            });
        assert_eq!(visible_text(&state, &mut transcript, 40, 8), before);
        assert!(transcript.unseen_activity());
        transcript.jump_to_latest();
        let _ = visible_text(&state, &mut transcript, 40, 8);
        assert!(!transcript.unseen_activity());
    }

    #[test]
    fn growing_foreground_output_stays_reachable_with_a_bounded_preview() {
        let _guard = crate::ui::tests::THEME_TEST_LOCK.lock().unwrap();
        for width in [24, 80] {
            let mut state = RenderState::new();
            state.verbosity = rustcode::controller::Verbosity::Low;
            state.status = AppStatus::Streaming;
            state
                .history
                .push(ChatMessage::new("assistant", streamed_rows(0..30)));
            let mut call = rustcode::controller::LiveToolCall::new(
                "foreground-1",
                None,
                "run_command",
                "Bash",
                "echo rows",
            );
            call.execution_started = true;
            call.output
                .push_back(rustcode::controller::LiveToolOutputChunk {
                    stderr: false,
                    text: "command row 00 界\ncommand row 01 界".to_owned(),
                });
            std::sync::Arc::make_mut(&mut state.live_tool_calls).push(call);
            let mut transcript = TranscriptState::default();
            let initial = visible_text(&state, &mut transcript, width, 8).join("\n");
            assert!(
                initial.contains("command row 01 界"),
                "width={width}: {initial}"
            );

            transcript.scroll_up(12);
            let reading = visible_text(&state, &mut transcript, width, 8);
            assert!(reading.join("\n").contains("live row"), "{reading:?}");
            assert!(!transcript.is_following());
            std::sync::Arc::make_mut(&mut state.live_tool_calls)[0]
                .output
                .push_back(rustcode::controller::LiveToolOutputChunk {
                    stderr: false,
                    text: (2..10)
                        .map(|row| format!("command row {row:02} 界"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                });
            assert_eq!(
                visible_text(&state, &mut transcript, width, 8),
                reading,
                "foreground body growth must preserve history reading at width={width}"
            );
            assert!(transcript.unseen_activity());

            transcript.scroll_down(transcript.scroll_rows().saturating_sub(1));
            let near_live = visible_text(&state, &mut transcript, width, 8).join("\n");
            assert!(
                near_live.contains("command row 08 界"),
                "width={width}: {near_live}"
            );
            assert!(!transcript.is_following());
            transcript.jump_to_latest();
            let latest = visible_text(&state, &mut transcript, width, 8);
            let latest_text = latest.join("\n");
            assert!(
                latest_text.contains("command row 09 界"),
                "width={width}: {latest_text}"
            );
            assert!(latest_text.contains("… +6 lines"), "{latest_text}");
            assert_eq!(
                latest
                    .iter()
                    .filter(|row| row.contains("command row"))
                    .count(),
                4,
                "the five-row body keeps two head rows, the omission marker, and two tail rows"
            );
            assert!(!latest_text.contains("command row 02"), "{latest_text}");
            assert!(
                latest.iter().all(|row| row.width() <= usize::from(width)),
                "{latest:?}"
            );
            assert!(transcript.is_following());
            assert!(!transcript.unseen_activity());
        }
    }

    #[test]
    fn width_resize_rewraps_unicode_live_rows_and_preserves_subsequent_reading() {
        let _guard = crate::ui::tests::THEME_TEST_LOCK.lock().unwrap();
        let mut state = streaming_state();
        let mut transcript = TranscriptState::default();
        let _ = visible_text(&state, &mut transcript, 40, 8);
        transcript.scroll_up(20);
        let _ = visible_text(&state, &mut transcript, 40, 8);
        let narrow = visible_text(&state, &mut transcript, 12, 8);
        assert_eq!(narrow.len(), 8);
        assert!(narrow.iter().all(|line| line.width() <= 12), "{narrow:?}");
        assert!(narrow.join("\n").contains("界"), "{narrow:?}");
        assert_eq!(visible_text(&state, &mut transcript, 12, 8), narrow);
        set_current_response(&mut state, &streamed_rows(0..45));
        assert_eq!(visible_text(&state, &mut transcript, 12, 8), narrow);
        transcript.jump_to_latest();
        let bottom = visible_text(&state, &mut transcript, 12, 8).join("\n");
        assert!(bottom.contains("44"), "{bottom}");
    }

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
            let live_height = u16::MAX;
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

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[test]
    fn completion_metadata_is_persisted_and_renders_after_prose() {
        let _guard = crate::ui::tests::THEME_TEST_LOCK.lock().unwrap();
        let mut state = RenderState::new();
        let mut message = rustcode::controller::ChatMessage::new("assistant", "Task finished.");
        message.completed_at = Some("2000-09-06T14:32:00+02:00".to_string());
        message.response_time_ms = Some(125_999);
        let json = serde_json::to_string(&message).unwrap();
        let restored = serde_json::from_str(&json).unwrap();
        state.history.push(restored);
        let lines = render_committed_history_block_snapshot(&render_snapshot(&state), 0, 100);
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.find("Task finished.").unwrap() < text.find("Worked for 2m 5s").unwrap(),
            "{text}"
        );
        assert!(text.contains("Sep 6, 2000 at"), "{text}");
        assert_eq!(text.matches("Worked for").count(), 1);
        assert!(lines.last().unwrap().spans.is_empty());
        state
            .history
            .push(rustcode::controller::ChatMessage::new("user", "Next task"));
        let next = render_committed_history_block_snapshot(&render_snapshot(&state), 1, 100);
        assert!(
            next[0]
                .spans
                .iter()
                .any(|span| span.style.bg == Some(COLOR_PANEL()))
        );
        assert!(next[1].to_string().contains("Next task"));
        assert!(lines[lines.len() - 2].to_string().contains("Worked for"));

        let timing = lines
            .iter()
            .find(|line| line.to_string().contains("Worked for"))
            .unwrap();
        assert!(timing.to_string().starts_with("  Worked for"));
        assert!(
            timing
                .spans
                .iter()
                .all(|span| span.style.add_modifier.contains(Modifier::DIM))
        );
    }

    #[test]
    fn completion_omits_unknown_times_and_wraps_short_durations() {
        let _guard = crate::ui::tests::THEME_TEST_LOCK.lock().unwrap();
        let mut message = rustcode::controller::ChatMessage::new("assistant", "Done");
        message.response_time_ms = Some(250);
        assert!(render_turn_completion(&message, 100).is_empty());
        message.completed_at = Some("invalid".to_string());
        assert!(render_turn_completion(&message, 100).is_empty());
        message.completed_at = Some(chrono::Local::now().to_rfc3339());
        for (milliseconds, expected) in [(250, "<1s"), (12_000, "12s"), (3_605_000, "1h 0m 5s")] {
            message.response_time_ms = Some(milliseconds);
            let text = render_turn_completion(&message, 100)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains(&format!("Worked for {expected}")), "{text}");
        }
        let narrow = render_turn_completion(&message, 15);
        assert!(narrow.len() > 1);
        assert!(narrow.iter().all(|line| line.width() <= 15));
        assert!(render_turn_completion(&message, 0).is_empty());
        for width in 1..10 {
            assert!(
                render_turn_completion(&message, width)
                    .iter()
                    .all(|line| line.width() <= width as usize)
            );
        }
    }
}
