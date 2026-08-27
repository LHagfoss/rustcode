use super::*;

/// Render the model picker directly above the chat input.
pub(in crate::ui) fn render_model_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let filtered_items = get_filtered_picker_items(state);

    let selected_idx = state
        .model_picker_index()
        .min(filtered_items.len().saturating_sub(1));

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
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let search_part = if state.model_picker_search().is_empty() {
        "".to_owned()
    } else {
        format!(" · {}", state.model_picker_search())
    };
    let title_text = format!("Select model{search_part}");
    let right_esc = if state.model_picker_search().is_empty() {
        "type to search  esc"
    } else {
        "esc"
    };
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

    let mut list_lines = Vec::new();
    for (idx, item) in filtered_items.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let max_name_width = (inner_area.width as usize).saturating_sub(item.desc.width() + 5);
        let name_display = truncate_middle_to_width(&item.name, max_name_width);
        let line = if is_selected {
            let left_text = format!(" ● {}", name_display);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + item.desc.width());
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
                    item.desc.clone(),
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let left_text = format!("   {}", name_display);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + item.desc.width());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(item.desc.clone(), Style::default().fg(COLOR_MUTED())),
            ])
        };
        list_lines.push(line);
    }

    let list_height = modal_chunks[2].height as usize;
    let total_lines = list_lines.len();
    let scroll_y: u16 = if total_lines <= list_height {
        0
    } else {
        let ideal = selected_idx.saturating_sub(list_height / 3);
        let lo = selected_idx.saturating_sub(list_height.saturating_sub(1));
        let hi = selected_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(list_paragraph, modal_chunks[2]);

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

/// Render the session history picker modal overlay (/history).
pub(in crate::ui) fn render_history_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    // Confirmation overlay for delete (Ctrl+D)
    if let Some(del_idx) = state.pending_delete_session_idx() {
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
                Constraint::Length(1), // Session title line
                Constraint::Length(1), // Session info line
                Constraint::Min(1),    // Spacer
                Constraint::Length(1), // Footer buttons
            ])
            .split(inner_area);

        let header_line = Line::from(vec![Span::styled(
            "⚠ Delete session?",
            Style::default()
                .fg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(
            Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[0],
        );

        if let Some(meta) = state.history_picker_sessions().get(del_idx) {
            let title_line = Line::from(vec![
                Span::styled("  session  ", Style::default().fg(COLOR_MUTED())),
                Span::styled(
                    &meta.title,
                    Style::default()
                        .fg(COLOR_TEXT())
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            f.render_widget(
                Paragraph::new(title_line).style(Style::default().bg(COLOR_PANEL())),
                modal_chunks[2],
            );

            let info_line = Line::from(vec![
                Span::styled("  info     ", Style::default().fg(COLOR_MUTED())),
                Span::styled(
                    format!("{} messages  ({})", meta.message_count, meta.when),
                    Style::default().fg(COLOR_MUTED()),
                ),
            ]);
            f.render_widget(
                Paragraph::new(info_line).style(Style::default().bg(COLOR_PANEL())),
                modal_chunks[3],
            );
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "  y / enter",
                Style::default()
                    .fg(COLOR_GREEN())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" delete  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "n / esc",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(
            Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[5],
        );

        return;
    }

    let sessions = &state.history_picker_sessions();
    let selected_idx = state
        .history_picker_index()
        .min(sessions.len().saturating_sub(1));

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
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Resume session";
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

    let mut list_lines = Vec::new();
    for (idx, session) in sessions.iter().enumerate() {
        let desc = format!("{} msgs  {}", session.message_count, session.when);
        let is_selected = selected_idx == idx;
        let max_title_width = (inner_area.width as usize).saturating_sub(desc.width() + 5);
        let title_display = truncate_middle_to_width(&session.title, max_title_width);
        let line = if is_selected {
            let left_text = format!(" ● {}", title_display);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + desc.width());
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
                Span::styled(desc, Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY())),
            ])
        } else {
            let left_text = format!("   {}", title_display);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + desc.width());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(desc, Style::default().fg(COLOR_MUTED())),
            ])
        };
        list_lines.push(line);
    }

    let list_height = modal_chunks[2].height as usize;
    let total_lines = list_lines.len();
    let scroll_y: u16 = if total_lines <= list_height {
        0
    } else {
        let ideal = selected_idx.saturating_sub(list_height / 3);
        let lo = selected_idx.saturating_sub(list_height - 1);
        let hi = selected_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(list_paragraph, modal_chunks[2]);

    let mut footer_spans = vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT())),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("delete ", Style::default().fg(COLOR_TEXT())),
        Span::styled("ctrl+d", Style::default().fg(COLOR_MUTED())),
    ];
    if state.history_picker_truncated() {
        footer_spans.push(Span::styled(
            "   (Truncated to 50 sessions. Use /delete_chat to clean up.)",
            Style::default()
                .fg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD),
        ));
    }
    let footer_line = Line::from(footer_spans);
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[3],
    );
}

/// Render the navigable subagent context picker. The selected context keeps
/// its own transcript in state; this surface makes that history visible before
/// the user switches the active view.
pub(in crate::ui) fn render_subagent_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let total = state.subagents().len() + 1;
    let selected = state.subagent_picker_index().min(total.saturating_sub(1));
    let modal_area = input_anchor_rect(f, input_area, 18);
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
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);
    let title_text = "Agent contexts";
    let right_esc = "esc";
    let padding_header =
        (inner.width as usize).saturating_sub(title_text.width() + right_esc.width());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                title_text,
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ".repeat(padding_header), Style::default()),
            Span::styled(right_esc, Style::default().fg(COLOR_MUTED())),
        ]))
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Select a conversation context; parent history is preserved",
            Style::default().fg(COLOR_MUTED()),
        )))
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[1],
    );

    let list_height = chunks[2].height as usize;
    let mut lines = Vec::with_capacity(total);
    let root_selected = selected == 0;
    lines.push(agent_picker_line(
        root_selected,
        "main",
        "root conversation",
        state.selected_subagent_id().is_none(),
        inner.width as usize,
    ));
    for (index, agent) in state.subagents().iter().enumerate() {
        let is_selected = selected == index + 1;
        let status = match agent.status {
            crate::app::SubAgentStatus::Running => "running",
            crate::app::SubAgentStatus::Completed => "completed",
            crate::app::SubAgentStatus::Failed => "failed",
            crate::app::SubAgentStatus::Cancelled => "cancelled",
        };
        let task = agent.task.chars().take(32).collect::<String>();
        lines.push(agent_picker_line(
            is_selected,
            &agent.name,
            &format!("{status} · {task}"),
            state.selected_subagent_id() == Some(agent.id),
            inner.width as usize,
        ));
    }

    let offset = if selected >= list_height {
        selected + 1 - list_height
    } else {
        0
    };
    f.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(offset)
                .take(list_height)
                .collect::<Vec<_>>(),
        )
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[2],
    );

    let detail = if selected == 0 {
        "main · root context".to_owned()
    } else if let Some(agent) = state.subagents().get(selected - 1) {
        let status = match agent.status {
            crate::app::SubAgentStatus::Running => "running",
            crate::app::SubAgentStatus::Completed => "completed",
            crate::app::SubAgentStatus::Failed => "failed",
            crate::app::SubAgentStatus::Cancelled => "cancelled",
        };
        format!("{} · {} · {}", agent.name, status, agent.last_message())
    } else {
        "No subagent contexts".to_owned()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            detail,
            Style::default().fg(COLOR_MUTED()),
        )))
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[3],
    );
}

pub(super) fn agent_picker_line(
    selected: bool,
    name: &str,
    detail: &str,
    active: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "› " } else { "  " };
    let active_marker = if active { "●" } else { "○" };
    let text = format!("{marker}{active_marker} {name} · {detail}");
    let text = text.chars().take(width).collect::<String>();
    let style = if selected {
        Style::default()
            .fg(COLOR_BG())
            .bg(COLOR_PRIMARY())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_TEXT())
    };
    Line::from(Span::styled(text, style))
}

pub(in crate::ui) fn render_mcp_config_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let servers = &state.config().mcp_servers;
    let selected_idx = state.mcp_picker_index();

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
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // Content area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    if let Some(ref edit_state) = state.mcp_edit_state() {
        // --- ADD / EDIT MODE ---
        let title = if edit_state.is_add {
            "Add MCP Server"
        } else {
            "Edit MCP Server"
        };
        let header_line = Line::from(vec![Span::styled(
            title,
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(
            Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[0],
        );

        // Draw 3 input fields
        let form_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Name input
                Constraint::Length(3), // Command input
                Constraint::Length(3), // Args input
            ])
            .split(modal_chunks[2]);

        for field_idx in 0..3 {
            let label = match field_idx {
                0 => "Server Name",
                1 => "Executable Command",
                _ => "Arguments (space-separated)",
            };
            let val = match field_idx {
                0 => &edit_state.name_input,
                1 => &edit_state.command_input,
                _ => &edit_state.args_input,
            };

            let is_active = edit_state.active_field == field_idx;
            let display_val = if is_active {
                let pos = edit_state.cursor_pos.min(val.len());
                let (left, right) = val.split_at(pos);
                format!("{left}█{right}")
            } else {
                val.clone()
            };

            let border_style = if is_active {
                Style::default().fg(COLOR_TEXT())
            } else {
                Style::default().fg(COLOR_MUTED())
            };

            f.render_widget(
                Paragraph::new(display_val)
                    .style(Style::default().bg(COLOR_PANEL()))
                    .block(
                        Block::default()
                            .title(Span::styled(label, Style::default().fg(COLOR_MUTED())))
                            .borders(Borders::ALL)
                            .border_style(border_style)
                            .style(Style::default().bg(COLOR_PANEL())),
                    ),
                form_chunks[field_idx],
            );
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "enter",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Save    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "esc",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Cancel    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "tab / arrows",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Switch Field", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(
            Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[3],
        );
    } else {
        // --- LIST MODE ---
        let title_text = "MCP Servers Configuration";
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

        let mut list_lines = Vec::new();
        for (idx, srv) in servers.iter().enumerate() {
            let is_selected = selected_idx == idx;
            let status = if srv.enabled { "Enabled" } else { "Disabled" };
            let status_style = if srv.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(COLOR_MUTED())
            };

            let cmd_text = format!("{} {}", srv.command, srv.args.join(" "));

            let line = if is_selected {
                let left_text = format!(" ● {}", srv.name);
                let right_text = format!(" [{}] {}", status, cmd_text);
                let padding_len = (inner_area.width as usize)
                    .saturating_sub(left_text.width() + right_text.width());

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
                    Span::styled(format!(" [{}]", status), status_style.bg(COLOR_PRIMARY())),
                    Span::styled(
                        format!(" {}", cmd_text),
                        Style::default()
                            .fg(COLOR_BG())
                            .bg(COLOR_PRIMARY())
                            .add_modifier(Modifier::ITALIC),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        format!("{:<20}", srv.name),
                        Style::default().fg(COLOR_MUTED()),
                    ),
                    Span::styled(" [", Style::default().fg(COLOR_MUTED())),
                    Span::styled(status, status_style),
                    Span::styled("] ", Style::default().fg(COLOR_MUTED())),
                    Span::styled(cmd_text, Style::default().fg(COLOR_MUTED())),
                ])
            };
            list_lines.push(line);
        }

        if list_lines.is_empty() {
            f.render_widget(
                Paragraph::new("No MCP servers configured.\nPress 'a' to add a new server.")
                    .style(Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL())),
                modal_chunks[2],
            );
        } else {
            f.render_widget(
                Paragraph::new(list_lines).style(Style::default().bg(COLOR_PANEL())),
                modal_chunks[2],
            );
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "a",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Add    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "e",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Edit    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "d",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Delete    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "enter",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Toggle Enabled", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(
            Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[3],
        );
    }
}

#[derive(Clone)]
pub struct PaletteItem {
    pub group: &'static str,
    pub name: &'static str,
    pub shortcut: &'static str,
}

pub const PALETTE_ITEMS: &[PaletteItem] = &[
    PaletteItem {
        group: "Session",
        name: "New session",
        shortcut: "/new",
    },
    PaletteItem {
        group: "Session",
        name: "Fork session",
        shortcut: "/fork",
    },
    PaletteItem {
        group: "Session",
        name: "Archive session",
        shortcut: "/archive",
    },
    PaletteItem {
        group: "Session",
        name: "Resume session",
        shortcut: "/resume",
    },
    PaletteItem {
        group: "Session",
        name: "Copy last reply",
        shortcut: "/copy",
    },
    PaletteItem {
        group: "Agent",
        name: "Browse agent contexts",
        shortcut: "/agents",
    },
    PaletteItem {
        group: "Agent",
        name: "Switch model",
        shortcut: "/model",
    },
    PaletteItem {
        group: "Agent",
        name: "Show context usage",
        shortcut: "/context",
    },
    PaletteItem {
        group: "Agent",
        name: "Set parser/tool protocol",
        shortcut: "/parser",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure provider profile",
        shortcut: "/provider",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure Ollama models",
        shortcut: "/ollama",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure MCP servers",
        shortcut: "/mcp",
    },
    PaletteItem {
        group: "Agent",
        name: "Set automatic tool confirmation",
        shortcut: "/yolo",
    },
    PaletteItem {
        group: "Session",
        name: "Change session title",
        shortcut: "/change_title",
    },
    PaletteItem {
        group: "Session",
        name: "Clear conversation",
        shortcut: "/clear",
    },
    PaletteItem {
        group: "Session",
        name: "Cancel active stream",
        shortcut: "/cancel",
    },
    PaletteItem {
        group: "System",
        name: "About rustcode",
        shortcut: "/about",
    },
    PaletteItem {
        group: "System",
        name: "Show info",
        shortcut: "/info",
    },
    PaletteItem {
        group: "System",
        name: "Show help",
        shortcut: "/help",
    },
    PaletteItem {
        group: "System",
        name: "Show token usage stats",
        shortcut: "/stats",
    },
    PaletteItem {
        group: "System",
        name: "Show token usage (alias)",
        shortcut: "/usage",
    },
    PaletteItem {
        group: "System",
        name: "Show RAM usage",
        shortcut: "/memory",
    },
    PaletteItem {
        group: "System",
        name: "List available tools",
        shortcut: "/tools",
    },
    PaletteItem {
        group: "System",
        name: "Exit the app",
        shortcut: "ctrl+c",
    },
    PaletteItem {
        group: "System",
        name: "Quit the app",
        shortcut: "/quit",
    },
];

pub(in crate::ui) fn render_command_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let search = state.command_picker_search().to_lowercase();
    let filtered_items: Vec<&PaletteItem> = PALETTE_ITEMS
        .iter()
        .filter(|item| {
            item.name.to_lowercase().contains(&search)
                || item.group.to_lowercase().contains(&search)
        })
        .collect();

    let selected_idx = state
        .command_picker_index()
        .min(filtered_items.len().saturating_sub(1));

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
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let search_part = if state.command_picker_search().is_empty() {
        "".to_owned()
    } else {
        format!(" · {}", state.command_picker_search())
    };
    let title_text = format!("Commands{search_part}");
    let right_esc = if state.command_picker_search().is_empty() {
        "type to search  esc"
    } else {
        "esc"
    };
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

    let mut list_lines = Vec::new();
    for (idx, item) in filtered_items.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let line = if is_selected {
            let left_text = format!(" ● {}", item.name);
            let padding_len = (inner_area.width as usize)
                .saturating_sub(left_text.width() + item.shortcut.width());
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
                    item.shortcut.to_string(),
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let left_text = format!("   {}", item.name);
            let padding_len = (inner_area.width as usize)
                .saturating_sub(left_text.width() + item.shortcut.width());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(
                    item.shortcut.to_string(),
                    Style::default().fg(COLOR_MUTED()),
                ),
            ])
        };
        list_lines.push(line);
    }

    let list_height = modal_chunks[2].height as usize;
    let total_lines = list_lines.len();
    let scroll_y: u16 = if total_lines <= list_height {
        0
    } else {
        let ideal = selected_idx.saturating_sub(list_height / 3);
        let lo = selected_idx.saturating_sub(list_height.saturating_sub(1));
        let hi = selected_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(list_paragraph, modal_chunks[2]);

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

pub(in crate::ui) fn tool_confirmation_height(state: &RenderSnapshot, available: u16) -> u16 {
    let pending = state.pending_tool_confirmation();
    let Some(confirmations) = pending else {
        return 3;
    };
    let preview = confirmations
        .first()
        .map(|confirmation| confirmation.content_preview.lines().count() as u16)
        .unwrap_or(0)
        .min(8);
    let content = if confirmations.len() > 1 {
        7u16.saturating_add(confirmations.len().min(8) as u16)
    } else {
        9u16.saturating_add(preview)
    };
    content.saturating_add(2).min(available.max(3))
}
