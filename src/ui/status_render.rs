use super::*;

pub(super) fn render_status_panel<'a>(
    content: &str,
    width: u16,
    show_picker: bool,
    lines: &mut Vec<Line<'a>>,
) {
    let version = env!("CARGO_PKG_VERSION");
    let lower = content.to_ascii_lowercase();

    if lower.starts_with("resumed session") {
        push_centered_separator(lines, "Resumed Session", width, show_picker);
        return;
    }
    if lower.contains("new chat started") {
        push_centered_separator(lines, "New Chat Started", width, show_picker);
        return;
    }
    if is_turn_cancelled_notice(content) {
        push_centered_separator(lines, "✕ Turn cancelled", width, show_picker);
        return;
    }
    if let Some(label) = yolo_mode_notice_label(content) {
        push_centered_separator(lines, label, width, show_picker);
        return;
    }

    // Convert verbose internal agent-steering prompts into concise, human-friendly status lines in the UI.
    let human_summary = if content.contains("stuck in a loop")
        || content.contains("CRITICAL — you are stuck in a loop")
    {
        Some("Repetitive tool loop detected — stopping tools and requesting final response")
    } else if content.contains("Your reasoning became repetitive")
        || content.contains("reasoning loop")
    {
        Some("Reasoning loop detected — continuing turn to take concrete action")
    } else if content.contains("Evidence-based recovery:")
        || content.contains("previous tool action repeated without making progress")
    {
        Some("Repetitive tool actions detected — nudging agent to make progress")
    } else if content.starts_with("[harness: failure replan") {
        Some("Repeated tool execution failures — requesting alternative strategy")
    } else {
        None
    };

    if let Some(summary) = human_summary {
        lines.push(Line::from(vec![
            Span::styled(
                "! ",
                get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
            Span::styled(
                summary.to_string(),
                get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ]));
        return;
    }

    let is_info_notice = lower.starts_with("session status")
        || lower.starts_with("session usage")
        || lower.starts_with("rustcode info")
        || lower.starts_with("about rustcode")
        || lower.starts_with("notice: rustcode")
        || lower.starts_with("rustcode help")
        || lower.starts_with("available commands")
        || lower.starts_with("core & session")
        || lower.starts_with("help & commands")
        || lower.starts_with("discovered skills")
        || lower.starts_with("available themes")
        || lower.contains("model quota status")
        || lower.starts_with("quota:");

    let is_warning = !is_info_notice
        && ["warning", "error", "failed", "blocked", "abort", "loop"]
            .iter()
            .any(|word| lower.contains(word));

    if !is_info_notice {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                lines.push(Line::from(""));
                continue;
            }
            if is_warning {
                lines.push(Line::from(vec![
                    Span::styled(
                        "! ",
                        get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::BOLD, show_picker),
                    ),
                    Span::styled(
                        trimmed.to_string(),
                        get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::empty(), show_picker),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        "  ",
                        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                    ),
                    Span::styled(
                        trimmed.to_string(),
                        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                    ),
                ]));
            }
        }
        return;
    }

    let border_c = COLOR_PRIMARY();
    let reset_bg = COLOR_BG();

    let box_w = (width as usize).saturating_sub(2).max(40);
    let inner_w = box_w.saturating_sub(2);
    let content_w = inner_w.saturating_sub(2);

    // Top border: ╭─ >_ RustCode v0.17.0 ──────────────────────────────────────────╮
    let title_str = format!(">_ RustCode v{version}");
    let top_pad = inner_w.saturating_sub(title_str.chars().count() + 3);
    let top_border = format!("╭─ {title_str} {}╮", "─".repeat(top_pad));
    lines.push(Line::from(vec![Span::styled(
        top_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));

    // Top blank padding line
    lines.push(Line::from(vec![
        Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
        Span::styled(" ".repeat(inner_w), Style::default().bg(reset_bg)),
        Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
    ]));

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("rustcode info") {
            continue;
        }

        let is_header = trimmed.ends_with(':')
            || trimmed.starts_with("📊")
            || trimmed.starts_with("📦")
            || trimmed.starts_with("🎨")
            || trimmed.starts_with("Core & Session")
            || trimmed.starts_with("Help & Commands")
            || trimmed.starts_with("Discovered Skills");

        if is_header {
            let padded_header = fit_to_width(&format!("  {trimmed}"), content_w);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(
                    padded_header,
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else if trimmed.starts_with('/') {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let cmd_name = parts.first().copied().unwrap_or("");
            let cmd_desc = if parts.len() > 1 {
                parts[1..].join(" ")
            } else {
                String::new()
            };
            let left_sp = format!("  {:<18}", cmd_name);
            let right_len = content_w.saturating_sub(left_sp.chars().count());
            let right_sp = fit_to_width(&cmd_desc, right_len);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(
                    left_sp,
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    right_sp,
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else if trimmed.starts_with("Enter")
            || trimmed.starts_with("Shift+")
            || trimmed.starts_with("Esc")
            || trimmed.starts_with("Up/Down")
            || trimmed.starts_with("Ctrl+")
            || trimmed.starts_with("Alt+")
            || trimmed.starts_with('?')
        {
            let parts: Vec<&str> = trimmed.splitn(2, "  ").collect();
            let key = parts.first().copied().unwrap_or("").trim();
            let desc = if parts.len() > 1 { parts[1].trim() } else { "" };
            let left_sp = format!("  {:<18}", key);
            let right_len = content_w.saturating_sub(left_sp.chars().count());
            let right_sp = fit_to_width(desc, right_len);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(
                    left_sp,
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    right_sp,
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else if trimmed.starts_with('•') || trimmed.starts_with('-') {
            let bullet_text = trimmed
                .trim_start_matches('•')
                .trim_start_matches('-')
                .trim();
            let full_str = format!("  • {bullet_text}");
            let padded_str = fit_to_width(&full_str, content_w);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(
                    padded_str,
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        } else {
            let full_str = format!("  {trimmed}");
            let padded_str = fit_to_width(&full_str, content_w);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(
                    padded_str,
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
        }
    }

    // Bottom blank padding line
    lines.push(Line::from(vec![
        Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
        Span::styled(" ".repeat(inner_w), Style::default().bg(reset_bg)),
        Span::styled("│", Style::default().fg(border_c).bg(reset_bg)),
    ]));

    // Bottom border: ╰──────────────────────────────────────────────────────────╯
    let bot_border = format!("╰{}╯", "─".repeat(inner_w));
    lines.push(Line::from(vec![Span::styled(
        bot_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));
}

pub(super) fn is_turn_cancelled_notice(content: &str) -> bool {
    content.trim() == "[harness: turn stopped — cancelled]"
}

pub(super) fn yolo_mode_notice_label(content: &str) -> Option<&'static str> {
    match content.trim() {
        "YOLO mode enabled" => Some("⚡ YOLO mode enabled"),
        "YOLO mode disabled" => Some("✕ YOLO mode disabled"),
        _ => None,
    }
}

pub(crate) fn build_claude_startup_banner_snapshot(
    state: &RenderSnapshot,
    total_width: usize,
    _max_height: usize,
) -> Vec<Line<'static>> {
    let mut banner = Vec::new();
    let version = env!("CARGO_PKG_VERSION");
    let model_name = model_label(state);

    let box_w = total_width.saturating_sub(2).min(66).max(45);
    let inner_w = box_w.saturating_sub(2);

    let border_c = COLOR_PRIMARY();
    let primary = COLOR_PRIMARY();
    let text_c = COLOR_TEXT();
    let muted_c = COLOR_MUTED();
    let reset_bg = COLOR_BG();

    // Top border
    let title_str = format!(">_ RustCode v{version}");
    let top_pad = inner_w.saturating_sub(title_str.chars().count() + 3);
    let top_border = format!("╭─ {title_str} {}╮", "─".repeat(top_pad));
    banner.push(Line::from(vec![Span::styled(
        top_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));

    let make_row = |spans: Vec<Span<'static>>| -> Line<'static> {
        let mut line_spans = Vec::new();
        line_spans.push(Span::styled(
            "│",
            Style::default().fg(border_c).bg(reset_bg),
        ));

        let mut used = 0;
        for s in &spans {
            used += s.content.chars().count();
        }
        line_spans.extend(spans);

        let pad = inner_w.saturating_sub(used);
        if pad > 0 {
            line_spans.push(Span::styled(" ".repeat(pad), Style::default().bg(reset_bg)));
        }
        line_spans.push(Span::styled(
            "│",
            Style::default().fg(border_c).bg(reset_bg),
        ));
        Line::from(line_spans)
    };

    // Blank line after title
    banner.push(make_row(vec![]));

    // Row 1: model
    let label_w = 15;
    let mut model_spans = vec![
        Span::styled(
            fit_to_width("  model:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            model_name.clone(),
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let used_for_model = label_w + model_name.chars().count();
    if inner_w >= used_for_model + 22 {
        model_spans.push(Span::styled("    ", Style::default().bg(reset_bg)));
        model_spans.push(Span::styled(
            "/model",
            Style::default().fg(primary).bg(reset_bg),
        ));
        model_spans.push(Span::styled(
            " to change",
            Style::default().fg(muted_c).bg(reset_bg),
        ));
    }
    banner.push(make_row(model_spans));

    // Row 2: reasoning effort
    let effort = state
        .active_model_profile()
        .and_then(|profile| profile.reasoning_effort.clone())
        .unwrap_or_else(|| "default".to_string());
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  effort:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            effort,
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("    ", Style::default().bg(reset_bg)),
        Span::styled("/effort", Style::default().fg(primary).bg(reset_bg)),
        Span::styled(" to change", Style::default().fg(muted_c).bg(reset_bg)),
    ]));

    // Row 3: context window
    let context_window = format!(
        "{} tokens",
        format_token_count(state.active_context_window())
    );
    let mut context_spans = vec![
        Span::styled(
            fit_to_width("  context:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            context_window.clone(),
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let used_for_context = label_w + context_window.chars().count();
    if inner_w >= used_for_context + 22 {
        context_spans.push(Span::styled("    ", Style::default().bg(reset_bg)));
        context_spans.push(Span::styled(
            "/context",
            Style::default().fg(primary).bg(reset_bg),
        ));
        context_spans.push(Span::styled(
            " to change",
            Style::default().fg(muted_c).bg(reset_bg),
        ));
    }
    banner.push(make_row(context_spans));

    // Row 4: directory
    let (dir_display, _) = state
        .cwd_and_branch()
        .rsplit_once(':')
        .unwrap_or((state.cwd_and_branch(), ""));
    let dir_display = if dir_display.is_empty() {
        "~"
    } else {
        dir_display
    };

    let max_dir_len = inner_w.saturating_sub(label_w + 1);
    let dir_fitted = fit_to_width(&dir_display, max_dir_len);
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  directory:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(dir_fitted, Style::default().fg(text_c).bg(reset_bg)),
    ]));

    // Row 5: branch
    let branch_name = state
        .cwd_and_branch()
        .rsplit_once(':')
        .map(|(_, branch)| branch)
        .filter(|branch| !branch.is_empty())
        .unwrap_or("unknown");
    let branch_fitted = fit_to_width(branch_name, inner_w.saturating_sub(label_w));
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  branch:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(branch_fitted, Style::default().fg(text_c).bg(reset_bg)),
    ]));

    // Row 6: permissions
    let (perm_text, perm_style) = if state.auto_confirm() {
        (
            "YOLO mode",
            Style::default()
                .fg(Color::Rgb(255, 125, 155))
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            "Interactive",
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        )
    };
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  permissions:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(perm_text, perm_style),
    ]));

    // Help shortcut
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width("  help:", label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled("/help", Style::default().fg(primary).bg(reset_bg)),
        Span::styled(" for commands", Style::default().fg(muted_c).bg(reset_bg)),
    ]));

    // Blank line before the bottom border
    banner.push(make_row(vec![]));

    // Bottom border
    let bot_border = format!("╰{}╯", "─".repeat(inner_w));
    banner.push(Line::from(vec![Span::styled(
        bot_border,
        Style::default().fg(border_c).bg(reset_bg),
    )]));

    // Padding below welcome message
    banner.push(Line::from(""));

    banner
}
