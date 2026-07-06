//! UI layer: terminal layout, rendering widgets, custom prompt prefix, footer.

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
    let turns = state.history.iter().filter(|m| m.role == "user").count();

    let mut context_text = match state.current_token_usage {
        Some(ref usage) => {
            let percentage = (usage.total_tokens as f32 / crate::config::MAX_CONTEXT_TOKENS as f32) * 100.0;
            format!(
                "Context: {} / {} tokens ({:.1}% used) | Turns: {}",
                usage.total_tokens,
                crate::config::MAX_CONTEXT_TOKENS,
                percentage,
                turns
            )
        }
        None => format!("Context: 0 / {} tokens (0.0% used) | Turns: {}", crate::config::MAX_CONTEXT_TOKENS, turns),
    };
    if let Some(dur) = state.response_time {
        context_text.push_str(&format!(" ({:.1}s)", dur.as_secs_f32()));
    }

    let footer_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
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
        .border_style(Style::default().fg(Color::DarkGray)) // Grayed out border
        .title(Span::styled(input_title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));

    let input_inner = chunks[1].inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(input_block, chunks[1]);

    let prompt_prefix = "> ";
    let prefix_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let text_style = if state.input_buffer.starts_with('/') {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Reset)
    };

    let inner_width = input_inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    if inner_width > 0 {
        // Build the full sequence of styled characters
        let mut styled_chars: Vec<(char, Style)> = Vec::new();

        // 1. Prefix
        for c in prompt_prefix.chars() {
            styled_chars.push((c, prefix_style));
        }
        // 2. Input buffer
        for c in state.input_buffer.chars() {
            styled_chars.push((c, text_style));
        }
        // 3. Suffix (suggestion)
        if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
            for c in suffix.chars() {
                styled_chars.push((c, suggestion_style));
            }
        }

        // Chunk styled_chars into lines of length inner_width exactly (hard wrapping)
        let mut current_line_spans = Vec::new();
        let mut current_run: Option<(Style, String)> = None;

        for (idx, (c, style)) in styled_chars.into_iter().enumerate() {
            if idx > 0 && idx % inner_width == 0 {
                // Flush current run and push line
                if let Some((run_style, run_text)) = current_run.take() {
                    current_line_spans.push(Span::styled(run_text, run_style));
                }
                lines.push(Line::from(current_line_spans));
                current_line_spans = Vec::new();
            }

            if let Some(ref mut run) = current_run {
                if run.0 == style {
                    run.1.push(c);
                } else {
                    let old_run = current_run.replace((style, c.to_string()));
                    if let Some((run_style, run_text)) = old_run {
                        current_line_spans.push(Span::styled(run_text, run_style));
                    }
                }
            } else {
                current_run = Some((style, c.to_string()));
            }
        }

        // Flush last run
        if let Some((run_style, run_text)) = current_run {
            current_line_spans.push(Span::styled(run_text, run_style));
        }
        lines.push(Line::from(current_line_spans));
    }

    if lines.is_empty() {
        lines.push(Line::from(""));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, input_inner);

    // Place cursor relative to state.cursor_position + prefix length.
    if inner_width > 0 {
        let virtual_pos = state.cursor_position + prompt_prefix.len();
        let cursor_dx = (virtual_pos % inner_width) as u16;
        let cursor_dy = (virtual_pos / inner_width) as u16;
        f.set_cursor_position((input_inner.x + cursor_dx, input_inner.y + cursor_dy));
    }
}

/// Render the main conversation area with proper line wrapping and auto-scroll.
fn render_conversation(f: &mut Frame, chunks: &[ratatui::layout::Rect], state: &AppState) {
    let history_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Conversation ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));

    let inner_area = chunks[0].inner(Margin { vertical: 1, horizontal: 2 });

    // Build wrapped display lines.
    let mut lines: Vec<Line> = Vec::new();

    for msg in &state.history {
        if msg.role == "system" {
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
                        Span::styled("> ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                        Span::styled(raw_line, Style::default().fg(Color::Cyan)),
                    ])
                } else {
                    Line::from(Span::styled(
                        format!("{:>2}", ""), // 2 spaces indent.
                        Style::default(),
                    ))
                });
                first = false;
            }
        } else if msg.role == "assistant" {
            // No "Apple FM: " prefix - display text directly.
            for raw_line in msg.content.lines() {
                lines.push(Line::from(Span::styled(raw_line, Style::default().fg(Color::White))));
            }
        }

        lines.push(Line::from("")); // spacer.
    }

    // Streaming / thinking message (latest response still being built).
    if state.status == AppStatus::Streaming {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        let frame_idx = ((millis / 80) % spinner_frames.len() as u128) as usize;
        let spinner = spinner_frames[frame_idx];

        if state.current_response.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", spinner), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::styled("Thinking...", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
            ]));
        } else {
            let mut lines_to_add: Vec<String> = state.current_response.lines().map(String::from).collect();
            if lines_to_add.is_empty() {
                lines_to_add.push(String::new());
            }
            let last_idx = lines_to_add.len() - 1;
            for (idx, raw_line) in lines_to_add.iter().enumerate() {
                if idx == last_idx {
                    lines.push(Line::from(vec![
                        Span::styled(raw_line.clone(), Style::default().fg(Color::White)),
                        Span::styled(format!(" {}", spinner), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    ]));
                } else {
                    lines.push(Line::from(Span::styled(raw_line.clone(), Style::default().fg(Color::White))));
                }
            }
        }
    }

    // Calculate auto-scroll offset based on wrapped line heights.
    let mut total_wrapped_lines = 0u16;
    for line in &lines {
        match line.width() {
            0 => total_wrapped_lines += 1,
            w => total_wrapped_lines += (w as u16).div_ceil(inner_area.width),
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
    // Determine dynamic input area height based on input buffer length and terminal width.
    // Inner input width is terminal width minus margins and borders (6 characters).
    let inner_width = f.area().width.saturating_sub(6).max(1);
    let input_lines = (state.input_buffer.len() as u16 + 2).div_ceil(inner_width).max(1);
    let input_height = input_lines + 2;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(3),  // conversation area (grows/shrinks dynamically).
            Constraint::Length(input_height), // input area.
            Constraint::Length(1), // footer.
        ])
        .split(f.area());

    render_conversation(f, &chunks, state);
    render_input(f, &chunks, state);
    render_footer(f, &chunks, state);
}
