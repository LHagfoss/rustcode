use super::*;

const STATUS_PANEL_MIN_WIDTH: usize = 40;

fn status_panel_title(content: &str) -> Option<&'static str> {
    let first_line = content.lines().find(|line| !line.trim().is_empty())?;
    let first_line = first_line.trim().trim_end_matches(':');
    let lower = first_line.to_ascii_lowercase();

    if lower.starts_with("session usage") {
        Some("Usage")
    } else if lower.starts_with("session status") {
        Some("Status")
    } else if lower.starts_with("session id") {
        Some("Session")
    } else if lower == "rustcode info" || lower.starts_with("about rustcode") {
        Some("Info")
    } else if lower.starts_with("available commands")
        || lower.starts_with("core & session")
        || lower.starts_with("help & commands")
    {
        Some("Help")
    } else if lower.starts_with("discovered skills") {
        Some("Skills")
    } else if lower.starts_with("available themes") {
        Some("Themes")
    } else if lower.contains("model quota status") || lower.starts_with("quota") {
        Some("Quota")
    } else {
        None
    }
}

fn is_status_panel_heading(line: &str, title: Option<&str>) -> bool {
    let normalized = line.trim().trim_end_matches(':').to_ascii_lowercase();
    match title {
        Some("Usage") => normalized == "session usage",
        Some("Info") => normalized == "rustcode info" || normalized == "about rustcode",
        Some("Help") => {
            normalized == "available commands"
                || normalized == "core & session"
                || normalized == "help & commands"
        }
        Some("Skills") => normalized == "discovered skills",
        Some("Themes") => normalized == "available themes",
        _ => false,
    }
}

fn status_panel_line_width(line: &str) -> usize {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return 0;
    }

    if trimmed.starts_with('/') {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let command = parts.first().copied().unwrap_or_default();
        let description = parts.get(1..).unwrap_or_default().join(" ");
        return format!("  {:<18}{}", command, description).width();
    }

    if trimmed.starts_with("Enter")
        || trimmed.starts_with("Shift+")
        || trimmed.starts_with("Esc")
        || trimmed.starts_with("Up/Down")
        || trimmed.starts_with("Ctrl+")
        || trimmed.starts_with("Alt+")
        || trimmed.starts_with('?')
    {
        let parts: Vec<&str> = trimmed.splitn(2, "  ").collect();
        let key = parts.first().copied().unwrap_or_default().trim();
        let description = parts.get(1).map(|part| part.trim()).unwrap_or_default();
        return format!("  {:<18}{}", key, description).width();
    }

    if trimmed.starts_with('•') || trimmed.starts_with('-') {
        let bullet_text = trimmed
            .trim_start_matches('•')
            .trim_start_matches('-')
            .trim();
        return format!("  • {bullet_text}").width();
    }

    format!("  {trimmed}").width()
}

fn status_panel_content_width(content: &str, title: Option<&str>, available_width: usize) -> usize {
    let body_width = content
        .lines()
        .filter(|line| !is_status_panel_heading(line, title))
        .map(status_panel_line_width)
        .max()
        .unwrap_or_default();
    let title_width = title
        .map(|title| format!(">_ RustCode · {title}").width() + 1)
        .unwrap_or_default();
    let desired = STATUS_PANEL_MIN_WIDTH
        .saturating_sub(4)
        .max(body_width)
        .max(title_width);
    available_width.saturating_sub(6).max(1).min(desired)
}

fn agent_activity_summary(content: &str) -> Option<String> {
    let content = content.strip_prefix("agent-")?;
    let (id, activity) = content.split_once(' ')?;
    if id.is_empty() || !id.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let activity = activity.split_whitespace().collect::<Vec<_>>().join(" ");
    let summary = if let Some(tool_activity) = activity.strip_prefix("→ ") {
        let (tool_name, target) = tool_activity.split_once(' ').unwrap_or((tool_activity, ""));
        let (action, _) = super::format_pi_tool_action(tool_name, &serde_json::Value::Null, None);
        let home = std::env::var("HOME").ok();
        let target = super::contract_home_path(target, home.as_deref());
        if target.is_empty() {
            format!("Agent {id} · {action}")
        } else {
            format!("Agent {id} · {action} {target}")
        }
    } else if let Some(follow_up) = activity.strip_prefix("← follow-up (") {
        format!(
            "Agent {id} · follow-up: {}",
            follow_up.trim_end_matches(')')
        )
    } else if let Some(spawned) = activity.strip_prefix("spawned: ") {
        let task = spawned
            .split_once(" (write_access=")
            .map_or(spawned, |(task, _)| task);
        format!("Agent {id} spawned · {task}")
    } else {
        format!("Agent {id} {activity}")
    };
    Some(summary)
}

fn truncate_display_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_owned();
    }
    let budget = max_width.saturating_sub(1);
    let mut output = String::new();
    let mut used = 0;
    for character in text.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or_default();
        if used + width > budget {
            break;
        }
        used += width;
        output.push(character);
    }
    if max_width > 0 {
        output.push('…');
    }
    output
}

fn is_compact_tool_batch_notice(content: &str) -> bool {
    content.starts_with("Some tool calls were queued;")
        || content.starts_with("Some tool calls were not run;")
}

pub(super) fn render_status_panel<'a>(
    content: &str,
    width: u16,
    show_picker: bool,
    lines: &mut Vec<Line<'a>>,
) {
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
        push_left_aligned_separator(lines, "User Stopped", width, show_picker);
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
    } else if content.contains("[Recoverable provider interruption:") {
        Some(
            "Provider stream interrupted — output saved safely; no tool was replayed (send `continue` or use --resume)",
        )
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

    if let Some(summary) = agent_activity_summary(content)
        .or_else(|| is_compact_tool_batch_notice(content).then(|| content.to_owned()))
    {
        if width == 0 {
            return;
        }
        let prefix = if width > 1 { "  " } else { " " };
        let summary =
            truncate_display_width(&summary, (width as usize).saturating_sub(prefix.width()));
        lines.push(Line::from(vec![
            Span::styled(
                prefix,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                summary,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ]));
        return;
    }

    let is_info_notice = lower.starts_with("session status")
        || lower.starts_with("session usage")
        || lower.starts_with("session id")
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
            let (marker, color, marker_weight) = if is_warning {
                ("! ", COLOR_TIP(), Modifier::BOLD)
            } else {
                ("  ", COLOR_MUTED(), Modifier::empty())
            };
            let text_style = get_themed_style(color, COLOR_BG(), Modifier::empty(), show_picker);
            // Wrapped here so the rows after the first stay under the text
            // instead of starting at the left edge, under the marker.
            let mut wrapped = Vec::new();
            push_wrapped_with_continuation(
                &mut wrapped,
                vec![
                    Span::styled(
                        marker,
                        get_themed_style(color, COLOR_BG(), marker_weight, show_picker),
                    ),
                    Span::styled(trimmed.to_string(), text_style),
                ],
                usize::from(width).max(4),
                Some(Span::styled("  ", text_style)),
            );
            lines.extend(wrapped);
        }
        return;
    }

    let border_c = COLOR_PRIMARY();
    let reset_bg = COLOR_BG();

    let panel_title = status_panel_title(content);
    let content_w = status_panel_content_width(content, panel_title, width as usize);
    let box_w = content_w.saturating_add(4);
    let inner_w = box_w.saturating_sub(2);

    let title_str = panel_title
        .map(|title| format!(">_ RustCode · {title}"))
        .unwrap_or_else(|| format!(">_ RustCode v{}", env!("CARGO_PKG_VERSION")));
    let title_str = fit_to_width(&title_str, inner_w.saturating_sub(3))
        .trim_end()
        .to_owned();
    let top_pad = inner_w.saturating_sub(title_str.width() + 3);
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
        if is_status_panel_heading(trimmed, panel_title) {
            continue;
        }

        if trimmed.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(border_c).bg(reset_bg)),
                Span::styled(" ".repeat(content_w), Style::default().bg(reset_bg)),
                Span::styled(" │", Style::default().fg(border_c).bg(reset_bg)),
            ]));
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
        "YOLO mode enabled" => Some("YOLO mode enabled"),
        "YOLO mode disabled" => Some("YOLO mode disabled"),
        _ => None,
    }
}

pub(crate) fn build_claude_startup_banner_snapshot(
    state: &RenderSnapshot,
    total_width: usize,
    _max_height: usize,
) -> Vec<Line<'static>> {
    if total_width < 8 {
        return vec![Line::from("")];
    }

    let mut banner = Vec::new();
    let version = env!("CARGO_PKG_VERSION");
    let model_name = model_label(state);

    let box_w = total_width.saturating_sub(2).min(66);
    let inner_w = box_w.saturating_sub(2);

    let border_c = COLOR_PRIMARY();
    let primary = COLOR_PRIMARY();
    let text_c = COLOR_TEXT();
    let muted_c = COLOR_MUTED();
    let reset_bg = COLOR_BG();

    // Top border
    let title_str = fit_to_width(
        &format!(">_ RustCode v{version}"),
        inner_w.saturating_sub(3),
    )
    .trim_end()
    .to_owned();
    let top_pad = inner_w.saturating_sub(title_str.width() + 3);
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
            used += s.content.width();
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

    // Blank padding above the session row (the legacy boxed panel had this;
    // the compact rewrite dropped it).
    banner.push(make_row(vec![]));

    // Session identity is useful when copying a report or resuming a run, so
    // keep it visually separate from the mutable model/workspace settings.
    // Keep the welcome card's content comfortably away from the border on
    // roomy terminals, while retaining a compact layout on narrow ones.
    let label_indent = if total_width >= 80 { "    " } else { "  " };
    let label_w = if total_width >= 80 { 17 } else { 15 }.min(inner_w);
    let label = |name: &str| format!("{label_indent}{name}:");
    let session_id = fit_to_width(state.active_session_id(), inner_w.saturating_sub(label_w));
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width(&label("session"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            session_id,
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Model and reasoning settings. Keep each hint beside its value when it
    // fits; dropping the hint is preferable to letting it touch the border.
    let model_display = fit_to_width(&model_name, inner_w.saturating_sub(label_w))
        .trim_end()
        .to_owned();
    let model_width = model_display.width();

    let effort = state
        .active_model_profile()
        .and_then(|profile| profile.reasoning_effort.clone())
        .unwrap_or_else(|| "default".to_string());
    let effort_display = fit_to_width(&effort, inner_w.saturating_sub(label_w))
        .trim_end()
        .to_owned();
    let effort_width = effort_display.width();

    let context_window = format!(
        "{} tokens",
        format_context_window(u64::from(state.active_context_window()))
    );
    let context_display = fit_to_width(&context_window, inner_w.saturating_sub(label_w))
        .trim_end()
        .to_owned();
    let context_width = context_display.width();

    // Keep the command hints in a stable second column. The values are all
    // rendered after the fixed label column, so the longest displayed value
    // determines where every hint starts.
    let hint_column = label_w + model_width.max(effort_width).max(context_width) + 4;
    let append_change_hint = |spans: &mut Vec<Span<'static>>, left_width: usize, command: &str| {
        let hint_width = command.width() + " to change".width();
        if hint_column <= inner_w && hint_column + hint_width <= inner_w {
            spans.push(Span::styled(
                " ".repeat(hint_column.saturating_sub(left_width)),
                Style::default().bg(reset_bg),
            ));
            spans.extend([
                Span::styled(
                    command.to_owned(),
                    Style::default().fg(primary).bg(reset_bg),
                ),
                Span::styled(" to change", Style::default().fg(muted_c).bg(reset_bg)),
            ]);
        }
    };

    let mut model_spans = vec![
        Span::styled(
            fit_to_width(&label("model"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            model_display,
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    append_change_hint(&mut model_spans, label_w + model_width, "/model");
    banner.push(make_row(model_spans));

    let mut effort_spans = vec![
        Span::styled(
            fit_to_width(&label("effort"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            effort_display,
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    append_change_hint(&mut effort_spans, label_w + effort_width, "/effort");
    banner.push(make_row(effort_spans));

    let mut context_spans = vec![
        Span::styled(
            fit_to_width(&label("context"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            context_display.clone(),
            Style::default()
                .fg(text_c)
                .bg(reset_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    append_change_hint(&mut context_spans, label_w + context_width, "/context");
    banner.push(make_row(context_spans));

    // Workspace location
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
            fit_to_width(&label("directory"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(dir_fitted, Style::default().fg(text_c).bg(reset_bg)),
    ]));

    let branch_name = state
        .cwd_and_branch()
        .rsplit_once(':')
        .map(|(_, branch)| branch)
        .filter(|branch| !branch.is_empty())
        .unwrap_or("unknown");
    let branch_fitted = fit_to_width(branch_name, inner_w.saturating_sub(label_w));
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width(&label("branch"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(branch_fitted, Style::default().fg(text_c).bg(reset_bg)),
    ]));

    // Access mode
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
            fit_to_width(&label("permissions"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            fit_to_width(perm_text, inner_w.saturating_sub(label_w))
                .trim_end()
                .to_owned(),
            perm_style,
        ),
    ]));

    let sandbox_display = fit_to_width(
        rustcode::controller::sandbox_effective_description(
            state.config().sandbox_mode.effective(state.auto_confirm()),
        ),
        inner_w.saturating_sub(label_w),
    )
    .trim_end()
    .to_owned();
    banner.push(make_row(vec![
        Span::styled(
            fit_to_width(&label("OS sandbox"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(sandbox_display, Style::default().fg(text_c).bg(reset_bg)),
    ]));

    // Help gets its own row because it explains the command interface rather
    // than changing one of the values above.
    // Start the explanatory help copy immediately after its label. Unlike the
    // value hints above, this row needs the extra room before `/help`.
    let help_column = label_w;
    let help_available = inner_w.saturating_sub(help_column);
    let mut help_spans = vec![
        Span::styled(
            fit_to_width(&label("help"), label_w),
            Style::default().fg(muted_c).bg(reset_bg),
        ),
        Span::styled(
            " ".repeat(help_column.saturating_sub(label_w)),
            Style::default().bg(reset_bg),
        ),
    ];
    let help_prefix = "run this command to get help: ";
    if help_prefix.width() + "/help".width() <= help_available {
        help_spans.push(Span::styled(
            help_prefix,
            Style::default().fg(muted_c).bg(reset_bg),
        ));
    }
    help_spans.push(Span::styled(
        fit_to_width("/help", help_available).trim_end().to_owned(),
        Style::default().fg(primary).bg(reset_bg),
    ));
    banner.push(make_row(help_spans));

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
