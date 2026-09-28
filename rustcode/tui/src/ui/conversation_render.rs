use super::*;
#[cfg(test)]
use crate::ui::render_snapshot::render_snapshot;

pub(super) fn conversation_area_height(content_height: u16, available_height: u16) -> u16 {
    if available_height == 0 {
        return 0;
    }
    content_height.min(available_height)
}

/// Render only the mutable portion of the current turn. Completed history is
/// deliberately excluded: it will be committed to terminal scrollback.
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
pub(crate) fn render_live_tail(state: &AppState, width: u16, height: u16) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(&state);
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

    let visible_history_is_empty =
        state.history().is_empty() || state.history_display_start() >= state.history().len();
    if visible_history_is_empty
        && state.current_response().is_empty()
        && matches!(state.status(), AppStatus::Idle)
        && state.running_tools().is_empty()
        && state.live_tool_calls().is_empty()
        && state.background_tasks().is_empty()
    {
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
    if activity_visible {
        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::from(""));
        }
        lines.push(activity_status_line(state, false));
        lines.extend(background_command_lines(state));
        lines.push(Line::from(""));
    }

    if height > 0 && lines.len() > height as usize {
        let visible_start = lines.len() - height as usize;
        lines = lines.split_off(visible_start);
    }

    lines.into_iter().map(|line| own_line(&line)).collect()
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
    let live_height = if transcript.scroll_rows() > 0 && !state.history().is_empty() {
        0
    } else {
        height
    };
    let live = render_live_tail_mode(state, width, live_height, transcript, true);
    if height == 0 || state.selected_subagent().is_some() || state.modal_open() {
        return live;
    }

    let capacity = height as usize;
    let target_rows = capacity
        .saturating_add(transcript.scroll_rows())
        .saturating_add(1);
    let mut blocks = Vec::new();
    let mut rows = live.len();
    for index in (state.history_display_start()..state.history().len()).rev() {
        if rows >= target_rows {
            break;
        }
        let block = render_committed_history_block_snapshot(state, index, width);
        rows += block.len();
        blocks.push(block);
    }
    let mut lines = Vec::new();
    for block in blocks.into_iter().rev() {
        lines.extend(block);
    }
    lines.extend(live);
    let max_scroll = lines.len().saturating_sub(capacity);
    let scroll = transcript.clamp_scroll_rows(max_scroll);
    let end = lines.len().saturating_sub(scroll);
    let start = end.saturating_sub(capacity);
    lines.truncate(end);
    lines.drain(..start);
    lines
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
    let message_padding = 2;
    let content_width = (width as usize).saturating_sub(message_padding).max(10);
    let label = "─ Conversation recap ─";
    let line_style = get_themed_style(COLOR_TURN_SEPARATOR(), COLOR_BG(), Modifier::empty(), false);
    let label_style = get_themed_style(COLOR_TURN_SEPARATOR(), COLOR_BG(), Modifier::BOLD, false);
    let label_width = label.width();
    let mut lines = vec![Line::from("")];
    lines.push(Line::from(vec![
        Span::styled(label, label_style),
        Span::styled(
            "─".repeat((width as usize).saturating_sub(label_width)),
            line_style,
        ),
    ]));
    lines.push(Line::from(""));
    let message_padding = Span::styled("  ", line_style);
    let recap = rustcode::controller::sanitize_recap_content(content);
    if !recap.is_empty() {
        let mut recap_lines = Vec::new();
        push_wrapped_with_continuation(
            &mut recap_lines,
            vec![Span::styled(
                recap,
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), false),
            )],
            content_width,
            Some(message_padding.clone()),
        );
        if let Some(first) = recap_lines.first_mut() {
            first.spans.insert(0, message_padding);
        }
        lines.extend(recap_lines);
    }
    lines.push(Line::from(""));
    lines.into_iter().map(|line| own_line(&line)).collect()
}

#[cfg(test)]
pub(crate) fn render_committed_tool_result_group(
    state: &AppState,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(&state);
    render_committed_tool_result_group_snapshot(&snapshot, message_indices, width, show_picker)
}

#[cfg(test)]
pub(crate) fn render_work_separator_before_assistant(
    state: &AppState,
    assistant_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(&state);
    render_work_separator_before_assistant_snapshot(&snapshot, assistant_index, width)
}

#[cfg(test)]
pub(crate) fn build_claude_startup_banner(
    state: &AppState,
    total_width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(&state);
    build_claude_startup_banner_snapshot(&snapshot, total_width, max_height)
}

#[cfg(test)]
pub(crate) fn render_committed_history_block(
    state: &AppState,
    message_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let snapshot = render_snapshot(&state);
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

#[cfg(test)]
pub(crate) fn render_committed_assistant_text(
    _state: &AppState,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    render_committed_assistant_text_snapshot(&RenderSnapshot::new(_state), content, width)
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
