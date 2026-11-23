mod keyboard;

use super::*;
use crate::ui::keymap::{KeyAction, KeyMap};
use crossterm::event::KeyEvent;
use rustcode::controller::text_edit::TextEdit;

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

fn handle_question_custom_key(
    question: &mut rustcode::controller::PendingQuestion,
    key: KeyEvent,
) -> QuestionEditAction {
    use QuestionEditAction::{Handled, Paste, Submit};

    let action = KeyMap::from_environment().resolve(key);
    match action {
        KeyAction::InsertNewline => question.insert_char('\n'),
        KeyAction::Submit => return Submit,
        KeyAction::Paste => return Paste,
        // The answer may span lines: Up and Down move through them, and Up on
        // the first line steps back to the options with the text kept.
        KeyAction::HistoryPrevious => {
            if !question.custom_cursor_on_first_line() || !question.leave_custom_input() {
                question.edit(TextEdit::LineUp);
            }
        }
        KeyAction::HistoryNext => question.edit(TextEdit::LineDown),
        // Modal and composer actions do not edit an answer.
        _ => {
            if let Some(edit) = action.text_edit(&mut [0; 4]) {
                question.edit(edit);
            }
        }
    }
    Handled
}

/// One key in the `/scroll-speed` panel. The value being chosen lives in
/// `modal_picker_index`; arrows step it within range, Home/End jump to the
/// ends, digits pick a value directly. Returns the value to save on Enter;
/// Esc closes without saving.
fn scroll_speed_picker_key(state: &mut AppState, key: KeyCode) -> Option<u16> {
    let range = rustcode::controller::SCROLL_LINES_RANGE;
    let (min, max) = (usize::from(*range.start()), usize::from(*range.end()));
    let value = state.modal_picker_index.clamp(min, max);
    state.modal_picker_index = match key {
        KeyCode::Left | KeyCode::Down | KeyCode::Char('-') => value.saturating_sub(1).max(min),
        KeyCode::Right | KeyCode::Up | KeyCode::Char('+') | KeyCode::Char('=') => {
            (value + 1).min(max)
        }
        KeyCode::Home => min,
        KeyCode::End => max,
        KeyCode::Char(digit @ '1'..='9') => usize::from(digit as u8 - b'0'),
        KeyCode::Char('0') => 10,
        KeyCode::Enter => return u16::try_from(value).ok(),
        KeyCode::Esc => {
            state.close_modal_status();
            value
        }
        _ => value,
    };
    None
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

/// Keys the tasks panel owns while it is open. Everything else is swallowed
/// so typing cannot leak into the composer behind it.
fn tasks_panel_input_for_key(key: KeyCode) -> Option<rustcode::controller::TasksPanelInput> {
    use rustcode::controller::TasksPanelInput;
    match key {
        KeyCode::Up | KeyCode::Char('k') => Some(TasksPanelInput::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(TasksPanelInput::Down),
        KeyCode::Enter | KeyCode::Right => Some(TasksPanelInput::Open),
        KeyCode::Esc | KeyCode::Left => Some(TasksPanelInput::Back),
        KeyCode::Char('x') | KeyCode::Char('X') => Some(TasksPanelInput::Stop),
        KeyCode::Char('q') | KeyCode::Char('Q') => Some(TasksPanelInput::Close),
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
    // A search query is one line; the list owns Up and Down.
    let action = crate::ui::keymap::KeyMap::from_environment().resolve(key);
    match action.text_edit(&mut [0; 4]) {
        Some(edit) => {
            rustcode::controller::text_edit::apply(query, cursor, edit);
            true
        }
        None => false,
    }
}

fn insert_picker_search_text(state: &mut AppState, target: PickerSearchTarget, text: &str) {
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
    let text = rustcode::controller::text_edit::single_line(text);
    rustcode::controller::text_edit::apply(query, cursor, TextEdit::Insert(&text));
    *selected = 0;
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

fn scroll_command_panel(row: &mut u16, limit: u16, delta: i16) {
    // Clamp before moving as well: resizing or replacing content can shrink
    // the viewport limit while a previous offset is still in flight.
    *row = (*row).min(limit).saturating_add_signed(delta).min(limit);
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
        let step = ui::wheel_scroll_lines() as i16;
        scroll_command_panel(
            modal_scroll_row,
            transcript.panel_max_scroll_row,
            if direction < 0 { -step } else { step },
        );
    }
    true
}

/// The expand key (ctrl+o, or ctrl+shift+o for one entry at a time), also
/// reached by clicking a tool row in the transcript.
fn toggle_tool_expansion(state: &mut AppState, width: u16, step: bool) {
    let snapshot = ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(state));
    let candidates = ui::collapsible_tool_indices(&snapshot, width);
    if candidates.is_empty() {
        state.set_transient_notice("No collapsed tool output");
        return;
    }
    let open_by_default = open_by_default(state, &snapshot, width);
    with_visible_expansion(state, &open_by_default, |state| {
        if step {
            // The single-entry step keeps the focus-driven walk
            // that the whole-transcript toggle replaced.
            rustcode::controller::toggle_expanded_thought(state, &candidates);
        } else {
            rustcode::controller::toggle_all_expanded_thoughts(state, &candidates);
        }
    });
}

/// The edit key (ctrl+d): open every file edit, or close them all once all
/// are open, leaving the other tool output as it is.
fn toggle_edit_expansion(state: &mut AppState, width: u16) {
    let snapshot = ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(state));
    let edits = ui::collapsible_edit_indices(&snapshot, width);
    if edits.is_empty() {
        state.set_transient_notice("No file edits to expand");
        return;
    }
    let open_by_default = open_by_default(state, &snapshot, width);
    let outcome = with_visible_expansion(state, &open_by_default, |state| {
        rustcode::controller::toggle_all_expanded_thoughts(state, &edits)
    });
    if let rustcode::controller::ExpandOutcome::CollapsedAll { .. } = outcome {
        state.set_transient_notice("Collapsed file edits");
    } else {
        state.set_transient_notice("Expanded file edits");
    }
}

/// The tool bodies the current verbosity opens before any toggle: the file
/// edits at `edits` verbosity, nothing otherwise.
fn open_by_default(
    state: &AppState,
    snapshot: &ui::render_snapshot::RenderSnapshot,
    width: u16,
) -> Vec<usize> {
    if state.verbosity.edits_open() {
        ui::collapsible_edit_indices(snapshot, width)
    } else {
        Vec::new()
    }
}

/// Run a toggle over the *visible* open state. The stored set records flips
/// from the verbosity's default, so flip the defaults in, toggle, and flip
/// them back out; the toggles themselves stay unaware of verbosity.
fn with_visible_expansion<R>(
    state: &mut AppState,
    open_by_default: &[usize],
    toggle: impl FnOnce(&mut AppState) -> R,
) -> R {
    let flip = |state: &mut AppState| {
        for index in open_by_default {
            if !state.expanded_thoughts.remove(index) {
                state.expanded_thoughts.insert(*index);
            }
        }
    };
    flip(state);
    let outcome = toggle(state);
    flip(state);
    outcome
}

/// Open or close the output of one tool block: open while any of its calls is
/// closed, close once all are open. A block of several calls is a folded run
/// at low or edits verbosity, which opens to its calls instead.
fn toggle_tool_block(state: &mut AppState, width: u16, (first, end): ui::ToolBlock) {
    if end - first > 1 && state.verbosity.compact() {
        if !state.expanded_tool_runs.remove(&first) {
            state.expanded_tool_runs.insert(first);
        }
        state.request_redraw();
        return;
    }
    let snapshot = ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(state));
    let calls = ui::collapsible_tool_indices(&snapshot, width)
        .into_iter()
        .filter(|index| (first..end).contains(index))
        .collect::<Vec<_>>();
    if calls.is_empty() {
        state.set_transient_notice("No output to show");
        return;
    }
    let open_by_default = open_by_default(state, &snapshot, width);
    with_visible_expansion(state, &open_by_default, |state| {
        if calls
            .iter()
            .all(|index| state.expanded_thoughts.contains(index))
        {
            for index in &calls {
                state.expanded_thoughts.remove(index);
            }
        } else {
            state.expanded_thoughts.extend(calls);
        }
    });
    state.expanded_thought_focus = None;
    state.request_redraw();
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
    report_copy(app_state, text, "selection", copy).await;
}

/// Copy `text` and tell the user what happened to the `subject` they copied.
async fn report_copy(
    app_state: &Arc<Mutex<AppState>>,
    text: &str,
    subject: &str,
    copy: impl FnOnce(&str) -> rustcode::clipboard::ClipboardCopyStatus,
) {
    let notice = match copy(text) {
        rustcode::clipboard::ClipboardCopyStatus::Confirmed => {
            format!("Copied {subject} to clipboard")
        }
        rustcode::clipboard::ClipboardCopyStatus::Requested => {
            "Copy sent to terminal; paste to verify".to_owned()
        }
        rustcode::clipboard::ClipboardCopyStatus::Failed => "Copy failed; try again".to_owned(),
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
            remote::apply_terminal_approval(app_state, current_cancel_token, decision).await;
            *needs_redraw = true;
        }
        AppEvent::AnswerQuestion(answer) => {
            remote::apply_terminal_answer(app_state, current_cancel_token, answer).await;
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
        event
        @ (AppEvent::NewSession | AppEvent::ResumeSession(_) | AppEvent::DeleteSession(_)) => {
            let mut state = app_state.lock().await;
            if let Err(error) = apply_session_event(&mut state, current_cancel_token, event) {
                state.set_notice(error.to_string());
                state.request_redraw();
            }
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
                return keyboard::handle_key(
                    key,
                    InputContext {
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
                    },
                )
                .await;
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
                    // A panel's `esc` hint and its list rows act like the
                    // keys they stand for, so the panel needs no second path.
                    event::MouseEventKind::Down(event::MouseButton::Left)
                        if transcript_state
                            .panel_target_at(mouse.column, mouse.row)
                            .is_some() =>
                    {
                        let keys = match transcript_state
                            .panel_target_at(mouse.column, mouse.row)
                            .expect("checked")
                        {
                            ui::PanelTarget::Escape => vec![KeyCode::Esc],
                            ui::PanelTarget::ListRow(rows) => {
                                let step = if rows < 0 { KeyCode::Up } else { KeyCode::Down };
                                let mut keys = vec![step; rows.unsigned_abs()];
                                keys.push(KeyCode::Enter);
                                keys
                            }
                        };
                        for key in keys {
                            let _ = app_event_sender.send(AppEvent::Tui(TuiEvent::Key(
                                event::KeyEvent::new(key, event::KeyModifiers::NONE),
                            )));
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }
                    // Pointer motion only moves the hover; a frame is drawn
                    // when it reaches or leaves a tool block.
                    event::MouseEventKind::Moved => {
                        if transcript_state.hover_at(mouse.column, mouse.row) {
                            frame_requester.schedule_frame();
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }
                    // The footer's task counter opens the tasks panel.
                    event::MouseEventKind::Down(event::MouseButton::Left)
                        if transcript_state.tasks_chip_at(mouse.column, mouse.row) =>
                    {
                        let mut state = app_state.lock().await;
                        if !state.modal_open() {
                            rustcode::controller::show_tasks_panel(&mut state);
                        }
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    // The composer's posture token cycles ask -> auto-approve -> plan.
                    event::MouseEventKind::Down(event::MouseButton::Left)
                        if transcript_state.posture_chip_at(mouse.column, mouse.row) =>
                    {
                        let mut state = app_state.lock().await;
                        if !state.modal_open() {
                            super::composer::cycle_permission_posture(&mut state);
                        }
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    // The wheel moves the tasks panel's selection, or scrolls
                    // its log, instead of the transcript behind it.
                    event::MouseEventKind::ScrollUp | event::MouseEventKind::ScrollDown
                        if app_state.lock().await.tasks_panel.is_some() =>
                    {
                        let input = if mouse.kind == event::MouseEventKind::ScrollUp {
                            rustcode::controller::TasksPanelInput::Up
                        } else {
                            rustcode::controller::TasksPanelInput::Down
                        };
                        let mut state = app_state.lock().await;
                        rustcode::controller::tasks_panel_input(&mut state, input);
                        *needs_redraw = true;
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
                            .queue_scroll(-1, ui::wheel_scroll_lines());
                    }
                    event::MouseEventKind::ScrollDown if transcript_state.selection.is_active() => {
                        transcript_state
                            .selection
                            .queue_scroll(1, ui::wheel_scroll_lines());
                    }
                    event::MouseEventKind::ScrollUp => {
                        transcript_state.scroll_up(ui::wheel_scroll_lines());
                    }
                    event::MouseEventKind::ScrollDown => {
                        transcript_state.scroll_down(ui::wheel_scroll_lines());
                    }
                    // A right-click on a tool call copies the call and what it
                    // returned; a live selection keeps the right-click for
                    // itself.
                    event::MouseEventKind::Down(event::MouseButton::Right)
                        if !transcript_state.selection.has_selection()
                            && transcript_state.panel_selection_area.is_none()
                            && transcript_state
                                .tool_block_at(mouse.column, mouse.row)
                                .is_some() =>
                    {
                        let (call, _) = transcript_state
                            .tool_block_at(mouse.column, mouse.row)
                            .expect("checked");
                        let text = {
                            let state = app_state.lock().await;
                            let snapshot = ui::render_snapshot::render_snapshot(
                                &rustcode::controller::render_state(&state),
                            );
                            ui::tool_call_copy_text(&snapshot, call)
                        };
                        if let Some(text) = text {
                            report_copy(
                                app_state,
                                &text,
                                "tool call",
                                rustcode::clipboard::copy_to_clipboard,
                            )
                            .await;
                        }
                        frame_requester.schedule_frame();
                        return Ok(InputFlow::ContinueIteration);
                    }
                    // A right-click on a message copies the whole message; a
                    // live selection keeps the right-click for itself.
                    event::MouseEventKind::Down(event::MouseButton::Right)
                        if !transcript_state.selection.has_selection()
                            && transcript_state.panel_selection_area.is_none()
                            && transcript_state
                                .message_at(mouse.column, mouse.row)
                                .is_some() =>
                    {
                        let index = transcript_state
                            .message_at(mouse.column, mouse.row)
                            .expect("checked");
                        let text = app_state
                            .lock()
                            .await
                            .history
                            .get(index)
                            .map(|message| message.content.trim().to_owned());
                        if let Some(text) = text {
                            report_copy(
                                app_state,
                                &text,
                                "message",
                                rustcode::clipboard::copy_to_clipboard,
                            )
                            .await;
                        }
                        frame_requester.schedule_frame();
                        return Ok(InputFlow::ContinueIteration);
                    }
                    _ => {
                        // Composer drag selection (#1493). Down starts a
                        // selection, Drag extends it, Up keeps the highlight.
                        // While composer-selecting, events never reach the
                        // transcript path.
                        if matches!(
                            mouse.kind,
                            event::MouseEventKind::Down(event::MouseButton::Left)
                                | event::MouseEventKind::Down(event::MouseButton::Right)
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
                                // Same gestures as the transcript selection:
                                // double-click a word, triple-click a line,
                                // shift-click to extend, right-click to copy.
                                match mouse.kind {
                                    event::MouseEventKind::Down(event::MouseButton::Left)
                                        if mouse.modifiers.is_empty()
                                            || mouse.modifiers == event::KeyModifiers::SHIFT =>
                                    {
                                        if let Some(cursor) = cursor_opt {
                                            let mouse_state = &mut transcript_state.composer_mouse;
                                            if mouse.modifiers == event::KeyModifiers::SHIFT {
                                                mouse_state.shift_press();
                                                let anchor = state
                                                    .composer_selection_anchor
                                                    .unwrap_or(state.cursor_position);
                                                state.composer_selection_anchor = Some(anchor);
                                                state.cursor_position = cursor;
                                            } else {
                                                let (anchor, cursor) = mouse_state.press(
                                                    &state.input_buffer,
                                                    mouse.column,
                                                    mouse.row,
                                                    cursor,
                                                );
                                                state.composer_selection_anchor = Some(anchor);
                                                state.cursor_position = cursor;
                                            }
                                            state.composer_selecting = true;
                                            state.reset_suggestion_cycle();
                                            state.request_redraw();
                                            transcript_state.selection.clear();
                                            frame_requester.schedule_frame();
                                            return Ok(InputFlow::ContinueIteration);
                                        }
                                    }
                                    event::MouseEventKind::Down(event::MouseButton::Right)
                                        if cursor_opt.is_some()
                                            && state.has_composer_selection() =>
                                    {
                                        let text = state.composer_selected_text();
                                        drop(state);
                                        if let Some(text) = text {
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
                                    event::MouseEventKind::Drag(event::MouseButton::Left)
                                        if state.composer_selecting =>
                                    {
                                        if let Some(cursor) = clamped {
                                            let anchor = state
                                                .composer_selection_anchor
                                                .unwrap_or(state.cursor_position);
                                            let (anchor, cursor) = transcript_state
                                                .composer_mouse
                                                .extend(&state.input_buffer, anchor, cursor);
                                            state.composer_selection_anchor = Some(anchor);
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
                                            let anchor = state
                                                .composer_selection_anchor
                                                .unwrap_or(state.cursor_position);
                                            let (anchor, cursor) = transcript_state
                                                .composer_mouse
                                                .extend(&state.input_buffer, anchor, cursor);
                                            state.composer_selection_anchor = Some(anchor);
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
                        // A press on a tool row arms a toggle; a drag turns
                        // the gesture into a selection instead.
                        match mouse.kind {
                            event::MouseEventKind::Down(event::MouseButton::Left) => {
                                transcript_state.tool_click = transcript_state
                                    .tool_block_at(mouse.column, mouse.row)
                                    .filter(|_| mouse.modifiers.is_empty())
                                    .map(|block| ((mouse.column, mouse.row), block));
                                transcript_state.viz_click = transcript_state
                                    .viz_target_at(mouse.column, mouse.row)
                                    .filter(|_| mouse.modifiers.is_empty())
                                    .map(|target| ((mouse.column, mouse.row), target));
                            }
                            event::MouseEventKind::Up(event::MouseButton::Left) => {}
                            _ => {
                                transcript_state.tool_click = None;
                                transcript_state.viz_click = None;
                            }
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
                        if mouse.kind == event::MouseEventKind::Up(event::MouseButton::Left)
                            && let Some((pressed, block)) = transcript_state.tool_click.take()
                            && pressed == (mouse.column, mouse.row)
                            && !transcript_state.selection.has_selection()
                        {
                            let mut state = app_state.lock().await;
                            if !state.modal_open() {
                                let width = terminal_runtime.terminal().area().width;
                                toggle_tool_block(&mut state, width, block);
                                *needs_redraw = true;
                            }
                        }
                        // A click on a table header sorts it; one on a tree
                        // node with children opens or closes it.
                        if mouse.kind == event::MouseEventKind::Up(event::MouseButton::Left)
                            && let Some((pressed, target)) = transcript_state.viz_click.take()
                            && pressed == (mouse.column, mouse.row)
                            && !transcript_state.selection.has_selection()
                        {
                            let snapshot = {
                                let state = app_state.lock().await;
                                (!state.modal_open()).then(|| {
                                    ui::render_snapshot::render_snapshot(
                                        &rustcode::controller::render_state(&state),
                                    )
                                })
                            };
                            if let Some(snapshot) = snapshot {
                                transcript_state.apply_viz_target(&snapshot, target);
                                *needs_redraw = true;
                            }
                        }
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
                        edit_state.insert_str(&normalized);
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
mod tests;
