pub(super) fn render_finalized_assistant_scrollback(
    snapshot: &crate::ui::render_snapshot::RenderSnapshot,
    transcript_cursor: &mut crate::ui::scrollback::TranscriptCursor,
    message_index: usize,
    message: &str,
    width: u16,
) -> Vec<ratatui::text::Line<'static>> {
    let is_continuation = transcript_cursor.has_committed_stream();
    match transcript_cursor.take_final_stream_remainder(message) {
        Some(remainder) if !remainder.is_empty() => {
            let mut chunk = crate::ui::render_committed_assistant_chunk_snapshot(
                snapshot,
                &remainder,
                width,
                is_continuation,
            );
            if !chunk.is_empty() {
                chunk.push(ratatui::text::Line::from(""));
            }
            chunk
        }
        Some(_) => vec![ratatui::text::Line::from("")],
        None => crate::ui::render_committed_history_block_snapshot(snapshot, message_index, width),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn commit_transcript(
    terminal_runtime: &mut crate::ui::TerminalRuntime,
    snapshot: &crate::ui::render_snapshot::RenderSnapshot,
    transcript_cursor: &mut crate::ui::scrollback::TranscriptCursor,
    stream_commits: &mut crate::ui::scrollback::StreamCommitQueue,
    replaying_transcript: &mut bool,
    terminal_width: u16,
    response_active: bool,
    response_just_finished: bool,
) -> std::io::Result<()> {
    let live_response = snapshot.current_response();
    transcript_cursor.begin_stream(&live_response);
    let stable_source = if *replaying_transcript {
        String::new()
    } else {
        transcript_cursor.pending_stable_source(&live_response)
    };
    if !stable_source.is_empty() {
        let is_continuation = transcript_cursor.has_committed_stream();
        let lines = crate::ui::render_committed_assistant_chunk_snapshot(
            snapshot,
            &stable_source,
            terminal_width,
            is_continuation,
        );
        if !lines.is_empty() {
            stream_commits.push(lines);
        }
        transcript_cursor.commit_stable_stream(&stable_source);
    }

    let history_range = transcript_cursor.pending_history_range(snapshot.history().len());
    let stable_lines = stream_commits.take_ready(!history_range.is_empty() || !response_active);
    if !stable_lines.is_empty() {
        crate::insert_scrollback_lines(terminal_runtime.terminal(), stable_lines, terminal_width)?;
    }
    let mut blocks = Vec::new();
    if crate::should_clear_mutable_viewport_before_history(
        response_just_finished,
        transcript_cursor.is_at_start(),
        !history_range.is_empty(),
    ) {
        terminal_runtime.terminal().draw_height(0, |_| {})?;
    }
    if transcript_cursor.is_at_start() && !history_range.is_empty() {
        let banner =
            crate::ui::build_claude_startup_banner_snapshot(snapshot, terminal_width as usize, 24);
        if !banner.is_empty() {
            blocks.push(banner);
        }
    }
    let mut index = history_range.start;
    while index < history_range.end {
        let message = &snapshot.history()[index];
        if message.role == "tool" {
            let group_end = (index + 1..history_range.end)
                .find(|&next| snapshot.history()[next].role != "tool")
                .unwrap_or(history_range.end);
            let indices = (index..group_end).collect::<Vec<_>>();
            let mut block = crate::ui::render_committed_tool_result_group_snapshot(
                snapshot,
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
            let separator = crate::ui::render_work_separator_before_assistant_snapshot(
                snapshot,
                index,
                terminal_width,
            );
            if !separator.is_empty() {
                blocks.push(separator);
            }
            blocks.push(render_finalized_assistant_scrollback(
                snapshot,
                transcript_cursor,
                index,
                &message.content,
                terminal_width,
            ));
        } else {
            blocks.push(crate::ui::render_committed_history_block_snapshot(
                snapshot,
                index,
                terminal_width,
            ));
        }
        index += 1;
    }
    for lines in blocks {
        crate::insert_scrollback_lines(terminal_runtime.terminal(), lines, terminal_width)?;
    }

    transcript_cursor.commit_history_through(history_range.end);
    if *replaying_transcript {
        let stable_source = transcript_cursor.pending_stable_source(&live_response);
        if !stable_source.is_empty() {
            let is_continuation = transcript_cursor.has_committed_stream();
            let lines = crate::ui::render_committed_assistant_chunk_snapshot(
                snapshot,
                &stable_source,
                terminal_width,
                is_continuation,
            );
            if !lines.is_empty() {
                stream_commits.push(lines);
            }
            transcript_cursor.commit_stable_stream(&stable_source);
        }
        let stable_lines = stream_commits.take_ready(true);
        if !stable_lines.is_empty() {
            crate::insert_scrollback_lines(
                terminal_runtime.terminal(),
                stable_lines,
                terminal_width,
            )?;
        }
        *replaying_transcript = false;
    }
    Ok(())
}
