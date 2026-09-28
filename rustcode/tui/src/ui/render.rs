use super::*;
#[cfg(test)]
use crate::ui::render_snapshot::render_snapshot;

pub(super) fn render_live_conversation(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    lines: Vec<Line<'static>>,
) {
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(COLOR_BG())),
        area,
    );
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
        rustcode::app::UiRect::new(
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
    let provisional_input_height = if approval_active {
        tool_confirmation_height(state, f.area().height.saturating_sub(2))
    } else if question_active {
        question_height(state, f.area().width, f.area().height.saturating_sub(2))
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
    let footer_visible =
        composer_footer_visible(state, !filtered_cmds.is_empty(), !at_files.is_empty());
    // Reserve the footer row while completion hides its text so the composer
    // does not jump when the popup opens or closes.
    let footer_height = 1;
    let (top_padding, bottom_padding) = live_surface_padding(state);
    let vertical_padding = top_padding.saturating_add(bottom_padding);
    // Reserve completion rows above the composer so input stays anchored.
    let popup_height = popup_rows.min(
        f.area()
            .height
            .saturating_sub(vertical_padding)
            .saturating_sub(queue_block_height)
            .saturating_sub(provisional_input_height)
            .saturating_sub(footer_height),
    );

    let input_height = if approval_active || question_active {
        provisional_input_height
    } else {
        let max_input_lines = f
            .area()
            .height
            .saturating_sub(vertical_padding)
            .saturating_sub(queue_block_height)
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
        .saturating_sub(footer_height)
        .saturating_sub(popup_height);
    let layout_area = inset_vertical(f.area(), top_padding, bottom_padding);

    let lines =
        render_visible_conversation_with_transcript(state, chat_width, max_chat_height, transcript);
    let conversation_content_height = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(chat_width) as u16;

    let chat_height = max_chat_height;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .horizontal_margin(0)
        .constraints([
            Constraint::Length(chat_height),
            Constraint::Length(queue_block_height),
            Constraint::Length(popup_height),
            Constraint::Length(input_height),
            Constraint::Length(footer_height),
        ])
        .split(layout_area);

    render_live_conversation(f, chunks[0], lines);

    render_queue_line(f, &chunks, state);
    let input_margin = if approval_active {
        render_tool_confirmation_modal(f, state, chunks[3]);
        Margin {
            vertical: 0,
            horizontal: 0,
        }
    } else if question_active {
        render_question_modal(f, state, chunks[3]);
        Margin {
            vertical: 0,
            horizontal: 0,
        }
    } else {
        Composer::default().render(f, &chunks, state)
    };
    if footer_visible {
        render_composer_footer(f, chunks[4], state);
    }

    if !filtered_cmds.is_empty() {
        let input_inner = chunks[3].inner(input_margin);
        let popup_area = ratatui::layout::Rect::new(
            input_inner.x,
            chunks[2].y,
            input_inner.width,
            chunks[2].height,
        );
        render_popup_menu(f, state, &filtered_cmds, popup_area);
    } else if !at_files.is_empty() {
        let input_inner = chunks[3].inner(input_margin);
        let popup_area = ratatui::layout::Rect::new(
            input_inner.x,
            chunks[2].y,
            input_inner.width,
            chunks[2].height,
        );
        render_at_popup_menu(f, state, &at_files, popup_area);
    }

    let input_box_area = chunks[3];

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
        rustcode::app::UiRect::new(
            input_area.x,
            input_area.y,
            input_area.width,
            input_area.height,
        ),
    );
}
