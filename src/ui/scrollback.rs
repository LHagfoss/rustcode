use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

use pulldown_cmark::{Event, Options, Parser};
use ratatui::text::Line;

const STREAM_COMMIT_INTERVAL: Duration = Duration::from_millis(16);
const STREAM_COMMIT_PRESSURE_LINES: usize = 64;

/// Rendered stable rows waiting for the next transcript commit tick.
///
/// Keeping this queue separate from `ChatMessage` mirrors Codex's streaming
/// controller: semantic source remains canonical, the mutable tail stays in an
/// active cell, and completed blocks enter terminal scrollback on a bounded
/// cadence rather than directly from arbitrary provider deltas.
pub(crate) struct StreamCommitQueue {
    pending: VecDeque<Line<'static>>,
    last_commit: Instant,
}

impl Default for StreamCommitQueue {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            last_commit: Instant::now()
                .checked_sub(STREAM_COMMIT_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }
}

impl StreamCommitQueue {
    pub(crate) fn reset(&mut self) {
        self.pending.clear();
        self.last_commit = Instant::now()
            .checked_sub(STREAM_COMMIT_INTERVAL)
            .unwrap_or_else(Instant::now);
    }

    pub(crate) fn push(&mut self, lines: Vec<Line<'static>>) {
        self.pending.extend(lines);
    }

    pub(crate) fn take_ready(&mut self, force: bool) -> Vec<Line<'static>> {
        if self.pending.is_empty()
            || (!force
                && self.pending.len() < STREAM_COMMIT_PRESSURE_LINES
                && self.last_commit.elapsed() < STREAM_COMMIT_INTERVAL)
        {
            return Vec::new();
        }
        self.last_commit = Instant::now();
        self.pending.drain(..).collect()
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

fn stream_starts_with_thought(stream: &str) -> bool {
    let text = stream.trim_start();
    text.starts_with("<think>")
        || text
            .strip_prefix("thought")
            .is_some_and(|rest| rest.chars().next().is_some_and(char::is_uppercase))
}

/// Return the newline-terminated rows that can be appended permanently and
/// leave the unfinished suffix for the mutable live viewport.
pub(crate) fn split_stable_rows(text: &str) -> (Vec<String>, String) {
    let Some(last_newline) = text.rfind('\n') else {
        return (Vec::new(), text.to_owned());
    };

    (
        text[..last_newline]
            .split('\n')
            .map(str::to_owned)
            .collect(),
        text[last_newline + 1..].to_owned(),
    )
}

/// Return the byte offset of an unmatched fenced-code opener. Keeping that
/// block mutable lets the Markdown renderer see its opener and body together;
/// otherwise terminal scrollback would render each streamed code line as a
/// separate paragraph before the closing fence arrives.
pub(crate) fn fence_line_info(line: &str) -> Option<(u8, usize, &str)> {
    let content = line.strip_suffix('\n').unwrap_or(line);
    let content = content.strip_suffix('\r').unwrap_or(content);
    let indentation = content.len() - content.trim_start_matches(' ').len();
    if indentation > 3 {
        return None;
    }
    let bytes = content.as_bytes();
    let marker_position = indentation;
    let marker = *bytes.get(marker_position)?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let mut marker_length = 0;
    while bytes.get(marker_position + marker_length) == Some(&marker) {
        marker_length += 1;
    }
    if marker_length < 3 {
        return None;
    }
    let rest = &content[marker_position + marker_length..];
    if marker == b'`' && rest.contains('`') {
        return None;
    }
    Some((marker, marker_length, rest))
}

fn unfinished_fence_start(text: &str) -> Option<usize> {
    let mut open: Option<(u8, usize, usize)> = None;
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        if let Some((marker, marker_length, rest)) = fence_line_info(line) {
            if let Some((open_marker, open_length, _)) = open {
                // A closing fence must use the same marker, be at least as
                // long as its opener, and have no info text.
                if marker == open_marker && marker_length >= open_length && rest.trim().is_empty() {
                    open = None;
                }
                // Fence-like lines inside an open block are content; they
                // must not toggle the block state.
            } else {
                open = Some((marker, marker_length, line_start));
            }
        }
        line_start += line.len();
    }
    open.map(|(_, _, start)| start)
}

/// Return the byte offset of a pipe-table header that is not safe to commit yet.
///
/// A Markdown table cannot be recognized from its header alone: the delimiter
/// row may arrive in the next model delta. Keep a possible header mutable until
/// that next line settles the decision, and keep a confirmed table mutable for
/// the rest of the stream so later rows can reflow its columns. This is
/// presentation-only holdback; the raw assistant message remains canonical.
fn unfinished_table_start(text: &str) -> Option<usize> {
    let mut open_fence: Option<(u8, usize)> = None;
    let mut line_start = 0;
    let mut possible_header = None;

    for line in text.split_inclusive('\n') {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let content = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
        let indentation = content.len() - content.trim_start_matches(' ').len();
        let bytes = content.as_bytes();

        if indentation <= 3 {
            let marker_position = indentation;
            if let Some(&marker) = bytes.get(marker_position)
                && (marker == b'`' || marker == b'~')
            {
                let mut marker_length = 0;
                while bytes.get(marker_position + marker_length) == Some(&marker) {
                    marker_length += 1;
                }
                let rest = &content[marker_position + marker_length..];
                if marker_length >= 3 {
                    if let Some((open_marker, open_length)) = open_fence {
                        if marker == open_marker
                            && marker_length >= open_length
                            && rest.trim().is_empty()
                        {
                            open_fence = None;
                        }
                        possible_header = None;
                        line_start += line.len();
                        continue;
                    } else if marker != b'`' || !rest.contains('`') {
                        open_fence = Some((marker, marker_length));
                        possible_header = None;
                        line_start += line.len();
                        continue;
                    }
                }
            }
        }

        if open_fence.is_some() {
            possible_header = None;
            line_start += line.len();
            continue;
        }

        let table_line = strip_blockquote_prefix(content).trim();
        if is_table_delimiter_line(table_line) {
            if let Some(header_start) = possible_header.take() {
                return Some(header_start);
            }
            line_start += line.len();
            continue;
        }

        possible_header = is_table_header_line(table_line).then_some(line_start);
        line_start += line.len();
    }

    possible_header
}

fn strip_blockquote_prefix(line: &str) -> &str {
    let mut rest = line.trim_start();
    while let Some(stripped) = rest.strip_prefix('>') {
        rest = stripped.trim_start();
    }
    rest
}

fn is_table_header_line(line: &str) -> bool {
    let cells = line.trim_matches('|').split('|').count();
    cells >= 2 && line.contains('|')
}

fn is_table_delimiter_line(line: &str) -> bool {
    let body = line.trim().trim_matches('|');
    let cells = body.split('|').map(str::trim).collect::<Vec<_>>();
    cells.len() >= 2
        && cells.iter().all(|cell| {
            let mut chars = cell.chars();
            let Some(first) = chars.next() else {
                return false;
            };
            let Some(last) = cell.chars().next_back() else {
                return false;
            };
            (first == ':' || first == '-')
                && (last == ':' || last == '-')
                && cell.chars().filter(|character| *character == '-').count() >= 3
                && cell
                    .chars()
                    .all(|character| character == '-' || character == ':')
        })
}

fn stream_holdback_start(text: &str) -> Option<usize> {
    [
        markdown_stream_holdback_start(text),
        unfinished_fence_start(text),
        unfinished_table_start(text),
    ]
    .into_iter()
    .flatten()
    .min()
}

/// Return the source boundary before the final top-level Markdown block.
///
/// Codex keeps the final block in its active streaming cell because a later
/// delta can still change its shape: a paragraph can become a list, a setext
/// heading, a table, or a fenced block. RustCode used to commit every complete
/// physical line, which made those transitions impossible to re-render without
/// duplicating or flickering terminal output.
///
/// Offset parsing is deliberately used only for block boundaries. The normal
/// renderer remains responsible for styling and layout, so this is not a
/// second Markdown implementation. A single block returns offset zero and is
/// therefore held back in its entirety until the response is finalized.
fn markdown_stream_holdback_start(text: &str) -> Option<usize> {
    if text.trim().is_empty() {
        return None;
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let mut depth = 0usize;
    let mut block_count = 0usize;
    let mut last_block_start = 0usize;

    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        if depth == 0 && matches!(event, Event::Start(_) | Event::Rule | Event::Html(_)) {
            block_count += 1;
            last_block_start = range.start;
        }

        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }

    (block_count > 0).then_some(if block_count == 1 {
        0
    } else {
        last_block_start
    })
}

#[cfg(test)]
mod tests {
    use super::{
        StreamCommitQueue, markdown_stream_holdback_start, unfinished_fence_start,
        unfinished_table_start,
    };

    #[test]
    fn stream_commit_queue_flushes_in_order_and_resets_on_reflow() {
        let mut queue = StreamCommitQueue::default();
        queue.push(vec!["first".into(), "second".into()]);
        assert_eq!(queue.pending_len(), 2);
        assert_eq!(
            queue
                .take_ready(true)
                .into_iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        queue.push(vec!["stale width".into()]);
        queue.reset();
        assert_eq!(queue.pending_len(), 0);
    }

    #[test]
    fn streaming_keeps_the_final_markdown_block_mutable() {
        assert_eq!(markdown_stream_holdback_start("one paragraph\n"), Some(0));
        assert_eq!(
            markdown_stream_holdback_start("first paragraph\n\nsecond paragraph\n"),
            Some(17)
        );
        assert_eq!(
            markdown_stream_holdback_start("# Heading\n\n- first\n- second\n"),
            Some(11)
        );
    }

    #[test]
    fn streaming_block_boundary_handles_incomplete_fenced_markdown() {
        assert_eq!(
            markdown_stream_holdback_start("intro\n\n```rust\nlet value = 1;\n"),
            Some(7)
        );
        assert_eq!(
            markdown_stream_holdback_start("intro\n\n```rust\nbody\n```\n\nnext"),
            Some(25)
        );
    }

    #[test]
    fn fenced_stream_scanner_respects_marker_type_and_length() {
        assert_eq!(unfinished_fence_start("```rust\nbody\n```\n"), None);
        assert_eq!(
            unfinished_fence_start("```rust\nfirst\n```\n```json\nsecond\n```\n"),
            None
        );
        assert_eq!(unfinished_fence_start("```a\nx\n```\n```b\ny\n"), Some(11));
        assert_eq!(unfinished_fence_start("~~~rust\nbody\n"), Some(0));
        assert_eq!(unfinished_fence_start("```rust\n~~~\n"), Some(0));
        assert_eq!(unfinished_fence_start("````rust\n```\nbody\n"), Some(0));
        assert_eq!(unfinished_fence_start("````rust\n```\nbody\n````\n"), None);
    }

    #[test]
    fn completed_fences_do_not_toggle_on_backticks_in_the_info_string() {
        assert_eq!(unfinished_fence_start("```has`backtick\nbody\n"), None);
        assert_eq!(
            unfinished_fence_start("````rust\n```\n````\n```\n"),
            Some(18)
        );
    }

    #[test]
    fn streaming_fence_holdback_releases_only_after_the_matching_close() {
        let mut cursor = super::TranscriptCursor::default();
        let before_close = "intro\n~~~rust\nlet value = 1;\n";

        assert_eq!(cursor.pending_stable_source(before_close), "intro\n");
        assert_eq!(
            super::mutable_stream_text(before_close),
            "~~~rust\nlet value = 1;\n"
        );
        cursor.commit_stable_stream("intro\n");

        let after_close = "intro\n~~~rust\nlet value = 1;\n~~~\nAfter";
        assert_eq!(
            cursor.pending_stable_source(after_close),
            "~~~rust\nlet value = 1;\n~~~\n"
        );
        assert_eq!(super::mutable_stream_text(after_close), "After");

        cursor.commit_stable_stream("~~~rust\nlet value = 1;\n~~~\n");
        cursor.reset();
        assert_eq!(
            cursor.pending_stable_source(after_close),
            "intro\n~~~rust\nlet value = 1;\n~~~\n"
        );
    }

    #[test]
    fn table_holdback_waits_for_delimiter_and_later_rows() {
        assert_eq!(unfinished_table_start("intro\n| Name | Value |\n"), Some(6));
        assert_eq!(
            unfinished_table_start("intro\n| Name | Value |\n| --- | --- |\nrow\n"),
            Some(6)
        );
        assert_eq!(
            unfinished_table_start("| Name | Value |\nplain prose\n"),
            None
        );
    }

    #[test]
    fn table_holdback_ignores_fenced_pipe_text() {
        assert_eq!(
            unfinished_table_start("```text\n| Name | Value |\n| --- | --- |\n```\n"),
            None
        );
        assert_eq!(
            unfinished_table_start("> | Name | Value |\n> | --- | --- |\n"),
            Some(0)
        );
    }
}

/// Return the portion that must remain in the mutable viewport. Reasoning-led
/// responses are held out of terminal scrollback until finalization, so their
/// complete formatted answer must stay live instead of only the final row.
pub(crate) fn mutable_stream_text(text: &str) -> String {
    if stream_starts_with_thought(text) {
        crate::network::text::promote_bare_thought_markers(text)
    } else if let Some(start) = stream_holdback_start(text) {
        text[start..].to_owned()
    } else {
        split_stable_rows(text).1
    }
}

pub(crate) fn mutable_stream_is_continuation(text: &str) -> bool {
    !stream_starts_with_thought(text) && text.contains('\n')
}

/// Tracks transcript content already handed to the terminal's scrollback.
#[derive(Default)]
pub(crate) struct TranscriptCursor {
    next_history_index: usize,
    committed_stream: String,
}

impl TranscriptCursor {
    /// Forget terminal-specific progress so the canonical transcript can be
    /// rendered again after an inline viewport resize.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn begin_stream(&mut self, stream: &str) {
        // Finalizing a response clears the live buffer before its matching
        // history message reaches the draw loop. Keep the committed prefix
        // through that handoff so the history renderer only emits the tail.
        if stream.is_empty() {
            return;
        }
        if !stream.starts_with(&self.committed_stream) {
            self.committed_stream.clear();
        }
    }

    pub(crate) fn is_at_start(&self) -> bool {
        self.next_history_index == 0
    }

    pub(crate) fn has_committed_stream(&self) -> bool {
        !self.committed_stream.is_empty()
    }

    pub(crate) fn pending_history_range(&self, history_len: usize) -> Range<usize> {
        self.next_history_index.min(history_len)..history_len
    }

    pub(crate) fn commit_history_through(&mut self, history_len: usize) {
        self.next_history_index = history_len;
    }

    pub(crate) fn take_history_range(&mut self, history_len: usize) -> Range<usize> {
        let range = self.pending_history_range(history_len);
        self.commit_history_through(history_len);
        range
    }

    pub(crate) fn pending_stable_stream(&self, stream: &str) -> Vec<String> {
        // Terminal scrollback cannot revise earlier rows. Hold a response that
        // starts with reasoning until it is finalized, so it is emitted once
        // with its normalized thought block and final timing/token metadata.
        if self.committed_stream.is_empty() && stream_starts_with_thought(stream) {
            return Vec::new();
        }
        stable_rows(&self.pending_stable_source(stream))
    }

    /// Return only the source that is safe to hand to terminal scrollback.
    /// The current fenced block remains in the mutable viewport until its
    /// closing fence makes the Markdown structure stable.
    pub(crate) fn pending_stable_source(&self, stream: &str) -> String {
        if self.committed_stream.is_empty() && stream_starts_with_thought(stream) {
            return String::new();
        }
        let pending = stream
            .strip_prefix(&self.committed_stream)
            .unwrap_or(stream);
        let stable = if let Some(start) = stream_holdback_start(pending) {
            pending[..start].to_owned()
        } else {
            split_stable_rows(pending).0.join("\n")
        };
        if stable.is_empty() {
            String::new()
        } else if stable.ends_with('\n') {
            stable
        } else {
            format!("{stable}\n")
        }
    }

    pub(crate) fn commit_stable_stream(&mut self, stable: &str) {
        self.committed_stream.push_str(stable);
    }

    /// When the stream is finalized into a durable assistant history entry,
    /// return the part not already emitted above the live viewport.
    pub(crate) fn take_final_stream_remainder(&mut self, final_text: &str) -> Option<String> {
        if self.committed_stream.is_empty() || !final_text.starts_with(&self.committed_stream) {
            return None;
        }
        let remainder = final_text[self.committed_stream.len()..].to_owned();
        self.committed_stream.clear();
        Some(remainder)
    }

    pub(crate) fn take_stable_stream(&mut self, stream: &str) -> Vec<String> {
        let committed_len = self.committed_stream.len().min(stream.len());
        if stream.len() < self.committed_stream.len()
            || !stream.starts_with(&self.committed_stream[..committed_len])
        {
            self.committed_stream.clear();
        }

        let pending = &stream[self.committed_stream.len()..];
        let stable = self.pending_stable_source(pending);
        let rows = stable_rows(&stable);
        if !rows.is_empty() {
            self.commit_stable_stream(&stable);
        }
        rows
    }
}

fn stable_rows(source: &str) -> Vec<String> {
    let mut rows = split_stable_rows(source).0;
    // Markdown renderers intentionally discard blank rows at the end of a
    // committed block. Keep interior blank rows for layout, but don't expose
    // the separator that only exists to delimit the mutable final block.
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows
}
