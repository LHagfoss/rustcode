#[macro_use]
mod logger;
mod app;
mod cli;
mod clipboard;
mod config;
mod context;
mod mcp;
mod network;
mod notifications;
mod raw_cli;
mod skills;
mod symbols;
mod tools;
mod ui;
mod discord_rpc;

use crate::app::{AppState, AppStatus, ChatMessage};
use clap::Parser;
use crossterm::{
    cursor::SetCursorStyle,
    event::{self, DisableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16); // 60Hz for smooth scrolling

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli_args = cli::Cli::parse();
    let model_override = cli_args.model.clone();

    if let Some(cli::Commands::Sync { command }) = cli_args.command {
        match command {
            Some(cli::SyncCommands::Pull) => {
                println!("📥 [sync] Pulling latest config and skills from remote origin/main...");
                if let Err(e) = config::sync_config_pull() {
                    eprintln!("Sync pull failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            Some(cli::SyncCommands::Push) => {
                println!("💾 [sync] Staging config.toml, skills/, and session logs...");
                if let Err(e) = config::sync_config_push() {
                    eprintln!("Sync push failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            Some(cli::SyncCommands::Init { remote_url }) => {
                if let Err(e) = config::init_sync_repo(&remote_url) {
                    eprintln!("Error initializing sync repo: {e}");
                    std::process::exit(1);
                }
                println!("Sync repository setup complete! You can now run `rustcode sync` anytime.");
            }
            None => {
                // Default behavior for `rustcode sync` (pull then push)
                println!("📥 [sync] Pulling latest config and skills from remote origin/main...");
                if let Err(e) = config::sync_config_pull() {
                    eprintln!("Sync failed during pull: {e}");
                    std::process::exit(1);
                }
                println!("💾 [sync] Staging config.toml, skills/, and session logs...");
                if let Err(e) = config::sync_config_push() {
                    eprintln!("Sync failed during push: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
        }
        return Ok(());
    }

    if let Some(prompt) = cli_args.prompt {
        raw_cli::run_raw_cli(&prompt, model_override.as_deref()).await?;
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Use SGR Mouse Mode (ESC[?1006h) — reliable on macOS Terminal.app and iTerm2
    use std::io::Write;
    write!(stdout, "\x1b[?1006h").ok();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableFocusChange,
        SetCursorStyle::BlinkingBlock,
        crossterm::style::Print("\x1b]0;rustcode · new session\x07")
    )?;

    let _ = execute!(
        stdout,
        event::PushKeyboardEnhancementFlags(
            event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        )
    );

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    crate::config::archive_live_history();

    let mut app_state_struct = AppState::new();
    if cli_args.resume || cli_args.continue_session {
        crate::app::resume_latest_session(&mut app_state_struct);
    }
    if let Some(ref m_name) = model_override
        && let Some(profile) = app_state_struct
            .config
            .models
            .iter()
            .find(|m| m.name == *m_name)
        {
            app_state_struct.api_base_url = profile.url.clone();
            app_state_struct.model_name = profile.model.clone();
        }
    let app_state = Arc::new(Mutex::new(app_state_struct));

    // Initialize Discord RPC if enabled
    {
        let mut s = app_state.lock().await;
        if s.config.discord_rpc_enabled {
            s.discord_rpc.connect();
            let model_name = s.model_name.clone();
            s.discord_rpc.set_activity("Idle", &format!("Using model: {}", model_name));
        }
    }

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut current_cancel_token = tokio_util::sync::CancellationToken::new();

    // Register the background task wakeup callback
    let state_cb = Arc::clone(&app_state);
    let client_cb = client.clone();
    let token_cb = current_cancel_token.clone();
    let handle = tokio::runtime::Handle::current();
    crate::tools::register_wakeup_callback(move |session_id, task_id, output| {
        let state_clone = Arc::clone(&state_cb);
        let client_clone = client_cb.clone();
        let token_clone = token_cb.clone();
        let handle_clone = handle.clone();
        handle_clone.spawn(async move {
            let mut s = state_clone.lock().await;
            if s.active_session_id == session_id {
                // Background output can be huge (long-running servers dump MBs of
                // logs). Head+tail truncate it like any other tool result so it
                // doesn't bloat context and the scroll buffer.
                let body = crate::network::truncate_tool_output("background_task", output);
                s.history.push(ChatMessage::new(
                    "tool",
                    format!("background_task: Task {task_id} completed. Output:\n{body}"),
                ));
                crate::config::save_session_history(&session_id, &s.history);
                // Only drive a fresh model turn when the agent is actively working
                // toward a goal. After completion / in plain chat, a late task
                // finish must NOT wake the model: it would run against the whole
                // history with no task and tends to regurgitate its system prompt.
                // The result is still recorded above for the user to act on.
                if s.continuous_mode {
                    s.pending_queue.push(format!("__task_wakeup__:{task_id}"));
                    // Only spawn if no orchestrator is alive; otherwise the
                    // running one drains the queue. Gating on status==Idle raced
                    // an exiting turn and spawned a second concurrent orchestrator.
                    if !s.orchestrator_running {
                        s.orchestrator_running = true;
                        s.status = AppStatus::Queued;
                        if s.config.discord_rpc_enabled {
                            let model_name = s.model_name.clone();
                            s.discord_rpc.set_activity("Queued", &format!("Using model: {}", model_name));
                        }
                        drop(s);
                        crate::network::process_queue_orchestrator(
                            client_clone,
                            state_clone,
                            token_clone,
                        )
                        .await;
                    }
                }
            } else {
                let mut history = crate::config::load_session_history_direct(&session_id);
                history.push(ChatMessage::new(
                    "tool",
                    format!("background_task: Task {task_id} completed. Output:\n{output}"),
                ));
                crate::config::save_session_history(&session_id, &history);
            }
        });
    });

    // Spawn startup initialization of enabled MCP servers
    {
        let s = app_state.lock().await;
        for srv in &s.config.mcp_servers {
            if srv.enabled {
                let name = srv.name.clone();
                tokio::spawn(async move {
                    let _ = crate::mcp::start_server_by_name(&name).await;
                });
            }
        }
    }

    crate::app::spawn_context_window_detection(Arc::clone(&app_state), client.clone());

    {
        let state_quota = Arc::clone(&app_state);
        let client_quota = client.clone();
        tokio::spawn(async move {
            crate::network::fetch_model_quota(&client_quota, &state_quota).await;
        });
    }

    let mut needs_redraw = true;
    let mut last_draw = std::time::Instant::now();
    let mut was_responding = false;
    let mut terminal_focused = true;
    // Scroll coalescing: batch rapid scroll events
    let mut scroll_coalesce: i32 = 0;
    const SCROLL_COALESCE_WINDOW: Duration = Duration::from_millis(16);
    let mut last_scroll_time = Instant::now();

    loop {
        let response_active = app_state.lock().await.status != AppStatus::Idle;

        {
            let mut s = app_state.lock().await;
            if !s.orchestrator_running && !s.pending_queue.is_empty() {
                s.orchestrator_running = true;
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
            || (response_active && last_draw.elapsed() >= std::time::Duration::from_millis(16))
            || last_draw.elapsed() >= std::time::Duration::from_millis(100);

        if should_draw {
            let mut guard = app_state.lock().await;

            // Update terminal title based on session state
            let title_display = if guard.history.is_empty() {
                "rustcode".to_string()
            } else {
                // Check for custom title first
                let session_id = guard.active_session_id.clone();
                let custom_title = crate::config::load_session_title(&session_id).or_else(|| {
                    guard
                        .history
                        .iter()
                        .find(|m| m.role == "user" && !m.content.starts_with('/'))
                        .map(|m| m.content.lines().next().unwrap_or("").trim().to_string())
                });
                match custom_title {
                    Some(title) if !title.is_empty() && !title.starts_with('/') => {
                        let display_title = title.replace('|', "\\|").replace('\x07', "");
                        format!("rustcode · {}", display_title)
                    }
                    _ => "rustcode".to_string(),
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

        if event::poll(EVENT_POLL_INTERVAL)? {
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
                                    if let Some(tx) = s.tool_confirmation_response.take() {
                                        let _ = tx.send(false);
                                    }
                                    s.pending_tool_confirmation = None;
                                }
                                KeyCode::Tab => {
                                    let mut s = app_state.lock().await;
                                    s.auto_confirm = !s.auto_confirm;
                                }
                                KeyCode::Up => {
                                    let mut s = app_state.lock().await;
                                    s.modal_scroll_row = s.modal_scroll_row.saturating_sub(1);
                                }
                                KeyCode::Down => {
                                    let mut s = app_state.lock().await;
                                    let total_lines = s
                                        .pending_tool_confirmation
                                        .as_ref()
                                        .and_then(|c| c.first())
                                        .map(|c| c.content_preview.lines().count())
                                        .unwrap_or(0);
                                    if total_lines > 0 && (s.modal_scroll_row as usize) + 1 < total_lines {
                                        s.modal_scroll_row += 1;
                                    }
                                }
                                _ => {}
                            }
                            continue;
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
                                // Freeform "write your own answer" text entry.
                                match key.code {
                                    // Cmd/Ctrl+V — paste clipboard text into the slot.
                                    // (Bracketed-paste terminals arrive via Event::Paste
                                    // instead; this covers terminals that forward the key.)
                                    KeyCode::Char('v') | KeyCode::Char('V')
                                        if key.modifiers.contains(event::KeyModifiers::CONTROL)
                                            || key.modifiers.contains(event::KeyModifiers::SUPER)
                                            || key.modifiers.contains(event::KeyModifiers::META) =>
                                    {
                                        if let Some(text) =
                                            crate::clipboard::read_text_from_clipboard()
                                        {
                                            let normalized =
                                                text.replace("\r\n", "\n").replace('\r', "\n");
                                            let mut s = app_state.lock().await;
                                            if let Some(q) = s.pending_question.as_mut()
                                                && let Some(buf) = q.custom_input.as_mut()
                                            {
                                                for c in normalized.chars() {
                                                    if c != '\n' {
                                                        buf.push(c);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    KeyCode::Char(c) => {
                                        let mut s = app_state.lock().await;
                                        if let Some(q) = s.pending_question.as_mut()
                                            && let Some(buf) = q.custom_input.as_mut()
                                        {
                                            buf.push(c);
                                        }
                                    }
                                    KeyCode::Backspace => {
                                        let mut s = app_state.lock().await;
                                        if let Some(q) = s.pending_question.as_mut()
                                            && let Some(buf) = q.custom_input.as_mut()
                                        {
                                            buf.pop();
                                        }
                                    }
                                    KeyCode::Enter => {
                                        let mut s = app_state.lock().await;
                                        let answer = s
                                            .pending_question
                                            .as_ref()
                                            .and_then(|q| q.custom_input.clone())
                                            .unwrap_or_default()
                                            .trim()
                                            .to_string();
                                        if !answer.is_empty() {
                                            if let Some(tx) = s.question_response.take() {
                                                let _ = tx.send(answer);
                                            }
                                            s.pending_question = None;
                                        }
                                    }
                                    KeyCode::Esc => {
                                        // Back out to the option list, keep the modal open.
                                        let mut s = app_state.lock().await;
                                        if let Some(q) = s.pending_question.as_mut() {
                                            q.custom_input = None;
                                        }
                                    }
                                    _ => {}
                                }
                                continue;
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
                                        // options.len() is the "write your own answer" slot.
                                        let last = q.options.len();
                                        q.selected = (q.selected + 1).min(last);
                                    }
                                }
                                KeyCode::Char(' ') => {
                                    let mut s = app_state.lock().await;
                                    if let Some(q) = s.pending_question.as_mut() {
                                        if q.selected == q.options.len() {
                                            q.custom_input = Some(String::new());
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
                                            let answer = q.options[idx].clone();
                                            if let Some(tx) = s.question_response.take() {
                                                let _ = tx.send(answer);
                                            }
                                            s.pending_question = None;
                                        }
                                    }
                                }
                                KeyCode::Enter => {
                                    let mut s = app_state.lock().await;
                                    // Selecting the "write your own answer" slot enters text mode.
                                    let is_custom_slot = s
                                        .pending_question
                                        .as_ref()
                                        .map(|q| q.selected == q.options.len())
                                        .unwrap_or(false);
                                    if is_custom_slot {
                                        if let Some(q) = s.pending_question.as_mut() {
                                            q.custom_input = Some(String::new());
                                        }
                                    } else if let Some(q) = s.pending_question.as_ref() {
                                        let answer = if q.is_multi_select {
                                            let picked: Vec<String> = q
                                                .options
                                                .iter()
                                                .zip(q.chosen.iter())
                                                .filter(|(_, c)| **c)
                                                .map(|(o, _)| o.clone())
                                                .collect();
                                            if picked.is_empty() {
                                                q.options.get(q.selected).cloned().unwrap_or_default()
                                            } else {
                                                picked.join(", ")
                                            }
                                        } else {
                                            q.options.get(q.selected).cloned().unwrap_or_default()
                                        };
                                        if let Some(tx) = s.question_response.take() {
                                            let _ = tx.send(answer);
                                        }
                                        s.pending_question = None;
                                    }
                                }
                                KeyCode::Esc => {
                                    // Dismiss question and cancel active generation stream
                                    current_cancel_token.cancel();
                                    current_cancel_token = tokio_util::sync::CancellationToken::new();
                                    let mut s = app_state.lock().await;
                                    s.question_response = None;
                                    s.pending_question = None;
                                }
                                _ => {}
                            }
                            continue;
                        }
                    }


                    let mut s = app_state.lock().await;
                    if s.show_history_picker {
                        // Ctrl+D triggers delete confirmation overlay
                        if key.modifiers.contains(event::KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('d')
                        {
                            let idx = s.history_picker_index.min(
                                s.history_picker_sessions.len().saturating_sub(1),
                            );
                            s.pending_delete_session_idx = Some(idx);
                            drop(s);
                            continue;
                        }

                        // Confirmation overlay for delete
                        if let Some(del_idx) = s.pending_delete_session_idx {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Enter => {
                                    if let Some(meta) =
                                        s.history_picker_sessions.get(del_idx).cloned()
                                    {
                                        crate::config::delete_session_file(&meta.path);
                                    }
                                    s.history_picker_sessions.remove(del_idx);
                                    if !s.history_picker_sessions.is_empty()
                                        && del_idx >= s.history_picker_sessions.len().saturating_sub(1)
                                    {
                                        s.history_picker_index = (del_idx as i64 - 1).max(0) as usize;
                                    }
                                    s.pending_delete_session_idx = None;
                                    if s.history_picker_sessions.is_empty() {
                                        s.show_history_picker = false;
                                    }
                                }
                                KeyCode::Esc | KeyCode::Char('n') => {
                                    s.pending_delete_session_idx = None;
                                }
                                _ => {}
                            }
                            drop(s);
                            continue;
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
                                    let prev = if edit_state.active_field == 0 { 2 } else { edit_state.active_field - 1 };
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
                                            && idx < s.config.mcp_servers.len() {
                                                let old_name =
                                                    s.config.mcp_servers[idx].name.clone();
                                                s.config.mcp_servers[idx] = new_srv;
                                                if old_name != name {
                                                    crate::mcp::shutdown_server(&old_name).await;
                                                }
                                            }

                                        crate::config::save_entire_config(&s.config);

                                        let name_clone = name.clone();
                                        tokio::spawn(async move {
                                            let _ =
                                                crate::mcp::start_server_by_name(&name_clone).await;
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
                                                    crate::mcp::start_server_by_name(&name_clone)
                                                        .await;
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
                                    match item.shortcut {
                                        "ctrl+c" => {
                                            exit_flag = true;
                                        }
                                        "/model" => {
                                            s.show_model_picker = true;
                                        }
                                        "/new" => {
                                            current_cancel_token.cancel();
                                            current_cancel_token =
                                                tokio_util::sync::CancellationToken::new();
                                            crate::app::start_new_session(&mut s);
                                        }
                                        "/resume" => {
                                            crate::app::resume_latest_session(&mut s);
                                        }
                                        "/skills" => {
                                            let skills = crate::skills::discover_skills();
                                            if skills.is_empty() {
                                                s.history.push(ChatMessage::new(
                                                    "system",
                                                    "No skills discovered.\nPlace SKILL.md files in .rustcode/skills/ or ~/.config/rustcode/skills/",
                                                ));
                                            } else {
                                                let mut out = format!("📦 Discovered Skills ({}):\n\n", skills.len());
                                                for skill in &skills {
                                                    out.push_str(&format!("  • {}\n", skill.name));
                                                    out.push_str(&format!("    Description: {}\n", skill.description));
                                                    out.push_str(&format!("    Path: {}\n\n", skill.path.display()));
                                                }
                                                s.history.push(ChatMessage::new("system", out));
                                            }
                                        }
                                        "/info" => {
                                            let info = crate::app::actions::build_info_text();
                                            s.history.push(ChatMessage::new("system", info));
                                        }
                                        "/changelog" => {
                                            let log_text = crate::app::actions::build_latest_changelog();
                                            s.history.push(ChatMessage::new("system", log_text));
                                        }
                                        "/quota" => {
                                            crate::app::actions::trigger_quota_fetch(
                                                &s,
                                                &app_state,
                                                &client,
                                            );
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
                                            s.current_response.clear();
                                            s.current_token_usage = None;
                                            s.status = crate::app::AppStatus::Idle;
                                        }
                                        "/cancel" => {
                                            current_cancel_token.cancel();
                                            current_cancel_token =
                                                tokio_util::sync::CancellationToken::new();
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
                            break;
                        }
                        continue;
                    }
                    drop(s);
                    dbg_log!("[KEY_EVENT] code={:?} modifiers={:?}", key.code, key.modifiers);

                    match key.code {
                        KeyCode::BackTab => {
                            let mut s = app_state.lock().await;
                            s.auto_confirm = !s.auto_confirm;
                        }
                        KeyCode::Esc => {
                            let now = std::time::Instant::now();
                            let mut s = app_state.lock().await;
                            
                            // Check if this is a double-esc (within 500ms)
                            match s.last_escape_time {
                                Some(last_time) if now.duration_since(last_time).as_millis() < 500 => {
                                    drop(s);
                                    crate::app::handle_escape(&app_state, &mut current_cancel_token)
                                        .await;
                                    needs_redraw = true;
                                }
                                _ => {
                                    // Single esc: clear selection or input buffer only (no cancel)
                                    if s.sel_start.is_some() || s.sel_end.is_some() {
                                        s.clear_selection();
                                    } else {
                                        s.input_buffer.clear();
                                        s.cursor_position = 0;
                                    }
                                    
                                    // Update last escape time for double-esc detection
                                    s.last_escape_time = Some(now);
                                }
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
                            } else if s.input_buffer.is_empty() {
                                s.history_up();
                            } else {
                                s.move_cursor_line_up();
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
                            } else if s.history_index.is_some() {
                                s.history_down();
                            } else {
                                s.move_cursor_line_down();
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
                        KeyCode::End => {
                            let mut s = app_state.lock().await;
                            s.scroll_to_bottom();
                        }
                        KeyCode::Tab => {
                            let mut s = app_state.lock().await;
                            let has_at = crate::app::get_at_word_query(&s.input_buffer, s.cursor_position).is_some();
                            if s.active_suggestion_index.is_some() || has_at {
                                crate::app::apply_autocomplete(&mut s);
                            } else if s.input_buffer.starts_with('/') && !s.input_buffer.contains(' ') {
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
                                    crate::config::AgentMode::Build => "Switched to Build Mode (Full Code Editing)",
                                    crate::config::AgentMode::Plan => "Switched to Plan Mode (Read-only / Design only)",
                                };
                                s.history.push(ChatMessage::new("system", notice.to_string()));
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

                        // Ctrl+Y, Cmd+C, or Ctrl+C copies the current app selection.
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('c') | KeyCode::Char('C')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL)
                                || key.modifiers.contains(event::KeyModifiers::SUPER)
                                || key.modifiers.contains(event::KeyModifiers::META) =>
                        {
                            let mut s = app_state.lock().await;
                            if let Some(text) = s.selected_text.clone() {
                                dbg_log!("[MAIN] KeyCopy copying selected text ({} chars): {:?}", text.len(), text);
                                crate::clipboard::copy_to_clipboard(&text);
                            }
                            s.clear_selection();
                        }
                        KeyCode::Char('t')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            let mut s = app_state.lock().await;
                            s.mouse_capture_enabled = !s.mouse_capture_enabled;
                            s.clear_selection();
                            use std::io::Write;
                            if s.mouse_capture_enabled {
                                write!(terminal.backend_mut(), "\x1b[?1006h").ok();
                            } else {
                                write!(terminal.backend_mut(), "\x1b[?1006l").ok();
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
                    let now = Instant::now();
                    // Coalesce rapid scroll events
                    if now.duration_since(last_scroll_time) < SCROLL_COALESCE_WINDOW {
                        match mouse.kind {
                            MouseEventKind::ScrollUp => scroll_coalesce += 1,
                            MouseEventKind::ScrollDown => scroll_coalesce -= 1,
                            _ => {}
                        }
                        continue;
                    }
                    // Process accumulated scroll
                    if scroll_coalesce != 0 {
                        let mut s = app_state.lock().await;
                        let modal = s.modal_open();
                        if !modal {
                            if scroll_coalesce > 0 {
                                s.scroll_up((scroll_coalesce as u16).min(10));
                            } else {
                                s.scroll_down((-scroll_coalesce as u16).min(10));
                            }
                            needs_redraw = true;
                        }
                        scroll_coalesce = 0;
                    }
                    last_scroll_time = now;

                    let mut s = app_state.lock().await;
                    let modal = s.modal_open();
                    match mouse.kind {
                        MouseEventKind::ScrollUp if !modal => {
                            s.scroll_up(3);
                            if s.selecting {
                                s.sel_end = Some((mouse.column, mouse.row + s.scroll_row));
                            }
                            needs_redraw = true;
                        }
                        MouseEventKind::ScrollDown if !modal => {
                            s.scroll_down(3);
                            if s.selecting {
                                s.sel_end = Some((mouse.column, mouse.row + s.scroll_row));
                            }
                            needs_redraw = true;
                        }
                        MouseEventKind::Down(MouseButton::Left) if !modal => {
                            let hit_scroll_btn = s
                                .scroll_to_bottom_btn
                                .map(|r| {
                                    r.contains(ratatui::layout::Position::new(
                                        mouse.column,
                                        mouse.row,
                                    ))
                                })
                                .unwrap_or(false);
                            if hit_scroll_btn {
                                // Jump to newest — do NOT start a text selection, and do
                                // NOT return from the event loop (that would quit the app).
                                s.scroll_to_bottom();
                                needs_redraw = true;
                            } else {
                            let inside_chat = if let Some(ca) = s.chat_area {
                                mouse.row >= ca.y && mouse.row < ca.y + ca.height &&
                                mouse.column >= ca.x && mouse.column < ca.x + ca.width
                            } else {
                                true
                            };
                            let inside_input = s.input_text_area.map(|ia| {
                                mouse.row >= ia.y && mouse.row < ia.y + ia.height &&
                                mouse.column >= ia.x && mouse.column < ia.x + ia.width
                            }).unwrap_or(false);
                            if inside_chat {
                                s.sel_in_input = false;
                                s.sel_start = Some((mouse.column, mouse.row + s.scroll_row));
                                s.sel_end = Some((mouse.column, mouse.row + s.scroll_row));
                                s.selecting = true;
                            } else if inside_input {
                                // Input box has no scroll offset: store raw screen rows.
                                s.sel_in_input = true;
                                s.sel_start = Some((mouse.column, mouse.row));
                                s.sel_end = Some((mouse.column, mouse.row));
                                s.selecting = true;
                            } else {
                                s.clear_selection();
                            }
                            needs_redraw = true;
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) if s.selecting => {
                            let target_row = if s.sel_in_input {
                                // Clamp to the input rect; no scroll in the input box.
                                match s.input_text_area {
                                    Some(ia) => mouse.row.max(ia.y).min((ia.y + ia.height).saturating_sub(1)),
                                    None => mouse.row,
                                }
                            } else {
                                let mut tr = mouse.row + s.scroll_row;
                                if let Some(ca) = s.chat_area {
                                    if mouse.row < ca.y {
                                        s.scroll_up(1);
                                        tr = ca.y + s.scroll_row;
                                    } else if mouse.row >= ca.y + ca.height {
                                        s.scroll_down(1);
                                        tr = (ca.y + ca.height).saturating_sub(1) + s.scroll_row;
                                    }
                                }
                                tr
                            };
                            s.sel_end = Some((mouse.column, target_row));
                            needs_redraw = true;
                        }
                        MouseEventKind::Up(MouseButton::Left) if s.selecting => {
                            let target_row = if s.sel_in_input {
                                match s.input_text_area {
                                    Some(ia) => mouse.row.max(ia.y).min((ia.y + ia.height).saturating_sub(1)),
                                    None => mouse.row,
                                }
                            } else {
                                let mut tr = mouse.row + s.scroll_row;
                                if let Some(ca) = s.chat_area {
                                    tr = tr.max(ca.y + s.scroll_row).min((ca.y + ca.height).saturating_sub(1) + s.scroll_row);
                                }
                                tr
                            };
                            s.sel_end = Some((mouse.column, target_row));
                            s.selecting = false;
                            if let (Some(a), Some(b)) = (s.sel_start, s.sel_end) {
                                if a != b {
                                    // Dragged: copy on release, like selecting on a web page.
                                    if let Some(text) = s.selected_text.take() {
                                        dbg_log!("[MAIN] MouseUp copying selected text ({} chars): {:?}", text.len(), text);
                                        crate::clipboard::copy_to_clipboard(&text);
                                    }
                                } else {
                                    // A plain click: toggle a thought if one sits on this
                                    // row, otherwise just clear any existing selection.
                                    s.clear_selection();
                                    let click_screen_row = b.1.saturating_sub(s.scroll_row);
                                    if let Some(&(_, idx)) =
                                        s.thought_toggle_rows.iter().find(|(row, _)| *row == click_screen_row)
                                    {
                                        s.toggle_thought(idx);
                                    } else if let Some((_, code)) = s
                                        .code_copy_rows
                                        .iter()
                                        .find(|(row, _)| *row == click_screen_row)
                                        .map(|(r, t)| (*r, t.clone()))
                                    {
                                        // Clicked a code block's [Copy] badge row.
                                        crate::clipboard::copy_to_clipboard(&code);
                                        s.last_copy_time = Some(std::time::Instant::now());
                                    }
                                }
                            }
                            needs_redraw = true;
                        }
                        _ => {}
                    }
                }
                Event::Paste(text) => {
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
                        needs_redraw = true;
                        continue;
                    }
                    let mut s = app_state.lock().await;
                    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                    // Route the paste into whichever text field is focused: the
                    // ask_question custom-answer slot, the MCP editor, else chat.
                    if s.status == AppStatus::AwaitingQuestion {
                        if let Some(q) = s.pending_question.as_mut()
                            && let Some(buf) = q.custom_input.as_mut()
                        {
                            for c in normalized.chars() {
                                if c != '\n' && c != '\r' {
                                    buf.push(c);
                                }
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
                        for c in normalized.chars() {
                            s.insert_char(c);
                        }
                        s.reset_suggestion_cycle();
                    }
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
