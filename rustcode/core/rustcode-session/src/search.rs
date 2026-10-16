//! Content search over saved session transcripts.
//!
//! A case-insensitive substring match over what the user and the assistant
//! wrote. Tool output, system messages, slash commands, idle recaps and image
//! markers are not conversation text and never match.

use super::{
    ChatMessageMetaRef, SessionMeta, SessionScope, SessionStore, read_transcript,
    unwrap_title_paste_markers,
};
use std::borrow::Cow;

/// Transcripts larger than this are skipped rather than read for a search; a
/// sidebar keystroke must not pull hundreds of megabytes off disk.
const MAX_SEARCHED_TRANSCRIPT_BYTES: u64 = 32 * 1024 * 1024;
/// Characters of context kept before and after the first match.
const SNIPPET_BEFORE: usize = 40;
const SNIPPET_AFTER: usize = 80;
const IMAGE_MARKER: &str = "![image](file://";

/// One session whose messages contain the query.
#[derive(Debug, Clone)]
pub struct SessionSearchHit {
    pub id: String,
    pub meta: SessionMeta,
    /// Whitespace-collapsed text around the first match, with `…` where it
    /// was cut.
    pub snippet: String,
    /// Matches across every searched message of the session.
    pub match_count: usize,
}

impl SessionStore {
    /// Sessions under `scope` whose user or assistant messages contain `query`
    /// (case-insensitive), newest first, at most `limit` of them. Stops reading
    /// transcripts as soon as `limit` sessions matched. An empty query matches
    /// nothing.
    pub fn search_sessions(
        &self,
        query: &str,
        limit: usize,
        scope: &SessionScope,
    ) -> Vec<SessionSearchHit> {
        let needle = fold(query.trim()).0;
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let scope = scope.prepare();
        let mut hits = Vec::new();
        for path in self.sorted_session_paths() {
            if hits.len() >= limit {
                break;
            }
            let recorded = self.session_workspace_cwd(&path);
            if !scope.accepts(recorded.as_deref()) {
                continue;
            }
            let Some(read_path) = self.session_read_path(&path) else {
                continue;
            };
            if std::fs::metadata(&read_path).map_or(true, |metadata| {
                metadata.len() > MAX_SEARCHED_TRANSCRIPT_BYTES
            }) {
                continue;
            }
            let Ok(content) = read_transcript(&read_path) else {
                continue;
            };
            let Ok(messages) = serde_json::from_str::<Vec<ChatMessageMetaRef<'_>>>(&content) else {
                continue;
            };
            let Some((snippet, match_count)) = search_messages(&messages, &needle) else {
                continue;
            };
            let Some(meta) = self.meta_from_messages(&path, recorded, &messages) else {
                continue;
            };
            let Some(id) = Self::session_id_from_path(&path) else {
                continue;
            };
            hits.push(SessionSearchHit {
                id,
                meta,
                snippet,
                match_count,
            });
        }
        hits
    }
}

/// The snippet around the first match and the total match count, or `None`
/// when no searchable message contains `needle` (already folded).
fn search_messages(messages: &[ChatMessageMetaRef<'_>], needle: &str) -> Option<(String, usize)> {
    let mut snippet = None;
    let mut count = 0;
    for message in messages {
        let Some(text) = searchable_text(message) else {
            continue;
        };
        let (folded, offsets) = fold(&text);
        let mut matches = folded.match_indices(needle).peekable();
        if snippet.is_none()
            && let Some(&(start, _)) = matches.peek()
        {
            let end = start + needle.len();
            let original_start = offsets[start];
            let original_end = offsets.get(end).copied().unwrap_or(text.len());
            snippet = Some(make_snippet(&text, original_start, original_end));
        }
        count += matches.count();
    }
    snippet.map(|snippet| (snippet, count))
}

/// The conversation text of a message, or `None` for messages search skips.
fn searchable_text<'a>(message: &'a ChatMessageMetaRef<'_>) -> Option<Cow<'a, str>> {
    let is_user = message.role == "user";
    if !(is_user || message.role == "assistant") || message.conversation_recap {
        return None;
    }
    if is_user && message.content.starts_with('/') {
        return None;
    }
    let text = unwrap_title_paste_markers(&message.content);
    if !text.contains(IMAGE_MARKER) {
        return Some(text);
    }
    Some(Cow::Owned(strip_image_markers(&text)))
}

/// Remove `![image](file://…)` attachment markers, keeping the text around them.
fn strip_image_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(IMAGE_MARKER) {
        out.push_str(&rest[..index]);
        let after = &rest[index + IMAGE_MARKER.len()..];
        rest = after.find(')').map_or("", |close| &after[close + 1..]);
    }
    out.push_str(rest);
    out
}

/// Lowercase `text` one character at a time, returning the folded string and,
/// for every byte of it, the byte offset of the original character it came
/// from (plus one trailing entry for the end). Per-character folding keeps the
/// query and the text folded the same way and lets a match map back to the
/// original text for the snippet.
fn fold(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::with_capacity(text.len());
    let mut offsets = Vec::with_capacity(text.len() + 1);
    for (index, character) in text.char_indices() {
        for lower in character.to_lowercase() {
            folded.push(lower);
            offsets.extend(std::iter::repeat_n(index, lower.len_utf8()));
        }
    }
    offsets.push(text.len());
    (folded, offsets)
}

/// About `SNIPPET_BEFORE` characters before the match through `SNIPPET_AFTER`
/// after it, whitespace collapsed to single spaces, with `…` at cut ends.
fn make_snippet(text: &str, start: usize, end: usize) -> String {
    let before: Vec<(usize, char)> = text[..start].char_indices().collect();
    let from = before
        .len()
        .checked_sub(SNIPPET_BEFORE)
        .map_or(0, |skip| before[skip].0);
    let to = text[end..]
        .char_indices()
        .nth(SNIPPET_AFTER)
        .map_or(text.len(), |(offset, _)| end + offset);
    let body = text[from..to]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut snippet = String::new();
    if from > 0 {
        snippet.push('…');
    }
    snippet.push_str(&body);
    if to < text.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::message;
    use crate::{SessionWorkspace, flush_history};
    use rustcode_core::ChatMessage;

    fn save(store: &SessionStore, id: &str, history: &[ChatMessage]) {
        store.save_session_history(id, history);
        flush_history();
    }

    fn recap(content: &str) -> ChatMessage {
        let mut message = message("assistant", content);
        message.conversation_recap = true;
        message
    }

    #[test]
    fn finds_user_and_assistant_text_case_insensitively() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        save(
            &store,
            "1700000000000",
            &[
                message("user", "Fix the Parser bug"),
                message("assistant", "The parser now handles PARSER edge cases."),
            ],
        );
        save(
            &store,
            "1700000001000",
            &[
                message("user", "unrelated"),
                message("assistant", "nothing here"),
            ],
        );
        let hits = store.search_sessions("parser", 10, &SessionScope::All);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "1700000000000");
        assert_eq!(hits[0].match_count, 3);
        assert_eq!(hits[0].snippet, "Fix the Parser bug");
        assert_eq!(hits[0].meta.title, "Fix the Parser bug");
    }

    #[test]
    fn skips_tool_output_system_commands_recaps_and_image_markers() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        save(
            &store,
            "1700000000000",
            &[
                message("user", "look at ![image](file:///tmp/needle.png) please"),
                message("user", "/model needle"),
                message("system", "needle"),
                message("tool", "needle in tool output"),
                recap("needle recap"),
                message("assistant", "done"),
            ],
        );
        assert!(
            store
                .search_sessions("needle", 10, &SessionScope::All)
                .is_empty()
        );
        assert_eq!(
            store
                .search_sessions("please", 10, &SessionScope::All)
                .len(),
            1
        );
    }

    #[test]
    fn returns_newest_first_and_stops_at_limit() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        for id in ["1700000000000", "1700000002000", "1700000001000"] {
            save(
                &store,
                id,
                &[message("user", "shared word"), message("assistant", "ok")],
            );
        }
        let ids: Vec<_> = store
            .search_sessions("SHARED", 10, &SessionScope::All)
            .into_iter()
            .map(|hit| hit.id)
            .collect();
        assert_eq!(ids, ["1700000002000", "1700000001000", "1700000000000"]);
        assert_eq!(
            store.search_sessions("shared", 2, &SessionScope::All).len(),
            2
        );
        assert!(
            store
                .search_sessions("  ", 10, &SessionScope::All)
                .is_empty()
        );
    }

    #[test]
    fn honours_workspace_scope() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        let work = tempfile::tempdir().unwrap();
        let project = work.path().join("project");
        let other = work.path().join("other");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        for (id, cwd) in [("1700000000000", &project), ("1700000001000", &other)] {
            save(
                &store,
                id,
                &[message("user", "scoped text"), message("assistant", "ok")],
            );
            store
                .save_session_workspace(
                    id,
                    &SessionWorkspace {
                        cwd: cwd.clone(),
                        ..SessionWorkspace::default()
                    },
                )
                .unwrap();
        }
        let scoped = store.search_sessions("scoped", 10, &SessionScope::WorkspaceTree(project));
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "1700000000000");
        assert_eq!(
            store
                .search_sessions("scoped", 10, &SessionScope::All)
                .len(),
            2
        );
    }

    #[test]
    fn snippet_is_cut_around_the_first_match() {
        let long = format!("{} Needle {}", "a ".repeat(100), "b ".repeat(100));
        let messages: Vec<ChatMessage> = vec![message("user", &long)];
        let json = serde_json::to_string(&messages).unwrap();
        let parsed: Vec<ChatMessageMetaRef<'_>> = serde_json::from_str(&json).unwrap();
        let (snippet, count) = search_messages(&parsed, "needle").unwrap();
        assert_eq!(count, 1);
        assert!(snippet.starts_with('…') && snippet.ends_with('…'));
        assert!(snippet.contains("Needle"));
        assert!(snippet.chars().count() <= SNIPPET_BEFORE + "Needle".len() + SNIPPET_AFTER + 2);
    }

    #[test]
    fn folding_maps_non_ascii_matches_back_to_the_original() {
        let text = "Ärger mit İstanbul";
        let (folded, offsets) = fold(text);
        assert!(folded.starts_with("ärger"));
        let at = offsets[folded.find("stanbul").unwrap()];
        assert_eq!(&text[at..], "stanbul");
        assert_eq!(strip_image_markers("a ![image](file:///x.png) b"), "a  b");
    }
}
