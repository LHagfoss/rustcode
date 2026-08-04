//! Modal, popup, picker and welcome-screen rendering for the TUI.
//!
//! Extracted from `ui/mod.rs`. Shared colour constants and small helpers
//! (`get_themed_style`, `model_label`, `count_input_lines`) live in the parent
//! module and are pulled in via the `super::*` glob; diff highlighting comes
//! from the sibling `highlight` module.

use super::highlight::highlight_diff_line;
use super::*;
use crate::app::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub(super) fn render_popup_menu(
    f: &mut Frame,
    state: &AppState,
    filtered_cmds: &[&CommandInfo],
    area: ratatui::layout::Rect,
) {
    // Only `area.height` rows fit above the input box; scroll the list so the
    // selected command stays visible instead of spilling over the prompt.
    let max_rows = (area.height as usize).max(1);
    let selected = state.active_suggestion_index.unwrap_or(0);
    let offset = if selected >= max_rows {
        selected + 1 - max_rows
    } else {
        0
    };

    let mut popup_lines = Vec::new();
    for (idx, cmd) in filtered_cmds.iter().enumerate().skip(offset).take(max_rows) {
        let is_selected = state
            .active_suggestion_index
            .map(|i| i == idx)
            .unwrap_or(false);

        let line = if is_selected {
            let left_text = format!("{:<12}   {}", cmd.name, cmd.desc);
            let total_len = left_text.len();
            let padding_len = (area.width as usize).saturating_sub(total_len);
            let full_text = format!("{}{}", left_text, " ".repeat(padding_len));

            Line::from(Span::styled(
                full_text,
                Style::default()
                    .fg(COLOR_BG())
                    .bg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            let left_text = format!("{:<12}   ", cmd.name);
            let desc_text = cmd.desc.to_string();
            let total_len = left_text.len() + desc_text.len();
            let padding_len = (area.width as usize).saturating_sub(total_len);

            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL()),
                ),
                Span::styled(
                    desc_text,
                    Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL()),
                ),
                Span::styled(" ".repeat(padding_len), Style::default().bg(COLOR_PANEL())),
            ])
        };
        popup_lines.push(line);
    }
    f.render_widget(
        Paragraph::new(popup_lines).style(Style::default().bg(COLOR_PANEL())),
        area,
    );
}

pub(super) fn render_at_popup_menu(
    f: &mut Frame,
    state: &AppState,
    file_matches: &[String],
    area: ratatui::layout::Rect,
) {
    let mut popup_lines = Vec::new();
    for (idx, file) in file_matches.iter().enumerate() {
        let is_selected = state
            .active_suggestion_index
            .map(|i| i == idx)
            .unwrap_or(false);

        let line = if is_selected {
            let left_text = format!("📄 {:<35}", file);
            let total_len = left_text.len();
            let padding_len = (area.width as usize).saturating_sub(total_len);
            let full_text = format!("{}{}", left_text, " ".repeat(padding_len));

            Line::from(Span::styled(
                full_text,
                Style::default()
                    .fg(COLOR_BG())
                    .bg(COLOR_SECONDARY())
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            let left_text = format!("📄 {:<35}", file);
            let padding_len = (area.width as usize).saturating_sub(left_text.len());

            Line::from(vec![
                Span::styled(
                    left_text,
                    Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL()),
                ),
                Span::styled(" ".repeat(padding_len), Style::default().bg(COLOR_PANEL())),
            ])
        };
        popup_lines.push(line);
    }
    f.render_widget(
        Paragraph::new(popup_lines).style(Style::default().bg(COLOR_PANEL())),
        area,
    );
}

pub(super) fn render_welcome_screen(
    f: &mut Frame,
    state: &AppState,
) -> (ratatui::layout::Rect, ratatui::layout::Rect) {
    let width = f.area().width;
    let height = f.area().height;

    let show_picker = state.modal_open();

    let box_width = 80u16.min(width.saturating_sub(6));
    let inner_width = box_width.saturating_sub(5).max(1);

    let input_lines = if state.input_buffer.is_empty() {
        1
    } else {
        count_input_lines(&state.input_buffer, inner_width as usize)
    };
    let prompt_box_height = input_lines + 4;

    let logo_start_y = height.saturating_sub(17).saturating_sub(input_lines - 1) / 2;

    let welcome_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(logo_start_y),
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Length(prompt_box_height),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(f.area());

    let logo_area = welcome_chunks[1];
    let padding_left = (logo_area.width.saturating_sub(45) / 2) as usize;
    let mut logo_lines = Vec::new();

    for line in LOGO {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() >= 22 {
            let part1: String = chars[0..22].iter().collect();
            let part2: String = chars[22..].iter().collect();

            logo_lines.push(Line::from(vec![
                Span::styled(
                    format!("{}{}", " ".repeat(padding_left), part1),
                    get_themed_style(COLOR_SECONDARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    part2,
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
            ]));
        } else {
            logo_lines.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(padding_left), line),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            )));
        }
    }
    f.render_widget(
        Paragraph::new(logo_lines).style(Style::default().bg(COLOR_BG())),
        logo_area,
    );

    let box_padding = width.saturating_sub(box_width) / 2;
    let box_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(box_padding),
            Constraint::Length(box_width),
            Constraint::Min(0),
        ])
        .split(welcome_chunks[3]);

    let prompt_box_area = box_chunks[1];

    let prompt_box_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(prompt_box_area);

    let line_chars = "▌\n".repeat(prompt_box_area.height as usize);
    let vertical_line_widget = Paragraph::new(line_chars).style(get_themed_style(
        COLOR_SECONDARY(),
        COLOR_BG(),
        Modifier::empty(),
        show_picker,
    ));
    f.render_widget(vertical_line_widget, prompt_box_split[0]);

    let solid_panel = Block::default().style(Style::default().bg(COLOR_PANEL()));

    let mut box_lines = Vec::new();
    let mut cursor_dx = 0u16;
    let mut cursor_dy = 0u16;

    if state.input_buffer.is_empty() {
        box_lines.push(Line::from(Span::styled(
            "Ask anything... \"Fix a TODO in the codebase\"",
            get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::empty(), show_picker),
        )));
    } else {
        let text_style = if state.input_buffer.starts_with('/') {
            get_themed_style(COLOR_PRIMARY(), COLOR_PANEL(), Modifier::BOLD, show_picker)
        } else {
            get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker)
        };

        let mut styled_chars: Vec<(char, Style)> = state
            .input_buffer
            .chars()
            .map(|c| (c, text_style))
            .collect();

        if let Some(suffix) = state.get_command_suggestion() {
            let suggestion_style =
                get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::ITALIC, show_picker);
            styled_chars.extend(suffix.chars().map(|c| (c, suggestion_style)));
        }

        let cursor_char_index = state.input_buffer
            [..state.cursor_position.min(state.input_buffer.len())]
            .chars()
            .count();

        let mut current_line_spans = Vec::new();
        let mut current_run: Option<(Style, String)> = None;

        let mut col = 0;
        let mut row = 0;

        let total_chars = styled_chars.len();
        for (i, &(c, style)) in styled_chars.iter().enumerate() {
            if i == cursor_char_index {
                cursor_dx = col as u16;
                cursor_dy = row as u16;
            }

            if c == '\n' {
                if let Some((st, s)) = current_run.take() {
                    current_line_spans.push(Span::styled(s, st));
                }
                box_lines.push(Line::from(current_line_spans.clone()));
                current_line_spans.clear();
                row += 1;
                col = 0;
            } else {
                if col >= inner_width as usize {
                    if let Some((st, s)) = current_run.take() {
                        current_line_spans.push(Span::styled(s, st));
                    }
                    box_lines.push(Line::from(current_line_spans.clone()));
                    current_line_spans.clear();
                    row += 1;
                    col = 0;
                }

                match current_run.as_mut() {
                    Some((st, s)) if *st == style => {
                        s.push(c);
                    }
                    _ => {
                        if let Some((st, s)) = current_run.take() {
                            current_line_spans.push(Span::styled(s, st));
                        }
                        current_run = Some((style, c.to_string()));
                    }
                }
                col += 1;
            }
        }

        if cursor_char_index == total_chars {
            cursor_dx = col as u16;
            cursor_dy = row as u16;
        }

        if let Some((st, s)) = current_run.take() {
            current_line_spans.push(Span::styled(s, st));
        }
        box_lines.push(Line::from(current_line_spans));
    }

    box_lines.push(Line::from(""));

    let agent_label = match state.agent_mode {
        crate::config::AgentMode::Build => "Build",
        crate::config::AgentMode::Plan => "Plan",
    };
    let agent_style = match state.agent_mode {
        crate::config::AgentMode::Build => get_themed_style(
            COLOR_SECONDARY(),
            COLOR_PANEL(),
            Modifier::BOLD,
            show_picker,
        ),
        crate::config::AgentMode::Plan => get_themed_style(
            Color::Rgb(229, 192, 123),
            COLOR_PANEL(),
            Modifier::BOLD,
            show_picker,
        ),
    };

    box_lines.push(Line::from(vec![
        Span::styled(agent_label, agent_style),
        Span::styled(
            " · ",
            get_themed_style(COLOR_MUTED(), COLOR_PANEL(), Modifier::empty(), show_picker),
        ),
        Span::styled(
            model_label(state),
            get_themed_style(COLOR_TEXT(), COLOR_PANEL(), Modifier::empty(), show_picker),
        ),
    ]));

    let inner = prompt_box_split[1].inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    f.render_widget(solid_panel, prompt_box_split[1]);
    f.render_widget(
        Paragraph::new(box_lines).style(Style::default().bg(COLOR_PANEL())),
        inner,
    );

    if inner.width > 0 && !show_picker {
        f.set_cursor_position(ratatui::layout::Position {
            x: inner.x + cursor_dx.min(inner.width.saturating_sub(1)),
            y: inner.y + cursor_dy,
        });
    }

    let hint_area = welcome_chunks[5];
    let hint_box_width_area =
        ratatui::layout::Rect::new(prompt_box_area.x, hint_area.y, prompt_box_area.width, 1);
    let hint_text = Paragraph::new(Line::from(vec![
        Span::styled(
            "tab",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " agents   ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ),
        Span::styled(
            "ctrl+p",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            " commands",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ),
    ]))
    .alignment(ratatui::layout::Alignment::Right)
    .style(Style::default().bg(COLOR_BG()));
    f.render_widget(hint_text, hint_box_width_area);

    let tip_area = welcome_chunks[7];
    let tip_text = crate::app::TIPS[state.tip_index % crate::app::TIPS.len()];
    let tip_full = tip_text.to_string();
    let tip_prefix = "● ";
    let prefix_w = tip_prefix.width();
    let tip_w = tip_full.width();
    let total_w = prefix_w + tip_w + 4;
    let tip_padding = (width.saturating_sub(total_w as u16) / 2) as usize;
    let centered_spans = vec![
        Span::styled(" ".repeat(tip_padding), Style::default()),
        Span::styled(
            "● ",
            get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::empty(), show_picker),
        ),
        Span::styled(
            "Tip ",
            get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            tip_full,
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ),
    ];
    f.render_widget(
        Paragraph::new(Line::from(centered_spans)).style(Style::default().bg(COLOR_BG())),
        tip_area,
    );

    let bottom_y = height.saturating_sub(2);
    let metadata_area = ratatui::layout::Rect::new(2, bottom_y, width.saturating_sub(4), 1);

    let meta_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(metadata_area);

    let left_meta = Paragraph::new(Span::styled(
        &state.cwd_and_branch,
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    ))
    .style(Style::default().bg(COLOR_BG()));
    let version_label = match state.update_check {
        crate::update::UpdateState::Available(latest) => format!(
            "v{} · update available: v{}",
            env!("CARGO_PKG_VERSION"),
            crate::update::format_version(latest)
        ),
        crate::update::UpdateState::Checking => {
            format!("v{} · checking for updates…", env!("CARGO_PKG_VERSION"))
        }
        _ => format!("v{}", env!("CARGO_PKG_VERSION")),
    };
    let right_meta = Paragraph::new(Span::styled(
        version_label,
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    ))
    .alignment(ratatui::layout::Alignment::Right)
    .style(Style::default().bg(COLOR_BG()));

    f.render_widget(left_meta, meta_chunks[0]);
    f.render_widget(right_meta, meta_chunks[1]);

    (prompt_box_area, prompt_box_split[1])
}

fn centered_rect_fixed(width: u16, height: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let x = r.x + r.width.saturating_sub(width) / 2;
    let y = r.y + r.height.saturating_sub(height) / 2;
    ratatui::layout::Rect::new(x, y, width.min(r.width), height.min(r.height))
}

#[derive(Clone)]
pub struct PickerItem {
    pub group: String,
    pub name: String,
    pub desc: String,
}

fn picker_group_for_url(url: &str) -> &'static str {
    if url.contains(":11434") {
        "ollama"
    } else if url.contains(":1976") {
        "Apple Foundation Models"
    } else {
        "custom providers"
    }
}

/// Model picker rows for the current config profiles, filtered by the
/// active search string. Shared by rendering (ui) and selection (main).
pub fn get_filtered_picker_items(state: &AppState) -> Vec<PickerItem> {
    let search = state.model_picker_search.to_lowercase();
    state
        .config
        .models
        .iter()
        .map(|p| PickerItem {
            group: picker_group_for_url(&p.url).to_string(),
            name: p.name.clone(),
            desc: p.model.clone(),
        })
        .filter(|item| {
            item.name.to_lowercase().contains(&search)
                || item.group.to_lowercase().contains(&search)
                || item.desc.to_lowercase().contains(&search)
        })
        .collect()
}

pub(super) fn render_verbosity_picker_modal(f: &mut Frame, state: &AppState) {
    let modal_area = centered_rect_fixed(40, 10, f.area());
    f.render_widget(Clear, modal_area);
    let modal_block = Block::default()
        .title("Verbosity Level")
        .borders(Borders::ALL)
        .style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(modal_block, modal_area);

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 1
    });

    let choices = [("Low", crate::app::state::Verbosity::Low), ("High", crate::app::state::Verbosity::High)];
    let mut lines = Vec::new();

    for (idx, (label, verbosity_level)) in choices.iter().enumerate() {
        let is_selected = state.modal_picker_index == idx;
        let is_current = state.verbosity == *verbosity_level;
        let mut style = Style::default().fg(COLOR_TEXT());

        if is_selected {
            style = style.bg(COLOR_PRIMARY()).add_modifier(Modifier::BOLD);
        }
        if is_current {
            lines.push(Line::from(Span::styled(format!("{} (current)", label), style)));
        } else {
            lines.push(Line::from(Span::styled(label.to_string(), style)));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(Block::default())
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, inner_area);
}

/// Render the model picker modal overlay.
pub(super) fn render_model_picker_modal(f: &mut Frame, state: &AppState) {
    let filtered_items = get_filtered_picker_items(state);

    let selected_idx = state
        .model_picker_index
        .min(filtered_items.len().saturating_sub(1));

    // Fixed modal box in center of terminal
    let modal_area = centered_rect_fixed(65, 18, f.area());

    // Clear the background to prevent text bleed-through
    f.render_widget(Clear, modal_area);

    // Borderless solid background panel
    let modal_block = Block::default().style(Style::default().bg(COLOR_PANEL()));

    f.render_widget(modal_block, modal_area);

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
    });

    // Layout constraints inside modal: Header (1), Search (1), List (Min), Footer (1)
    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Search
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    // 1. Modal Header
    let header_line = Line::from(vec![
        Span::styled(
            "Select model",
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ".repeat(inner_area.width.saturating_sub(15) as usize),
            Style::default(),
        ),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[0],
    );

    // 2. Search Box with cursor (flashing peach block)
    let search_line = if state.model_picker_search.is_empty() {
        Line::from(vec![
            Span::styled("█", Style::default().fg(COLOR_PRIMARY())),
            Span::styled("Search", Style::default().fg(COLOR_MUTED())),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                state.model_picker_search.clone(),
                Style::default().fg(COLOR_TEXT()),
            ),
            Span::styled("█", Style::default().fg(COLOR_PRIMARY())),
        ])
    };
    f.render_widget(
        Paragraph::new(search_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    // 3. Models List
    let mut list_lines = Vec::new();
    let mut current_group = String::new();

    for (idx, item) in filtered_items.iter().enumerate() {
        if item.group != current_group {
            current_group = item.group.clone();
            list_lines.push(Line::from("")); // spacer
            list_lines.push(Line::from(Span::styled(
                current_group.clone(),
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let is_selected = selected_idx == idx;
        let line = if is_selected {
            // Selected row: solid Peach background block
            let left_text = format!(" ● {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.len() + item.desc.len());
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
                    item.desc.clone(),
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let left_text = format!("   {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.len() + item.desc.len());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(item.desc.clone(), Style::default().fg(COLOR_MUTED())),
            ])
        };
        list_lines.push(line);
    }

    // Scrollable widget viewport
    let list_height = modal_chunks[4].height as usize;
    // Find the actual line index in list_lines for the selected item (accounting for group headers)
    let mut list_line_idx = 0;
    let mut target_list_idx: usize = 0;
    for (i, item) in filtered_items.iter().enumerate() {
        if i == 0 || item.group != filtered_items[i - 1].group {
            list_line_idx += 2; // blank line + group header
        }
        if i == selected_idx {
            target_list_idx = list_line_idx;
            break;
        }
        list_line_idx += 1;
    }
    let total_lines = list_lines.len();
    let scroll_y: u16 = if total_lines <= list_height {
        0
    } else {
        let ideal = target_list_idx.saturating_sub(list_height / 3);
        let lo = target_list_idx.saturating_sub(list_height - 1);
        let hi = target_list_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(list_paragraph, modal_chunks[4]);

    // 4. Modal Footer
    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT())),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("search ", Style::default().fg(COLOR_TEXT())),
        Span::styled("type", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[5],
    );
}

/// Render the session history picker modal overlay (/history).
pub(super) fn render_history_picker_modal(f: &mut Frame, state: &AppState) {
    // Confirmation overlay for delete (Ctrl+D)
    if let Some(del_idx) = state.pending_delete_session_idx {
        let modal_area = centered_rect_fixed(60, 10, f.area());
        f.render_widget(Clear, modal_area);
        f.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_PRIMARY()))
                .style(Style::default().bg(COLOR_PANEL())),
            modal_area,
        );

        let inner_area = modal_area.inner(Margin {
            vertical: 1,
            horizontal: 3,
        });
        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Header
                Constraint::Length(1), // Spacer
                Constraint::Length(1), // Session title line
                Constraint::Length(1), // Session info line
                Constraint::Min(1),    // Spacer
                Constraint::Length(1), // Footer buttons
            ])
            .split(inner_area);

        let header_line = Line::from(vec![Span::styled(
            "⚠ Delete session?",
            Style::default()
                .fg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(
            Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[0],
        );

        if let Some(meta) = state.history_picker_sessions.get(del_idx) {
            let title_line = Line::from(vec![
                Span::styled("  session  ", Style::default().fg(COLOR_MUTED())),
                Span::styled(
                    &meta.title,
                    Style::default()
                        .fg(COLOR_TEXT())
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            f.render_widget(
                Paragraph::new(title_line).style(Style::default().bg(COLOR_PANEL())),
                modal_chunks[2],
            );

            let info_line = Line::from(vec![
                Span::styled("  info     ", Style::default().fg(COLOR_MUTED())),
                Span::styled(
                    format!("{} messages  ({})", meta.message_count, meta.when),
                    Style::default().fg(COLOR_MUTED()),
                ),
            ]);
            f.render_widget(
                Paragraph::new(info_line).style(Style::default().bg(COLOR_PANEL())),
                modal_chunks[3],
            );
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "  y / enter",
                Style::default()
                    .fg(COLOR_GREEN())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" delete  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "n / esc",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(
            Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[5],
        );

        return;
    }

    let sessions = &state.history_picker_sessions;
    let selected_idx = state
        .history_picker_index
        .min(sessions.len().saturating_sub(1));

    let modal_area = centered_rect_fixed(65, 18, f.area());
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        modal_area,
    );

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let header_line = Line::from(vec![
        Span::styled(
            "Resume session",
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ".repeat(inner_area.width.saturating_sub(17) as usize),
            Style::default(),
        ),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[0],
    );

    let mut list_lines = Vec::new();
    for (idx, session) in sessions.iter().enumerate() {
        let desc = format!("{} msgs  {}", session.message_count, session.when);
        let is_selected = selected_idx == idx;
        let line = if is_selected {
            let left_text = format!(" ● {}", session.title);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.len() + desc.len());
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
                Span::styled(desc, Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY())),
            ])
        } else {
            let left_text = format!("   {}", session.title);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.len() + desc.len());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(desc, Style::default().fg(COLOR_MUTED())),
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
        let lo = selected_idx.saturating_sub(list_height - 1);
        let hi = selected_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(list_paragraph, modal_chunks[2]);

    let mut footer_spans = vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT())),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("delete ", Style::default().fg(COLOR_TEXT())),
        Span::styled("ctrl+d", Style::default().fg(COLOR_MUTED())),
    ];
    if state.history_picker_truncated {
        footer_spans.push(Span::styled(
            "   (Truncated to 50 sessions. Use /delete_chat to clean up.)",
            Style::default()
                .fg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD),
        ));
    }
    let footer_line = Line::from(footer_spans);
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[3],
    );
}

pub(super) fn render_mcp_config_modal(f: &mut Frame, state: &AppState) {
    let servers = &state.config.mcp_servers;
    let selected_idx = state.mcp_picker_index;

    let modal_area = centered_rect_fixed(70, 18, f.area());
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        modal_area,
    );

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // Content area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    if let Some(ref edit_state) = state.mcp_edit_state {
        // --- ADD / EDIT MODE ---
        let title = if edit_state.is_add {
            "Add MCP Server"
        } else {
            "Edit MCP Server"
        };
        let header_line = Line::from(vec![Span::styled(
            title,
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(Paragraph::new(header_line), modal_chunks[0]);

        // Draw 3 input fields
        let form_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Name input
                Constraint::Length(3), // Command input
                Constraint::Length(3), // Args input
            ])
            .split(modal_chunks[2]);

        for field_idx in 0..3 {
            let label = match field_idx {
                0 => "Server Name",
                1 => "Executable Command",
                _ => "Arguments (space-separated)",
            };
            let val = match field_idx {
                0 => &edit_state.name_input,
                1 => &edit_state.command_input,
                _ => &edit_state.args_input,
            };

            let is_active = edit_state.active_field == field_idx;
            let display_val = if is_active {
                let pos = edit_state.cursor_pos.min(val.len());
                let (left, right) = val.split_at(pos);
                format!("{left}█{right}")
            } else {
                val.clone()
            };

            let border_style = if is_active {
                Style::default().fg(COLOR_TEXT())
            } else {
                Style::default().fg(COLOR_MUTED())
            };

            f.render_widget(
                Paragraph::new(display_val).block(
                    Block::default()
                        .title(Span::styled(label, Style::default().fg(COLOR_MUTED())))
                        .borders(Borders::ALL)
                        .border_style(border_style),
                ),
                form_chunks[field_idx],
            );
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "enter",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Save    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "esc",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Cancel    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "tab / arrows",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Switch Field", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[3]);
    } else {
        // --- LIST MODE ---
        let header_line = Line::from(vec![
            Span::styled(
                "MCP Servers Configuration",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " ".repeat(inner_area.width.saturating_sub(29) as usize),
                Style::default(),
            ),
            Span::styled("esc", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(header_line), modal_chunks[0]);

        let mut list_lines = Vec::new();
        for (idx, srv) in servers.iter().enumerate() {
            let is_selected = selected_idx == idx;
            let status = if srv.enabled { "Enabled" } else { "Disabled" };
            let status_style = if srv.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(COLOR_MUTED())
            };

            let cmd_text = format!("{} {}", srv.command, srv.args.join(" "));

            let line = if is_selected {
                let left_text = format!(" ● {}", srv.name);
                let right_text = format!(" [{}] {}", status, cmd_text);
                let padding_len =
                    (inner_area.width as usize).saturating_sub(left_text.len() + right_text.len());

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
                    Span::styled(format!(" [{}]", status), status_style.bg(COLOR_PRIMARY())),
                    Span::styled(
                        format!(" {}", cmd_text),
                        Style::default()
                            .fg(COLOR_BG())
                            .bg(COLOR_PRIMARY())
                            .add_modifier(Modifier::ITALIC),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        format!("{:<20}", srv.name),
                        Style::default().fg(COLOR_MUTED()),
                    ),
                    Span::styled(" [", Style::default().fg(COLOR_MUTED())),
                    Span::styled(status, status_style),
                    Span::styled("] ", Style::default().fg(COLOR_MUTED())),
                    Span::styled(cmd_text, Style::default().fg(COLOR_MUTED())),
                ])
            };
            list_lines.push(line);
        }

        if list_lines.is_empty() {
            f.render_widget(
                Paragraph::new("No MCP servers configured.\nPress 'a' to add a new server.")
                    .style(Style::default().fg(COLOR_MUTED())),
                modal_chunks[2],
            );
        } else {
            f.render_widget(Paragraph::new(list_lines), modal_chunks[2]);
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "a",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Add    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "e",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Edit    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "d",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Delete    ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "enter",
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Toggle Enabled", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[3]);
    }
}

#[derive(Clone)]
pub struct PaletteItem {
    pub group: &'static str,
    pub name: &'static str,
    pub shortcut: &'static str,
}

pub const PALETTE_ITEMS: &[PaletteItem] = &[
    PaletteItem {
        group: "Session",
        name: "New session",
        shortcut: "/new",
    },
    PaletteItem {
        group: "Session",
        name: "Resume session",
        shortcut: "/resume",
    },
    PaletteItem {
        group: "Session",
        name: "Copy last reply",
        shortcut: "/copy",
    },
    PaletteItem {
        group: "Agent",
        name: "Switch model",
        shortcut: "/model",
    },
    PaletteItem {
        group: "Agent",
        name: "Set context window",
        shortcut: "/context",
    },
    PaletteItem {
        group: "Agent",
        name: "Set parser/tool protocol",
        shortcut: "/parser",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure provider profile",
        shortcut: "/provider",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure Ollama models",
        shortcut: "/ollama",
    },
    PaletteItem {
        group: "Agent",
        name: "Configure MCP servers",
        shortcut: "/mcp",
    },
    PaletteItem {
        group: "Session",
        name: "Change session title",
        shortcut: "/change_title",
    },
    PaletteItem {
        group: "Session",
        name: "Clear conversation",
        shortcut: "/clear",
    },
    PaletteItem {
        group: "Session",
        name: "Cancel active stream",
        shortcut: "/cancel",
    },
    PaletteItem {
        group: "System",
        name: "Show info",
        shortcut: "/info",
    },
    PaletteItem {
        group: "System",
        name: "Show help",
        shortcut: "/help",
    },
    PaletteItem {
        group: "System",
        name: "Show token usage stats",
        shortcut: "/stats",
    },
    PaletteItem {
        group: "System",
        name: "Show token usage (alias)",
        shortcut: "/usage",
    },
    PaletteItem {
        group: "System",
        name: "Show RAM usage",
        shortcut: "/memory",
    },
    PaletteItem {
        group: "System",
        name: "List available tools",
        shortcut: "/tools",
    },
    PaletteItem {
        group: "System",
        name: "Exit the app",
        shortcut: "ctrl+c",
    },
    PaletteItem {
        group: "System",
        name: "Quit the app",
        shortcut: "/quit",
    },
];

pub(super) fn render_command_picker_modal(f: &mut Frame, state: &AppState) {
    let search = state.command_picker_search.to_lowercase();
    let filtered_items: Vec<&PaletteItem> = PALETTE_ITEMS
        .iter()
        .filter(|item| {
            item.name.to_lowercase().contains(&search)
                || item.group.to_lowercase().contains(&search)
        })
        .collect();

    let selected_idx = state
        .command_picker_index
        .min(filtered_items.len().saturating_sub(1));

    let modal_area = centered_rect_fixed(65, 20, f.area());

    f.render_widget(Clear, modal_area);

    let modal_block = Block::default().style(Style::default().bg(COLOR_PANEL()));

    f.render_widget(modal_block, modal_area);

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
    });

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
        ])
        .split(inner_area);

    let header_line = Line::from(vec![
        Span::styled(
            "Commands",
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ".repeat(inner_area.width.saturating_sub(12) as usize),
            Style::default(),
        ),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[0],
    );

    let search_line = if state.command_picker_search.is_empty() {
        Line::from(vec![
            Span::styled("█", Style::default().fg(COLOR_PRIMARY())),
            Span::styled("Search", Style::default().fg(COLOR_MUTED())),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                state.command_picker_search.clone(),
                Style::default().fg(COLOR_TEXT()),
            ),
            Span::styled("█", Style::default().fg(COLOR_PRIMARY())),
        ])
    };
    f.render_widget(
        Paragraph::new(search_line).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let mut list_lines = Vec::new();
    let mut current_group = String::new();

    for (idx, item) in filtered_items.iter().enumerate() {
        if item.group != current_group {
            current_group = item.group.to_string();
            list_lines.push(Line::from(""));
            list_lines.push(Line::from(Span::styled(
                current_group.clone(),
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let is_selected = selected_idx == idx;
        let line = if is_selected {
            let name_part = format!(" {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(name_part.len() + item.shortcut.len());
            Line::from(vec![
                Span::styled(
                    name_part,
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
                    item.shortcut.to_string(),
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let name_part = format!("  {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(name_part.len() + item.shortcut.len());
            Line::from(vec![
                Span::styled(name_part, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(
                    item.shortcut.to_string(),
                    Style::default().fg(COLOR_MUTED()),
                ),
            ])
        };
        list_lines.push(line);
    }

    let list_height = modal_chunks[4].height as usize;
    // Find the actual line index in list_lines for the selected item (accounting for group headers)
    let mut list_line_idx = 0;
    let mut target_list_idx: usize = 0;
    for (i, item) in filtered_items.iter().enumerate() {
        if i == 0 || item.group != filtered_items[i - 1].group {
            list_line_idx += 2; // blank line + group header
        }
        if i == selected_idx {
            target_list_idx = list_line_idx;
            break;
        }
        list_line_idx += 1;
    }
    let total_lines = list_lines.len();
    let scroll_y: u16 = if total_lines <= list_height {
        0
    } else {
        let ideal = target_list_idx.saturating_sub(list_height / 3);
        let lo = target_list_idx.saturating_sub(list_height - 1);
        let hi = target_list_idx.min(total_lines - list_height);
        ideal.clamp(lo, hi)
    } as u16;
    let list_paragraph = Paragraph::new(list_lines)
        .scroll((scroll_y, 0))
        .style(Style::default().bg(COLOR_PANEL()));
    f.render_widget(list_paragraph, modal_chunks[4]);
}

pub(super) fn render_tool_confirmation_modal(f: &mut Frame, state: &AppState) {
    let confirmations = match &state.pending_tool_confirmation {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    if confirmations.len() == 1 {
        let confirmation = &confirmations[0];
        let screen_width = f.area().width;
        let screen_height = f.area().height;
        let width = if confirmation.content_preview.contains('\x00') {
            (screen_width.saturating_sub(4)).clamp(80, 160)
        } else {
            (screen_width.saturating_sub(10)).clamp(60, 120)
        };
        let has_preview = !confirmation.content_preview.trim().is_empty();
        let preview_lines = confirmation.content_preview.lines().count();
        let height = if has_preview {
            ((preview_lines as u16) + 9).clamp(14, (screen_height.saturating_sub(4)).min(40))
        } else {
            9
        };
        let modal_area = centered_rect_fixed(width, height, f.area());

        f.render_widget(Clear, modal_area);

        let modal_block = Block::default().style(Style::default().bg(COLOR_PANEL()));
        f.render_widget(modal_block, modal_area);

        let inner_area = modal_area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });

        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                            // 0: Header
                Constraint::Length(1),                            // 1: Spacer
                Constraint::Length(1),                            // 2: Tool
                Constraint::Length(1),                            // 3: Path
                Constraint::Length(1),                            // 4: Size
                Constraint::Length(1),                            // 5: Auto-confirm status
                Constraint::Length(1),                            // 6: Spacer
                Constraint::Min(if has_preview { 2 } else { 0 }), // 7: Preview Diff / Content
                Constraint::Length(1),                            // 8: Spacer
                Constraint::Length(1),                            // 9: Footer buttons
            ])
            .split(inner_area);

        let action_label = match confirmation.tool_name.as_str() {
            "write_to_file" => "Write to file",
            "replace_file_content" => "Replace file content",
            "multi_replace_file_content" => "Apply multi-replace",
            "create_file" => "Create file",
            "write_file" => "Overwrite file",
            "delete_file" => "Delete file",
            "move_file" => "Move file",
            "copy_file" => "Copy file",
            "run_command" => "Run command",
            _ => "Execute tool",
        };
        let header_line = Line::from(vec![Span::styled(
            format!("⚠ {action_label}?"),
            Style::default()
                .fg(COLOR_TIP())
                .add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(
            Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[0],
        );

        let tool_line = Line::from(vec![
            Span::styled("  tool  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                &confirmation.tool_name,
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(
            Paragraph::new(tool_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[2],
        );

        let path_display = if confirmation.path.len() > inner_area.width as usize - 10 {
            let cut = inner_area.width as usize - 13;
            format!("…{}", &confirmation.path[confirmation.path.len() - cut..])
        } else {
            confirmation.path.clone()
        };
        let path_title = match confirmation.tool_name.as_str() {
            "run_command" => "  cmd   ",
            _ => "  path  ",
        };
        let path_line = Line::from(vec![
            Span::styled(path_title, Style::default().fg(COLOR_MUTED())),
            Span::styled(path_display, Style::default().fg(COLOR_PRIMARY())),
        ]);
        f.render_widget(
            Paragraph::new(path_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[3],
        );

        let size_line = Line::from(vec![
            Span::styled("  size  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                format!("{} bytes", confirmation.content_bytes),
                Style::default().fg(COLOR_TEXT()),
            ),
        ]);
        f.render_widget(
            Paragraph::new(size_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[4],
        );

        let auto_confirm_status = if state.auto_confirm {
            "[x] Auto-confirm future tool calls"
        } else {
            "[ ] Auto-confirm future tool calls"
        };
        let auto_confirm_line = Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                auto_confirm_status,
                Style::default()
                    .fg(COLOR_TIP())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" (Tab to toggle)", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(
            Paragraph::new(auto_confirm_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[5],
        );

        if !confirmation.content_preview.is_empty() {
            let diff_height = modal_chunks[7].height as usize;
            let scroll = state.modal_scroll_row as usize;

            let has_null = confirmation.content_preview.contains('\x00');
            if has_null {
                let diff_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(50),
                        Constraint::Length(1), // Divider
                        Constraint::Percentage(50),
                    ])
                    .split(modal_chunks[7]);

                let mut left_lines = Vec::new();
                let mut right_lines = Vec::new();

                let half_width = (diff_chunks[0].width as usize).saturating_sub(2);

                for line in confirmation
                    .content_preview
                    .lines()
                    .skip(scroll)
                    .take(diff_height)
                {
                    let parts: Vec<&str> = line.split('\x00').collect();
                    if parts.len() == 2 {
                        left_lines.push(highlight_diff_line(parts[0], half_width, false));
                        right_lines.push(highlight_diff_line(parts[1], half_width, false));
                    } else {
                        left_lines.push(highlight_diff_line(line, half_width, false));
                        right_lines.push(Line::from(""));
                    }
                }

                f.render_widget(
                    Paragraph::new(left_lines).wrap(Wrap { trim: false }),
                    diff_chunks[0],
                );

                let divider_lines = vec![Line::from("│"); diff_chunks[1].height as usize];
                f.render_widget(
                    Paragraph::new(divider_lines).style(Style::default().fg(COLOR_MUTED())),
                    diff_chunks[1],
                );

                f.render_widget(
                    Paragraph::new(right_lines).wrap(Wrap { trim: false }),
                    diff_chunks[2],
                );
            } else {
                let preview_lines: Vec<Line> = confirmation
                    .content_preview
                    .lines()
                    .skip(scroll)
                    .take(diff_height)
                    .map(|l| {
                        let width = (inner_area.width as usize).saturating_sub(4);
                        highlight_diff_line(l, width, false)
                    })
                    .collect();
                f.render_widget(
                    Paragraph::new(preview_lines).wrap(Wrap { trim: false }),
                    modal_chunks[7],
                );
            }
        }

        let total_lines = confirmation.content_preview.lines().count();
        let scroll_info = if modal_chunks.len() > 7 && total_lines > modal_chunks[7].height as usize
        {
            format!(
                "  ↑/↓ scroll ({}/{})",
                state.modal_scroll_row + 1,
                total_lines
            )
        } else {
            String::new()
        };

        let footer_line = Line::from(vec![
            Span::styled(
                "  y / enter",
                Style::default()
                    .fg(COLOR_GREEN())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" approve  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "n",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" deny  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "tab",
                Style::default()
                    .fg(COLOR_TIP())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" toggle auto-confirm  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "esc",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(COLOR_MUTED())),
            Span::styled(scroll_info, Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(
            Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[9],
        );
    } else {
        // Render batch confirmation modal
        let modal_area = centered_rect_fixed(70, 16, f.area());
        f.render_widget(Clear, modal_area);
        let modal_block = Block::default().style(Style::default().bg(COLOR_PANEL()));
        f.render_widget(modal_block, modal_area);

        let inner_area = modal_area.inner(Margin {
            vertical: 1,
            horizontal: 3,
        });

        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Header
                Constraint::Length(1), // Spacer
                Constraint::Min(5),    // List of tools
                Constraint::Length(1), // Auto-confirm option
                Constraint::Length(1), // Spacer
                Constraint::Length(1), // Footer/Actions
            ])
            .split(inner_area);

        let header_line = Line::from(vec![Span::styled(
            format!("⚠ Approve {} tool calls in parallel?", confirmations.len()),
            Style::default()
                .fg(COLOR_TIP())
                .add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(Paragraph::new(header_line), modal_chunks[0]);

        let mut tool_lines = Vec::new();
        for (i, c) in confirmations.iter().enumerate() {
            let action = match c.tool_name.as_str() {
                "write_to_file" => "Write to file",
                "replace_file_content" => "Replace file content",
                "multi_replace_file_content" => "Apply multi-replace",
                "create_file" => "Create file",
                "write_file" => "Overwrite file",
                "delete_file" => "Delete file",
                "move_file" => "Move file",
                "copy_file" => "Copy file",
                "run_command" => "Run command",
                _ => "Execute tool",
            };

            let path_display = if c.path.len() > inner_area.width as usize - 25 {
                let cut = inner_area.width as usize - 28;
                format!("…{}", &c.path[c.path.len() - cut..])
            } else {
                c.path.clone()
            };

            let line = Line::from(vec![
                Span::styled(format!("  {}. ", i + 1), Style::default().fg(COLOR_MUTED())),
                Span::styled(
                    format!("{:<15}", action),
                    Style::default()
                        .fg(COLOR_TEXT())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", path_display),
                    Style::default().fg(COLOR_PRIMARY()),
                ),
            ]);
            tool_lines.push(line);
        }

        f.render_widget(Paragraph::new(tool_lines), modal_chunks[2]);

        let auto_confirm_status = if state.auto_confirm {
            "[x] Auto-confirm future tool calls"
        } else {
            "[ ] Auto-confirm future tool calls"
        };
        let auto_confirm_line = Line::from(vec![
            Span::styled(
                auto_confirm_status,
                Style::default()
                    .fg(COLOR_TIP())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" (Tab to toggle)", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(auto_confirm_line), modal_chunks[3]);

        let footer_line = Line::from(vec![
            Span::styled(
                "  y / enter",
                Style::default()
                    .fg(COLOR_GREEN())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" approve all  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "n / esc",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" deny all", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[5]);
    }
}

/// Interactive `ask_question` modal: renders the question and its options, with
/// the highlighted option (and, for multi-select, ticked options) emphasized.
pub(super) fn render_question_modal(f: &mut Frame, state: &AppState) {
    let Some(q) = &state.pending_question else {
        return;
    };

    let screen = f.area();
    let width = screen.width.saturating_sub(10).clamp(48, 84);

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
    let modal_area = centered_rect_fixed(width, height, screen);

    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
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
                .bg(COLOR_PANEL())
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));

    for (i, opt) in q.options.iter().enumerate() {
        let is_sel = i == q.selected;
        let marker = if q.is_multi_select {
            if q.chosen.get(i).copied().unwrap_or(false) {
                "[x] "
            } else {
                "[ ] "
            }
        } else if is_sel {
            "› "
        } else {
            "  "
        };
        let label = format!("{marker}{}. {opt}", i + 1);
        let style = if is_sel {
            Style::default()
                .fg(COLOR_BG())
                .bg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL())
        };
        // Pad the highlighted row so the selection bar spans the modal width.
        let padded = format!("{label:<width$}", width = inner.width as usize);
        lines.push(Line::from(Span::styled(padded, style)));
    }

    // The always-present "write your own answer" slot (index == options.len()).
    let custom_idx = q.options.len();
    let custom_sel = q.selected == custom_idx;
    let custom_label = match &q.custom_input {
        Some(text) => format!("✎ {text}▏"),
        None => "✎ Write your own answer…".to_string(),
    };
    let custom_style = if custom_sel || q.custom_input.is_some() {
        Style::default()
            .fg(COLOR_BG())
            .bg(COLOR_PRIMARY())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_TIP()).bg(COLOR_PANEL())
    };
    let custom_padded = format!("{custom_label:<width$}", width = inner.width as usize);
    lines.push(Line::from(Span::styled(custom_padded, custom_style)));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL()),
    )));

    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(COLOR_PANEL())),
        inner,
    );
}

/// Minimal greedy word-wrap used by the question modal (avoids pulling the chat
/// wrapping helpers into modal code).
fn textwrap_simple(text: &str, width: usize) -> Vec<String> {
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

pub(super) fn render_theme_picker_modal(f: &mut Frame, state: &AppState) {
    let p = crate::ui::theme::get_palette(&state.config.theme);
    let modal_area = centered_rect_fixed(65, 14, f.area());

    f.render_widget(Clear, modal_area);

    let modal_block = Block::default().style(Style::default().bg(p.panel));
    f.render_widget(modal_block, modal_area);

    let inner_area = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 3,
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

    let header_line = Line::from(vec![
        Span::styled(
            "Select UI Theme (Live Preview)",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ".repeat(inner_area.width.saturating_sub(35) as usize),
            Style::default(),
        ),
        Span::styled("esc", Style::default().fg(p.muted)),
    ]);
    f.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(p.panel)),
        modal_chunks[0],
    );

    let themes = crate::ui::theme::load_available_themes();
    let selected_idx = state.theme_picker_index.min(themes.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, theme) in themes.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_active = state.theme_picker_initial.eq_ignore_ascii_case(&theme.name);
        let active_badge = if is_active { " [active]" } else { "" };
        let line = if is_selected {
            let text = format!(
                " ● {:<12} — {}{}",
                theme.name, theme.description, active_badge
            );
            let padding = (inner_area.width as usize).saturating_sub(text.len());
            Line::from(Span::styled(
                format!("{}{}", text, " ".repeat(padding)),
                Style::default()
                    .fg(p.bg)
                    .bg(p.primary)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            let left = format!("   {:<12} — {}", theme.name, theme.description);
            Line::from(vec![
                Span::styled(left, Style::default().fg(p.text).bg(p.panel)),
                Span::styled(
                    active_badge.to_string(),
                    Style::default().fg(p.muted).bg(p.panel),
                ),
            ])
        };
        list_lines.push(line);
    }

    f.render_widget(
        Paragraph::new(list_lines).style(Style::default().bg(p.panel)),
        modal_chunks[2],
    );

    let footer_line = Line::from(Span::styled(
        "↑/↓ or j/k: preview theme • Enter: save • Esc: cancel",
        Style::default().fg(p.muted),
    ));
    f.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(p.panel)),
        modal_chunks[3],
    );
}
