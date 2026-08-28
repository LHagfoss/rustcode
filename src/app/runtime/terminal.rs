use super::*;

/// Start a new terminal transcript projection from the canonical history.
///
/// Session replacement and viewport resize both invalidate terminal-specific
/// progress. Keeping this operation shared ensures a loaded session cannot be
/// rendered as a continuation of the previous session's scrollback.
pub(super) fn reset_transcript_presentation(
    transcript_cursor: &mut crate::ui::scrollback::TranscriptCursor,
    transcript_state: &mut TranscriptState,
    stream_commits: &mut crate::ui::scrollback::StreamCommitQueue,
    replaying_transcript: &mut bool,
    history_display_start: usize,
) {
    transcript_cursor.reset();
    transcript_cursor.commit_history_through(history_display_start);
    transcript_state.reset();
    stream_commits.reset();
    *replaying_transcript = true;
}

/// Clear both the inline viewport and the terminal's native scrollback before
/// replaying a replacement transcript.
pub(super) fn clear_terminal_for_transcript_replacement(
    terminal_runtime: &mut TerminalRuntime,
) -> std::io::Result<()> {
    execute!(
        terminal_runtime.terminal().backend_mut(),
        crossterm::style::Print("\x1b[3J")
    )?;
    terminal_runtime.terminal().clear_screen()
}

pub(super) async fn handle_terminal_resize(
    terminal_runtime: &mut TerminalRuntime,
    app_state: &Arc<Mutex<AppState>>,
    terminal_size: &mut Size,
    transcript_cursor: &mut crate::ui::scrollback::TranscriptCursor,
    transcript_state: &mut TranscriptState,
    stream_commits: &mut crate::ui::scrollback::StreamCommitQueue,
    replaying_transcript: &mut bool,
) -> Result<bool, Box<dyn Error>> {
    let observed_size = terminal_runtime.terminal().size()?;
    if observed_size == *terminal_size {
        return Ok(false);
    }

    clear_terminal_for_transcript_replacement(terminal_runtime)?;
    *terminal_size = observed_size;
    let history_display_start = app_state.lock().await.history_display_start;
    reset_transcript_presentation(
        transcript_cursor,
        transcript_state,
        stream_commits,
        replaying_transcript,
        history_display_start,
    );
    Ok(true)
}

pub(super) fn notify_response_finished(terminal_runtime: &mut TerminalRuntime) {
    let _ = execute!(
        terminal_runtime.terminal().backend_mut(),
        crossterm::style::Print("\x1b]9;rustcode · response finished\x07\x07")
    );
}

pub(super) fn restore_terminal(
    terminal_runtime: &mut TerminalRuntime,
    composer_y: Option<u16>,
) -> std::io::Result<()> {
    terminal_runtime.restore_at(composer_y)
}

#[cfg(test)]
mod tests {
    use super::reset_transcript_presentation;
    use crate::ui::{
        TranscriptState,
        scrollback::{StreamCommitQueue, TranscriptCursor},
    };
    use ratatui::text::Line;

    #[test]
    fn transcript_reset_discards_old_cursor_cells_and_queued_stream_rows() {
        let mut cursor = TranscriptCursor::default();
        cursor.commit_history_through(3);
        let mut transcript = TranscriptState::default();
        transcript.set_assistant("session A", false, None, None, None);
        let mut commits = StreamCommitQueue::default();
        commits.push(vec![Line::from("session A stream")]);
        let mut replaying = false;

        reset_transcript_presentation(
            &mut cursor,
            &mut transcript,
            &mut commits,
            &mut replaying,
            0,
        );

        assert!(cursor.is_at_start());
        assert!(!cursor.has_committed_stream());
        assert_eq!(transcript.revision(), 0);
        assert_eq!(commits.pending_len(), 0);
        assert!(replaying);
    }
}
