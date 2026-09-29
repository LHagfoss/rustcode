use super::*;
#[cfg(test)]
use crate::ui::render_snapshot::render_snapshot;

pub(super) fn render_live_conversation(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    lines: Vec<Line<'static>>,
    layout_width: u16,
) {
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(COLOR_BG()));
    if layout_width == area.width {
        f.render_widget(paragraph, area);
        return;
    }
    // Retain the selected snapshot's visual row layout across terminal resize.
    let source_area = ratatui::layout::Rect::new(0, 0, layout_width, area.height);
    let mut source = ratatui::buffer::Buffer::empty(source_area);
    ratatui::widgets::Widget::render(paragraph, source_area, &mut source);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(COLOR_BG())),
        area,
    );
    for y in 0..area.height {
        for x in 0..area.width.min(layout_width) {
            if let Some(cell) = f.buffer_mut().cell_mut((area.x + x, area.y + y)) {
                *cell = source[(x, y)].clone();
            }
        }
    }
}

#[cfg(test)]
pub fn render(f: &mut Frame, state: &mut AppState) {
    let mut transcript = TranscriptState::default();
    let snapshot = render_snapshot(&state);
    let revision = snapshot.revision();
    let (content_height, input_area) =
        render_with_transcript_snapshot(f, &snapshot, &mut transcript);
    state.publish_render_metrics(
        revision,
        content_height,
        rustcode::controller::UiRect::new(
            input_area.x,
            input_area.y,
            input_area.width,
            input_area.height,
        ),
    );
}

pub(super) fn live_surface_padding(state: &RenderSnapshot) -> (u16, u16) {
    let active = matches!(state.status(), AppStatus::Streaming | AppStatus::Queued)
        || !state.running_tools().is_empty()
        || !state.background_tasks().is_empty();
    (u16::from(!active), 1)
}

/// Optional breathing room above/below the live activity block (#1494).
/// Returns `(top_gap, bottom_gap)` in terminal rows. Gaps are 1 row each when
/// activity is visible and the terminal has room to keep chat/input unclipped;
/// otherwise 0. Idle views never gain a gap.
pub(super) fn activity_spacing(
    activity_visible: bool,
    terminal_height: u16,
    reserved_height: u16,
    min_chat_height: u16,
) -> (u16, u16) {
    if !activity_visible {
        return (0, 0);
    }
    // Reserve 2 rows for gaps plus a minimal chat window; drop gaps first on
    // short terminals so content is not clipped.
    if terminal_height.saturating_sub(reserved_height) >= min_chat_height.saturating_add(2) {
        (1, 1)
    } else {
        (0, 0)
    }
}

pub(super) fn inset_vertical(
    area: ratatui::layout::Rect,
    top: u16,
    bottom: u16,
) -> ratatui::layout::Rect {
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
    _state: &RenderSnapshot,
    _transcript: &mut TranscriptState,
    _width: u16,
    terminal_height: u16,
) -> u16 {
    // Keep the composer anchored to the terminal bottom. The mutable viewport
    // owns the screen while the transcript still commits finalized rows to
    // scrollback above it.
    terminal_height.max(1)
}

#[cfg(test)]
pub(crate) fn desired_height(
    state: &AppState,
    transcript: &mut TranscriptState,
    width: u16,
    terminal_height: u16,
) -> u16 {
    let snapshot = render_snapshot(&state);
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
        rustcode::controller::filtered_commands(&state.input_buffer())
    };

    let inner_width = f.area().width.max(1);
    let chat_width = f.area().width.max(1);
    let raw_input_lines = input_line_count(state, inner_width as usize);
    let approval_active = *state.status() == AppStatus::AwaitingToolConfirmation;
    let question_active = *state.status() == AppStatus::AwaitingQuestion;
    // Keep a lone prior answer visible above the question. With a longer
    // transcript, overlay the question so the visible chat does not reflow.
    let question_in_bottom_pane = question_active && state.history().len() <= 1;
    let provisional_input_height = if approval_active {
        tool_confirmation_height(state, f.area().height.saturating_sub(2))
    } else if question_in_bottom_pane {
        question_height(state, f.area().width, f.area().height.saturating_sub(2))
    } else if question_active {
        3
    } else {
        raw_input_lines + 2
    };
    let queue_block_height = queue_preview_height(state);

    let (_, at_query) =
        rustcode_core::input::get_at_word_query(&state.input_buffer(), state.cursor_position())
            .unwrap_or((0, String::new()));
    let at_files = if !completion_dismissed
        && (!at_query.is_empty()
            || state.input_buffer()
                [..safe_byte_index(&state.input_buffer(), state.cursor_position())]
                .ends_with('@'))
    {
        rustcode::controller::list_project_file_paths(&at_query)
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
    let popup_hint = if !filtered_cmds.is_empty() || !at_files.is_empty() {
        Some(completion_footer_hint(!filtered_cmds.is_empty()))
    } else {
        None
    };
    let footer_visible = composer_footer_visible(state);
    // Reserve the footer row even while a completion popup replaces its text so
    // the composer does not jump when the popup opens or closes.
    let footer_height = 1;
    let (top_padding, bottom_padding) = live_surface_padding(state);
    let vertical_padding = top_padding.saturating_add(bottom_padding);
    let activity_visible = matches!(state.status(), AppStatus::Streaming | AppStatus::Queued)
        || !state.running_tools().is_empty()
        || !state.background_tasks().is_empty();
    let mut activity_lines = if activity_visible {
        let mut lines = background_command_lines(state);
        lines.push(activity_status_line(state, false));
        lines
    } else {
        Vec::new()
    };
    // Decide on optional breathing room around live activity (#1494). Gaps are
    // dropped first on short terminals so chat/input are never clipped for
    // spacing, and never added when activity is absent.
    let reserved_without_gaps = vertical_padding
        .saturating_add(queue_block_height)
        .saturating_add(provisional_input_height)
        .saturating_add(footer_height)
        .saturating_add(activity_lines.len() as u16)
        .saturating_add(popup_rows);
    let (activity_gap_top, activity_gap_bottom) =
        activity_spacing(activity_visible, f.area().height, reserved_without_gaps, 3);
    let activity_gaps = activity_gap_top.saturating_add(activity_gap_bottom);
    let activity_height = (activity_lines.len() as u16).min(
        f.area()
            .height
            .saturating_sub(vertical_padding)
            .saturating_sub(queue_block_height)
            .saturating_sub(provisional_input_height)
            .saturating_sub(footer_height)
            .saturating_sub(activity_gaps),
    );
    if activity_lines.len() > activity_height as usize {
        activity_lines.drain(..activity_lines.len() - activity_height as usize);
    }
    // Reserve completion rows above the composer so input stays anchored.
    let popup_height = popup_rows.min(
        f.area()
            .height
            .saturating_sub(vertical_padding)
            .saturating_sub(queue_block_height)
            .saturating_sub(provisional_input_height)
            .saturating_sub(activity_height)
            .saturating_sub(activity_gaps)
            .saturating_sub(footer_height),
    );

    let input_height = if approval_active || question_in_bottom_pane {
        provisional_input_height
    } else {
        let max_input_lines = f
            .area()
            .height
            .saturating_sub(vertical_padding)
            .saturating_sub(queue_block_height)
            .saturating_sub(activity_height)
            .saturating_sub(activity_gaps)
            .saturating_sub(footer_height)
            .saturating_sub(popup_height)
            .saturating_sub(2)
            .max(1);
        raw_input_lines.min(max_input_lines) + 2
    };

    let max_chat_height = f
        .area()
        .height
        .saturating_sub(vertical_padding)
        .saturating_sub(queue_block_height)
        .saturating_sub(input_height)
        .saturating_sub(activity_height)
        .saturating_sub(activity_gaps)
        .saturating_sub(footer_height)
        .saturating_sub(popup_height);
    // Slash-command modals render as a bounded panel above the composer.
    // Reserve their rows so the transcript keeps the space above the panel
    // instead of being painted over.
    let modal_height = open_modal_max_height(state).min(max_chat_height);
    let chat_height = max_chat_height.saturating_sub(modal_height);
    let layout_area = inset_vertical(f.area(), top_padding, bottom_padding);

    let pinned = transcript.selection.pinned_snapshot();
    let layout_width = transcript.selection.pinned_width().unwrap_or(chat_width);
    let display_state = pinned.as_deref().unwrap_or(state);
    let lines = render_visible_conversation_with_transcript(
        display_state,
        layout_width,
        chat_height,
        transcript,
    );
    let soft_wrap_before = lines
        .iter()
        .flat_map(|line| {
            let count = Paragraph::new(line.clone())
                .wrap(Wrap { trim: false })
                .line_count(layout_width)
                .max(1);
            std::iter::once(false).chain(std::iter::repeat_n(true, count.saturating_sub(1)))
        })
        .take(chat_height as usize)
        .collect::<Vec<_>>();
    let conversation_content_height = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(layout_width) as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .horizontal_margin(0)
        .constraints([
            Constraint::Length(chat_height),
            Constraint::Length(modal_height),
            Constraint::Length(queue_block_height),
            Constraint::Length(popup_height),
            Constraint::Length(activity_gap_top),
            Constraint::Length(activity_height),
            Constraint::Length(activity_gap_bottom),
            Constraint::Length(input_height),
            Constraint::Length(footer_height),
        ])
        .split(layout_area);

    render_live_conversation(f, chunks[0], lines, layout_width);

    // The composer indexes these as [chat, queue, popup, input, footer]; the
    // reserved modal rows sit between the chat and the queue.
    let composer_chunks = [chunks[0], chunks[2], chunks[3], chunks[7], chunks[8]];
    render_queue_line(f, &composer_chunks, state);
    // Optional breathing room around live activity (#1494). Gaps are empty
    // background rows; they are omitted when activity is absent or the
    // terminal is too short.
    if activity_gap_top > 0 {
        f.render_widget(
            Paragraph::new("").style(Style::default().bg(COLOR_BG())),
            chunks[4],
        );
    }
    if activity_height > 0 {
        f.render_widget(
            Paragraph::new(activity_lines).style(Style::default().bg(COLOR_BG())),
            chunks[5],
        );
    }
    if activity_gap_bottom > 0 {
        f.render_widget(
            Paragraph::new("").style(Style::default().bg(COLOR_BG())),
            chunks[6],
        );
    }
    let question_area = if question_active && !question_in_bottom_pane {
        let height = question_height(
            state,
            f.area().width,
            layout_area.height.saturating_sub(footer_height),
        );
        Some(ratatui::layout::Rect::new(
            chunks[7].x,
            chunks[7].bottom().saturating_sub(height),
            chunks[7].width,
            height,
        ))
    } else {
        None
    };
    let input_margin = if approval_active {
        render_tool_confirmation_modal(f, state, chunks[7]);
        Margin {
            vertical: 0,
            horizontal: 0,
        }
    } else if question_in_bottom_pane {
        render_question_modal(f, state, chunks[7]);
        Margin {
            vertical: 0,
            horizontal: 0,
        }
    } else if let Some(area) = question_area {
        render_question_modal(f, state, area);
        Margin {
            vertical: 0,
            horizontal: 0,
        }
    } else {
        Composer::default().render(f, &composer_chunks, state)
    };
    if footer_visible {
        render_composer_footer(f, chunks[8], state, popup_hint);
    }

    if !filtered_cmds.is_empty() {
        let input_inner = chunks[7].inner(input_margin);
        let popup_area = ratatui::layout::Rect::new(
            input_inner.x,
            chunks[3].y,
            input_inner.width,
            chunks[3].height,
        );
        render_popup_menu(f, state, &filtered_cmds, popup_area);
    } else if !at_files.is_empty() {
        let input_inner = chunks[7].inner(input_margin);
        let popup_area = ratatui::layout::Rect::new(
            input_inner.x,
            chunks[3].y,
            input_inner.width,
            chunks[3].height,
        );
        render_at_popup_menu(f, state, &at_files, popup_area);
    }

    let input_box_area = question_area.unwrap_or(chunks[7]);

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

    if state.show_status_modal() {
        render_status_modal(f, state, input_box_area);
    }

    if state.show_stats_modal() {
        render_stats_modal(f, state, input_box_area);
    }

    if state.show_session_modal() {
        render_session_modal(f, state, input_box_area);
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

    let selection_area = if let Some(question_area) = question_area {
        ratatui::layout::Rect::new(
            chunks[0].x,
            chunks[0].y,
            chunks[0].width,
            question_area.y.saturating_sub(chunks[0].y),
        )
    } else {
        chunks[0]
    };
    transcript.selection.refresh_view(
        selection_area,
        f.buffer(),
        &soft_wrap_before,
        transcript.scroll_rows(),
    );
    transcript.selection.highlight(f.buffer_mut());

    (conversation_content_height, input_box_area)
}

#[cfg(test)]
pub fn render_with_transcript(
    f: &mut Frame,
    state: &mut AppState,
    transcript: &mut TranscriptState,
) {
    let snapshot = render_snapshot(&state);
    let revision = snapshot.revision();
    let (content_height, input_area) = render_with_transcript_snapshot(f, &snapshot, transcript);
    state.publish_render_metrics(
        revision,
        content_height,
        rustcode::controller::UiRect::new(
            input_area.x,
            input_area.y,
            input_area.width,
            input_area.height,
        ),
    );
}
