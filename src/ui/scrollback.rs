use std::ops::Range;

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
fn unfinished_fence_start(text: &str) -> Option<usize> {
    let mut open: Option<(u8, usize, usize)> = None;
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let content = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .strip_suffix('\r')
            .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line));
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
                    if let Some((open_marker, open_length, _)) = open {
                        // A closing fence must use the same marker, be at
                        // least as long as its opener, and have no info text.
                        if marker == open_marker
                            && marker_length >= open_length
                            && rest.trim().is_empty()
                        {
                            open = None;
                        }
                        // Fence-like lines inside an open block are content;
                        // they must not toggle the block state.
                    } else if marker != b'`' || !rest.contains('`') {
                        open = Some((marker, marker_length, line_start));
                    }
                }
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
    [unfinished_fence_start(text), unfinished_table_start(text)]
        .into_iter()
        .flatten()
        .min()
}

#[cfg(test)]
mod tests {
    use super::{unfinished_fence_start, unfinished_table_start};

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
        split_stable_rows(&self.pending_stable_source(stream)).0
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
        let rows = split_stable_rows(&stable).0;
        if !rows.is_empty() {
            self.commit_stable_stream(&stable);
        }
        rows
    }
}
