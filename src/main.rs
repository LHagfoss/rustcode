mod app;
mod clipboard;
mod config;
mod network;
mod raw_cli;
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
        SetCursorStyle::BlinkingBlock
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

    let mut app_state_struct = AppState::new();
    if let Some(ref m_name) = model_override {
        if let Some(profile) = app_state_struct.config.models.iter().find(|m| m.name == *m_name) {
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

    let mut needs_redraw = true;
    let mut last_draw = std::time::Instant::now();
    let mut was_responding = false;
    let mut terminal_focused = true;

    loop {
        let response_active = app_state.lock().await.status != AppStatus::Idle;

        if was_responding && !response_active && !terminal_focused {
            use crossterm::style::Print;
            let _ = execute!(
                terminal.backend_mut(),
                Print("\x1b]9;rustcode · response finished\x07\x07")
            );
        }
        was_responding = response_active;
        if needs_redraw
            || response_active
            || last_draw.elapsed() >= std::time::Duration::from_millis(250)
        {
            let mut guard = app_state.lock().await;
            terminal.draw(|f| ui::render(f, &mut guard))?;
            drop(guard);
            last_draw = std::time::Instant::now();
            needs_redraw = false;
        }

        if event::poll(std::time::Duration::from_millis(50))? {
            needs_redraw = true;
            let ev = event::read()?;
            match ev {
                Event::Key(key) => {
                    if key.kind == event::KeyEventKind::Release {
                        continue;
                    }

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
                                    if let Some(tx) = s.tool_confirmation_response.take() {
                                        let _ = tx.send(true);
                                    }
                                    s.pending_tool_confirmation = None;
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                    let mut s = app_state.lock().await;
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
                                            crate::app::start_new_session(&mut s);
                                        }
                                        "Resume session" => {
                                            crate::app::resume_history(&mut s);
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
                        KeyCode::Esc => crate::app::handle_escape(&app_state, &mut current_cancel_token).await,
                        KeyCode::Up => {
                            let mut s = app_state.lock().await;
                            if s.active_suggestion_index.is_some() {
                                let filtered_len = crate::app::get_filtered_cmds_len(&s.input_buffer);
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
                                let filtered_len = crate::app::get_filtered_cmds_len(&s.input_buffer);
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
                            s.scroll_up(5);
                        }
                        KeyCode::PageDown => {
                            let mut s = app_state.lock().await;
                            s.scroll_down(5);
                        }
                        KeyCode::Tab => {
                            let mut s = app_state.lock().await;
                            if s.active_suggestion_index.is_some() {
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
                                if crate::app::handle_enter(&app_state, &client, &mut current_cancel_token)
                                    .await
                                {
                                    break;
                                }
                            }
                        }
                        KeyCode::Char('v')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
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
                        KeyCode::Char(c) => {
                            let mut s = app_state.lock().await;
                            let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                            let alt = key.modifiers.contains(event::KeyModifiers::ALT);

                            if alt && c == 'b' {
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
                }
                Event::FocusLost => {
                    terminal_focused = false;
                }
                _ => {}
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
