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
