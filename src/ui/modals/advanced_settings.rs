use super::*;

pub(in crate::ui) fn render_thinking_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 10);
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
            Constraint::Min(3),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Model thinking";
    let right_esc = "esc";
    let padding_header =
        (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let current = state
        .config()
        .models
        .iter()
        .find(|prof| prof.url == state.api_base_url())
        .and_then(|prof| prof.enable_thinking);

    let choices = [
        ("On", Some(true), "Force <think> reasoning on"),
        (
            "Off",
            Some(false),
            "Skip <think> entirely (faster, no trace)",
        ),
        ("Default", None, "Leave it to the server/Modelfile"),
    ];

    let selected_idx = state
        .modal_picker_index()
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, val, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = current == *val;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!(" ● {}", name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + full_desc.width());
            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default()
                        .fg(COLOR_BG())
                        .bg(COLOR_PRIMARY())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ".repeat(padding_len),
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
                Span::styled(
                    full_desc,
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let left_text = format!("   {}", name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + full_desc.width());
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

pub(in crate::ui) fn render_effort_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 11);
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
            Constraint::Min(4),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Reasoning effort";
    let right_esc = "esc";
    let padding_header =
        (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let current = state
        .config()
        .models
        .iter()
        .find(|prof| prof.url == state.api_base_url())
        .and_then(|prof| prof.reasoning_effort.as_deref());

    let choices = [
        ("Low", Some("low"), "Compact reasoning traces (fastest)"),
        ("Medium", Some("medium"), "Balanced reasoning depth"),
        ("High", Some("high"), "Deep reasoning analysis"),
        ("Off", None, "Clear reasoning effort parameter"),
    ];

    let selected_idx = state
        .modal_picker_index()
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, val, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = current == *val;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!(" ● {}", name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + full_desc.width());
            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default()
                        .fg(COLOR_BG())
                        .bg(COLOR_PRIMARY())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ".repeat(padding_len),
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
                Span::styled(
                    full_desc,
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let left_text = format!("   {}", name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + full_desc.width());
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

pub(in crate::ui) fn render_protocol_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 10);
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
            Constraint::Min(3),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Tool protocol";
    let right_esc = "esc";
    let padding_header =
        (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let current = state.active_tool_protocol();

    let choices = [
        (
            "ApiNative",
            crate::config::ToolProtocol::ApiNative,
            "Structured API schema (`tools` field + `tool_calls` output)",
        ),
        (
            "Json",
            crate::config::ToolProtocol::Json,
            "Standard JSON markdown (```tool)",
        ),
        (
            "Native",
            crate::config::ToolProtocol::Native,
            "Bracketed format ([TOOL_CALLS])",
        ),
    ];

    let selected_idx = state
        .modal_picker_index()
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, val, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = current == *val;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!(" ● {}", name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + full_desc.width());
            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default()
                        .fg(COLOR_BG())
                        .bg(COLOR_PRIMARY())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ".repeat(padding_len),
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
                Span::styled(
                    full_desc,
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let left_text = format!("   {}", name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + full_desc.width());
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

/// Render the startup Homebrew update prompt directly above the chat input.
pub(in crate::ui) fn render_update_prompt_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 14);
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
            Constraint::Length(1), // Versions
            Constraint::Length(1), // Command
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // Options
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title = "RustCode update available";
    let header = Line::from(vec![
        Span::styled(
            title,
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  esc to skip", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(header).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[0],
    );

    let latest = match state.update_check() {
        crate::update::UpdateState::Available(latest) => latest,
        _ => crate::update::current_version(),
    };
    let versions = format!(
        "v{} → v{}",
        crate::update::format_version(crate::update::current_version()),
        crate::update::format_version(latest)
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("New version: ", Style::default().fg(COLOR_MUTED())),
            Span::styled(versions, Style::default().fg(COLOR_TEXT())),
        ]))
        .style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[1],
    );

    let (command_label, command_text, update_action_desc) = if crate::update::is_brew_install() {
        (
            "Method: ",
            crate::update::BREW_UPGRADE_COMMAND,
            "run Homebrew and restart rustcode",
        )
    } else {
        (
            "Method: ",
            "GitHub Releases (in-place update)",
            "download update and restart rustcode",
        )
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(command_label, Style::default().fg(COLOR_MUTED())),
            Span::styled(command_text, Style::default().fg(COLOR_PRIMARY())),
        ]))
        .style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let options = [
        ("Update now", update_action_desc),
        ("Skip", "do not update this time"),
        ("Skip until next version", "hide this version for this run"),
    ];
    let selected = state.update_prompt_index().min(options.len() - 1);
    let option_lines = options
        .iter()
        .enumerate()
        .map(|(index, (label, description))| {
            let selected = index == selected;
            let prefix = if selected { " ● " } else { "   " };
            let left = format!("{prefix}{label}");
            let padding =
                (inner_area.width as usize).saturating_sub(left.width() + description.width());
            if selected {
                Line::from(vec![
                    Span::styled(
                        left,
                        Style::default()
                            .fg(COLOR_BG())
                            .bg(COLOR_PRIMARY())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " ".repeat(padding),
                        Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                    ),
                    Span::styled(
                        *description,
                        Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(left, Style::default().fg(COLOR_TEXT())),
                    Span::styled(" ".repeat(padding), Style::default()),
                    Span::styled(*description, Style::default().fg(COLOR_MUTED())),
                ])
            }
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(option_lines).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[4],
    );

    let footer = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT())),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("skip ", Style::default().fg(COLOR_TEXT())),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(footer).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[5],
    );
}
