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

/// Test-only single-frame entry point.
///
/// Returns the composer rect the frame laid out. The view is a read
/// projection, so there is no engine session left to publish layout metrics
/// into; tests that assert the composer position read this instead.
#[cfg(test)]
pub fn render(f: &mut Frame, view: &RenderState) -> (u16, ratatui::layout::Rect) {
    let mut transcript = TranscriptState::default();
    let snapshot = render_snapshot(view);
    render_with_transcript_snapshot(f, &snapshot, &mut transcript)
}

// Memoized wrap measurement for the visible viewport.
//
// `lines` is already viewport-bounded, but `Paragraph::new(line.clone())`
// per line per frame still allocates. Hash the contents (no allocation) and
// reuse the last counts when width + hash match; the height is the sum so no
// second full `lines.clone()` is needed (#1582).
thread_local! {
    static WRAP_CACHE: std::cell::RefCell<Option<(u16, u64, Vec<usize>, u16)>> =
        const { std::cell::RefCell::new(None) };
}

fn lines_content_hash(lines: &[Line<'static>]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lines.len().hash(&mut hasher);
    for line in lines {
        line.width().hash(&mut hasher);
        for span in &line.spans {
            span.content.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn cached_wrap_counts(lines: &[Line<'static>], layout_width: u16) -> (Vec<usize>, u16) {
    let hash = lines_content_hash(lines);
    if let Some((w, h, counts, total)) = WRAP_CACHE.with(|c| c.borrow().clone())
        && w == layout_width
        && h == hash
    {
        return (counts, total);
    }
    let mut counts = Vec::with_capacity(lines.len());
    let mut total: usize = 0;
    for line in lines {
        let count = Paragraph::new(line.clone())
            .wrap(Wrap { trim: false })
            .line_count(layout_width)
            .max(1);
        total += count;
        counts.push(count);
    }
    let height = total.min(u16::MAX as usize) as u16;
    WRAP_CACHE.with(|c| {
        *c.borrow_mut() = Some((layout_width, hash, counts.clone(), height));
    });
    (counts, height)
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

/// Paint the transcript status row into the composer's own top padding row.
///
/// The composer's blank panel row above the input is the one place that is
/// neither transcript content nor composer content, so the affordance never
/// overwrites a row the reader is on and never changes the layout: the wheel
/// and page-step math above it is untouched. Following is the default, so an
/// ordinary frame paints nothing, and a hidden control owns no rectangle, so
/// it cannot intercept a click either (#1595, #1594).
fn render_transcript_status_row(
    f: &mut Frame,
    state: &RenderSnapshot,
    transcript: &mut TranscriptState,
    control_area: Option<ratatui::layout::Rect>,
) {
    let Some(gap) = control_area else {
        transcript.follow_control.clear();
        return;
    };
    if !transcript.tail_visible() {
        transcript.follow_control.render(
            Some(gap),
            f.area().width,
            transcript.unseen_activity(),
            f.buffer_mut(),
            false,
        );
        return;
    }
    transcript.follow_control.clear();
    // The candidate walk is proportional to the tool history, and the collapsed
    // default is the common case: an empty set is answered without walking, so
    // an ordinary frame never pays for the readout.
    if state.expanded_thoughts().is_empty() {
        return;
    }
    let (expanded, collapsible) = super::tool_transcript::expand_progress(state, f.area().width);
    if collapsible == 0 {
        return;
    }
    f.render_widget(
        Paragraph::new(format!(
            "{expanded}/{collapsible} expanded · ctrl+o all · ctrl+shift+o step"
        ))
        .style(Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL())),
        gap,
    );
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

/// Height of the mutable inline surface for the next frame.
///
/// Full height, so the readable transcript is the projection this surface
/// paints rather than rows the terminal has already scrolled away (#1587).
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
    view: &RenderState,
    transcript: &mut TranscriptState,
    width: u16,
    terminal_height: u16,
) -> u16 {
    let snapshot = render_snapshot(view);
    desired_height_snapshot(&snapshot, transcript, width, terminal_height)
}

/// Interactive TUI entry point. `transcript` is terminal-only mutable state;
/// it must never be persisted with `ChatMessage` history or included in a
/// provider request.
/// True when every cell of `row` inside `area` is still blank, so an overlay
/// can claim it without hiding anything the transcript painted.
fn row_is_blank(buffer: &ratatui::buffer::Buffer, area: ratatui::layout::Rect, row: u16) -> bool {
    if area.width == 0 || row < area.y || row >= area.bottom() {
        return false;
    }
    (area.x..area.right()).all(|x| {
        let cell = &buffer[(x, row)];
        cell.symbol() == " " && cell.bg == COLOR_BG()
    })
}

/// The first row after the last row that carries content in `area`.
///
/// Returns `None` when every row is blank, so callers can tell "anchor under
/// the activity" apart from "there is nothing on screen yet".
fn row_after_last_content(
    buffer: &ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
) -> Option<u16> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    (area.y..area.bottom())
        .rev()
        .find(|row| !row_is_blank(buffer, area, *row))
        .and_then(|row| {
            let below = row.saturating_add(1);
            (below < area.bottom()).then_some(below)
        })
}

pub(crate) fn render_with_transcript_snapshot(
    f: &mut Frame,
    state: &RenderSnapshot,
    transcript: &mut TranscriptState,
) -> (u16, ratatui::layout::Rect) {
    theme::set_active_theme(&state.config().theme);

    let completion_dismissed =
        state.dismissed_completion() == state.completion_identity().as_deref();
    let filtered_cmds: Vec<&CommandInfo> = if completion_dismissed || state.modal_open() {
        Vec::new()
    } else {
        rustcode::controller::filtered_commands(&state.input_buffer())
    };

    let inner_width = f.area().width.max(1);
    let chat_width = f.area().width.max(1);
    let raw_input_lines = input_line_count(state, inner_width as usize);
    let approval_active =
        *state.status() == AppStatus::AwaitingToolConfirmation && !state.user_overlay_open();
    let question_active =
        *state.status() == AppStatus::AwaitingQuestion && !state.user_overlay_open();
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
        && !state.modal_open()
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
        Some(completion_footer_hint_clauses(!filtered_cmds.is_empty()))
    } else {
        None
    };
    let footer_visible = composer_footer_visible(state);
    // Reserve the footer row even while a completion popup replaces its text so
    // the composer does not jump when the popup opens or closes.
    let footer_height = 1;
    let (top_padding, _) = live_surface_padding(state);
    let mut activity_lines =
        super::composer_render::background_command_lines_with_width(state, chat_width);
    // Reserve a stable row above the composer for return-to-latest. Panels,
    // confirmations, questions, and completions suppress the control there.
    let control_row_suppressed =
        state.modal_open() || approval_active || question_active || popup_rows > 0;
    // Keep viewport geometry stable as the reader scrolls; conditionally
    // adding this row after a wheel/drag event would shift selection anchors.
    let control_row_height = 1;
    // The stable control slot takes over the old one-row bottom pad. Keeping
    // its height fixed, even when a panel hides the control, preserves both
    // the transcript viewport and the composer's bottom anchor.
    let bottom_padding = 0;
    let vertical_padding = top_padding.saturating_add(bottom_padding);
    // Decide on optional breathing room around live activity (#1494). Gaps are
    // dropped first on short terminals so chat/input are never clipped for
    // spacing, and never added when activity is absent.
    let reserved_without_gaps = vertical_padding
        .saturating_add(queue_block_height)
        .saturating_add(provisional_input_height)
        .saturating_add(control_row_height)
        .saturating_add(footer_height)
        .saturating_add(activity_lines.len() as u16)
        .saturating_add(popup_rows);
    let (activity_gap_top, activity_gap_bottom) = activity_spacing(
        !activity_lines.is_empty(),
        f.area().height,
        reserved_without_gaps,
        3,
    );
    let activity_gaps = activity_gap_top.saturating_add(activity_gap_bottom);
    let activity_height = (activity_lines.len() as u16).min(
        f.area()
            .height
            .saturating_sub(vertical_padding)
            .saturating_sub(queue_block_height)
            .saturating_sub(provisional_input_height)
            .saturating_sub(control_row_height)
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
            .saturating_sub(control_row_height)
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
            .saturating_sub(control_row_height)
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
        .saturating_sub(control_row_height)
        .saturating_sub(input_height)
        .saturating_sub(activity_height)
        .saturating_sub(activity_gaps)
        .saturating_sub(footer_height)
        .saturating_sub(popup_height);
    // Slash-command modals render as a bounded panel above the composer.
    // Reserve their rows so the transcript keeps the space above the panel
    // instead of being painted over.
    let modal_height = open_modal_max_height(state).min(max_chat_height);
    let chat_surface_height = max_chat_height.saturating_sub(modal_height);
    let indicator = live_running_indicator(state);
    // Own the gap and indicator rows so even a full transcript keeps activity
    // visible. Leave at least one transcript row on short terminals.
    let indicator_height = if indicator.is_some() {
        2.min(chat_surface_height.saturating_sub(1))
    } else {
        0
    };
    let chat_height = chat_surface_height.saturating_sub(indicator_height);
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
    // Wrap measurement is memoized by width + content hash: repeated frames
    // with the same viewport reuse counts without cloning Lines per line.
    // The content height is the sum of the same counts (no second full
    // `lines.clone()`) (#1582).
    let (wrapped_counts, conversation_content_height) = cached_wrap_counts(&lines, layout_width);
    let soft_wrap_before: Vec<bool> = wrapped_counts
        .iter()
        .flat_map(|&count| {
            std::iter::once(false).chain(std::iter::repeat_n(true, count.saturating_sub(1)))
        })
        .take(chat_height as usize)
        .collect();

    let mut chunks = Layout::default()
        .direction(Direction::Vertical)
        .horizontal_margin(0)
        .constraints([
            Constraint::Length(chat_surface_height),
            Constraint::Length(activity_gap_top),
            Constraint::Length(activity_height),
            Constraint::Length(activity_gap_bottom),
            Constraint::Length(queue_block_height),
            Constraint::Length(control_row_height),
            Constraint::Length(modal_height),
            Constraint::Length(popup_height),
            Constraint::Length(input_height),
            Constraint::Length(footer_height),
        ])
        .split(layout_area)
        .to_vec();
    let chat_surface = chunks[0];
    chunks[0].height = chat_height;

    f.render_widget(
        Paragraph::new("").style(Style::default().bg(COLOR_BG())),
        chat_surface,
    );
    render_live_conversation(f, chunks[0], lines, layout_width);

    // Scan backgrounds as well as text: the user's shaded bottom padding
    // belongs to the message. Keep a blank row between it and the indicator.
    if let Some(indicator) = indicator
        && indicator_height > 0
    {
        let target = row_after_last_content(f.buffer(), chat_surface)
            .map(|row| row.saturating_add(1))
            .unwrap_or(chat_surface.y)
            .min(chat_surface.bottom().saturating_sub(1));
        if row_is_blank(f.buffer(), chat_surface, target) {
            f.render_widget(
                Paragraph::new(indicator).style(Style::default().bg(COLOR_BG())),
                ratatui::layout::Rect::new(chat_surface.x, target, chat_surface.width, 1),
            );
        }
    }

    // The composer indexes these as [chat, queue, popup, input, footer]; the
    // activity stays above the queue, panels and completions. Panels claim
    // the rows directly above input, exactly where their anchor paints.
    let composer_chunks = [chunks[0], chunks[4], chunks[7], chunks[8], chunks[9]];
    render_queue_line(f, &composer_chunks, state);
    // Optional breathing room around live activity (#1494). Gaps are empty
    // background rows; they are omitted when activity is absent or the
    // terminal is too short.
    if activity_gap_top > 0 {
        f.render_widget(
            Paragraph::new("").style(Style::default().bg(COLOR_BG())),
            chunks[1],
        );
    }
    if activity_height > 0 {
        f.render_widget(
            Paragraph::new(activity_lines).style(Style::default().bg(COLOR_BG())),
            chunks[2],
        );
    }
    if activity_gap_bottom > 0 {
        f.render_widget(
            Paragraph::new("").style(Style::default().bg(COLOR_BG())),
            chunks[3],
        );
    }
    let question_area = if question_active && !question_in_bottom_pane {
        let height = question_height(
            state,
            f.area().width,
            layout_area.height.saturating_sub(footer_height),
        );
        Some(ratatui::layout::Rect::new(
            chunks[8].x,
            chunks[8].bottom().saturating_sub(height),
            chunks[8].width,
            height,
        ))
    } else {
        None
    };
    let input_margin = if approval_active {
        render_tool_confirmation_modal(f, state, chunks[8]);
        Margin {
            vertical: 0,
            horizontal: 0,
        }
    } else if question_in_bottom_pane {
        render_question_modal(f, state, chunks[8]);
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
        // A live selection is the modal gesture, so the footer names the copy
        // key for as long as it lasts and drops it the moment the selection is
        // cleared (#1542). Both gestures pin the painted viewport, so the
        // content-drift clear in `refresh_view` below cannot fire while a
        // selection is live: reading the range here is already its answer.
        render_composer_footer(
            f,
            chunks[9],
            state,
            popup_hint,
            transcript.selection.has_selection() || transcript.panel_selection.has_selection(),
        );
    }

    if !filtered_cmds.is_empty() {
        let input_inner = chunks[8].inner(input_margin);
        let popup_area = ratatui::layout::Rect::new(
            input_inner.x,
            chunks[7].y,
            input_inner.width,
            chunks[7].height,
        );
        render_popup_menu(f, state, &filtered_cmds, popup_area);
    } else if !at_files.is_empty() {
        let input_inner = chunks[8].inner(input_margin);
        let popup_area = ratatui::layout::Rect::new(
            input_inner.x,
            chunks[7].y,
            input_inner.width,
            chunks[7].height,
        );
        render_at_popup_menu(f, state, &at_files, popup_area);
    }

    let input_box_area = question_area.unwrap_or(chunks[8]);

    f.render_in_area(chunks[6], |f| {
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

        if state.command_panel().is_some() {
            render_command_panel(f, state, input_box_area);
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

        if state.settings_picker() == Some(rustcode::controller::SettingsPicker::Verbosity) {
            render_verbosity_picker_modal(f, state, input_box_area);
        }

        if state.settings_picker() == Some(rustcode::controller::SettingsPicker::Thinking) {
            render_thinking_picker_modal(f, state, input_box_area);
        }

        if state.settings_picker() == Some(rustcode::controller::SettingsPicker::Effort) {
            render_effort_picker_modal(f, state, input_box_area);
        }

        if state.settings_picker() == Some(rustcode::controller::SettingsPicker::Protocol) {
            render_protocol_picker_modal(f, state, input_box_area);
        }

        if state.settings_picker() == Some(rustcode::controller::SettingsPicker::Yolo) {
            render_yolo_picker_modal(f, state, input_box_area);
        }
    });

    render_transcript_status_row(
        f,
        state,
        transcript,
        (!control_row_suppressed).then_some(chunks[5]),
    );

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

    let panel_selection = panel_selection_surface(f, state, input_box_area);
    transcript.panel_selection_area = panel_selection.as_ref().map(|(area, _)| *area);
    transcript.panel_selection_scrollable = state.command_panel().is_some();
    if let Some((area, soft_wrap_before)) = panel_selection {
        transcript
            .panel_selection
            .refresh_view(area, f.buffer(), &soft_wrap_before, 0);
        transcript.panel_selection.highlight(f.buffer_mut());
    } else {
        // Closing the panel must not leave its selection available over the
        // conversation on the next frame.
        transcript.panel_selection.clear();
    }

    (conversation_content_height, input_box_area)
}

/// Test-only frame entry point that keeps a caller-owned transcript.
///
/// Returns the composer rect the frame laid out, for the same reason
/// [`render`] does.
#[cfg(test)]
pub fn render_with_transcript(
    f: &mut Frame,
    view: &RenderState,
    transcript: &mut TranscriptState,
) -> (u16, ratatui::layout::Rect) {
    let snapshot = render_snapshot(view);
    render_with_transcript_snapshot(f, &snapshot, transcript)
}
