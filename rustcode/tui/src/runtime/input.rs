use super::*;

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

fn selection_owns_key(transcript: &TranscriptState, key: crossterm::event::KeyEvent) -> bool {
    let has_range_or_mode =
        transcript.selection.has_selection() || transcript.selection.is_keyboard_mode();
    has_range_or_mode
        && (key.code == KeyCode::Esc
            || (matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
                && key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)))
}

fn clear_selection_for_composer_key(
    transcript: &mut TranscriptState,
    key: crossterm::event::KeyEvent,
) {
    if transcript.selection.is_active() && !is_transcript_navigation(key) {
        transcript.selection.clear();
    }
}

fn return_to_latest_for_key(transcript: &mut TranscriptState, key: KeyCode) -> bool {
    if key != KeyCode::Esc || transcript.scroll_rows() == 0 {
        return false;
    }
    transcript.scroll_down(usize::MAX);
    true
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
            rustcode::app::handle_escape(&app_state, current_cancel_token).await;
            *needs_redraw = true;
        }
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
                        ui::render_snapshot::render_snapshot(&state)
                    };
                    transcript_state
                        .selection
                        .begin_keyboard_with_snapshot(snapshot, transcript_state.scroll_rows());
                    return Ok(InputFlow::ContinueIteration);
                }
                if transcript_state.selection.is_keyboard_mode() {
                    if is_keyboard_range_key(key) {
                        transcript_state.selection.move_keyboard(key.code);
                        return Ok(InputFlow::ContinueIteration);
                    }
                }

                if selection_owns_key(transcript_state, key) {
                    if key.code == KeyCode::Esc {
                        transcript_state.selection.clear();
                        return Ok(InputFlow::ContinueIteration);
                    }
                    if let Some(text) = transcript_state.selection.selected_text() {
                        report_selection_copy(
                            app_state,
                            &text,
                            rustcode::clipboard::copy_to_clipboard,
                        )
                        .await;
                    }
                    return Ok(InputFlow::ContinueIteration);
                }

                // Composer selection owns Ctrl/Cmd+C and Esc (#1493).
                // Copy keeps the highlight; Esc dismisses it.
                if app_state.lock().await.has_composer_selection() {
                    if key.code == KeyCode::Esc {
                        app_state.lock().await.clear_composer_selection();
                        return Ok(InputFlow::ContinueIteration);
                    }
                    if matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
                        && key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
                    {
                        if let Some(text) = app_state.lock().await.composer_selected_text() {
                            report_selection_copy(
                                app_state,
                                &text,
                                rustcode::clipboard::copy_to_clipboard,
                            )
                            .await;
                        }
                        return Ok(InputFlow::ContinueIteration);
                    }
                }

                let transcript_navigation = is_transcript_navigation(key);
                clear_selection_for_composer_key(transcript_state, key);

                if is_ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
                    if rustcode::app::handle_ctrl_c(&app_state).await {
                        return Ok(InputFlow::Exit { update: false });
                    }
                    return Ok(InputFlow::ContinueIteration);
                }

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
                        (s.status == AppStatus::AwaitingToolConfirmation).then(|| {
                            let prefix = s
                                .pending_tool_confirmation
                                .as_ref()
                                .filter(|items| {
                                    items.len() == 1 && items[0].rememberable_prefix.is_some()
                                        || items.len() == 1 && items[0].forbidden_prefix.is_some()
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
                    if s.status == AppStatus::AwaitingQuestion {
                        let typing = s
                            .pending_question
                            .as_ref()
                            .map(|q| q.custom_input.is_some())
                            .unwrap_or(false);
                        drop(s);

                        if typing {
                            match key.code {
                                KeyCode::Char('v') | KeyCode::Char('V')
                                    if key.modifiers.contains(event::KeyModifiers::CONTROL)
                                        || key.modifiers.contains(event::KeyModifiers::SUPER)
                                        || key.modifiers.contains(event::KeyModifiers::META) =>
                                {
                                    if let Some(text) =
                                        rustcode::clipboard::read_text_from_clipboard()
                                    {
                                        let normalized =
                                            text.replace("\r\n", "\n").replace('\r', "\n");
                                        let mut s = app_state.lock().await;
                                        if let Some(q) = s.pending_question.as_mut() {
                                            q.insert_str(&normalized);
                                        }
                                    }
                                }
                                KeyCode::Char('a') | KeyCode::Char('A')
                                    if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                                {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.move_cursor_home();
                                    }
                                }
                                KeyCode::Char('e') | KeyCode::Char('E')
                                    if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                                {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.move_cursor_end();
                                    }
                                }
                                KeyCode::Char('w') | KeyCode::Char('W')
                                    if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                                {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.delete_word_before();
                                    }
                                }
                                KeyCode::Char(c) => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.insert_char(c);
                                    }
                                }
                                KeyCode::Backspace => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        if key.modifiers.contains(event::KeyModifiers::ALT) {
                                            q.delete_word_before();
                                        } else {
                                            q.delete_char_before();
                                        }
                                    }
                                }
                                KeyCode::Delete => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.delete_char_after();
                                    }
                                }
                                KeyCode::Left => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        if key.modifiers.contains(event::KeyModifiers::ALT)
                                            || key.modifiers.contains(event::KeyModifiers::CONTROL)
                                        {
                                            q.move_cursor_word_left();
                                        } else {
                                            q.move_cursor_left();
                                        }
                                    }
                                }
                                KeyCode::Right => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        if key.modifiers.contains(event::KeyModifiers::ALT)
                                            || key.modifiers.contains(event::KeyModifiers::CONTROL)
                                        {
                                            q.move_cursor_word_right();
                                        } else {
                                            q.move_cursor_right();
                                        }
                                    }
                                }
                                KeyCode::Home => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.move_cursor_home();
                                    }
                                }
                                KeyCode::End => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.move_cursor_end();
                                    }
                                }
                                KeyCode::Up => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.selected = q.selected.saturating_sub(1);
                                        if q.selected < q.options.len() {
                                            q.custom_input = None;
                                            q.custom_cursor = 0;
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
                                KeyCode::Enter => {
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
                                KeyCode::Esc => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        q.custom_input = None;
                                        q.custom_cursor = 0;
                                    }
                                }
                                _ => {}
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
                    if s.status == AppStatus::VerbosityPicker {
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

                    if s.status == AppStatus::ThinkingPicker {
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

                    if s.status == AppStatus::EffortPicker {
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

                    if s.status == AppStatus::ProtocolPicker {
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
                                s.history.push(ChatMessage::new(
                                    "system",
                                    format!(
                                        "Switched tool protocol to {} for model '{}'.",
                                        label, active_model
                                    ),
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

                    if s.status == AppStatus::YoloPicker {
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
                    match key.code {
                        KeyCode::Esc => {
                            s.show_subagent_picker = false;
                        }
                        KeyCode::Up => {
                            if total > 0 {
                                s.subagent_picker_index = if s.subagent_picker_index == 0 {
                                    total - 1
                                } else {
                                    s.subagent_picker_index - 1
                                };
                            }
                        }
                        KeyCode::Down => {
                            if total > 0 {
                                s.subagent_picker_index = (s.subagent_picker_index + 1) % total;
                            }
                        }
                        KeyCode::Enter => {
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
                        _ => {}
                    }
                    drop(s);
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
                            if len > 0 {
                                s.history_picker_index = if s.history_picker_index == 0 {
                                    len - 1
                                } else {
                                    s.history_picker_index - 1
                                };
                            }
                        }
                        KeyCode::Down => {
                            let len = s.history_picker_sessions.len();
                            if len > 0 {
                                s.history_picker_index = if s.history_picker_index + 1 >= len {
                                    0
                                } else {
                                    s.history_picker_index + 1
                                };
                            }
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
                    let existing_mcp_always_include = s
                        .mcp_edit_state
                        .as_ref()
                        .and_then(|edit_state| edit_state.edit_index)
                        .and_then(|idx| s.config.mcp_servers.get(idx))
                        .is_some_and(|server| server.always_include);
                    if let Some(ref mut edit_state) = s.mcp_edit_state {
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
                                if len > 0 {
                                    s.mcp_picker_index = if s.mcp_picker_index == 0 {
                                        len - 1
                                    } else {
                                        s.mcp_picker_index - 1
                                    };
                                }
                            }
                            KeyCode::Down => {
                                let len = s.config.mcp_servers.len();
                                if len > 0 {
                                    s.mcp_picker_index = if s.mcp_picker_index + 1 >= len {
                                        0
                                    } else {
                                        s.mcp_picker_index + 1
                                    };
                                }
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
                    match key.code {
                        KeyCode::Esc => {
                            s.show_model_picker = false;
                        }
                        KeyCode::Up => {
                            let len = rustcode::app::get_picker_items_count(&s);
                            if len > 0 {
                                s.model_picker_index = if s.model_picker_index == 0 {
                                    len - 1
                                } else {
                                    s.model_picker_index - 1
                                };
                            }
                        }
                        KeyCode::Down => {
                            let len = rustcode::app::get_picker_items_count(&s);
                            if len > 0 {
                                s.model_picker_index = if s.model_picker_index + 1 >= len {
                                    0
                                } else {
                                    s.model_picker_index + 1
                                };
                            }
                        }
                        KeyCode::Enter => {
                            rustcode::app::select_picker_model(&mut s);
                            s.show_model_picker = false;
                            rustcode::app::spawn_context_window_detection(
                                Arc::clone(&app_state),
                                client.clone(),
                            );
                        }
                        KeyCode::Backspace => {
                            s.model_picker_search.pop();
                            s.model_picker_index = 0;
                        }
                        KeyCode::Char(c)
                            if !key.modifiers.contains(event::KeyModifiers::CONTROL)
                                && !key.modifiers.contains(event::KeyModifiers::ALT) =>
                        {
                            s.model_picker_search.push(c);
                            s.model_picker_index = 0;
                        }
                        _ => {}
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
                            if len > 0 {
                                s.theme_picker_index = if s.theme_picker_index == 0 {
                                    len - 1
                                } else {
                                    s.theme_picker_index - 1
                                };
                                s.config.theme = themes[s.theme_picker_index].name.clone();
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if len > 0 {
                                s.theme_picker_index = if s.theme_picker_index + 1 >= len {
                                    0
                                } else {
                                    s.theme_picker_index + 1
                                };
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
                            s.set_notice(format!("Theme set to '{}'", selected));
                        }
                        _ => {}
                    }
                    drop(s);
                    return Ok(InputFlow::ContinueIteration);
                }

                if s.show_command_picker {
                    let search = s.command_picker_search.to_lowercase();
                    let filtered_items: Vec<&crate::ui::PaletteItem> = crate::ui::PALETTE_ITEMS
                        .iter()
                        .filter(|item| {
                            item.name.to_lowercase().contains(&search)
                                || item.group.to_lowercase().contains(&search)
                                || item.shortcut.to_lowercase().contains(&search)
                        })
                        .collect();

                    let mut exit_flag = false;
                    match key.code {
                        KeyCode::Esc => {
                            s.show_command_picker = false;
                        }
                        KeyCode::Up => {
                            let len = filtered_items.len();
                            if len > 0 {
                                s.command_picker_index = if s.command_picker_index == 0 {
                                    len - 1
                                } else {
                                    s.command_picker_index - 1
                                };
                            }
                        }
                        KeyCode::Down => {
                            let len = filtered_items.len();
                            if len > 0 {
                                s.command_picker_index = if s.command_picker_index + 1 >= len {
                                    0
                                } else {
                                    s.command_picker_index + 1
                                };
                            }
                        }
                        KeyCode::Enter => {
                            let idx = s
                                .command_picker_index
                                .min(filtered_items.len().saturating_sub(1));
                            if !filtered_items.is_empty() {
                                let item = filtered_items[idx];
                                s.show_command_picker = false;
                                match item.shortcut {
                                    "ctrl+c" => {
                                        exit_flag = true;
                                    }
                                    "/model" | "/models" => {
                                        s.show_model_picker = true;
                                    }
                                    "/new" => {
                                        current_cancel_token.cancel();
                                        *current_cancel_token =
                                            tokio_util::sync::CancellationToken::new();
                                        rustcode::app::start_new_session(&mut s);
                                    }
                                    "/resume" => {
                                        rustcode::app::resume_latest_session(&mut s);
                                    }
                                    "/continue" => {
                                        let queued =
                                            rustcode::app::actions::queue_restored_segment(&mut s);
                                        let message = if queued {
                                            "Queued the pending session work."
                                        } else {
                                            "No pending session work is available to continue."
                                        };
                                        s.history.push(ChatMessage::new("system", message));
                                    }
                                    "/agents" => {
                                        s.show_subagent_picker = true;
                                        s.subagent_picker_index = 0;
                                    }
                                    "/skills" => {
                                        let skills = rustcode::skills::discover_skills();
                                        s.history.push(ChatMessage::new(
                                            "system",
                                            rustcode::skills::format_skill_catalog(&skills),
                                        ));
                                    }
                                    "/info" | "/about" => {
                                        let info = rustcode::app::actions::build_info_text();
                                        s.history.push(ChatMessage::new("system", info));
                                    }
                                    "/changelog" => {
                                        let log_text =
                                            rustcode::app::actions::build_latest_changelog();
                                        s.history.push(ChatMessage::new("assistant", log_text));
                                    }
                                    "/quota" => {
                                        rustcode::app::actions::trigger_quota_fetch(
                                            &s, &app_state, &client,
                                        );
                                    }
                                    "/sync" => {
                                        rustcode::app::actions::trigger_sync(
                                            &app_state, None, None,
                                        );
                                    }
                                    "/update" => {
                                        s.update_check =
                                            rustcode_core::update::UpdateState::Checking;
                                        s.set_notice("🔍 Checking for a RustCode update...");
                                        rustcode::app::actions::trigger_update(&app_state, &client);
                                    }
                                    "/copy" => {
                                        rustcode::app::copy_last_reply(&mut s);
                                    }
                                    "/help" => {
                                        let help = rustcode::app::build_help_text();
                                        s.history.push(ChatMessage::new("system", help));
                                    }
                                    "/context" => {
                                        s.show_context_modal = true;
                                    }
                                    "/parser" | "/protocol" => {
                                        s.history.push(ChatMessage::new(
                                            "system",
                                            "Only JSON tool format is supported",
                                        ));
                                    }
                                    "/provider" => {
                                        s.history.push(ChatMessage::new(
                                    "system",
                                    "Use /provider <name> <url> <model> to configure a provider profile",
                                ));
                                    }
                                    "/ollama" => {
                                        s.history.push(ChatMessage::new(
                                            "system",
                                            "Use /ollama list to list available Ollama models",
                                        ));
                                    }
                                    "/mcp" => {
                                        s.show_mcp_config = true;
                                        s.mcp_picker_index = 0;
                                        s.mcp_edit_state = None;
                                    }
                                    "/change_title" => {
                                        s.history.push(ChatMessage::new(
                                            "system",
                                            "Use /change_title <new title> to rename this session",
                                        ));
                                    }
                                    "/clear" => {
                                        s.history_display_start = s.history.len();
                                        s.clear_current_response();
                                        s.current_token_usage = None;
                                        s.enter_idle();
                                    }
                                    "/cancel" => {
                                        current_cancel_token.cancel();
                                        *current_cancel_token =
                                            tokio_util::sync::CancellationToken::new();
                                    }
                                    "/yolo" => {
                                        s.modal_picker_index = if s.auto_confirm { 0 } else { 1 };
                                        s.status = rustcode::app::AppStatus::YoloPicker;
                                    }
                                    "/status" => {
                                        s.show_status_modal = true;
                                    }
                                    "/stats" | "/usage" => {
                                        s.open_stats_modal();
                                    }
                                    "/session" => {
                                        s.show_session_modal = true;
                                    }
                                    "/memory" => {
                                        rustcode::app::check_memory_usage(&mut s);
                                    }
                                    "/tools" => {
                                        let mut text = String::from("Available tools:");
                                        for t in rustcode::tools::TOOLS {
                                            text.push_str(&format!("\n  {}", t.name));
                                        }
                                        s.history.push(ChatMessage::new("system", text));
                                    }
                                    _ => {}
                                }
                            } else {
                                s.show_command_picker = false;
                            }
                        }
                        KeyCode::Backspace => {
                            s.command_picker_search.pop();
                            s.command_picker_index = 0;
                        }
                        KeyCode::Char(c)
                            if !key.modifiers.contains(event::KeyModifiers::CONTROL)
                                && !key.modifiers.contains(event::KeyModifiers::ALT) =>
                        {
                            s.command_picker_search.push(c);
                            s.command_picker_index = 0;
                        }
                        _ => {}
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
                        if let Some(img_markdown) =
                            rustcode::clipboard::paste_image_from_clipboard()
                        {
                            let mut state = app_state.lock().await;
                            composer.handle_paste(&mut state, &img_markdown);
                        } else if let Some(text) = rustcode::clipboard::read_text_from_clipboard() {
                            let mut state = app_state.lock().await;
                            composer.handle_paste(&mut state, &text);
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
                        let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
                        if shift && s.composer_selection_anchor.is_none() {
                            s.composer_selection_anchor = Some(s.cursor_position);
                        }
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
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
                        let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
                        if shift && s.composer_selection_anchor.is_none() {
                            s.composer_selection_anchor = Some(s.cursor_position);
                        }
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
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
                        if let Some(img_markdown) =
                            rustcode::clipboard::paste_image_from_clipboard()
                        {
                            let mut s = app_state.lock().await;
                            for c in img_markdown.chars() {
                                s.insert_char(c);
                            }
                            s.reset_suggestion_cycle();
                        } else if let Some(text) = rustcode::clipboard::read_text_from_clipboard() {
                            let mut s = app_state.lock().await;
                            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                            const PASTE_THRESHOLD: usize = 300;
                            let text_to_insert = if normalized.chars().count() >= PASTE_THRESHOLD {
                                format!(
                                    "<!--PASTE:{}:{}-->",
                                    normalized.chars().count(),
                                    normalized
                                )
                            } else {
                                normalized
                            };
                            for c in text_to_insert.chars() {
                                s.insert_char(c);
                            }
                            s.reset_suggestion_cycle();
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
                            s.move_cursor_word_left();
                        } else if (alt && c == 'f') || c == 'ƒ' {
                            s.move_cursor_word_right();
                        } else if (alt && c == 'd') || c == '∂' {
                            s.delete_word_forward();
                            s.reset_suggestion_cycle();
                        } else if ctrl && c == 'o' {
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
                    event::MouseEventKind::ScrollUp if transcript_state.selection.is_active() => {
                        transcript_state.selection.queue_scroll(-1, 1);
                    }
                    event::MouseEventKind::ScrollDown if transcript_state.selection.is_active() => {
                        transcript_state.selection.queue_scroll(1, 1);
                    }
                    event::MouseEventKind::ScrollUp => transcript_state.scroll_up(1),
                    event::MouseEventKind::ScrollDown => transcript_state.scroll_down(1),
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
                        let selected = if mouse.kind
                            == event::MouseEventKind::Down(event::MouseButton::Left)
                            && !(mouse.modifiers.contains(KeyModifiers::SHIFT)
                                && transcript_state.selection.has_selection())
                        {
                            let snapshot = {
                                let state = app_state.lock().await;
                                ui::render_snapshot::render_snapshot(&state)
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
                        // the highlight and copies via Ctrl/Cmd+C (#1492).
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
                        for c in img_markdown.chars() {
                            s.insert_char(c);
                        }
                        s.reset_suggestion_cycle();
                    }
                    *needs_redraw = true;
                    return Ok(InputFlow::ContinueIteration);
                }
                let mut s = app_state.lock().await;
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                // Route the paste into whichever text field is focused: the
                // ask_question custom-answer slot, the MCP editor, else chat.
                if s.status == AppStatus::AwaitingQuestion {
                    if let Some(q) = s.pending_question.as_mut() {
                        if q.custom_input.is_some() {
                            q.insert_str(&normalized);
                        }
                    }
                } else if s.show_mcp_config {
                    if let Some(ref mut edit_state) = s.mcp_edit_state {
                        for c in normalized.chars() {
                            if c != '\n' && c != '\r' {
                                edit_state.insert_char(c);
                            }
                        }
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
        clear_selection_for_composer_key, is_keyboard_range_key, is_shift_tab,
        is_transcript_navigation, report_selection_copy, return_to_latest_for_key,
        selection_owns_key,
    };
    use crate::ui::TranscriptState;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{buffer::Buffer, layout::Rect};
    use rustcode::app::AppState;
    use rustcode::clipboard::ClipboardCopyStatus;
    use std::sync::Arc;
    use tokio::sync::Mutex;

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
            crate::ui::render_snapshot::render_snapshot(&AppState::new()),
            0,
        );
        assert!(transcript.selection.is_keyboard_mode());
        assert!(selection_owns_key(
            &transcript,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        ));
        assert!(selection_owns_key(
            &transcript,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
        assert!(selection_owns_key(
            &transcript,
            KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SUPER)
        ));
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
                crate::ui::render_snapshot::render_snapshot(&AppState::new()),
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
}
