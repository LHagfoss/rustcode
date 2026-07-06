mod app;
mod network;
mod ui;

use app::{AppState, AppStatus, ChatMessage};
use crossterm::{
    cursor::SetCursorStyle,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

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
    let mut current_cancel_token = CancellationToken::new();

    loop {
        let state_guard = app_state.lock().await;
        terminal.draw(|f| ui::render(f, &state_guard))?;
        drop(state_guard);

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Ctrl+C to force exit
                if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    break;
                }

                match key.code {
                    KeyCode::Esc => {
                        let mut state = app_state.lock().await;
                        state.reset_suggestion_cycle();

                        // Cancel active streaming
                        current_cancel_token.cancel();
                        current_cancel_token = CancellationToken::new();

                        // Or remove first item from queue if we have queued prompts and not streaming
                        if state.status != AppStatus::Streaming && !state.pending_queue.is_empty() {
                            state.pending_queue.remove(0);
                            if state.pending_queue.is_empty() {
                                state.status = AppStatus::Idle;
                            }
                        }
                    }
                    KeyCode::Tab => {
                        let mut state = app_state.lock().await;
                        state.cycle_suggestion();
                    }
                    KeyCode::Left => {
                        let mut state = app_state.lock().await;
                        if key.modifiers.contains(event::KeyModifiers::ALT) {
                            state.move_cursor_word_left();
                        } else {
                            state.move_cursor_left();
                        }
                    }
                    KeyCode::Right => {
                        let mut state = app_state.lock().await;
                        if key.modifiers.contains(event::KeyModifiers::ALT) {
                            state.move_cursor_word_right();
                        } else {
                            state.move_cursor_right();
                        }
                    }
                    KeyCode::Home => {
                        let mut state = app_state.lock().await;
                        state.move_cursor_to_start();
                    }
                    KeyCode::End => {
                        let mut state = app_state.lock().await;
                        state.move_cursor_to_end();
                    }
                    KeyCode::Enter => {
                        let mut state = app_state.lock().await;
                        state.reset_suggestion_cycle();
                        let raw_input = state.input_buffer.trim().to_string();

                        if raw_input.is_empty() {
                            continue;
                        }

                        if raw_input.starts_with('/') {
                            match raw_input.as_str() {
                                "/clear" | "/new" => {
                                    current_cancel_token.cancel();
                                    current_cancel_token = CancellationToken::new();
                                    state.history.clear();
                                    state.current_response.clear();
                                    state.pending_queue.clear();
                                    state.status = AppStatus::Idle;
                                    state.input_buffer.clear();
                                    state.move_cursor_to_start();
                                }
                                "/cancel" => {
                                    current_cancel_token.cancel();
                                    current_cancel_token = CancellationToken::new();
                                    state.input_buffer.clear();
                                    state.move_cursor_to_start();
                                }
                                "/help" => {
                                    state.input_buffer.clear();
                                    state.move_cursor_to_start();
                                    state.history.push(ChatMessage {
                                        role: "system".to_string(),
                                        content: "Available commands:\n  /help   - Show this help message\n  /clear  - Clear conversation history\n  /new    - Start a new conversation\n  /cancel - Cancel active streaming or queued prompt\n  /exit   - Quit the application\n  /quit   - Quit the application".to_string(),
                                        token_usage: None,
                                    });
                                }
                                "/exit" | "/quit" => break,
                                _ => {
                                    state.input_buffer.clear();
                                    state.move_cursor_to_start();
                                }
                            }
                            continue;
                        }

                        // Append to stream queue without locking input interaction capabilities
                        state.pending_queue.push(raw_input);
                        state.input_buffer.clear();
                        state.move_cursor_to_start();

                        if state.status == AppStatus::Idle {
                            state.status = AppStatus::Queued;
                            let client_clone = client.clone();
                            let state_clone = Arc::clone(&app_state);
                            let token_clone = current_cancel_token.clone();

                            drop(state);

                            tokio::spawn(async move {
                                network::process_queue_orchestrator(client_clone, state_clone, token_clone).await;
                            });
                        }
                    }
                    KeyCode::Char(c) => {
                        let mut state = app_state.lock().await;
                        if key.modifiers.contains(event::KeyModifiers::ALT) && c == 'b' {
                            state.move_cursor_word_left();
                        } else if key.modifiers.contains(event::KeyModifiers::ALT) && c == 'f' {
                            state.move_cursor_word_right();
                        } else if !key.modifiers.contains(event::KeyModifiers::CONTROL) && !key.modifiers.contains(event::KeyModifiers::ALT) {
                            state.insert_char(c);
                            state.reset_suggestion_cycle();
                        }
                    }
                    KeyCode::Backspace => {
                        let mut state = app_state.lock().await;
                        state.delete_char_backspace();
                        state.reset_suggestion_cycle();
                    }
                    KeyCode::Delete => {
                        let mut state = app_state.lock().await;
                        state.delete_char_delete();
                        state.reset_suggestion_cycle();
                    }
                    _ => {}
                }
            }
        }
    }

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
