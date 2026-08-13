use std::ops::Range;

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
        let pending = stream
            .strip_prefix(&self.committed_stream)
            .unwrap_or(stream);
        split_stable_rows(pending).0
    }

    pub(crate) fn commit_stable_stream(&mut self, stable: &str) {
        self.committed_stream.push_str(stable);
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
