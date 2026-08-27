use super::*;

/// Bottom-pane approval view matching Codex's interaction layout. The
/// execution/confirmation channel remains RustCode's; this function only owns
/// presentation and keeps the normal composer hidden while a decision is due.
pub(in crate::ui) fn render_tool_confirmation_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    area: ratatui::layout::Rect,
) {
    let pending = state.pending_tool_confirmation();
    let confirmations = match pending {
        Some(confirmations) if !confirmations.is_empty() => confirmations,
        _ => return,
    };
    let panel = crate::ui::theme::get_palette(&state.config().theme).panel;
    let content_area = render_padded_panel_with_color(f, area, panel);

    let mut lines = Vec::new();
    let single = confirmations.len() == 1;
    let first = &confirmations[0];
    let is_command = single && first.tool_name == "run_command";
    let heading = if is_command {
        "Would you like to run the following command?".to_owned()
    } else if single {
        "Would you like to make the following change?".to_owned()
    } else {
        format!(
            "Would you like to approve these {} tool calls?",
            confirmations.len()
        )
    };
    lines.push(Line::from(Span::styled(
        format!("  {heading}"),
        Style::default()
            .fg(COLOR_TEXT())
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    if single {
        if is_command {
            let command_width = content_area.width.saturating_sub(4) as usize;
            let command = truncate_middle_to_width(&first.path, command_width);
            for (index, command) in highlight_shell_command(&command, panel, false)
                .into_iter()
                .enumerate()
            {
                let mut spans = vec![Span::styled(
                    if index == 0 { "  $ " } else { "    " },
                    Style::default().fg(COLOR_TEXT()).bg(panel),
                )];
                spans.extend(command.spans);
                lines.push(Line::from(spans));
            }
        } else {
            let prefix_width = 2 + first.tool_name.width() + 1;
            let path = truncate_middle_to_width(
                &first.path,
                (content_area.width as usize).saturating_sub(prefix_width),
            );
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    first.tool_name.clone(),
                    Style::default().fg(COLOR_SECONDARY()),
                ),
                Span::raw(" "),
                Span::styled(path, Style::default().fg(COLOR_TEXT())),
            ]));
        }
        for source in first.content_preview.lines().take(8) {
            let source =
                truncate_middle_to_width(source, content_area.width.saturating_sub(4) as usize);
            let mut line = highlight_diff_line(
                &source,
                content_area.width.saturating_sub(4) as usize,
                false,
            );
            line.spans.insert(0, Span::raw("    "));
            lines.push(line);
        }
    } else {
        for confirmation in confirmations.iter().take(8) {
            let mut spans = vec![
                Span::raw("  • "),
                Span::styled(
                    confirmation.tool_name.clone(),
                    Style::default().fg(COLOR_SECONDARY()),
                ),
                Span::raw(" "),
            ];
            if confirmation.tool_name == "run_command" {
                spans.push(Span::styled(
                    "$ ",
                    Style::default().fg(COLOR_TEXT()).bg(panel),
                ));
                let prefix_width = spans.iter().map(|span| span.content.width()).sum::<usize>();
                let command = truncate_middle_to_width(
                    &confirmation.path,
                    (content_area.width as usize).saturating_sub(prefix_width),
                );
                if let Some(command) = highlight_shell_command(&command, panel, false)
                    .into_iter()
                    .next()
                {
                    spans.extend(command.spans);
                }
            } else {
                let prefix_width = spans.iter().map(|span| span.content.width()).sum::<usize>();
                spans.push(Span::styled(
                    truncate_middle_to_width(
                        &confirmation.path,
                        (content_area.width as usize).saturating_sub(prefix_width),
                    ),
                    Style::default().fg(COLOR_TEXT()),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));
    let approve_selected = state.tool_confirmation_selected() == 0;
    lines.push(Line::from(vec![
        Span::styled(
            if approve_selected { "› " } else { "  " },
            Style::default()
                .fg(if approve_selected {
                    COLOR_PRIMARY()
                } else {
                    COLOR_MUTED()
                })
                .add_modifier(if approve_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        Span::styled(
            "1. Yes, proceed",
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(if approve_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        Span::styled(" (y)", Style::default().fg(COLOR_MUTED())),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            if approve_selected { "  " } else { "› " },
            Style::default()
                .fg(if approve_selected {
                    COLOR_MUTED()
                } else {
                    COLOR_PRIMARY()
                })
                .add_modifier(if approve_selected {
                    Modifier::empty()
                } else {
                    Modifier::BOLD
                }),
        ),
        Span::styled(
            "2. No, cancel this tool call ",
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(if approve_selected {
                    Modifier::empty()
                } else {
                    Modifier::BOLD
                }),
        ),
        Span::styled("(esc)", Style::default().fg(COLOR_MUTED())),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "  Press enter to confirm · tab to {} auto-confirm",
            if state.auto_confirm() {
                "disable"
            } else {
                "enable"
            }
        ),
        Style::default().fg(COLOR_MUTED()),
    )));

    if lines.len() > content_area.height as usize {
        let heading = lines.first().cloned().unwrap_or_default();
        let approve = lines
            .iter()
            .find(|line| line.to_string().contains("1. Yes, proceed"))
            .cloned()
            .unwrap_or_default();
        let cancel = lines
            .iter()
            .find(|line| line.to_string().contains("2. No, cancel"))
            .cloned()
            .unwrap_or_default();
        let target = lines
            .iter()
            .skip(1)
            .find(|line| {
                let text = line.to_string();
                !text.trim().is_empty()
                    && !text.contains("1. Yes")
                    && !text.contains("2. No")
                    && !text.contains("Press enter")
            })
            .cloned();
        let footer = lines.last().cloned();
        let mut compact = vec![heading];
        if content_area.height >= 4
            && let Some(target) = target
        {
            compact.push(target);
        }
        compact.push(approve);
        compact.push(cancel);
        if content_area.height >= 5
            && let Some(footer) = footer
        {
            compact.push(footer);
        }
        lines = compact;
    }
    paint_panel_line_backgrounds(&mut lines, panel);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(panel)),
        content_area,
    );
}

#[allow(dead_code)]
pub(super) fn render_tool_confirmation_modal_legacy(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let pending = state.pending_tool_confirmation();
    let confirmations = match pending {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    let screen_height = f.area().height;

    if confirmations.len() == 1 {
        let confirmation = &confirmations[0];
        let has_preview = !confirmation.content_preview.trim().is_empty();
        let preview_lines = confirmation.content_preview.lines().count();
        let height = if has_preview {
            ((preview_lines as u16) + 8)
                .max(9)
                .min((screen_height.saturating_sub(4)).min(22))
        } else {
            7
        };
        let modal_area = input_anchor_rect(f, input_area, height);

        f.render_widget(Clear, modal_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COLOR_TIP()))
            .style(Style::default().bg(COLOR_BG()));
        f.render_widget(modal_block, modal_area);

        let inner_area = modal_area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });

        let compact = inner_area.height < 5;
        let very_compact = inner_area.height < 3;
        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if very_compact {
                [
                    Constraint::Length(1), // 0: Combined header and target
                    Constraint::Length(0), // 1: Spacer
                    Constraint::Length(0), // 2: Tool & target line
                    Constraint::Length(0), // 3: Auto-confirm status
                    Constraint::Length(0), // 4: Spacer
                    Constraint::Length(0), // 5: Preview Diff / Content
                    Constraint::Length(0), // 6: Spacer
                    Constraint::Length(1), // 7: Footer buttons
                ]
            } else if compact {
                [
                    Constraint::Length(1), // 0: Header
                    Constraint::Length(0), // 1: Spacer
                    Constraint::Length(1), // 2: Tool & target line
                    Constraint::Length(0), // 3: Auto-confirm status
                    Constraint::Length(0), // 4: Spacer
                    Constraint::Length(0), // 5: Preview Diff / Content
                    Constraint::Length(0), // 6: Spacer
                    Constraint::Length(1), // 7: Footer buttons
                ]
            } else {
                [
                    Constraint::Length(1),                               // 0: Header
                    Constraint::Length(1),                               // 1: Spacer
                    Constraint::Length(1),                               // 2: Tool & target line
                    Constraint::Length(1),                               // 3: Auto-confirm status
                    Constraint::Length(1),                               // 4: Spacer
                    Constraint::Min(if has_preview { 2 } else { 0 }), // 5: Preview Diff / Content
                    Constraint::Length(if has_preview { 1 } else { 0 }), // 6: Spacer
                    Constraint::Length(1),                            // 7: Footer buttons
                ]
            })
            .split(inner_area);

        let action_label = match confirmation.tool_name.as_str() {
            "write_to_file" => "Write to file",
            "replace_file_content" => "Replace file content",
            "multi_replace_file_content" => "Apply multi-replace",
            "create_file" => "Create file",
            "write_file" => "Overwrite file",
            "delete_file" => "Delete file",
            "move_file" => "Move file",
            "copy_file" => "Copy file",
            "run_command" => "Run command",
            _ => "Execute tool",
        };
        let header_text = if confirmation.tool_name == "run_command" {
            "⚠ Would you like to run the following command?".to_owned()
        } else {
            format!("⚠ {action_label}?")
        };
        let header_line = Line::from(vec![Span::styled(
            header_text,
            Style::default()
                .fg(COLOR_TIP())
                .add_modifier(Modifier::BOLD),
        )]);
        let path_display = if confirmation.path.len() > inner_area.width as usize - 22 {
            let cut = (inner_area.width as usize).saturating_sub(25).max(5);
            format!(
                "…{}",
                &confirmation.path[confirmation.path.len().saturating_sub(cut)..]
            )
        } else {
            confirmation.path.clone()
        };

        let size_str = if confirmation.tool_name != "run_command" && confirmation.content_bytes > 0
        {
            format!(" ({} bytes)", confirmation.content_bytes)
        } else {
            String::new()
        };

        let command_prefix = (confirmation.tool_name == "run_command").then_some("$ ");
        let tool_line = Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{:<15}", action_label),
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}{}", command_prefix.unwrap_or(""), path_display),
                Style::default().fg(COLOR_PRIMARY()),
            ),
            Span::styled(size_str, Style::default().fg(COLOR_MUTED())),
        ]);
        if very_compact {
            let mut compact_spans = header_line.spans;
            compact_spans.push(Span::raw(" "));
            compact_spans.extend(tool_line.spans);
            f.render_widget(Paragraph::new(Line::from(compact_spans)), modal_chunks[0]);
        } else {
            f.render_widget(Paragraph::new(header_line), modal_chunks[0]);
            f.render_widget(Paragraph::new(tool_line), modal_chunks[2]);
        }

        let auto_confirm_status = if state.auto_confirm() {
            "[x] Auto-confirm future tool calls"
        } else {
            "[ ] Auto-confirm future tool calls"
        };
        let auto_confirm_line = Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                auto_confirm_status,
                Style::default()
                    .fg(COLOR_TIP())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" (Tab to toggle)", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(auto_confirm_line), modal_chunks[3]);

        if !confirmation.content_preview.is_empty() {
            let diff_height = modal_chunks[5].height as usize;
            let scroll = state.modal_scroll_row() as usize;

            let has_null = confirmation.content_preview.contains('\x00');
            if has_null {
                let diff_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(50),
                        Constraint::Length(1), // Divider
                        Constraint::Percentage(50),
                    ])
                    .split(modal_chunks[5]);

                let mut left_lines = Vec::new();
                let mut right_lines = Vec::new();

                let half_width = (diff_chunks[0].width as usize).saturating_sub(2);

                for line in confirmation
                    .content_preview
                    .lines()
                    .skip(scroll)
                    .take(diff_height)
                {
                    let parts: Vec<&str> = line.split('\x00').collect();
                    if parts.len() == 2 {
                        left_lines.push(highlight_diff_line(parts[0], half_width, false));
                        right_lines.push(highlight_diff_line(parts[1], half_width, false));
                    } else {
                        left_lines.push(highlight_diff_line(line, half_width, false));
                        right_lines.push(Line::from(""));
                    }
                }

                f.render_widget(
                    Paragraph::new(left_lines).wrap(Wrap { trim: false }),
                    diff_chunks[0],
                );

                let divider_lines = vec![Line::from("│"); diff_chunks[1].height as usize];
                f.render_widget(
                    Paragraph::new(divider_lines).style(Style::default().fg(COLOR_MUTED())),
                    diff_chunks[1],
                );

                f.render_widget(
                    Paragraph::new(right_lines).wrap(Wrap { trim: false }),
                    diff_chunks[2],
                );
            } else {
                let preview_lines: Vec<Line> = confirmation
                    .content_preview
                    .lines()
                    .skip(scroll)
                    .take(diff_height)
                    .map(|l| {
                        let width = (inner_area.width as usize).saturating_sub(4);
                        highlight_diff_line(l, width, false)
                    })
                    .collect();
                f.render_widget(
                    Paragraph::new(preview_lines).wrap(Wrap { trim: false }),
                    modal_chunks[5],
                );
            }
        }

        let total_lines = confirmation.content_preview.lines().count();
        let scroll_info = if modal_chunks.len() > 5
            && modal_chunks[5].height > 0
            && total_lines > modal_chunks[5].height as usize
        {
            format!(
                "  ↑/↓ scroll ({}/{})",
                state.modal_scroll_row() + 1,
                total_lines
            )
        } else {
            String::new()
        };

        let footer_line = Line::from(vec![
            Span::styled(
                "  y / enter",
                Style::default()
                    .fg(COLOR_GREEN())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" approve  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "n / esc",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" deny  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "tab",
                Style::default()
                    .fg(COLOR_TIP())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" toggle auto-confirm", Style::default().fg(COLOR_MUTED())),
            Span::styled(scroll_info, Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[7]);
    } else {
        // Render batch confirmation modal
        let height = (confirmations.len() as u16 + 7)
            .max(8)
            .min((screen_height.saturating_sub(4)).min(22));
        let modal_area = input_anchor_rect(f, input_area, height);
        f.render_widget(Clear, modal_area);
        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COLOR_TIP()))
            .style(Style::default().bg(COLOR_BG()));
        f.render_widget(modal_block, modal_area);

        let inner_area = modal_area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });

        let compact = inner_area.height < (confirmations.len() as u16).saturating_add(5);
        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if compact {
                [
                    Constraint::Length(1), // Header
                    Constraint::Length(0), // Spacer
                    Constraint::Min(0),    // Truncated list of tools
                    Constraint::Length(0), // Auto-confirm option
                    Constraint::Length(0), // Spacer
                    Constraint::Length(1), // Footer/Actions
                ]
            } else {
                [
                    Constraint::Length(1),                       // Header
                    Constraint::Length(1),                       // Spacer
                    Constraint::Min(confirmations.len() as u16), // List of tools
                    Constraint::Length(1),                       // Auto-confirm option
                    Constraint::Length(1),                       // Spacer
                    Constraint::Length(1),                       // Footer/Actions
                ]
            })
            .split(inner_area);

        let header_line = Line::from(vec![Span::styled(
            format!("⚠ Approve {} tool calls in parallel?", confirmations.len()),
            Style::default()
                .fg(COLOR_TIP())
                .add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(Paragraph::new(header_line), modal_chunks[0]);

        let mut tool_lines = Vec::new();
        for (i, c) in confirmations.iter().enumerate() {
            let action = match c.tool_name.as_str() {
                "write_to_file" => "Write to file",
                "replace_file_content" => "Replace file content",
                "multi_replace_file_content" => "Apply multi-replace",
                "create_file" => "Create file",
                "write_file" => "Overwrite file",
                "delete_file" => "Delete file",
                "move_file" => "Move file",
                "copy_file" => "Copy file",
                "run_command" => "Run command",
                _ => "Execute tool",
            };

            let path_display = if c.path.len() > inner_area.width as usize - 25 {
                let cut = (inner_area.width as usize).saturating_sub(28).max(5);
                format!("…{}", &c.path[c.path.len().saturating_sub(cut)..])
            } else {
                c.path.clone()
            };

            let marker = if i == 0 { "›" } else { " " };
            let command_prefix = (c.tool_name == "run_command").then_some("$ ");
            let line = Line::from(vec![
                Span::styled(
                    format!("{} {}. ", marker, i + 1),
                    Style::default().fg(if i == 0 {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_MUTED()
                    }),
                ),
                Span::styled(
                    format!("{:<15}", action),
                    Style::default()
                        .fg(COLOR_TEXT())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}{}", command_prefix.unwrap_or(""), path_display),
                    Style::default().fg(COLOR_PRIMARY()),
                ),
            ]);
            tool_lines.push(line);
        }

        f.render_widget(Paragraph::new(tool_lines), modal_chunks[2]);

        let auto_confirm_status = if state.auto_confirm() {
            "[x] Auto-confirm future tool calls"
        } else {
            "[ ] Auto-confirm future tool calls"
        };
        let auto_confirm_line = Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                auto_confirm_status,
                Style::default()
                    .fg(COLOR_TIP())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" (Tab to toggle)", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(auto_confirm_line), modal_chunks[3]);

        let footer_line = Line::from(vec![
            Span::styled(
                "  y / enter",
                Style::default()
                    .fg(COLOR_GREEN())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to confirm all  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "n / esc",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to cancel all  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "tab",
                Style::default()
                    .fg(COLOR_TIP())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" toggle auto-confirm", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[5]);
    }
}

/// Interactive `ask_question` modal: renders the question and its options, with
/// the highlighted option (and, for multi-select, ticked options) emphasized.
pub(in crate::ui) fn question_height(state: &RenderSnapshot, width: u16, available: u16) -> u16 {
    let pending = state.pending_question();
    let Some(question) = pending else {
        return 3;
    };
    let question_rows =
        textwrap_simple(&question.question, width.saturating_sub(4).max(10) as usize).len() as u16;
    let option_rows = if question.custom_input.is_some() {
        1
    } else {
        question.options.len().saturating_add(1) as u16
    };
    (question_rows + option_rows + 7).min(available.max(3))
}
