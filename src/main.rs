#[macro_use]
mod logger;
mod acp;
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
mod update;

use crate::app::{AppState, AppStatus, ChatMessage, Verbosity};
use clap::Parser;
use crossterm::{
    cursor::SetCursorStyle,
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16); // 60Hz for smooth scrolling

/// Frame budget while a response is in flight: 60Hz, so streamed tokens,
/// spinners, the elapsed-second counter and the rotating status label (which
/// changes every two seconds) all stay live.
const STREAM_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Whether the next loop iteration should render.
///
/// A frame is drawn when something actually changed (`needs_redraw`, set by
/// input handling and by background tasks via `AppState::request_redraw`), or
/// on the streaming cadence while a response is active. Nothing animates on a
/// timer once the app is idle, so an idle app with no input draws nothing at
/// all instead of re-rendering ten times a second.
fn should_draw(needs_redraw: bool, response_active: bool, since_last_draw: Duration) -> bool {
    needs_redraw || (response_active && since_last_draw >= STREAM_FRAME_INTERVAL)
}

fn background_task_history_message(
    task_id: &str,
    output: crate::tools::ToolExecutionOutput,
) -> ChatMessage {
    let prefix = format!("background_task: Task {task_id} completed. Output:\n");
    crate::network::bounded_tool_result_history_message(
        crate::network::ToolResult {
            tool_name: "background_task".to_string(),
            content: output.content,
            diff: None,
            file_preview: None,
            metadata: crate::network::ToolResultMetadata {
                success: output.success,
                exit_code: output.exit_code,
                truncated: output.truncated,
                ..Default::default()
            },
        },
        &prefix,
        None,
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Cheap, once-per-process check: rotate debug.log out of the way if a
    // prior session let it grow past the size cap, instead of letting every
    // subsequent write add to an already-huge file.
    crate::logger::rotate_if_oversized();

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
                println!("💾 [sync] Staging all config files...");
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
                println!(
                    "Sync repository setup complete! You can now run `rustcode sync` anytime."
                );
            }
            None => {
                // Default behavior for `rustcode sync` (pull then push)
                println!("📥 [sync] Pulling latest config and skills from remote origin/main...");
                if let Err(e) = config::sync_config_pull() {
                    eprintln!("Sync failed during pull: {e}");
                    std::process::exit(1);
                }
                println!("💾 [sync] Staging all config files...");
                if let Err(e) = config::sync_config_push() {
                    eprintln!("Sync failed during push: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
        }
        return Ok(());
    }

    if cli_args.upgrade {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;
        match crate::update::upgrade_if_available(&client).await {
            Ok(crate::update::UpdateCheck::UpToDate { current, latest }) => {
                println!(
                    "No update available. rustcode v{} is up to date (latest published: v{}).",
                    crate::update::format_version(current),
                    crate::update::format_version(latest)
                );
            }
            Ok(crate::update::UpdateCheck::Available { current, latest }) => {
                println!(
                    "Updated rustcode from v{} to v{}.",
                    crate::update::format_version(current),
                    crate::update::format_version(latest)
                );
            }
            Err(error) => {
                eprintln!("Upgrade failed: {error}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    if cli_args.acp {
        crate::acp::run_acp().await?;
        crate::config::flush_history();
        return Ok(());
    }

    if let Some(prompt) = cli_args.prompt {
        raw_cli::run_raw_cli(&prompt, model_override.as_deref()).await?;
        crate::config::flush_history();
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableFocusChange,
        SetCursorStyle::BlinkingBar,
        crossterm::style::Print("\x1b]0;rustcode · new session\x07")
    )?;

    let _ = execute!(
        stdout,
        event::PushKeyboardEnhancementFlags(
            event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        )
    );

    let backend = CrosstermBackend::new(stdout);
    let terminal_height = crossterm::terminal::size()?.1;
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(terminal_height),
        },
    )?;
    terminal.clear()?;

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

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;
    {
        let mut state = app_state.lock().await;
        state.update_check = crate::update::UpdateState::Checking;
    }
    let update_state = Arc::clone(&app_state);
    let update_client = client.clone();
    tokio::spawn(async move {
        let result = crate::update::check_for_update(&update_client).await;
        let mut state = update_state.lock().await;
        state.update_check = match result {
            Ok(crate::update::UpdateCheck::UpToDate { latest, .. }) => {
                crate::update::UpdateState::UpToDate(latest)
            }
            Ok(crate::update::UpdateCheck::Available { latest, .. }) => {
                crate::update::UpdateState::Available(latest)
            }
            Err(_) => crate::update::UpdateState::Failed,
        };
        state.request_redraw();
    });
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
                s.history
                    .push(background_task_history_message(&task_id, output));
                crate::config::save_session_history(&session_id, &s.history);
                s.request_redraw();
                // Drive a fresh model turn when a background task completes in the active session
                // so the agent automatically receives the result and continues working.
                s.pending_queue.push(format!("__task_wakeup__:{task_id}"));
                // Only spawn if no orchestrator is alive; otherwise the
                // running one drains the queue. Gating on status==Idle raced
                // an exiting turn and spawned a second concurrent orchestrator.
                if !s.orchestrator_running {
                    s.orchestrator_running = true;
                    s.status = AppStatus::Queued;
                    drop(s);
                    crate::network::process_queue_orchestrator(
                        client_clone,
                        state_clone,
                        token_clone,
                        std::sync::Arc::new(crate::network::policy::InteractivePolicy),
                    )
                    .await;
                }
            } else {
                let mut history = crate::config::load_session_history_direct(&session_id);
                history.push(background_task_history_message(&task_id, output));
                crate::config::save_session_history(&session_id, &history);
            }
        });
    });

    // Spawn startup initialization of enabled MCP servers.
    let mcp_servers = app_state.lock().await.config.mcp_servers.clone();
    tokio::spawn(async move {
        crate::mcp::start_enabled_servers(&mcp_servers, |name| async move {
            crate::mcp::start_server_by_name(&name).await
        })
        .await;
    });

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
        let (response_active, background_redraw, active_notice) = {
            let mut s = app_state.lock().await;
            (
                s.status != AppStatus::Idle,
                s.take_redraw_request(),
                s.has_active_notice(),
            )
        };
        needs_redraw |= background_redraw || active_notice;

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
                        std::sync::Arc::new(crate::network::policy::InteractivePolicy),
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
        let should_draw = should_draw(needs_redraw, response_active, last_draw.elapsed());

        if should_draw {
            let mut guard = app_state.lock().await;

            // Update terminal title based on the same activity snapshot used by
            // the footer, so state and animation stay synchronized.
            let custom_title = guard.cached_session_title().or_else(|| {
                guard
                    .history
                    .iter()
                    .find(|m| m.role == "user" && !m.content.starts_with('/'))
                    .map(|m| m.content.lines().next().unwrap_or("").trim().to_string())
            });
            let session_name = custom_title
                .filter(|title| !title.is_empty() && !title.starts_with('/'))
                .unwrap_or_else(|| "session".to_string());
            let activity =
                crate::app::activity::classify_activity(&guard.status, &guard.running_tools);
            let animation_frame = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
                / 100;
            let title_display = crate::app::activity::format_terminal_title(
                activity.kind,
                &session_name,
                animation_frame,
            );

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
                    let is_ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                    let is_cmd = key.modifiers.contains(event::KeyModifiers::SUPER);

                    if (is_ctrl || is_cmd)
                        && (key.code == KeyCode::Char('k') || key.code == KeyCode::Char('K'))
                    {
                        terminal.clear().ok();
                        continue;
                    }
                    if is_ctrl && (key.code == KeyCode::Char('l') || key.code == KeyCode::Char('L'))
                    {
                        terminal.clear().ok();
                        continue;
                    }

                    if is_ctrl && key.code == KeyCode::Char('c') {
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
                                    if total_lines > 0
                                        && (s.modal_scroll_row as usize) + 1 < total_lines
                                    {
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
                                match key.code {
                                    KeyCode::Char('v') | KeyCode::Char('V')
                                        if key.modifiers.contains(event::KeyModifiers::CONTROL)
                                            || key
                                                .modifiers
                                                .contains(event::KeyModifiers::SUPER)
                                            || key
                                                .modifiers
                                                .contains(event::KeyModifiers::META) =>
                                    {
                                        if let Some(text) =
                                            crate::clipboard::read_text_from_clipboard()
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
                                                || key
                                                    .modifiers
                                                    .contains(event::KeyModifiers::CONTROL)
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
                                                || key
                                                    .modifiers
                                                    .contains(event::KeyModifiers::CONTROL)
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
                                        let mut s = app_state.lock().await;
                                        let mut answer = s
                                            .pending_question
                                            .as_ref()
                                            .and_then(|q| q.custom_input.clone())
                                            .unwrap_or_default()
                                            .trim()
                                            .to_string();
                                        if answer.is_empty() {
                                            answer = "No response provided".to_string();
                                        }
                                        if let Some(tx) = s.question_response.take() {
                                            let _ = tx.send(answer);
                                        }
                                        s.pending_question = None;
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
                                needs_redraw = true;
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
                                            let answer = q.options[idx].clone();
                                            if let Some(tx) = s.question_response.take() {
                                                let _ = tx.send(answer);
                                            }
                                            s.pending_question = None;
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
                                                q.options
                                                    .get(q.selected)
                                                    .cloned()
                                                    .unwrap_or_default()
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
                                    current_cancel_token.cancel();
                                    current_cancel_token =
                                        tokio_util::sync::CancellationToken::new();
                                    let mut s = app_state.lock().await;
                                    if let Some(tx) = s.question_response.take() {
                                        let _ = tx.send("User cancelled prompt.".to_string());
                                    }
                                    s.pending_question = None;
                                    s.status = AppStatus::Idle;
                                }
                                _ => {}
                            }
                            needs_redraw = true;
                            continue;
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
                                    s.status = AppStatus::Idle;
                                }
                                KeyCode::Esc => {
                                    let mut s = app_state.lock().await;
                                    s.status = AppStatus::Idle;
                                }
                                _ => {}
                            }
                            continue;
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
                                    s.status = AppStatus::Idle;
                                }
                                KeyCode::Esc => {
                                    let mut s = app_state.lock().await;
                                    s.status = AppStatus::Idle;
                                }
                                _ => {}
                            }
                            continue;
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
                                    s.status = AppStatus::Idle;
                                }
                                KeyCode::Esc => {
                                    let mut s = app_state.lock().await;
                                    s.status = AppStatus::Idle;
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
                            let idx = s
                                .history_picker_index
                                .min(s.history_picker_sessions.len().saturating_sub(1));
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
                                        && del_idx
                                            >= s.history_picker_sessions.len().saturating_sub(1)
                                    {
                                        s.history_picker_index =
                                            (del_idx as i64 - 1).max(0) as usize;
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
                                let selected = themes
                                    [s.theme_picker_index.min(len.saturating_sub(1))]
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
                                        "/update" => {
                                            crate::app::actions::trigger_update(
                                                &app_state, &client,
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
                                            s.history_display_start = s.history.len();
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
                    dbg_log!(
                        "[KEY_EVENT] code={:?} modifiers={:?}",
                        key.code,
                        key.modifiers
                    );

                    match key.code {
                        KeyCode::BackTab => {
                            let mut s = app_state.lock().await;
                            s.auto_confirm = !s.auto_confirm;
                        }
                        KeyCode::Esc => {
                            let mut s = app_state.lock().await;
                            if s.sel_start.is_some() || s.sel_end.is_some() {
                                s.clear_selection();
                            } else if !s.input_buffer.is_empty() {
                                s.input_buffer.clear();
                                s.cursor_position = 0;
                            } else {
                                drop(s);
                                crate::app::handle_escape(&app_state, &mut current_cancel_token)
                                    .await;
                            }
                            needs_redraw = true;
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
                            } else if s.input_buffer.is_empty() || s.history_index.is_some() {
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
                        KeyCode::Tab => {
                            let mut s = app_state.lock().await;
                            let has_at =
                                crate::app::get_at_word_query(&s.input_buffer, s.cursor_position)
                                    .is_some();
                            if s.active_suggestion_index.is_some() || has_at {
                                crate::app::apply_autocomplete(&mut s);
                            } else if s.input_buffer.starts_with('/')
                                && !s.input_buffer.contains(' ')
                            {
                                s.cycle_suggestion();
                            } else {
                                // Toggle Agent Mode (Build vs Plan)
                                s.agent_mode = match s.agent_mode {
                                    crate::config::AgentMode::Build => {
                                        crate::config::AgentMode::Plan
                                    }
                                    crate::config::AgentMode::Plan => {
                                        crate::config::AgentMode::Build
                                    }
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
                                const PASTE_THRESHOLD: usize = 300;
                                let text_to_insert =
                                    if normalized.chars().count() >= PASTE_THRESHOLD {
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
                        KeyCode::Char('p')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            let mut s = app_state.lock().await;
                            s.show_command_picker = true;
                            s.command_picker_index = 0;
                            s.command_picker_search.clear();
                        }

                        // Ctrl+Y, Cmd+C, or Ctrl+C copies the current app selection.
                        KeyCode::Char('y')
                        | KeyCode::Char('Y')
                        | KeyCode::Char('c')
                        | KeyCode::Char('C')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL)
                                || key.modifiers.contains(event::KeyModifiers::SUPER)
                                || key.modifiers.contains(event::KeyModifiers::META) =>
                        {
                            let mut s = app_state.lock().await;
                            if let Some(text) = s.selected_text.clone() {
                                dbg_log!(
                                    "[MAIN] KeyCopy copying selected text ({} chars): {:?}",
                                    text.len(),
                                    text
                                );
                                if crate::clipboard::copy_to_clipboard(&text) {
                                    s.set_notice("Copied to clipboard");
                                }
                            }
                            s.clear_selection();
                            needs_redraw = true;
                        }
                        KeyCode::Char('t')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            let mut s = app_state.lock().await;
                            s.mouse_capture_enabled = !s.mouse_capture_enabled;
                            s.clear_selection();
                            use std::io::Write;
                            if s.mouse_capture_enabled {
                                write!(terminal.backend_mut(), "\x1b[?1006h\x1b[?1003h").ok();
                            } else {
                                s.hover = crate::app::HoverTarget::None;
                                write!(terminal.backend_mut(), "\x1b[?1006l\x1b[?1003l").ok();
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

                    // Refresh the hover highlight for every pointer event, not just
                    // motion: scrolling moves rows under a stationary pointer too.
                    // Any-motion reporting fires on each cell crossed, so a redraw
                    // is only worth it when the hovered element actually changes.
                    let next_hover = if modal {
                        crate::app::HoverTarget::None
                    } else {
                        s.hover_target_at(mouse.column, mouse.row)
                    };
                    if s.hover != next_hover {
                        s.hover = next_hover;
                        needs_redraw = true;
                    }

                    match mouse.kind {
                        MouseEventKind::Moved => {}
                        MouseEventKind::ScrollUp if !modal => {
                            s.scroll_up(1);
                            if s.selecting {
                                s.sel_end = Some((mouse.column, mouse.row + s.scroll_row));
                            }
                            needs_redraw = true;
                        }
                        MouseEventKind::ScrollDown if !modal => {
                            s.scroll_down(1);
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
                                    mouse.row >= ca.y
                                        && mouse.row < ca.y + ca.height
                                        && mouse.column >= ca.x
                                        && mouse.column < ca.x + ca.width
                                } else {
                                    true
                                };
                                let inside_input = s
                                    .input_text_area
                                    .map(|ia| {
                                        mouse.row >= ia.y
                                            && mouse.row < ia.y + ia.height
                                            && mouse.column >= ia.x
                                            && mouse.column < ia.x + ia.width
                                    })
                                    .unwrap_or(false);
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
                                    Some(ia) => mouse
                                        .row
                                        .max(ia.y)
                                        .min((ia.y + ia.height).saturating_sub(1)),
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
                                    Some(ia) => mouse
                                        .row
                                        .max(ia.y)
                                        .min((ia.y + ia.height).saturating_sub(1)),
                                    None => mouse.row,
                                }
                            } else {
                                let mut tr = mouse.row + s.scroll_row;
                                if let Some(ca) = s.chat_area {
                                    tr = tr
                                        .max(ca.y + s.scroll_row)
                                        .min((ca.y + ca.height).saturating_sub(1) + s.scroll_row);
                                }
                                tr
                            };
                            s.sel_end = Some((mouse.column, target_row));
                            s.selecting = false;
                            if let (Some(a), Some(b)) = (s.sel_start, s.sel_end) {
                                if a != b {
                                    // Dragged: copy on release, like selecting on a web page.
                                    if let Some(text) = s.selected_text.take() {
                                        dbg_log!(
                                            "[MAIN] MouseUp copying selected text ({} chars): {:?}",
                                            text.len(),
                                            text
                                        );
                                        if crate::clipboard::copy_to_clipboard(&text) {
                                            s.set_notice("Copied to clipboard");
                                        }
                                    }
                                    // The text is on the clipboard, so the marks
                                    // have done their job — drop them instead of
                                    // leaving the block highlighted.
                                    s.clear_selection();
                                } else {
                                    // A plain click clears any existing selection.
                                    s.clear_selection();
                                    let click_screen_row = b.1.saturating_sub(s.scroll_row);
                                    if let Some((_, code)) = s
                                        .code_copy_rows
                                        .iter()
                                        .find(|(row, _)| *row == click_screen_row)
                                        .map(|(r, t)| (*r, t.clone()))
                                    {
                                        // Clicked a code block's header row. Only copy if click is on the right edge Copy button.
                                        let badge_width = if s.last_copy_text.as_ref().is_some_and(
                                            |(t_text, t)| {
                                                t_text == &code && t.elapsed().as_secs() < 2
                                            },
                                        ) {
                                            12
                                        } else {
                                            9
                                        };
                                        let is_on_copy_button = s.chat_area.map_or(true, |ca| {
                                            b.0 >= (ca.x + ca.width).saturating_sub(badge_width)
                                        });
                                        if is_on_copy_button {
                                            if crate::clipboard::copy_to_clipboard(&code) {
                                                s.set_notice("Copied to clipboard");
                                            }
                                            s.last_copy_text =
                                                Some((code.clone(), std::time::Instant::now()));
                                        }
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
                    needs_redraw = true;
                }
                Event::Resize(_, _) => {
                    needs_redraw = true;
                }
            }
        }
    }

    // Shutdown: nothing queued may be lost, so write it out synchronously.
    crate::config::flush_history();

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
        SetCursorStyle::DefaultUserShape
    )?;
    let goodbye_origin = goodbye_cursor_position(terminal.size()?.height);
    execute!(
        terminal.backend_mut(),
        crossterm::cursor::MoveTo(goodbye_origin.0, goodbye_origin.1)
    )?;
    terminal.show_cursor()?;

    print_goodbye();

    Ok(())
}

fn goodbye_cursor_position(height: u16) -> (u16, u16) {
    (0, height.saturating_sub(1))
}

/// Printed on every exit path (/quit, /exit, Ctrl+C, ...) after the terminal
/// is restored, so the box lands on the normal screen like other CLIs'
/// farewell messages.
fn print_goodbye() {
    use std::io::Write;
    use unicode_width::UnicodeWidthStr;

    let (_, _, config) = crate::config::load_config();
    let duration_seconds = config
        .start_time
        .map_or(0, |start| start.elapsed().map_or(0, |d| d.as_secs()));

    // Theme colors are ratatui `Color`s; convert the RGB variants to ANSI
    // true-color escapes (falls back to default fg for anything else).
    fn fg(c: ratatui::style::Color) -> String {
        match c {
            ratatui::style::Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
            _ => String::new(),
        }
    }
    const RESET: &str = "\x1b[0m";

    let border = fg(crate::ui::theme::color_primary());
    let text = fg(crate::ui::theme::color_text());

    let title = format!(" rustcode v{} ", env!("CARGO_PKG_VERSION"));
    let msg = format!("👋 Goodbye - session ran for {}s", duration_seconds);
    let msg_width = UnicodeWidthStr::width(msg.as_str());
    let title_width = UnicodeWidthStr::width(title.as_str());
    let content_width = msg_width.max(title_width);

    let top = format!(
        "╭─{}{}{}─╮",
        title,
        "─".repeat(content_width - title_width),
        border
    );
    let bot = format!("{border}╰{}{border}╯", "─".repeat(content_width + 2));
    let msg_fill = " ".repeat(content_width - msg_width);

    let mut out = std::io::stdout();
    let _ = writeln!(out);
    let _ = writeln!(out, "{border}{top}");
    let _ = writeln!(out, "{border}│ {text}{msg}{msg_fill}{border} │");
    let _ = writeln!(out, "{border}{bot}{RESET}");
    let _ = writeln!(out);
}

#[cfg(test)]
mod draw_loop_tests {
    use super::{
        STREAM_FRAME_INTERVAL, background_task_history_message, goodbye_cursor_position,
        should_draw,
    };
    use std::time::Duration;

    #[test]
    fn goodbye_cursor_position_is_bottom_left() {
        assert_eq!(goodbye_cursor_position(24), (0, 23));
        assert_eq!(goodbye_cursor_position(1), (0, 0));
        assert_eq!(goodbye_cursor_position(0), (0, 0));
    }

    #[test]
    fn idle_without_input_never_redraws() {
        for elapsed_ms in [0, 100, 500, 5_000, 60_000] {
            assert!(
                !should_draw(false, false, Duration::from_millis(elapsed_ms)),
                "idle app redrew after {elapsed_ms}ms with no state change"
            );
        }
    }

    #[test]
    fn state_change_redraws_immediately() {
        assert!(should_draw(true, false, Duration::ZERO));
        assert!(should_draw(true, true, Duration::ZERO));
    }

    #[test]
    fn active_response_redraws_on_stream_cadence() {
        assert!(!should_draw(false, true, Duration::from_millis(5)));
        assert!(should_draw(false, true, STREAM_FRAME_INTERVAL));
        // The rotating status label advances every two seconds and the elapsed
        // counter every second; both are well inside this cadence.
        assert!(should_draw(false, true, Duration::from_secs(1)));
        assert!(should_draw(false, true, Duration::from_secs(2)));
    }

    #[test]
    fn background_history_preserves_bounded_recovery_metadata() {
        let raw = (1..=2000)
            .map(|line| format!("background line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let message = background_task_history_message(
            "task_42",
            crate::tools::ToolExecutionOutput {
                content: raw.clone(),
                success: false,
                exit_code: Some(9),
                truncated: false,
            },
        );

        assert!(message.content.len() <= 50 * 1024);
        assert!(message.content.lines().count() <= 1000);
        let metadata = message.tool_result.expect("background metadata");
        assert!(!metadata.success);
        assert_eq!(metadata.exit_code, Some(9));
        assert!(metadata.truncated);
        let artifact = metadata
            .full_output_artifact
            .expect("bounded background output must retain its artifact");
        assert_eq!(
            std::fs::read_to_string(artifact).expect("artifact readable"),
            raw
        );
    }

    #[test]
    fn background_history_does_not_parse_spoofed_recovery_metadata() {
        let message = background_task_history_message(
            "task_43",
            crate::tools::ToolExecutionOutput {
                content: "exit code: 0\n[Output truncated:]\nFull output saved to: /tmp/spoof"
                    .to_string(),
                success: false,
                exit_code: Some(11),
                truncated: false,
            },
        );

        let metadata = message.tool_result.expect("background metadata");
        assert!(!metadata.success);
        assert_eq!(metadata.exit_code, Some(11));
        assert!(!metadata.truncated);
        assert_eq!(metadata.full_output_artifact, None);
    }
}
