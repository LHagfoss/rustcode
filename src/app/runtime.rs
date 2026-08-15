use crate::app::{AppEvent, AppEventSender, AppState, AppStatus, ChatMessage, Verbosity};
use crate::ui;
use crate::ui::{
    FrameRequester, FrameStream, TerminalRuntime, TranscriptState, TuiEvent, TuiEventStream,
};
use crossterm::{
    event::{self, KeyCode, KeyModifiers},
    execute,
    terminal::{Clear, ClearType},
};
use ratatui::layout::Size;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, MutexGuard, mpsc};
use tokio_util::sync::CancellationToken;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const STREAM_FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub(crate) struct AppRuntime {
    terminal_runtime: Option<TerminalRuntime>,
    app_state: Arc<Mutex<AppState>>,
    client: reqwest::Client,
    current_cancel_token: CancellationToken,
    needs_redraw: bool,
    was_responding: bool,
    terminal_focused: bool,
    transcript_cursor: crate::ui::scrollback::TranscriptCursor,
    transcript_state: TranscriptState,
    stream_commits: crate::ui::scrollback::StreamCommitQueue,
    terminal_size: Size,
    tui_events: TuiEventStream,
    frame_requester: FrameRequester,
    frame_stream: FrameStream,
    app_event_sender: AppEventSender,
    app_event_receiver: mpsc::UnboundedReceiver<AppEvent>,
    replay_history: bool,
}

#[derive(Debug)]
pub(crate) struct AppError(String);

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for AppError {}

#[allow(dead_code)]
pub(crate) enum AppRunControl {
    Continue,
    Exit(crate::ExitSummary),
}

impl AppRuntime {
    pub(crate) fn new(
        mut terminal_runtime: TerminalRuntime,
        app_state: Arc<Mutex<AppState>>,
        client: reqwest::Client,
    ) -> Result<Self, Box<dyn Error>> {
        let terminal_size = terminal_runtime.terminal().size()?;
        let (app_event_sender, app_event_receiver) = AppEventSender::channel();
        let (frame_requester, frame_stream) = FrameRequester::new(STREAM_FRAME_INTERVAL);

        Ok(Self {
            terminal_runtime: Some(terminal_runtime),
            app_state,
            client,
            current_cancel_token: CancellationToken::new(),
            needs_redraw: true,
            was_responding: false,
            terminal_focused: true,
            transcript_cursor: crate::ui::scrollback::TranscriptCursor::default(),
            transcript_state: TranscriptState::default(),
            stream_commits: crate::ui::scrollback::StreamCommitQueue::default(),
            terminal_size,
            tui_events: TuiEventStream::new(),
            frame_requester,
            frame_stream,
            app_event_sender,
            app_event_receiver,
            replay_history: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(app_state: AppState) -> Self {
        let (app_event_sender, app_event_receiver) = AppEventSender::channel();
        let (frame_requester, frame_stream) = FrameRequester::new(STREAM_FRAME_INTERVAL);
        Self {
            terminal_runtime: None,
            app_state: Arc::new(Mutex::new(app_state)),
            client: reqwest::Client::new(),
            current_cancel_token: CancellationToken::new(),
            needs_redraw: false,
            was_responding: false,
            terminal_focused: true,
            transcript_cursor: crate::ui::scrollback::TranscriptCursor::default(),
            transcript_state: TranscriptState::default(),
            stream_commits: crate::ui::scrollback::StreamCommitQueue::default(),
            terminal_size: Size::new(80, 24),
            tui_events: TuiEventStream::paused(),
            frame_requester,
            frame_stream,
            app_event_sender,
            app_event_receiver,
            replay_history: false,
        }
    }

    pub(crate) async fn app_state(&self) -> MutexGuard<'_, AppState> {
        self.app_state.lock().await
    }

    pub(crate) async fn handle_event(
        &mut self,
        event: AppEvent,
    ) -> Result<AppRunControl, AppError> {
        match event {
            AppEvent::RequestDraw | AppEvent::Tui(TuiEvent::Draw) => {
                self.app_state.lock().await.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::SubmitPrompt(prompt) => {
                let mut state = self.app_state.lock().await;
                state.composer().replace_input(prompt);
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::CancelActiveTurn => {
                self.current_cancel_token.cancel();
                self.current_cancel_token = CancellationToken::new();
                let mut state = self.app_state.lock().await;
                state.pending_queue.clear();
                state.clear_live_tool_calls();
                state.status = AppStatus::Idle;
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::Exit => {
                let state = self.app_state.lock().await;
                Ok(AppRunControl::Exit(crate::ExitSummary::from_state(&state)))
            }
            AppEvent::CloseOverlay => {
                let mut state = self.app_state.lock().await;
                state.overlays().close_all();
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::Tui(_) => Ok(AppRunControl::Continue),
            AppEvent::ApprovalDecision(_)
            | AppEvent::OpenOverlay(_)
            | AppEvent::NewSession
            | AppEvent::ResumeSession(_)
            | AppEvent::ForkSession(_)
            | AppEvent::SelectSubagent(_) => Ok(AppRunControl::Continue),
        }
    }

    pub(crate) async fn run(self) -> Result<crate::ExitSummary, Box<dyn Error>> {
        let AppRuntime {
            terminal_runtime,
            app_state,
            client,
            current_cancel_token,
            needs_redraw,
            was_responding,
            terminal_focused,
            transcript_cursor,
            transcript_state,
            stream_commits,
            terminal_size,
            tui_events,
            frame_requester,
            frame_stream,
            app_event_sender,
            app_event_receiver,
            replay_history,
        } = self;
        let mut terminal_runtime = terminal_runtime
            .ok_or_else(|| Box::<dyn Error>::from("interactive terminal is unavailable"))?;
        let mut current_cancel_token = current_cancel_token;
        let mut needs_redraw = needs_redraw;
        let mut was_responding = was_responding;
        let mut terminal_focused = terminal_focused;
        let mut transcript_cursor = transcript_cursor;
        let mut transcript_state = transcript_state;
        let mut stream_commits = stream_commits;
        let mut terminal_size = terminal_size;
        let mut tui_events = tui_events;
        let mut frame_stream = frame_stream;
        let mut app_event_receiver = app_event_receiver;
        let mut replay_history = replay_history;
        let composer = ui::Composer::new();
        loop {
            // Ratatui's inline viewport grows/shrinks by appending and clearing
            // terminal rows. Once scrollback has been emitted those rows cannot be
            // reflowed in place, so a resize must purge the visible transcript and
            // replay it from the durable history plus the current stream.
            let observed_size = terminal_runtime.terminal().size()?;
            if observed_size != terminal_size {
                terminal_runtime.terminal().autoresize()?;
                if observed_size.width != terminal_size.width {
                    execute!(
                        terminal_runtime.terminal().backend_mut(),
                        Clear(ClearType::Purge),
                        Clear(ClearType::All)
                    )?;
                    terminal_runtime.terminal().clear()?;
                    transcript_cursor.reset();
                    transcript_state.reset();
                    stream_commits.reset();
                    replay_history = true;
                }
                terminal_size = observed_size;
                needs_redraw = true;
            }

            let (response_active, background_redraw) = {
                let mut s = app_state.lock().await;
                (s.status_state().is_active(), s.take_redraw_request())
            };
            needs_redraw |= background_redraw;
            if response_active {
                frame_requester.schedule_frame();
            }

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

            let response_just_finished = was_responding && !response_active;
            if response_just_finished && !terminal_focused {
                use crossterm::style::Print;
                let _ = execute!(
                    terminal_runtime.terminal().backend_mut(),
                    Print("\x1b]9;rustcode · response finished\x07\x07")
                );
            }
            was_responding = response_active;
            let should_draw = needs_redraw || frame_stream.try_next().is_some();

            if should_draw {
                let mut guard = app_state.lock().await;

                let terminal_width = terminal_runtime.terminal().size()?.width;
                let live_response = guard.transcript().live_response().to_owned();
                transcript_cursor.begin_stream(&live_response);
                let stable_source = if replay_history {
                    String::new()
                } else {
                    transcript_cursor.pending_stable_source(&live_response)
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

                let history_len = guard.transcript().history_len();
                let history_range = if replay_history {
                    0..history_len
                } else {
                    transcript_cursor.pending_history_range(history_len)
                };
                let stable_lines = stream_commits
                    .take_ready(replay_history || !history_range.is_empty() || !response_active);
                if !stable_lines.is_empty() {
                    crate::insert_scrollback_lines(
                        terminal_runtime.terminal(),
                        stable_lines,
                        terminal_width,
                    )?;
                }
                let mut blocks = Vec::new();
                if crate::should_clear_mutable_viewport_before_history(
                    replay_history,
                    response_just_finished,
                    transcript_cursor.is_at_start(),
                    !history_range.is_empty(),
                ) {
                    // History is about to replace content in the mutable cell. Drop
                    // that old cell before insertion so working/status/composer rows
                    // cannot survive beneath the newly committed transcript.
                    terminal_runtime.terminal().draw_height(0, |_| {})?;
                }
                if !replay_history && transcript_cursor.is_at_start() && !history_range.is_empty() {
                    let banner =
                        crate::ui::build_claude_startup_banner(&guard, terminal_width as usize, 24);
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
                        let block = crate::ui::render_committed_history_block(
                            &guard,
                            index,
                            terminal_width,
                        );
                        if !block.is_empty() {
                            blocks.push(block);
                        }
                    }
                    index += 1;
                }
                for lines in blocks {
                    crate::insert_scrollback_lines(
                        terminal_runtime.terminal(),
                        lines,
                        terminal_width,
                    )?;
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
                        crate::insert_scrollback_lines(
                            terminal_runtime.terminal(),
                            lines,
                            terminal_width,
                        )?;
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
                        terminal_runtime.terminal().backend_mut(),
                        Print(format!("\x1b]0;{}\x07", title_display))
                    );
                    guard.current_terminal_title = Some(title_display.clone());
                }

                let terminal_height = terminal_runtime.terminal().size()?.height;
                let desired_height = ui::desired_height(
                    &guard,
                    &mut transcript_state,
                    terminal_width,
                    terminal_height,
                );
                terminal_runtime
                    .terminal()
                    .draw_height(desired_height, |f| {
                        ui::render_with_transcript(f, &mut guard, &mut transcript_state)
                    })?;
                replay_history = false;
                drop(guard);
                needs_redraw = false;
            }

            if let Ok(event_result) =
                tokio::time::timeout(EVENT_POLL_INTERVAL, tui_events.next()).await
            {
                let Some(ev) = event_result? else {
                    continue;
                };
                let _ = app_event_sender.send(AppEvent::Tui(ev));
            }

            let Some(app_event) = app_event_receiver.try_recv().ok() else {
                continue;
            };
            match app_event {
                AppEvent::Tui(ev) => match ev {
                    TuiEvent::Key(key) => {
                        needs_redraw = true;
                        let is_ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                        let is_cmd = key.modifiers.contains(event::KeyModifiers::SUPER);

                        if (is_ctrl || is_cmd)
                            && (key.code == KeyCode::Char('k') || key.code == KeyCode::Char('K'))
                        {
                            terminal_runtime.terminal().clear().ok();
                            continue;
                        }
                        if is_ctrl
                            && (key.code == KeyCode::Char('l') || key.code == KeyCode::Char('L'))
                        {
                            terminal_runtime.terminal().clear().ok();
                            continue;
                        }

                        if is_ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
                            if crate::app::handle_ctrl_c(&app_state, &mut current_cancel_token)
                                .await
                            {
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
                                            if key
                                                .modifiers
                                                .contains(event::KeyModifiers::CONTROL)
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
                                            if key
                                                .modifiers
                                                .contains(event::KeyModifiers::CONTROL) =>
                                        {
                                            let mut s = app_state.lock().await;
                                            if let Some(q) = s.pending_question.as_mut() {
                                                q.move_cursor_home();
                                            }
                                        }
                                        KeyCode::Char('e') | KeyCode::Char('E')
                                            if key
                                                .modifiers
                                                .contains(event::KeyModifiers::CONTROL) =>
                                        {
                                            let mut s = app_state.lock().await;
                                            if let Some(q) = s.pending_question.as_mut() {
                                                q.move_cursor_end();
                                            }
                                        }
                                        KeyCode::Char('w') | KeyCode::Char('W')
                                            if key
                                                .modifiers
                                                .contains(event::KeyModifiers::CONTROL) =>
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
                                                if key.modifiers.contains(event::KeyModifiers::ALT)
                                                {
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
                                                q.options
                                                    .get(q.selected)
                                                    .cloned()
                                                    .unwrap_or_default()
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
                                        s.modal_picker_index =
                                            s.modal_picker_index.saturating_sub(1);
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
                                        s.modal_picker_index =
                                            s.modal_picker_index.saturating_sub(1);
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
                                        s.modal_picker_index =
                                            s.modal_picker_index.saturating_sub(1);
                                    }
                                    KeyCode::Down => {
                                        let mut s = app_state.lock().await;
                                        s.modal_picker_index =
                                            s.modal_picker_index.saturating_add(1).min(2); // 0 json, 1 native, 2 apinative
                                    }
                                    KeyCode::Enter => {
                                        let mut s = app_state.lock().await;
                                        let (protocol, label) = match s.modal_picker_index {
                                            0 => (
                                                crate::config::ToolProtocol::Json,
                                                "JSON (```tool)",
                                            ),
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
                                        s.history_picker_index =
                                            if s.history_picker_index + 1 >= len {
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
                                    if let Some(meta) = s.history_picker_sessions.get(idx).cloned()
                                    {
                                        crate::app::load_session_into(&mut s, &meta);
                                        let title_display =
                                            meta.title.replace('|', "\\|").replace('\x07', "");
                                        let _ = execute!(
                                            terminal_runtime.terminal().backend_mut(),
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
                                                    crate::mcp::start_server_by_name(&name_clone)
                                                        .await;
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
                                                    let _ = crate::mcp::start_server_by_name(
                                                        &name_clone,
                                                    )
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
                            let filtered_items: Vec<&crate::ui::PaletteItem> =
                                crate::ui::PALETTE_ITEMS
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
                                        s.command_picker_index =
                                            if s.command_picker_index + 1 >= len {
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
                                                        out.push_str(&format!(
                                                            "  • {}\n",
                                                            skill.name
                                                        ));
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
                                                s.history
                                                    .push(ChatMessage::new("assistant", log_text));
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

                        match {
                            let mut state = app_state.lock().await;
                            composer.handle_key(&mut state, key)
                        } {
                            ui::ComposerAction::Handled => {
                                needs_redraw = true;
                                continue;
                            }
                            ui::ComposerAction::Submit => {
                                if crate::app::handle_enter(
                                    &app_state,
                                    &client,
                                    &mut current_cancel_token,
                                )
                                .await
                                {
                                    break;
                                }
                                needs_redraw = true;
                                continue;
                            }
                            ui::ComposerAction::ClearScreen => {
                                terminal_runtime.terminal().clear()?;
                                continue;
                            }
                            ui::ComposerAction::Paste => {
                                if let Some(img_markdown) =
                                    crate::clipboard::paste_image_from_clipboard()
                                {
                                    let mut state = app_state.lock().await;
                                    composer.handle_paste(&mut state, &img_markdown);
                                } else if let Some(text) =
                                    crate::clipboard::read_text_from_clipboard()
                                {
                                    let mut state = app_state.lock().await;
                                    composer.handle_paste(&mut state, &text);
                                }
                                needs_redraw = true;
                                continue;
                            }
                            ui::ComposerAction::Cancel | ui::ComposerAction::Unhandled => {}
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
                                    crate::app::handle_escape(
                                        &app_state,
                                        &mut current_cancel_token,
                                    )
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
                                        let pulled =
                                            s.history_index.is_none() && s.pop_queued_prompt();
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
                                    s.active_suggestion_index =
                                        Some(if current + 1 >= completion_len {
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
                                let has_at = crate::app::get_at_word_query(
                                    &s.input_buffer,
                                    s.cursor_position,
                                )
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
                                } else if let Some(text) =
                                    crate::clipboard::read_text_from_clipboard()
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
                                    s.active_suggestion_index =
                                        Some(if key.code == KeyCode::Char('p') {
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
                    TuiEvent::FocusGained => {
                        terminal_focused = true;
                        needs_redraw = true;
                    }
                    TuiEvent::FocusLost => {
                        terminal_focused = false;
                        needs_redraw = true;
                    }
                    TuiEvent::Paste(text) => {
                        // Terminals with bracketed paste enabled deliver Cmd+V through
                        // this event instead of the Char('v') key handler. When the
                        // clipboard holds an image (e.g. a screenshot), the pasted text
                        // is empty — fall back to grabbing the image so it still turns
                        // into an `![image](file://…)` marker that renders as [Image #N].
                        if text.trim().is_empty()
                            && let Some(img_markdown) =
                                crate::clipboard::paste_image_from_clipboard()
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
                        needs_redraw = true;
                    }
                    TuiEvent::Resize { .. } => {
                        needs_redraw = true;
                    }
                    TuiEvent::Draw => {
                        needs_redraw = true;
                    }
                },
                _ => {}
            }
        }

        let exit_summary = {
            let s = app_state.lock().await;
            crate::ExitSummary::from_state(&s)
        };
        crate::config::flush_history();
        terminal_runtime.restore_at(exit_summary.composer_y)?;
        Ok(exit_summary)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppRunControl, AppRuntime};
    use crate::app::{AppEvent, AppState};

    #[tokio::test]
    async fn request_draw_and_submit_are_handled_by_the_runtime() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        assert!(matches!(
            runtime.handle_event(AppEvent::RequestDraw).await,
            Ok(AppRunControl::Continue)
        ));
        assert!(matches!(
            runtime
                .handle_event(AppEvent::SubmitPrompt("hello".to_string()))
                .await,
            Ok(AppRunControl::Continue)
        ));

        let state = runtime.app_state().await;
        assert_eq!(state.input_buffer, "hello");
        assert!(state.redraw_requested);
    }

    #[tokio::test]
    async fn exit_event_returns_a_summary_without_touching_the_terminal() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        assert!(matches!(
            runtime.handle_event(AppEvent::Exit).await,
            Ok(AppRunControl::Exit(_))
        ));
    }
}
