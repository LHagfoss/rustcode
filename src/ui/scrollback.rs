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

/// Tracks transcript content already handed to the terminal's scrollback.
#[derive(Default)]
pub(crate) struct TranscriptCursor {
    next_history_index: usize,
    committed_stream: String,
}

impl TranscriptCursor {
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
        let pending = stream.strip_prefix(&self.committed_stream).unwrap_or(stream);
        split_stable_rows(pending).0
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
        let rows = self.pending_stable_stream(pending);
        if !rows.is_empty() {
            let stable_len = pending
                .rfind('\n')
                .expect("rows require a terminating newline")
                + 1;
            self.commit_stable_stream(&pending[..stable_len]);
        }
        rows
    }
}
