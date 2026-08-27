use super::*;

pub(in crate::ui) fn render_question_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    area: ratatui::layout::Rect,
) {
    let pending = state.pending_question();
    let Some(question) = pending else {
        return;
    };
    let panel = crate::ui::theme::get_palette(&state.config().theme).panel;
    let content_area = render_padded_panel_with_color(f, area, panel);
    let mut lines = vec![Line::from(Span::styled(
        "  Question 1/1 (1 unanswered)",
        Style::default()
            .fg(COLOR_TEXT())
            .add_modifier(Modifier::BOLD),
    ))];
    for line in textwrap_simple(
        &question.question,
        content_area.width.saturating_sub(4).max(10) as usize,
    ) {
        lines.push(Line::from(format!("  {line}")));
    }
    lines.push(Line::from(""));

    let custom_row = if let Some(custom) = question.custom_input.as_ref() {
        let row = lines.len() as u16;
        lines.push(Line::from(vec![
            Span::styled(
                "  › ",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if custom.is_empty() {
                    "Type your answer (optional)".to_owned()
                } else {
                    custom.clone()
                },
                Style::default().fg(if custom.is_empty() {
                    COLOR_MUTED()
                } else {
                    COLOR_TEXT()
                }),
            ),
        ]));
        Some(row)
    } else {
        for (index, option) in question.options.iter().enumerate() {
            let selected = question.selected == index;
            let checked = question.chosen.get(index).copied().unwrap_or(false);
            lines.push(Line::from(vec![
                Span::styled(
                    if selected { "  › " } else { "    " },
                    Style::default().fg(if selected {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_TEXT()
                    }),
                ),
                Span::styled(
                    if question.is_multi_select {
                        format!("{} {}. ", if checked { "[x]" } else { "[ ]" }, index + 1)
                    } else {
                        format!("{}. ", index + 1)
                    },
                    Style::default().fg(if selected {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_MUTED()
                    }),
                ),
                Span::styled(
                    option.clone(),
                    Style::default().fg(COLOR_TEXT()).add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ),
            ]));
        }
        let custom_selected = question.selected == question.options.len();
        lines.push(Line::from(vec![
            Span::styled(
                if custom_selected { "  › " } else { "    " },
                Style::default().fg(if custom_selected {
                    COLOR_PRIMARY()
                } else {
                    COLOR_TEXT()
                }),
            ),
            Span::styled(
                "Type your own answer",
                Style::default()
                    .fg(if custom_selected {
                        COLOR_TEXT()
                    } else {
                        COLOR_MUTED()
                    })
                    .add_modifier(if custom_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]));
        None
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if question.custom_input.is_some() {
            "  enter to submit answer | esc to go back"
        } else if question.is_multi_select {
            "  space to toggle | enter to submit answer | esc to interrupt"
        } else {
            "  enter to submit answer | esc to interrupt"
        },
        Style::default().fg(COLOR_MUTED()),
    )));

    lines.truncate(content_area.height as usize);
    paint_panel_line_backgrounds(&mut lines, panel);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(panel)),
        content_area,
    );
    if let (Some(row), Some(custom)) = (custom_row, question.custom_input.as_ref())
        && row < content_area.height
    {
        let cursor = question.custom_cursor.min(custom.len());
        let cursor = custom[..cursor].width() as u16;
        f.set_cursor_position((
            content_area.x + 4 + cursor.min(content_area.width.saturating_sub(5)),
            content_area.y + row,
        ));
    }
}

#[allow(dead_code)]
pub(super) fn render_question_modal_legacy(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let Some(q) = &state.pending_question() else {
        return;
    };

    let screen = f.area();
    let width = input_area.width.clamp(48, screen.width.saturating_sub(4));

    // Wrap the question to the inner width so the modal height fits it.
    let inner_w = width.saturating_sub(4).max(10) as usize;
    let q_lines = textwrap_simple(&q.question, inner_w);
    let typing = q.custom_input.is_some();
    let hint = if typing {
        "Type your answer · Enter submit · Esc back"
    } else if q.is_multi_select {
        "↑/↓ move · Space toggle · Enter confirm · Esc cancel"
    } else {
        "↑/↓ move · Enter select · 1-9 quick pick · Esc cancel"
    };

    // Real options + the always-present "write your own answer" slot.
    let row_count = q.options.len() as u16 + 1;
    let body_rows = q_lines.len() as u16 + 1 + row_count + 1 + 1; // question + gap + rows + gap + hint
    let height = (body_rows + 2).min(screen.height.saturating_sub(2)).max(6);
    let modal_area = input_anchor_rect(f, input_area, height);

    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COLOR_PRIMARY()))
            .style(Style::default().bg(COLOR_BG())),
        modal_area,
    );
    let inner = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let mut lines: Vec<Line> = Vec::new();
    for ql in q_lines {
        lines.push(Line::from(Span::styled(
            ql,
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));

    for (i, opt) in q.options.iter().enumerate() {
        let is_sel = i == q.selected;
        let prefix_span = if is_sel {
            Span::styled(
                "❯ ",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .bg(COLOR_BG())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("  ", Style::default().fg(COLOR_TEXT()).bg(COLOR_BG()))
        };

        let check_span = if q.is_multi_select {
            let is_checked = q.chosen.get(i).copied().unwrap_or(false);
            let check_str = if is_checked { "[x] " } else { "[ ] " };
            let style = if is_sel {
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .bg(COLOR_BG())
                    .add_modifier(Modifier::BOLD)
            } else if is_checked {
                Style::default().fg(COLOR_TIP()).bg(COLOR_BG())
            } else {
                Style::default().fg(COLOR_MUTED()).bg(COLOR_BG())
            };
            Some(Span::styled(check_str, style))
        } else {
            None
        };

        let num_str = format!("{}. ", i + 1);
        let num_span = Span::styled(
            num_str,
            if is_sel {
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .bg(COLOR_BG())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_MUTED()).bg(COLOR_BG())
            },
        );

        let opt_span = Span::styled(
            opt.to_string(),
            if is_sel {
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .bg(COLOR_BG())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_TEXT()).bg(COLOR_BG())
            },
        );

        let mut row_spans = vec![prefix_span];
        if let Some(cs) = check_span {
            row_spans.push(cs);
        }
        row_spans.push(num_span);
        row_spans.push(opt_span);
        lines.push(Line::from(row_spans));
    }

    // The always-present "write your own answer" slot (index == options.len()).
    let custom_idx = q.options.len();
    let custom_sel = q.selected == custom_idx;
    let prefix_span = if custom_sel {
        Span::styled(
            "❯ ",
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  ", Style::default().fg(COLOR_TEXT()).bg(COLOR_BG()))
    };

    let icon_span = Span::styled(
        "✎ ",
        if custom_sel {
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_MUTED()).bg(COLOR_BG())
        },
    );

    if let Some(text) = &q.custom_input {
        let mut cursor_pos = q.custom_cursor.min(text.len());
        while cursor_pos > 0 && !text.is_char_boundary(cursor_pos) {
            cursor_pos -= 1;
        }
        let before_cursor = &text[..cursor_pos];
        let after_cursor = &text[cursor_pos..];

        let before_span = Span::styled(
            before_cursor.to_string(),
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        );
        let cursor_span = Span::styled(
            "│",
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        );
        let after_span = Span::styled(
            after_cursor.to_string(),
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        );

        lines.push(Line::from(vec![
            prefix_span,
            icon_span,
            before_span,
            cursor_span,
            after_span,
        ]));
    } else if custom_sel {
        let text_span = Span::styled(
            "Write your own answer…│",
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        );
        lines.push(Line::from(vec![prefix_span, icon_span, text_span]));
    } else {
        let text_span = Span::styled(
            "Write your own answer…",
            Style::default().fg(COLOR_MUTED()).bg(COLOR_BG()),
        );
        lines.push(Line::from(vec![prefix_span, icon_span, text_span]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(COLOR_MUTED()).bg(COLOR_BG()),
    )));

    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(COLOR_BG())),
        inner,
    );
}

/// Minimal greedy word-wrap used by the question modal (avoids pulling the chat
/// wrapping helpers into modal code).
pub(super) fn textwrap_simple(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if line.is_empty() {
                line.push_str(word);
            } else if line.width() + 1 + word.width() <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line.push_str(word);
            }
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}
