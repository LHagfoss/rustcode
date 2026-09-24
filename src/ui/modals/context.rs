use super::*;

pub(in crate::ui) fn render_theme_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 12);
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
            Constraint::Min(6),    // Theme list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Select theme (live preview)";
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

    let themes = crate::ui::theme::load_available_themes();
    let selected_idx = state
        .theme_picker_index()
        .min(themes.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, theme) in themes.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_active = state
            .theme_picker_initial()
            .eq_ignore_ascii_case(&theme.name);
        let active_badge = if is_active { " (active)" } else { "" };
        let full_desc = format!("{}{}", theme.description, active_badge);
        let line = if is_selected {
            let left_text = format!(" ● {}", theme.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + full_desc.width());
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
                    full_desc,
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let left_text = format!("   {}", theme.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + full_desc.width());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(full_desc, Style::default().fg(COLOR_MUTED())),
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
        Span::styled("preview ", Style::default().fg(COLOR_TEXT())),
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

#[derive(Debug, Clone)]
pub struct ContextBreakdown {
    pub model_name: String,
    pub context_window: usize,
    pub current_usage: super::super::context_usage::ContextUsage,
    pub user_tokens: usize,
    pub assistant_tokens: usize,
    pub tool_tokens: usize,
    pub system_prompt_tokens: usize,
    pub system_tools_tokens: usize,
    pub skills_tokens: usize,
    pub subagent_tokens: usize,
    pub prompt_headroom_tokens: usize,
}

pub fn calculate_context_breakdown(state: &RenderSnapshot) -> ContextBreakdown {
    let context_window = state.active_context_window() as usize;
    let current_usage = super::super::context_usage::context_usage(state);

    let mut user_tokens = 0;
    let mut assistant_tokens = 0;
    let mut tool_tokens = 0;

    for msg in state.active_history() {
        match msg.role.as_str() {
            "user" => {
                user_tokens += crate::network::compaction::estimate_tokens(&msg.content);
            }
            "assistant" => {
                assistant_tokens += crate::network::compaction::estimate_tokens(&msg.content);
                if !msg.tool_calls.is_empty() {
                    if let Ok(tc_str) = serde_json::to_string(&msg.tool_calls) {
                        tool_tokens += crate::network::compaction::estimate_tokens(&tc_str);
                    }
                }
            }
            "tool" => {
                tool_tokens += crate::network::compaction::estimate_tokens(&msg.content);
                if let Some(ref id) = msg.tool_call_id {
                    tool_tokens += crate::network::compaction::estimate_tokens(id);
                }
            }
            _ => {}
        }
    }

    let protocol = state
        .config()
        .models
        .iter()
        .find(|m| m.url == state.api_base_url())
        .and_then(|m| m.tool_protocol)
        .unwrap_or(state.config().tool_protocol);
    let agent_mode = state.agent_mode();
    let tools_prompt =
        crate::tools::tool_system_prompt(state.delegation_active(), protocol, agent_mode);
    let full_system_prompt_tokens = crate::network::compaction::estimate_tokens(&tools_prompt);

    let skills = crate::skills::discover_skills();
    let skills_str = skills
        .iter()
        .map(|s| format!("{} {}", s.name, s.description))
        .collect::<Vec<_>>()
        .join(" ");
    let skills_tokens = if skills.is_empty() {
        0
    } else {
        crate::network::compaction::estimate_tokens(&skills_str)
    };

    let system_tools_tokens = full_system_prompt_tokens.saturating_sub(skills_tokens) / 2;
    let system_prompt_tokens = full_system_prompt_tokens
        .saturating_sub(system_tools_tokens)
        .saturating_sub(skills_tokens);

    let subagent_tokens: usize = if state.selected_subagent_id().is_none() {
        state
            .subagents()
            .iter()
            .map(crate::ui::render_snapshot::SubAgentSnapshot::history_tokens)
            .sum()
    } else {
        0
    };

    let prompt_headroom_tokens = context_window.saturating_sub(current_usage.used_tokens as usize);

    ContextBreakdown {
        model_name: state.model_name().to_owned(),
        context_window,
        current_usage,
        user_tokens,
        assistant_tokens,
        tool_tokens,
        system_prompt_tokens,
        system_tools_tokens,
        skills_tokens,
        subagent_tokens,
        prompt_headroom_tokens,
    }
}

pub(super) fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub(in crate::ui) fn render_context_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    // The stats column needs twelve content rows. Reserve one row above the
    // header, then size the panel so the content reaches its bottom edge.
    let modal_area = input_anchor_rect(f, input_area, 14);
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        modal_area,
    );

    let inner_area = modal_area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Top padding
            Constraint::Length(1), // Header
            Constraint::Min(6),    // Content
        ])
        .split(inner_area);

    let title_text = "context usage";
    let right_esc = "Esc to close";
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
        modal_chunks[1],
    );

    let breakdown = calculate_context_breakdown(state);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(56), // Matrix grid
            Constraint::Length(2),      // Spacer
            Constraint::Percentage(42), // Stats breakdown
        ])
        .split(modal_chunks[2]);

    let mut grid_area = cols[0];
    grid_area.y = grid_area.y.saturating_add(1);
    grid_area.height = grid_area.height.saturating_sub(1);
    let stats_area = cols[2];

    // Build the matrix of token dots/blocks
    let grid_w = (grid_area.width as usize).max(4);
    let grid_h = (grid_area.height as usize).max(2);
    let cols_per_row = (grid_w / 2).max(1);
    let total_blocks = cols_per_row * grid_h;

    let window = breakdown.context_window.max(1) as f64;
    let compute_blocks = |tokens: usize| -> usize {
        if tokens == 0 {
            0
        } else {
            let b = ((tokens as f64 / window) * (total_blocks as f64)).round() as usize;
            b.max(1)
        }
    };

    let prompt_b = compute_blocks(breakdown.current_usage.used_tokens as usize);
    let free_b = total_blocks.saturating_sub(prompt_b);

    let color_prompt = Color::Rgb(100, 160, 255);
    let color_asst = Color::Rgb(120, 220, 120);
    let color_tool = Color::Rgb(240, 200, 100);
    let color_sys_p = Color::Rgb(140, 180, 220);
    let color_sys_t = Color::Rgb(170, 175, 190);
    let color_skill = Color::Rgb(210, 150, 240);
    let color_sub = Color::Rgb(100, 200, 200);
    let color_free = Color::Rgb(80, 95, 110);

    let mut dot_spans: Vec<Span<'static>> = Vec::with_capacity(total_blocks);

    let mut push_dots = |count: usize, ch: &'static str, color: Color| {
        for _ in 0..count {
            dot_spans.push(Span::styled(
                ch,
                Style::default().fg(color).bg(COLOR_PANEL()),
            ));
        }
    };

    push_dots(prompt_b, "● ", color_prompt);
    push_dots(free_b, "□ ", color_free);

    while dot_spans.len() < total_blocks {
        dot_spans.push(Span::styled(
            "□ ",
            Style::default().fg(color_free).bg(COLOR_PANEL()),
        ));
    }
    dot_spans.truncate(total_blocks);

    let mut grid_lines: Vec<Line<'static>> = Vec::new();
    for chunk in dot_spans.chunks(cols_per_row) {
        grid_lines.push(Line::from(chunk.to_vec()));
    }

    f.render_widget(
        Paragraph::new(grid_lines).style(Style::default().bg(COLOR_PANEL())),
        grid_area,
    );

    // Right side breakdown stats
    let current_usage_pct = if breakdown.context_window > 0 {
        (breakdown.current_usage.used_tokens as f64 / breakdown.context_window as f64) * 100.0
    } else {
        0.0
    };

    let headroom_pct = if breakdown.context_window > 0 {
        (breakdown.prompt_headroom_tokens as f64 / breakdown.context_window as f64) * 100.0
    } else {
        0.0
    };

    let pct = |tokens: usize| -> f64 {
        if breakdown.context_window > 0 {
            (tokens as f64 / breakdown.context_window as f64) * 100.0
        } else {
            0.0
        }
    };

    let mut stats_lines: Vec<Line<'static>> = Vec::new();

    // Model and overall usage header
    stats_lines.push(Line::default());
    stats_lines.push(Line::from(vec![
        Span::styled(
            format!("{} · ", breakdown.model_name),
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{}/{} ({:.1}%) {}",
                format_token_count(breakdown.current_usage.used_tokens as usize),
                format_token_count(breakdown.context_window),
                current_usage_pct,
                match breakdown.current_usage.source {
                    super::super::context_usage::ContextUsageSource::ProviderPrompt => {
                        "prompt"
                    }
                    super::super::context_usage::ContextUsageSource::HistoryEstimate => "estimate",
                }
            ),
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    stats_lines.push(Line::default());
    stats_lines.push(Line::from(vec![Span::styled(
        "Saved history estimate · % of context window",
        Style::default().fg(COLOR_MUTED()),
    )]));

    // Category breakdown lines
    let categories = [
        (
            "●",
            color_prompt,
            "User messages",
            breakdown.user_tokens,
            pct(breakdown.user_tokens),
            true,
        ),
        (
            "●",
            color_asst,
            "Agent responses",
            breakdown.assistant_tokens,
            pct(breakdown.assistant_tokens),
            true,
        ),
        (
            "●",
            color_tool,
            "Tool calls",
            breakdown.tool_tokens,
            pct(breakdown.tool_tokens),
            true,
        ),
        (
            "⛃",
            color_sys_p,
            "System prompt",
            breakdown.system_prompt_tokens,
            pct(breakdown.system_prompt_tokens),
            true,
        ),
        (
            "⛃",
            color_sys_t,
            "System tools",
            breakdown.system_tools_tokens,
            pct(breakdown.system_tools_tokens),
            true,
        ),
        (
            "⛃",
            color_skill,
            "Skills",
            breakdown.skills_tokens,
            pct(breakdown.skills_tokens),
            true,
        ),
        (
            "⛃",
            color_sub,
            "Subagents",
            breakdown.subagent_tokens,
            pct(breakdown.subagent_tokens),
            true,
        ),
    ];

    for (icon, color, label, count, percent, include_tokens_word) in categories {
        let count_str = if include_tokens_word {
            format!(": {} tokens ({:.1}%)", format_token_count(count), percent)
        } else {
            format!(": {} ({:.1}%)", format_token_count(count), percent)
        };

        stats_lines.push(Line::from(vec![
            Span::styled(
                format!("{icon} "),
                Style::default().fg(color).bg(COLOR_PANEL()),
            ),
            Span::styled(label, Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL())),
            Span::styled(
                count_str,
                Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL()),
            ),
        ]));
    }

    stats_lines.push(Line::from(vec![
        Span::styled("□ ", Style::default().fg(color_free).bg(COLOR_PANEL())),
        Span::styled(
            format!(
                "{}: {} ({:.1}%)",
                match breakdown.current_usage.source {
                    super::super::context_usage::ContextUsageSource::ProviderPrompt => {
                        "Latest prompt headroom"
                    }
                    super::super::context_usage::ContextUsageSource::HistoryEstimate => {
                        "Estimated headroom"
                    }
                },
                format_token_count(breakdown.prompt_headroom_tokens),
                headroom_pct
            ),
            Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL()),
        ),
    ]));
    f.render_widget(
        Paragraph::new(stats_lines).style(Style::default().bg(COLOR_PANEL())),
        stats_area,
    );
}
