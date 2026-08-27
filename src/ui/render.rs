use super::*;

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

pub fn render(f: &mut Frame, state: &mut AppState) {
    let mut transcript = TranscriptState::default();
    let snapshot = state.render_snapshot();
    let revision = snapshot.revision();
    let (content_height, input_area) =
        render_with_transcript_snapshot(f, &snapshot, &mut transcript);
    state.publish_render_metrics(revision, content_height, input_area);
}

pub(super) fn live_surface_padding(state: &RenderSnapshot) -> (u16, u16) {
    let active = matches!(state.status(), AppStatus::Streaming | AppStatus::Queued)
        || !state.running_tools().is_empty();
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
