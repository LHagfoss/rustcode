use super::*;
use crate::ui::keymap::{KeyAction, KeyMap};
use crossterm::event::KeyEvent;

pub(super) enum InputFlow {
    ContinueIteration,
    ContinueLoop,
    Exit { update: bool },
}

pub(super) struct InputContext<'a> {
    pub(super) terminal_runtime: &'a mut TerminalRuntime,
    pub(super) app_state: &'a Arc<Mutex<AppState>>,
    pub(super) client: &'a reqwest::Client,
    pub(super) current_cancel_token: &'a mut CancellationToken,
    pub(super) needs_redraw: &'a mut bool,
    pub(super) frame_requester: &'a FrameRequester,
    pub(super) terminal_focused: &'a mut bool,
    pub(super) transcript_state: &'a mut TranscriptState,
    pub(super) app_event_sender: &'a AppEventSender,
    pub(super) agent_ui_event_sender: &'a AgentUiEventSender,
    pub(super) composer: &'a ui::Composer,
    pub(super) demo_state: &'a mut Option<ui::DemoState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerSearchTarget {
    Model,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuestionEditAction {
    Handled,
    Submit,
    Paste,
}

fn insert_question_answer_text(question: &mut rustcode::controller::PendingQuestion, text: &str) {
    question.insert_str(text);
}

fn move_question_cursor_vertical(question: &mut rustcode::controller::PendingQuestion, down: bool) {
    let Some(text) = question.custom_input.as_ref() else {
        return;
    };
    let cursor = question.custom_cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return;
    }
    let before = &text[..cursor];
    let current_line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let column = before[current_line_start..].chars().count();
    if down {
        let Some(next_line_start_rel) = text[cursor..].find('\n') else {
            question.custom_cursor = text.len();
            return;
        };
        let next_line_start = cursor + next_line_start_rel + 1;
        let next_line_end = text[next_line_start..]
            .find('\n')
            .map_or(text.len(), |index| next_line_start + index);
        let target_column = column.min(text[next_line_start..next_line_end].chars().count());
        question.custom_cursor = next_line_start
            + text[next_line_start..next_line_end]
                .chars()
                .take(target_column)
                .map(char::len_utf8)
                .sum::<usize>();
    } else if current_line_start > 0 {
        let previous_line_end = current_line_start - 1;
        let previous_line_start = text[..previous_line_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let previous_line = &text[previous_line_start..previous_line_end];
        let target_column = column.min(previous_line.chars().count());
        question.custom_cursor = previous_line_start
            + previous_line
                .chars()
                .take(target_column)
                .map(char::len_utf8)
                .sum::<usize>();
    } else {
        question.custom_cursor = 0;
    }
}

fn delete_question_word_forward(question: &mut rustcode::controller::PendingQuestion) {
    let start = question.custom_cursor;
    question.move_cursor_word_right();
    let end = question.custom_cursor;
    question.custom_cursor = start;
    if start < end
        && let Some(text) = question.custom_input.as_mut()
    {
        text.replace_range(start..end, "");
    }
}

fn kill_question_line_to_start(question: &mut rustcode::controller::PendingQuestion) {
    let end = question.custom_cursor;
    let Some(text) = question.custom_input.as_mut() else {
        return;
    };
    let start = text[..end].rfind('\n').map_or(0, |index| index + 1);
    if start < end {
        text.replace_range(start..end, "");
        question.custom_cursor = start;
    }
}

fn handle_question_custom_key(
    question: &mut rustcode::controller::PendingQuestion,
    key: KeyEvent,
) -> QuestionEditAction {
    use QuestionEditAction::{Handled, Paste, Submit};

    match KeyMap::from_environment().resolve(key) {
        KeyAction::Insert(character) => question.insert_char(character),
        KeyAction::InsertNewline => question.insert_char('\n'),
        KeyAction::Submit => return Submit,
        KeyAction::Paste => return Paste,
        KeyAction::MoveLeft => question.move_cursor_left(),
        KeyAction::MoveRight => question.move_cursor_right(),
        KeyAction::MoveWordLeft => question.move_cursor_word_left(),
        KeyAction::MoveWordRight => question.move_cursor_word_right(),
        KeyAction::MoveStart => question.move_cursor_home(),
        KeyAction::MoveEnd => question.move_cursor_end(),
        KeyAction::DeleteBackward => question.delete_char_before(),
        KeyAction::DeleteForward => question.delete_char_after(),
        KeyAction::DeleteWordBackward => question.delete_word_before(),
        KeyAction::DeleteWordForward => delete_question_word_forward(question),
        KeyAction::KillLineStart => kill_question_line_to_start(question),
        KeyAction::HistoryPrevious => move_question_cursor_vertical(question, false),
        KeyAction::HistoryNext => move_question_cursor_vertical(question, true),
        // Modal and composer actions do not edit an answer.
        _ => {}
    }
    Handled
}

/// Bound list movement at the visible endpoints so repeated arrow presses can
/// reach every row without unexpectedly jumping back to the top.
fn move_picker_selection(selected: usize, len: usize, next: bool) -> usize {
    if len == 0 {
        0
    } else if next {
        selected.min(len - 1).saturating_add(1).min(len - 1)
    } else {
        selected.min(len - 1).saturating_sub(1)
    }
}

fn picker_selection_for_key(selected: usize, len: usize, key: KeyCode) -> Option<usize> {
    match key {
        KeyCode::Up => Some(move_picker_selection(selected, len, false)),
        KeyCode::Down => Some(move_picker_selection(selected, len, true)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubagentPickerAction {
    Close,
    MoveUp,
    MoveDown,
    Select,
}

fn subagent_picker_action(key: KeyCode) -> Option<SubagentPickerAction> {
    match key {
        KeyCode::Esc | KeyCode::Right => Some(SubagentPickerAction::Close),
        KeyCode::Up => Some(SubagentPickerAction::MoveUp),
        KeyCode::Down => Some(SubagentPickerAction::MoveDown),
        KeyCode::Enter | KeyCode::Left => Some(SubagentPickerAction::Select),
        _ => None,
    }
}

fn agent_context_return_target(state: &AppState, key: KeyCode) -> Option<u32> {
    if key != KeyCode::Right || !state.input_buffer.is_empty() {
        return None;
    }
    state.selected_subagent_id.and_then(|selected_id| {
        state
            .subagents
            .iter()
            .find(|agent| agent.id == selected_id)
            .map(|agent| agent.parent_id.unwrap_or(0))
    })
}

fn filtered_command_picker_items(search: &str) -> Vec<&'static crate::ui::PaletteItem> {
    let search = search.to_lowercase();
    crate::ui::PALETTE_ITEMS
        .iter()
        .filter(|item| {
            rustcode::controller::fuzzy_matches(&item.name, &search)
                || rustcode::controller::fuzzy_matches(&item.group, &search)
                || rustcode::controller::fuzzy_matches(&item.shortcut, &search)
        })
        .collect()
}

/// Route a key from a picker search field through the same editing actions as
/// the chat composer. Cursor positions are byte offsets and stay on UTF-8
/// boundaries, while Up/Down remain reserved for list navigation.
fn handle_picker_search_key(
    state: &mut AppState,
    target: PickerSearchTarget,
    key: crossterm::event::KeyEvent,
) -> bool {
    let (query, cursor, selected) = match target {
        PickerSearchTarget::Model => (
            &mut state.model_picker_search,
            &mut state.model_picker_search_cursor,
            &mut state.model_picker_index,
        ),
        PickerSearchTarget::Command => (
            &mut state.command_picker_search,
            &mut state.command_picker_search_cursor,
            &mut state.command_picker_index,
        ),
    };
    let before = query.clone();
    let handled = edit_picker_search(query, cursor, key);
    if *query != before {
        *selected = 0;
    }
    handled
}

fn edit_picker_search(
    query: &mut String,
    cursor: &mut usize,
    key: crossterm::event::KeyEvent,
) -> bool {
    use crate::ui::keymap::KeyAction;

    *cursor = picker_cursor_boundary(query, *cursor);
    let action = crate::ui::keymap::KeyMap::from_environment().resolve(key);
    match action {
        KeyAction::Insert(character) => {
            query.insert(*cursor, character);
            *cursor += character.len_utf8();
        }
        KeyAction::MoveLeft => *cursor = previous_char_boundary(query, *cursor),
        KeyAction::MoveRight => *cursor = next_char_boundary(query, *cursor),
        KeyAction::MoveWordLeft => *cursor = picker_word_left(query, *cursor),
        KeyAction::MoveWordRight => *cursor = picker_word_right(query, *cursor),
        KeyAction::MoveStart => *cursor = query[..*cursor].rfind('\n').map_or(0, |i| i + 1),
        KeyAction::MoveEnd => {
            *cursor = query[*cursor..]
                .find('\n')
                .map_or(query.len(), |i| *cursor + i)
        }
        KeyAction::DeleteBackward => {
            let start = previous_char_boundary(query, *cursor);
            query.replace_range(start..*cursor, "");
            *cursor = start;
        }
        KeyAction::DeleteForward => {
            let end = next_char_boundary(query, *cursor);
            query.replace_range(*cursor..end, "");
        }
        KeyAction::DeleteWordBackward => {
            let start = picker_word_left(query, *cursor);
            query.replace_range(start..*cursor, "");
            *cursor = start;
        }
        KeyAction::DeleteWordForward => {
            let end = picker_word_right(query, *cursor);
            query.replace_range(*cursor..end, "");
        }
        KeyAction::KillLineStart => {
            let end = *cursor;
            let start = query[..end].rfind('\n').map_or(0, |i| i + 1);
            query.replace_range(start..end, "");
            *cursor = start;
        }
        _ => return false,
    }
    true
}

fn insert_picker_search_text(state: &mut AppState, target: PickerSearchTarget, text: &str) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    match target {
        PickerSearchTarget::Model => {
            let cursor = picker_cursor_boundary(
                &state.model_picker_search,
                state.model_picker_search_cursor,
            );
            state.model_picker_search.insert_str(cursor, &normalized);
            state.model_picker_search_cursor = cursor + normalized.len();
            state.model_picker_index = 0;
        }
        PickerSearchTarget::Command => {
            let cursor = picker_cursor_boundary(
                &state.command_picker_search,
                state.command_picker_search_cursor,
            );
            state.command_picker_search.insert_str(cursor, &normalized);
            state.command_picker_search_cursor = cursor + normalized.len();
            state.command_picker_index = 0;
        }
    }
}

fn insert_mcp_edit_paste(edit_state: &mut rustcode::app::McpEditState, text: &str) {
    let (buffer, cursor) = edit_state.active_buf_and_pos_mut();
    *cursor = picker_cursor_boundary(buffer, *cursor);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for character in normalized.chars().filter(|character| *character != '\n') {
        buffer.insert(*cursor, character);
        *cursor += character.len_utf8();
    }
}

fn picker_cursor_boundary(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

async fn open_demo_if_requested(
    app_state: &Arc<Mutex<AppState>>,
    demo_state: &mut Option<ui::DemoState>,
) -> bool {
    let mut state = app_state.lock().await;
    if state.input_buffer.trim() != "/test" {
        return false;
    }

    let mut demo = ui::demo_state(rustcode::controller::render_state(&state));
    demo.freeze();
    *demo_state = Some(demo);
    state.input_buffer.clear();
    state.cursor_position = 0;
    state.reset_suggestion_cycle();
    true
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map_or(cursor, |character| cursor + character.len_utf8())
}

fn picker_word_left(text: &str, cursor: usize) -> usize {
    let mut position = cursor;
    while let Some(character) = text[..position].chars().next_back() {
        if !character.is_whitespace() {
            break;
        }
        position -= character.len_utf8();
    }
    while let Some(character) = text[..position].chars().next_back() {
        if character.is_whitespace() {
            break;
        }
        position -= character.len_utf8();
    }
    position
}

fn picker_word_right(text: &str, cursor: usize) -> usize {
    let mut position = cursor;
    while let Some(character) = text[position..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        position += character.len_utf8();
    }
    while let Some(character) = text[position..].chars().next() {
        if character.is_whitespace() {
            break;
        }
        position += character.len_utf8();
    }
    position
}

fn is_shift_tab(key: crossterm::event::KeyEvent) -> bool {
    matches!(key.code, KeyCode::BackTab)
        || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
}

fn is_transcript_navigation(key: crossterm::event::KeyEvent) -> bool {
    matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
        || (key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Up | KeyCode::Down))
}

fn is_keyboard_range_key(key: crossterm::event::KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
    ) && !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT)
}

/// Whether the transcript selection claims Esc before any other handler.
///
/// Ctrl+C is deliberately absent because it can copy and arm double-press exit;
/// Cmd+C is handled as copy-only first. Esc only dismisses the selection.
fn selection_owns_key(transcript: &TranscriptState, key: crossterm::event::KeyEvent) -> bool {
    let selection = if transcript.panel_selection_area.is_some() {
        &transcript.panel_selection
    } else {
        &transcript.selection
    };
    (selection.has_selection() || selection.is_keyboard_mode()) && key.code == KeyCode::Esc
}

fn clear_selection_for_composer_key(
    transcript: &mut TranscriptState,
    key: crossterm::event::KeyEvent,
) {
    let selection = if transcript.panel_selection_area.is_some() {
        &mut transcript.panel_selection
    } else {
        &mut transcript.selection
    };
    if (selection.is_active() || selection.has_selection() || selection.is_keyboard_mode())
        && !is_transcript_navigation(key)
    {
        selection.clear();
    }
}

/// Let scrollable output panels own wheel input without discarding a text
/// selection in fixed info panels.
fn scroll_panel_selection(
    transcript: &mut TranscriptState,
    modal_scroll_row: &mut u16,
    direction: isize,
) -> bool {
    if transcript.panel_selection_area.is_none() {
        return false;
    }
    if transcript.panel_selection_scrollable {
        transcript.panel_selection.clear();
        if direction < 0 {
            *modal_scroll_row = modal_scroll_row.saturating_sub(3);
        } else {
            *modal_scroll_row = modal_scroll_row.saturating_add(3);
        }
    }
    true
}

/// Esc re-enters follow when the transcript is not already showing the newest
/// row. Routed through [`TranscriptState::jump_to_latest`] so the keyboard and
/// the "back to bottom" control clear the same state (#1595).
fn return_to_latest_for_key(transcript: &mut TranscriptState, key: KeyCode) -> bool {
    if key != KeyCode::Esc || transcript.scroll_rows() == 0 {
        return false;
    }
    transcript.jump_to_latest();
    true
}

/// Payload a clipboard paste should insert: an image becomes
/// `![image](file://…)` markdown and wins over plain text, so pasting a
/// screenshot still renders as [Image #N].
fn clipboard_paste_payload() -> Option<String> {
    rustcode::clipboard::paste_image_from_clipboard()
        .or_else(rustcode::clipboard::read_text_from_clipboard)
}

/// Insert one clipboard payload into the composer.
///
/// Both the keymap paste and the Ctrl/Cmd+V fallback route through this, so
/// `Composer::handle_paste` stays the only place that normalizes newlines and
/// frames a large paste (#1527).
fn insert_clipboard_paste(composer: &ui::Composer, state: &mut AppState, payload: &str) {
    composer.handle_paste(state, payload);
}

async fn report_selection_copy(
    app_state: &Arc<Mutex<AppState>>,
    text: &str,
    copy: impl FnOnce(&str) -> rustcode::clipboard::ClipboardCopyStatus,
) {
    let notice = match copy(text) {
        rustcode::clipboard::ClipboardCopyStatus::Confirmed => "Copied selection to clipboard",
        rustcode::clipboard::ClipboardCopyStatus::Requested => {
            "Copy sent to terminal; paste to verify"
        }
        rustcode::clipboard::ClipboardCopyStatus::Failed => "Copy failed; try again",
    };
    app_state.lock().await.set_transient_notice(notice);
}

/// Whether this event is the Ctrl+C copy/exit chord.
fn is_copy_or_exit_chord(key: crossterm::event::KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Whether this event is Cmd+C without Control, which copies without exiting.
fn is_cmd_copy_chord(key: crossterm::event::KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        && key.modifiers.contains(KeyModifiers::SUPER)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Copy the live transcript or composer selection, if there is one.
///
/// Ctrl+C is both a copy binding and the first half of the double-press exit,
/// so this is an action the exit path performs rather than a branch that
/// consumes the key. Returns whether anything was actually copied.
async fn copy_live_selection(
    app_state: &Arc<Mutex<AppState>>,
    transcript: &TranscriptState,
) -> bool {
    copy_live_selection_with(
        app_state,
        transcript,
        rustcode::clipboard::copy_to_clipboard,
    )
    .await
}

async fn copy_live_selection_with(
    app_state: &Arc<Mutex<AppState>>,
    transcript: &TranscriptState,
    copy: impl FnOnce(&str) -> rustcode::clipboard::ClipboardCopyStatus,
) -> bool {
    // Bind the text first: an `if let` on the guard would hold the state lock
    // across `report_selection_copy`, which locks it again and hangs the TUI.
    let composer_text = app_state.lock().await.composer_selected_text();
    if let Some(text) = composer_text {
        report_selection_copy(app_state, &text, copy).await;
        return true;
    }
    let selection = if transcript.panel_selection_area.is_some() {
        &transcript.panel_selection
    } else {
        &transcript.selection
    };
    match selection.selected_text() {
        Some(text) => {
            report_selection_copy(app_state, &text, copy).await;
            true
        }
        // Keyboard-select mode can own the chord with a collapsed caret, which
        // has nothing to copy. Say so rather than failing silently.
        None => {
            if selection.is_keyboard_mode() {
                app_state
                    .lock()
                    .await
                    .set_transient_notice("Nothing selected to copy");
                true
            } else {
                false
            }
        }
    }
}

/// Handle Ctrl+C once, for both of its meanings.
///
/// A selection owns this chord so it can be copied, which previously meant the
/// key was consumed before the exit check ever ran: with any live selection the
/// double-press exit was unreachable, and a pinned selection survives an entire
/// turn. Copying is therefore an action here instead of a competing early
/// return, so press one both copies and arms, and press two exits.
async fn handle_copy_or_exit_chord(
    app_state: &Arc<Mutex<AppState>>,
    transcript: &TranscriptState,
) -> InputFlow {
    if copy_live_selection(app_state, transcript).await {
        // The copy notice is useful, but the exit hint is the more urgent of
        // the two; the footer already renders the hint from the armed state.
        app_state.lock().await.request_redraw();
    }
    if rustcode::app::handle_ctrl_c(app_state).await {
        return InputFlow::Exit { update: false };
    }
    InputFlow::ContinueIteration
}

async fn handle_cmd_copy_chord_with(
    app_state: &Arc<Mutex<AppState>>,
    transcript: &TranscriptState,
    copy: impl FnOnce(&str) -> rustcode::clipboard::ClipboardCopyStatus,
) -> InputFlow {
    // A different chord breaks the Ctrl+C double-press sequence.
    app_state.lock().await.clear_ctrl_c_exit_arming();
    if copy_live_selection_with(app_state, transcript, copy).await {
        app_state.lock().await.request_redraw();
    }
    InputFlow::ContinueIteration
}

async fn handle_cmd_copy_chord(
    app_state: &Arc<Mutex<AppState>>,
    transcript: &TranscriptState,
) -> InputFlow {
    handle_cmd_copy_chord_with(
        app_state,
        transcript,
        rustcode::clipboard::copy_to_clipboard,
    )
    .await
}

pub(super) async fn handle_app_event(
    app_event: AppEvent,
    ctx: InputContext<'_>,
) -> Result<InputFlow, Box<dyn Error>> {
    let InputContext {
        terminal_runtime,
        app_state,
        client,
        current_cancel_token,
        needs_redraw,
        frame_requester,
        terminal_focused,
        transcript_state,
        app_event_sender,
        agent_ui_event_sender,
        composer,
        demo_state,
    } = ctx;
    match app_event {
        AppEvent::ApprovalDecision(decision) => {
            apply_approval_decision(&app_state, current_cancel_token, decision).await;
            *needs_redraw = true;
        }
        AppEvent::AnswerQuestion(answer) => {
            apply_question_answer(&app_state, current_cancel_token, answer).await;
            *needs_redraw = true;
        }
        AppEvent::UpdateDecision(decision) => {
            let update_version = {
                let mut state = app_state.lock().await;
                let latest = match state.update_check {
                    rustcode_core::update::UpdateState::Available(latest) => Some(latest),
                    _ => None,
                };
                latest.filter(|_| apply_update_decision(&mut state, decision))
            };
            if let Some(update_version) = update_version {
                match run_update_command(terminal_runtime, &client, update_version).await {
                    Ok(()) => {
                        println!("🎉 Update ran successfully! Please restart rustcode.")
                    }
                    Err(error) => eprintln!("Update failed: {error}"),
                }
                return Ok(InputFlow::Exit { update: true });
            }
            *needs_redraw = true;
        }
        AppEvent::OpenOverlay(overlay) => {
            let mut state = app_state.lock().await;
            open_overlay(&mut state, overlay);
            state.request_redraw();
            *needs_redraw = true;
        }
        event @ (AppEvent::NewSession
        | AppEvent::ResumeSession(_)
        | AppEvent::ForkSession(_)
        | AppEvent::ClearSession
        | AppEvent::ArchiveSession
        | AppEvent::DeleteSession(_)) => {
            let mut state = app_state.lock().await;
            if let Err(error) = apply_session_event(&mut state, current_cancel_token, event) {
                state.set_notice(error.to_string());
                state.request_redraw();
            }
            *needs_redraw = true;
        }
        AppEvent::CloseOverlay => {
            let mut state = app_state.lock().await;
            state.overlays().close_all();
            state.request_redraw();
            *needs_redraw = true;
        }
        AppEvent::RequestDraw => {
            app_state.lock().await.request_redraw();
            *needs_redraw = true;
        }
        AppEvent::SelectSubagent(id) => {
            let mut state = app_state.lock().await;
            if let Err(error) = apply_subagent_selection(&mut state, id) {
                state.set_notice(error.to_string());
            }
            transcript_state.reset();
            *needs_redraw = true;
        }
        AppEvent::CancelActiveTurn => {
            if demo_state.is_none() {
                rustcode::app::handle_escape(&app_state, current_cancel_token).await;
                *needs_redraw = true;
            }
        }
        AppEvent::Tui(ev) if demo_state.is_some() => match ev {
            TuiEvent::Key(key)
                if key.code == KeyCode::Esc
                    && !key.modifiers.contains(event::KeyModifiers::CONTROL)
                    && !key.modifiers.contains(event::KeyModifiers::ALT)
                    && !key.modifiers.contains(event::KeyModifiers::SUPER) =>
            {
                *demo_state = None;
                *needs_redraw = true;
            }
            TuiEvent::Key(key)
                if key.code == KeyCode::Tab
                    && !key.modifiers.contains(event::KeyModifiers::CONTROL)
                    && !key.modifiers.contains(event::KeyModifiers::ALT)
                    && !key.modifiers.contains(event::KeyModifiers::SUPER) =>
            {
                if let Some(demo) = demo_state {
                    demo.toggle_question_preview();
                    *needs_redraw = true;
                }
            }
            TuiEvent::Resize { .. } | TuiEvent::Draw => *needs_redraw = true,
            _ => {}
        },
        AppEvent::Tui(ev) => match ev {
            TuiEvent::Key(key) => {
                *needs_redraw = true;
                app_state.lock().await.mark_user_activity();
                let is_ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                let is_cmd = key.modifiers.contains(event::KeyModifiers::SUPER);

                if is_ctrl && key.code == KeyCode::Char(' ') && !app_state.lock().await.modal_open()
                {
                    let snapshot = {
                        let state = app_state.lock().await;
                        ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
                            &state,
                        ))
                    };
                    transcript_state
                        .selection
                        .begin_keyboard_with_snapshot(snapshot, transcript_state.scroll_rows());
                    return Ok(InputFlow::ContinueIteration);
                }
                if transcript_state.panel_selection_area.is_none()
                    && transcript_state.selection.is_keyboard_mode()
                {
                    if is_keyboard_range_key(key) {
                        transcript_state.selection.move_keyboard(key.code);
                        return Ok(InputFlow::ContinueIteration);
                    }
                }

                // Ctrl+C runs before selection handlers so the chord can copy
                // and retain its double-press exit behavior. Cmd+C is copy-only.
                if is_copy_or_exit_chord(key) {
                    return Ok(handle_copy_or_exit_chord(&app_state, transcript_state).await);
                }
                if is_cmd_copy_chord(key) {
                    return Ok(handle_cmd_copy_chord(&app_state, transcript_state).await);
                }

                if selection_owns_key(transcript_state, key) {
                    let selection = if transcript_state.panel_selection_area.is_some() {
                        &mut transcript_state.panel_selection
                    } else {
                        &mut transcript_state.selection
                    };
                    selection.clear();
                    return Ok(InputFlow::ContinueIteration);
                }

                // Composer selection owns Esc (#1493, #1566). The copy chord is
                // already handled above so it can also arm the exit.
                if key.code == KeyCode::Esc && app_state.lock().await.has_composer_selection() {
                    app_state.lock().await.clear_composer_selection();
                    return Ok(InputFlow::ContinueIteration);
                }

                let transcript_navigation = is_transcript_navigation(key);
                clear_selection_for_composer_key(transcript_state, key);

                {
                    let mut s = app_state.lock().await;
                    s.clear_ctrl_c_exit_arming();
                }

                if (is_ctrl || is_cmd)
                    && (key.code == KeyCode::Char('k') || key.code == KeyCode::Char('K'))
                {
                    let mut s = app_state.lock().await;
                    s.request_clear_screen();
                    *needs_redraw = true;
                    return Ok(InputFlow::ContinueIteration);
                }
                if is_ctrl && (key.code == KeyCode::Char('l') || key.code == KeyCode::Char('L')) {
                    let mut s = app_state.lock().await;
                    s.request_clear_screen();
                    *needs_redraw = true;
                    return Ok(InputFlow::ContinueIteration);
                }

                {
                    let selected = {
                        let state = app_state.lock().await;
                        state
                            .show_update_prompt
                            .then_some(state.update_prompt_index)
                    };
                    if let Some(selected) = selected {
                        match key.code {
                            KeyCode::Up => {
                                let mut state = app_state.lock().await;
                                state.update_prompt_index =
                                    state.update_prompt_index.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                let mut state = app_state.lock().await;
                                state.update_prompt_index = (state.update_prompt_index + 1).min(2);
                            }
                            KeyCode::Enter => {
                                let decision = match selected {
                                    0 => UpdateDecision::UpdateNow,
                                    1 => UpdateDecision::Skip,
                                    _ => UpdateDecision::SkipUntilNextVersion,
                                };
                                let _ = app_event_sender.send(AppEvent::UpdateDecision(decision));
                            }
                            KeyCode::Esc => {
                                let _ = app_event_sender
                                    .send(AppEvent::UpdateDecision(UpdateDecision::Skip));
                            }
                            _ => {}
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }
                }

                {
                    let selected = {
                        let s = app_state.lock().await;
                        (s.status == AppStatus::AwaitingToolConfirmation && !s.user_overlay_open())
                            .then(|| {
                                let prefix = s
                                    .pending_tool_confirmation
                                    .as_ref()
                                    .filter(|items| {
                                        items.len() == 1 && items[0].rememberable_prefix.is_some()
                                            || items.len() == 1
                                                && items[0].forbidden_prefix.is_some()
                                    })
                                    .and_then(|items| items[0].rememberable_prefix.clone());
                                let forbidden_prefix = s
                                    .pending_tool_confirmation
                                    .as_ref()
                                    .filter(|items| items.len() == 1)
                                    .and_then(|items| items[0].forbidden_prefix.clone());
                                (s.tool_confirmation_selected, prefix, forbidden_prefix)
                            })
                    };
                    if let Some((selected, prefix, forbidden_prefix)) = selected {
                        if let Some(event) = ui::approval_event_for_key(
                            key,
                            selected,
                            prefix.as_deref(),
                            forbidden_prefix.as_deref(),
                        ) {
                            let _ = app_event_sender.send(event);
                        } else {
                            if is_shift_tab(key) {
                                let mut s = app_state.lock().await;
                                s.overlays().toggle_auto_confirm();
                            } else {
                                match key.code {
                                    KeyCode::Tab => {
                                        let mut s = app_state.lock().await;
                                        s.overlays().toggle_auto_confirm();
                                    }
                                    KeyCode::Up => {
                                        let mut s = app_state.lock().await;
                                        s.overlays().move_approval_selection(-1);
                                    }
                                    KeyCode::Down => {
                                        let mut s = app_state.lock().await;
                                        s.overlays().move_approval_selection(1);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }
                }

                {
                    let s = app_state.lock().await;
                    if s.status == AppStatus::AwaitingQuestion && !s.user_overlay_open() {
                        let typing = s
                            .pending_question
                            .as_ref()
                            .map(|q| q.custom_input.is_some())
                            .unwrap_or(false);
                        drop(s);

                        if typing {
                            let action = match key.code {
                                KeyCode::Tab => {
                                    app_state.lock().await.focus_question(1);
                                    QuestionEditAction::Handled
                                }
                                KeyCode::BackTab => {
                                    app_state.lock().await.focus_question(-1);
                                    QuestionEditAction::Handled
                                }
                                KeyCode::Esc => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.custom_input = None;
                                        q.custom_cursor = 0;
                                    }
                                    QuestionEditAction::Handled
                                }
                                _ => {
                                    let mut s = app_state.lock().await;
                                    s.pending_question
                                        .as_mut()
                                        .map_or(QuestionEditAction::Handled, |q| {
                                            handle_question_custom_key(q, key)
                                        })
                                }
                            };
                            match action {
                                QuestionEditAction::Submit => {
                                    let answer_event = {
                                        let s = app_state.lock().await;
                                        s.pending_question
                                            .as_ref()
                                            .map(ui::question_custom_answer_event)
                                    };
                                    if let Some(answer_event) = answer_event {
                                        let _ = app_event_sender.send(answer_event);
                                    }
                                }
                                QuestionEditAction::Paste => {
                                    if let Some(text) =
                                        rustcode::clipboard::read_text_from_clipboard()
                                    {
                                        let mut s = app_state.lock().await;
                                        if let Some(q) = s.pending_question.as_mut() {
                                            insert_question_answer_text(q, &text);
                                        }
                                    }
                                }
                                QuestionEditAction::Handled => {}
                            }
                            *needs_redraw = true;
                            return Ok(InputFlow::ContinueIteration);
                        }

                        match key.code {
                            KeyCode::Up => {
                                let mut s = app_state.lock().await;
                                if let Some(q) = s.pending_question.as_mut() {
                                    q.selected = q.selected.saturating_sub(1);
                                }
                            }
                            KeyCode::Down => {
                                let mut s = app_state.lock().await;
                                if let Some(q) = s.pending_question.as_mut() {
                                    let last = q.options.len();
                                    q.selected = (q.selected + 1).min(last);
                                    if q.selected == last {
                                        q.activate_custom_input();
                                    }
                                }
                            }
                            KeyCode::Tab => {
                                let mut s = app_state.lock().await;
                                s.focus_question(1);
                            }
                            KeyCode::BackTab => {
                                let mut s = app_state.lock().await;
                                s.focus_question(-1);
                            }
                            KeyCode::Char(' ') => {
                                let mut s = app_state.lock().await;
                                if let Some(q) = s.pending_question.as_mut() {
                                    if q.selected == q.options.len() {
                                        q.activate_custom_input();
                                    } else if q.is_multi_select
                                        && let Some(c) = q.chosen.get_mut(q.selected)
                                    {
                                        *c = !*c;
                                    }
                                }
                            }
                            KeyCode::Char(d @ '1'..='9') => {
                                let idx = (d as usize) - ('1' as usize);
                                let mut s = app_state.lock().await;
                                if let Some(q) = s.pending_question.as_mut()
                                    && idx < q.options.len()
                                {
                                    q.selected = idx;
                                    if q.is_multi_select {
                                        if let Some(c) = q.chosen.get_mut(idx) {
                                            *c = !*c;
                                        }
                                    } else {
                                        let answer_event = ui::question_answer_event(q);
                                        if let Some(answer_event) = answer_event {
                                            let _ = app_event_sender.send(answer_event);
                                        }
                                    }
                                }
                            }
                            KeyCode::Char(c) => {
                                let mut s = app_state.lock().await;
                                if let Some(q) = s.pending_question.as_mut() {
                                    if q.selected == q.options.len() {
                                        q.activate_custom_input();
                                        q.insert_char(c);
                                    }
                                }
                            }
                            KeyCode::Enter => {
                                let mut s = app_state.lock().await;
                                let is_custom_slot = s
                                    .pending_question
                                    .as_ref()
                                    .map(|q| q.selected == q.options.len())
                                    .unwrap_or(false);
                                if is_custom_slot {
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.activate_custom_input();
                                    }
                                } else if let Some(q) = s.pending_question.as_ref()
                                    && let Some(answer_event) = ui::question_answer_event(q)
                                {
                                    let _ = app_event_sender.send(answer_event);
                                }
                            }
                            KeyCode::Esc => {
                                let _ = app_event_sender.send(ui::question_cancel_event());
                            }
                            _ => {}
                        }
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                }

                {
                    let s = app_state.lock().await;
                    if s.settings_picker == Some(rustcode::controller::SettingsPicker::Verbosity) {
                        drop(s);
                        match key.code {
                            KeyCode::Up => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index =
                                    s.modal_picker_index.saturating_add(1).min(1); // 0 for Low, 1 for High
                            }
                            KeyCode::Enter => {
                                let mut s = app_state.lock().await;
                                let new_verbosity = match s.modal_picker_index {
                                    0 => Verbosity::Low,
                                    1 => Verbosity::High,
                                    _ => Verbosity::Low, // Should not happen
                                };
                                s.verbosity = new_verbosity.clone();
                                s.config.verbosity = new_verbosity;
                                rustcode::controller::save_config(&s.config);
                                s.close_modal_status();
                            }
                            KeyCode::Esc => {
                                let mut s = app_state.lock().await;
                                s.close_modal_status();
                            }
                            _ => {}
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }

                    if s.settings_picker == Some(rustcode::controller::SettingsPicker::Thinking) {
                        drop(s);
                        match key.code {
                            KeyCode::Up => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index =
                                    s.modal_picker_index.saturating_add(1).min(2); // 0 on, 1 off, 2 default
                            }
                            KeyCode::Enter => {
                                let mut s = app_state.lock().await;
                                let value = match s.modal_picker_index {
                                    0 => Some(true),
                                    1 => Some(false),
                                    _ => None,
                                };
                                let url = s.api_base_url.clone();
                                if let Some(profile) =
                                    s.config.models.iter_mut().find(|p| p.url == url)
                                {
                                    profile.enable_thinking = value;
                                }
                                rustcode::controller::save_config(&s.config);
                                s.close_modal_status();
                            }
                            KeyCode::Esc => {
                                let mut s = app_state.lock().await;
                                s.close_modal_status();
                            }
                            _ => {}
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }

                    if s.settings_picker == Some(rustcode::controller::SettingsPicker::Effort) {
                        drop(s);
                        match key.code {
                            KeyCode::Up => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index =
                                    s.modal_picker_index.saturating_add(1).min(3); // 0 low, 1 medium, 2 high, 3 off
                            }
                            KeyCode::Enter => {
                                let mut s = app_state.lock().await;
                                let value = match s.modal_picker_index {
                                    0 => Some("low".to_string()),
                                    1 => Some("medium".to_string()),
                                    2 => Some("high".to_string()),
                                    _ => None,
                                };
                                let url = s.api_base_url.clone();
                                if let Some(profile) =
                                    s.config.models.iter_mut().find(|p| p.url == url)
                                {
                                    profile.reasoning_effort = value;
                                }
                                rustcode::controller::save_config(&s.config);
                                s.close_modal_status();
                            }
                            KeyCode::Esc => {
                                let mut s = app_state.lock().await;
                                s.close_modal_status();
                            }
                            _ => {}
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }

                    if s.settings_picker == Some(rustcode::controller::SettingsPicker::Protocol) {
                        drop(s);
                        match key.code {
                            KeyCode::Up => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index =
                                    s.modal_picker_index.saturating_add(1).min(2); // 0 json, 1 native, 2 apinative
                            }
                            KeyCode::Enter => {
                                let mut s = app_state.lock().await;
                                let (protocol, label) = match s.modal_picker_index {
                                    0 => (rustcode::config::ToolProtocol::Json, "JSON (```tool)"),
                                    1 => (
                                        rustcode::config::ToolProtocol::Native,
                                        "Native ([TOOL_CALLS])",
                                    ),
                                    _ => (
                                        rustcode::config::ToolProtocol::ApiNative,
                                        "ApiNative (schema in request `tools`, structured `tool_calls` back)",
                                    ),
                                };
                                let url = s.api_base_url.clone();
                                let scoped = s
                                    .config
                                    .models
                                    .iter_mut()
                                    .find(|profile| profile.url == url);
                                if let Some(profile) = scoped {
                                    profile.tool_protocol = Some(protocol);
                                } else {
                                    s.config.tool_protocol = protocol;
                                }
                                rustcode::controller::save_config(&s.config);
                                let active_model = s.model_name.clone();
                                s.set_transient_notice(format!(
                                    "Switched tool protocol to {label} for model '{active_model}'."
                                ));
                                s.close_modal_status();
                            }
                            KeyCode::Esc => {
                                let mut s = app_state.lock().await;
                                s.close_modal_status();
                            }
                            _ => {}
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }

                    if s.settings_picker == Some(rustcode::controller::SettingsPicker::Yolo) {
                        drop(s);
                        match key.code {
                            KeyCode::Up => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                let mut s = app_state.lock().await;
                                s.modal_picker_index =
                                    s.modal_picker_index.saturating_add(1).min(1); // 0 on, 1 off
                            }
                            KeyCode::Enter => {
                                let mut s = app_state.lock().await;
                                let enable = s.modal_picker_index == 0;
                                s.auto_confirm = enable;
                                let status = if enable { "enabled" } else { "disabled" };
                                s.set_transient_notice(format!("YOLO mode {status}"));
                                s.close_modal_status();
                            }
                            KeyCode::Esc => {
                                let mut s = app_state.lock().await;
                                s.close_modal_status();
                            }
                            _ => {}
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }
                }

                let mut s = app_state.lock().await;
                if s.show_subagent_picker {
                    let total = s.subagents.len() + 1;
                    match subagent_picker_action(key.code) {
                        Some(SubagentPickerAction::Close) => {
                            s.show_subagent_picker = false;
                        }
                        Some(SubagentPickerAction::MoveUp) => {
                            s.subagent_picker_index = picker_selection_for_key(
                                s.subagent_picker_index,
                                total,
                                KeyCode::Up,
                            )
                            .unwrap_or(s.subagent_picker_index);
                        }
                        Some(SubagentPickerAction::MoveDown) => {
                            s.subagent_picker_index = picker_selection_for_key(
                                s.subagent_picker_index,
                                total,
                                KeyCode::Down,
                            )
                            .unwrap_or(s.subagent_picker_index);
                        }
                        Some(SubagentPickerAction::Select) => {
                            let selected = s.subagent_picker_index.min(total.saturating_sub(1));
                            let id = if selected == 0 {
                                0
                            } else {
                                s.subagents[selected - 1].id
                            };
                            s.show_subagent_picker = false;
                            drop(s);
                            let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                            return Ok(InputFlow::ContinueIteration);
                        }
                        None => {}
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.command_panel.is_some() {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q') => {
                            s.command_panel = None;
                        }
                        KeyCode::Up => s.modal_scroll_row = s.modal_scroll_row.saturating_sub(1),
                        KeyCode::Down => s.modal_scroll_row = s.modal_scroll_row.saturating_add(1),
                        KeyCode::PageUp => {
                            s.modal_scroll_row = s.modal_scroll_row.saturating_sub(10)
                        }
                        KeyCode::PageDown => {
                            s.modal_scroll_row = s.modal_scroll_row.saturating_add(10)
                        }
                        _ => {}
                    }
                    return Ok(InputFlow::ContinueIteration);
                }
                if s.show_context_modal {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q') => {
                            s.show_context_modal = false;
                        }
                        _ => {}
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_status_modal {
                    if matches!(
                        key.code,
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q')
                    ) {
                        s.show_status_modal = false;
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_stats_modal {
                    if matches!(
                        key.code,
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q')
                    ) {
                        s.show_stats_modal = false;
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_session_modal {
                    if matches!(
                        key.code,
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q')
                    ) {
                        s.show_session_modal = false;
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_history_picker {
                    // Ctrl+D triggers delete confirmation overlay
                    if key.modifiers.contains(event::KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('d')
                    {
                        let idx = s
                            .history_picker_index
                            .min(s.history_picker_sessions.len().saturating_sub(1));
                        s.pending_delete_session_idx = Some(idx);
                        drop(s);
                        return Ok(InputFlow::ContinueIteration);
                    }

                    // Confirmation overlay for delete
                    if let Some(del_idx) = s.pending_delete_session_idx {
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Enter => {
                                let action = s
                                    .history_picker_sessions
                                    .get(del_idx)
                                    .and_then(
                                        rustcode::app::session_controller::session_id_from_meta,
                                    )
                                    .map(rustcode::app::events::SessionAction::Id);
                                s.pending_delete_session_idx = None;
                                if let Some(action) = action {
                                    let _ = app_event_sender.send(AppEvent::DeleteSession(action));
                                }
                            }
                            KeyCode::Esc | KeyCode::Char('n') => {
                                s.pending_delete_session_idx = None;
                            }
                            _ => {}
                        }
                        drop(s);
                        return Ok(InputFlow::ContinueIteration);
                    }

                    match key.code {
                        KeyCode::Esc => {
                            s.show_history_picker = false;
                        }
                        KeyCode::Up => {
                            let len = s.history_picker_sessions.len();
                            s.history_picker_index =
                                picker_selection_for_key(s.history_picker_index, len, key.code)
                                    .unwrap_or(s.history_picker_index);
                        }
                        KeyCode::Down => {
                            let len = s.history_picker_sessions.len();
                            s.history_picker_index =
                                picker_selection_for_key(s.history_picker_index, len, key.code)
                                    .unwrap_or(s.history_picker_index);
                        }
                        KeyCode::Enter => {
                            let idx = s
                                .history_picker_index
                                .min(s.history_picker_sessions.len().saturating_sub(1));
                            if let Some(action) = s
                                .history_picker_sessions
                                .get(idx)
                                .and_then(rustcode::app::session_controller::session_id_from_meta)
                                .map(rustcode::app::events::SessionAction::Id)
                            {
                                let _ = app_event_sender.send(AppEvent::ResumeSession(action));
                            }
                        }
                        _ => {}
                    }

                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_mcp_config {
                    let (existing_mcp_always_include, existing_mcp_client_id) = s
                        .mcp_edit_state
                        .as_ref()
                        .and_then(|edit_state| edit_state.edit_index)
                        .and_then(|idx| s.config.mcp_servers.get(idx))
                        .map(|server| (server.always_include, server.client_id.clone()))
                        .unwrap_or_default();
                    if let Some(ref mut edit_state) = s.mcp_edit_state {
                        if crate::ui::keymap::KeyMap::from_environment().resolve(key)
                            == crate::ui::keymap::KeyAction::Paste
                        {
                            if let Some(payload) = clipboard_paste_payload() {
                                insert_mcp_edit_paste(edit_state, &payload);
                            }
                            drop(s);
                            *needs_redraw = true;
                            return Ok(InputFlow::ContinueIteration);
                        }
                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        let alt = key.modifiers.contains(KeyModifiers::ALT);
                        let super_key = key.modifiers.contains(KeyModifiers::SUPER);

                        match key.code {
                            KeyCode::Esc => {
                                s.mcp_edit_state = None;
                            }
                            KeyCode::Up => {
                                let prev = if edit_state.active_field == 0 {
                                    2
                                } else {
                                    edit_state.active_field - 1
                                };
                                edit_state.set_active_field(prev);
                            }
                            KeyCode::Down | KeyCode::Tab => {
                                let next = (edit_state.active_field + 1) % 3;
                                edit_state.set_active_field(next);
                            }
                            KeyCode::Left => {
                                if alt || ctrl {
                                    edit_state.move_cursor_word_left();
                                } else {
                                    edit_state.move_cursor_left();
                                }
                            }
                            KeyCode::Right => {
                                if alt || ctrl {
                                    edit_state.move_cursor_word_right();
                                } else {
                                    edit_state.move_cursor_right();
                                }
                            }
                            KeyCode::Home => {
                                edit_state.move_cursor_home();
                            }
                            KeyCode::End => {
                                edit_state.move_cursor_end();
                            }
                            KeyCode::Backspace => {
                                if super_key {
                                    edit_state.delete_line_left();
                                } else if alt || ctrl {
                                    edit_state.delete_word_left();
                                } else {
                                    edit_state.delete_char_left();
                                }
                            }
                            KeyCode::Delete => {
                                edit_state.delete_char_right();
                            }
                            KeyCode::Char(c) => {
                                if ctrl && (c == 'w' || c == 'W') {
                                    edit_state.delete_word_left();
                                } else if ctrl && (c == 'u' || c == 'U') {
                                    edit_state.delete_line_left();
                                } else if !ctrl && !super_key {
                                    edit_state.insert_char(c);
                                }
                            }
                            KeyCode::Enter => {
                                let name = edit_state.name_input.trim().to_string();
                                let command = edit_state.command_input.trim().to_string();
                                let args = edit_state
                                    .args_input
                                    .split_whitespace()
                                    .map(|s| s.to_string())
                                    .collect::<Vec<_>>();

                                // An http(s) value in the command field declares a
                                // remote Streamable HTTP server; anything else is
                                // spawned over stdio.
                                let remote = command.starts_with("http://")
                                    || command.starts_with("https://");
                                if !name.is_empty() && !command.is_empty() {
                                    let url = remote.then(|| command.clone());
                                    let new_srv = rustcode::config::McpServerConfig {
                                        name: name.clone(),
                                        command: if remote { String::new() } else { command },
                                        // Arguments only apply to stdio servers.
                                        args: if remote { Vec::new() } else { args },
                                        env: std::collections::HashMap::new(),
                                        url,
                                        headers: std::collections::HashMap::new(),
                                        client_id: existing_mcp_client_id,
                                        enabled: true,
                                        always_include: existing_mcp_always_include,
                                    };

                                    if edit_state.is_add {
                                        s.config.mcp_servers.push(new_srv);
                                    } else if let Some(idx) = edit_state.edit_index
                                        && idx < s.config.mcp_servers.len()
                                    {
                                        let old_name = s.config.mcp_servers[idx].name.clone();
                                        s.config.mcp_servers[idx] = new_srv;
                                        if old_name != name {
                                            rustcode::mcp::shutdown_server(&old_name).await;
                                        }
                                    }

                                    rustcode::controller::save_config(&s.config);

                                    let name_clone = name.clone();
                                    tokio::spawn(async move {
                                        let _ =
                                            rustcode::mcp::start_server_by_name(&name_clone).await;
                                    });

                                    s.mcp_edit_state = None;
                                }
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Esc => {
                                s.show_mcp_config = false;
                            }
                            KeyCode::Up => {
                                let len = s.config.mcp_servers.len();
                                s.mcp_picker_index =
                                    picker_selection_for_key(s.mcp_picker_index, len, key.code)
                                        .unwrap_or(s.mcp_picker_index);
                            }
                            KeyCode::Down => {
                                let len = s.config.mcp_servers.len();
                                s.mcp_picker_index =
                                    picker_selection_for_key(s.mcp_picker_index, len, key.code)
                                        .unwrap_or(s.mcp_picker_index);
                            }
                            KeyCode::Char('a') | KeyCode::Char('A') => {
                                s.mcp_edit_state = Some(rustcode::app::McpEditState {
                                    is_add: true,
                                    edit_index: None,
                                    name_input: String::new(),
                                    command_input: String::new(),
                                    args_input: String::new(),
                                    active_field: 0,
                                    cursor_pos: 0,
                                });
                            }
                            KeyCode::Char('e') | KeyCode::Char('E') => {
                                let idx = s.mcp_picker_index;
                                if let Some(srv) = s.config.mcp_servers.get(idx) {
                                    s.mcp_edit_state = Some(rustcode::app::McpEditState {
                                        is_add: false,
                                        edit_index: Some(idx),
                                        name_input: srv.name.clone(),
                                        command_input: if srv.is_remote() {
                                            srv.url.clone().unwrap_or_default()
                                        } else {
                                            srv.command.clone()
                                        },
                                        args_input: srv.args.join(" "),
                                        active_field: 0,
                                        cursor_pos: srv.name.len(),
                                    });
                                }
                            }
                            KeyCode::Char('d') | KeyCode::Char('D') => {
                                let idx = s.mcp_picker_index;
                                if idx < s.config.mcp_servers.len() {
                                    let removed = s.config.mcp_servers.remove(idx);
                                    rustcode::controller::save_config(&s.config);
                                    let name_clone = removed.name.clone();
                                    tokio::spawn(async move {
                                        rustcode::mcp::shutdown_server(&name_clone).await;
                                    });
                                    if s.mcp_picker_index >= s.config.mcp_servers.len()
                                        && s.mcp_picker_index > 0
                                    {
                                        s.mcp_picker_index -= 1;
                                    }
                                }
                            }
                            KeyCode::Enter => {
                                let idx = s.mcp_picker_index;
                                if let Some(srv) = s.config.mcp_servers.get_mut(idx) {
                                    srv.enabled = !srv.enabled;
                                    let name_clone = srv.name.clone();
                                    let enabled = srv.enabled;
                                    rustcode::controller::save_config(&s.config);
                                    tokio::spawn(async move {
                                        if enabled {
                                            let _ =
                                                rustcode::mcp::start_server_by_name(&name_clone)
                                                    .await;
                                        } else {
                                            rustcode::mcp::shutdown_server(&name_clone).await;
                                        }
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_model_picker {
                    if crate::ui::keymap::KeyMap::from_environment().resolve(key)
                        == crate::ui::keymap::KeyAction::Paste
                    {
                        if let Some(payload) = clipboard_paste_payload() {
                            insert_picker_search_text(&mut s, PickerSearchTarget::Model, &payload);
                        }
                        drop(s);
                        return Ok(InputFlow::ContinueIteration);
                    }
                    match key.code {
                        KeyCode::Esc => {
                            s.show_model_picker = false;
                        }
                        KeyCode::Up => {
                            let len = rustcode::app::get_picker_items_count(&s);
                            s.model_picker_index =
                                picker_selection_for_key(s.model_picker_index, len, key.code)
                                    .unwrap_or(s.model_picker_index);
                        }
                        KeyCode::Down => {
                            let len = rustcode::app::get_picker_items_count(&s);
                            s.model_picker_index =
                                picker_selection_for_key(s.model_picker_index, len, key.code)
                                    .unwrap_or(s.model_picker_index);
                        }
                        KeyCode::Enter => {
                            rustcode::app::select_picker_model(&mut s);
                            s.show_model_picker = false;
                            rustcode::app::spawn_context_window_detection(
                                Arc::clone(&app_state),
                                client.clone(),
                            );
                        }
                        _ => {
                            handle_picker_search_key(&mut s, PickerSearchTarget::Model, key);
                        }
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_theme_picker {
                    let themes = crate::ui::theme::load_available_themes();
                    let len = themes.len();
                    match key.code {
                        KeyCode::Esc => {
                            s.config.theme = s.theme_picker_initial.clone();
                            s.show_theme_picker = false;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            s.theme_picker_index =
                                move_picker_selection(s.theme_picker_index, len, false);
                            if len > 0 {
                                s.config.theme = themes[s.theme_picker_index].name.clone();
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            s.theme_picker_index =
                                move_picker_selection(s.theme_picker_index, len, true);
                            if len > 0 {
                                s.config.theme = themes[s.theme_picker_index].name.clone();
                            }
                        }
                        KeyCode::Enter => {
                            let selected = themes[s.theme_picker_index.min(len.saturating_sub(1))]
                                .name
                                .clone();
                            s.config.theme = selected.clone();
                            s.show_theme_picker = false;
                            rustcode::controller::save_config(&s.config);
                            s.set_transient_notice(format!("Theme set to '{}'", selected));
                        }
                        _ => {}
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_command_picker {
                    let filtered_items = filtered_command_picker_items(&s.command_picker_search);

                    if crate::ui::keymap::KeyMap::from_environment().resolve(key)
                        == crate::ui::keymap::KeyAction::Paste
                    {
                        if let Some(payload) = clipboard_paste_payload() {
                            insert_picker_search_text(
                                &mut s,
                                PickerSearchTarget::Command,
                                &payload,
                            );
                        }
                        drop(s);
                        return Ok(InputFlow::ContinueIteration);
                    }

                    let mut exit_flag = false;
                    match key.code {
                        KeyCode::Esc => {
                            s.show_command_picker = false;
                        }
                        KeyCode::Up => {
                            let len = filtered_items.len();
                            s.command_picker_index =
                                picker_selection_for_key(s.command_picker_index, len, key.code)
                                    .unwrap_or(s.command_picker_index);
                        }
                        KeyCode::Down => {
                            let len = filtered_items.len();
                            s.command_picker_index =
                                picker_selection_for_key(s.command_picker_index, len, key.code)
                                    .unwrap_or(s.command_picker_index);
                        }
                        KeyCode::Enter => {
                            let idx = s
                                .command_picker_index
                                .min(filtered_items.len().saturating_sub(1));
                            if !filtered_items.is_empty() {
                                let item = filtered_items[idx];
                                s.show_command_picker = false;
                                if item.shortcut == "ctrl+c" {
                                    exit_flag = true;
                                } else {
                                    // Palette commands share slash dispatch, including panel
                                    // presentation, arguments and immediate actions.
                                    s.input_buffer = item.shortcut.to_owned();
                                    s.cursor_position = s.input_buffer.len();
                                    drop(s);
                                    if open_demo_if_requested(app_state, demo_state).await {
                                        *needs_redraw = true;
                                        return Ok(InputFlow::ContinueIteration);
                                    }
                                    let should_exit = rustcode::app::handle_enter_with_ui_events(
                                        app_state,
                                        client,
                                        current_cancel_token,
                                        agent_ui_event_sender.clone(),
                                        &|| {
                                            crate::ui::theme::load_available_themes()
                                                .into_iter()
                                                .map(|theme| theme.name)
                                                .collect()
                                        },
                                    )
                                    .await;
                                    *needs_redraw = true;
                                    return Ok(if should_exit {
                                        InputFlow::Exit { update: false }
                                    } else {
                                        InputFlow::ContinueIteration
                                    });
                                }
                            } else {
                                s.show_command_picker = false;
                            }
                        }
                        _ => {
                            handle_picker_search_key(&mut s, PickerSearchTarget::Command, key);
                        }
                    }
                    drop(s);
                    if exit_flag {
                        return Ok(InputFlow::Exit { update: false });
                    }
                    return Ok(InputFlow::ContinueIteration);
                }
                drop(s);
                if transcript_navigation {
                    let page = terminal_runtime.terminal().area().height.saturating_sub(4) as usize;
                    // Keep every intermediate row visible while a mouse range is growing.
                    let page = if transcript_state.selection.is_dragging() {
                        1
                    } else {
                        page.max(1)
                    };
                    if transcript_state.selection.is_active() {
                        let direction = if matches!(key.code, KeyCode::PageUp | KeyCode::Up) {
                            -1
                        } else {
                            1
                        };
                        transcript_state.selection.queue_scroll(direction, page);
                        frame_requester.schedule_frame();
                    } else if matches!(key.code, KeyCode::PageUp | KeyCode::Up) {
                        transcript_state.scroll_up(page);
                    } else {
                        transcript_state.scroll_down(page);
                    }
                    return Ok(InputFlow::ContinueIteration);
                }
                // Escape closes transcript browsing without discarding a draft.
                if return_to_latest_for_key(transcript_state, key.code) {
                    return Ok(InputFlow::ContinueIteration);
                }
                if key.code == KeyCode::Enter
                    && !key.modifiers.intersects(
                        event::KeyModifiers::SHIFT
                            | event::KeyModifiers::CONTROL
                            | event::KeyModifiers::ALT,
                    )
                    && open_demo_if_requested(app_state, demo_state).await
                {
                    return Ok(InputFlow::ContinueIteration);
                }
                match {
                    let mut state = app_state.lock().await;
                    composer.handle_key(&mut state, key)
                } {
                    ui::ComposerAction::Handled => {
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    ui::ComposerAction::Submit => {
                        if rustcode::app::handle_enter_with_ui_events(
                            &app_state,
                            &client,
                            current_cancel_token,
                            agent_ui_event_sender.clone(),
                            &|| {
                                crate::ui::theme::load_available_themes()
                                    .into_iter()
                                    .map(|t| t.name)
                                    .collect()
                            },
                        )
                        .await
                        {
                            return Ok(InputFlow::Exit { update: false });
                        }
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    ui::ComposerAction::ClearScreen => {
                        terminal_runtime.terminal().clear()?;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    ui::ComposerAction::Paste => {
                        if let Some(payload) = clipboard_paste_payload() {
                            let mut state = app_state.lock().await;
                            insert_clipboard_paste(composer, &mut state, &payload);
                        }
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    action @ (ui::ComposerAction::ToggleExpandAll
                    | ui::ComposerAction::ToggleExpandStep) => {
                        let step = matches!(action, ui::ComposerAction::ToggleExpandStep);
                        let mut state = app_state.lock().await;
                        let width = terminal_runtime.terminal().area().width;
                        let snapshot = ui::render_snapshot::render_snapshot(
                            &rustcode::controller::render_state(&state),
                        );
                        let high_verbosity =
                            matches!(snapshot.verbosity(), rustcode::controller::Verbosity::High);
                        let candidates = ui::collapsible_tool_indices(&snapshot, width);
                        if candidates.is_empty() {
                            // A press at high verbosity used to be a silent
                            // no-op: the renderers already show bodies inline,
                            // so there is nothing to expand. Say so instead of
                            // reporting the misleading "Nothing to expand"
                            // (#1594).
                            let notice = if high_verbosity {
                                "Tool bodies are already shown at high verbosity"
                            } else {
                                "No collapsed tool output"
                            };
                            state.set_transient_notice(notice);
                        } else if step {
                            // The single-entry step keeps the focus-driven walk
                            // that the whole-transcript toggle replaced.
                            rustcode::controller::toggle_expanded_thought(&mut state, &candidates);
                        } else {
                            rustcode::controller::toggle_all_expanded_thoughts(
                                &mut state,
                                &candidates,
                            );
                        }
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    ui::ComposerAction::Unhandled => {}
                }

                match key.code {
                    KeyCode::Esc => {
                        let mut s = app_state.lock().await;
                        if s.dismiss_completion() {
                            // Popup dismissal keeps the draft intact. Typing or moving
                            // to another token makes completion eligible again.
                        } else if s.sel_start.is_some() || s.sel_end.is_some() {
                            s.clear_selection();
                        } else if !s.input_buffer.is_empty()
                            && matches!(s.status, AppStatus::Idle)
                            && s.running_tools.is_empty()
                        {
                            s.input_buffer.clear();
                            s.cursor_position = 0;
                        } else {
                            drop(s);
                            rustcode::app::handle_escape(&app_state, current_cancel_token).await;
                        }
                        *needs_redraw = true;
                    }
                    KeyCode::Up => {
                        let mut s = app_state.lock().await;
                        let completion_len =
                            rustcode::app::get_completion_len(&s.input_buffer, s.cursor_position);
                        if s.active_suggestion_index.is_some() && completion_len > 0 {
                            let current = s.active_suggestion_index.unwrap_or(0);
                            s.active_suggestion_index = Some(if current == 0 {
                                completion_len - 1
                            } else {
                                current - 1
                            });
                        } else {
                            s.active_suggestion_index = None;
                            if s.input_buffer.is_empty() || s.history_index.is_some() {
                                // With an empty buffer, Up first pulls the most
                                // recent queued prompt back for editing; only
                                // when nothing is queued does it recall history.
                                // Once recall has started, keep walking it —
                                // without this, the recalled text made the buffer
                                // non-empty and the next Up fell through to
                                // cursor movement, pinning recall on the most
                                // recent entry.
                                let pulled = s.history_index.is_none() && s.pop_queued_prompt();
                                if !pulled {
                                    s.history_up();
                                }
                            } else {
                                s.move_cursor_line_up();
                            }
                        }
                    }
                    KeyCode::Down => {
                        let mut s = app_state.lock().await;
                        let completion_len =
                            rustcode::app::get_completion_len(&s.input_buffer, s.cursor_position);
                        if s.active_suggestion_index.is_some() && completion_len > 0 {
                            let current = s.active_suggestion_index.unwrap_or(0);
                            s.active_suggestion_index = Some(if current + 1 >= completion_len {
                                0
                            } else {
                                current + 1
                            });
                        } else {
                            s.active_suggestion_index = None;
                            if s.history_index.is_some() {
                                s.history_down();
                            } else {
                                s.move_cursor_line_down();
                            }
                        }
                    }
                    KeyCode::Left => {
                        let mut s = app_state.lock().await;
                        // An empty composer has no cursor to move, so Left opens
                        // the agent contexts the way Codex opens its agents overview.
                        if key.modifiers.is_empty()
                            && s.input_buffer.is_empty()
                            && !s.subagents.is_empty()
                        {
                            open_overlay(&mut s, rustcode::app::events::Overlay::Subagents);
                            return Ok(InputFlow::ContinueIteration);
                        }
                        let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
                        // Word motion wins whenever there is a draft; only an
                        // empty composer gives Alt+arrows to agent switching.
                        if alt
                            && !shift
                            && s.input_buffer.is_empty()
                            && let Some(id) = sessions::adjacent_context_id(&s, false)
                        {
                            drop(s);
                            let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                            return Ok(InputFlow::ContinueIteration);
                        }
                        if shift && s.composer_selection_anchor.is_none() {
                            s.composer_selection_anchor = Some(s.cursor_position);
                        }
                        if alt {
                            s.move_cursor_word_left();
                        } else {
                            s.move_cursor_left();
                        }
                        if !shift {
                            s.composer_selection_anchor = None;
                        } else if s.composer_selection_anchor == Some(s.cursor_position) {
                            s.composer_selection_anchor = None;
                        }
                    }
                    KeyCode::Right => {
                        let mut s = app_state.lock().await;
                        if key.modifiers.is_empty()
                            && let Some(id) = agent_context_return_target(&s, key.code)
                        {
                            drop(s);
                            let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                            return Ok(InputFlow::ContinueIteration);
                        }
                        let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
                        // Word motion wins whenever there is a draft; only an
                        // empty composer gives Alt+arrows to agent switching.
                        if alt
                            && !shift
                            && s.input_buffer.is_empty()
                            && let Some(id) = sessions::adjacent_context_id(&s, true)
                        {
                            drop(s);
                            let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                            return Ok(InputFlow::ContinueIteration);
                        }
                        if shift && s.composer_selection_anchor.is_none() {
                            s.composer_selection_anchor = Some(s.cursor_position);
                        }
                        if alt {
                            s.move_cursor_word_right();
                        } else {
                            s.move_cursor_right();
                        }
                        if !shift {
                            s.composer_selection_anchor = None;
                        } else if s.composer_selection_anchor == Some(s.cursor_position) {
                            s.composer_selection_anchor = None;
                        }
                    }
                    KeyCode::Home => {
                        let mut s = app_state.lock().await;
                        let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
                        if shift && s.composer_selection_anchor.is_none() {
                            s.composer_selection_anchor = Some(s.cursor_position);
                        }
                        s.move_cursor_to_start();
                        if !shift {
                            s.composer_selection_anchor = None;
                        } else if s.composer_selection_anchor == Some(s.cursor_position) {
                            s.composer_selection_anchor = None;
                        }
                    }
                    KeyCode::End => {
                        let mut s = app_state.lock().await;
                        let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
                        if shift && s.composer_selection_anchor.is_none() {
                            s.composer_selection_anchor = Some(s.cursor_position);
                        }
                        s.move_cursor_to_end();
                        if !shift {
                            s.composer_selection_anchor = None;
                        } else if s.composer_selection_anchor == Some(s.cursor_position) {
                            s.composer_selection_anchor = None;
                        }
                    }
                    KeyCode::Char('l') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        terminal_runtime.terminal().clear()?;
                    }
                    KeyCode::Enter => {
                        let modifiers = key.modifiers;
                        if modifiers.contains(event::KeyModifiers::SHIFT)
                            || modifiers.contains(event::KeyModifiers::CONTROL)
                            || modifiers.contains(event::KeyModifiers::ALT)
                        {
                            let mut s = app_state.lock().await;
                            s.insert_char('\n');
                            s.reset_suggestion_cycle();
                        } else {
                            if open_demo_if_requested(app_state, demo_state).await {
                                return Ok(InputFlow::ContinueIteration);
                            }
                            if rustcode::app::handle_enter_with_ui_events(
                                &app_state,
                                &client,
                                current_cancel_token,
                                agent_ui_event_sender.clone(),
                                &|| {
                                    crate::ui::theme::load_available_themes()
                                        .into_iter()
                                        .map(|t| t.name)
                                        .collect()
                                },
                            )
                            .await
                            {
                                return Ok(InputFlow::Exit { update: false });
                            }
                        }
                    }
                    KeyCode::Char('v') | KeyCode::Char('V')
                        if key.modifiers.contains(event::KeyModifiers::CONTROL)
                            || key.modifiers.contains(event::KeyModifiers::SUPER)
                            || key.modifiers.contains(event::KeyModifiers::META) =>
                    {
                        // Terminals without bracketed paste deliver Ctrl/Cmd+V
                        // as a key. Route it through the same helper the keymap
                        // paste uses so the large-paste marker cannot drift
                        // between the two (#1527).
                        if let Some(payload) = clipboard_paste_payload() {
                            let mut s = app_state.lock().await;
                            insert_clipboard_paste(composer, &mut s, &payload);
                        }
                    }
                    KeyCode::Char('p') | KeyCode::Char('n')
                        if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                    {
                        let mut s = app_state.lock().await;
                        let completion_len =
                            rustcode::app::get_completion_len(&s.input_buffer, s.cursor_position);
                        if s.active_suggestion_index.is_some() && completion_len > 0 {
                            let current = s.active_suggestion_index.unwrap_or(0);
                            s.active_suggestion_index = Some(if key.code == KeyCode::Char('p') {
                                if current == 0 {
                                    completion_len - 1
                                } else {
                                    current - 1
                                }
                            } else if current + 1 >= completion_len {
                                0
                            } else {
                                current + 1
                            });
                        } else if key.code == KeyCode::Char('p') {
                            s.show_command_picker = true;
                            s.command_picker_index = 0;
                            s.command_picker_search.clear();
                        }
                    }

                    KeyCode::Char(c) => {
                        let mut s = app_state.lock().await;
                        let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
                        let cmd = key.modifiers.contains(event::KeyModifiers::SUPER);

                        if c == '\x7f' || c == '\x08' || c == '\x17' {
                            // Option+Backspace, Ctrl+W, or raw DEL on Mac
                            if alt || cmd || c == '\x17' {
                                s.delete_word_backspace();
                            } else {
                                s.delete_char_backspace();
                            }
                            s.reset_suggestion_cycle();
                        } else if cmd {
                            if c == 'u' {
                                s.kill_line_to_start();
                                s.reset_suggestion_cycle();
                            }
                        } else if (alt && c == 'b') || c == '∫' {
                            // Terminals without enhanced key reporting send
                            // Option+Left/Right as these word-motion keys.
                            if s.input_buffer.is_empty()
                                && let Some(id) = sessions::adjacent_context_id(&s, false)
                            {
                                drop(s);
                                let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                                return Ok(InputFlow::ContinueIteration);
                            }
                            s.move_cursor_word_left();
                        } else if (alt && c == 'f') || c == 'ƒ' {
                            if s.input_buffer.is_empty()
                                && let Some(id) = sessions::adjacent_context_id(&s, true)
                            {
                                drop(s);
                                let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                                return Ok(InputFlow::ContinueIteration);
                            }
                            s.move_cursor_word_right();
                        } else if (alt && c == 'd') || c == '∂' {
                            s.delete_word_forward();
                            s.reset_suggestion_cycle();
                        } else if ctrl && c == 'j' {
                            s.insert_char('\n');
                            s.reset_suggestion_cycle();
                        } else if ctrl && c == 'a' {
                            s.move_cursor_to_start();
                        } else if ctrl && c == 'e' {
                            s.move_cursor_to_end();
                        } else if ctrl && c == 'u' {
                            s.kill_line_to_start();
                            s.reset_suggestion_cycle();
                        } else if ctrl && c == 'w' {
                            s.delete_word_backspace();
                            s.reset_suggestion_cycle();
                        } else if c == '?' && !ctrl && !alt && !cmd && s.input_buffer.is_empty() {
                            s.history
                                .push(ChatMessage::new("system", rustcode::app::build_help_text()));
                            s.request_redraw();
                        } else if !ctrl && !alt && !c.is_control() {
                            s.insert_char(c);
                            s.reset_suggestion_cycle();
                        }
                    }
                    KeyCode::Backspace => {
                        let mut s = app_state.lock().await;
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
                        let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                        let cmd = key.modifiers.contains(event::KeyModifiers::SUPER);
                        if cmd {
                            s.kill_line_to_start();
                        } else if alt || ctrl {
                            s.delete_word_backspace();
                        } else {
                            s.delete_char_backspace();
                        }
                        s.reset_suggestion_cycle();
                    }
                    KeyCode::Delete => {
                        let mut s = app_state.lock().await;
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
                        let cmd = key.modifiers.contains(event::KeyModifiers::SUPER);
                        if cmd {
                            s.kill_line_to_start();
                        } else if alt {
                            s.delete_word_forward();
                        } else {
                            s.delete_char_delete();
                        }
                        s.reset_suggestion_cycle();
                    }
                    _ => {}
                }
            }
            TuiEvent::Mouse(mouse) => {
                match mouse.kind {
                    // The return-to-bottom control owns its rectangle only while
                    // it is painted, so a hidden control can never swallow this
                    // click (#1595).
                    event::MouseEventKind::Down(event::MouseButton::Left)
                        if transcript_state
                            .follow_control()
                            .area()
                            .is_some_and(|area| {
                                area.contains(ratatui::layout::Position::new(
                                    mouse.column,
                                    mouse.row,
                                ))
                            }) =>
                    {
                        transcript_state.jump_to_latest();
                        frame_requester.schedule_frame();
                    }
                    event::MouseEventKind::ScrollUp
                        if transcript_state.panel_selection_area.is_some() =>
                    {
                        let mut state = app_state.lock().await;
                        scroll_panel_selection(transcript_state, &mut state.modal_scroll_row, -1);
                        frame_requester.schedule_frame();
                    }
                    event::MouseEventKind::ScrollDown
                        if transcript_state.panel_selection_area.is_some() =>
                    {
                        let mut state = app_state.lock().await;
                        scroll_panel_selection(transcript_state, &mut state.modal_scroll_row, 1);
                        frame_requester.schedule_frame();
                    }
                    event::MouseEventKind::ScrollUp if transcript_state.selection.is_active() => {
                        transcript_state
                            .selection
                            .queue_scroll(-1, ui::WHEEL_SCROLL_LINES);
                    }
                    event::MouseEventKind::ScrollDown if transcript_state.selection.is_active() => {
                        transcript_state
                            .selection
                            .queue_scroll(1, ui::WHEEL_SCROLL_LINES);
                    }
                    event::MouseEventKind::ScrollUp => {
                        transcript_state.scroll_up(ui::WHEEL_SCROLL_LINES);
                    }
                    event::MouseEventKind::ScrollDown => {
                        transcript_state.scroll_down(ui::WHEEL_SCROLL_LINES);
                    }
                    _ => {
                        // Composer drag selection (#1493). Down starts a
                        // selection, Drag extends it, Up keeps the highlight.
                        // While composer-selecting, events never reach the
                        // transcript path.
                        if matches!(
                            mouse.kind,
                            event::MouseEventKind::Down(event::MouseButton::Left)
                                | event::MouseEventKind::Drag(event::MouseButton::Left)
                                | event::MouseEventKind::Up(event::MouseButton::Left)
                        ) {
                            let mut state = app_state.lock().await;
                            let in_composer = !state.modal_open()
                                && state.status != AppStatus::AwaitingQuestion
                                && state.status != AppStatus::AwaitingToolConfirmation
                                && state.input_text_area.is_some();
                            if in_composer {
                                let area = state.input_text_area.expect("checked");
                                let rect = ratatui::layout::Rect::new(
                                    area.x,
                                    area.y,
                                    area.width,
                                    area.height,
                                );
                                let cursor_opt = ui::composer_cursor_from_mouse(
                                    &state.input_buffer,
                                    state.cursor_position,
                                    state.get_command_suggestion().as_deref(),
                                    rect,
                                    mouse.column,
                                    mouse.row,
                                );
                                // Clamp drags outside the composer to its
                                // bounds so selections extend without
                                // scrolling the transcript.
                                let clamped = cursor_opt.or_else(|| {
                                    if mouse.row < area.y {
                                        Some(0)
                                    } else if mouse.row >= area.y.saturating_add(area.height) {
                                        Some(state.input_buffer.len())
                                    } else {
                                        None
                                    }
                                });
                                match mouse.kind {
                                    event::MouseEventKind::Down(event::MouseButton::Left)
                                        if mouse.modifiers.is_empty() =>
                                    {
                                        if let Some(cursor) = cursor_opt {
                                            state.cursor_position = cursor;
                                            state.composer_selection_anchor = Some(cursor);
                                            state.composer_selecting = true;
                                            state.reset_suggestion_cycle();
                                            state.request_redraw();
                                            transcript_state.selection.clear();
                                            frame_requester.schedule_frame();
                                            return Ok(InputFlow::ContinueIteration);
                                        }
                                    }
                                    event::MouseEventKind::Drag(event::MouseButton::Left)
                                        if state.composer_selecting =>
                                    {
                                        if let Some(cursor) = clamped {
                                            state.cursor_position = cursor;
                                            state.request_redraw();
                                            frame_requester.schedule_frame();
                                            return Ok(InputFlow::ContinueIteration);
                                        }
                                        frame_requester.schedule_frame();
                                        return Ok(InputFlow::ContinueIteration);
                                    }
                                    event::MouseEventKind::Up(event::MouseButton::Left)
                                        if state.composer_selecting =>
                                    {
                                        if let Some(cursor) = clamped {
                                            state.cursor_position = cursor;
                                        }
                                        state.composer_selecting = false;
                                        // Click without drag clears; drag
                                        // keeps the highlight for explicit copy.
                                        if state.composer_selection_anchor
                                            == Some(state.cursor_position)
                                        {
                                            state.composer_selection_anchor = None;
                                        }
                                        state.request_redraw();
                                        frame_requester.schedule_frame();
                                        return Ok(InputFlow::ContinueIteration);
                                    }
                                    _ => {}
                                }
                            }
                            // A composer selection is dismissed by clicking
                            // elsewhere, matching normal editor behavior.
                            if mouse.kind == event::MouseEventKind::Down(event::MouseButton::Left)
                                && mouse.modifiers.is_empty()
                                && state.has_composer_selection()
                            {
                                state.clear_composer_selection();
                            }
                        }
                        if transcript_state.panel_selection_area.is_some() {
                            let selected = transcript_state.panel_selection.mouse(mouse);
                            if let Some(text) = selected {
                                report_selection_copy(
                                    app_state,
                                    &text,
                                    rustcode::clipboard::copy_to_clipboard,
                                )
                                .await;
                            }
                            frame_requester.schedule_frame();
                            return Ok(InputFlow::ContinueIteration);
                        }
                        let selected = if mouse.kind
                            == event::MouseEventKind::Down(event::MouseButton::Left)
                            && !(mouse.modifiers.contains(KeyModifiers::SHIFT)
                                && transcript_state.selection.has_selection())
                        {
                            let snapshot = {
                                let state = app_state.lock().await;
                                ui::render_snapshot::render_snapshot(
                                    &rustcode::controller::render_state(&state),
                                )
                            };
                            let scroll_rows = transcript_state.scroll_rows();
                            transcript_state.selection.begin_with_snapshot(
                                mouse,
                                snapshot,
                                scroll_rows,
                            );
                            None
                        } else {
                            transcript_state.selection.mouse(mouse)
                        };
                        // `mouse()` returns `Some` only for the explicit
                        // right-click copy action; left-button release keeps
                        // the highlight and copies via Ctrl+C (#1492, #1566).
                        if let Some(text) = selected {
                            report_selection_copy(
                                app_state,
                                &text,
                                rustcode::clipboard::copy_to_clipboard,
                            )
                            .await;
                        }
                        frame_requester.schedule_frame();
                        return Ok(InputFlow::ContinueIteration);
                    }
                }
                // Accumulate queued wheel steps before painting the next frame.
                frame_requester.schedule_frame();
                return Ok(InputFlow::ContinueIteration);
            }
            TuiEvent::FocusGained => {
                *terminal_focused = true;
                *needs_redraw = true;
            }
            TuiEvent::FocusLost => {
                *terminal_focused = false;
                *needs_redraw = true;
            }
            TuiEvent::Paste(text) => {
                transcript_state.selection.clear();
                app_state.lock().await.mark_user_activity();
                // Terminals with bracketed paste enabled deliver Cmd+V through
                // this event instead of the Char('v') key handler. When the
                // clipboard holds an image (e.g. a screenshot), the pasted text
                // is empty — fall back to grabbing the image so it still turns
                // into an `![image](file://…)` marker that renders as [Image #N].
                if text.trim().is_empty()
                    && let Some(img_markdown) = rustcode::clipboard::paste_image_from_clipboard()
                {
                    let mut s = app_state.lock().await;
                    if !s.show_mcp_config && s.status != AppStatus::AwaitingQuestion {
                        composer.handle_paste(&mut s, &img_markdown);
                    }
                    *needs_redraw = true;
                    return Ok(InputFlow::ContinueIteration);
                }
                let mut s = app_state.lock().await;
                if s.show_model_picker {
                    insert_picker_search_text(&mut s, PickerSearchTarget::Model, &text);
                    *needs_redraw = true;
                    return Ok(InputFlow::ContinueIteration);
                }
                if s.show_command_picker {
                    insert_picker_search_text(&mut s, PickerSearchTarget::Command, &text);
                    *needs_redraw = true;
                    return Ok(InputFlow::ContinueIteration);
                }
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                // Route the paste into whichever text field is focused: the
                // ask_question custom-answer slot, the MCP editor, else chat.
                if s.status == AppStatus::AwaitingQuestion && !s.user_overlay_open() {
                    if let Some(q) = s.pending_question.as_mut() {
                        if q.custom_input.is_some() {
                            insert_question_answer_text(q, &normalized);
                        }
                    }
                } else if s.show_mcp_config {
                    if let Some(ref mut edit_state) = s.mcp_edit_state {
                        insert_mcp_edit_paste(edit_state, &normalized);
                    }
                } else {
                    composer.handle_paste(&mut s, &normalized);
                }
                *needs_redraw = true;
            }
            TuiEvent::Resize { .. } => {
                *needs_redraw = true;
            }
            TuiEvent::Draw => *needs_redraw = true,
        },
        _ => {}
    }
    Ok(InputFlow::ContinueLoop)
}

#[cfg(test)]
mod tests {
    use super::{
        InputFlow, PickerSearchTarget, QuestionEditAction, SubagentPickerAction,
        agent_context_return_target, clear_selection_for_composer_key,
        filtered_command_picker_items, handle_cmd_copy_chord, handle_cmd_copy_chord_with,
        handle_copy_or_exit_chord, handle_picker_search_key, handle_question_custom_key,
        insert_clipboard_paste, insert_mcp_edit_paste, insert_question_answer_text,
        is_cmd_copy_chord, is_copy_or_exit_chord, is_keyboard_range_key, is_shift_tab,
        is_transcript_navigation, open_demo_if_requested, picker_selection_for_key,
        report_selection_copy, return_to_latest_for_key, scroll_panel_selection,
        selection_owns_key, subagent_picker_action,
    };
    use crate::ui::{Composer, TranscriptState};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{buffer::Buffer, layout::Rect};
    use rustcode::app::AppState;
    use rustcode::clipboard::ClipboardCopyStatus;
    use rustcode::controller::PendingQuestion;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn ask_question_editor_matches_composer_modified_enter_and_word_navigation() {
        let mut question = PendingQuestion::new("Question?".to_owned(), vec![], false);
        question.activate_custom_input();
        question.insert_str("café tools");

        assert_eq!(
            handle_question_custom_key(
                &mut question,
                KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)
            ),
            QuestionEditAction::Handled
        );
        assert_eq!(question.custom_cursor, "café ".len());
        handle_question_custom_key(
            &mut question,
            KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
        );
        assert_eq!(question.custom_input.as_deref(), Some("café Xtools"));

        handle_question_custom_key(
            &mut question,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        );
        handle_question_custom_key(
            &mut question,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        );
        assert_eq!(question.custom_input.as_deref(), Some("café X\n\ntools"));
        assert_eq!(
            handle_question_custom_key(
                &mut question,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            QuestionEditAction::Submit
        );
    }

    #[test]
    fn ask_question_editor_supports_forward_word_delete_and_line_kill() {
        let mut question = PendingQuestion::new("Question?".to_owned(), vec![], false);
        question.activate_custom_input();
        question.insert_str("one two three");
        question.custom_cursor = "one ".len();

        handle_question_custom_key(
            &mut question,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT),
        );
        assert_eq!(question.custom_input.as_deref(), Some("one three"));
        assert_eq!(question.custom_cursor, "one ".len());

        handle_question_custom_key(
            &mut question,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER),
        );
        assert_eq!(question.custom_input.as_deref(), Some("three"));
        assert_eq!(question.custom_cursor, 0);
    }

    #[test]
    fn ask_question_paste_preserves_normalized_newlines_and_unicode() {
        let mut question = PendingQuestion::new("Question?".to_owned(), vec![], false);
        question.activate_custom_input();
        insert_question_answer_text(&mut question, "først\r\n二つ\rslutt");

        assert_eq!(question.custom_input.as_deref(), Some("først\n二つ\nslutt"));
        assert_eq!(question.custom_cursor, "først\n二つ\nslutt".len());
    }

    #[tokio::test]
    async fn opening_demo_consumes_only_the_command_and_preserves_live_session() {
        let mut state = AppState::new();
        state.active_session_id = "live-session".to_owned();
        state
            .history
            .push(rustcode::app::ChatMessage::new("user", "existing history"));
        state.input_buffer = "/test".to_owned();
        state.cursor_position = state.input_buffer.len();
        let expected_history = state.history.snapshot();
        let app_state = Arc::new(Mutex::new(state));
        let mut demo_state = None;

        assert!(open_demo_if_requested(&app_state, &mut demo_state).await);
        let live = app_state.lock().await;
        assert_eq!(live.active_session_id, "live-session");
        assert_eq!(live.history.snapshot(), expected_history);
        assert!(live.input_buffer.is_empty());
        let demo = demo_state.expect("demo view is separate from the live session");
        assert_eq!(demo.render_state().active_session_id, "static-visual-demo");
        assert_eq!(demo.render_state().history.len(), 4);
    }

    #[tokio::test]
    async fn demo_command_does_not_match_a_prompt_with_arguments() {
        let mut state = AppState::new();
        state.input_buffer = "/test please".to_owned();
        let app_state = Arc::new(Mutex::new(state));
        let mut demo_state = None;

        assert!(!open_demo_if_requested(&app_state, &mut demo_state).await);
        assert!(demo_state.is_none());
        assert_eq!(app_state.lock().await.input_buffer, "/test please");
    }

    #[test]
    fn picker_navigation_stops_at_both_ends_and_reaches_long_list_tail() {
        let last = 127;
        let mut selected = 0;
        assert_eq!(
            picker_selection_for_key(selected, last + 1, KeyCode::Up),
            Some(0)
        );
        for _ in 0..last {
            selected = picker_selection_for_key(selected, last + 1, KeyCode::Down).unwrap();
        }
        assert_eq!(selected, last);
        assert_eq!(
            picker_selection_for_key(selected, last + 1, KeyCode::Down),
            Some(last)
        );
        assert_eq!(
            picker_selection_for_key(selected, last + 1, KeyCode::Up),
            Some(last - 1)
        );
        assert_eq!(picker_selection_for_key(0, 0, KeyCode::Down), Some(0));
    }

    #[test]
    fn subagent_picker_left_selects_and_right_returns() {
        assert_eq!(
            subagent_picker_action(KeyCode::Left),
            Some(SubagentPickerAction::Select)
        );
        assert_eq!(
            subagent_picker_action(KeyCode::Right),
            Some(SubagentPickerAction::Close)
        );
        assert_eq!(
            subagent_picker_action(KeyCode::Enter),
            Some(SubagentPickerAction::Select)
        );
    }

    #[test]
    fn right_from_an_empty_child_composer_returns_to_its_parent() {
        let mut state = AppState::new();
        let child = rustcode::app::SubagentController
            .spawn(
                &mut state,
                "child",
                None,
                None,
                false,
                Vec::new(),
                None,
                None,
            )
            .raw();
        state.selected_subagent_id = Some(child);
        assert_eq!(agent_context_return_target(&state, KeyCode::Right), Some(0));

        state.input_buffer = "draft".to_owned();
        assert_eq!(agent_context_return_target(&state, KeyCode::Right), None);
    }

    #[test]
    fn command_picker_filter_includes_visible_fuzzy_group_matches() {
        let expected = crate::ui::PALETTE_ITEMS
            .iter()
            .find(|item| item.group == "System")
            .expect("palette has a System group");
        assert!(
            filtered_command_picker_items("system")
                .iter()
                .any(|item| item.name == expected.name)
        );
    }

    #[test]
    fn picker_search_supports_composer_cursor_editing_without_splitting_unicode() {
        let mut state = AppState::new();
        state.command_picker_search = "café tools".to_owned();
        state.command_picker_search_cursor = state.command_picker_search.len();
        state.command_picker_index = 7;
        let key = |code, modifiers| KeyEvent::new(code, modifiers);

        assert!(handle_picker_search_key(
            &mut state,
            PickerSearchTarget::Command,
            key(KeyCode::Left, KeyModifiers::NONE)
        ));
        assert_eq!(
            state.command_picker_index, 7,
            "cursor motion keeps selection"
        );
        assert!(handle_picker_search_key(
            &mut state,
            PickerSearchTarget::Command,
            key(KeyCode::Left, KeyModifiers::ALT)
        ));
        assert_eq!(
            &state.command_picker_search[..state.command_picker_search_cursor],
            "café "
        );
        assert!(handle_picker_search_key(
            &mut state,
            PickerSearchTarget::Command,
            key(KeyCode::Char('X'), KeyModifiers::NONE)
        ));
        assert_eq!(state.command_picker_search, "café Xtools");
        assert_eq!(
            state.command_picker_index, 0,
            "query changes reset selection"
        );
        assert!(handle_picker_search_key(
            &mut state,
            PickerSearchTarget::Command,
            key(KeyCode::Left, KeyModifiers::NONE)
        ));
        assert!(handle_picker_search_key(
            &mut state,
            PickerSearchTarget::Command,
            key(KeyCode::Delete, KeyModifiers::ALT)
        ));
        assert_eq!(state.command_picker_search, "café ");
        assert_eq!(state.command_picker_search_cursor, "café ".len());
        assert!(handle_picker_search_key(
            &mut state,
            PickerSearchTarget::Command,
            key(KeyCode::Home, KeyModifiers::NONE)
        ));
        assert!(handle_picker_search_key(
            &mut state,
            PickerSearchTarget::Command,
            key(KeyCode::Char('a'), KeyModifiers::CONTROL)
        ));
        assert_eq!(state.command_picker_search_cursor, 0);
        assert!(!handle_picker_search_key(
            &mut state,
            PickerSearchTarget::Command,
            key(KeyCode::Up, KeyModifiers::NONE)
        ));
    }

    #[test]
    fn mcp_editor_paste_uses_active_utf8_cursor_for_key_and_bracketed_paste() {
        use crate::ui::keymap::{KeyAction, KeyMap};

        let keymap = KeyMap::from_environment();
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::SUPER,
            KeyModifiers::META,
        ] {
            assert_eq!(
                keymap.resolve(KeyEvent::new(KeyCode::Char('v'), modifiers)),
                KeyAction::Paste
            );
        }

        let mut edit_state = rustcode::app::McpEditState {
            is_add: true,
            edit_index: None,
            name_input: "café".to_owned(),
            command_input: String::new(),
            args_input: String::new(),
            active_field: 0,
            cursor_pos: "caf".len(),
        };
        insert_mcp_edit_paste(&mut edit_state, "X\r\nY");
        assert_eq!(edit_state.name_input, "cafXYé");
        assert_eq!(edit_state.cursor_pos, "cafXY".len());
    }

    #[tokio::test]
    async fn selection_copy_feedback_reports_backend_result_without_chat_message() {
        let state = Arc::new(Mutex::new(AppState::new()));
        for (result, expected) in [
            (
                ClipboardCopyStatus::Confirmed,
                "Copied selection to clipboard",
            ),
            (
                ClipboardCopyStatus::Requested,
                "Copy sent to terminal; paste to verify",
            ),
            (ClipboardCopyStatus::Failed, "Copy failed; try again"),
        ] {
            report_selection_copy(&state, "selected text", |text| {
                assert_eq!(text, "selected text");
                result
            })
            .await;
            let state = state.lock().await;
            assert_eq!(state.active_transient_notice(), Some(expected));
            assert!(state.history.is_empty());
        }
    }

    #[test]
    fn shift_tab_is_normalized_from_supported_terminal_events() {
        assert!(is_shift_tab(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::NONE
        )));
        assert!(is_shift_tab(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::SHIFT
        )));
        assert!(!is_shift_tab(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn keyboard_range_only_owns_unmodified_or_shifted_arrows() {
        assert!(is_keyboard_range_key(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::NONE
        )));
        assert!(is_keyboard_range_key(KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::SHIFT
        )));
        assert!(!is_keyboard_range_key(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::ALT
        )));
        assert!(!is_keyboard_range_key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn ordinary_composer_key_exits_keyboard_range_mode() {
        let mut transcript = TranscriptState::default();
        let area = Rect::new(0, 0, 8, 1);
        transcript
            .selection
            .refresh(area, &Buffer::empty(area), &[false]);
        transcript.selection.begin_keyboard_with_snapshot(
            crate::ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
                &AppState::new(),
            )),
            0,
        );
        assert!(transcript.selection.is_keyboard_mode());
        assert!(selection_owns_key(
            &transcript,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        ));
        assert!(!selection_owns_key(
            &transcript,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
        // Ctrl+C stays out of the selection's hands so it can arm the exit;
        // Cmd+C stays out too, but takes the copy-only path.
        assert!(is_copy_or_exit_chord(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(is_cmd_copy_chord(KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::SUPER
        )));
        assert!(!selection_owns_key(
            &transcript,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)
        ));
        clear_selection_for_composer_key(
            &mut transcript,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert!(!transcript.selection.is_keyboard_mode());
        assert!(!transcript.selection.is_active());
    }

    /// The advertised copy binding reaches the selection handler for both
    /// mouse and keyboard selections, and `ctrl+c` without a selection falls
    /// through to interrupt/exit (#1566).
    #[test]
    fn ctrl_c_copies_every_selection_and_still_reaches_the_exit_arm() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        // The hint and the handler share this definition: the footer promises
        // exactly the chord the handler owns.
        assert_eq!(rustcode::controller::copy_selection_binding(), "ctrl+c");
        let copy = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(is_copy_or_exit_chord(copy));
        // Cmd+C is handled as copy-only; Control takes precedence when both
        // modifiers arrive in the same event.
        let cmd_copy = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER);
        assert!(!is_copy_or_exit_chord(cmd_copy));
        assert!(is_cmd_copy_chord(cmd_copy));
        let mixed_copy = KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::SUPER | KeyModifiers::CONTROL,
        );
        assert!(is_copy_or_exit_chord(mixed_copy));
        assert!(!is_cmd_copy_chord(mixed_copy));
        assert!(!is_copy_or_exit_chord(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        )));

        // A live selection does not take the chord away from the exit path, so
        // copying and arming happen together.
        let area = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello", ratatui::style::Style::default());

        // Mouse selection.
        let mut mouse = TranscriptState::default();
        mouse.selection.refresh(area, &buffer, &[false]);
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        mouse.selection.mouse(down);
        mouse.selection.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        mouse.selection.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(mouse.selection.has_selection());
        assert!(!mouse.selection.is_keyboard_mode());
        assert_eq!(mouse.selection.selected_text().as_deref(), Some("hell"));

        // Keyboard selection offers the same copyable text.
        let mut keyboard = TranscriptState::default();
        keyboard.selection.refresh(area, &buffer, &[false]);
        keyboard.selection.begin_keyboard_with_snapshot(
            crate::ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
                &AppState::new(),
            )),
            0,
        );
        keyboard.selection.move_keyboard(KeyCode::Right);
        assert!(keyboard.selection.has_selection());

        // Esc is the only key the selection owns outright; the chord is handled
        // before it so the double-press exit stays reachable.
        assert!(!selection_owns_key(&mouse, copy));
        assert!(selection_owns_key(
            &mouse,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        ));

        // No selection: the key is not owned, so the caller falls through to
        // the existing interruption/exit behavior.
        let idle = TranscriptState::default();
        assert!(!idle.selection.has_selection());
        assert!(!selection_owns_key(
            &idle,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        ));
    }

    /// Two presses of the chord must exit even while a selection is live.
    ///
    /// The selection handlers used to sit above the exit check and returned
    /// early, so any selection made the double-press unreachable — and a
    /// pinned selection survives a whole turn.
    #[tokio::test]
    async fn two_ctrl_c_presses_exit_while_a_selection_is_live() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        let area = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello", ratatui::style::Style::default());

        let mut transcript = TranscriptState::default();
        transcript.selection.refresh(area, &buffer, &[false]);
        for event in [
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 3,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 3,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
        ] {
            transcript.selection.mouse(event);
        }
        assert!(transcript.selection.has_selection());

        let app_state = Arc::new(Mutex::new(AppState::new()));

        // First press: copies and arms, without exiting.
        assert!(matches!(
            handle_copy_or_exit_chord(&app_state, &transcript).await,
            InputFlow::ContinueIteration
        ));
        assert!(app_state.lock().await.ctrl_c_exit_armed());

        // Second press inside the window exits.
        assert!(matches!(
            handle_copy_or_exit_chord(&app_state, &transcript).await,
            InputFlow::Exit { update: false }
        ));
    }

    #[tokio::test]
    async fn repeated_cmd_c_copies_selection_without_arming_exit() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        let area = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello", ratatui::style::Style::default());
        let mut transcript = TranscriptState::default();
        // `/session` displays a fixed session panel whose selection is stored
        // separately from the main transcript selection.
        transcript.panel_selection_area = Some(area);
        transcript.panel_selection_scrollable = false;
        transcript.panel_selection.refresh(area, &buffer, &[false]);
        for event in [
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 3,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 3,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
        ] {
            transcript.panel_selection.mouse(event);
        }
        assert_eq!(
            transcript.panel_selection.selected_text().as_deref(),
            Some("hell")
        );

        let app_state = Arc::new(Mutex::new(AppState::new()));
        for _ in 0..2 {
            let copied = std::sync::Arc::new(std::sync::Mutex::new(None));
            let copied_by_backend = copied.clone();
            assert!(matches!(
                handle_cmd_copy_chord_with(&app_state, &transcript, move |text| {
                    *copied_by_backend.lock().unwrap() = Some(text.to_owned());
                    rustcode::clipboard::ClipboardCopyStatus::Confirmed
                })
                .await,
                InputFlow::ContinueIteration
            ));
            assert_eq!(copied.lock().unwrap().as_deref(), Some("hell"));
            assert!(!app_state.lock().await.ctrl_c_exit_armed());
        }
    }

    #[tokio::test]
    async fn cmd_c_copies_a_composer_selection_without_hanging() {
        let app_state = Arc::new(Mutex::new(AppState::new()));
        {
            let mut state = app_state.lock().await;
            state.input_buffer = "see ![image](file:///tmp/a.png) here".to_owned();
            state.composer_selection_anchor = Some(0);
            state.cursor_position = 3;
        }
        let copied = std::sync::Arc::new(std::sync::Mutex::new(None));
        let copied_by_backend = copied.clone();
        let flow = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle_cmd_copy_chord_with(&app_state, &TranscriptState::default(), move |text| {
                *copied_by_backend.lock().unwrap() = Some(text.to_owned());
                rustcode::clipboard::ClipboardCopyStatus::Confirmed
            }),
        )
        .await
        .expect("copying a composer selection must not deadlock on the state lock");
        assert!(matches!(flow, InputFlow::ContinueIteration));
        assert_eq!(copied.lock().unwrap().as_deref(), Some("see"));
    }

    #[tokio::test]
    async fn cmd_c_without_app_selection_never_arms_exit() {
        let app_state = Arc::new(Mutex::new(AppState::new()));
        let transcript = TranscriptState::default();
        // Cmd+C also breaks a pending Ctrl+C confirmation. A later Ctrl+C
        // starts a fresh sequence instead of exiting unexpectedly.
        assert!(!rustcode::app::handle_ctrl_c(&app_state).await);
        assert!(app_state.lock().await.ctrl_c_exit_armed());
        for _ in 0..2 {
            assert!(matches!(
                handle_cmd_copy_chord(&app_state, &transcript).await,
                InputFlow::ContinueIteration
            ));
            assert!(!app_state.lock().await.ctrl_c_exit_armed());
        }
        assert!(!rustcode::app::handle_ctrl_c(&app_state).await);
        assert!(app_state.lock().await.ctrl_c_exit_armed());
    }

    /// With no selection the chord is purely the exit affordance.
    #[tokio::test]
    async fn two_ctrl_c_presses_exit_with_no_selection() {
        let transcript = TranscriptState::default();
        let app_state = Arc::new(Mutex::new(AppState::new()));

        assert!(matches!(
            handle_copy_or_exit_chord(&app_state, &transcript).await,
            InputFlow::ContinueIteration
        ));
        assert!(app_state.lock().await.ctrl_c_exit_armed());
        assert!(matches!(
            handle_copy_or_exit_chord(&app_state, &transcript).await,
            InputFlow::Exit { update: false }
        ));
    }

    /// Keyboard-select mode owns the chord with a collapsed caret: nothing to
    /// copy, but it must still report progress and arm the exit.
    #[tokio::test]
    async fn collapsed_keyboard_selection_reports_instead_of_failing_silently() {
        let mut transcript = TranscriptState::default();
        let area = Rect::new(0, 0, 8, 1);
        transcript
            .selection
            .refresh(area, &Buffer::empty(area), &[false]);
        transcript.selection.begin_keyboard_with_snapshot(
            crate::ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
                &AppState::new(),
            )),
            0,
        );
        assert!(transcript.selection.is_keyboard_mode());
        assert_eq!(transcript.selection.selected_text(), None);

        let app_state = Arc::new(Mutex::new(AppState::new()));
        assert!(matches!(
            handle_copy_or_exit_chord(&app_state, &transcript).await,
            InputFlow::ContinueIteration
        ));
        assert!(app_state.lock().await.ctrl_c_exit_armed());
    }

    #[test]
    fn informational_panel_selection_keeps_its_range_across_wheel_scroll() {
        let area = Rect::new(0, 0, 12, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "panel text", ratatui::style::Style::default());

        let mut transcript = TranscriptState::default();
        transcript.selection.refresh(area, &buffer, &[false]);
        transcript.selection.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        transcript.selection.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        transcript.selection.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let transcript_text = transcript.selection.selected_text();

        transcript.panel_selection_area = Some(area);
        transcript.panel_selection.refresh(area, &buffer, &[false]);
        transcript.panel_selection.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 6,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        transcript.panel_selection.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 9,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        transcript.panel_selection.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 9,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(
            transcript.panel_selection.selected_text().as_deref(),
            Some("text")
        );
        assert_eq!(transcript.selection.selected_text(), transcript_text);
        let mut modal_scroll_row = 0;
        assert!(scroll_panel_selection(
            &mut transcript,
            &mut modal_scroll_row,
            1,
        ));
        assert_eq!(modal_scroll_row, 0);
        assert_eq!(
            transcript.panel_selection.selected_text().as_deref(),
            Some("text"),
            "wheel input over a fixed info panel preserves the copyable range"
        );
        clear_selection_for_composer_key(
            &mut transcript,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert!(!transcript.panel_selection.has_selection());
        assert_eq!(transcript.selection.selected_text(), transcript_text);
    }

    #[test]
    fn composer_keys_keep_reading_position_and_escape_returns_to_latest() {
        let mut transcript = TranscriptState::default();
        transcript.scroll_up(3);
        assert!(!return_to_latest_for_key(
            &mut transcript,
            KeyCode::Char('x')
        ));
        assert_eq!(transcript.scroll_rows(), 3);

        transcript.scroll_up(2);
        assert!(return_to_latest_for_key(&mut transcript, KeyCode::Esc));
        assert_eq!(transcript.scroll_rows(), 0);
    }

    #[test]
    fn typing_and_submission_end_pinned_selection_while_transcript_navigation_keeps_it() {
        let mut transcript = TranscriptState::default();
        let area = Rect::new(0, 0, 8, 2);
        let buffer = Buffer::empty(area);
        transcript.selection.refresh(area, &buffer, &[false, false]);
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        let begin = |transcript: &mut TranscriptState| {
            transcript.selection.begin_with_snapshot(
                down,
                crate::ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(
                    &AppState::new(),
                )),
                transcript.scroll_rows(),
            );
            assert!(transcript.selection.is_active());
        };
        begin(&mut transcript);
        assert!(is_transcript_navigation(KeyEvent::new(
            KeyCode::PageUp,
            KeyModifiers::NONE
        )));
        clear_selection_for_composer_key(
            &mut transcript,
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
        );
        assert!(transcript.selection.is_active());
        clear_selection_for_composer_key(
            &mut transcript,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert!(!transcript.selection.is_active());
        begin(&mut transcript);
        clear_selection_for_composer_key(
            &mut transcript,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(!transcript.selection.is_active());
    }

    /// The Ctrl/Cmd+V fallback and the keymap paste share one insert helper,
    /// so both land the same buffer for the same clipboard text at and just
    /// below the large-paste threshold (#1527).
    #[test]
    fn clipboard_paste_paths_agree_on_threshold_and_newlines() {
        let composer = Composer::default();
        for payload in [
            "y".repeat(299),
            "z".repeat(300),
            "one\r\ntwo\rthree".to_owned(),
        ] {
            let mut fallback = AppState::new();
            insert_clipboard_paste(&composer, &mut fallback, &payload);

            let mut keymap = AppState::new();
            composer.handle_paste(&mut keymap, &payload);

            assert_eq!(
                fallback.input_buffer, keymap.input_buffer,
                "fallback and keymap paste must insert the same buffer for {payload:?}"
            );
        }
    }

    #[test]
    fn clipboard_paste_below_threshold_inserts_verbatim() {
        let mut state = AppState::new();
        insert_clipboard_paste(&Composer::default(), &mut state, &"y".repeat(299));
        assert_eq!(state.input_buffer, "y".repeat(299));

        let mut large = AppState::new();
        insert_clipboard_paste(&Composer::default(), &mut large, &"y".repeat(300));
        assert!(large.input_buffer.starts_with("<!--PASTE:300:"));
        assert!(large.input_buffer.ends_with("-->"));
    }
}
