use crate::ui::render_snapshot::render_snapshot;
use rustcode::app::AppState;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::terminal::{clear_terminal_for_transcript_replacement, reset_transcript_presentation};
use super::transcript::commit_transcript;
use crate::ui::{FrameRequester, TerminalRuntime, TranscriptState};

fn step_selection_for_frame(transcript_state: &mut TranscriptState, frames: &FrameRequester) {
    if transcript_state.step_selection_scroll() {
        frames.schedule_frame();
    }
}

pub(super) async fn session_title_for_render(
    state: &Arc<Mutex<AppState>>,
) -> (String, Option<String>) {
    let (session_id, generation, cached_title) = {
        let guard = state.lock().await;
        (
            guard.active_session_id.clone(),
            guard.session_title_cache_generation,
            guard.cached_session_title(),
        )
    };
    if let Some(title) = cached_title {
        return (session_id, title);
    }

    let title = rustcode::config::load_session_title(&session_id);
    let mut guard = state.lock().await;
    if guard.install_session_title_cache(&session_id, generation, title.clone()) {
        return (session_id, title);
    }
    let current_session_id = guard.active_session_id.clone();
    let current_title = guard.cached_session_title().flatten();
    (current_session_id, current_title)
}

pub(super) struct RenderFrameContext<'a> {
    pub terminal_runtime: &'a mut TerminalRuntime,
    pub frame_requester: &'a FrameRequester,
    pub app_state: &'a Arc<Mutex<AppState>>,
    pub discord_rpc: &'a rustcode::discord_rpc::DiscordRpcWorker,
    pub transcript_cursor: &'a mut crate::ui::scrollback::TranscriptCursor,
    pub transcript_state: &'a mut TranscriptState,
    pub stream_commits: &'a mut crate::ui::scrollback::StreamCommitQueue,
    pub replaying_transcript: &'a mut bool,
    pub response_active: bool,
    pub response_just_finished: bool,
    pub last_progress_sent: &'a mut std::time::Instant,
}

pub(super) async fn render_frame(
    context: RenderFrameContext<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let RenderFrameContext {
        terminal_runtime,
        frame_requester,
        app_state,
        discord_rpc,
        transcript_cursor,
        transcript_state,
        stream_commits,
        replaying_transcript,
        response_active,
        response_just_finished,
        last_progress_sent,
    } = context;
    let (
        snapshot,
        terminal_width,
        terminal_height,
        clear_screen,
        clear_history_display_start,
        title_display,
        old_title,
        progress,
        should_send_progress,
    ) = {
        let (title_session_id, loaded_title) = session_title_for_render(app_state).await;
        let mut guard = app_state.lock().await;
        let terminal_size = terminal_runtime.terminal().size()?;
        let clear_screen = guard.clear_screen_requested;
        if clear_screen {
            guard.clear_screen_requested = false;
        }
        let clear_history_display_start = guard.history_display_start;
        // Keep the cached location fresh even when the next frame was
        // requested by another state change (for example a completed tool).
        // The cache still debounces Git discovery; this only makes the frame
        // boundary the final source of truth for the footer and welcome panel.
        guard.refresh_workspace_location(std::time::Instant::now());
        let generated_title = (guard.active_session_id == title_session_id)
            .then_some(loaded_title)
            .flatten();
        let prompt_title = guard
            .history
            .iter()
            .find(|m| m.role == "user" && !m.content.starts_with('/'))
            .map(|_| rustcode::config::session_title(&guard.history));
        let snapshot = render_snapshot(&rustcode::controller::render_state(&guard));
        let activity =
            rustcode::app::activity::classify_activity(snapshot.status(), snapshot.running_tools());
        let terminal_title = generated_title.clone().or(prompt_title);
        let session_name = terminal_title
            .as_deref()
            .filter(|title| !title.is_empty() && !title.starts_with('/'))
            .unwrap_or("session");
        let presence_title = generated_title
            .as_deref()
            .filter(|title| !title.is_empty() && !title.starts_with('/'))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let workspace = guard
                    .workspace_root
                    .as_deref()
                    .or(guard.task_working_directory.as_deref())
                    .map(std::path::Path::to_path_buf)
                    .or_else(|| std::env::current_dir().ok());
                rustcode::discord_rpc::workspace_basename(workspace.as_deref())
            });
        let title_display = rustcode::app::activity::format_terminal_title(
            rustcode::controller::ActivityKind::Ready,
            session_name,
            0,
        );
        let old_title = guard.current_terminal_title.clone();
        if old_title.as_deref() != Some(title_display.as_str()) {
            guard.current_terminal_title = Some(title_display.clone());
        }

        let progress = rustcode::app::activity::terminal_progress_for_activity(activity.kind);
        discord_rpc.update(
            rustcode::discord_rpc::DiscordPresence::from_activity_with_usage(
                &activity,
                &presence_title,
                snapshot.current_token_usage(),
            ),
        );
        let should_send_progress = guard.current_terminal_progress != Some(progress)
            || (progress != rustcode::app::activity::TerminalProgress::Hidden
                && last_progress_sent.elapsed() >= std::time::Duration::from_secs(3));
        if should_send_progress {
            guard.current_terminal_progress = Some(progress);
        }

        (
            snapshot,
            terminal_size.width,
            terminal_size.height,
            clear_screen,
            clear_history_display_start,
            title_display,
            old_title,
            progress,
            should_send_progress,
        )
    };

    if clear_screen {
        clear_terminal_for_transcript_replacement(terminal_runtime).ok();
        reset_transcript_presentation(
            transcript_cursor,
            transcript_state,
            stream_commits,
            replaying_transcript,
            clear_history_display_start,
        );
    }

    commit_transcript(
        terminal_runtime,
        &snapshot,
        transcript_cursor,
        stream_commits,
        replaying_transcript,
        terminal_width,
        response_active,
        response_just_finished,
    )?;

    if old_title.as_deref() != Some(title_display.as_str()) {
        use crossterm::{execute, style::Print};
        let _ = execute!(
            terminal_runtime.terminal().backend_mut(),
            Print(format!("\x1b]0;{}\x07", title_display))
        );
    }
    if should_send_progress {
        use crossterm::{execute, style::Print};
        let _ = execute!(
            terminal_runtime.terminal().backend_mut(),
            Print(progress.osc_sequence())
        );
        *last_progress_sent = std::time::Instant::now();
    }

    step_selection_for_frame(transcript_state, frame_requester);
    let desired_height = crate::ui::desired_height_snapshot(
        &snapshot,
        transcript_state,
        terminal_width,
        terminal_height,
    );
    let mut frame_metrics = None;
    terminal_runtime
        .terminal()
        .draw_height(desired_height, |f| {
            frame_metrics = Some(crate::ui::render_with_transcript_snapshot(
                f,
                &snapshot,
                transcript_state,
            ));
        })?;
    let (content_height, input_area) =
        frame_metrics.expect("render_with_transcript_snapshot must run once");
    app_state.lock().await.publish_render_metrics(
        snapshot.revision(),
        content_height,
        rustcode::app::UiRect::new(
            input_area.x,
            input_area.y,
            input_area.width,
            input_area.height,
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline_terminal::InlineTerminal;
    use crate::ui::render_snapshot::render_snapshot;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::backend::TestBackend;
    use rustcode::app::ChatMessage;
    use std::time::Duration;

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[tokio::test]
    async fn frame_path_advances_edge_drag_and_requests_follow_up_frame() {
        let mut state = AppState::new();
        state.history.push(ChatMessage::new(
            "assistant",
            (0..40)
                .map(|row| format!("history row {row:02}"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        ));
        let snapshot = render_snapshot(&rustcode::controller::render_state(&state));
        let mut transcript = TranscriptState::default();
        let mut terminal = InlineTerminal::new(TestBackend::new(32, 14)).unwrap();
        terminal
            .draw(|frame| {
                crate::ui::render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
            })
            .unwrap();
        let area = transcript.selection.area();
        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 1,
            ),
            render_snapshot(&rustcode::controller::render_state(&state)),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));

        let (frames, mut draws) = FrameRequester::new(Duration::from_millis(1));
        step_selection_for_frame(&mut transcript, &frames);
        terminal
            .draw(|frame| {
                crate::ui::render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
            })
            .unwrap();

        assert_eq!(transcript.scroll_rows(), 1);
        assert!(transcript.selection.selected_text().is_some());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), draws.next())
                .await
                .unwrap(),
            Some(crate::ui::TuiEvent::Draw)
        );
    }
}
