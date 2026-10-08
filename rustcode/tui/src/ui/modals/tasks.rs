use super::*;

/// Render the tasks panel: the task list, or the selected task's log.
///
/// Everything shown comes from the view the controller rebuilt for this
/// frame, so a row's elapsed time and state are never a stale copy.
pub(in crate::ui) fn render_tasks_panel_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let Some(panel) = state.tasks_panel() else {
        return;
    };
    let modal_area = input_anchor_rect(f, input_area, TASKS_PANEL_HEIGHT);
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        modal_area,
    );
    let inner = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);
    let width = inner.width as usize;
    let muted = Style::default().fg(COLOR_MUTED());
    let running = panel.rows.iter().filter(|row| row.is_running()).count();

    let (title, hints): (String, &[&'static str]) = match &panel.log {
        Some(log) => (
            format!("Task log · {}", tasks_panel_label(&log.command, width)),
            &["↑/↓ scroll", "esc back", "x stop"],
        ),
        None => (
            match running {
                0 => "Tasks".to_owned(),
                1 => "Tasks · 1 running".to_owned(),
                count => format!("Tasks · {count} running"),
            },
            &["↑/↓ select", "enter log", "x stop", "esc close"],
        ),
    };
    let right = "esc";
    let title = crate::ui::composer_render::truncate_queue_prompt(
        &title,
        width.saturating_sub(right.len() + 1),
    );
    let padding = picker_header_padding(width, &title, right);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                title,
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(padding)),
            Span::styled(right, muted),
        ]))
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            crate::ui::composer_render::fit_hint_clauses("", hints, width).unwrap_or_default(),
            muted,
        )))
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[1],
    );

    let height = chunks[2].height as usize;
    let lines = match &panel.log {
        Some(log) => tasks_panel_log_lines(log, width, height),
        None => tasks_panel_list_lines(panel, running, width, height),
    };
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(COLOR_PANEL())),
        chunks[2],
    );
}

fn tasks_panel_label(command: &str, width: usize) -> String {
    rustcode::controller::background_command_label(command, width.clamp(8, 80))
}

fn tasks_panel_list_lines(
    panel: &rustcode::controller::TasksPanelView,
    running: usize,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    use rustcode::controller::{TaskOutcome, TaskRowState};
    let muted = Style::default().fg(COLOR_MUTED());
    let mut lines = Vec::with_capacity(panel.rows.len() + 3);
    // The row index each line stands for, so the window follows the selection
    // across the headings between the groups.
    let mut line_rows = Vec::with_capacity(panel.rows.len() + 3);
    if running == 0 {
        lines.push(Line::from(Span::styled("No tasks are running.", muted)));
        line_rows.push(None);
    }
    for (index, row) in panel.rows.iter().enumerate() {
        if index == running {
            if !lines.is_empty() {
                lines.push(Line::default());
                line_rows.push(None);
            }
            lines.push(Line::from(Span::styled("Finished", muted)));
            line_rows.push(None);
        }
        let (marker, detail) = match &row.state {
            TaskRowState::Running { started_at } => (
                '•',
                rustcode_core::status::format_elapsed_compact(started_at.elapsed().as_secs()),
            ),
            TaskRowState::Finished { outcome, ran_for } => (
                match outcome {
                    TaskOutcome::Done => '✓',
                    TaskOutcome::Stopped => '✗',
                    TaskOutcome::Exit(_) | TaskOutcome::Failed => '✗',
                },
                format!(
                    "{} · {}",
                    outcome.label(),
                    rustcode_core::status::format_elapsed_compact(ran_for.as_secs())
                ),
            ),
        };
        let selected = index == panel.selected;
        let pointer = if selected { "› " } else { "  " };
        let fixed = pointer.width() + 2 + " · ".width() + detail.width();
        let label = tasks_panel_label(&row.command, width.saturating_sub(fixed));
        let text = crate::ui::composer_render::truncate_queue_prompt(
            &format!("{pointer}{marker} {label} · {detail}"),
            width,
        );
        let style = if selected {
            Style::default()
                .fg(COLOR_BG())
                .bg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD)
        } else if row.is_running() {
            Style::default().fg(COLOR_TEXT())
        } else {
            muted.add_modifier(Modifier::DIM)
        };
        lines.push(Line::from(Span::styled(text, style)));
        line_rows.push(Some(index));
    }
    let selected_line = line_rows
        .iter()
        .position(|row| *row == Some(panel.selected))
        .unwrap_or(0);
    let offset = usize::from(picker_list_window(selected_line, lines.len(), height));
    lines.into_iter().skip(offset).take(height).collect()
}

/// The newest rows of the log that fit, moved up by the view's scroll.
fn tasks_panel_log_lines(
    log: &rustcode::controller::TaskLogView,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let muted = Style::default().fg(COLOR_MUTED());
    let mut rows = Vec::new();
    if log.earlier_omitted {
        rows.push(Line::from(Span::styled(
            "… earlier output omitted",
            muted.add_modifier(Modifier::ITALIC),
        )));
    }
    for line in log.text.lines() {
        let mut wrapped = Vec::new();
        push_wrapped_with_continuation(
            &mut wrapped,
            vec![Span::styled(
                line.to_owned(),
                Style::default().fg(COLOR_TEXT()),
            )],
            width.max(1),
            None,
        );
        if wrapped.is_empty() {
            wrapped.push(Line::default());
        }
        rows.extend(wrapped);
    }
    let scroll = log.scroll.min(rows.len().saturating_sub(height));
    let end = rows.len() - scroll;
    rows.drain(..end.saturating_sub(height));
    rows.truncate(height);
    rows
}
