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
    let header_line = question_modal_header(
        &question,
        state.pending_question_chain_len(),
        state.pending_question_chain_position(),
        state
            .pending_question_chain_len()
            .saturating_sub(state.pending_question_chain_answered()),
    );
    let (mut lines, custom_row, custom_text_width) =
        question_modal_lines(question, &header_line, content_area.width as usize);
    let footer = question_modal_footer_lines(
        &question,
        state.pending_question_chain_len() > 1,
        content_area.width as usize,
    );
    let footer_start = lines.len();
    lines.extend(footer);

    // Keep the submission and chain-navigation hint visible when a long
    // question fills the panel. While editing, scroll the body to keep the
    // custom-answer cursor in view.
    let visible_height = content_area.height as usize;
    let footer = lines.split_off(footer_start.min(lines.len()));
    let body_height = visible_height.saturating_sub(footer.len());
    let body_start =
        if let (Some(row), Some(custom)) = (custom_row, question.custom_input.as_ref()) {
            let (cursor_row, _) = question_custom_cursor_position(
                custom,
                question.custom_cursor,
                custom_text_width.max(1) as usize,
            );
            (row as usize)
                .saturating_add(cursor_row)
                .saturating_sub(body_height.saturating_sub(1))
        } else {
            0
        }
        .min(footer_start.saturating_sub(body_height));
    let mut visible_lines = lines
        .into_iter()
        .skip(body_start)
        .take(body_height)
        .collect::<Vec<_>>();
    visible_lines.extend(
        footer
            .into_iter()
            .take(visible_height.saturating_sub(visible_lines.len())),
    );
    let visible_body_end = body_start.saturating_add(body_height);
    paint_panel_line_backgrounds(&mut visible_lines, panel);
    f.render_widget(
        Paragraph::new(visible_lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(panel)),
        content_area,
    );
    if let (Some(row), Some(custom)) = (custom_row, question.custom_input.as_ref()) {
        let (extra_rows, cursor_column) = question_custom_cursor_position(
            custom,
            question.custom_cursor,
            custom_text_width.max(1) as usize,
        );
        let cursor_row = (row as usize).saturating_add(extra_rows);
        if cursor_row >= body_start && cursor_row < visible_body_end {
            f.set_cursor_position((
                content_area.x
                    + 4
                    + (cursor_column as u16).min(content_area.width.saturating_sub(5)),
                content_area.y + (cursor_row - body_start) as u16,
            ));
        }
    }
}

/// Build the question body as explicitly wrapped rows so height measurement
/// and painting use the same layout.
pub(super) fn question_modal_lines(
    question: &rustcode::controller::PendingQuestion,
    header: &str,
    width: usize,
) -> (Vec<Line<'static>>, Option<u16>, u16) {
    let width = width.max(1);
    let mut lines = wrap_indented(
        header.trim_start(),
        2,
        width,
        Style::default()
            .fg(COLOR_TEXT())
            .add_modifier(Modifier::BOLD),
    );
    lines.extend(wrap_indented(
        &question.question,
        2,
        width,
        Style::default(),
    ));
    lines.push(Line::from(""));

    if let Some(custom) = question.custom_input.as_ref() {
        let prefix = "  › ";
        let row = lines.len() as u16;
        let display = if custom.is_empty() {
            "Type your answer (optional)".to_owned()
        } else {
            custom.clone()
        };
        let style = Style::default().fg(if custom.is_empty() {
            COLOR_MUTED()
        } else {
            COLOR_TEXT()
        });
        let custom_text_width = width.saturating_sub(prefix.width()).max(1) as u16;
        let mut wrapped = question_custom_answer_lines(&display, custom_text_width as usize, style);
        lines.append(&mut wrapped);
        lines.push(Line::from(""));
        return (lines, Some(row), custom_text_width);
    }

    for (index, option) in question.options.iter().enumerate() {
        let selected = question.selected == index;
        let checked = question.chosen.get(index).copied().unwrap_or(false);
        let marker = if selected { "  › " } else { "    " };
        let number = if question.is_multi_select {
            format!("{} {}. ", if checked { "[x]" } else { "[ ]" }, index + 1)
        } else {
            format!("{}. ", index + 1)
        };
        let prefix_width = marker.width() + number.width();
        let label_style = Style::default().fg(COLOR_TEXT()).add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
        let mut content = vec![Span::styled(option.clone(), label_style)];
        if let Some(description) = question.description(index)
            && !description.is_empty()
        {
            content.push(Span::styled(
                format!(" — {description}"),
                Style::default().fg(COLOR_MUTED()),
            ));
        }
        let hanging = " ".repeat(prefix_width);
        let mut wrapped = wrap_spans_with_prefix(
            vec![
                Span::styled(
                    marker,
                    Style::default().fg(if selected {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_TEXT()
                    }),
                ),
                Span::styled(
                    number,
                    Style::default().fg(if selected {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_MUTED()
                    }),
                ),
            ],
            content,
            &hanging,
            width,
        );
        lines.append(&mut wrapped);
    }
    let custom_selected = question.selected == question.options.len();
    lines.extend(wrap_spans_with_prefix(
        vec![Span::styled(
            if custom_selected { "  › " } else { "    " },
            Style::default().fg(if custom_selected {
                COLOR_PRIMARY()
            } else {
                COLOR_TEXT()
            }),
        )],
        vec![Span::styled(
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
        )],
        "    ",
        width,
    ));
    lines.push(Line::from(""));
    (lines, None, 1)
}

fn question_custom_answer_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut column = 0usize;
    for character in text.chars() {
        if character == '\n' {
            rows.push(String::new());
            column = 0;
            continue;
        }
        let character_width = character.width().unwrap_or(1);
        if column > 0 && column.saturating_add(character_width) > width {
            rows.push(String::new());
            column = 0;
        }
        rows.last_mut().expect("answer row exists").push(character);
        column = column.saturating_add(character_width);
    }
    if column >= width {
        rows.push(String::new());
    }
    rows.into_iter()
        .enumerate()
        .map(|(index, text)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 { "  › " } else { "    " }.to_owned(),
                    Style::default().fg(COLOR_PRIMARY()),
                ),
                Span::styled(text, style),
            ])
        })
        .collect()
}

pub(super) fn question_custom_cursor_position(
    text: &str,
    cursor: usize,
    width: usize,
) -> (usize, usize) {
    let width = width.max(1);
    let cursor = cursor.min(text.len());
    let mut row = 0usize;
    let mut column = 0usize;
    for (byte, character) in text.char_indices() {
        if byte >= cursor {
            if byte == cursor
                && character != '\n'
                && column > 0
                && column.saturating_add(character.width().unwrap_or(1)) > width
            {
                row += 1;
                column = 0;
            }
            break;
        }
        if character == '\n' {
            row += 1;
            column = 0;
            continue;
        }
        let character_width = character.width().unwrap_or(1);
        if column > 0 && column.saturating_add(character_width) > width {
            row += 1;
            column = 0;
        }
        column = column.saturating_add(character_width);
    }
    if cursor == text.len() && column >= width {
        row += 1;
        column = 0;
    }
    (row, column)
}

pub(super) fn question_modal_header(
    question: &rustcode::controller::PendingQuestion,
    chain_len: usize,
    position: usize,
    unanswered: usize,
) -> String {
    if chain_len > 1 {
        format!(
            "  {} · Question {position}/{chain_len} ({unanswered} unanswered)",
            question.header
        )
    } else {
        format!("  {}", question.header)
    }
}

pub(super) fn question_modal_footer(
    question: &rustcode::controller::PendingQuestion,
    chained: bool,
    width: usize,
) -> Line<'static> {
    if width < 40 {
        let text = if question.custom_input.is_some() {
            "↵ submit · esc back".to_owned()
        } else if question.is_multi_select {
            "space toggle · ↵ submit · esc".to_owned()
        } else if chained {
            "↵ submit · tab next · ⇧tab back".to_owned()
        } else {
            "↵ submit · esc interrupt".to_owned()
        };
        return Line::from(Span::styled(text, Style::default().fg(COLOR_MUTED())));
    }
    let nav_hint = if chained {
        " · tab next · shift+tab back"
    } else {
        ""
    };
    Line::from(Span::styled(
        if question.custom_input.is_some() {
            format!("enter to submit answer | esc to go back{nav_hint}")
        } else if question.is_multi_select {
            format!("space to toggle | enter to submit answer | esc to interrupt{nav_hint}")
        } else {
            format!("enter to submit answer | esc to interrupt{nav_hint}")
        },
        Style::default().fg(COLOR_MUTED()),
    ))
}

pub(super) fn question_modal_footer_lines(
    question: &rustcode::controller::PendingQuestion,
    chained: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let footer = question_modal_footer(question, chained, width);
    let text = footer
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let style = footer
        .spans
        .first()
        .map_or(Style::default(), |span| span.style);
    wrap_indented(&text, 2, width, style)
}

fn wrap_indented(text: &str, indent: usize, width: usize, style: Style) -> Vec<Line<'static>> {
    let indent = " ".repeat(indent.min(width.saturating_sub(1)));
    let mut lines = text
        .split('\n')
        .flat_map(|paragraph| {
            wrap_spans(
                vec![Span::styled(paragraph.to_owned(), style)],
                width.saturating_sub(indent.width()).max(1),
            )
        })
        .collect::<Vec<_>>();
    for line in &mut lines {
        line.spans.insert(0, Span::styled(indent.clone(), style));
    }
    lines
}

pub(super) fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = vec![Vec::<Span<'static>>::new()];
    let mut line_width = 0usize;
    let mut has_content = false;
    for span in spans {
        let style = span.style;
        for word in span.content.split_whitespace() {
            let mut pieces = Vec::new();
            let mut piece = String::new();
            let mut piece_width = 0;
            for character in word.chars() {
                let character_width = character.to_string().width();
                if !piece.is_empty() && piece_width + character_width > width {
                    pieces.push(std::mem::take(&mut piece));
                    piece_width = 0;
                }
                piece.push(character);
                piece_width += character_width;
            }
            if !piece.is_empty() {
                pieces.push(piece);
            }

            for (index, piece) in pieces.iter().enumerate() {
                let piece_width = piece.width();
                if has_content
                    && line_width
                        .saturating_add(usize::from(index == 0))
                        .saturating_add(piece_width)
                        > width
                {
                    lines.push(Vec::new());
                    line_width = 0;
                    has_content = false;
                }
                if has_content && index == 0 {
                    lines
                        .last_mut()
                        .unwrap()
                        .push(Span::styled(" ".to_owned(), style));
                    line_width += 1;
                }
                lines
                    .last_mut()
                    .unwrap()
                    .push(Span::styled(piece.clone(), style));
                line_width += piece_width;
                has_content = true;
                if index + 1 < pieces.len() {
                    lines.push(Vec::new());
                    line_width = 0;
                    has_content = false;
                }
            }
        }
    }
    lines.into_iter().map(Line::from).collect()
}

fn wrap_spans_with_prefix(
    prefix: Vec<Span<'static>>,
    content: Vec<Span<'static>>,
    hanging: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let prefix_width = prefix
        .iter()
        .map(|span| span.content.width())
        .sum::<usize>();
    let available = width.saturating_sub(prefix_width).max(1);
    let mut wrapped = wrap_spans(content, available);
    if let Some(first) = wrapped.first_mut() {
        let mut line = prefix;
        line.append(&mut first.spans);
        first.spans = line;
    }
    for line in wrapped.iter_mut().skip(1) {
        line.spans.insert(0, Span::raw(hanging.to_owned()));
    }
    wrapped
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
