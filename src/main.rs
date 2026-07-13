mod app;
mod clipboard;
mod config;
mod context;
mod network;
mod notifications;
mod raw_cli;
mod remote_server;
mod symbols;
mod tools;
mod ui;

use crate::app::{AppState, AppStatus, ChatMessage};
use crossterm::{
    cursor::SetCursorStyle,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut model_override = None;
    if let Some(pos) = args.iter().position(|arg| arg == "--model" || arg == "-m") {
        if pos + 1 < args.len() {
            model_override = Some(args[pos + 1].clone());
        }
    }

    if args.iter().any(|arg| arg == "--raw" || arg == "-r") {
        let prompt_idx = args
            .iter()
            .position(|arg| arg == "--raw" || arg == "-r")
            .unwrap()
            + 1;
        if prompt_idx < args.len() {
            let prompt = args[prompt_idx].clone();
            raw_cli::run_raw_cli(&prompt, model_override.as_deref()).await?;
            return Ok(());
        } else {
            eprintln!("Usage: rustcode --raw <prompt> [--model <model_name>]");
            return Ok(());
        }
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableFocusChange,
        SetCursorStyle::BlinkingBlock,
        crossterm::style::Print("\x1b]0;rustcode · new session\x07")
    )?;

    let keyboard_enhanced = matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    );
    if keyboard_enhanced {
        execute!(
            stdout,
            event::PushKeyboardEnhancementFlags(
                event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        )?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    crate::config::archive_live_history();

    let mut app_state_struct = AppState::new();
    if let Some(ref m_name) = model_override {
        if let Some(profile) = app_state_struct
            .config
            .models
            .iter()
            .find(|m| m.name == *m_name)
        {
            app_state_struct.api_base_url = profile.url.clone();
            app_state_struct.model_name = profile.model.clone();
        }
    }
    app_state_struct.history = Vec::new();
    let app_state = Arc::new(Mutex::new(app_state_struct));

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut current_cancel_token = tokio_util::sync::CancellationToken::new();
    app_state.lock().await.cancel_token = Some(current_cancel_token.clone());

    crate::app::spawn_context_window_detection(Arc::clone(&app_state), client.clone());

    let mut needs_redraw = true;
    let mut last_draw = std::time::Instant::now();
    let mut was_responding = false;
    let mut terminal_focused = true;

    loop {
        if current_cancel_token.is_cancelled() {
            current_cancel_token = tokio_util::sync::CancellationToken::new();
            app_state.lock().await.cancel_token = Some(current_cancel_token.clone());
        }
        let response_active = app_state.lock().await.status != AppStatus::Idle;

        {
            let mut s = app_state.lock().await;
            if s.status == AppStatus::Idle && !s.pending_queue.is_empty() {
                s.status = AppStatus::Queued;
                let client_clone = client.clone();
                let state_clone = Arc::clone(&app_state);
                let token_clone = current_cancel_token.clone();
                drop(s);
                tokio::spawn(async move {
                    crate::network::process_queue_orchestrator(
                        client_clone,
                        state_clone,
                        token_clone,
                    )
                    .await;
                });
                needs_redraw = true;
            }
        }

        if was_responding && !response_active && !terminal_focused {
            use crossterm::style::Print;
            let _ = execute!(
                terminal.backend_mut(),
                Print("\x1b]9;rustcode · response finished\x07\x07")
            );
        }
        was_responding = response_active;
        let should_draw = needs_redraw
            || (response_active && last_draw.elapsed() >= std::time::Duration::from_millis(50))
            || last_draw.elapsed() >= std::time::Duration::from_millis(250);

        if should_draw {
            let mut guard = app_state.lock().await;

            // Update terminal title based on session state
            let title_display = if guard.history.is_empty() {
                "rustcode".to_string()
            } else {
                let first_user_msg = guard
                    .history
                    .iter()
                    .find(|m| m.role == "user" && !m.content.starts_with('/'));
                match first_user_msg {
                    Some(msg) => {
                        let title = msg.content.lines().next().unwrap_or("").trim();
                        if title.is_empty() || title.starts_with('/') {
                            "rustcode".to_string()
                        } else {
                            let display_title = title.replace('|', "\\|").replace('\x07', "");
                            format!("rustcode · {}", display_title)
                        }
                    }
                    None => "rustcode".to_string(),
                }
            };

            // Only update if the title changed to avoid unnecessary OSC sequences
            let old_title = guard.current_terminal_title.clone();
            if old_title.as_deref() != Some(title_display.as_str()) {
                use crossterm::style::Print;
                let _ = execute!(
                    terminal.backend_mut(),
                    Print(format!("\x1b]0;{}\x07", title_display))
                );
                guard.current_terminal_title = Some(title_display.clone());
            }

            terminal.draw(|f| ui::render(f, &mut guard))?;
            drop(guard);
            last_draw = std::time::Instant::now();
            needs_redraw = false;
        }

        if event::poll(std::time::Duration::from_millis(50))? {
            let ev = event::read()?;
            match ev {
                Event::Key(key) => {
                    if key.kind == event::KeyEventKind::Release {
                        continue;
                    }
                    needs_redraw = true;

                    if key.modifiers.contains(event::KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        break;
                    }

                    {
                        let s = app_state.lock().await;
                        if s.status == AppStatus::AwaitingToolConfirmation {
                            drop(s);
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                    let mut s = app_state.lock().await;
                                    s.pending_tool_confirmation = None;
                                    if let Some(tx) = s.tool_confirmation_response.take() {
                                        let _ = tx.send(true);
                                    }
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                    // Cancel the running agent stream when denying
                                    current_cancel_token.cancel();
                                    let new_token = tokio_util::sync::CancellationToken::new();
                                    current_cancel_token = new_token;
                                    let mut s = app_state.lock().await;
                                    s.cancel_token = Some(current_cancel_token.clone());
                                    if let Some(tx) = s.tool_confirmation_response.take() {
                                        let _ = tx.send(false);
                                    }
                                    s.pending_tool_confirmation = None;
                                }
                                KeyCode::Tab => {
                                    let mut s = app_state.lock().await;
                                    s.auto_confirm = !s.auto_confirm;
                                }
                                _ => {}
                            }
                            continue;
                        }
                    }

                    let mut s = app_state.lock().await;
                    if s.show_history_picker {
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
                                if let Some(meta) = s.history_picker_sessions.get(idx).cloned() {
                                    crate::app::load_session_into(&mut s, &meta);
                                    let title_display =
                                        meta.title.replace('|', "\\|").replace('\x07', "");
                                    let _ = execute!(
                                        terminal.backend_mut(),
                                        crossterm::style::Print(format!(
                                            "\x1b]0;rustcode · {}\x07",
                                            title_display
                                        ))
                                    );
                                }
                                s.show_history_picker = false;
                            }
                            _ => {}
                        }
                        drop(s);
                        continue;
                    }

                    if s.show_remote_modal {
                        match key.code {
                            KeyCode::Esc => {
                                s.show_remote_modal = false;
                            }
                            KeyCode::Tab => {
                                use crate::app::RemoteModalField;
                                s.remote_modal_field = match s.remote_modal_field {
                                    RemoteModalField::Host => RemoteModalField::Port,
                                    RemoteModalField::Port => RemoteModalField::Token,
                                    RemoteModalField::Token => RemoteModalField::Host,
                                };
                            }
                            KeyCode::BackTab => {
                                use crate::app::RemoteModalField;
                                s.remote_modal_field = match s.remote_modal_field {
                                    RemoteModalField::Host => RemoteModalField::Token,
                                    RemoteModalField::Port => RemoteModalField::Host,
                                    RemoteModalField::Token => RemoteModalField::Port,
                                };
                            }
                            KeyCode::Backspace => {
                                use crate::app::RemoteModalField;
                                match s.remote_modal_field {
                                    RemoteModalField::Host => {
                                        s.remote_host.pop();
                                    }
                                    RemoteModalField::Port => {
                                        s.remote_port.pop();
                                    }
                                    RemoteModalField::Token => {
                                        s.remote_token.pop();
                                    }
                                }
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(event::KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(event::KeyModifiers::ALT) =>
                            {
                                use crate::app::RemoteModalField;
                                match s.remote_modal_field {
                                    RemoteModalField::Host => {
                                        s.remote_host.push(c);
                                    }
                                    RemoteModalField::Port => {
                                        s.remote_port.push(c);
                                    }
                                    RemoteModalField::Token => {
                                        s.remote_token.push(c);
                                    }
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(handle) = s.remote_server.take() {
                                    handle.cancel.cancel();
                                    s.history.push(ChatMessage::new(
                                        "system",
                                        format!("Remote server stopped (was on :{})", handle.port),
                                    ));
                                } else {
                                    let host = s.remote_host.clone();
                                    let port_str = s.remote_port.clone();
                                    let token = s.remote_token.clone();

                                    let port = port_str.parse::<u16>().unwrap_or(8080);
                                    let cancel = tokio_util::sync::CancellationToken::new();
                                    s.remote_server = Some(crate::app::RemoteHandle {
                                        token: token.clone(),
                                        port,
                                        cancel: cancel.clone(),
                                    });
                                    s.history.push(ChatMessage::new(
                                        "system",
                                        format!("Remote server on :{port} — token: {token}"),
                                    ));
                                    let app = Arc::clone(&app_state);
                                    tokio::spawn(async move {
                                        crate::remote_server::run_server(
                                            app, host, port, token, cancel,
                                        )
                                        .await;
                                    });
                                }
                                s.show_remote_modal = false;
                            }
                            _ => {}
                        }
                        drop(s);
                        continue;
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
                        continue;
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
                                    match item.name {
                                        "Exit the app" => {
                                            exit_flag = true;
                                        }
                                        "Switch model" => {
                                            s.show_model_picker = true;
                                        }
                                        "New session" => {
                                            current_cancel_token.cancel();
                                            current_cancel_token =
                                                tokio_util::sync::CancellationToken::new();
                                            s.cancel_token = Some(current_cancel_token.clone());
                                            crate::app::start_new_session(&mut s);
                                        }
                                        "Resume session" => {
                                            crate::app::resume_latest_session(&mut s);
                                        }
                                        "Copy last reply" => {
                                            crate::app::copy_last_reply(&mut s);
                                        }
                                        "Help" => {
                                            let help = crate::app::build_help_text();
                                            s.history.push(ChatMessage::new("system", help));
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
                            break;
                        }
                        continue;
                    }
                    drop(s);

                    match key.code {
                        KeyCode::BackTab => {
                            let mut s = app_state.lock().await;
                            s.auto_confirm = !s.auto_confirm;
                        }
                        KeyCode::Esc => {
                            let mut s = app_state.lock().await;
                            if s.sel_start.is_some() || s.sel_end.is_some() {
                                s.clear_selection();
                            } else {
                                drop(s);
                                crate::app::handle_escape(&app_state, &mut current_cancel_token)
                                    .await
                            }
                        }
                        KeyCode::Up => {
                            let mut s = app_state.lock().await;
                            if s.active_suggestion_index.is_some() {
                                let filtered_len =
                                    crate::app::get_filtered_cmds_len(&s.input_buffer);
                                if filtered_len > 0 {
                                    let current = s.active_suggestion_index.unwrap_or(0);
                                    s.active_suggestion_index = Some(if current == 0 {
                                        filtered_len - 1
                                    } else {
                                        current - 1
                                    });
                                }
                            } else {
                                s.history_up();
                            }
                        }
                        KeyCode::Down => {
                            let mut s = app_state.lock().await;
                            if s.active_suggestion_index.is_some() {
                                let filtered_len =
                                    crate::app::get_filtered_cmds_len(&s.input_buffer);
                                if filtered_len > 0 {
                                    let current = s.active_suggestion_index.unwrap_or(0);
                                    s.active_suggestion_index =
                                        Some(if current + 1 >= filtered_len {
                                            0
                                        } else {
                                            current + 1
                                        });
                                }
                            } else {
                                s.history_down();
                            }
                        }
                        KeyCode::PageUp => {
                            let mut s = app_state.lock().await;
                            let page = s.page_rows();
                            s.scroll_up(page);
                        }
                        KeyCode::PageDown => {
                            let mut s = app_state.lock().await;
                            let page = s.page_rows();
                            s.scroll_down(page);
                        }
                        KeyCode::Tab => {
                            let mut s = app_state.lock().await;
                            if s.input_buffer.is_empty() && s.history.is_empty() {
                                s.show_model_picker = true;
                            } else if s.active_suggestion_index.is_some() {
                                crate::app::apply_autocomplete(&mut s);
                            } else {
                                s.cycle_suggestion();
                            }
                        }
                        KeyCode::Left => {
                            let mut s = app_state.lock().await;
                            if key.modifiers.contains(event::KeyModifiers::ALT) {
                                s.move_cursor_word_left();
                            } else {
                                s.move_cursor_left();
                            }
                        }
                        KeyCode::Right => {
                            let mut s = app_state.lock().await;
                            if key.modifiers.contains(event::KeyModifiers::ALT) {
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
                        KeyCode::Char('l')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            terminal.clear()?;
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
                                if crate::app::handle_enter(
                                    &app_state,
                                    &client,
                                    &mut current_cancel_token,
                                )
                                .await
                                {
                                    break;
                                }
                            }
                        }
                        KeyCode::Char('v') | KeyCode::Char('V')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL)
                                || key.modifiers.contains(event::KeyModifiers::SUPER)
                                || key.modifiers.contains(event::KeyModifiers::META) =>
                        {
                            if let Some(img_markdown) =
                                crate::clipboard::paste_image_from_clipboard()
                            {
                                let mut s = app_state.lock().await;
                                for c in img_markdown.chars() {
                                    s.insert_char(c);
                                }
                                s.reset_suggestion_cycle();
                            } else if let Some(text) = crate::clipboard::read_text_from_clipboard()
                            {
                                let mut s = app_state.lock().await;
                                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                                for c in normalized.chars() {
                                    s.insert_char(c);
                                }
                                s.reset_suggestion_cycle();
                            }
                        }
                        KeyCode::Char('p')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            let mut s = app_state.lock().await;
                            s.show_command_picker = true;
                            s.command_picker_index = 0;
                            s.command_picker_search.clear();
                        }
                        // Ctrl+R opens the remote config modal.
                        KeyCode::Char('r')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            let mut s = app_state.lock().await;
                            s.show_remote_modal = true;
                            s.remote_modal_field = crate::app::RemoteModalField::Host;
                        }
                        // Ctrl+Y or Cmd/⌘+C copies the current app selection.
                        KeyCode::Char('y') | KeyCode::Char('Y')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            let mut s = app_state.lock().await;
                            if let (Some(a), Some(b)) = (s.sel_start, s.sel_end) {
                                let text =
                                    ui::extract_selection(terminal.current_buffer_mut(), a, b);
                                if !text.is_empty() {
                                    crate::clipboard::copy_to_clipboard(&text);
                                }
                            }
                            s.clear_selection();
                        }
                        KeyCode::Char('c') | KeyCode::Char('C')
                            if key.modifiers.contains(event::KeyModifiers::SUPER)
                                || key.modifiers.contains(event::KeyModifiers::META) =>
                        {
                            let mut s = app_state.lock().await;
                            if let (Some(a), Some(b)) = (s.sel_start, s.sel_end) {
                                let text =
                                    ui::extract_selection(terminal.current_buffer_mut(), a, b);
                                if !text.is_empty() {
                                    crate::clipboard::copy_to_clipboard(&text);
                                }
                            }
                            s.clear_selection();
                        }
                        KeyCode::Char('t')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            let mut s = app_state.lock().await;
                            s.mouse_capture_enabled = !s.mouse_capture_enabled;
                            s.clear_selection();
                            if s.mouse_capture_enabled {
                                let _ = execute!(terminal.backend_mut(), EnableMouseCapture);
                            } else {
                                let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
                            }
                        }
                        KeyCode::Char(c) => {
                            let mut s = app_state.lock().await;
                            let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                            let alt = key.modifiers.contains(event::KeyModifiers::ALT);
                            let cmd = key.modifiers.contains(event::KeyModifiers::SUPER)
                                || key.modifiers.contains(event::KeyModifiers::META);

                            if cmd {
                                // Cmd/⌘ shortcuts are not text — never insert them.
                            } else if alt && c == 'b' {
                                s.move_cursor_word_left();
                            } else if alt && c == 'f' {
                                s.move_cursor_word_right();
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
                            } else if !ctrl && !alt {
                                s.insert_char(c);
                                s.reset_suggestion_cycle();
                            }
                        }
                        KeyCode::Backspace => {
                            let mut s = app_state.lock().await;
                            s.delete_char_backspace();
                            s.reset_suggestion_cycle();
                        }
                        KeyCode::Delete => {
                            let mut s = app_state.lock().await;
                            s.delete_char_delete();
                            s.reset_suggestion_cycle();
                        }
                        _ => {}
                    }
                }
                Event::FocusGained => {
                    terminal_focused = true;
                    needs_redraw = true;
                }
                Event::FocusLost => {
                    terminal_focused = false;
                    needs_redraw = true;
                }
                Event::Mouse(mouse) => {
                    use crossterm::event::{MouseButton, MouseEventKind};
                    let mut s = app_state.lock().await;
                    let modal = s.modal_open();
                    match mouse.kind {
                        MouseEventKind::ScrollUp if !modal => {
                            s.scroll_up(3);
                            needs_redraw = true;
                        }
                        MouseEventKind::ScrollDown if !modal => {
                            s.scroll_down(3);
                            needs_redraw = true;
                        }
                        MouseEventKind::Down(MouseButton::Left) if !modal => {
                            let on_scrollbar = s.scrollbar_height > 0
                                && mouse.column == s.scrollbar_col
                                && mouse.row >= s.scrollbar_top
                                && mouse.row < s.scrollbar_top + s.scrollbar_height;
                            if on_scrollbar {
                                s.dragging_scrollbar = true;
                                s.scrollbar_drag_to(mouse.row);
                            } else {
                                s.sel_start = Some((mouse.column, mouse.row));
                                s.sel_end = Some((mouse.column, mouse.row));
                                s.selecting = true;
                            }
                            needs_redraw = true;
                        }
                        MouseEventKind::Drag(MouseButton::Left) if s.dragging_scrollbar => {
                            s.scrollbar_drag_to(mouse.row);
                            needs_redraw = true;
                        }
                        MouseEventKind::Drag(MouseButton::Left) if s.selecting => {
                            s.sel_end = Some((mouse.column, mouse.row));
                            needs_redraw = true;
                        }
                        MouseEventKind::Up(MouseButton::Left) if s.dragging_scrollbar => {
                            s.dragging_scrollbar = false;
                            needs_redraw = true;
                        }
                        MouseEventKind::Up(MouseButton::Left) if s.selecting => {
                            s.sel_end = Some((mouse.column, mouse.row));
                            s.selecting = false;
                            if let (Some(a), Some(b)) = (s.sel_start, s.sel_end) {
                                if a != b {
                                    // Dragged: copy on release, like selecting on a web page.
                                    let text =
                                        ui::extract_selection(terminal.current_buffer_mut(), a, b);
                                    if !text.is_empty() {
                                        crate::clipboard::copy_to_clipboard(&text);
                                    }
                                } else {
                                    // A plain click: toggle a thought if one sits on this
                                    // row, otherwise just clear any existing selection.
                                    s.clear_selection();
                                    if let Some(&(_, idx)) =
                                        s.thought_toggle_rows.iter().find(|(row, _)| *row == b.1)
                                    {
                                        s.toggle_thought(idx);
                                    }
                                }
                            }
                            needs_redraw = true;
                        }
                        _ => {}
                    }
                }
                Event::Paste(text) => {
                    let mut s = app_state.lock().await;
                    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                    for c in normalized.chars() {
                        s.insert_char(c);
                    }
                    s.reset_suggestion_cycle();
                    needs_redraw = true;
                }
                Event::Resize(_, _) => {
                    needs_redraw = true;
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
        SetCursorStyle::DefaultUserShape
    )?;
    terminal.show_cursor()?;

    Ok(())
}
