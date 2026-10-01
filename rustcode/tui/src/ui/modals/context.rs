use super::*;

/// Rectangle containing the selectable body of the active read-only info
/// modal. The geometry mirrors the corresponding render function below, while
/// excluding panel borders and outer padding from mouse selection and copy.
pub(in crate::ui) fn panel_selection_surface(
    f: &Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) -> Option<(ratatui::layout::Rect, Vec<bool>)> {
    if state.show_context_modal() {
        let area = input_anchor_rect(f, input_area, CONTEXT_MODAL_HEIGHT);
        let inner = area.inner(Margin {
            vertical: 0,
            horizontal: 2,
        });
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(6),
            ])
            .split(inner);
        let area = chunks[2];
        return Some((area, vec![false; usize::from(area.height)]));
    }

    let height = if state.show_status_modal() {
        STATUS_MODAL_HEIGHT
    } else if state.show_stats_modal() {
        STATS_MODAL_HEIGHT
    } else if state.show_session_modal() {
        SESSION_MODAL_HEIGHT
    } else if state.command_panel().is_some() {
        let area = input_anchor_rect(f, input_area, super::panel::COMMAND_PANEL_HEIGHT);
        let inner = area
            .inner(Margin {
                vertical: 1,
                horizontal: 0,
            })
            .inner(Margin {
                vertical: 0,
                horizontal: 2,
            });
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);
        let body = chunks[2];
        let Some(panel) = state.command_panel() else {
            return None;
        };
        let lines = super::panel::render_panel_content(&panel.content, inner.width as usize);
        let scroll = usize::from(state.modal_scroll_row());
        let soft_wrap_before = lines
            .iter()
            .flat_map(|line| {
                let count = Paragraph::new(line.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(body.width.max(1))
                    .max(1);
                std::iter::once(false).chain(std::iter::repeat_n(true, count - 1))
            })
            .skip(scroll)
            .take(usize::from(body.height))
            .collect::<Vec<_>>();
        let mut visible_wraps = soft_wrap_before;
        visible_wraps.resize(usize::from(body.height), false);
        return Some((body, visible_wraps));
    } else {
        return None;
    };
    let inner = input_anchor_rect(f, input_area, height).inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    (inner.width > 0 && inner.height > 0).then_some((inner, vec![false; usize::from(inner.height)]))
}

pub(in crate::ui) fn render_status_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let area = input_anchor_rect(f, input_area, STATUS_MODAL_HEIGHT);
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let user_count = state.history().iter().filter(|m| m.role == "user").count();
    let assistant_count = state
        .history()
        .iter()
        .filter(|m| m.role == "assistant")
        .count();
    let tool_count = state.history().iter().filter(|m| m.role == "tool").count();
    let mut lines = vec![
        modal_header("Session status"),
        Line::default(),
        panel_line("Model       ", state.model_name(), PanelEmphasis::Normal),
        panel_line(
            "Session     ",
            state.active_session_id(),
            PanelEmphasis::Normal,
        ),
        panel_line(
            "Messages    ",
            &format!("{user_count} user · {assistant_count} assistant · {tool_count} tool calls"),
            PanelEmphasis::Normal,
        ),
    ];
    if let Some(usage) = state.current_token_usage() {
        lines.push(panel_line(
            "Last turn   ",
            &format!(
                "{} prompt + {} completion = {} tokens",
                usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
            ),
            PanelEmphasis::Normal,
        ));
    }
    render_modal_body(f, lines, inner);
}

pub(in crate::ui) fn render_stats_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let area = input_anchor_rect(f, input_area, STATS_MODAL_HEIGHT);
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let mut lines = vec![modal_header("Token usage")];
    match state.current_token_usage() {
        Some(usage) => {
            lines.push(panel_line(
                "Last turn   ",
                &format!(
                    "{} prompt + {} completion = {} tokens",
                    usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
                ),
                PanelEmphasis::Normal,
            ));
        }
        None => lines.push(panel_line(
            "Last turn   ",
            "no token data yet",
            PanelEmphasis::Normal,
        )),
    }
    if let Some(rt) = state.response_time() {
        lines.push(panel_line(
            "Latency     ",
            &format!("{:.1}s last response", rt.as_secs_f32()),
            PanelEmphasis::Normal,
        ));
    }
    let usage_history = state.stats_usage_history();
    if usage_history.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(
            "Monthly usage appears after your first request.",
        ));
    } else {
        lines.push(Line::default());
        lines.push(Line::from("Monthly usage"));
        for (month, stats) in usage_history.iter().rev().take(4) {
            lines.push(panel_line(
                &format!("  {month}   "),
                &format!(
                    "{} total tokens · {} calls",
                    thousands(stats.total_tokens),
                    thousands(stats.calls)
                ),
                PanelEmphasis::Normal,
            ));
        }
    }

    render_modal_body(f, lines, inner);
}

pub(in crate::ui) fn render_session_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let area = input_anchor_rect(f, input_area, SESSION_MODAL_HEIGHT);
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let user_count = state.history().iter().filter(|m| m.role == "user").count();
    let assistant_count = state
        .history()
        .iter()
        .filter(|m| m.role == "assistant")
        .count();
    let lines = vec![
        modal_header("Session"),
        Line::default(),
        panel_line(
            "ID         ",
            state.active_session_id(),
            PanelEmphasis::Normal,
        ),
        panel_line("Model      ", state.model_name(), PanelEmphasis::Normal),
        panel_line(
            "Messages   ",
            &format!("{user_count} user · {assistant_count} assistant"),
            PanelEmphasis::Normal,
        ),
    ];

    render_modal_body(f, lines, inner);
}

/// Title row shared by the read-only info modals: bold title plus the key that
/// dismisses the panel.
pub(in crate::ui) fn modal_header(title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            title.to_owned(),
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  esc to close", Style::default().fg(COLOR_MUTED())),
    ])
}

fn render_modal_body(f: &mut Frame, lines: Vec<Line<'static>>, inner: ratatui::layout::Rect) {
    f.render_widget(
        Paragraph::new(lines).style(Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL())),
        inner,
    );
}

/// Thousands separators, so large monthly token counts stay readable.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub(in crate::ui) fn render_theme_picker_modal(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, THEME_PICKER_HEIGHT);
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
    let padding_header = picker_header_padding(inner_area.width as usize, &title_text, right_esc);
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
            let left_text = format!("› {}", theme.name);
            let padding_len = picker_row_padding(inner_area.width as usize, &left_text, &full_desc);
            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default()
                        .fg(COLOR_TEXT())
                        .bg(COLOR_HOVER_BG())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ".repeat(padding_len),
                    Style::default().fg(COLOR_TEXT()).bg(COLOR_HOVER_BG()),
                ),
                Span::styled(
                    full_desc,
                    Style::default().fg(COLOR_TEXT()).bg(COLOR_HOVER_BG()),
                ),
            ])
        } else {
            let left_text = format!("  {}", theme.name);
            let padding_len = picker_row_padding(inner_area.width as usize, &left_text, &full_desc);
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
    let scroll_y = picker_list_window(selected_idx, total_lines, list_height);
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
                user_tokens += rustcode::controller::estimate_tokens(&msg.content);
            }
            "assistant" => {
                assistant_tokens += rustcode::controller::estimate_tokens(&msg.content);
                if !msg.tool_calls.is_empty() {
                    if let Ok(tc_str) = serde_json::to_string(&msg.tool_calls) {
                        tool_tokens += rustcode::controller::estimate_tokens(&tc_str);
                    }
                }
            }
            "tool" => {
                tool_tokens += rustcode::controller::estimate_tokens(&msg.content);
                if let Some(ref id) = msg.tool_call_id {
                    tool_tokens += rustcode::controller::estimate_tokens(id);
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
        rustcode::controller::tool_system_prompt(state.delegation_active(), protocol, agent_mode);
    let full_system_prompt_tokens = rustcode::controller::estimate_tokens(&tools_prompt);

    let skills = rustcode::controller::discover_skills();
    let skills_str = skills
        .iter()
        .map(|s| format!("{} {}", s.name, s.description))
        .collect::<Vec<_>>()
        .join(" ");
    let skills_tokens = if skills.is_empty() {
        0
    } else {
        rustcode::controller::estimate_tokens(&skills_str)
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
    let modal_area = input_anchor_rect(f, input_area, CONTEXT_MODAL_HEIGHT);
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
    let padding_header = picker_header_padding(inner_area.width as usize, &title_text, right_esc);
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

    // Category colors come from the active theme (see `panel::context_category_colors`)
    // so the panel matches the themed UI in light and dark modes alike.
    let cat_colors = context_category_colors();
    let color_user = cat_colors[0];
    let color_asst = cat_colors[1];
    let color_tool = cat_colors[2];
    let color_sys_p = cat_colors[3];
    let color_sys_t = cat_colors[4];
    let color_skill = cat_colors[5];
    let color_sub = cat_colors[6];
    let color_free = cat_colors[7];

    let mut dot_spans: Vec<Span<'static>> = Vec::with_capacity(total_blocks);

    let mut push_dots = |count: usize, ch: &'static str, color: Color| {
        for _ in 0..count {
            dot_spans.push(Span::styled(
                ch,
                Style::default().fg(color).bg(COLOR_PANEL()),
            ));
        }
    };

    // One grid segment per usage category, in legend order, so the matrix
    // shows the same breakdown as the stats column. Segments are capped at
    // the remaining cells so rounding never overflows the grid.
    let mut remaining = total_blocks;
    for (tokens, color) in [
        breakdown.user_tokens,
        breakdown.assistant_tokens,
        breakdown.tool_tokens,
        breakdown.system_prompt_tokens,
        breakdown.system_tools_tokens,
        breakdown.skills_tokens,
        breakdown.subagent_tokens,
    ]
    .into_iter()
    .zip([
        color_user,
        color_asst,
        color_tool,
        color_sys_p,
        color_sys_t,
        color_skill,
        color_sub,
    ]) {
        let segment = compute_blocks(tokens).min(remaining);
        push_dots(segment, "● ", color);
        remaining = remaining.saturating_sub(segment);
    }
    push_dots(remaining, "□ ", color_free);

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

    // Model and overall usage header. The summary value turns Strong once
    // overall usage crosses HIGH_USAGE_PCT so a nearly-full window stands out.
    let summary_text = format!(
        "{}/{} ({:.1}%) {}",
        format_token_count(breakdown.current_usage.used_tokens as usize),
        format_token_count(breakdown.context_window),
        current_usage_pct,
        match breakdown.current_usage.source {
            super::super::context_usage::ContextUsageSource::ProviderPrompt => "prompt",
            super::super::context_usage::ContextUsageSource::HistoryEstimate => "estimate",
        }
    );
    let summary_emphasis = if current_usage_pct >= HIGH_USAGE_PCT {
        PanelEmphasis::Strong
    } else {
        PanelEmphasis::Normal
    };
    let mut summary_spans = vec![Span::styled(
        format!("{} · ", breakdown.model_name),
        Style::default()
            .fg(COLOR_TEXT())
            .add_modifier(Modifier::BOLD),
    )];
    summary_spans.extend(panel_value_spans(
        &summary_text,
        summary_emphasis,
        Style::default()
            .fg(COLOR_TEXT())
            .add_modifier(Modifier::BOLD),
    ));
    stats_lines.push(Line::default());
    stats_lines.push(Line::from(summary_spans));

    stats_lines.push(Line::default());
    stats_lines.push(Line::from(vec![Span::styled(
        "Saved history estimate · % of context window",
        Style::default().fg(COLOR_MUTED()),
    )]));

    // Category breakdown lines. Every swatch uses the same ● glyph as the
    // grid; over-threshold categories (see OVER_THRESHOLD_PCT) get an
    // emphasized value through the shared panel markup helper.
    let categories = [
        (
            "●",
            color_user,
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
            "●",
            color_sys_p,
            "System prompt",
            breakdown.system_prompt_tokens,
            pct(breakdown.system_prompt_tokens),
            true,
        ),
        (
            "●",
            color_sys_t,
            "System tools",
            breakdown.system_tools_tokens,
            pct(breakdown.system_tools_tokens),
            true,
        ),
        (
            "●",
            color_skill,
            "Skills",
            breakdown.skills_tokens,
            pct(breakdown.skills_tokens),
            true,
        ),
        (
            "●",
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

        let mut row_spans = vec![
            Span::styled(
                format!("{icon} "),
                Style::default().fg(color).bg(COLOR_PANEL()),
            ),
            Span::styled(label, Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL())),
        ];
        row_spans.extend(panel_value_spans(
            &count_str,
            emphasis_for_share(percent),
            Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL()),
        ));
        stats_lines.push(Line::from(row_spans));
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
