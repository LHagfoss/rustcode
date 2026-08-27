//! Modal, popup, picker and welcome-screen rendering for the TUI.
//!
//! Extracted from `ui/mod.rs`. Shared colour constants and small helpers
//! (`get_themed_style`, `model_label`, `count_input_lines`) live in the parent
//! module and are pulled in via the `super::*` glob; diff highlighting comes
//! from the sibling `highlight` module.

use super::highlight::{highlight_diff_line, highlight_shell_command};
use super::*;
use crate::app::{AppEvent, AppState, ApprovalDecision, PendingQuestion, QuestionAnswer};
use crate::inline_terminal::Frame;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

mod advanced_settings;
mod confirmation;
mod context;
mod navigation;
mod question;
mod settings;

#[cfg(test)]
mod tests;

pub(in crate::ui) use advanced_settings::{
    render_effort_picker_modal, render_protocol_picker_modal, render_thinking_picker_modal,
    render_update_prompt_modal,
};
pub(in crate::ui) use confirmation::{question_height, render_tool_confirmation_modal};
pub(super) use context::calculate_context_breakdown;
pub(in crate::ui) use context::{render_context_modal, render_theme_picker_modal};
pub use navigation::{PALETTE_ITEMS, PaletteItem};
pub(in crate::ui) use navigation::{
    render_command_picker_modal, render_history_picker_modal, render_mcp_config_modal,
    render_model_picker_modal, render_subagent_picker_modal, tool_confirmation_height,
};
pub(in crate::ui) use question::render_question_modal;
use question::textwrap_simple;
pub(in crate::ui) use settings::{render_verbosity_picker_modal, render_yolo_picker_modal};

pub(crate) fn approval_event_for_key(key: KeyEvent, selected: usize) -> Option<AppEvent> {
    let decision = match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => ApprovalDecision::Approve,
        KeyCode::Char('a') | KeyCode::Char('A') => ApprovalDecision::ApproveAll,
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => ApprovalDecision::Deny,
        KeyCode::Enter => {
            if selected == 0 {
                ApprovalDecision::Approve
            } else {
                ApprovalDecision::Deny
            }
        }
        _ => return None,
    };
    Some(AppEvent::ApprovalDecision(decision))
}

pub(crate) fn question_custom_answer_event(question: &PendingQuestion) -> AppEvent {
    let answer = question.custom_input.as_deref().unwrap_or_default().trim();
    let answer = if answer.is_empty() {
        "No response provided"
    } else {
        answer
    };
    AppEvent::AnswerQuestion(QuestionAnswer::Custom(answer.to_owned()))
}

pub(crate) fn question_answer_event(question: &PendingQuestion) -> Option<AppEvent> {
    if question.selected >= question.options.len() {
        return None;
    }

    let answer = if question.is_multi_select {
        let picked = question
            .options
            .iter()
            .zip(question.chosen.iter())
            .filter(|(_, chosen)| **chosen)
            .map(|(option, _)| option.clone())
            .collect::<Vec<_>>();
        if picked.is_empty() {
            question.options.get(question.selected)?.clone()
        } else {
            picked.join(", ")
        }
    } else {
        question.options.get(question.selected)?.clone()
    };

    Some(AppEvent::AnswerQuestion(QuestionAnswer::Selected(answer)))
}

pub(crate) fn question_cancel_event() -> AppEvent {
    AppEvent::AnswerQuestion(QuestionAnswer::Cancelled)
}

pub(super) fn render_popup_menu(
    f: &mut Frame,
    state: &RenderSnapshot,
    filtered_cmds: &[&CommandInfo],
    area: ratatui::layout::Rect,
) {
    // The popup is allocated below the input box. Scroll the list so the
    // selected command stays visible when the available rows are bounded.
    let max_rows = (area.height as usize).max(1);
    let selected = state.active_suggestion_index().unwrap_or(0);
    let offset = if selected >= max_rows {
        selected + 1 - max_rows
    } else {
        0
    };

    f.render_widget(Clear, area);
    let mut popup_lines = Vec::new();
    for (idx, cmd) in filtered_cmds.iter().enumerate().skip(offset).take(max_rows) {
        let is_selected = state
            .active_suggestion_index()
            .map(|i| i == idx)
            .unwrap_or(false);

        let marker = if is_selected { "› " } else { "  " };
        let left_text = format!("{marker}{:<10}  ", cmd.name);
        let desc_text = cmd.desc.to_string();
        let total_len = left_text.width() + desc_text.width();
        let padding_len = (area.width as usize).saturating_sub(total_len);
        let line = Line::from(vec![
            Span::styled(
                left_text,
                Style::default()
                    .fg(if is_selected {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_TEXT()
                    })
                    .bg(COLOR_PANEL())
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                desc_text,
                Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL()),
            ),
            Span::styled(" ".repeat(padding_len), Style::default().bg(COLOR_PANEL())),
        ]);
        popup_lines.push(line);
    }
    f.render_widget(
        Paragraph::new(popup_lines).style(Style::default().bg(COLOR_PANEL())),
        area,
    );
}

pub(super) fn render_at_popup_menu(
    f: &mut Frame,
    state: &RenderSnapshot,
    file_matches: &[String],
    area: ratatui::layout::Rect,
) {
    let max_rows = (area.height as usize).max(1);
    let selected = state.active_suggestion_index().unwrap_or(0);
    let offset = if selected >= max_rows {
        selected + 1 - max_rows
    } else {
        0
    };

    f.render_widget(Clear, area);

    let mut popup_lines = Vec::new();
    for (i, file) in file_matches.iter().skip(offset).take(max_rows).enumerate() {
        let is_selected = selected == (offset + i);
        let marker = if is_selected { "› " } else { "  " };
        let left_text = format!("{marker}{file}");
        let padding_len = (area.width as usize).saturating_sub(left_text.width());
        let line = Line::from(vec![
            Span::styled(
                left_text,
                Style::default()
                    .fg(if is_selected {
                        COLOR_PRIMARY()
                    } else {
                        COLOR_TEXT()
                    })
                    .bg(COLOR_PANEL())
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(" ".repeat(padding_len), Style::default().bg(COLOR_PANEL())),
        ]);
        popup_lines.push(line);
    }
    f.render_widget(
        Paragraph::new(popup_lines).style(Style::default().bg(COLOR_PANEL())),
        area,
    );
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
pub fn get_filtered_picker_items(state: &RenderSnapshot) -> Vec<PickerItem> {
    let search = state.model_picker_search().to_lowercase();
    state
        .config()
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

/// Computes a rect for an inline picker anchored directly above the chat input box (`input_area`).
pub(super) fn input_anchor_rect(
    _f: &Frame,
    input_area: ratatui::layout::Rect,
    max_height: u16,
) -> ratatui::layout::Rect {
    let width = input_area.width;
    let available_h = input_area.y;
    let height = max_height.min(available_h).max(4);
    let x = input_area.x;
    let y = input_area.y.saturating_sub(height);
    ratatui::layout::Rect::new(x, y, width, height)
}

#[allow(dead_code)]
fn render_padded_panel(f: &mut Frame, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    render_padded_panel_with_color(f, area, COLOR_PANEL())
}

fn render_padded_panel_with_color(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    panel: Color,
) -> ratatui::layout::Rect {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(Style::default().bg(panel)), area);
    area.inner(Margin {
        vertical: 1,
        horizontal: 0,
    })
}

fn paint_panel_line_backgrounds(lines: &mut [Line<'static>], panel: Color) {
    for line in lines {
        line.style = line.style.patch(Style::default().bg(panel));
        for span in &mut line.spans {
            span.style = span.style.patch(Style::default().bg(panel));
        }
    }
}

fn truncate_middle_to_width(text: &str, max_width: usize) -> String {
    let text = text.replace(['\r', '\n'], " ");
    if text.width() <= max_width {
        return text;
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_owned();
    }

    let content_width = max_width - 1;
    let tail_width = (content_width / 3).max(1);
    let head_width = content_width.saturating_sub(tail_width);
    let mut head = String::new();
    let mut used = 0;
    for character in text.chars() {
        let width = character.width().unwrap_or(0);
        if used + width > head_width {
            break;
        }
        head.push(character);
        used += width;
    }

    let mut tail = Vec::new();
    used = 0;
    for character in text.chars().rev() {
        let width = character.width().unwrap_or(0);
        if used + width > tail_width {
            break;
        }
        tail.push(character);
        used += width;
    }
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
}
