use crate::app::{AppState, AppStatus};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

pub fn render(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    // Conversation container block
    let history_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::DarkGray))
        .title(" Conversation ");

    let inner_area = chunks[0].inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let width = inner_area.width as usize;
    let mut lines = Vec::new();

    // Format historical messages
    for msg in &state.history {
        if msg.role == "tool_call" {
            let tool_name = msg.content
                .trim_start_matches("[TOOL: ")
                .trim_end_matches("]")
                .trim();
            lines.push(Line::from(vec![
                Span::styled(format!("  ⚙️  Running {}()...", tool_name), Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
            ]));
        } else if msg.role == "tool_output" {
            let mut display_content = msg.content.as_str();
            if let Some(pos) = msg.content.find("): ") {
                display_content = &msg.content[pos + 3..];
            }
            for raw_line in display_content.lines() {
                lines.push(Line::from(vec![
                    Span::styled("  ↳  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(raw_line, Style::default().fg(Color::DarkGray)),
                ]));
            }
        } else {
            let (role_span, content_style) = if msg.role == "user" {
                (
                    Span::styled("You: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Style::default().fg(Color::Cyan),
                )
            } else if msg.role == "system" {
                (
                    Span::styled("System: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Style::default().fg(Color::Yellow),
                )
            } else {
                (
                    Span::styled("Apple FM: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Style::default().fg(Color::White),
                )
            };

            let mut first = true;
            for raw_line in msg.content.lines() {
                if first {
                    lines.push(Line::from(vec![
                        role_span.clone(),
                        Span::styled(raw_line, content_style),
                    ]));
                    first = false;
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(raw_line, content_style),
                    ]));
                }
            }
            lines.push(Line::from("")); // Spacer line
        }
    }

    // Format currently streaming/thinking message
    if !state.current_response.is_empty() || state.status == AppStatus::Streaming {
        let content = if state.current_response.is_empty() {
            "Thinking..."
        } else {
            &state.current_response
        };

        let (role_span, content_style) = (
            Span::styled("Apple FM: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            if state.current_response.is_empty() {
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)
            } else {
                Style::default().fg(Color::White)
            },
        );

        let mut first = true;
        for raw_line in content.lines() {
            if first {
                lines.push(Line::from(vec![
                    role_span.clone(),
                    Span::styled(raw_line, content_style),
                ]));
                first = false;
            } else {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(raw_line, content_style),
                ]));
            }
        }
    }

    // Calculate dynamic scrolling offset based on actual wrapped line height
    let mut total_wrapped_lines = 0;
    for line in &lines {
        let line_len = line.width();
        let wrapped = if line_len == 0 {
            1
        } else {
            (line_len + width - 1) / width
        };
        total_wrapped_lines += wrapped;
    }

    let scroll_offset = total_wrapped_lines.saturating_sub(inner_area.height as usize) as u16;

    let conversation_paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .scroll((scroll_offset, 0));

    f.render_widget(history_block, chunks[0]);
    f.render_widget(conversation_paragraph, inner_area);

    // Context-Aware Input Title Borders
    let input_title = match state.status {
        AppStatus::Idle => " Prompt (Press Enter to Send) ".to_string(),
        AppStatus::Streaming => format!(" Streaming... (Type /cancel to abort) | Queue: {} ", state.pending_queue.len()),
        AppStatus::Queued => format!(" Action Queued | Queue: {} Pending ", state.pending_queue.len()),
    };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(input_title);

    let input_inner = chunks[1].inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    f.render_widget(input_block, chunks[1]);

    if let Some(suggestion) = state.get_command_suggestion() {
        let suggestion_style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
        let current_len = state.input_buffer.len();
        let remaining_suggestion = &suggestion[current_len..];

        f.render_widget(Paragraph::new(state.input_buffer.as_str()), input_inner);

        let mut offset_inner = input_inner;
        offset_inner.x = offset_inner.x.saturating_add(current_len as u16);
        f.render_widget(Paragraph::new(remaining_suggestion).style(suggestion_style), offset_inner);
    } else {
        let text_style = if state.input_buffer.starts_with('/') {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Reset)
        };

        f.render_widget(Paragraph::new(state.input_buffer.as_str()).style(text_style), input_inner);
    }

    // Set cursor position relative to state.cursor_position
    let cursor_x = input_inner.x + state.cursor_position as u16;
    let cursor_y = input_inner.y;
    f.set_cursor_position((cursor_x, cursor_y));

    // Footer / Status Bar
    let mut context_text = "Context: 0 / 2048 tokens".to_string();
    if let Some(last_msg) = state.history.iter().rev().find(|m| m.role == "assistant" && m.token_usage.is_some()) {
        if let Some(ref usage) = last_msg.token_usage {
            context_text = format!("Context: {} / 2048 tokens", usage.total_tokens);
        }
    }

    let footer_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(chunks[2]);

    let context_paragraph = Paragraph::new(Span::styled(
        context_text,
        Style::default().fg(Color::DarkGray),
    ));

    let help_paragraph = Paragraph::new(Span::styled(
        "Type /help for command list",
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    ))
    .alignment(ratatui::layout::Alignment::Right);

    f.render_widget(context_paragraph, footer_chunks[0]);
    f.render_widget(help_paragraph, footer_chunks[1]);
}
