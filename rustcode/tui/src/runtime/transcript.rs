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

fn is_tool_only_assistant(
    snapshot: &crate::ui::render_snapshot::RenderSnapshot,
    message_index: usize,
    width: u16,
) -> bool {
    let Some(message) = snapshot.active_history().get(message_index) else {
        return false;
    };
    if message.role != "assistant" {
        return false;
    }
    let has_tool_calls = !message.tool_calls.is_empty()
        || !rustcode_tool_protocol::resolve_tool_calls(message, snapshot.active_tool_protocol())
            .is_empty();
    has_tool_calls
        && crate::ui::render_committed_history_block_snapshot(snapshot, message_index, width)
            .is_empty()
}

/// Collect tool results from one provider batch and adjacent empty tool-only
/// turns. A visible assistant message, system notice, or user message remains
/// a hard transcript boundary even when its content is not part of the tool
/// result group.
pub(super) fn tool_result_group(
    snapshot: &crate::ui::render_snapshot::RenderSnapshot,
    start: usize,
    end: usize,
    width: u16,
) -> (Vec<usize>, usize) {
    let history = snapshot.active_history();
    let mut indices = Vec::new();
    let mut index = start;
    while index < end
        && history
            .get(index)
            .is_some_and(|message| message.role == "tool")
    {
        indices.push(index);
        index += 1;

        if index < end
            && is_tool_only_assistant(snapshot, index, width)
            && index + 1 < end
            && history
                .get(index + 1)
                .is_some_and(|message| message.role == "tool")
        {
            index += 1;
        }
    }
    (indices, index)
}

/// Whether committed transcript rows may be copied into the terminal's own
/// scrollback.
///
/// Off by default (#1587). Native scrollback is write-only, so rows copied
/// there survive an expanded body being collapsed, survive the exit erase, and
/// make every new commit push the reader's scrollback position further away
/// (#1593, #1595). The readable transcript does not need them: the full-height
/// mutable viewport re-projects it from the render snapshot every frame. The
/// opt-in exists for users who rely on the copy for terminal copy/paste and
/// shell piping.
pub(super) fn transcript_scrollback_enabled(
    snapshot: &crate::ui::render_snapshot::RenderSnapshot,
) -> bool {
    snapshot.config().preserve_transcript_scrollback
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
    // Native scrollback is opt-in (#1587). By default the readable transcript
    // lives in the mutable viewport, which the full-height frame re-projects
    // from the render snapshot every frame: collapsing an expanded body or
    // scrolling back cannot leave orphaned rows, and an exit has nothing in
    // scrollback it cannot erase. `preserve_transcript_scrollback` opts back
    // into the write-only copy for copy/paste and shell piping (#1593).
    let keep_scrollback = transcript_scrollback_enabled(snapshot);
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
    if !stable_lines.is_empty() && keep_scrollback {
        crate::run::insert_scrollback_lines(
            terminal_runtime.terminal(),
            stable_lines,
            terminal_width,
        )?;
    }
    let mut blocks = Vec::new();
    if crate::run::should_clear_mutable_viewport_before_history(
        response_just_finished,
        transcript_cursor.is_at_start(),
        !history_range.is_empty(),
    ) {
        // Flag the repaint without presenting an intermediate blank frame:
        // the old `draw_height(0)` reset left a blank viewport behind when
        // the repaint that followed it panicked.
        terminal_runtime.terminal().mark_viewport_dirty();
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
            let (indices, group_end) =
                tool_result_group(snapshot, index, history_range.end, terminal_width);
            let group_kind = crate::ui::tool_result_group_kind(snapshot, &indices, terminal_width);
            let continuing =
                group_kind.is_some() && transcript_cursor.tool_group_kind() == group_kind;
            let mut block = if continuing {
                crate::ui::render_committed_tool_result_continuation_snapshot(
                    snapshot,
                    &indices,
                    terminal_width,
                    false,
                )
            } else {
                crate::ui::render_committed_tool_result_group_snapshot(
                    snapshot,
                    &indices,
                    terminal_width,
                    false,
                )
            };
            if !block.is_empty() {
                if !continuing {
                    block.push(ratatui::text::Line::from(""));
                }
                blocks.push(block);
                transcript_cursor.set_tool_group_kind(group_kind);
            } else {
                // Hidden control-plane results (for example complete_task)
                // still terminate the visible exploration group.
                transcript_cursor.set_tool_group_kind(None);
            }
            index = group_end;
            continue;
        } else if message.role == "assistant"
            && is_tool_only_assistant(snapshot, index, terminal_width)
            && snapshot
                .history()
                .get(index + 1)
                .is_some_and(|next| next.role == "tool")
        {
            // One-tool-per-round orchestration inserts an empty assistant
            // call between results. It is part of the active visual group,
            // not a new transcript boundary.
            index += 1;
            continue;
        } else if message.role == "assistant" && !message.conversation_recap {
            transcript_cursor.set_tool_group_kind(None);
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
            transcript_cursor.set_tool_group_kind(None);
            blocks.push(crate::ui::render_committed_history_block_snapshot(
                snapshot,
                index,
                terminal_width,
            ));
        }
        index += 1;
    }
    if keep_scrollback {
        for lines in blocks {
            crate::run::insert_scrollback_lines(
                terminal_runtime.terminal(),
                lines,
                terminal_width,
            )?;
        }
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
        if !stable_lines.is_empty() && keep_scrollback {
            crate::run::insert_scrollback_lines(
                terminal_runtime.terminal(),
                stable_lines,
                terminal_width,
            )?;
        }
        *replaying_transcript = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{tool_result_group, transcript_scrollback_enabled};
    use crate::ui::render_snapshot::render_snapshot;
    use rustcode::app::{AppState, ChatMessage, ToolCallRef};

    /// #1587: the transcript reaches the terminal's own scrollback only when
    /// the user opts back in, and the opt-in is what the copy/paste path costs.
    #[test]
    fn native_scrollback_is_opt_in_and_the_transcript_is_still_rendered() {
        let mut state = AppState::new();
        state
            .history
            .push(ChatMessage::new("assistant", "a committed answer"));
        state
            .history
            .push(ChatMessage::new("tool", "get_time: noon").answering(Some("call-1".to_owned())));
        let snapshot = render_snapshot(&rustcode::controller::render_state(&state));

        assert!(
            !transcript_scrollback_enabled(&snapshot),
            "the default must not write transcript rows into native scrollback"
        );
        let block = crate::ui::render_committed_history_block_snapshot(&snapshot, 0, 80);
        assert!(
            block
                .iter()
                .any(|line| line.to_string().contains("a committed answer")),
            "the transcript is still available to the viewport: {block:?}"
        );

        state.config.preserve_transcript_scrollback = true;
        let opted_in = render_snapshot(&rustcode::controller::render_state(&state));
        assert!(transcript_scrollback_enabled(&opted_in));
    }

    fn tool_turn(id: &str) -> ChatMessage {
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: id.to_owned(),
            name: "get_time".to_owned(),
            arguments: "{}".to_owned(),
        }])
    }

    fn result(id: &str) -> ChatMessage {
        ChatMessage::new("tool", "get_time: result").answering(Some(id.to_owned()))
    }

    #[test]
    fn adjacent_empty_tool_turns_share_one_group() {
        let mut state = AppState::new();
        state.history.extend([
            tool_turn("call-1"),
            result("call-1"),
            tool_turn("call-2"),
            result("call-2"),
        ]);
        let snapshot = render_snapshot(&rustcode::controller::render_state(&state));

        assert_eq!(tool_result_group(&snapshot, 1, 4, 80), (vec![1, 3], 4));
    }

    #[test]
    fn visible_assistant_prose_keeps_tool_groups_separate() {
        let mut state = AppState::new();
        state.history.extend([
            tool_turn("call-1"),
            result("call-1"),
            ChatMessage::new("assistant", "I need to inspect one more thing.").with_tool_calls(
                vec![ToolCallRef {
                    id: "call-2".to_owned(),
                    name: "get_time".to_owned(),
                    arguments: "{}".to_owned(),
                }],
            ),
            result("call-2"),
        ]);
        let snapshot = render_snapshot(&rustcode::controller::render_state(&state));

        assert_eq!(tool_result_group(&snapshot, 1, 4, 80), (vec![1], 2));
    }

    #[test]
    fn visible_assistant_thought_keeps_tool_groups_separate() {
        let mut state = AppState::new();
        state.history.extend([
            tool_turn("call-1"),
            result("call-1"),
            ChatMessage::new("assistant", "<think>Planning the next read.</think>")
                .with_tool_calls(vec![ToolCallRef {
                    id: "call-2".to_owned(),
                    name: "get_time".to_owned(),
                    arguments: "{}".to_owned(),
                }]),
            result("call-2"),
        ]);
        let snapshot = render_snapshot(&rustcode::controller::render_state(&state));

        assert_eq!(tool_result_group(&snapshot, 1, 4, 80), (vec![1], 2));
    }

    #[test]
    fn system_boundaries_are_not_crossed_by_tool_grouping() {
        let mut state = AppState::new();
        state.history.extend([
            tool_turn("call-1"),
            result("call-1"),
            tool_turn("call-2"),
            ChatMessage::new("system", "a recovery boundary"),
            result("call-2"),
        ]);
        let snapshot = render_snapshot(&rustcode::controller::render_state(&state));

        assert_eq!(tool_result_group(&snapshot, 1, 5, 80), (vec![1], 2));
    }
}
