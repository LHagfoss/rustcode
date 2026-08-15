#[macro_use]
mod logger;
mod acp;
mod app;
mod cli;
mod clipboard;
mod config;
mod context;
mod inline_terminal;
mod mcp;
mod memory;
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
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    widgets::{Paragraph, Widget, Wrap},
};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16); // 60Hz for smooth scrolling
/// Frame budget while a response is in flight: 60Hz, so streamed tokens,
/// spinners, the elapsed-second counter and the rotating status label (which
/// changes every two seconds) all stay live.
const STREAM_FRAME_INTERVAL: Duration = Duration::from_millis(16);

fn insert_scrollback_lines<B: Backend>(
    terminal: &mut crate::inline_terminal::InlineTerminal<B>,
    lines: Vec<ratatui::text::Line<'static>>,
    width: u16,
) -> Result<(), B::Error> {
    if lines.is_empty() {
        return Ok(());
    }
    let height = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(width)
        .max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(buffer.area, buffer);
    })
}

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
                replayed: output.replayed,
                error_kind: output.error_kind,
                retryable: output.retryable,
                ..Default::default()
            },
        },
        &prefix,
        None,
    )
}

fn queue_background_wakeup(state: &mut AppState, task_id: &str) {
    state.pending_queue.push(format!("__task_wakeup__:{task_id}"));
    state.request_redraw();
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
    // Like Codex, start with an empty inline viewport at the shell cursor and
    // grow it to the renderer's desired height on each frame.
    let mut terminal = crate::inline_terminal::InlineTerminal::new(backend)?;

    crate::config::archive_live_history();

    let mut app_state_struct = AppState::new();
    if cli_args.yolo {
        app_state_struct.auto_confirm = true;
    }
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
    let handle = tokio::runtime::Handle::current();
    crate::tools::register_wakeup_callback(move |session_id, task_id, output| {
        let state_clone = Arc::clone(&state_cb);
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
                // Drive a fresh model turn when a background task completes in the active session
                // so the agent automatically receives the result and continues working.
                queue_background_wakeup(&mut s, &task_id);
                // Do not start an orchestrator here. This callback outlives individual
                // turns, so capturing their cancellation token leaves future wakeups
                // permanently cancelled after the first interrupt. The main event loop
                // observes this queued item and starts it with the current token instead.
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
    let mut transcript_cursor = crate::ui::scrollback::TranscriptCursor::default();
    let mut transcript_state = crate::ui::TranscriptState::default();
    let mut stream_commits = crate::ui::scrollback::StreamCommitQueue::default();
    let mut terminal_size = terminal.size()?;
    let mut replay_history = false;

    loop {
        // Ratatui's inline viewport grows/shrinks by appending and clearing
        // terminal rows. Once scrollback has been emitted those rows cannot be
        // reflowed in place, so a resize must purge the visible transcript and
        // replay it from the durable history plus the current stream.
        let observed_size = terminal.size()?;
        if observed_size != terminal_size {
            terminal.autoresize()?;
            if observed_size.width != terminal_size.width {
                execute!(terminal.backend_mut(), Clear(ClearType::Purge), Clear(ClearType::All))?;
                terminal.clear()?;
                transcript_cursor.reset();
                transcript_state.reset();
                stream_commits.reset();
                replay_history = true;
            }
            terminal_size = observed_size;
            needs_redraw = true;
        }

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

            let terminal_width = terminal.size()?.width;
            transcript_cursor.begin_stream(&guard.current_response);
            let stable_source = if replay_history {
                String::new()
            } else {
                transcript_cursor.pending_stable_source(&guard.current_response)
            };
            if !stable_source.is_empty() {
                let is_continuation = transcript_cursor.has_committed_stream();
                let lines = crate::ui::render_committed_assistant_chunk(
                    &guard,
                    &stable_source,
                    terminal_width,
                    is_continuation,
                );
                if !lines.is_empty() {
                    stream_commits.push(lines);
                }
                transcript_cursor.commit_stable_stream(&stable_source);
            }

            let history_range = if replay_history {
                0..guard.history.len()
            } else {
                transcript_cursor.pending_history_range(guard.history.len())
            };
            let stable_lines = stream_commits.take_ready(
                replay_history || !history_range.is_empty() || !response_active,
            );
            if !stable_lines.is_empty() {
                insert_scrollback_lines(&mut terminal, stable_lines, terminal_width)?;
            }
            let mut blocks = Vec::new();
            if !replay_history && transcript_cursor.is_at_start() && !history_range.is_empty() {
                let banner = crate::ui::build_claude_startup_banner(&guard, terminal_width as usize, 24);
                if !banner.is_empty() {
                    blocks.push(banner);
                }
            }
            let mut index = history_range.start;
            while index < history_range.end {
                let message = &guard.history[index];
                if message.role == "tool" {
                    let group_end = (index + 1..history_range.end)
                        .find(|&next| guard.history[next].role != "tool")
                        .unwrap_or(history_range.end);
                    let indices = (index..group_end).collect::<Vec<_>>();
                    let mut block = crate::ui::render_committed_tool_result_group(
                        &guard,
                        &indices,
                        terminal_width,
                        false,
                    );
                    if !block.is_empty() {
                        block.push(ratatui::text::Line::from(""));
                        blocks.push(block);
                    }
                    index = group_end;
                    continue;
                } else if message.role == "assistant" {
                    let separator = crate::ui::render_work_separator_before_assistant(
                        &guard,
                        index,
                        terminal_width,
                    );
                    if !separator.is_empty() {
                        blocks.push(separator);
                    }
                    let is_continuation = transcript_cursor.has_committed_stream();
                    if let Some(remainder) =
                        transcript_cursor.take_final_stream_remainder(&message.content)
                    {
                        if !remainder.is_empty() {
                            let mut chunk = crate::ui::render_committed_assistant_chunk(
                                &guard,
                                &remainder,
                                terminal_width,
                                is_continuation,
                            );
                            if !chunk.is_empty() {
                                chunk.push(ratatui::text::Line::from(""));
                                blocks.push(chunk);
                            }
                        } else {
                            // Stable stream rows were already inserted above the live viewport.
                            // They do not carry a trailing separator while the response is
                            // streaming, so add one when the finalized history entry hands off
                            // to the next message. Without this row a follow-up prompt can sit
                            // directly on the last table/multiline response row.
                            blocks.push(vec![ratatui::text::Line::from("")]);
                        }
                    } else {
                        blocks.push(crate::ui::render_committed_history_block(
                            &guard,
                            index,
                            terminal_width,
                        ));
                    }
                } else {
                    let block = crate::ui::render_committed_history_block(&guard, index, terminal_width);
                    if !block.is_empty() {
                        blocks.push(block);
                    }
                }
                index += 1;
            }
            for lines in blocks {
                insert_scrollback_lines(&mut terminal, lines, terminal_width)?;
            }

            // During a resize, history must be inserted before the still-live
            // response. Re-emit the stable prefix only after the history pass.
            if replay_history {
                let stable_source =
                    transcript_cursor.pending_stable_source(&guard.current_response);
                if !stable_source.is_empty() {
                    let lines = crate::ui::render_committed_assistant_chunk(
                        &guard,
                        &stable_source,
                        terminal_width,
                        false,
                    );
                    insert_scrollback_lines(&mut terminal, lines, terminal_width)?;
                    transcript_cursor.commit_stable_stream(&stable_source);
                }
            }
            transcript_cursor.commit_history_through(history_range.end);

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

            let terminal_height = terminal.size()?.height;
            let desired_height = ui::desired_height(
                &guard,
                &mut transcript_state,
                terminal_width,
                terminal_height,
            );
            terminal.draw_height(desired_height, |f| {
                ui::render_with_transcript(f, &mut guard, &mut transcript_state)
            })?;
            replay_history = false;
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

                    if is_ctrl
                        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
                    {
                        if crate::app::handle_ctrl_c(&app_state, &mut current_cancel_token).await {
                            break;
                        }
                        continue;
                    }

                    {
                        let s = app_state.lock().await;
                        if s.status == AppStatus::AwaitingToolConfirmation {
                            drop(s);
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    let mut s = app_state.lock().await;
                                    s.pending_tool_confirmation = None;
                                    if let Some(tx) = s.tool_confirmation_response.take() {
                                        let _ = tx.send(true);
                                    }
                                }
                                KeyCode::Enter => {
                                    let approved = {
                                        let s = app_state.lock().await;
                                        s.tool_confirmation_selected == 0
                                    };
                                    if !approved {
                                        current_cancel_token.cancel();
                                        current_cancel_token =
                                            tokio_util::sync::CancellationToken::new();
                                    }
                                    let mut s = app_state.lock().await;
                                    if let Some(tx) = s.tool_confirmation_response.take() {
                                        let _ = tx.send(approved);
                                    }
                                    s.pending_tool_confirmation = None;
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
                                    s.move_tool_confirmation_selection(-1);
                                }
                                KeyCode::Down => {
                                    let mut s = app_state.lock().await;
                                    s.move_tool_confirmation_selection(1);
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
                                        "/yolo" => {
                                            crate::app::actions::toggle_auto_confirm(&mut s);
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
                                crate::app::handle_escape(&app_state, &mut current_cancel_token)
                                    .await;
                            }
                            needs_redraw = true;
                        }
                        KeyCode::Up => {
                            let mut s = app_state.lock().await;
                            let completion_len = crate::app::get_completion_len(
                                &s.input_buffer,
                                s.cursor_position,
                            );
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
                            let completion_len = crate::app::get_completion_len(
                                &s.input_buffer,
                                s.cursor_position,
                            );
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
                            } else if crate::app::suggestion::command_token(&s.input_buffer)
                                .is_some()
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
                        KeyCode::Char('p') | KeyCode::Char('n')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            let mut s = app_state.lock().await;
                            let completion_len = crate::app::get_completion_len(
                                &s.input_buffer,
                                s.cursor_position,
                            );
                            if s.active_suggestion_index.is_some() && completion_len > 0 {
                                let current = s.active_suggestion_index.unwrap_or(0);
                                s.active_suggestion_index = Some(if key.code == KeyCode::Char('p') {
                                    if current == 0 { completion_len - 1 } else { current - 1 }
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
                            } else if c == '?'
                                && !ctrl
                                && !alt
                                && !cmd
                                && s.input_buffer.is_empty()
                            {
                                s.history.push(ChatMessage::new(
                                    "system",
                                    crate::app::build_help_text(),
                                ));
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
                Event::FocusGained => {
                    terminal_focused = true;
                    needs_redraw = true;
                }
                Event::FocusLost => {
                    terminal_focused = false;
                    needs_redraw = true;
                }
                // Native terminal mouse handling owns transcript scrolling and selection.
                Event::Mouse(_) => {}
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

    let exit_summary = {
        let s = app_state.lock().await;
        ExitSummary::from_state(&s)
    };

    // Shutdown: nothing queued may be lost, so write it out synchronously.
    crate::config::flush_history();

    disable_raw_mode()?;
    let transcript_end = exit_summary.composer_y.unwrap_or_else(|| {
        terminal
            .area()
            .y
            .saturating_add(terminal.area().height.saturating_sub(1))
    });
    execute!(
        terminal.backend_mut(),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
        SetCursorStyle::DefaultUserShape,
        crossterm::cursor::MoveTo(0, transcript_end),
        Clear(ClearType::FromCursorDown)
    )?;
    terminal.show_cursor()?;

    print_exit_summary(&exit_summary);

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExitSummary {
    prompt_tokens: u64,
    cached_tokens: u64,
    completion_tokens: u64,
    reasoning_tokens: u64,
    session_id: String,
    composer_y: Option<u16>,
}

impl ExitSummary {
    fn from_state(state: &AppState) -> Self {
        let mut summary = Self {
            prompt_tokens: 0,
            cached_tokens: 0,
            completion_tokens: 0,
            reasoning_tokens: 0,
            session_id: state.active_session_id.clone(),
            composer_y: state.input_text_area.map(|area| area.y),
        };

        for message in &state.history {
            if let Some(usage) = &message.token_usage {
                summary.prompt_tokens = summary
                    .prompt_tokens
                    .saturating_add(u64::from(usage.prompt_tokens));
                summary.cached_tokens = summary
                    .cached_tokens
                    .saturating_add(u64::from(usage.cached_tokens.unwrap_or(0)));
                summary.completion_tokens = summary
                    .completion_tokens
                    .saturating_add(u64::from(usage.completion_tokens));
            }
            summary.reasoning_tokens = summary
                .reasoning_tokens
                .saturating_add(u64::from(message.thought_tokens.unwrap_or(0)));
        }

        summary
    }

    fn usage_line(&self) -> Option<String> {
        let total = self.prompt_tokens.saturating_add(self.completion_tokens);
        if total == 0 {
            return None;
        }
        let cached = (self.cached_tokens > 0)
            .then(|| format!(" (+ {} cached)", format_number(self.cached_tokens)))
            .unwrap_or_default();
        let reasoning = (self.reasoning_tokens > 0)
            .then(|| format!(" (reasoning {})", format_number(self.reasoning_tokens)))
            .unwrap_or_default();
        Some(format!(
            "Token usage: total={} input={}{} output={}{}",
            format_number(total),
            format_number(self.prompt_tokens),
            cached,
            format_number(self.completion_tokens),
            reasoning,
        ))
    }
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

/// Printed after restoring the terminal and erasing the transient composer,
/// matching Codex's compact usage and resume handoff.
fn print_exit_summary(summary: &ExitSummary) {
    use std::io::Write;

    let mut out = std::io::stdout();
    if let Some(usage) = summary.usage_line() {
        let _ = writeln!(out, "{usage}");
    }
    if !summary.session_id.is_empty() {
        let _ = writeln!(out, "To continue this session, run rustcode --resume");
    }
}

#[cfg(test)]
mod draw_loop_tests {
    use super::{
        STREAM_FRAME_INTERVAL, background_task_history_message, ExitSummary, format_number,
        queue_background_wakeup, should_draw,
    };
    use std::time::Duration;

    #[test]
    fn exit_summary_formats_codex_style_usage() {
        let summary = ExitSummary {
            prompt_tokens: 2_249_608,
            cached_tokens: 60_154_240,
            completion_tokens: 132_560,
            reasoning_tokens: 48_884,
            session_id: "session-id".to_string(),
            composer_y: Some(12),
        };
        assert_eq!(
            summary.usage_line().as_deref(),
            Some(
                "Token usage: total=2,382,168 input=2,249,608 (+ 60,154,240 cached) output=132,560 (reasoning 48,884)"
            )
        );
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
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
    fn background_wakeup_waits_for_main_loop_to_use_current_cancel_token() {
        let mut state = crate::app::AppState::new();
        state.orchestrator_running = false;
        state.redraw_requested = false;

        queue_background_wakeup(&mut state, "task_42");

        assert_eq!(state.pending_queue, ["__task_wakeup__:task_42"]);
        assert!(!state.orchestrator_running);
        assert!(state.take_redraw_request());
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
                replayed: false,
                error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
                retryable: false,
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
                replayed: false,
                error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
                retryable: false,
            },
        );

        let metadata = message.tool_result.expect("background metadata");
        assert!(!metadata.success);
        assert_eq!(metadata.exit_code, Some(11));
        assert!(!metadata.truncated);
        assert_eq!(metadata.full_output_artifact, None);
    }
}
