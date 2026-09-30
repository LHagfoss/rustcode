//! Modal, popup, picker and welcome-screen rendering for the TUI.
//!
//! Extracted from `ui/mod.rs`. Shared colour constants and small helpers
//! (`get_themed_style`, `model_label`, `count_input_lines`) live in the parent
//! module and are pulled in via the `super::*` glob; diff highlighting comes
//! from the sibling `highlight` module.

use super::highlight::{highlight_diff_line, highlight_shell_command};
use super::*;
use crate::inline_terminal::Frame;
use crate::runtime::events::AppEvent;
#[cfg(test)]
use crossterm::event::KeyModifiers;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use rustcode::controller::{ApprovalDecision, PendingQuestion, QuestionAnswer, RenderState};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

mod advanced_settings;
mod confirmation;
mod context;
mod navigation;
mod panel;
mod question;
mod settings;

#[cfg(test)]
mod tests;

pub(in crate::ui) use advanced_settings::{
    render_effort_picker_modal, render_protocol_picker_modal, render_thinking_picker_modal,
    render_update_prompt_modal,
};
pub(in crate::ui) use confirmation::{question_height, render_tool_confirmation_modal};
#[cfg(test)]
pub(super) use context::calculate_context_breakdown;
pub(in crate::ui) use context::{
    render_context_modal, render_session_modal, render_stats_modal, render_status_modal,
    render_theme_picker_modal,
};
pub use navigation::{PALETTE_ITEMS, PaletteItem};
pub(in crate::ui) use navigation::{
    render_command_picker_modal, render_history_picker_modal, render_mcp_config_modal,
    render_model_picker_modal, render_subagent_picker_modal, tool_confirmation_height,
};
pub(in crate::ui) use panel::{
    HIGH_USAGE_PCT, OVER_THRESHOLD_PCT, PanelEmphasis, context_category_colors, emphasis_for_share,
    panel_line, panel_value_spans,
};
pub(in crate::ui) use question::render_question_modal;
pub(in crate::ui) use settings::{render_verbosity_picker_modal, render_yolo_picker_modal};

pub(crate) fn approval_event_for_key(
    key: KeyEvent,
    selected: usize,
    rememberable_prefix: Option<&str>,
    forbidden_prefix: Option<&str>,
) -> Option<AppEvent> {
    let decision = match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => ApprovalDecision::Approve,
        KeyCode::Char('a') | KeyCode::Char('A') => ApprovalDecision::ApproveAll,
        KeyCode::Char('r') | KeyCode::Char('R') => {
            ApprovalDecision::ApproveAndRemember(rememberable_prefix?.to_owned())
        }
        KeyCode::Char('f') | KeyCode::Char('F') => {
            ApprovalDecision::ForbidAndRemember(forbidden_prefix?.to_owned())
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => ApprovalDecision::Deny,
        KeyCode::Enter => {
            if selected == 0 {
                ApprovalDecision::Approve
            } else if selected == 1 {
                ApprovalDecision::Deny
            } else if selected == 2 && rememberable_prefix.is_some() {
                ApprovalDecision::ApproveAndRemember(rememberable_prefix?.to_owned())
            } else {
                ApprovalDecision::ForbidAndRemember(forbidden_prefix?.to_owned())
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
    if question.selected > question.options.len() {
        return None;
    }
    let answer = question.display_answer();
    if answer.is_empty() {
        return None;
    }
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
    // Scroll the menu while keeping the selected command in the space above
    // the composer. A zero-height area never paints over the input.
    let max_rows = area.height as usize;
    if max_rows == 0 || area.width == 0 {
        return;
    }
    let selected = state.active_suggestion_index().unwrap_or(0);
    let offset = if selected >= max_rows {
        selected + 1 - max_rows
    } else {
        0
    };

    f.render_widget(Clear, area);
    let mut popup_lines = Vec::new();
    let name_width = filtered_cmds
        .iter()
        .map(|command| command.name.width())
        .max()
        .unwrap_or(0)
        .min(picker_column_budget(area.width as usize, 0));
    for (idx, cmd) in filtered_cmds.iter().enumerate().skip(offset).take(max_rows) {
        let is_selected = state
            .active_suggestion_index()
            .map(|i| i == idx)
            .unwrap_or(false);

        // The command and description share a row, with the whole selected row
        // highlighted like Codex's completion menu. The gap between the two
        // columns is the shared picker gap, so this menu measures its rows with
        // the same rule as the inline pickers (#1528).
        let marker = if is_selected { "› " } else { "  " };
        let left_text = truncate_to_width(
            &format!("{marker}{:<name_width$}   ", cmd.name),
            area.width as usize,
        );
        let description_width = (area.width as usize).saturating_sub(left_text.width());
        let desc_text = truncate_to_width(cmd.desc, description_width);
        let padding_len = description_width.saturating_sub(desc_text.width());
        let background = if is_selected {
            COLOR_PRIMARY()
        } else {
            COLOR_PANEL()
        };
        let line = Line::from(vec![
            Span::styled(
                left_text,
                Style::default()
                    .fg(if is_selected {
                        Color::Black
                    } else {
                        COLOR_TEXT()
                    })
                    .bg(background)
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                desc_text,
                Style::default()
                    .fg(if is_selected {
                        Color::Black
                    } else {
                        COLOR_MUTED()
                    })
                    .bg(background)
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(" ".repeat(padding_len), Style::default().bg(background)),
        ]);
        popup_lines.push(line);
    }
    f.render_widget(
        Paragraph::new(popup_lines).style(Style::default().bg(COLOR_PANEL())),
        area,
    );
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }

    let budget = max_width.saturating_sub(1);
    let mut output = String::new();
    let mut used = 0;
    for character in text.chars() {
        let char_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + char_width > budget {
            break;
        }
        used += char_width;
        output.push(character);
    }
    output.push('…');
    output
}

pub(super) fn render_at_popup_menu(
    f: &mut Frame,
    state: &RenderSnapshot,
    file_matches: &[String],
    area: ratatui::layout::Rect,
) {
    let max_rows = area.height as usize;
    if max_rows == 0 || area.width == 0 {
        return;
    }
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
        let left_text = truncate_to_width(&format!("{marker}{file}"), area.width as usize);
        let padding_len = (area.width as usize).saturating_sub(left_text.width());
        let background = if is_selected {
            COLOR_PRIMARY()
        } else {
            COLOR_PANEL()
        };
        let line = Line::from(vec![
            Span::styled(
                left_text,
                Style::default()
                    .fg(if is_selected {
                        Color::Black
                    } else {
                        COLOR_TEXT()
                    })
                    .bg(background)
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(" ".repeat(padding_len), Style::default().bg(background)),
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

/// Panel heights for the inline modals anchored above the composer. Each
/// modal renders within its own bound; `open_modal_max_height` reserves the
/// same number of rows so the transcript stays visible above the panel.
pub(super) const MODEL_PICKER_HEIGHT: u16 = 14;
pub(super) const HISTORY_PICKER_HEIGHT: u16 = 14;
pub(super) const HISTORY_CONFIRM_HEIGHT: u16 = 10;
pub(super) const SUBAGENT_PICKER_HEIGHT: u16 = 18;
pub(super) const MCP_CONFIG_HEIGHT: u16 = 14;
pub(super) const COMMAND_PICKER_HEIGHT: u16 = 14;
pub(super) const THEME_PICKER_HEIGHT: u16 = 12;
pub(super) const CONTEXT_MODAL_HEIGHT: u16 = 14;
/// Header, blank row, model/session/messages, an optional token line and the
/// one-row padding above and below the panel.
pub(super) const STATUS_MODAL_HEIGHT: u16 = 8;
/// Header, blank row, last-turn tokens, optional latency, a blank row, the
/// monthly heading and up to three months of totals, plus panel padding.
pub(super) const STATS_MODAL_HEIGHT: u16 = 12;
/// Header, blank row, id/model/messages and the one-row padding above and
/// below the panel.
pub(super) const SESSION_MODAL_HEIGHT: u16 = 8;
pub(super) const UPDATE_PROMPT_HEIGHT: u16 = 14;
pub(super) const VERBOSITY_PICKER_HEIGHT: u16 = 10;
pub(super) const YOLO_PICKER_HEIGHT: u16 = 10;
pub(super) const THINKING_PICKER_HEIGHT: u16 = 10;
pub(super) const EFFORT_PICKER_HEIGHT: u16 = 11;
pub(super) const PROTOCOL_PICKER_HEIGHT: u16 = 10;

/// Smallest usable panel, so a modal never collapses to a title bar when the
/// terminal is short.
pub(super) const MIN_MODAL_HEIGHT: u16 = 4;

/// Cells a picker row spends on the `› ` / `  ` selection marker.
const PICKER_MARKER_WIDTH: usize = 2;

/// Cells between the primary (name) and secondary (description) column of a
/// picker row. Every inline picker uses this gap, so the two columns line up
/// across the whole picker family (#1528).
const PICKER_COLUMN_GAP: usize = 3;

/// Column budget for a picker row whose secondary column needs `secondary`
/// cells, given the frame width the row must fit inside.
///
/// The primary column is truncated to this budget rather than wrapped, so a row
/// never outgrows its frame no matter how long the name or description is.
/// Single owner of the marker-plus-gap reservation: the inline pickers and the
/// popup menu all derive their name column from here (#1528).
pub(super) fn picker_column_budget(frame_width: usize, secondary: usize) -> usize {
    frame_width.saturating_sub(secondary + PICKER_MARKER_WIDTH + PICKER_COLUMN_GAP)
}

/// Cells between the title and the right-hand hint on a picker header row.
/// Saturates to zero when the pair already fills the row, so a header is never
/// built wider than its frame.
pub(super) fn picker_header_padding(frame_width: usize, title: &str, right: &str) -> usize {
    frame_width.saturating_sub(title.width() + right.width())
}

/// Cells between the primary and secondary halves of a picker row. Saturates to
/// zero when the two halves already fill the row.
pub(super) fn picker_row_padding(frame_width: usize, primary: &str, secondary: &str) -> usize {
    frame_width.saturating_sub(primary.width() + secondary.width())
}

/// First row of the `list_height`-row window that keeps `selected` visible.
///
/// Aims a third of a screen above the selection so the rows below stay legible
/// without hiding the selection itself. Single owner of the list-window rule:
/// every scrollable inline picker scrolls by this (#1528).
pub(super) fn picker_list_window(selected: usize, total: usize, list_height: usize) -> u16 {
    if total <= list_height {
        return 0;
    }
    let ideal = selected.saturating_sub(list_height / 3);
    let lo = selected.saturating_sub(list_height.saturating_sub(1));
    let hi = selected.min(total - list_height);
    ideal.clamp(lo, hi) as u16
}

/// Panel height the currently open modal claims above the composer. Callers
/// reserve these rows in the chat area so the panel overlays blank space
/// instead of painting over transcript text.
pub(super) fn open_modal_max_height(state: &RenderSnapshot) -> u16 {
    let height = if state.show_model_picker() {
        MODEL_PICKER_HEIGHT
    } else if state.show_theme_picker() {
        THEME_PICKER_HEIGHT
    } else if state.show_command_picker() {
        COMMAND_PICKER_HEIGHT
    } else if state.show_history_picker() {
        if state.pending_delete_session_idx().is_some() {
            HISTORY_CONFIRM_HEIGHT
        } else {
            HISTORY_PICKER_HEIGHT
        }
    } else if state.show_subagent_picker() {
        SUBAGENT_PICKER_HEIGHT
    } else if state.show_context_modal() {
        CONTEXT_MODAL_HEIGHT
    } else if state.show_status_modal() {
        STATUS_MODAL_HEIGHT
    } else if state.show_stats_modal() {
        STATS_MODAL_HEIGHT
    } else if state.show_session_modal() {
        SESSION_MODAL_HEIGHT
    } else if state.show_update_prompt() {
        UPDATE_PROMPT_HEIGHT
    } else if state.show_mcp_config() {
        MCP_CONFIG_HEIGHT
    } else {
        match state.status() {
            AppStatus::VerbosityPicker => VERBOSITY_PICKER_HEIGHT,
            AppStatus::ThinkingPicker => THINKING_PICKER_HEIGHT,
            AppStatus::EffortPicker => EFFORT_PICKER_HEIGHT,
            AppStatus::ProtocolPicker => PROTOCOL_PICKER_HEIGHT,
            AppStatus::YoloPicker => YOLO_PICKER_HEIGHT,
            _ => return 0,
        }
    };
    height.max(MIN_MODAL_HEIGHT)
}

/// Bounded panel for an inline modal, anchored directly above the chat input
/// box (`input_area`) and never taller than the space available there.
pub(super) fn input_anchor_rect(
    f: &Frame,
    input_area: ratatui::layout::Rect,
    max_height: u16,
) -> ratatui::layout::Rect {
    let viewport = f.area();
    let width = input_area
        .width
        .min(viewport.width.saturating_sub(input_area.x));
    let available_h = input_area.y.saturating_sub(viewport.y);
    let height = max_height
        .min(available_h)
        .max(MIN_MODAL_HEIGHT.min(available_h));
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
