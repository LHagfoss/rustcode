use crate::app::AppState;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::terminal::{clear_terminal_for_transcript_replacement, reset_transcript_presentation};
use super::transcript::commit_transcript;
use crate::ui::{TerminalRuntime, TranscriptState};

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

    let title = crate::config::load_session_title(&session_id);
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
    pub app_state: &'a Arc<Mutex<AppState>>,
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
        app_state,
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
        let custom_title = (guard.active_session_id == title_session_id)
            .then_some(loaded_title)
            .flatten()
            .or_else(|| {
                guard
                    .history
                    .iter()
                    .find(|m| m.role == "user" && !m.content.starts_with('/'))
                    .map(|m| m.content.lines().next().unwrap_or("").trim().to_string())
            });
        let snapshot = guard.render_snapshot();
        let activity =
            crate::app::activity::classify_activity(snapshot.status(), snapshot.running_tools());
        let animation_frame = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            / 100;
        let session_name = custom_title
            .filter(|title| !title.is_empty() && !title.starts_with('/'))
            .unwrap_or_else(|| "session".to_string());
        let title_display = crate::app::activity::format_terminal_title(
            activity.kind,
            &session_name,
            animation_frame,
        );
        let old_title = guard.current_terminal_title.clone();
        if old_title.as_deref() != Some(title_display.as_str()) {
            guard.current_terminal_title = Some(title_display.clone());
        }

        let progress = crate::app::activity::terminal_progress_for_activity(activity.kind);
        let should_send_progress = guard.current_terminal_progress != Some(progress)
            || (progress != crate::app::activity::TerminalProgress::Hidden
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
    app_state
        .lock()
        .await
        .publish_render_metrics(snapshot.revision(), content_height, input_area);
    Ok(())
}
