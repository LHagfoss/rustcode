mod app;
mod config;
mod network;
mod ui;

use crate::app::{AppStatus, AppState};
use crossterm::{
    cursor::SetCursorStyle,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use tokio::sync::Mutex;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        SetCursorStyle::BlinkingBlock
    )?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app_state = Arc::new(Mutex::new(AppState::new()));
    let client = reqwest::Client::new();
    let mut current_cancel_token = tokio_util::sync::CancellationToken::new();

    loop {
        // Draw with state held briefly for consistency.
        {
            let guard = app_state.lock().await;
            terminal.draw(|f| ui::render(f, &guard))?;
        }

        if event::poll(std::time::Duration::from_millis(50))?
            && let Event::Key(key) = event::read()? {
                // Ctrl+C → hard exit.
                if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    break;
                }

                match key.code {
                    KeyCode::Esc => handle_escape(&app_state, &mut current_cancel_token).await,
                    KeyCode::Up => {
                        let mut s = app_state.lock().await;
                        s.history_up();
                    }
                    KeyCode::Down => {
                        let mut s = app_state.lock().await;
                        s.history_down();
                    }
                    KeyCode::Tab => {
                        let mut s = app_state.lock().await;
                        s.cycle_suggestion();
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
                    KeyCode::Enter => {
                        if handle_enter(&app_state, &client, &mut current_cancel_token).await {
                            break;
                        }
                    }
                    KeyCode::Char(c) => {
                        let mut s = app_state.lock().await;
                        // macOS Option+Left/Right → b/f with ALT modifier.
                        if key.modifiers.contains(event::KeyModifiers::ALT) && c == 'b' {
                            s.move_cursor_word_left();
                        } else if key.modifiers.contains(event::KeyModifiers::ALT) && c == 'f' {
                            s.move_cursor_word_right();
                        } else if !key.modifiers.contains(event::KeyModifiers::CONTROL)
                            && !key.modifiers.contains(event::KeyModifiers::ALT)
                        {
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
    }

    // Restore terminal state on exit.
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape
    )?;
    terminal.show_cursor()?;

    Ok(())
}

/// Escape: cancel active stream / dequeue next queued prompt, and clear input field.
async fn handle_escape(state: &Arc<Mutex<AppState>>, cancel_token: &mut tokio_util::sync::CancellationToken) {
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
    let raw_input = s.input_buffer.trim().to_string();

    if raw_input.is_empty() {
        return false;
    }

    if raw_input.starts_with('/') {
        let should_exit = match raw_input.as_str() {
            "/clear" | "/new" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                s.history.clear();
                s.current_response.clear();
                s.pending_queue.clear();
                s.status = AppStatus::Idle;
                false
            }
            "/cancel" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                false
            }
            "/help" => {
                s.history.push(app::ChatMessage::new(
                    "system",
                    "Available commands:\n  /help   - Show this help message\n  /clear  - Clear conversation history\n  /new    - Start a new conversation\n  /cancel - Cancel active streaming or queued prompt\n  /exit   - Quit the application\n  /quit   - Quit the application",
                ));
                false
            }
            "/exit" | "/quit" => true,
            _ => false,
        };
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
