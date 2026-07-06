use crate::app::{AppStatus, AppState};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

/// Format the footer status bar.
fn render_footer(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) {
    let mut context_text = match state.current_token_usage {
        Some(ref usage) => format!("Context: {} / {} tokens", usage.total_tokens, crate::config::MAX_CONTEXT_TOKENS),
        None => "Context: N/A".to_string(),
    };
    if let Some(dur) = state.response_time {
        context_text.push_str(&format!(" ({:.1}s)", dur.as_secs_f32()));
    }

    let footer_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    let context_paragraph = Paragraph::new(Span::styled(
        context_text, Style::default().fg(Color::DarkGray),
    ));

    let help_paragraph = Paragraph::new(Span::styled(
        "Type /help for commands",
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    )).alignment(ratatui::layout::Alignment::Right);

    f.render_widget(context_paragraph, footer_chunks[0]);
    f.render_widget(help_paragraph, footer_chunks[1]);
}

/// Render the input area (block + cursor + optional completion suffix).
fn render_input(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) {
    let input_title = match state.status {
        AppStatus::Idle => " Prompt (Enter to send) ".to_string(),
        AppStatus::Streaming => format!(" Streaming... (/cancel abort) | Queue: {}", state.pending_queue.len()),
        AppStatus::Queued => format!(" Queued | {} pending ", state.pending_queue.len()),
    };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(input_title);

    let input_inner = chunks[1].inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(input_block, chunks[1]);

    // If there's a completion suffix to show, render it separately.
    if let Some(suffix) = state.get_command_suggestion() {
        let suggestion_style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
        f.render_widget(Paragraph::new(state.input_buffer.as_str()), input_inner);

        let mut offset_inner = input_inner;
        offset_inner.x = offset_inner.x.saturating_add(state.input_buffer.len() as u16);
        f.render_widget(Paragraph::new(suffix).style(suggestion_style), offset_inner);
    } else {
        let text_style = if state.input_buffer.starts_with('/') {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Reset)
        };
        f.render_widget(Paragraph::new(state.input_buffer.as_str()).style(text_style), input_inner);
    }

    // Place cursor at the user's position in the buffer.
    let cursor_x = input_inner.x + state.cursor_position as u16;
    let cursor_y = input_inner.y;
    f.set_cursor_position((cursor_x, cursor_y));
}

/// Render the main conversation area with proper line wrapping and auto-scroll.
fn render_conversation(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) {
    let history_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::DarkGray))
        .title(" Conversation ");

    let inner_area = chunks[0].inner(Margin { vertical: 1, horizontal: 2 });
    let _width = inner_area.width as usize;

    // Build wrapped display lines.
    let mut lines: Vec<Line> = Vec::new();

    for msg in &state.history {
        if msg.role == "tool_call" {
            let tool_name = msg.content
                .trim_start_matches("[TOOL: ")
                .trim_end_matches("]")
                .trim();
            lines.push(Line::from(vec![
                Span::styled(format!("  Running {}()...", tool_name), Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
            ]));
        } else if msg.role == "tool_output" {
            let mut display_content = msg.content.as_str();
            // Strip the "Tool Output (name): " prefix for clean display.
            if let Some(pos) = msg.content.find("): ") {
                display_content = &msg.content[pos + 3..];
            }
            for raw_line in display_content.lines() {
                lines.push(Line::from(vec![
                    Span::styled(" ↳ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(raw_line, Style::default().fg(Color::DarkGray)),
                ]));
            }
        } else if msg.role == "system" {
            // Local helper messages (e.g. /help output).
            for raw_line in msg.content.lines() {
                lines.push(Line::from(vec![
                    Span::styled("System: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::styled(raw_line, Style::default().fg(Color::Yellow)),
                ]));
            }
        } else if msg.role == "user" {
            let mut first = true;
            for raw_line in msg.content.lines() {
                lines.push(if first {
                    Line::from(vec![
                        Span::styled("You: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                        Span::styled(raw_line, Style::default().fg(Color::Cyan)),
                    ])
                } else {
                    Line::from(Span::styled(
                        format!("{:>4}", ""), // 4 spaces indent for alignment.
                        Style::default(),
                    ))
                });
                first = false;
            }
        } else if msg.role == "assistant" {
            let content = &msg.content;
            let mut first = true;
            for raw_line in content.lines() {
                lines.push(if first {
                    Line::from(vec![
                        Span::styled("Apple FM: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                        Span::styled(raw_line, Style::default().fg(Color::White)),
                    ])
                } else {
                    Line::from(Span::raw(format!("{:>4}", "")))
                });
                first = false;
            }
        }

        lines.push(Line::from("")); // spacer.
    }

    // Streaming / thinking message (latest response still being built).
    if !state.current_response.is_empty() || state.status == AppStatus::Streaming {
        let content = if state.current_response.is_empty() { "Thinking..." } else { &state.current_response };
        let mut first = true;
        for raw_line in content.lines() {
            lines.push(if first {
                Line::from(vec![
                    Span::styled("Apple FM: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    if state.current_response.is_empty() {
                        Span::styled(raw_line, Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))
                    } else {
                        Span::styled(raw_line, Style::default().fg(Color::White))
                    },
                ])
            } else {
                Line::from(Span::raw(format!("{:>4}", "")))
            });
            first = false;
        }
    }

    // Calculate auto-scroll offset based on wrapped line heights.
    let mut total_wrapped_lines = 0u16;
    for line in &lines {
        match line.width() {
            0 => total_wrapped_lines += 1,
            w => total_wrapped_lines += (w as u16).div_ceil(inner_area.height),
        }
    }
    let scroll_offset = total_wrapped_lines.saturating_sub(inner_area.height);

    let conversation_paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .scroll((scroll_offset, 0));

    f.render_widget(history_block, chunks[0]);
    f.render_widget(conversation_paragraph, inner_area);
}

/// Main render function called by the TUI event loop.
pub fn render(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(3),  // conversation area (grows).
            Constraint::Length(3), // input.
            Constraint::Length(1), // footer.
        ])
        .split(f.area());

    render_conversation(f, &chunks, state);
    render_input(f, &chunks, state);
    render_footer(f, &chunks, state);
}
