use super::*;

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

    execute!(
        terminal_runtime.terminal().backend_mut(),
        crossterm::style::Print("\x1b[3J")
    )?;
    terminal_runtime.terminal().clear_screen()?;
    *terminal_size = observed_size;
    let history_display_start = app_state.lock().await.history_display_start;
    transcript_cursor.reset();
    transcript_cursor.commit_history_through(history_display_start);
    transcript_state.reset();
    stream_commits.reset();
    *replaying_transcript = true;
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
