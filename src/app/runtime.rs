use crate::app::{
    AppEvent, AppEventSender, AppState, AppStatus, ApprovalDecision, ChatMessage, QuestionAnswer,
    UpdateDecision, Verbosity,
};
use crate::network::{AgentUiEvent, AgentUiEventReceiver, AgentUiEventSender};
use crate::ui;
use crate::ui::{
    FrameRequester, FrameStream, TerminalRuntime, TranscriptState, TuiEvent, TuiEventStream,
};
use crossterm::{
    event::{self, KeyCode, KeyModifiers},
    execute,
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

async fn apply_approval_decision(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    decision: ApprovalDecision,
) {
    let approved = match decision {
        ApprovalDecision::Approve => true,
        ApprovalDecision::ApproveAll => {
            state.lock().await.auto_confirm = true;
            true
        }
        ApprovalDecision::Deny => false,
        // Custom approval payloads are reserved for a future policy that can
        // persist a reason. Until then, a non-empty custom response is an
        // affirmative decision and an empty one is a denial.
        ApprovalDecision::Custom(reason) => !reason.trim().is_empty(),
    };

    if !approved {
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
    }

    let mut state = state.lock().await;
    if let Some(tx) = state.tool_confirmation_response.take() {
        let _ = tx.send(approved);
    }
    state.pending_tool_confirmation = None;
    state.request_redraw();
}

async fn apply_question_answer(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    answer: QuestionAnswer,
) {
    let (answer, cancelled) = match answer {
        QuestionAnswer::Selected(answer) | QuestionAnswer::Custom(answer) => (answer, false),
        QuestionAnswer::Cancelled => {
            cancel_token.cancel();
            *cancel_token = CancellationToken::new();
            ("User cancelled prompt.".to_owned(), true)
        }
    };

    let mut state = state.lock().await;
    if let Some(tx) = state.question_response.take() {
        let _ = tx.send(answer);
    }
    state.pending_question = None;
    if cancelled {
        state.status = AppStatus::Idle;
    }
    state.request_redraw();
}

fn apply_update_decision(state: &mut AppState, decision: UpdateDecision) -> bool {
    let latest = match state.update_check {
        crate::update::UpdateState::Available(latest) => Some(latest),
        _ => None,
    };

    state.show_update_prompt = false;
    state.update_prompt_index = 0;
    if matches!(decision, UpdateDecision::SkipUntilNextVersion) {
        state.dismissed_update_version = latest;
    }
    state.request_redraw();

    matches!(decision, UpdateDecision::UpdateNow) && latest.is_some()
}

async fn run_update_command(
    terminal_runtime: &mut TerminalRuntime,
    expected_version: crate::update::Version,
) -> Result<(), String> {
    terminal_runtime
        .terminal()
        .clear_screen()
        .map_err(|error| format!("failed to clear the terminal before updating: {error}"))?;
    terminal_runtime
        .restore()
        .map_err(|error| format!("failed to restore the terminal before updating: {error}"))?;
    tokio::task::spawn_blocking(move || crate::update::run_brew_upgrade(expected_version))
        .await
        .map_err(|error| format!("update task error: {error}"))?
}

fn apply_session_event(
    state: &mut AppState,
    cancel_token: &mut CancellationToken,
    event: AppEvent,
) -> Result<(), AppError> {
    let controller = crate::app::session_controller::SessionController::default();
    let archive_only = matches!(&event, AppEvent::ArchiveSession);
    if !archive_only {
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
    }

    let transition = match event {
        AppEvent::NewSession => controller.start_fresh(state),
        AppEvent::ResumeSession(action) => controller.resume(state, action),
        AppEvent::ForkSession(action) => controller.fork(state, action),
        AppEvent::ClearSession => controller.clear(state),
        AppEvent::ArchiveSession => controller.archive(state),
        AppEvent::DeleteSession(action) => controller.delete(state, action),
        _ => return Err(AppError("not a session event".to_owned())),
    }
    .map_err(|error| AppError(error.to_string()))?;

    if !archive_only {
        state.show_history_picker = false;
        state.pending_delete_session_idx = None;
        state.history_picker_sessions.clear();
    }
    state.set_notice(format_session_transition(&transition));
    state.request_redraw();
    Ok(())
}

fn format_session_transition(
    transition: &crate::app::session_controller::SessionTransition,
) -> String {
    match transition {
        crate::app::session_controller::SessionTransition::Started { .. } => {
            "Started a new session".to_owned()
        }
        crate::app::session_controller::SessionTransition::Resumed { .. } => {
            "Resumed session".to_owned()
        }
        crate::app::session_controller::SessionTransition::Forked { .. } => {
            "Forked session".to_owned()
        }
        crate::app::session_controller::SessionTransition::Cleared { .. } => {
            "Cleared transcript view".to_owned()
        }
        crate::app::session_controller::SessionTransition::Archived { .. } => {
            "Archived session".to_owned()
        }
        crate::app::session_controller::SessionTransition::Deleted { .. } => {
            "Deleted session".to_owned()
        }
    }
}

fn open_overlay(state: &mut AppState, overlay: crate::app::events::Overlay) {
    if matches!(overlay, crate::app::events::Overlay::History) {
        let (sessions, truncated) = crate::app::actions::build_session_list_with_truncation(state);
        state.history_picker_sessions = sessions;
        state.history_picker_index = 0;
        state.history_picker_truncated = truncated;
    }
    if matches!(overlay, crate::app::events::Overlay::Subagents) {
        state.subagent_picker_index = 0;
    }
    state.overlays().open(overlay);
}

fn apply_subagent_selection(state: &mut AppState, id: u32) -> Result<(), AppError> {
    if id == 0 {
        crate::app::SubagentController.select_root(state);
    } else {
        crate::app::SubagentController
            .select(state, crate::app::SubagentId::from_raw(id))
            .map_err(|error| AppError(error.to_string()))?;
    }
    state.show_subagent_picker = false;
    state.request_redraw();
    Ok(())
}

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
    agent_ui_event_sender: AgentUiEventSender,
    agent_ui_event_receiver: AgentUiEventReceiver,
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
        let (agent_ui_event_sender, agent_ui_event_receiver) = AgentUiEventSender::channel();
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
            agent_ui_event_sender,
            agent_ui_event_receiver,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(app_state: AppState) -> Self {
        let (app_event_sender, app_event_receiver) = AppEventSender::channel();
        let (agent_ui_event_sender, agent_ui_event_receiver) = AgentUiEventSender::channel();
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
            agent_ui_event_sender,
            agent_ui_event_receiver,
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
                state.subagent_supervisor.shutdown();
                Ok(AppRunControl::Exit(crate::ExitSummary::from_state(&state)))
            }
            AppEvent::CloseOverlay => {
                let mut state = self.app_state.lock().await;
                state.overlays().close_all();
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::ApprovalDecision(decision) => {
                apply_approval_decision(
                    &self.app_state,
                    &mut self.current_cancel_token,
                    decision,
                )
                .await;
                Ok(AppRunControl::Continue)
            }
            AppEvent::AnswerQuestion(answer) => {
                apply_question_answer(
                    &self.app_state,
                    &mut self.current_cancel_token,
                    answer,
                )
                .await;
                Ok(AppRunControl::Continue)
            }
            AppEvent::UpdateDecision(decision) => {
                let mut state = self.app_state.lock().await;
                if apply_update_decision(&mut state, decision) {
                    state.update_requested = true;
                }
                Ok(AppRunControl::Continue)
            }
            AppEvent::OpenOverlay(overlay) => {
                let mut state = self.app_state.lock().await;
                open_overlay(&mut state, overlay);
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            event @ (AppEvent::NewSession
            | AppEvent::ResumeSession(_)
            | AppEvent::ForkSession(_)
            | AppEvent::ClearSession
            | AppEvent::ArchiveSession
            | AppEvent::DeleteSession(_)) => {
                let mut state = self.app_state.lock().await;
                apply_session_event(&mut state, &mut self.current_cancel_token, event)?;
                Ok(AppRunControl::Continue)
            }
            AppEvent::Tui(_) => Ok(AppRunControl::Continue),
            AppEvent::SelectSubagent(id) => {
                let mut state = self.app_state.lock().await;
                apply_subagent_selection(&mut state, id)?;
                Ok(AppRunControl::Continue)
            }
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
            agent_ui_event_sender,
            agent_ui_event_receiver,
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
        let mut agent_ui_event_receiver = agent_ui_event_receiver;
        let mut update_exit = false;
        let composer = ui::Composer::new();
        loop {
            let update_requested = {
                let mut state = app_state.lock().await;
                let requested = state.update_requested;
                state.update_requested = false;
                requested
            };
            if update_requested {
                let check = crate::update::check_for_update(&client).await;
                match check {
                    Ok(crate::update::UpdateCheck::UpToDate { current, latest }) => {
                        let mut state = app_state.lock().await;
                        state.update_check = crate::update::UpdateState::UpToDate(latest);
                        state.set_notice(format!(
                            "✨ RustCode v{} is up to date (latest: v{}).",
                            crate::update::format_version(current),
                            crate::update::format_version(latest)
                        ));
                        needs_redraw = true;
                        continue;
                    }
                    Ok(crate::update::UpdateCheck::Available { current, latest }) => {
                        {
                            let mut state = app_state.lock().await;
                            state.update_check = crate::update::UpdateState::Available(latest);
                            state.set_notice(format!(
                                "Found new release: v{} → v{}",
                                crate::update::format_version(current),
                                crate::update::format_version(latest)
                            ));
                        }
                        match run_update_command(&mut terminal_runtime, latest).await {
                            Ok(()) => println!(
                                "🎉 Update ran successfully! Please restart rustcode."
                            ),
                            Err(error) => eprintln!("Update failed: {error}"),
                        }
                        update_exit = true;
                        break;
                    }
                    Err(error) => {
                        let mut state = app_state.lock().await;
                        state.update_check = crate::update::UpdateState::Failed;
                        state.set_warning_notice(format!("Update check failed: {error}"));
                        needs_redraw = true;
                        continue;
                    }
                }
            }

            // Ratatui's inline viewport grows/shrinks by appending and clearing
            // terminal rows. When the terminal is resized, update the viewport
            // bounds and clear the live area so the active frame redraws cleanly.
            let observed_size = terminal_runtime.terminal().size()?;
            if observed_size != terminal_size {
                terminal_runtime.terminal().autoresize()?;
                if observed_size.width != terminal_size.width {
                    terminal_runtime.terminal().clear()?;
                }
                terminal_size = observed_size;
                needs_redraw = true;
            }

            let (response_active, background_redraw) = {
                let mut s = app_state.lock().await;
                (s.status_state().is_active(), s.take_redraw_request())
            };
            needs_redraw |= background_redraw;
            while let Ok(agent_event) = agent_ui_event_receiver.try_recv() {
                if matches!(&agent_event, AgentUiEvent::ApprovalRequested { .. }) {
                    let _ = app_event_sender.send(AppEvent::OpenOverlay(
                        crate::app::events::Overlay::ToolConfirmation,
                    ));
                }
                transcript_state.apply_agent_event(&agent_event);
                frame_requester.schedule_frame();
                needs_redraw = true;
            }
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
                    let ui_event_sender = agent_ui_event_sender.clone();
                    drop(s);
                    tokio::spawn(async move {
                        crate::network::process_queue_orchestrator_with_ui_events(
                            client_clone,
                            state_clone,
                            token_clone,
                            std::sync::Arc::new(crate::network::policy::InteractivePolicy),
                            ui_event_sender,
                        )
                        .await;
                    });
                    needs_redraw = true;
                }
            }

            let response_just_finished = was_responding && !response_active;
            if crate::app::status::should_notify_response_finished(
                response_just_finished,
                terminal_focused,
            ) {
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

                if guard.clear_screen_requested {
                    guard.clear_screen_requested = false;
                    terminal_runtime.terminal().clear_screen().ok();
                    transcript_cursor.reset();
                    transcript_cursor.commit_history_through(guard.history_display_start);
                    transcript_state.reset();
                    stream_commits.reset();
                }

                let terminal_width = terminal_runtime.terminal().size()?.width;
                let live_response = guard.transcript().live_response().to_owned();
                transcript_cursor.begin_stream(&live_response);
                let stable_source = transcript_cursor.pending_stable_source(&live_response);
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
                let history_range = transcript_cursor.pending_history_range(history_len);
                let stable_lines = stream_commits
                    .take_ready(!history_range.is_empty() || !response_active);
                if !stable_lines.is_empty() {
                    crate::insert_scrollback_lines(
                        terminal_runtime.terminal(),
                        stable_lines,
                        terminal_width,
                    )?;
                }
                let mut blocks = Vec::new();
                if crate::should_clear_mutable_viewport_before_history(
                    response_just_finished,
                    transcript_cursor.is_at_start(),
                    !history_range.is_empty(),
                ) {
                    // History is about to replace content in the mutable cell. Drop
                    // that old cell before insertion so working/status/composer rows
                    // cannot survive beneath the newly committed transcript.
                    terminal_runtime.terminal().draw_height(0, |_| {})?;
                }
                if transcript_cursor.is_at_start() && !history_range.is_empty() {
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
                            if !crate::ui::tool_result_needs_assistant_gap(
                                &guard.history,
                                indices.last().copied().unwrap_or(index),
                            ) {
                                block.push(ratatui::text::Line::from(""));
                            }
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
                AppEvent::ApprovalDecision(decision) => {
                    apply_approval_decision(&app_state, &mut current_cancel_token, decision).await;
                    needs_redraw = true;
                }
                AppEvent::AnswerQuestion(answer) => {
                    apply_question_answer(&app_state, &mut current_cancel_token, answer).await;
                    needs_redraw = true;
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
                        match run_update_command(&mut terminal_runtime, update_version).await {
                            Ok(()) => println!(
                                "🎉 Update ran successfully! Please restart rustcode."
                            ),
                            Err(error) => eprintln!("Update failed: {error}"),
                        }
                        update_exit = true;
                        break;
                    }
                    needs_redraw = true;
                }
                AppEvent::OpenOverlay(overlay) => {
                    let mut state = app_state.lock().await;
                    open_overlay(&mut state, overlay);
                    state.request_redraw();
                    needs_redraw = true;
                }
                event @ (AppEvent::NewSession
                | AppEvent::ResumeSession(_)
                | AppEvent::ForkSession(_)
                | AppEvent::ClearSession
                | AppEvent::ArchiveSession
                | AppEvent::DeleteSession(_)) => {
                    let mut state = app_state.lock().await;
                    if let Err(error) = apply_session_event(
                        &mut state,
                        &mut current_cancel_token,
                        event,
                    ) {
                        state.set_notice(error.to_string());
                        state.request_redraw();
                    }
                    needs_redraw = true;
                }
                AppEvent::CloseOverlay => {
                    let mut state = app_state.lock().await;
                    state.overlays().close_all();
                    state.request_redraw();
                    needs_redraw = true;
                }
                AppEvent::RequestDraw => {
                    app_state.lock().await.request_redraw();
                    needs_redraw = true;
                }
                AppEvent::SelectSubagent(id) => {
                    let mut state = app_state.lock().await;
                    if let Err(error) = apply_subagent_selection(&mut state, id) {
                        state.set_notice(error.to_string());
                    }
                    transcript_state.reset();
                    needs_redraw = true;
                }
                AppEvent::CancelActiveTurn => {
                    current_cancel_token.cancel();
                    current_cancel_token = CancellationToken::new();
                    let mut state = app_state.lock().await;
                    state.pending_queue.clear();
                    state.clear_live_tool_calls();
                    state.status = AppStatus::Idle;
                    state.request_redraw();
                    needs_redraw = true;
                }
                AppEvent::Tui(ev) => match ev {
                    TuiEvent::Key(key) => {
                        needs_redraw = true;
                        let is_ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
                        let is_cmd = key.modifiers.contains(event::KeyModifiers::SUPER);

                        if (is_ctrl || is_cmd)
                            && (key.code == KeyCode::Char('k') || key.code == KeyCode::Char('K'))
                        {
                            let mut s = app_state.lock().await;
                            s.request_clear_screen();
                            needs_redraw = true;
                            continue;
                        }
                        if is_ctrl
                            && (key.code == KeyCode::Char('l') || key.code == KeyCode::Char('L'))
                        {
                            let mut s = app_state.lock().await;
                            s.request_clear_screen();
                            needs_redraw = true;
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
                                        state.update_prompt_index =
                                            (state.update_prompt_index + 1).min(2);
                                    }
                                    KeyCode::Enter => {
                                        let decision = match selected {
                                            0 => UpdateDecision::UpdateNow,
                                            1 => UpdateDecision::Skip,
                                            _ => UpdateDecision::SkipUntilNextVersion,
                                        };
                                        let _ = app_event_sender
                                            .send(AppEvent::UpdateDecision(decision));
                                    }
                                    KeyCode::Esc => {
                                        let _ = app_event_sender
                                            .send(AppEvent::UpdateDecision(UpdateDecision::Skip));
                                    }
                                    _ => {}
                                }
                                continue;
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

                            if s.status == AppStatus::EffortPicker {
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
                                        s.subagent_picker_index =
                                            (s.subagent_picker_index + 1) % total;
                                    }
                                }
                                KeyCode::Enter => {
                                    let selected = s
                                        .subagent_picker_index
                                        .min(total.saturating_sub(1));
                                    let id = if selected == 0 {
                                        0
                                    } else {
                                        s.subagents[selected - 1].id
                                    };
                                    s.show_subagent_picker = false;
                                    drop(s);
                                    let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                                    continue;
                                }
                                _ => {}
                            }
                            drop(s);
                            continue;
                        }

                        if s.show_context_modal {
                            match key.code {
                                KeyCode::Esc
                                | KeyCode::Enter
                                | KeyCode::Char('q')
                                | KeyCode::Char('Q') => {
                                    s.show_context_modal = false;
                                }
                                _ => {}
                            }
                            drop(s);
                            continue;
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
                                continue;
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
                                            "/sync" => {
                                                crate::app::actions::trigger_sync(
                                                    &app_state, None, None,
                                                );
                                            }
                                            "/update" => {
                                                s.update_requested = true;
                                                s.update_check =
                                                    crate::update::UpdateState::Checking;
                                                s.set_notice("🔍 Checking for a RustCode update...");
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

        let mut exit_summary = {
            let s = app_state.lock().await;
            s.subagent_supervisor.shutdown();
            crate::ExitSummary::from_state(&s)
        };
        if update_exit {
            exit_summary.print_handoff = false;
        }
        crate::config::flush_history();
        terminal_runtime.restore_at(exit_summary.composer_y)?;
        Ok(exit_summary)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppRunControl, AppRuntime, apply_update_decision};
    use crate::app::{
        AppEvent, AppState, AppStatus, ApprovalDecision, PendingQuestion, QuestionAnswer,
        SessionAction, UpdateDecision,
    };

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

    #[test]
    fn update_prompt_decisions_close_prompt_and_remember_version() {
        let mut state = AppState::new();
        state.show_update_prompt = true;
        state.update_check = crate::update::UpdateState::Available((0, 30, 0));

        assert!(!apply_update_decision(&mut state, UpdateDecision::Skip));
        assert!(!state.show_update_prompt);

        state.show_update_prompt = true;
        assert!(!apply_update_decision(
            &mut state,
            UpdateDecision::SkipUntilNextVersion
        ));
        assert_eq!(state.dismissed_update_version, Some((0, 30, 0)));

        state.show_update_prompt = true;
        assert!(apply_update_decision(&mut state, UpdateDecision::UpdateNow));
        assert!(!state.show_update_prompt);
    }

    #[tokio::test]
    async fn session_events_are_applied_by_the_runtime_controller() {
        let mut state = AppState::new();
        let old_session = state.active_session_id.clone();
        state.history.push(crate::app::ChatMessage::new("user", "old"));
        let mut runtime = AppRuntime::for_test(state);

        runtime
            .handle_event(AppEvent::NewSession)
            .await
            .expect("new session event should be handled");

        let state = runtime.app_state().await;
        assert_ne!(state.active_session_id, old_session);
        assert!(!state.show_history_picker);
        assert!(state
            .history
            .iter()
            .any(|message| message.content == "Started a new session"));
    }

    #[tokio::test]
    async fn delete_session_event_rejects_invalid_ids_without_mutation() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        assert!(runtime
            .handle_event(AppEvent::DeleteSession(SessionAction::Id(
                "../escape".to_owned(),
            )))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn select_subagent_event_switches_context_without_mutating_parent_history() {
        let mut state = AppState::new();
        state
            .history
            .push(crate::app::ChatMessage::new("user", "parent"));
        let id = crate::app::SubagentController.spawn(
            &mut state,
            "child",
            None,
            None,
            false,
            Vec::new(),
            None,
            None,
        );
        let mut runtime = AppRuntime::for_test(state);

        runtime
            .handle_event(AppEvent::SelectSubagent(id.raw()))
            .await
            .expect("selection event should be handled");

        let state = runtime.app_state().await;
        assert_eq!(state.selected_subagent_id, Some(id.raw()));
        assert_eq!(state.history[0].content, "parent");
        assert!(!state.show_subagent_picker);
    }

    #[tokio::test]
    async fn exit_event_returns_a_summary_without_touching_the_terminal() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        assert!(matches!(
            runtime.handle_event(AppEvent::Exit).await,
            Ok(AppRunControl::Exit(_))
        ));
    }

    #[tokio::test]
    async fn approval_events_resolve_the_existing_policy_channel() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut state = AppState::new();
        state.status = AppStatus::AwaitingToolConfirmation;
        state.pending_tool_confirmation = Some(Vec::new());
        state.tool_confirmation_response = Some(tx);
        let mut runtime = AppRuntime::for_test(state);

        runtime
            .handle_event(AppEvent::ApprovalDecision(ApprovalDecision::ApproveAll))
            .await
            .expect("approval event should be handled");

        assert!(rx.await.expect("approval response"));
        let state = runtime.app_state().await;
        assert!(state.auto_confirm);
        assert!(state.pending_tool_confirmation.is_none());
    }

    #[tokio::test]
    async fn denying_approval_cancels_the_turn_before_resolving_it() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut state = AppState::new();
        state.status = AppStatus::AwaitingToolConfirmation;
        state.pending_tool_confirmation = Some(Vec::new());
        state.tool_confirmation_response = Some(tx);
        let mut runtime = AppRuntime::for_test(state);
        let previous_token = runtime.current_cancel_token.clone();

        runtime
            .handle_event(AppEvent::ApprovalDecision(ApprovalDecision::Deny))
            .await
            .expect("approval event should be handled");

        assert!(!rx.await.expect("approval response"));
        assert!(previous_token.is_cancelled());
    }

    #[tokio::test]
    async fn question_events_return_typed_answers_through_the_existing_channel() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut state = AppState::new();
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(PendingQuestion::new(
            "Where?".to_owned(),
            vec!["Here".to_owned()],
            false,
        ));
        state.question_response = Some(tx);
        let mut runtime = AppRuntime::for_test(state);

        runtime
            .handle_event(AppEvent::AnswerQuestion(QuestionAnswer::Custom(
                "somewhere else".to_owned(),
            )))
            .await
            .expect("question event should be handled");

        assert_eq!(rx.await.expect("question response"), "somewhere else");
        let state = runtime.app_state().await;
        assert!(state.pending_question.is_none());
    }

    #[tokio::test]
    async fn cancelling_a_question_returns_the_legacy_cancel_text() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut state = AppState::new();
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(PendingQuestion::new(
            "Where?".to_owned(),
            vec!["Here".to_owned()],
            false,
        ));
        state.question_response = Some(tx);
        let mut runtime = AppRuntime::for_test(state);
        let previous_token = runtime.current_cancel_token.clone();

        runtime
            .handle_event(AppEvent::AnswerQuestion(QuestionAnswer::Cancelled))
            .await
            .expect("question event should be handled");

        assert_eq!(rx.await.expect("question response"), "User cancelled prompt.");
        let state = runtime.app_state().await;
        assert_eq!(state.status, AppStatus::Idle);
        assert!(previous_token.is_cancelled());
    }
}
