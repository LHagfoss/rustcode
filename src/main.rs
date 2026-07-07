mod app;
mod config;
mod network;
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
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
        SetCursorStyle::BlinkingBlock
    )?;

    // Kitty keyboard protocol (where supported) so Shift+Enter is
    // distinguishable from plain Enter for multiline input.
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

    // Initialize state with an empty history at startup so welcome splash is shown
    let mut app_state_struct = AppState::new();
    app_state_struct.history = Vec::new();
    let app_state = Arc::new(Mutex::new(app_state_struct));

    // Connect timeout only: streamed responses can legitimately run long,
    // but a dead server should fail fast instead of spinning forever.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut current_cancel_token = tokio_util::sync::CancellationToken::new();

    loop {
        // Draw with state held briefly for consistency.
        {
            let mut guard = app_state.lock().await;
            terminal.draw(|f| ui::render(f, &mut guard))?;
        }

        if event::poll(std::time::Duration::from_millis(50))? {
            let ev = event::read()?;
            match ev {
                Event::Key(key) => {
                    if key.kind == event::KeyEventKind::Release {
                        continue;
                    }
                    // Ctrl+C → hard exit.
                    if key.modifiers.contains(event::KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        break;
                    }

                    let mut s = app_state.lock().await;
                    if s.show_model_picker {
                        match key.code {
                            KeyCode::Esc => {
                                s.show_model_picker = false;
                            }
                            KeyCode::Up => {
                                let len = get_picker_items_count(&s);
                                if len > 0 {
                                    s.model_picker_index = if s.model_picker_index == 0 {
                                        len - 1
                                    } else {
                                        s.model_picker_index - 1
                                    };
                                }
                            }
                            KeyCode::Down => {
                                let len = get_picker_items_count(&s);
                                if len > 0 {
                                    s.model_picker_index = if s.model_picker_index + 1 >= len {
                                        0
                                    } else {
                                        s.model_picker_index + 1
                                    };
                                }
                            }
                            KeyCode::Enter => {
                                select_picker_model(&mut s);
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
                                    if item.name == "Exit the app" {
                                        exit_flag = true;
                                    } else if item.name == "Switch model" {
                                        s.show_model_picker = true;
                                        s.show_command_picker = false;
                                    } else if item.name == "New session" {
                                        current_cancel_token.cancel();
                                        current_cancel_token =
                                            tokio_util::sync::CancellationToken::new();
                                        s.history.clear();
                                        s.pending_queue.clear();
                                        s.current_response.clear();
                                        s.current_token_usage = None;
                                        s.response_time = None;
                                        s.status = AppStatus::Idle;
                                        s.input_buffer.clear();
                                        s.cursor_position = 0;
                                        s.show_command_picker = false;
                                        crate::config::save_history(&s.history);
                                    } else {
                                        s.history.push(ChatMessage::new(
                                            "system",
                                            format!("Action executed: {}", item.name),
                                        ));
                                        s.show_command_picker = false;
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
                        KeyCode::Esc => handle_escape(&app_state, &mut current_cancel_token).await,
                        KeyCode::Up => {
                            let mut s = app_state.lock().await;
                            if s.active_suggestion_index.is_some() {
                                let filtered_len = get_filtered_cmds_len(&s.input_buffer);
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
                                let filtered_len = get_filtered_cmds_len(&s.input_buffer);
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
                                apply_autocomplete(&mut s);
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
                                if handle_enter(&app_state, &client, &mut current_cancel_token)
                                    .await
                                {
                                    break;
                                }
                            }
                        }
                        KeyCode::Char('v')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            if let Some(img_markdown) = paste_image_from_clipboard() {
                                let mut s = app_state.lock().await;
                                for c in img_markdown.chars() {
                                    s.insert_char(c);
                                }
                                s.reset_suggestion_cycle();
                            } else if let Some(text) = read_text_from_clipboard() {
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
                            // macOS Option+Left/Right → b/f with ALT modifier.
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
                Event::Paste(text) => {
                    let mut s = app_state.lock().await;
                    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                    for c in normalized.chars() {
                        s.insert_char(c);
                    }
                    s.reset_suggestion_cycle();
                }
                Event::Mouse(mouse) => match mouse.kind {
                    event::MouseEventKind::ScrollUp => {
                        app_state.lock().await.scroll_up(3);
                    }
                    event::MouseEventKind::ScrollDown => {
                        app_state.lock().await.scroll_down(3);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    // Restore terminal state on exit.
    disable_raw_mode()?;
    if keyboard_enhanced {
        execute!(terminal.backend_mut(), event::PopKeyboardEnhancementFlags)?;
    }
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        SetCursorStyle::DefaultUserShape
    )?;
    terminal.show_cursor()?;

    Ok(())
}

/// Escape: cancel active stream / dequeue next queued prompt, and clear input field.
async fn handle_escape(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut tokio_util::sync::CancellationToken,
) {
    let mut s = state.lock().await;
    s.reset_suggestion_cycle();
    s.input_buffer.clear();
    s.cursor_position = 0;

    cancel_token.cancel();
    *cancel_token = tokio_util::sync::CancellationToken::new();

    if s.status != AppStatus::Streaming && !s.pending_queue.is_empty() {
        s.pending_queue.remove(0);
        if s.pending_queue.is_empty() {
            s.status = AppStatus::Idle;
        }
    }
}

/// Enter: dispatch slash-commands or queue the user message for streaming.
async fn handle_enter(
    state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel_token: &mut tokio_util::sync::CancellationToken,
) -> bool {
    let mut s = state.lock().await;
    s.reset_suggestion_cycle();

    if s.active_suggestion_index.is_some() {
        apply_autocomplete(&mut s);
        return false;
    }

    let raw_input = s.input_buffer.trim().to_string();

    if raw_input.is_empty() {
        return false;
    }

    if raw_input.starts_with('/') {
        let tokens: Vec<&str> = raw_input.split_whitespace().collect();
        if tokens.is_empty() {
            s.input_buffer.clear();
            s.cursor_position = 0;
            return false;
        }

        let cmd = tokens[0];
        let mut should_exit = false;

        match cmd {
            "/clear" | "/new" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                s.history.clear();
                s.current_response.clear();
                s.pending_queue.clear();
                s.current_token_usage = None;
                s.response_time = None;
                s.status = AppStatus::Idle;
                crate::config::save_history(&s.history);
            }
            "/cancel" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
            }
            "/help" => {
                let mut help = String::from("Available commands:");
                for cmd_info in crate::app::suggestion::COMMANDS {
                    help.push_str(&format!("\n  {:<10} - {}", cmd_info.name, cmd_info.desc));
                }
                help.push_str("\n\nUsage details:");
                help.push_str("\n  /model <name> - Switch to profile name, or set model override");
                help.push_str("\n  /provider <name> <url> <model> - Create/update profile");
                help.push_str("\n  /provider <url> <model> - Update active profile");
                help.push_str("\n  /ollama <url> <model> - Set 'ollama' profile URL and model");
                help.push_str("\n  /ollama list [url] - Fetch and list available Ollama models");
                s.history.push(ChatMessage::new("system", help));
            }
            "/exit" | "/quit" => {
                should_exit = true;
            }
            "/copy" => {
                let last_reply = s
                    .history
                    .iter()
                    .rev()
                    .find(|m| m.role == "assistant")
                    .map(|m| m.content.clone());
                match last_reply {
                    Some(content) => {
                        let msg = if copy_to_clipboard(&content) {
                            "Copied last assistant reply to clipboard."
                        } else {
                            "Failed to copy to clipboard."
                        };
                        s.history.push(ChatMessage::new("system", msg));
                    }
                    None => {
                        s.history
                            .push(ChatMessage::new("system", "No assistant reply to copy."));
                    }
                }
            }
            "/resume" | "/history" => {
                let saved = crate::config::load_history();
                if saved.is_empty() {
                    s.history
                        .push(ChatMessage::new("system", "No saved chat history found."));
                } else {
                    s.history = saved;
                    s.history.push(ChatMessage::new(
                        "system",
                        "Resumed chat history from disk.",
                    ));
                }
            }
            "/model" | "/models" => {
                if tokens.len() < 2 {
                    s.show_model_picker = true;
                    s.model_picker_index = 0;
                    s.model_picker_search.clear();
                } else {
                    let name = tokens[1].to_string();
                    if let Some(profile) = s.config.models.iter().find(|m| m.name == name) {
                        let url = profile.url.clone();
                        let model = profile.model.clone();
                        s.api_base_url = url;
                        s.model_name = model;
                        s.config.default = name.clone();
                        crate::config::save_entire_config(&s.config);
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("Switched to model profile '{}'", name),
                        ));
                    } else {
                        // Direct override of model name for active profile
                        s.model_name = name.clone();
                        let default_name = s.config.default.clone();
                        if let Some(profile) =
                            s.config.models.iter_mut().find(|m| m.name == default_name)
                        {
                            profile.model = name.clone();
                        }
                        crate::config::save_entire_config(&s.config);
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("Switched active model to '{}'", name),
                        ));
                    }
                }
            }
            "/provider" => {
                if tokens.len() >= 4 {
                    let name = tokens[1].to_string();
                    let url = tokens[2].to_string();
                    let model = tokens[3].to_string();

                    s.api_base_url = url.clone();
                    s.model_name = model.clone();

                    // Update or insert profile
                    if let Some(profile) = s.config.models.iter_mut().find(|m| m.name == name) {
                        profile.url = url;
                        profile.model = model;
                    } else {
                        s.config.models.push(crate::config::ModelProfile {
                            name: name.clone(),
                            url,
                            model,
                        });
                    }
                    s.config.default = name.clone();
                    crate::config::save_entire_config(&s.config);
                    s.history.push(ChatMessage::new(
                        "system",
                        format!("Created/updated profile '{}' and set as default", name),
                    ));
                } else if tokens.len() == 3 {
                    let url = tokens[1].to_string();
                    let model = tokens[2].to_string();
                    s.api_base_url = url.clone();
                    s.model_name = model.clone();

                    let default_name = s.config.default.clone();
                    if let Some(profile) =
                        s.config.models.iter_mut().find(|m| m.name == default_name)
                    {
                        profile.url = url;
                        profile.model = model;
                    }
                    crate::config::save_entire_config(&s.config);

                    let active_default = s.config.default.clone();
                    let active_url = s.api_base_url.clone();
                    let active_model = s.model_name.clone();
                    s.history.push(ChatMessage::new(
                        "system",
                        format!(
                            "Updated active profile '{}' with URL '{}' and model '{}'",
                            active_default, active_url, active_model
                        ),
                    ));
                } else {
                    s.history.push(ChatMessage::new("system", "Usage:\n  /provider <name> <url> <model> - Create/update profile\n  /provider <url> <model> - Update active profile"));
                }
            }
            "/ollama" => {
                if tokens.len() >= 2 && tokens[1] == "list" {
                    let ollama_url = if tokens.len() >= 3 {
                        tokens[2]
                    } else {
                        &s.api_base_url
                    };

                    let tags_url = if ollama_url.ends_with("/v1/chat/completions") {
                        ollama_url.replace("/v1/chat/completions", "/api/tags")
                    } else if ollama_url.ends_with("/v1/") {
                        ollama_url.replace("/v1/", "/api/tags")
                    } else if ollama_url.ends_with('/') {
                        format!("{}api/tags", ollama_url)
                    } else {
                        format!("{}/api/tags", ollama_url)
                    };

                    s.history.push(ChatMessage::new(
                        "system",
                        format!("Fetching Ollama models from '{}'...", tags_url),
                    ));

                    let client_clone = client.clone();
                    let state_clone = Arc::clone(state);
                    tokio::spawn(async move {
                        match client_clone.get(&tags_url).send().await {
                            Ok(res) => {
                                if res.status().is_success() {
                                    #[derive(serde::Deserialize)]
                                    struct OllamaModel {
                                        name: String,
                                    }
                                    #[derive(serde::Deserialize)]
                                    struct OllamaTags {
                                        models: Vec<OllamaModel>,
                                    }

                                    match res.json::<OllamaTags>().await {
                                        Ok(tags) => {
                                            let names: Vec<String> =
                                                tags.models.into_iter().map(|m| m.name).collect();
                                            let mut s = state_clone.lock().await;
                                            if names.is_empty() {
                                                s.history.push(ChatMessage::new(
                                                    "system",
                                                    "Ollama returned no models.",
                                                ));
                                            } else {
                                                s.history.push(ChatMessage::new(
                                                    "system",
                                                    format!(
                                                        "Available Ollama models:\n  {}",
                                                        names.join("\n  ")
                                                    ),
                                                ));
                                            }
                                        }
                                        Err(e) => {
                                            let mut s = state_clone.lock().await;
                                            s.history.push(ChatMessage::new(
                                                "system",
                                                format!(
                                                    "Failed to parse Ollama tags response: {}",
                                                    e
                                                ),
                                            ));
                                        }
                                    }
                                } else {
                                    let mut s = state_clone.lock().await;
                                    s.history.push(ChatMessage::new(
                                        "system",
                                        format!("Ollama returned status code: {}", res.status()),
                                    ));
                                }
                            }
                            Err(e) => {
                                let mut s = state_clone.lock().await;
                                s.history.push(ChatMessage::new(
                                    "system",
                                    format!("Failed to fetch Ollama models: {}", e),
                                ));
                            }
                        }
                    });
                } else if tokens.len() == 3 {
                    let url = tokens[1].to_string();
                    let model = tokens[2].to_string();
                    s.api_base_url = url.clone();
                    s.model_name = model.clone();

                    // Update or insert "ollama" profile
                    if let Some(profile) = s.config.models.iter_mut().find(|m| m.name == "ollama") {
                        profile.url = url;
                        profile.model = model;
                    } else {
                        s.config.models.push(crate::config::ModelProfile {
                            name: "ollama".to_string(),
                            url,
                            model,
                        });
                    }
                    s.config.default = "ollama".to_string();
                    crate::config::save_entire_config(&s.config);
                    s.history.push(ChatMessage::new(
                        "system",
                        "Switched to profile 'ollama' and updated its URL and model",
                    ));
                } else {
                    s.history.push(ChatMessage::new("system", "Usage:\n  /ollama list [url] - List available models\n  /ollama <url> <model> - Set 'ollama' profile URL and model"));
                }
            }
            _ => {
                s.history.push(ChatMessage::new(
                    "system",
                    format!("Unknown command: {}", cmd),
                ));
            }
        }

        s.input_buffer.clear();
        s.cursor_position = 0;
        return should_exit;
    }

    // Append to queue and launch streaming if idle.
    s.pending_queue.push(raw_input);
    s.input_buffer.clear();
    s.cursor_position = 0;

    if s.status == AppStatus::Idle {
        s.status = AppStatus::Queued;
        let client_clone = client.clone();
        let state_clone = Arc::clone(state);
        let token_clone = cancel_token.clone();
        drop(s);

        tokio::spawn(async move {
            network::process_queue_orchestrator(client_clone, state_clone, token_clone).await;
        });
    }
    false
}

/// Helper to save clipboard image on macOS and return its markdown link.
fn paste_image_from_clipboard() -> Option<String> {
    let attachments_dir = crate::config::get_config_dir()?.join("attachments");
    let _ = std::fs::create_dir_all(&attachments_dir);

    let filename = format!("clip_{}.png", chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let file_path = attachments_dir.join(&filename);
    let file_path_str = file_path.to_string_lossy().to_string();

    let script = format!(
        "write (the clipboard as «class PNGf») to (open for access \"{}\" with write permission)",
        file_path_str
    );

    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .ok()?;

    if output.status.success() {
        Some(format!("![image](file://{})", file_path_str))
    } else {
        None
    }
}

/// Helper to read clipboard text on macOS.
fn read_text_from_clipboard() -> Option<String> {
    let output = std::process::Command::new("pbpaste").output().ok()?;
    if output.status.success() {
        let text = std::str::from_utf8(&output.stdout).ok()?.to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// Helper to get length of filtered command autocomplete suggestions.
fn get_filtered_cmds_len(input_buffer: &str) -> usize {
    if input_buffer.starts_with('/') && !input_buffer.contains(' ') {
        crate::app::suggestion::COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(input_buffer))
            .count()
    } else {
        0
    }
}

/// Helper to apply selected autocomplete suggestion to input buffer.
fn apply_autocomplete(s: &mut AppState) {
    if s.input_buffer.starts_with('/') && !s.input_buffer.contains(' ') {
        let filtered_cmds: Vec<&crate::app::suggestion::CommandInfo> =
            crate::app::suggestion::COMMANDS
                .iter()
                .filter(|c| c.name.starts_with(&s.input_buffer))
                .collect();

        if let Some(idx) = s.active_suggestion_index {
            if idx < filtered_cmds.len() {
                s.input_buffer = format!("{} ", filtered_cmds[idx].name);
                s.cursor_position = s.input_buffer.len();
                s.active_suggestion_index = None;
            }
        }
    }
}

/// Helper to write text to the macOS clipboard.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    let child = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn get_picker_items_count(s: &AppState) -> usize {
    crate::ui::get_filtered_picker_items(s).len()
}

/// Apply the highlighted model picker row: switch to that profile and persist.
fn select_picker_model(s: &mut AppState) {
    let items = crate::ui::get_filtered_picker_items(s);
    if items.is_empty() {
        return;
    }
    let selected_idx = s.model_picker_index.min(items.len() - 1);
    let item = &items[selected_idx];

    if let Some(profile) = s.config.models.iter().find(|m| m.name == item.name) {
        s.api_base_url = profile.url.clone();
        s.model_name = profile.model.clone();
        s.config.default = item.name.clone();
        s.config.latest_model = Some(profile.model.clone());
        s.config.latest_url = Some(profile.url.clone());
        crate::config::save_entire_config(&s.config);
        s.history.push(ChatMessage::new(
            "system",
            format!("Switched to model profile '{}'", item.name),
        ));
    }
}
