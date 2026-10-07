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
    let is_command = single
        && (first.tool_name == "run_command"
            || first.rememberable_prefix.is_some()
            || first.forbidden_prefix.is_some());
    let rememberable_prefix = is_command
        .then_some(first.rememberable_prefix.as_deref())
        .flatten();
    let forbidden_prefix = is_command
        .then_some(first.forbidden_prefix.as_deref())
        .flatten();
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
    if is_command {
        lines.push(Line::from(Span::styled(
            "  Approval controls prompts; OS isolation is separate.",
            Style::default().fg(COLOR_MUTED()),
        )));
    }
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
            if confirmation.tool_name == "run_command"
                || confirmation.rememberable_prefix.is_some()
                || confirmation.forbidden_prefix.is_some()
            {
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
            if state.tool_confirmation_selected() == 1 {
                "› "
            } else {
                "  "
            },
            Style::default()
                .fg(if state.tool_confirmation_selected() == 1 {
                    COLOR_PRIMARY()
                } else {
                    COLOR_MUTED()
                })
                .add_modifier(if state.tool_confirmation_selected() == 1 {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        Span::styled(
            "2. No, cancel this tool call ",
            Style::default().fg(COLOR_TEXT()).add_modifier(
                if state.tool_confirmation_selected() == 1 {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                },
            ),
        ),
        Span::styled("(esc)", Style::default().fg(COLOR_MUTED())),
    ]));
    if rememberable_prefix.is_some() {
        let selected = state.tool_confirmation_selected() == 2;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                Style::default()
                    .fg(if selected {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_MUTED()
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                format!(
                    "3. Always allow plain token prefix `{}`",
                    rememberable_prefix.unwrap_or_default()
                ),
                Style::default().fg(COLOR_TEXT()).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(" (r)", Style::default().fg(COLOR_MUTED())),
        ]));
    }
    if let Some(prefix) = forbidden_prefix.as_deref() {
        let row_index = 2 + usize::from(rememberable_prefix.is_some());
        let selected = state.tool_confirmation_selected() == row_index;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                Style::default()
                    .fg(if selected {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_MUTED()
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                format!(
                    "{}. Always forbid literal tokens `{prefix}…`",
                    row_index + 1
                ),
                Style::default().fg(COLOR_TEXT()).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(" (f)", Style::default().fg(COLOR_MUTED())),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if rememberable_prefix.is_some() && forbidden_prefix.is_some() {
            format!(
                "  Press enter to confirm · r allows this token prefix · f blocks literal tokens · tab to {} auto-confirm",
                if state.auto_confirm() {
                    "disable"
                } else {
                    "enable"
                }
            )
        } else if rememberable_prefix.is_some() {
            format!(
                "  Press enter to confirm · r allows this token prefix · tab to {} auto-confirm",
                if state.auto_confirm() { "disable" } else { "enable" }
            )
        } else if forbidden_prefix.is_some() {
            format!(
                "  Press enter to confirm · f blocks literal tokens · tab to {} auto-confirm",
                if state.auto_confirm() { "disable" } else { "enable" }
            )
        } else {
            format!(
                "  Press enter to confirm · tab to {} auto-confirm",
                if state.auto_confirm() {
                    "disable"
                } else {
                    "enable"
                }
            )
        },
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
        let remember = lines
            .iter()
            .find(|line| {
                line.to_string()
                    .contains("3. Always allow plain token prefix")
            })
            .cloned();
        let forbid = lines
            .iter()
            .find(|line| line.to_string().contains("Always forbid"))
            .cloned();
        let target = lines
            .iter()
            .skip(1)
            .find(|line| {
                let text = line.to_string();
                !text.trim().is_empty()
                    && !text.contains("1. Yes")
                    && !text.contains("2. No")
                    && !text.contains("3. Always allow")
                    && !text.contains("Always forbid")
                    && !text.contains("Press enter")
                    && !text.contains("Approval controls prompts")
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
            && let Some(remember) = remember
        {
            compact.push(remember);
        }
        if content_area.height >= 6
            && let Some(forbid) = forbid
        {
            compact.push(forbid);
        }
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

/// Interactive `ask_question` modal: renders the question and its options, with
/// the highlighted option (and, for multi-select, ticked options) emphasized.
pub(in crate::ui) fn question_height(state: &RenderSnapshot, width: u16, available: u16) -> u16 {
    let pending = state.pending_question();
    let Some(question) = pending else {
        return 3;
    };
    let chain_len = state.pending_question_chain_len();
    let header = super::question::question_modal_header(
        &question,
        chain_len,
        state.pending_question_chain_position(),
        chain_len.saturating_sub(state.pending_question_chain_answered()),
    );
    let (body, _, _) = super::question::question_modal_lines(&question, &header, width as usize);
    let footer_rows = super::question::question_modal_footer_lines(
        &question,
        chain_len > 1,
        width as usize,
    )
    .len() as u16;
    // Two rows are the panel's vertical inset; keep one extra trailing row so
    // short questions retain the existing breathing room beneath the hint.
    let height = body.len() as u16 + footer_rows + 3;
    height.min(available.max(1))
}
