use super::*;

pub(super) fn wrap_input_chars(
    styled_chars: &[(char, Style)],
    inner_width: usize,
    cursor_char_index: usize,
    prompt_style: Style,
) -> (Vec<Line<'static>>, u16, u16) {
    if inner_width == 0 {
        return (vec![Line::default()], 0, 0);
    }

    type InputChar = (usize, char, Style);
    type InputLine = (Vec<InputChar>, usize);

    let indent = 2.min(inner_width);
    let mut wrapped: Vec<InputLine> = Vec::new();
    let mut current: Vec<InputChar> = Vec::new();
    let mut current_start = 0;
    let mut current_width = indent;

    for (index, &(character, style)) in styled_chars.iter().enumerate() {
        if character == '\n' {
            wrapped.push((std::mem::take(&mut current), current_start));
            current_start = index + 1;
            current_width = indent;
            continue;
        }

        let character_width = character.width().unwrap_or(1);
        if current_width + character_width > inner_width && !current.is_empty() {
            let split_at = current
                .iter()
                .rposition(|(_, character, _)| character.is_whitespace())
                .filter(|&index| index + 1 < current.len());
            let remainder = split_at.map(|index| current.split_off(index + 1));

            wrapped.push((std::mem::take(&mut current), current_start));
            current = remainder.unwrap_or_default();
            current_start = current.first().map(|(index, _, _)| *index).unwrap_or(index);
            current_width = indent
                + current
                    .iter()
                    .map(|(_, character, _)| character.width().unwrap_or(1))
                    .sum::<usize>();
        }

        current.push((index, character, style));
        current_width += character_width;
    }
    wrapped.push((current, current_start));

    let mut cursor_positions = vec![None; styled_chars.len() + 1];
    let mut lines = Vec::with_capacity(wrapped.len());
    for (row, (characters, start)) in wrapped.into_iter().enumerate() {
        let mut spans = vec![Span::styled(
            if row == 0 { "› " } else { "  " },
            prompt_style,
        )];
        let mut current_run: Option<(Style, String)> = None;
        let mut column = indent;
        cursor_positions[start] = Some((column as u16, row as u16));

        for (index, character, style) in characters {
            cursor_positions[index] = Some((column as u16, row as u16));
            match current_run.as_mut() {
                Some((run_style, text)) if *run_style == style => text.push(character),
                _ => {
                    if let Some((run_style, text)) = current_run.take() {
                        spans.push(Span::styled(text, run_style));
                    }
                    current_run = Some((style, character.to_string()));
                }
            }
            column += character.width().unwrap_or(1);
            cursor_positions[index + 1] = Some((column as u16, row as u16));
        }
        if let Some((run_style, text)) = current_run {
            spans.push(Span::styled(text, run_style));
        }
        lines.push(Line::from(spans));
    }

    let cursor = cursor_positions
        .get(cursor_char_index.min(styled_chars.len()))
        .copied()
        .flatten()
        .unwrap_or((indent as u16, 0));
    (lines, cursor.0, cursor.1)
}

pub(super) fn count_input_lines(input_buffer: &str, inner_width: usize) -> u16 {
    if inner_width == 0 {
        return 1;
    }

    let collapsed = collapse_image_markers(input_buffer);
    let styled_chars = collapsed
        .chars()
        .map(|character| (character, Style::default()))
        .collect::<Vec<_>>();
    wrap_input_chars(&styled_chars, inner_width, 0, Style::default())
        .0
        .len() as u16
}

pub(super) fn format_token_count(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}K", tokens as f32 / 1000.0)
    } else {
        tokens.to_string()
    }
}

pub(super) fn context_usage(state: &RenderSnapshot) -> (u32, Option<u32>) {
    if let Some(usage) = &state.current_token_usage() {
        return (usage.total_tokens, usage.cached_tokens);
    }

    if let Some(usage) = state
        .active_history()
        .iter()
        .rev()
        .find_map(|message| message.token_usage.as_ref())
    {
        return (usage.total_tokens, usage.cached_tokens);
    }

    let chars: usize = state
        .active_history()
        .iter()
        .map(|message| message.content.len())
        .sum();
    ((chars / 4) as u32, None)
}

pub(super) fn activity_status_label(state: &RenderSnapshot) -> String {
    if *state.status() == AppStatus::Idle && state.waiting_for_background_terminal() {
        return "Waiting for background terminal".to_string();
    }
    let base_activity = classify_activity(&state.status(), &state.running_tools());
    let activity = if base_activity.kind == ActivityKind::ActionRequired {
        base_activity
    } else {
        classify_live_tools(&state.live_tool_calls()).unwrap_or(base_activity)
    };
    if activity.kind == ActivityKind::ActionRequired {
        return "Action Required".to_string();
    }
    if activity.kind == ActivityKind::Queued {
        return "Queued".to_string();
    }
    if activity.kind == ActivityKind::Ready {
        return "Idle".to_string();
    }
    if state.current_thought_started_at().is_some() {
        return "Thinking".to_string();
    }
    "Working".to_string()
}

pub(super) fn background_terminal_summary(count: usize) -> String {
    let terminal = if count == 1 {
        "1 background terminal running".to_string()
    } else {
        format!("{count} background terminals running")
    };
    format!("{terminal} · /ps to view · /stop to close")
}

pub(super) fn background_command_lines(state: &RenderSnapshot) -> Vec<Line<'static>> {
    const MAX_VISIBLE_COMMANDS: usize = 3;
    let style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false);
    let mut lines = state
        .background_tasks()
        .iter()
        .take(MAX_VISIBLE_COMMANDS)
        .map(|task| {
            let command = crate::tools::background_command_label(&task.command, 240);
            Line::from(Span::styled(format!("  └ {command}"), style))
        })
        .collect::<Vec<_>>();
    let omitted = state
        .background_tasks()
        .len()
        .saturating_sub(MAX_VISIBLE_COMMANDS);
    if omitted > 0 {
        lines.push(Line::from(Span::styled(
            format!("  └ … {omitted} more (/ps to view)"),
            style,
        )));
    }
    lines
}

pub(super) fn blend_rgb(c1: (u8, u8, u8), c2: (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let f = factor.clamp(0.0, 1.0);
    let r = (c1.0 as f32 * f + c2.0 as f32 * (1.0 - f)) as u8;
    let g = (c1.1 as f32 * f + c2.1 as f32 * (1.0 - f)) as u8;
    let b = (c1.2 as f32 * f + c2.2 as f32 * (1.0 - f)) as u8;
    (r, g, b)
}

#[cfg(not(test))]
pub(super) static SHIMMER_START: OnceLock<Instant> = OnceLock::new();

pub(super) fn shimmer_rgb(color: Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    }
}

pub(super) fn shimmer_spans_at(text: &str, elapsed: Duration) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos = ((elapsed.as_secs_f32() % sweep_seconds) / sweep_seconds * period as f32) as isize;
    let band_half_width = 5.0f32;

    let base_rgb = shimmer_rgb(COLOR_MUTED(), (128, 128, 128));
    let highlight_rgb = shimmer_rgb(COLOR_TEXT(), (255, 255, 255));

    chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let i_pos = i as isize + padding as isize;
            let dist = (i_pos - pos).abs() as f32;
            let t = if dist <= band_half_width {
                0.5 * (1.0 + (std::f32::consts::PI * (dist / band_half_width)).cos())
            } else {
                0.0
            };
            let (r, g, b) = blend_rgb(highlight_rgb, base_rgb, t * 0.9);
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

pub(super) fn shimmer_spans(text: &str, _show_picker: bool) -> Vec<Span<'static>> {
    #[cfg(test)]
    let elapsed = Duration::ZERO;
    #[cfg(not(test))]
    let elapsed = SHIMMER_START.get_or_init(Instant::now).elapsed();
    shimmer_spans_at(text, elapsed)
}

pub(super) fn fmt_elapsed_compact(elapsed_secs: u64) -> String {
    crate::app::status::format_elapsed_compact(elapsed_secs)
}

pub(super) fn activity_status_line(state: &RenderSnapshot, show_picker: bool) -> Line<'static> {
    let base_activity = classify_activity(&state.status(), &state.running_tools());
    let activity = if base_activity.kind == ActivityKind::ActionRequired {
        base_activity
    } else if *state.status() == AppStatus::Idle && state.waiting_for_background_terminal() {
        ActivitySnapshot {
            kind: ActivityKind::Working,
            label: "Waiting for background terminal".to_string(),
            detail: None,
            animated: true,
        }
    } else {
        classify_live_tools(&state.live_tool_calls()).unwrap_or(base_activity)
    };
    let action_detail = state
        .pending_tool_confirmation()
        .as_ref()
        .and_then(|confirmations| confirmations.first())
        .map(|confirmation| format!("approve {}", confirmation.tool_name))
        .or_else(|| {
            state
                .pending_question()
                .as_ref()
                .map(|_| "answer question".to_string())
        });

    let mut spans = vec![Span::raw(" ")];

    let bullet_symbol = match activity.kind {
        ActivityKind::ActionRequired => "!",
        ActivityKind::Ready => "◦",
        _ => "•",
    };
    let bullet_color = match activity.kind {
        ActivityKind::ActionRequired => Color::Yellow,
        ActivityKind::Ready => COLOR_MUTED(),
        _ => COLOR_PRIMARY(),
    };
    spans.push(Span::styled(
        bullet_symbol,
        get_themed_style(bullet_color, COLOR_BG(), Modifier::BOLD, show_picker),
    ));
    spans.push(Span::raw(" "));

    let label_text = activity_status_label(state);
    if matches!(
        activity.kind,
        ActivityKind::Working | ActivityKind::RunningTool
    ) {
        spans.extend(shimmer_spans(&label_text, show_picker));
    } else {
        spans.push(Span::styled(
            label_text,
            get_themed_style(
                if activity.kind == ActivityKind::ActionRequired {
                    Color::Yellow
                } else if activity.kind == ActivityKind::Ready {
                    COLOR_MUTED()
                } else {
                    COLOR_PRIMARY()
                },
                COLOR_BG(),
                Modifier::BOLD,
                show_picker,
            ),
        ));
    }

    if activity.kind == ActivityKind::ActionRequired {
        if let Some(detail) = action_detail {
            spans.push(Span::styled(
                format!(" · {detail}"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    }

    let background_started = state.background_tasks().first().map(|task| task.start_time);
    let started = if state.waiting_for_background_terminal() {
        background_started
    } else {
        state.generation_start_time()
    };
    let interruptible = matches!(
        activity.kind,
        ActivityKind::Queued | ActivityKind::Working | ActivityKind::RunningTool
    );

    if state.waiting_for_background_terminal() {
        if let Some(started) = started {
            spans.push(Span::styled(
                format!(" ({} · ", fmt_elapsed_compact(started.elapsed().as_secs())),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
            spans.push(Span::styled(
                "esc to interrupt",
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
            spans.push(Span::styled(
                ")",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    } else if matches!(
        activity.kind,
        ActivityKind::Working | ActivityKind::RunningTool
    ) && let Some(started) = started
    {
        spans.push(Span::styled(
            format!(" ({})", fmt_elapsed_compact(started.elapsed().as_secs())),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }

    if interruptible && !state.waiting_for_background_terminal() {
        spans.push(Span::styled(
            " · esc ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        spans.push(Span::styled(
            "interrupt",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
    }

    if !state.background_tasks().is_empty() {
        spans.push(Span::styled(
            format!(
                " · {}",
                background_terminal_summary(state.background_tasks().len())
            ),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }

    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Maximum queued user prompts previewed above the composer.
pub(super) const MAX_QUEUE_PREVIEW_ROWS: usize = 3;

pub(super) fn queued_user_prompts(state: &RenderSnapshot) -> Vec<&str> {
    state
        .pending_queue()
        .iter()
        .filter(|prompt| !prompt.starts_with("__task_wakeup__:"))
        .rev()
        .take(MAX_QUEUE_PREVIEW_ROWS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

pub(super) fn queue_preview_height(state: &RenderSnapshot) -> u16 {
    let rows = queued_user_prompts(state).len();
    if rows == 0 { 0 } else { rows as u16 + 1 }
}

pub(super) fn truncate_queue_prompt(prompt: &str, max_width: usize) -> String {
    if prompt.width() <= max_width {
        return prompt.to_owned();
    }
    let ellipsis_width = "…".width();
    let mut text = String::new();
    let mut width = 0;
    for ch in prompt.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width + ellipsis_width > max_width {
            break;
        }
        text.push(ch);
        width += ch_width;
    }
    text.push('…');
    text
}

/// Shows the most recent queued user prompts directly above the input box.
/// Internal wakeups stay queued but never consume composer space or leak into
/// this transcript-like preview.
pub(super) fn render_queue_line(
    f: &mut Frame,
    chunks: &[ratatui::layout::Rect],
    state: &RenderSnapshot,
) {
    let prompts = queued_user_prompts(state);
    if prompts.is_empty() {
        return;
    }
    let block = chunks[1];
    if block.height == 0 {
        return;
    }
    let show_picker = state.modal_open();
    let queued_count = state
        .pending_queue()
        .iter()
        .filter(|prompt| !prompt.starts_with("__task_wakeup__:"))
        .count();
    let header = Line::from(Span::styled(
        format!("queued ({queued_count}) · ↑ edit last"),
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    ));
    f.render_widget(
        Paragraph::new(header).style(Style::default().bg(COLOR_BG())),
        ratatui::layout::Rect::new(block.x, block.y, block.width, 1),
    );

    for (row, prompt) in prompts.into_iter().enumerate() {
        let prefix = "  › ";
        let preview = truncate_queue_prompt(
            prompt,
            (block.width as usize).saturating_sub(prefix.width()),
        );
        let line = Line::from(vec![
            Span::styled(
                prefix,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                preview,
                get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(COLOR_BG())),
            ratatui::layout::Rect::new(block.x, block.y + row as u16 + 1, block.width, 1),
        );
    }
}

pub(super) fn render_input(
    f: &mut Frame,
    chunks: &[ratatui::layout::Rect],
    state: &RenderSnapshot,
) -> Margin {
    let show_picker = state.modal_open();
    let area = chunks[2];
    f.render_widget(Clear, area);
    f.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(COLOR_PANEL())),
        area,
    );
    let input_margin = Margin {
        vertical: 1,
        horizontal: 0,
    };
    let input_inner = area.inner(input_margin);

    let text_style = if state.input_buffer().starts_with('/') {
        get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker)
    } else {
        get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker)
    };

    let inner_width = input_inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_dx = 0u16;
    let mut cursor_dy = 0u16;

    if inner_width > 0 {
        let marker_style =
            get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker);
        let mut styled_chars = Vec::new();
        for (segment, marker) in collapsed_marker_segments(&state.input_buffer()) {
            let style = if marker.is_some() {
                marker_style
            } else {
                text_style
            };
            styled_chars.extend(segment.chars().map(|c| (c, style)));
        }

        if state.input_buffer().is_empty() && state.get_command_suggestion().is_none() {
            let placeholder_style =
                get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
            let placeholder_text = "Ask RustCode to do anything";
            styled_chars.extend(placeholder_text.chars().map(|c| (c, placeholder_style)));
        } else if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        let safe_end = state.cursor_position().min(state.input_buffer().len());
        let safe_end = if state.input_buffer().is_char_boundary(safe_end) {
            safe_end
        } else {
            state
                .input_buffer()
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= safe_end)
                .last()
                .unwrap_or(0)
        };
        let raw_prefix = &state.input_buffer()[..safe_end];
        let cursor_char_index = collapse_image_markers(raw_prefix).chars().count();

        let prompt_style =
            get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker);
        (lines, cursor_dx, cursor_dy) =
            wrap_input_chars(&styled_chars, inner_width, cursor_char_index, prompt_style);
    }

    let text_area_height = input_inner.height;
    let text_area = input_inner;
    let paragraph = Paragraph::new(lines).style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(paragraph, text_area);

    if inner_width > 0 && !show_picker {
        f.set_cursor_position((
            input_inner.x + cursor_dx.min(input_inner.width.saturating_sub(1)),
            input_inner.y + cursor_dy.min(text_area_height.saturating_sub(1)),
        ));
    }

    input_margin
}

pub(super) fn composer_footer_visible(
    state: &RenderSnapshot,
    has_command_completions: bool,
    has_file_completions: bool,
) -> bool {
    !state.modal_open() && !has_command_completions && !has_file_completions
}

pub(super) fn footer_location(state: &RenderSnapshot) -> String {
    let (path, branch) = state
        .cwd_and_branch()
        .rsplit_once(':')
        .unwrap_or((&state.cwd_and_branch(), "unknown"));
    let branch = if branch.is_empty() { "unknown" } else { branch };
    let branch = fit_to_width(branch, 24).trim_end().to_string();
    let path = if path.is_empty() { "~" } else { path };
    format!("{branch} · {path}")
}

pub(super) fn render_composer_footer(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: &RenderSnapshot,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let (used, _) = context_usage(state);
    let window = state.active_context_window().max(1);
    let remaining = crate::app::status::context_remaining_percent(used, window);
    let location = footer_location(state);
    let (left_content, left_style) = if state.ctrl_c_exit_armed() {
        (
            "  ⚠ Press Ctrl+C again to exit".to_owned(),
            get_themed_style(Color::Yellow, COLOR_BG(), Modifier::BOLD, false),
        )
    } else if let Some(agent) = state.selected_subagent() {
        (
            format!("  {} · {} · {}", agent.name(), state.model_name(), location),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        )
    } else {
        (
            format!("  {} · {}", state.model_name(), location),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        )
    };
    let right = format!("{remaining}% context left  ");
    let right_style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false);
    let left = fit_to_width(
        &left_content,
        (area.width as usize).saturating_sub(right.width()),
    );
    let padding = (area.width as usize).saturating_sub(left.width() + right.width());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(left, left_style),
            Span::styled(" ".repeat(padding), Style::default().bg(COLOR_BG())),
            Span::styled(right, right_style),
        ]))
        .style(Style::default().bg(COLOR_BG())),
        area,
    );
}
