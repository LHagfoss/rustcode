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

pub(crate) fn approval_event_for_key(
    key: KeyEvent,
    selected: usize,
) -> Option<AppEvent> {
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
    let answer = question
        .custom_input
        .as_deref()
        .unwrap_or_default()
        .trim();
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
    state: &AppState,
    filtered_cmds: &[&CommandInfo],
    area: ratatui::layout::Rect,
) {
    // The popup is allocated below the input box. Scroll the list so the
    // selected command stays visible when the available rows are bounded.
    let max_rows = (area.height as usize).max(1);
    let selected = state.active_suggestion_index.unwrap_or(0);
    let offset = if selected >= max_rows {
        selected + 1 - max_rows
    } else {
        0
    };

    f.render_widget(Clear, area);
    let mut popup_lines = Vec::new();
    for (idx, cmd) in filtered_cmds.iter().enumerate().skip(offset).take(max_rows) {
        let is_selected = state
            .active_suggestion_index
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
                    .fg(if is_selected { COLOR_PRIMARY() } else { COLOR_TEXT() })
                    .bg(COLOR_PANEL())
                    .add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() }),
            ),
            Span::styled(desc_text, Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL())),
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
    state: &AppState,
    file_matches: &[String],
    area: ratatui::layout::Rect,
) {
    let max_rows = (area.height as usize).max(1);
    let selected = state.active_suggestion_index.unwrap_or(0);
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
                    .fg(if is_selected { COLOR_PRIMARY() } else { COLOR_TEXT() })
                    .bg(COLOR_PANEL())
                    .add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() }),
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
    f.render_widget(
        Block::default().style(Style::default().bg(panel)),
        area,
    );
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

pub(super) fn render_verbosity_picker_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 10);
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
            Constraint::Min(2),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Output verbosity";
    let right_esc = "esc";
    let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let choices = [
        (
            "Low",
            crate::app::state::Verbosity::Low,
            "Compact tool outputs & clean diff summaries",
        ),
        (
            "High",
            crate::app::state::Verbosity::High,
            "Pure model text output (hides tool outputs & diffs)",
        ),
    ];

    let selected_idx = state
        .modal_picker_index
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, verbosity_level, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = state.verbosity == *verbosity_level;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!(" ● {}", name);
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
            let left_text = format!("   {}", name);
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

    f.render_widget(
        Paragraph::new(list_lines).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ToolConfirmation;
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::{backend::TestBackend, layout::Rect};

    #[test]
    fn single_command_confirmation_uses_codex_command_prompt() {
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        let mut state = AppState::new();
        state.config.theme = "default".to_owned();
        let panel = crate::ui::theme::get_palette(&state.config.theme).panel;
        crate::ui::theme::set_active_theme("nord");
        state.pending_tool_confirmation = Some(vec![ToolConfirmation {
            tool_name: "run_command".to_string(),
            path: "git commit --message \"hello\"".to_string(),
            content_preview: String::new(),
            content_bytes: 0,
        }]);

        let input_area = Rect::new(0, 2, 100, 10);
        terminal
            .draw(|frame| render_tool_confirmation_modal(frame, &state, input_area))
            .unwrap();

        let rendered = (0..14)
            .map(|y| {
                (0..100)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Would you like to run the following command?"));
        assert!(
            rendered.contains("$ git commit --message \"hello\""),
            "rendered modal:\n{rendered}"
        );
        assert!(rendered.contains("› 1. Yes, proceed"));
        assert!(rendered.contains("2. No, cancel this tool call"));

        let buffer = terminal.backend().buffer();
        let command_row = (2..12)
            .find(|y| {
                (0..100)
                    .map(|x| buffer[(x, *y)].symbol())
                    .collect::<String>()
                    .contains("$ git commit --message \"hello\"")
            })
            .expect("dynamic command row");
        assert!((0..100).all(|x| buffer[(x, command_row)].bg == panel));
        let mut command_foregrounds = Vec::new();
        for foreground in (0..100)
            .filter(|x| !buffer[(*x, command_row)].symbol().trim().is_empty())
            .map(|x| buffer[(x, command_row)].fg)
        {
            if !command_foregrounds.contains(&foreground) {
                command_foregrounds.push(foreground);
            }
        }
        assert!(
            command_foregrounds.len() > 1,
            "command should contain syntax colors: {command_foregrounds:?}"
        );
        assert!((0..100).all(|x| buffer[(x, 2)].bg == panel));
        assert!((0..100).all(|x| buffer[(x, 11)].bg == panel));
    }

    #[test]
    fn long_approval_rows_are_clipped_and_keep_the_panel_background() {
        let mut terminal = Terminal::new(TestBackend::new(72, 16)).unwrap();
        let mut state = AppState::new();
        let command = "git log v0.17.0..HEAD --oneline --no-merges; echo ---; git log -3 --oneline; echo ---; git tag --sort=-v:refname | head -5";
        state.pending_tool_confirmation = Some(vec![ToolConfirmation {
            tool_name: "run_command".to_owned(),
            path: command.to_owned(),
            content_preview: format!(
                "resolved command: {command}\nscope: unclassified or potentially mutating shell command"
            ),
            content_bytes: 0,
        }]);

        terminal
            .draw(|frame| {
                render_tool_confirmation_modal(frame, &state, Rect::new(0, 1, 72, 14))
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rows = (0..16)
            .map(|y| {
                (0..72)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(rows.iter().any(|row| row.contains("$ git log") && row.contains('…')));
        assert!(!rows.iter().any(|row| row.contains(command)));

        let preview_rows = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.contains("resolved command:") || row.contains("scope:"))
            .map(|(row, _)| row as u16)
            .collect::<Vec<_>>();
        assert_eq!(preview_rows.len(), 2, "approval rows: {rows:#?}");
        for row in preview_rows {
            let painted_panel = buffer[(71, row)].bg;
            assert!((0..72).all(|x| buffer[(x, row)].bg == painted_panel));
        }
    }

    #[test]
    fn middle_truncation_keeps_command_start_and_tail() {
        assert_eq!(truncate_middle_to_width("cargo check --tests", 40), "cargo check --tests");
        let clipped = truncate_middle_to_width(
            "git log --oneline; dangerous-command --force",
            24,
        );
        assert!(clipped.starts_with("git log"), "clipped command: {clipped}");
        assert!(clipped.ends_with("--force"), "clipped command: {clipped}");
        assert_eq!(clipped.width(), 24);
    }

    #[test]
    fn compact_approval_keeps_heading_and_actions_visible() {
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        let mut state = AppState::new();
        state.pending_tool_confirmation = Some(vec![ToolConfirmation {
            tool_name: "write_to_file".to_owned(),
            path: "src/main.rs".to_owned(),
            content_preview: "+new line".to_owned(),
            content_bytes: 9,
        }]);
        terminal
            .draw(|frame| {
                render_tool_confirmation_modal(frame, &state, Rect::new(0, 2, 80, 5))
            })
            .unwrap();
        let rendered = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>();
        assert!(rendered.contains("Would you like to make the following change?"));
        assert!(rendered.contains("1. Yes, proceed"));
        assert!(rendered.contains("2. No, cancel"));
    }

    #[test]
    fn approval_selection_visibly_moves_to_deny() {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        let mut state = AppState::new();
        state.tool_confirmation_selected = 1;
        state.pending_tool_confirmation = Some(vec![ToolConfirmation {
            tool_name: "run_command".to_owned(),
            path: "cargo test".to_owned(),
            content_preview: String::new(),
            content_bytes: 0,
        }]);
        terminal
            .draw(|frame| {
                render_tool_confirmation_modal(frame, &state, Rect::new(0, 1, 80, 10))
            })
            .unwrap();
        let rendered = terminal.backend().buffer().content.iter()
            .map(|cell| cell.symbol()).collect::<String>();

        assert!(rendered.contains("› 2. No, cancel this tool call"));
        assert!(!rendered.contains("› 1. Yes, proceed"));
    }

    #[test]
    fn batch_approval_lists_each_tool_in_the_bottom_pane() {
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        let mut state = AppState::new();
        state.pending_tool_confirmation = Some(vec![
            ToolConfirmation { tool_name: "write_to_file".to_owned(), path: "src/one.rs".to_owned(), content_preview: String::new(), content_bytes: 1 },
            ToolConfirmation { tool_name: "run_command".to_owned(), path: "cargo check".to_owned(), content_preview: String::new(), content_bytes: 11 },
        ]);
        terminal
            .draw(|frame| {
                render_tool_confirmation_modal(frame, &state, Rect::new(0, 2, 100, 12))
            })
            .unwrap();
        let rendered = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>();
        assert!(rendered.contains("approve these 2 tool calls"));
        assert!(rendered.contains("write_to_file src/one.rs"));
        assert!(rendered.contains("run_command $ cargo check"));
    }

    #[test]
    fn approval_keys_emit_typed_decisions() {
        assert!(matches!(
            approval_event_for_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE), 1),
            Some(AppEvent::ApprovalDecision(ApprovalDecision::Approve))
        ));
        assert!(matches!(
            approval_event_for_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE), 1),
            Some(AppEvent::ApprovalDecision(ApprovalDecision::ApproveAll))
        ));
        assert!(matches!(
            approval_event_for_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 1),
            Some(AppEvent::ApprovalDecision(ApprovalDecision::Deny))
        ));
        assert!(matches!(
            approval_event_for_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 0),
            Some(AppEvent::ApprovalDecision(ApprovalDecision::Deny))
        ));
    }

    #[test]
    fn question_answers_are_typed_without_mutating_the_question() {
        let mut question = PendingQuestion::new(
            "Where?".to_owned(),
            vec!["Here".to_owned(), "There".to_owned()],
            false,
        );
        question.selected = 1;
        assert!(matches!(
            question_answer_event(&question),
            Some(AppEvent::AnswerQuestion(QuestionAnswer::Selected(answer)))
                if answer == "There"
        ));

        question.selected = question.options.len();
        question.activate_custom_input();
        question.insert_str("somewhere");
        assert!(matches!(
            question_custom_answer_event(&question),
            AppEvent::AnswerQuestion(QuestionAnswer::Custom(answer)) if answer == "somewhere"
        ));
        assert_eq!(question.selected, question.options.len());
    }

    #[test]
    fn multi_select_question_answer_joins_selected_options() {
        let mut question = PendingQuestion::new(
            "Which?".to_owned(),
            vec!["one".to_owned(), "two".to_owned()],
            true,
        );
        question.chosen[0] = true;
        question.chosen[1] = true;
        assert!(matches!(
            question_answer_event(&question),
            Some(AppEvent::AnswerQuestion(QuestionAnswer::Selected(answer)))
                if answer == "one, two"
        ));
    }

    #[test]
    fn settings_picker_uses_unified_modal_picker_style() {
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        let mut state = AppState::new();
        state.modal_picker_index = 1;
        terminal
            .draw(|frame| render_verbosity_picker_modal(frame, &state, Rect::new(0, 12, 100, 3)))
            .unwrap();
        let rendered = terminal.backend().buffer().content.iter()
            .map(|cell| cell.symbol()).collect::<String>();

        assert!(rendered.contains("Output verbosity"));
        assert!(rendered.contains("● High"));
        assert!(rendered.contains("Pure model text output"));
    }

    #[test]
    fn effort_picker_renders_options() {
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        let mut state = AppState::new();
        state.modal_picker_index = 0;
        terminal
            .draw(|frame| render_effort_picker_modal(frame, &state, Rect::new(0, 12, 100, 3)))
            .unwrap();
        let rendered = terminal.backend().buffer().content.iter()
            .map(|cell| cell.symbol()).collect::<String>();

        assert!(rendered.contains("Reasoning effort"));
        assert!(rendered.contains("● Low"));
        assert!(rendered.contains("Medium"));
        assert!(rendered.contains("High"));
        assert!(rendered.contains("Off"));
    }

    #[test]
    fn history_picker_renders_borderless_full_width_options() {
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        let mut state = AppState::new();
        state.history_picker_sessions = vec![
            crate::config::SessionMeta {
                path: std::path::PathBuf::from("/tmp/test-1.json"),
                title: "Build a polished browser tower-defense game with canvas".to_string(),
                message_count: 6,
                when: "17:35".to_string(),
            },
        ];
        state.history_picker_index = 0;
        terminal
            .draw(|frame| render_history_picker_modal(frame, &state, Rect::new(0, 12, 100, 3)))
            .unwrap();
        let rendered = terminal.backend().buffer().content.iter()
            .map(|cell| cell.symbol()).collect::<String>();

        assert!(rendered.contains("Resume session"));
        assert!(rendered.contains("●"));
        assert!(rendered.contains("6 msgs"));
        assert!(rendered.contains("17:35"));
    }
}

pub(super) fn render_thinking_picker_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 10);
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
            Constraint::Min(3),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Model thinking";
    let right_esc = "esc";
    let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let current = state
        .config
        .models
        .iter()
        .find(|prof| prof.url == state.api_base_url)
        .and_then(|prof| prof.enable_thinking);

    let choices = [
        ("On", Some(true), "Force <think> reasoning on"),
        (
            "Off",
            Some(false),
            "Skip <think> entirely (faster, no trace)",
        ),
        ("Default", None, "Leave it to the server/Modelfile"),
    ];

    let selected_idx = state
        .modal_picker_index
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, val, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = current == *val;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!(" ● {}", name);
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
            let left_text = format!("   {}", name);
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

    f.render_widget(
        Paragraph::new(list_lines).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
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

pub(super) fn render_effort_picker_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 11);
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
            Constraint::Min(4),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Reasoning effort";
    let right_esc = "esc";
    let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let current = state
        .config
        .models
        .iter()
        .find(|prof| prof.url == state.api_base_url)
        .and_then(|prof| prof.reasoning_effort.as_deref());

    let choices = [
        ("Low", Some("low"), "Compact reasoning traces (fastest)"),
        ("Medium", Some("medium"), "Balanced reasoning depth"),
        ("High", Some("high"), "Deep reasoning analysis"),
        ("Off", None, "Clear reasoning effort parameter"),
    ];

    let selected_idx = state
        .modal_picker_index
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, val, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = current == *val;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!(" ● {}", name);
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
            let left_text = format!("   {}", name);
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

    f.render_widget(
        Paragraph::new(list_lines).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
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

pub(super) fn render_protocol_picker_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 10);
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
            Constraint::Min(3),    // Options list
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Tool protocol";
    let right_esc = "esc";
    let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let current = state.active_tool_protocol();

    let choices = [
        (
            "ApiNative",
            crate::config::ToolProtocol::ApiNative,
            "Structured API schema (`tools` field + `tool_calls` output)",
        ),
        (
            "Json",
            crate::config::ToolProtocol::Json,
            "Standard JSON markdown (```tool)",
        ),
        (
            "Native",
            crate::config::ToolProtocol::Native,
            "Bracketed format ([TOOL_CALLS])",
        ),
    ];

    let selected_idx = state
        .modal_picker_index
        .min(choices.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, (name, val, desc)) in choices.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_current = current == *val;
        let active_badge = if is_current { " (active)" } else { "" };
        let full_desc = format!("{}{}", desc, active_badge);
        let line = if is_selected {
            let left_text = format!(" ● {}", name);
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
            let left_text = format!("   {}", name);
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

    f.render_widget(
        Paragraph::new(list_lines).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let footer_line = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
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

/// Render the startup Homebrew update prompt directly above the chat input.
pub(super) fn render_update_prompt_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 14);
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
            Constraint::Length(1), // Versions
            Constraint::Length(1), // Command
            Constraint::Length(1), // Spacer
            Constraint::Min(3),    // Options
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title = "RustCode update available";
    let header = Line::from(vec![
        Span::styled(
            title,
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  esc to skip", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(header).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[0],
    );

    let latest = match state.update_check {
        crate::update::UpdateState::Available(latest) => latest,
        _ => crate::update::current_version(),
    };
    let versions = format!(
        "v{} → v{}",
        crate::update::format_version(crate::update::current_version()),
        crate::update::format_version(latest)
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("New version: ", Style::default().fg(COLOR_MUTED())),
            Span::styled(versions, Style::default().fg(COLOR_TEXT())),
        ]))
        .style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[1],
    );

    let (command_label, command_text, update_action_desc) = if crate::update::is_brew_install() {
        (
            "Method: ",
            crate::update::BREW_UPGRADE_COMMAND,
            "run Homebrew and restart rustcode",
        )
    } else {
        (
            "Method: ",
            "GitHub Releases (in-place update)",
            "download update and restart rustcode",
        )
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(command_label, Style::default().fg(COLOR_MUTED())),
            Span::styled(
                command_text,
                Style::default().fg(COLOR_PRIMARY()),
            ),
        ]))
        .style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[2],
    );

    let options = [
        ("Update now", update_action_desc),
        ("Skip", "do not update this time"),
        ("Skip until next version", "hide this version for this run"),
    ];
    let selected = state.update_prompt_index.min(options.len() - 1);
    let option_lines = options
        .iter()
        .enumerate()
        .map(|(index, (label, description))| {
            let selected = index == selected;
            let prefix = if selected { " ● " } else { "   " };
            let left = format!("{prefix}{label}");
            let padding = (inner_area.width as usize)
                .saturating_sub(left.width() + description.width());
            if selected {
                Line::from(vec![
                    Span::styled(
                        left,
                        Style::default()
                            .fg(COLOR_BG())
                            .bg(COLOR_PRIMARY())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " ".repeat(padding),
                        Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                    ),
                    Span::styled(
                        *description,
                        Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(left, Style::default().fg(COLOR_TEXT())),
                    Span::styled(" ".repeat(padding), Style::default()),
                    Span::styled(*description, Style::default().fg(COLOR_MUTED())),
                ])
            }
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(option_lines).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[4],
    );

    let footer = Line::from(vec![
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
        Span::styled("↑/↓   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("confirm ", Style::default().fg(COLOR_TEXT())),
        Span::styled("enter   ", Style::default().fg(COLOR_MUTED())),
        Span::styled("skip ", Style::default().fg(COLOR_TEXT())),
        Span::styled("esc", Style::default().fg(COLOR_MUTED())),
    ]);
    f.render_widget(
        Paragraph::new(footer).style(Style::default().bg(COLOR_PANEL())),
        modal_chunks[5],
    );
}

/// Render the model picker directly above the chat input.
pub(super) fn render_model_picker_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let filtered_items = get_filtered_picker_items(state);

    let selected_idx = state
        .model_picker_index
        .min(filtered_items.len().saturating_sub(1));

    let modal_area = input_anchor_rect(f, input_area, 14);
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
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let search_part = if state.model_picker_search.is_empty() {
        "".to_owned()
    } else {
        format!(" · {}", state.model_picker_search)
    };
    let title_text = format!("Select model{search_part}");
    let right_esc = if state.model_picker_search.is_empty() { "type to search  esc" } else { "esc" };
    let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let mut list_lines = Vec::new();
    for (idx, item) in filtered_items.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let max_name_width = (inner_area.width as usize).saturating_sub(item.desc.width() + 5);
        let name_display = truncate_middle_to_width(&item.name, max_name_width);
        let line = if is_selected {
            let left_text = format!(" ● {}", name_display);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + item.desc.width());
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
            let left_text = format!("   {}", name_display);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + item.desc.width());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(item.desc.clone(), Style::default().fg(COLOR_MUTED())),
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
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
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

/// Render the session history picker modal overlay (/history).
pub(super) fn render_history_picker_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    // Confirmation overlay for delete (Ctrl+D)
    if let Some(del_idx) = state.pending_delete_session_idx {
        let modal_area = input_anchor_rect(f, input_area, 10);
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

    let modal_area = input_anchor_rect(f, input_area, 14);
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
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let title_text = "Resume session";
    let right_esc = "esc";
    let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let mut list_lines = Vec::new();
    for (idx, session) in sessions.iter().enumerate() {
        let desc = format!("{} msgs  {}", session.message_count, session.when);
        let is_selected = selected_idx == idx;
        let max_title_width = (inner_area.width as usize).saturating_sub(desc.width() + 5);
        let title_display = truncate_middle_to_width(&session.title, max_title_width);
        let line = if is_selected {
            let left_text = format!(" ● {}", title_display);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + desc.width());
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
            let left_text = format!("   {}", title_display);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + desc.width());
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

/// Render the navigable subagent context picker. The selected context keeps
/// its own transcript in state; this surface makes that history visible before
/// the user switches the active view.
pub(super) fn render_subagent_picker_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let total = state.subagents.len() + 1;
    let selected = state.subagent_picker_index.min(total.saturating_sub(1));
    let modal_area = input_anchor_rect(f, input_area, 18);
    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_PANEL())),
        modal_area,
    );

    let inner = modal_area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);
    let title_text = "Agent contexts";
    let right_esc = "esc";
    let padding_header = (inner.width as usize).saturating_sub(title_text.width() + right_esc.width());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                title_text,
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ".repeat(padding_header), Style::default()),
            Span::styled(right_esc, Style::default().fg(COLOR_MUTED())),
        ]))
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Select a conversation context; parent history is preserved",
            Style::default().fg(COLOR_MUTED()),
        )))
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[1],
    );

    let list_height = chunks[2].height as usize;
    let mut lines = Vec::with_capacity(total);
    let root_selected = selected == 0;
    lines.push(agent_picker_line(
        root_selected,
        "main",
        "root conversation",
        state.selected_subagent_id.is_none(),
        inner.width as usize,
    ));
    for (index, agent) in state.subagents.iter().enumerate() {
        let is_selected = selected == index + 1;
        let status = match agent.status {
            crate::app::SubAgentStatus::Running => "running",
            crate::app::SubAgentStatus::Completed => "completed",
            crate::app::SubAgentStatus::Failed => "failed",
            crate::app::SubAgentStatus::Cancelled => "cancelled",
        };
        let task = agent.task.chars().take(32).collect::<String>();
        lines.push(agent_picker_line(
            is_selected,
            &agent.name,
            &format!("{status} · {task}"),
            state.selected_subagent_id == Some(agent.id),
            inner.width as usize,
        ));
    }

    let offset = if selected >= list_height {
        selected + 1 - list_height
    } else {
        0
    };
    f.render_widget(
        Paragraph::new(lines.into_iter().skip(offset).take(list_height).collect::<Vec<_>>())
            .style(Style::default().bg(COLOR_PANEL())),
        chunks[2],
    );

    let detail = if selected == 0 {
        "main · root context".to_owned()
    } else if let Some(agent) = state.subagents.get(selected - 1) {
        let status = match agent.status {
            crate::app::SubAgentStatus::Running => "running",
            crate::app::SubAgentStatus::Completed => "completed",
            crate::app::SubAgentStatus::Failed => "failed",
            crate::app::SubAgentStatus::Cancelled => "cancelled",
        };
        let last = agent
            .history
            .last()
            .map(|message| message.content.lines().next().unwrap_or_default())
            .unwrap_or_default();
        format!("{} · {} · {}", agent.name, status, last.chars().take(48).collect::<String>())
    } else {
        "No subagent contexts".to_owned()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            detail,
            Style::default().fg(COLOR_MUTED()),
        )))
        .style(Style::default().bg(COLOR_PANEL())),
        chunks[3],
    );
}

fn agent_picker_line(
    selected: bool,
    name: &str,
    detail: &str,
    active: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "› " } else { "  " };
    let active_marker = if active { "●" } else { "○" };
    let text = format!("{marker}{active_marker} {name} · {detail}");
    let text = text.chars().take(width).collect::<String>();
    let style = if selected {
        Style::default()
            .fg(COLOR_BG())
            .bg(COLOR_PRIMARY())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_TEXT())
    };
    Line::from(Span::styled(text, style))
}

pub(super) fn render_mcp_config_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let servers = &state.config.mcp_servers;
    let selected_idx = state.mcp_picker_index;

    let modal_area = input_anchor_rect(f, input_area, 14);
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
        f.render_widget(
            Paragraph::new(header_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[0],
        );

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
                Paragraph::new(display_val)
                    .style(Style::default().bg(COLOR_PANEL()))
                    .block(
                        Block::default()
                            .title(Span::styled(label, Style::default().fg(COLOR_MUTED())))
                            .borders(Borders::ALL)
                            .border_style(border_style)
                            .style(Style::default().bg(COLOR_PANEL())),
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
        f.render_widget(
            Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[3],
        );
    } else {
        // --- LIST MODE ---
        let title_text = "MCP Servers Configuration";
        let right_esc = "esc";
        let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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
                    (inner_area.width as usize).saturating_sub(left_text.width() + right_text.width());

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
                    .style(Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL())),
                modal_chunks[2],
            );
        } else {
            f.render_widget(
                Paragraph::new(list_lines).style(Style::default().bg(COLOR_PANEL())),
                modal_chunks[2],
            );
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
        f.render_widget(
            Paragraph::new(footer_line).style(Style::default().bg(COLOR_PANEL())),
            modal_chunks[3],
        );
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
        name: "Fork session",
        shortcut: "/fork",
    },
    PaletteItem {
        group: "Session",
        name: "Archive session",
        shortcut: "/archive",
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
        name: "Browse agent contexts",
        shortcut: "/agents",
    },
    PaletteItem {
        group: "Agent",
        name: "Switch model",
        shortcut: "/model",
    },
    PaletteItem {
        group: "Agent",
        name: "Show context usage",
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
        group: "Agent",
        name: "Toggle automatic tool confirmation",
        shortcut: "/yolo",
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
        name: "About rustcode",
        shortcut: "/about",
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

pub(super) fn render_command_picker_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
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

    let modal_area = input_anchor_rect(f, input_area, 14);
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
            Constraint::Min(3),    // List area
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    let search_part = if state.command_picker_search.is_empty() {
        "".to_owned()
    } else {
        format!(" · {}", state.command_picker_search)
    };
    let title_text = format!("Commands{search_part}");
    let right_esc = if state.command_picker_search.is_empty() { "type to search  esc" } else { "esc" };
    let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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

    let mut list_lines = Vec::new();
    for (idx, item) in filtered_items.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let line = if is_selected {
            let left_text = format!(" ● {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + item.shortcut.width());
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
                    item.shortcut.to_string(),
                    Style::default().fg(COLOR_BG()).bg(COLOR_PRIMARY()),
                ),
            ])
        } else {
            let left_text = format!("   {}", item.name);
            let padding_len =
                (inner_area.width as usize).saturating_sub(left_text.width() + item.shortcut.width());
            Line::from(vec![
                Span::styled(left_text, Style::default().fg(COLOR_TEXT())),
                Span::styled(" ".repeat(padding_len), Style::default()),
                Span::styled(item.shortcut.to_string(), Style::default().fg(COLOR_MUTED())),
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
        Span::styled("select ", Style::default().fg(COLOR_TEXT())),
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

pub(super) fn tool_confirmation_height(state: &AppState, available: u16) -> u16 {
    let Some(confirmations) = state.pending_tool_confirmation.as_ref() else {
        return 3;
    };
    let preview = confirmations
        .first()
        .map(|confirmation| confirmation.content_preview.lines().count() as u16)
        .unwrap_or(0)
        .min(8);
    let content = if confirmations.len() > 1 {
        7u16.saturating_add(confirmations.len().min(8) as u16)
    } else {
        9u16.saturating_add(preview)
    };
    content.saturating_add(2).min(available.max(3))
}

/// Bottom-pane approval view matching Codex's interaction layout. The
/// execution/confirmation channel remains RustCode's; this function only owns
/// presentation and keeps the normal composer hidden while a decision is due.
pub(super) fn render_tool_confirmation_modal(
    f: &mut Frame,
    state: &AppState,
    area: ratatui::layout::Rect,
) {
    let confirmations = match &state.pending_tool_confirmation {
        Some(confirmations) if !confirmations.is_empty() => confirmations,
        _ => return,
    };
    let panel = crate::ui::theme::get_palette(&state.config.theme).panel;
    let content_area = render_padded_panel_with_color(f, area, panel);

    let mut lines = Vec::new();
    let single = confirmations.len() == 1;
    let first = &confirmations[0];
    let is_command = single && first.tool_name == "run_command";
    let heading = if is_command {
        "Would you like to run the following command?".to_owned()
    } else if single {
        "Would you like to make the following change?".to_owned()
    } else {
        format!("Would you like to approve these {} tool calls?", confirmations.len())
    };
    lines.push(Line::from(Span::styled(
        format!("  {heading}"),
        Style::default().fg(COLOR_TEXT()).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    if single {
        if is_command {
            let command_width = content_area.width.saturating_sub(4) as usize;
            let command = truncate_middle_to_width(&first.path, command_width);
            for (index, command) in highlight_shell_command(&command, panel, false)
                .into_iter()
                .enumerate()
            {
                let mut spans = vec![Span::styled(
                    if index == 0 { "  $ " } else { "    " },
                    Style::default().fg(COLOR_TEXT()).bg(panel),
                )];
                spans.extend(command.spans);
                lines.push(Line::from(spans));
            }
        } else {
            let prefix_width = 2 + first.tool_name.width() + 1;
            let path = truncate_middle_to_width(
                &first.path,
                (content_area.width as usize).saturating_sub(prefix_width),
            );
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(first.tool_name.clone(), Style::default().fg(COLOR_SECONDARY())),
                Span::raw(" "),
                Span::styled(path, Style::default().fg(COLOR_TEXT())),
            ]));
        }
        for source in first.content_preview.lines().take(8) {
            let source = truncate_middle_to_width(
                source,
                content_area.width.saturating_sub(4) as usize,
            );
            let mut line = highlight_diff_line(
                &source,
                content_area.width.saturating_sub(4) as usize,
                false,
            );
            line.spans.insert(0, Span::raw("    "));
            lines.push(line);
        }
    } else {
        for confirmation in confirmations.iter().take(8) {
            let mut spans = vec![
                Span::raw("  • "),
                Span::styled(confirmation.tool_name.clone(), Style::default().fg(COLOR_SECONDARY())),
                Span::raw(" "),
            ];
            if confirmation.tool_name == "run_command" {
                spans.push(Span::styled(
                    "$ ",
                    Style::default().fg(COLOR_TEXT()).bg(panel),
                ));
                let prefix_width = spans.iter().map(|span| span.content.width()).sum::<usize>();
                let command = truncate_middle_to_width(
                    &confirmation.path,
                    (content_area.width as usize).saturating_sub(prefix_width),
                );
                if let Some(command) =
                    highlight_shell_command(&command, panel, false)
                        .into_iter()
                        .next()
                {
                    spans.extend(command.spans);
                }
            } else {
                let prefix_width = spans.iter().map(|span| span.content.width()).sum::<usize>();
                spans.push(Span::styled(
                    truncate_middle_to_width(
                        &confirmation.path,
                        (content_area.width as usize).saturating_sub(prefix_width),
                    ),
                    Style::default().fg(COLOR_TEXT()),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));
    let approve_selected = state.tool_confirmation_selected == 0;
    lines.push(Line::from(vec![
        Span::styled(
            if approve_selected { "› " } else { "  " },
            Style::default().fg(if approve_selected { COLOR_PRIMARY() } else { COLOR_MUTED() })
                .add_modifier(if approve_selected { Modifier::BOLD } else { Modifier::empty() }),
        ),
        Span::styled(
            "1. Yes, proceed",
            Style::default().fg(COLOR_TEXT())
                .add_modifier(if approve_selected { Modifier::BOLD } else { Modifier::empty() }),
        ),
        Span::styled(" (y)", Style::default().fg(COLOR_MUTED())),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            if approve_selected { "  " } else { "› " },
            Style::default().fg(if approve_selected { COLOR_MUTED() } else { COLOR_PRIMARY() })
                .add_modifier(if approve_selected { Modifier::empty() } else { Modifier::BOLD }),
        ),
        Span::styled(
            "2. No, cancel this tool call ",
            Style::default().fg(COLOR_TEXT())
                .add_modifier(if approve_selected { Modifier::empty() } else { Modifier::BOLD }),
        ),
        Span::styled("(esc)", Style::default().fg(COLOR_MUTED())),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "  Press enter to confirm · tab to {} auto-confirm",
            if state.auto_confirm { "disable" } else { "enable" }
        ),
        Style::default().fg(COLOR_MUTED()),
    )));

    if lines.len() > content_area.height as usize {
        let heading = lines.first().cloned().unwrap_or_default();
        let approve = lines
            .iter()
            .find(|line| line.to_string().contains("1. Yes, proceed"))
            .cloned()
            .unwrap_or_default();
        let cancel = lines
            .iter()
            .find(|line| line.to_string().contains("2. No, cancel"))
            .cloned()
            .unwrap_or_default();
        let target = lines
            .iter()
            .skip(1)
            .find(|line| {
                let text = line.to_string();
                !text.trim().is_empty()
                    && !text.contains("1. Yes")
                    && !text.contains("2. No")
                    && !text.contains("Press enter")
            })
            .cloned();
        let footer = lines.last().cloned();
        let mut compact = vec![heading];
        if content_area.height >= 4
            && let Some(target) = target
        {
            compact.push(target);
        }
        compact.push(approve);
        compact.push(cancel);
        if content_area.height >= 5
            && let Some(footer) = footer
        {
            compact.push(footer);
        }
        lines = compact;
    }
    paint_panel_line_backgrounds(&mut lines, panel);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(panel)),
        content_area,
    );
}

#[allow(dead_code)]
fn render_tool_confirmation_modal_legacy(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let confirmations = match &state.pending_tool_confirmation {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    let screen_height = f.area().height;

    if confirmations.len() == 1 {
        let confirmation = &confirmations[0];
        let has_preview = !confirmation.content_preview.trim().is_empty();
        let preview_lines = confirmation.content_preview.lines().count();
        let height = if has_preview {
            ((preview_lines as u16) + 8)
                .max(9)
                .min((screen_height.saturating_sub(4)).min(22))
        } else {
            7
        };
        let modal_area = input_anchor_rect(f, input_area, height);

        f.render_widget(Clear, modal_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COLOR_TIP()))
            .style(Style::default().bg(COLOR_BG()));
        f.render_widget(modal_block, modal_area);

        let inner_area = modal_area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });

        let compact = inner_area.height < 5;
        let very_compact = inner_area.height < 3;
        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if very_compact {
                [
                    Constraint::Length(1), // 0: Combined header and target
                    Constraint::Length(0), // 1: Spacer
                    Constraint::Length(0), // 2: Tool & target line
                    Constraint::Length(0), // 3: Auto-confirm status
                    Constraint::Length(0), // 4: Spacer
                    Constraint::Length(0), // 5: Preview Diff / Content
                    Constraint::Length(0), // 6: Spacer
                    Constraint::Length(1), // 7: Footer buttons
                ]
            } else if compact {
                [
                    Constraint::Length(1), // 0: Header
                    Constraint::Length(0), // 1: Spacer
                    Constraint::Length(1), // 2: Tool & target line
                    Constraint::Length(0), // 3: Auto-confirm status
                    Constraint::Length(0), // 4: Spacer
                    Constraint::Length(0), // 5: Preview Diff / Content
                    Constraint::Length(0), // 6: Spacer
                    Constraint::Length(1), // 7: Footer buttons
                ]
            } else {
                [
                    Constraint::Length(1),                               // 0: Header
                    Constraint::Length(1),                               // 1: Spacer
                    Constraint::Length(1),                               // 2: Tool & target line
                    Constraint::Length(1),                               // 3: Auto-confirm status
                    Constraint::Length(1),                               // 4: Spacer
                    Constraint::Min(if has_preview { 2 } else { 0 }),    // 5: Preview Diff / Content
                    Constraint::Length(if has_preview { 1 } else { 0 }), // 6: Spacer
                    Constraint::Length(1),                               // 7: Footer buttons
                ]
            })
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
        let header_text = if confirmation.tool_name == "run_command" {
            "⚠ Would you like to run the following command?".to_owned()
        } else {
            format!("⚠ {action_label}?")
        };
        let header_line = Line::from(vec![Span::styled(
            header_text,
            Style::default()
                .fg(COLOR_TIP())
                .add_modifier(Modifier::BOLD),
        )]);
        let path_display = if confirmation.path.len() > inner_area.width as usize - 22 {
            let cut = (inner_area.width as usize).saturating_sub(25).max(5);
            format!(
                "…{}",
                &confirmation.path[confirmation.path.len().saturating_sub(cut)..]
            )
        } else {
            confirmation.path.clone()
        };

        let size_str = if confirmation.tool_name != "run_command" && confirmation.content_bytes > 0
        {
            format!(" ({} bytes)", confirmation.content_bytes)
        } else {
            String::new()
        };

        let command_prefix = (confirmation.tool_name == "run_command").then_some("$ ");
        let tool_line = Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{:<15}", action_label),
                Style::default()
                    .fg(COLOR_TEXT())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}{}", command_prefix.unwrap_or(""), path_display),
                Style::default().fg(COLOR_PRIMARY()),
            ),
            Span::styled(size_str, Style::default().fg(COLOR_MUTED())),
        ]);
        if very_compact {
            let mut compact_spans = header_line.spans;
            compact_spans.push(Span::raw(" "));
            compact_spans.extend(tool_line.spans);
            f.render_widget(Paragraph::new(Line::from(compact_spans)), modal_chunks[0]);
        } else {
            f.render_widget(Paragraph::new(header_line), modal_chunks[0]);
            f.render_widget(Paragraph::new(tool_line), modal_chunks[2]);
        }

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
        f.render_widget(Paragraph::new(auto_confirm_line), modal_chunks[3]);

        if !confirmation.content_preview.is_empty() {
            let diff_height = modal_chunks[5].height as usize;
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
                    .split(modal_chunks[5]);

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
                    modal_chunks[5],
                );
            }
        }

        let total_lines = confirmation.content_preview.lines().count();
        let scroll_info = if modal_chunks.len() > 5
            && modal_chunks[5].height > 0
            && total_lines > modal_chunks[5].height as usize
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
                "n / esc",
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
            Span::styled(" toggle auto-confirm", Style::default().fg(COLOR_MUTED())),
            Span::styled(scroll_info, Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[7]);
    } else {
        // Render batch confirmation modal
        let height = (confirmations.len() as u16 + 7)
            .max(8)
            .min((screen_height.saturating_sub(4)).min(22));
        let modal_area = input_anchor_rect(f, input_area, height);
        f.render_widget(Clear, modal_area);
        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COLOR_TIP()))
            .style(Style::default().bg(COLOR_BG()));
        f.render_widget(modal_block, modal_area);

        let inner_area = modal_area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });

        let compact = inner_area.height < (confirmations.len() as u16).saturating_add(5);
        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if compact {
                [
                    Constraint::Length(1), // Header
                    Constraint::Length(0), // Spacer
                    Constraint::Min(0),    // Truncated list of tools
                    Constraint::Length(0), // Auto-confirm option
                    Constraint::Length(0), // Spacer
                    Constraint::Length(1), // Footer/Actions
                ]
            } else {
                [
                    Constraint::Length(1),                       // Header
                    Constraint::Length(1),                       // Spacer
                    Constraint::Min(confirmations.len() as u16), // List of tools
                    Constraint::Length(1),                       // Auto-confirm option
                    Constraint::Length(1),                       // Spacer
                    Constraint::Length(1),                       // Footer/Actions
                ]
            })
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
                let cut = (inner_area.width as usize).saturating_sub(28).max(5);
                format!("…{}", &c.path[c.path.len().saturating_sub(cut)..])
            } else {
                c.path.clone()
            };

            let marker = if i == 0 { "›" } else { " " };
            let command_prefix = (c.tool_name == "run_command").then_some("$ ");
            let line = Line::from(vec![
                Span::styled(
                    format!("{} {}. ", marker, i + 1),
                    Style::default().fg(if i == 0 { COLOR_PRIMARY() } else { COLOR_MUTED() }),
                ),
                Span::styled(
                    format!("{:<15}", action),
                    Style::default()
                        .fg(COLOR_TEXT())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}{}", command_prefix.unwrap_or(""), path_display),
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
            Span::styled("  ", Style::default()),
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
            Span::styled(" to confirm all  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "n / esc",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to cancel all  ", Style::default().fg(COLOR_MUTED())),
            Span::styled(
                "tab",
                Style::default()
                    .fg(COLOR_TIP())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" toggle auto-confirm", Style::default().fg(COLOR_MUTED())),
        ]);
        f.render_widget(Paragraph::new(footer_line), modal_chunks[5]);
    }
}

/// Interactive `ask_question` modal: renders the question and its options, with
/// the highlighted option (and, for multi-select, ticked options) emphasized.
pub(super) fn question_height(state: &AppState, width: u16, available: u16) -> u16 {
    let Some(question) = state.pending_question.as_ref() else {
        return 3;
    };
    let question_rows = textwrap_simple(&question.question, width.saturating_sub(4).max(10) as usize)
        .len() as u16;
    let option_rows = if question.custom_input.is_some() {
        1
    } else {
        question.options.len().saturating_add(1) as u16
    };
    (question_rows + option_rows + 7).min(available.max(3))
}

pub(super) fn render_question_modal(
    f: &mut Frame,
    state: &AppState,
    area: ratatui::layout::Rect,
) {
    let Some(question) = state.pending_question.as_ref() else {
        return;
    };
    let panel = crate::ui::theme::get_palette(&state.config.theme).panel;
    let content_area = render_padded_panel_with_color(f, area, panel);
    let mut lines = vec![
        Line::from(Span::styled(
            "  Question 1/1 (1 unanswered)",
            Style::default().fg(COLOR_TEXT()).add_modifier(Modifier::BOLD),
        )),
    ];
    for line in textwrap_simple(
        &question.question,
        content_area.width.saturating_sub(4).max(10) as usize,
    ) {
        lines.push(Line::from(format!("  {line}")));
    }
    lines.push(Line::from(""));

    let custom_row = if let Some(custom) = question.custom_input.as_ref() {
        let row = lines.len() as u16;
        lines.push(Line::from(vec![
            Span::styled("  › ", Style::default().fg(COLOR_PRIMARY()).add_modifier(Modifier::BOLD)),
            Span::styled(
                if custom.is_empty() { "Type your answer (optional)".to_owned() } else { custom.clone() },
                Style::default().fg(if custom.is_empty() { COLOR_MUTED() } else { COLOR_TEXT() }),
            ),
        ]));
        Some(row)
    } else {
        for (index, option) in question.options.iter().enumerate() {
            let selected = question.selected == index;
            let checked = question.chosen.get(index).copied().unwrap_or(false);
            lines.push(Line::from(vec![
                Span::styled(
                    if selected { "  › " } else { "    " },
                    Style::default().fg(if selected { COLOR_PRIMARY() } else { COLOR_TEXT() }),
                ),
                Span::styled(
                    if question.is_multi_select {
                        format!("{} {}. ", if checked { "[x]" } else { "[ ]" }, index + 1)
                    } else {
                        format!("{}. ", index + 1)
                    },
                    Style::default().fg(if selected { COLOR_PRIMARY() } else { COLOR_MUTED() }),
                ),
                Span::styled(
                    option.clone(),
                    Style::default()
                        .fg(COLOR_TEXT())
                        .add_modifier(if selected { Modifier::BOLD } else { Modifier::empty() }),
                ),
            ]));
        }
        let custom_selected = question.selected == question.options.len();
        lines.push(Line::from(vec![
            Span::styled(
                if custom_selected { "  › " } else { "    " },
                Style::default().fg(if custom_selected { COLOR_PRIMARY() } else { COLOR_TEXT() }),
            ),
            Span::styled(
                "Type your own answer",
                Style::default()
                    .fg(if custom_selected { COLOR_TEXT() } else { COLOR_MUTED() })
                    .add_modifier(if custom_selected { Modifier::BOLD } else { Modifier::empty() }),
            ),
        ]));
        None
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if question.custom_input.is_some() {
            "  enter to submit answer | esc to go back"
        } else if question.is_multi_select {
            "  space to toggle | enter to submit answer | esc to interrupt"
        } else {
            "  enter to submit answer | esc to interrupt"
        },
        Style::default().fg(COLOR_MUTED()),
    )));

    lines.truncate(content_area.height as usize);
    paint_panel_line_backgrounds(&mut lines, panel);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(panel)),
        content_area,
    );
    if let (Some(row), Some(custom)) = (custom_row, question.custom_input.as_ref())
        && row < content_area.height
    {
        let cursor = question.custom_cursor.min(custom.len());
        let cursor = custom[..cursor].width() as u16;
        f.set_cursor_position((
            content_area.x + 4 + cursor.min(content_area.width.saturating_sub(5)),
            content_area.y + row,
        ));
    }
}

#[allow(dead_code)]
fn render_question_modal_legacy(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let Some(q) = &state.pending_question else {
        return;
    };

    let screen = f.area();
    let width = input_area.width.clamp(48, screen.width.saturating_sub(4));

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
    let modal_area = input_anchor_rect(f, input_area, height);

    f.render_widget(Clear, modal_area);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COLOR_PRIMARY()))
            .style(Style::default().bg(COLOR_BG())),
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
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));

    for (i, opt) in q.options.iter().enumerate() {
        let is_sel = i == q.selected;
        let prefix_span = if is_sel {
            Span::styled(
                "❯ ",
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .bg(COLOR_BG())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("  ", Style::default().fg(COLOR_TEXT()).bg(COLOR_BG()))
        };

        let check_span = if q.is_multi_select {
            let is_checked = q.chosen.get(i).copied().unwrap_or(false);
            let check_str = if is_checked { "[x] " } else { "[ ] " };
            let style = if is_sel {
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .bg(COLOR_BG())
                    .add_modifier(Modifier::BOLD)
            } else if is_checked {
                Style::default().fg(COLOR_TIP()).bg(COLOR_BG())
            } else {
                Style::default().fg(COLOR_MUTED()).bg(COLOR_BG())
            };
            Some(Span::styled(check_str, style))
        } else {
            None
        };

        let num_str = format!("{}. ", i + 1);
        let num_span = Span::styled(
            num_str,
            if is_sel {
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .bg(COLOR_BG())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_MUTED()).bg(COLOR_BG())
            },
        );

        let opt_span = Span::styled(
            opt.to_string(),
            if is_sel {
                Style::default()
                    .fg(COLOR_PRIMARY())
                    .bg(COLOR_BG())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_TEXT()).bg(COLOR_BG())
            },
        );

        let mut row_spans = vec![prefix_span];
        if let Some(cs) = check_span {
            row_spans.push(cs);
        }
        row_spans.push(num_span);
        row_spans.push(opt_span);
        lines.push(Line::from(row_spans));
    }

    // The always-present "write your own answer" slot (index == options.len()).
    let custom_idx = q.options.len();
    let custom_sel = q.selected == custom_idx;
    let prefix_span = if custom_sel {
        Span::styled(
            "❯ ",
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  ", Style::default().fg(COLOR_TEXT()).bg(COLOR_BG()))
    };

    let icon_span = Span::styled(
        "✎ ",
        if custom_sel {
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_MUTED()).bg(COLOR_BG())
        },
    );

    if let Some(text) = &q.custom_input {
        let mut cursor_pos = q.custom_cursor.min(text.len());
        while cursor_pos > 0 && !text.is_char_boundary(cursor_pos) {
            cursor_pos -= 1;
        }
        let before_cursor = &text[..cursor_pos];
        let after_cursor = &text[cursor_pos..];

        let before_span = Span::styled(
            before_cursor.to_string(),
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        );
        let cursor_span = Span::styled(
            "│",
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        );
        let after_span = Span::styled(
            after_cursor.to_string(),
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        );

        lines.push(Line::from(vec![
            prefix_span,
            icon_span,
            before_span,
            cursor_span,
            after_span,
        ]));
    } else if custom_sel {
        let text_span = Span::styled(
            "Write your own answer…│",
            Style::default()
                .fg(COLOR_PRIMARY())
                .bg(COLOR_BG())
                .add_modifier(Modifier::BOLD),
        );
        lines.push(Line::from(vec![prefix_span, icon_span, text_span]));
    } else {
        let text_span = Span::styled(
            "Write your own answer…",
            Style::default().fg(COLOR_MUTED()).bg(COLOR_BG()),
        );
        lines.push(Line::from(vec![prefix_span, icon_span, text_span]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(COLOR_MUTED()).bg(COLOR_BG()),
    )));

    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(COLOR_BG())),
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

pub(super) fn render_theme_picker_modal(
    f: &mut Frame,
    state: &AppState,
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
    let padding_header = (inner_area.width as usize).saturating_sub(title_text.width() + right_esc.width());
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
    let selected_idx = state.theme_picker_index.min(themes.len().saturating_sub(1));

    let mut list_lines = Vec::new();
    for (idx, theme) in themes.iter().enumerate() {
        let is_selected = selected_idx == idx;
        let is_active = state.theme_picker_initial.eq_ignore_ascii_case(&theme.name);
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
    pub user_tokens: usize,
    pub assistant_tokens: usize,
    pub tool_tokens: usize,
    pub system_prompt_tokens: usize,
    pub system_tools_tokens: usize,
    pub skills_tokens: usize,
    pub subagent_tokens: usize,
    pub total_used: usize,
    pub free_tokens: usize,
}

pub fn calculate_context_breakdown(state: &AppState) -> ContextBreakdown {
    let context_window = state.active_context_window() as usize;

    let mut user_tokens = 0;
    let mut assistant_tokens = 0;
    let mut tool_tokens = 0;

    for msg in &state.history {
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
        .config
        .models
        .iter()
        .find(|m| m.url == state.api_base_url)
        .and_then(|m| m.tool_protocol)
        .unwrap_or(state.config.tool_protocol);
    let agent_mode = state.agent_mode;
    let tools_prompt = crate::tools::tool_system_prompt(state.delegation_active, protocol, agent_mode);
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

    let subagent_tokens: usize = state
        .subagents
        .iter()
        .flat_map(|sa| sa.history.iter())
        .map(crate::network::compaction::estimate_message_tokens)
        .sum();

    let total_used = user_tokens
        .saturating_add(assistant_tokens)
        .saturating_add(tool_tokens)
        .saturating_add(system_prompt_tokens)
        .saturating_add(system_tools_tokens)
        .saturating_add(skills_tokens)
        .saturating_add(subagent_tokens);

    let free_tokens = context_window.saturating_sub(total_used);

    ContextBreakdown {
        model_name: state.model_name.clone(),
        context_window,
        user_tokens,
        assistant_tokens,
        tool_tokens,
        system_prompt_tokens,
        system_tools_tokens,
        skills_tokens,
        subagent_tokens,
        total_used,
        free_tokens,
    }
}

fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub(super) fn render_context_modal(
    f: &mut Frame,
    state: &AppState,
    input_area: ratatui::layout::Rect,
) {
    let modal_area = input_anchor_rect(f, input_area, 18);
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
            Constraint::Min(6),    // Content
        ])
        .split(inner_area);

    let title_text = "context usage";
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

    let user_b = compute_blocks(breakdown.user_tokens);
    let asst_b = compute_blocks(breakdown.assistant_tokens);
    let tool_b = compute_blocks(breakdown.tool_tokens);
    let sys_p_b = compute_blocks(breakdown.system_prompt_tokens);
    let sys_t_b = compute_blocks(breakdown.system_tools_tokens);
    let skill_b = compute_blocks(breakdown.skills_tokens);
    let sub_b = compute_blocks(breakdown.subagent_tokens);

    let used_b = user_b + asst_b + tool_b + sys_p_b + sys_t_b + skill_b + sub_b;
    let free_b = total_blocks.saturating_sub(used_b);

    let color_user = Color::Rgb(100, 160, 255);
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

    push_dots(user_b, "● ", color_user);
    push_dots(asst_b, "● ", color_asst);
    push_dots(tool_b, "● ", color_tool);
    push_dots(sys_p_b, "● ", color_sys_p);
    push_dots(sys_t_b, "● ", color_sys_t);
    push_dots(skill_b, "● ", color_skill);
    push_dots(sub_b, "● ", color_sub);
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
    let total_pct = if breakdown.context_window > 0 {
        (breakdown.total_used as f64 / breakdown.context_window as f64) * 100.0
    } else {
        0.0
    };

    let free_pct = if breakdown.context_window > 0 {
        (breakdown.free_tokens as f64 / breakdown.context_window as f64) * 100.0
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
    stats_lines.push(Line::from(vec![
        Span::styled(
            format!("{} · ", breakdown.model_name),
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{}/{} tokens ({:.1}%)",
                format_token_count(breakdown.total_used),
                format_token_count(breakdown.context_window),
                total_pct
            ),
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    stats_lines.push(Line::from(vec![Span::styled(
        "Token usage by category",
        Style::default().fg(COLOR_MUTED()),
    )]));

    // Category breakdown lines
    let categories = [
        ("●", color_user, "User messages", breakdown.user_tokens, pct(breakdown.user_tokens), true),
        ("●", color_asst, "Agent responses", breakdown.assistant_tokens, pct(breakdown.assistant_tokens), true),
        ("●", color_tool, "Tool calls", breakdown.tool_tokens, pct(breakdown.tool_tokens), true),
        ("⛃", color_sys_p, "System prompt", breakdown.system_prompt_tokens, pct(breakdown.system_prompt_tokens), true),
        ("⛃", color_sys_t, "System tools", breakdown.system_tools_tokens, pct(breakdown.system_tools_tokens), true),
        ("⛃", color_skill, "Skills", breakdown.skills_tokens, pct(breakdown.skills_tokens), true),
        ("⛃", color_sub, "Subagents", breakdown.subagent_tokens, pct(breakdown.subagent_tokens), true),
        ("□", color_free, "Free space", breakdown.free_tokens, free_pct, false),
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
            Span::styled(
                label,
                Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL()),
            ),
            Span::styled(
                count_str,
                Style::default().fg(COLOR_MUTED()).bg(COLOR_PANEL()),
            ),
        ]));
    }

    f.render_widget(
        Paragraph::new(stats_lines).style(Style::default().bg(COLOR_PANEL())),
        stats_area,
    );
}
