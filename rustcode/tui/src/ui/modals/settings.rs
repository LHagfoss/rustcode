use super::*;

pub(in crate::ui) fn render_verbosity_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, VERBOSITY_PICKER_HEIGHT);
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        modal_area,
    );

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Spacer
            Constraint::Min(2),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Output verbosity";
    let right_esc = "esc";
    let padding_header = picker_header_padding(inner_area.width as usize, &title_text, right_esc);
    let header_line = Line::from(vec![
        Span::styled(
            title_text,
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding_header), Style::default()),
        Span::styled(right_esc, Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[0],
    );

    let choices = [
        (
            "Low",
            rustcode::controller::Verbosity::Low,
            "Runs of tool calls fold into one row · click opens",
        ),
        (
            "High",
            rustcode::controller::Verbosity::High,
            "Tool calls only · click or ctrl+o opens output",
        ),
        (
            "Edits",
            rustcode::controller::Verbosity::Edits,
            "Like low · file edits stay open · ctrl+d toggles",
        ),
    ];

    let selected_idx = state
        .modal_picker_index()
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, verbosity_level, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = *state.verbosity() == *verbosity_level;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!("› {}", name);
            let padding_len = picker_row_padding(inner_area.width as usize, &left_text, &full_desc);
            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default()
                        .fg(COLOR_TEXT())
                        .bg(COLOR_HOVER_BG())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ".repeat(padding_len),
                    Style::default().fg(COLOR_TEXT()).bg(COLOR_HOVER_BG()),
                ),
                Span::styled(
                    full_desc,
                    Style::default().fg(COLOR_TEXT()).bg(COLOR_HOVER_BG()),
                ),
            ])
        } else {
            let left_text = format!("  {}", name);
            let padding_len = picker_row_padding(inner_area.width as usize, &left_text, &full_desc);
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(full_desc, Style::default().fg(COLOR_MUTED())),
            ])
        };
        list_lines.push(line);
    }

    f.render_widget(
        Paragraph::new(list_lines).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT())),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("cancel ", Style::default().fg(COLOR_TEXT())),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[3],
    );
}

pub(in crate::ui) fn render_yolo_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, YOLO_PICKER_HEIGHT);
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        modal_area,
    );

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Spacer
            Constraint::Min(2),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Automatic tool confirmation";
    let right_esc = "esc";
    let padding_header = picker_header_padding(inner_area.width as usize, &title_text, right_esc);
    let header_line = Line::from(vec![
        Span::styled(
            title_text,
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding_header), Style::default()),
        Span::styled(right_esc, Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[0],
    );

    let choices = [
        (
            "On",
            true,
            "Auto-confirm tool executions without prompting (YOLO)",
        ),
        (
            "Off",
            false,
            "Prompt for confirmation before executing mutating tools",
        ),
    ];

    let selected_idx = state
        .modal_picker_index()
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, val, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = state.auto_confirm() == *val;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!("› {}", name);
            let padding_len = picker_row_padding(inner_area.width as usize, &left_text, &full_desc);
            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default()
                        .fg(COLOR_TEXT())
                        .bg(COLOR_HOVER_BG())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ".repeat(padding_len),
                    Style::default().fg(COLOR_TEXT()).bg(COLOR_HOVER_BG()),
                ),
                Span::styled(
                    full_desc,
                    Style::default().fg(COLOR_TEXT()).bg(COLOR_HOVER_BG()),
                ),
            ])
        } else {
            let left_text = format!("  {}", name);
            let padding_len = picker_row_padding(inner_area.width as usize, &left_text, &full_desc);
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(full_desc, Style::default().fg(COLOR_MUTED())),
            ])
        };
        list_lines.push(line);
    }

    f.render_widget(
        Paragraph::new(list_lines).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT())),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("cancel ", Style::default().fg(COLOR_TEXT())),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[3],
    );
}

/// The `/scroll-speed` panel: one value, stepped with the arrow keys, drawn
/// as a number and a bar so the choice reads at a glance.
pub(in crate::ui) fn render_scroll_speed_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, SCROLL_SPEED_PICKER_HEIGHT);
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        modal_area,
    );
    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let width = inner_area.width as usize;
    let range = rustcode::controller::SCROLL_LINES_RANGE;
    let max = usize::from(*range.end());
    let selected = state
        .modal_picker_index()
        .clamp(usize::from(*range.start()), max);
    let current = usize::from(rustcode::controller::effective_scroll_lines(
        state.config().scroll_lines,
    ));

    let title = "Scroll speed";
    let header = Line::from(vec![
        Span::styled(
            title,
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(picker_header_padding(width, title, "esc"))),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);

    let value = format!(
        "{selected} line{} per wheel tick",
        if selected == 1 { "" } else { "s" }
    );
    let badge = if selected == current { "(active)" } else { "" };
    let value_line = Line::from(vec![
        Span::styled("‹ ", Style::default().fg(COLOR_MUTED())),
        Span::styled(
            value.clone(),
            Style::default()
                .fg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ›", Style::default().fg(COLOR_MUTED())),
        Span::raw(" ".repeat(picker_row_padding(width, &format!("‹ {value} ›"), badge))),
        Span::styled(badge, Style::default().fg(COLOR_MUTED())),
    ]);

    // One cell per possible value, so the bar's length is the setting.
    let bar_width = max.min(width);
    let filled = selected * bar_width / max;
    let bar_line = Line::from(vec![
        Span::styled("■".repeat(filled), Style::default().fg(COLOR_PRIMARY())),
        Span::styled(
            "·".repeat(bar_width.saturating_sub(filled)),
            Style::default().fg(COLOR_MUTED()),
        ),
    ]);

    let hint = Line::from(Span::styled(
        "Your terminal sets how many ticks a wheel notch sends (kitty: touch_scroll_multiplier).",
        Style::default().fg(COLOR_MUTED()),
    ));
    let footer = Line::from(vec![
        Span::styled("adjust ", Style::default().fg(COLOR_TEXT())),
        Span::styled("←/→   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("save ", Style::default().fg(COLOR_TEXT())),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("cancel ", Style::default().fg(COLOR_TEXT())),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);
    let lines = vec![
        header,
        Line::default(),
        value_line,
        bar_line,
        Line::default(),
        hint,
        Line::default(),
        footer,
    ];
    f.render_widget(
        // Unwrapped, so a narrow panel clips the hint rather than the keys.
        Paragraph::new(lines).style(Style::default().bg(COLOR_PANEL())),
        inner_area,
    );
}
