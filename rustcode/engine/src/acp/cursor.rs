//! The replay cursor: where a client is in a session's stream of updates.
//!
//! Every `session/update` the engine sends for a session carries a cursor in
//! `_meta.rustcode.cursor`. Live updates are numbered one after the other;
//! the updates that rebuild a view on load (the transcript, the running
//! turn, the title, commands and usage) carry the cursor the view is current
//! as of, so a client that keeps the greatest cursor it saw knows where it is.
//! The `session/load` answer repeats it in `_meta.rustcode.cursor`: that is
//! the value to keep once the load has completed.
//!
//! A client that reconnects sends `_meta.rustcode.resume: { cursor }` in
//! `session/load`. The engine keeps the most recent updates of each session
//! ([`UpdateLog`]); when every update after `cursor` is still there it sends
//! just those, then streams live. Otherwise (the cursor is older than the
//! log, from before the engine restarted, or from the future) it sends the
//! whole transcript the way a load without `resume` does, with
//! `_meta.rustcode.reset: true` on an empty `session_info_update` before it
//! and in the answer, so the client clears what it shows first.
//!
//! A load without `resume` behaves as it always did and gets no `reset`.

use std::collections::VecDeque;

use agent_client_protocol::schema::v1::{LoadSessionRequest, LoadSessionResponse, SessionUpdate};
use serde_json::{Value, json};

/// Most updates kept per session. Text streams as small chunks (a few dozen
/// bytes each), so this covers a few minutes of a busy turn: far longer than
/// a dropped connection usually takes to come back. A client that is away
/// longer loads the transcript again.
pub(crate) const LOG_UPDATES: usize = 8192;

/// Most bytes (of the updates as JSON) kept per session. Tool output and
/// diffs are large; this bounds a session's log however big they are. One
/// update bigger than this is not kept, and nobody resumes across it.
pub(crate) const LOG_BYTES: usize = 8 << 20;

/// An update the log kept.
pub(crate) struct Logged {
    pub(crate) cursor: u64,
    pub(crate) update: SessionUpdate,
    bytes: usize,
}

/// The recent updates of one session, each with its cursor.
///
/// Cursors count up by one per update (an [`invalidate`](Self::invalidate)
/// skips one). They start from the clock in microseconds when the log is
/// made, so a log made after the engine restarted, or after the session was
/// closed and opened again, begins above every cursor an earlier one handed
/// out: an old cursor never looks like a recent one. Microseconds keep them
/// below 2^53, which a JavaScript client reads exactly.
pub(crate) struct UpdateLog {
    /// The cursor of the newest update, or the starting point when none.
    head: u64,
    /// The updates with cursors `head - len + 1 ..= head`, oldest first.
    entries: VecDeque<Logged>,
    bytes: usize,
}

impl UpdateLog {
    pub(crate) fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(0));
        Self::starting_at(now)
    }

    pub(crate) fn starting_at(head: u64) -> Self {
        Self {
            head,
            entries: VecDeque::new(),
            bytes: 0,
        }
    }

    /// The cursor of the newest update.
    pub(crate) fn head(&self) -> u64 {
        self.head
    }

    /// Keep `update`; its cursor.
    pub(crate) fn push(&mut self, update: &SessionUpdate) -> u64 {
        self.head += 1;
        let bytes = serde_json::to_string(update).map_or(0, |json| json.len());
        self.entries.push_back(Logged {
            cursor: self.head,
            update: update.clone(),
            bytes,
        });
        self.bytes += bytes;
        while self.entries.len() > LOG_UPDATES || self.bytes > LOG_BYTES {
            let Some(expired) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= expired.bytes;
        }
        self.head
    }

    /// The history changed in a way no update tells (a rewind): nobody
    /// resumes from before this.
    pub(crate) fn invalidate(&mut self) {
        self.head += 1;
        self.entries.clear();
        self.bytes = 0;
    }

    /// The updates after `cursor`, oldest first; `None` when they are not
    /// all kept (the cursor is older than the log) or `cursor` is beyond
    /// the newest update, which this log never handed out.
    pub(crate) fn since(&self, cursor: u64) -> Option<impl Iterator<Item = &Logged>> {
        if cursor > self.head {
            return None;
        }
        if cursor < self.head
            && self
                .entries
                .front()
                .is_none_or(|first| first.cursor > cursor + 1)
        {
            return None;
        }
        Some(
            self.entries
                .iter()
                .filter(move |logged| logged.cursor > cursor),
        )
    }
}

/// `_meta.rustcode.resume.cursor` of a `session/load`, if the client sent one.
pub(crate) fn requested_cursor(request: &LoadSessionRequest) -> Option<u64> {
    request
        .meta
        .as_ref()?
        .get("rustcode")?
        .get("resume")?
        .get("cursor")?
        .as_u64()
}

/// `meta` with `key: value` under `rustcode`, over `meta` when given.
pub(crate) fn rustcode_meta(
    meta: Option<serde_json::Map<String, Value>>,
    key: &str,
    value: Value,
) -> serde_json::Map<String, Value> {
    crate::worktrees::merge_rustcode_meta(meta, key, value)
}

/// What the answer to a `session/load` says about the client's position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Position {
    /// Keep this cursor once the load has completed.
    pub(crate) cursor: u64,
    /// The client asked to resume but could not: it was sent the whole
    /// transcript and has to clear its view first.
    pub(crate) reset: bool,
    /// Only what the client missed was sent; the load has nothing to add.
    pub(crate) resumed: bool,
}

impl Position {
    /// `response` with the position in `_meta.rustcode`.
    pub(crate) fn finish(self, mut response: LoadSessionResponse) -> LoadSessionResponse {
        let mut meta = rustcode_meta(response.meta.take(), "cursor", json!(self.cursor));
        if self.reset {
            meta = rustcode_meta(Some(meta), "reset", json!(true));
        }
        response.meta = Some(meta);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, TextContent};

    fn chunk(text: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    fn cursors(log: &UpdateLog, since: u64) -> Option<Vec<u64>> {
        log.since(since)
            .map(|logged| logged.map(|logged| logged.cursor).collect())
    }

    #[test]
    fn cursors_count_up_and_a_recent_one_gets_only_what_came_after() {
        let mut log = UpdateLog::starting_at(100);
        assert_eq!(cursors(&log, 100), Some(vec![]));
        let first = log.push(&chunk("a"));
        let second = log.push(&chunk("b"));
        let third = log.push(&chunk("c"));
        assert_eq!((first, second, third), (101, 102, 103));
        assert_eq!(cursors(&log, 101), Some(vec![102, 103]));
        assert_eq!(cursors(&log, 103), Some(vec![]));
        assert_eq!(cursors(&log, 100), Some(vec![101, 102, 103]));
    }

    #[test]
    fn a_cursor_older_than_the_log_or_from_the_future_cannot_resume() {
        let mut log = UpdateLog::starting_at(0);
        for _ in 0..LOG_UPDATES + 10 {
            log.push(&chunk("x"));
        }
        let head = log.head();
        let oldest = head - LOG_UPDATES as u64 + 1;
        assert_eq!(
            cursors(&log, oldest - 1).map(|kept| kept.len()),
            Some(LOG_UPDATES)
        );
        assert!(log.since(oldest - 2).is_none(), "one update was dropped");
        assert!(log.since(5).is_none());
        assert!(log.since(head + 1).is_none());
    }

    #[test]
    fn the_log_is_bounded_in_bytes_too() {
        let mut log = UpdateLog::starting_at(0);
        let big = "x".repeat(LOG_BYTES / 4);
        for _ in 0..8 {
            log.push(&chunk(&big));
        }
        assert!(log.bytes <= LOG_BYTES);
        assert!(log.entries.len() < 8);
        // Bigger than the whole budget: not kept, nobody resumes across it.
        let before = log.head();
        log.push(&chunk(&"y".repeat(LOG_BYTES + 1)));
        assert!(log.entries.is_empty());
        assert!(log.since(before).is_none());
        assert_eq!(cursors(&log, log.head()), Some(vec![]));
    }

    #[test]
    fn a_rewrite_of_the_history_ends_every_resume_from_before_it() {
        let mut log = UpdateLog::starting_at(10);
        let seen = log.push(&chunk("a"));
        log.invalidate();
        assert!(log.since(seen).is_none());
        assert_eq!(cursors(&log, log.head()), Some(vec![]));
        assert_eq!(log.push(&chunk("b")), seen + 2);
    }

    #[test]
    fn a_new_log_starts_above_the_cursors_of_an_earlier_one() {
        let mut earlier = UpdateLog::new();
        for _ in 0..100 {
            earlier.push(&chunk("x"));
        }
        let seen = earlier.head();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let later = UpdateLog::new();
        assert!(
            later.head() > seen,
            "an engine restart cannot reuse cursors"
        );
        assert!(later.since(seen).is_none());
    }
}
