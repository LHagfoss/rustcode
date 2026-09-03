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
    pub(super) terminal_focused: &'a mut bool,
    pub(super) transcript_state: &'a mut TranscriptState,
    pub(super) app_event_sender: &'a AppEventSender,
    pub(super) composer: &'a ui::Composer,
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
        terminal_focused,
        transcript_state,
        app_event_sender,
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
                    crate::update::UpdateState::Available(latest) => Some(latest),
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
            current_cancel_token.cancel();
            *current_cancel_token = CancellationToken::new();
            let mut state = app_state.lock().await;
            state.pending_queue.clear();
            state.background_turn_context = None;
            state.clear_live_tool_calls();
            state.status = AppStatus::Idle;
            state.request_redraw();
            *needs_redraw = true;
        }
        AppEvent::Tui(ev) => match ev {
            TuiEvent::Key(key) => {
                *needs_redraw = true;
                app_state.lock().await.mark_user_activity();
                let is_ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                let is_cmd = key.modifiers.contains(event::KeyModifiers::SUPER);

                if is_ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
                    if crate::app::handle_ctrl_c(&app_state).await {
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
                        (s.status == AppStatus::AwaitingToolConfirmation)
                            .then_some(s.tool_confirmation_selected)
                    };
                    if let Some(selected) = selected {
                        if let Some(event) = ui::approval_event_for_key(key, selected) {
                            let _ = app_event_sender.send(event);
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
                                    if let Some(text) = crate::clipboard::read_text_from_clipboard()
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
                                crate::config::save_entire_config(&s.config);
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
                                crate::config::save_entire_config(&s.config);
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
                                crate::config::save_entire_config(&s.config);
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
                                    0 => (crate::config::ToolProtocol::Json, "JSON (```tool)"),
                                    1 => (
                                        crate::config::ToolProtocol::Native,
                                        "Native ([TOOL_CALLS])",
                                    ),
                                    _ => (
                                        crate::config::ToolProtocol::ApiNative,
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
                                crate::config::save_entire_config(&s.config);
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
                                s.set_notice(format!("YOLO mode {status}"));
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
                                    .and_then(crate::app::session_controller::session_id_from_meta)
                                    .map(crate::app::events::SessionAction::Id);
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
                                .and_then(crate::app::session_controller::session_id_from_meta)
                                .map(crate::app::events::SessionAction::Id)
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

                                if !name.is_empty() && !command.is_empty() {
                                    let new_srv = crate::config::McpServerConfig {
                                        name: name.clone(),
                                        command,
                                        args,
                                        env: std::collections::HashMap::new(),
                                        enabled: true,
                                    };

                                    if edit_state.is_add {
                                        s.config.mcp_servers.push(new_srv);
                                    } else if let Some(idx) = edit_state.edit_index
                                        && idx < s.config.mcp_servers.len()
                                    {
                                        let old_name = s.config.mcp_servers[idx].name.clone();
                                        s.config.mcp_servers[idx] = new_srv;
                                        if old_name != name {
                                            crate::mcp::shutdown_server(&old_name).await;
                                        }
                                    }

                                    crate::config::save_entire_config(&s.config);

                                    let name_clone = name.clone();
                                    tokio::spawn(async move {
                                        let _ = crate::mcp::start_server_by_name(&name_clone).await;
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
                                s.mcp_edit_state = Some(crate::app::McpEditState {
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
                                    s.mcp_edit_state = Some(crate::app::McpEditState {
                                        is_add: false,
                                        edit_index: Some(idx),
                                        name_input: srv.name.clone(),
                                        command_input: srv.command.clone(),
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
                                    crate::config::save_entire_config(&s.config);
                                    let name_clone = removed.name.clone();
                                    tokio::spawn(async move {
                                        crate::mcp::shutdown_server(&name_clone).await;
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
                                    crate::config::save_entire_config(&s.config);
                                    tokio::spawn(async move {
                                        if enabled {
                                            let _ =
                                                crate::mcp::start_server_by_name(&name_clone).await;
                                        } else {
                                            crate::mcp::shutdown_server(&name_clone).await;
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
                            let len = crate::app::get_picker_items_count(&s);
                            if len > 0 {
                                s.model_picker_index = if s.model_picker_index == 0 {
                                    len - 1
                                } else {
                                    s.model_picker_index - 1
                                };
                            }
                        }
                        KeyCode::Down => {
                            let len = crate::app::get_picker_items_count(&s);
                            if len > 0 {
                                s.model_picker_index = if s.model_picker_index + 1 >= len {
                                    0
                                } else {
                                    s.model_picker_index + 1
                                };
                            }
                        }
                        KeyCode::Enter => {
                            crate::app::select_picker_model(&mut s);
                            s.show_model_picker = false;
                            crate::app::spawn_context_window_detection(
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
                            crate::config::save_entire_config(&s.config);
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
                                    "/model" => {
                                        s.show_model_picker = true;
                                    }
                                    "/new" => {
                                        current_cancel_token.cancel();
                                        *current_cancel_token =
                                            tokio_util::sync::CancellationToken::new();
                                        crate::app::start_new_session(&mut s);
                                    }
                                    "/resume" => {
                                        crate::app::resume_latest_session(&mut s);
                                    }
                                    "/agents" => {
                                        s.show_subagent_picker = true;
                                        s.subagent_picker_index = 0;
                                    }
                                    "/skills" => {
                                        let skills = crate::skills::discover_skills();
                                        if skills.is_empty() {
                                            s.history.push(ChatMessage::new(
                                        "system",
                                        "No skills discovered.\nPlace SKILL.md files in .rustcode/skills/ or ~/.config/rustcode/skills/",
                                    ));
                                        } else {
                                            let mut out = format!(
                                                "📦 Discovered Skills ({}):\n\n",
                                                skills.len()
                                            );
                                            for skill in &skills {
                                                out.push_str(&format!("  • {}\n", skill.name));
                                                out.push_str(&format!(
                                                    "    Description: {}\n",
                                                    skill.description
                                                ));
                                                out.push_str(&format!(
                                                    "    Path: {}\n\n",
                                                    skill.path.display()
                                                ));
                                            }
                                            s.history.push(ChatMessage::new("system", out));
                                        }
                                    }
                                    "/info" | "/about" => {
                                        let info = crate::app::actions::build_info_text();
                                        s.history.push(ChatMessage::new("system", info));
                                    }
                                    "/changelog" => {
                                        let log_text =
                                            crate::app::actions::build_latest_changelog();
                                        s.history.push(ChatMessage::new("assistant", log_text));
                                    }
                                    "/quota" => {
                                        crate::app::actions::trigger_quota_fetch(
                                            &s, &app_state, &client,
                                        );
                                    }
                                    "/sync" => {
                                        crate::app::actions::trigger_sync(&app_state, None, None);
                                    }
                                    "/update" => {
                                        s.update_check = crate::update::UpdateState::Checking;
                                        s.set_notice("🔍 Checking for a RustCode update...");
                                        crate::app::actions::trigger_update(&app_state, &client);
                                    }
                                    "/copy" => {
                                        crate::app::copy_last_reply(&mut s);
                                    }
                                    "/help" => {
                                        let help = crate::app::build_help_text();
                                        s.history.push(ChatMessage::new("system", help));
                                    }
                                    "/context" => {
                                        s.history.push(ChatMessage::new(
                                    "system",
                                    "Use /context <tokens> to set context window (e.g. /context 262144)",
                                ));
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
                                        s.status = crate::app::AppStatus::Idle;
                                    }
                                    "/cancel" => {
                                        current_cancel_token.cancel();
                                        *current_cancel_token =
                                            tokio_util::sync::CancellationToken::new();
                                    }
                                    "/yolo" => {
                                        s.modal_picker_index = if s.auto_confirm { 0 } else { 1 };
                                        s.status = crate::app::AppStatus::YoloPicker;
                                    }
                                    "/stats" | "/usage" | "/status" => {
                                        s.history.push(ChatMessage::new(
                                            "system",
                                            "Token usage data will appear after your next message",
                                        ));
                                    }
                                    "/memory" => {
                                        crate::app::check_memory_usage(&mut s);
                                    }
                                    "/tools" => {
                                        let mut text = String::from("Available tools:");
                                        for t in crate::tools::TOOLS {
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
                dbg_log!(
                    "[KEY_EVENT] code={:?} modifiers={:?}",
                    key.code,
                    key.modifiers
                );

                match {
                    let mut state = app_state.lock().await;
                    composer.handle_key(&mut state, key)
                } {
                    ui::ComposerAction::Handled => {
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    ui::ComposerAction::Submit => {
                        if crate::app::handle_enter(&app_state, &client, current_cancel_token).await
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
                        if let Some(img_markdown) = crate::clipboard::paste_image_from_clipboard() {
                            let mut state = app_state.lock().await;
                            composer.handle_paste(&mut state, &img_markdown);
                        } else if let Some(text) = crate::clipboard::read_text_from_clipboard() {
                            let mut state = app_state.lock().await;
                            composer.handle_paste(&mut state, &text);
                        }
                        *needs_redraw = true;
                        return Ok(InputFlow::ContinueIteration);
                    }
                    ui::ComposerAction::Unhandled => {}
                }

                match key.code {
                    KeyCode::BackTab => {
                        let mut s = app_state.lock().await;
                        s.auto_confirm = !s.auto_confirm;
                    }
                    KeyCode::Esc => {
                        let mut s = app_state.lock().await;
                        if s.dismiss_completion() {
                            // Popup dismissal keeps the draft intact. Typing or moving
                            // to another token makes completion eligible again.
                        } else if s.sel_start.is_some() || s.sel_end.is_some() {
                            s.clear_selection();
                        } else if !s.input_buffer.is_empty() {
                            s.input_buffer.clear();
                            s.cursor_position = 0;
                        } else {
                            drop(s);
                            crate::app::handle_escape(&app_state, current_cancel_token).await;
                        }
                        *needs_redraw = true;
                    }
                    KeyCode::Up => {
                        let mut s = app_state.lock().await;
                        let completion_len =
                            crate::app::get_completion_len(&s.input_buffer, s.cursor_position);
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
                            crate::app::get_completion_len(&s.input_buffer, s.cursor_position);
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
                    KeyCode::Tab => {
                        let mut s = app_state.lock().await;
                        s.dismissed_completion = None;
                        let has_at =
                            crate::app::get_at_word_query(&s.input_buffer, s.cursor_position)
                                .is_some();
                        if s.active_suggestion_index.is_some() || has_at {
                            crate::app::apply_autocomplete(&mut s);
                        } else if crate::app::suggestion::command_token(&s.input_buffer).is_some() {
                            s.cycle_suggestion();
                        } else {
                            // Toggle Agent Mode (Build vs Plan)
                            s.agent_mode = match s.agent_mode {
                                crate::config::AgentMode::Build => crate::config::AgentMode::Plan,
                                crate::config::AgentMode::Plan => crate::config::AgentMode::Build,
                            };
                            s.config.agent_mode = s.agent_mode;
                            crate::config::save_entire_config(&s.config);

                            let notice = match s.agent_mode {
                                crate::config::AgentMode::Build => {
                                    "Switched to Build Mode (Full Code Editing)"
                                }
                                crate::config::AgentMode::Plan => {
                                    "Switched to Plan Mode (Read-only / Design only)"
                                }
                            };
                            s.set_notice(notice);
                        }
                    }
                    KeyCode::Left => {
                        let mut s = app_state.lock().await;
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
                        if alt {
                            s.move_cursor_word_left();
                        } else {
                            s.move_cursor_left();
                        }
                    }
                    KeyCode::Right => {
                        let mut s = app_state.lock().await;
                        let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                            || key.modifiers.contains(event::KeyModifiers::META);
                        if alt {
                            s.move_cursor_word_right();
                        } else {
                            s.move_cursor_right();
                        }
                    }
                    KeyCode::Home => {
                        app_state.lock().await.move_cursor_to_start();
                    }
                    KeyCode::End => {
                        app_state.lock().await.move_cursor_to_end();
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
                            if crate::app::handle_enter(&app_state, &client, current_cancel_token)
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
                        if let Some(img_markdown) = crate::clipboard::paste_image_from_clipboard() {
                            let mut s = app_state.lock().await;
                            for c in img_markdown.chars() {
                                s.insert_char(c);
                            }
                            s.reset_suggestion_cycle();
                        } else if let Some(text) = crate::clipboard::read_text_from_clipboard() {
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
                            crate::app::get_completion_len(&s.input_buffer, s.cursor_position);
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
                                .push(ChatMessage::new("system", crate::app::build_help_text()));
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
            TuiEvent::FocusGained => {
                *terminal_focused = true;
                *needs_redraw = true;
            }
            TuiEvent::FocusLost => {
                *terminal_focused = false;
                *needs_redraw = true;
            }
            TuiEvent::Paste(text) => {
                app_state.lock().await.mark_user_activity();
                // Terminals with bracketed paste enabled deliver Cmd+V through
                // this event instead of the Char('v') key handler. When the
                // clipboard holds an image (e.g. a screenshot), the pasted text
                // is empty — fall back to grabbing the image so it still turns
                // into an `![image](file://…)` marker that renders as [Image #N].
                if text.trim().is_empty()
                    && let Some(img_markdown) = crate::clipboard::paste_image_from_clipboard()
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
                    const PASTE_THRESHOLD: usize = 300;
                    let text_to_insert = if normalized.chars().count() >= PASTE_THRESHOLD {
                        format!("<!--PASTE:{}:{}-->", normalized.chars().count(), normalized)
                    } else {
                        normalized
                    };
                    for c in text_to_insert.chars() {
                        s.insert_char(c);
                    }
                    s.reset_suggestion_cycle();
                }
                *needs_redraw = true;
            }
            TuiEvent::Resize { .. } => {
                *needs_redraw = true;
            }
            TuiEvent::Draw => {
                *needs_redraw = true;
            }
        },
        _ => {}
    }
    Ok(InputFlow::ContinueLoop)
}
