//! Who sent a user-role message.
//!
//! A thread's transcript holds more `user` messages than the user typed: a
//! task or message another thread's agent delivered (`start_session`,
//! `fan_out`, `message_session`), the wake the app sends when delegates
//! finish or ask, and the prompt the app continues with after it compacted
//! the thread. The app writes each of those into the message's own text,
//! because the text is the one thing every harness keeps: the built-in
//! history and an external harness's replay both carry it, and neither
//! carries a flag (`delivered`, `wakeMessage` and `CONTINUE_MARK` in
//! `apps/desktop/src/state`).
//!
//! This module reads that text back, in one place, so a client that did not
//! write it (a paired phone, the app on another computer) is told the origin
//! instead of parsing the wording: `RemoteMessage::origin` in the remote
//! protocol, `_meta.rustcode.origin` on an ACP `user_message_chunk`. The
//! message text itself is sent unchanged.

use serde_json::{Value, json};

use crate::app::ChatMessage;

/// The heading every agent-delivered message starts with (`delivered` in the app).
pub(crate) const AGENT_HEADING: &str = "From thread “";
const AFTER_TITLE: &str = "” (agent):";
/// The line right under the heading that names the sender by id:
/// `session <id>`, then ` · link_id <id>` when the sender waits for a reply
/// (`delivered` in the app). Older messages have none, and keep their title.
const SENDER_LINE: &str = "session ";
const LINK_BIT: &str = " · link_id ";
/// What a wake message ends with (`WAKE_MARK` in the app).
const WAKE_MARK: &str = "<!-- rustcode:wake -->";
/// What the app's continue-after-a-compaction prompt ends with (`CONTINUE_MARK`).
const CONTINUE_MARK: &str = "<!-- rustcode:continued -->";

/// One delegate a wake reports: `- link_id <id> · <status> · “<title>” · … · session <id> · …`.
const LINK_LINE: &str = "- link_id ";
const BIT: &str = " · ";
const SESSION_BIT: &str = " · session ";
/// A delegate a wake reports as stuck, which it names by title alone.
const STALLED_LINE: &str = "- Delegate “";
const AFTER_STALLED_TITLE: &str = "” has made no progress for ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OriginKind {
    /// The user wrote it, here or on a paired device.
    User,
    /// Another thread's agent sent it: a delegated task or a message.
    Agent,
    /// The app woke the thread because its delegates finished, failed, ask or stalled.
    Delegate,
    /// The app's own prompt, such as the one that continues after a compaction.
    System,
}

impl OriginKind {
    /// The name the remote protocol and ACP send.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            OriginKind::User => "user",
            OriginKind::Agent => "agent",
            OriginKind::Delegate => "delegate",
            OriginKind::System => "system",
        }
    }
}

/// A thread a message came from, with what the message says about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OriginThread {
    pub(crate) session_id: Option<String>,
    pub(crate) title: Option<String>,
    /// A delegate's state as the wake reports it: `done`, `failed`,
    /// `needs_user` or `stalled`.
    pub(crate) status: Option<String>,
    pub(crate) link_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageOrigin {
    pub(crate) kind: OriginKind,
    /// The sender of an agent's message, or each delegate a wake reports.
    pub(crate) threads: Vec<OriginThread>,
}

impl MessageOrigin {
    fn of_kind(kind: OriginKind) -> Self {
        Self {
            kind,
            threads: Vec::new(),
        }
    }

    /// The origin as ACP carries it in `_meta.rustcode.origin`.
    pub(crate) fn to_meta(&self) -> Value {
        let threads = self
            .threads
            .iter()
            .map(|thread| {
                let mut entry = serde_json::Map::new();
                for (key, value) in [
                    ("sessionId", &thread.session_id),
                    ("title", &thread.title),
                    ("status", &thread.status),
                    ("linkId", &thread.link_id),
                ] {
                    if let Some(value) = value {
                        entry.insert(key.to_owned(), Value::String(value.clone()));
                    }
                }
                Value::Object(entry)
            })
            .collect::<Vec<_>>();
        if threads.is_empty() {
            json!({ "kind": self.kind.as_str() })
        } else {
            json!({ "kind": self.kind.as_str(), "threads": threads })
        }
    }
}

/// The title in an agent-delivered message's heading, when `text` is one.
pub(crate) fn delivered_title(text: &str) -> Option<&str> {
    let rest = text.trim_start().strip_prefix(AGENT_HEADING)?;
    let (title, _) = rest.split_once(AFTER_TITLE)?;
    let title = title.trim();
    (!title.is_empty()).then_some(title)
}

/// The sender's session and link ids on the line right under a delivered
/// message's heading; `None` for a message without one (older ones).
pub(crate) fn delivered_sender(text: &str) -> Option<(String, Option<String>)> {
    let rest = text.trim_start().strip_prefix(AGENT_HEADING)?;
    let (_, after) = rest.split_once(AFTER_TITLE)?;
    let line = after.strip_prefix('\n')?.lines().next()?;
    let ids = line.strip_prefix(SENDER_LINE)?;
    let (session, link) = match ids.split_once(LINK_BIT) {
        Some((session, link)) => (session, Some(link)),
        None => (ids, None),
    };
    let id = |text: &str| {
        let text = text.trim();
        (!text.is_empty() && !text.contains(char::is_whitespace)).then(|| text.to_owned())
    };
    let session = id(session)?;
    Some((session, link.and_then(id)))
}

/// The origin of a message of `history`: `None` for anything that is not a
/// user request (replies, tool results, a tool's attachment).
pub(crate) fn of_message(message: &ChatMessage) -> Option<MessageOrigin> {
    message.is_user_request().then(|| {
        message
            .origin
            .as_ref()
            .and_then(from_meta)
            .unwrap_or_else(|| of_text(&message.content))
    })
}

/// Decode a structured ACP origin, falling back to text when it is absent or invalid.
pub(crate) fn from_meta(value: &Value) -> Option<MessageOrigin> {
    let kind = match value.get("kind")?.as_str()? {
        "user" => OriginKind::User,
        "agent" => OriginKind::Agent,
        "delegate" => OriginKind::Delegate,
        "system" => OriginKind::System,
        _ => return None,
    };
    let threads = value
        .get("threads")
        .and_then(Value::as_array)
        .map(|threads| {
            threads
                .iter()
                .filter_map(|entry| {
                    let entry = entry.as_object()?;
                    let get = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
                    Some(OriginThread {
                        session_id: get("sessionId"),
                        title: get("title"),
                        status: get("status"),
                        link_id: get("linkId"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(MessageOrigin { kind, threads })
}

/// The origin of a prompt's text.
pub(crate) fn of_text(text: &str) -> MessageOrigin {
    // The heading comes first: a delegated task may quote a mark in its body.
    if text.trim_start().starts_with(AGENT_HEADING) {
        // The ids the app writes under the heading come first; a message
        // from before them names its sender by title alone.
        let (session_id, link_id) = delivered_sender(text).unzip();
        let title = delivered_title(text).map(str::to_owned);
        let threads = (title.is_some() || session_id.is_some())
            .then(|| OriginThread {
                session_id,
                title,
                link_id: link_id.flatten(),
                ..OriginThread::default()
            })
            .into_iter()
            .collect();
        return MessageOrigin {
            kind: OriginKind::Agent,
            threads,
        };
    }
    if text.contains(WAKE_MARK) {
        return MessageOrigin {
            kind: OriginKind::Delegate,
            threads: text.lines().filter_map(wake_thread).collect(),
        };
    }
    if text.contains(CONTINUE_MARK) {
        return MessageOrigin::of_kind(OriginKind::System);
    }
    MessageOrigin::of_kind(OriginKind::User)
}

/// What the data store records of a wake turn (`thread.wake`, `docs/data-store.md`): how many
/// delegates the one message reported, split by what it said of them. Counts only; the wake's text
/// stays in the thread. `None` for any other message. The turn's own tokens and cost are on its
/// `turns` row, which a reader joins by thread and time (issue 3649).
pub(crate) fn wake_event_fields(text: &str) -> Option<Value> {
    let origin = of_text(text);
    if origin.kind != OriginKind::Delegate {
        return None;
    }
    let count = |status: &str| {
        origin
            .threads
            .iter()
            .filter(|thread| thread.status.as_deref() == Some(status))
            .count()
    };
    Some(json!({
        "delegates": origin.threads.len(),
        "finished": count("done") + count("failed"),
        "failed": count("failed"),
        "asking": count("needs_user"),
        "stalled": count("stalled"),
    }))
}

/// Writes the `thread.wake` event for a prompt that is a wake; nothing for any other prompt.
pub(crate) fn record_wake(session_id: &str, text: &str) {
    if let Some(mut fields) = wake_event_fields(text) {
        fields["session_id"] = Value::String(session_id.to_owned());
        crate::logger::operational_event("thread.wake", fields);
    }
}

/// The delegate one line of a wake message names. A delegate's answer is
/// indented under its line, so only the app's own lines start at the margin.
fn wake_thread(line: &str) -> Option<OriginThread> {
    if let Some(rest) = line.strip_prefix(STALLED_LINE) {
        let (title, _) = rest.split_once(AFTER_STALLED_TITLE)?;
        return Some(OriginThread {
            title: non_empty(title),
            status: Some("stalled".to_owned()),
            ..OriginThread::default()
        });
    }
    let rest = line.strip_prefix(LINK_LINE)?;
    let (link_id, rest) = rest.split_once(BIT)?;
    let (status, rest) = rest.split_once(BIT).unwrap_or((rest, ""));
    // A title may hold a separator of its own; nothing after it holds a
    // closing quote, and the thread is the last `session` bit.
    let (before_session, session_id) = match rest.rfind(SESSION_BIT) {
        Some(at) => {
            let after = &rest[at + SESSION_BIT.len()..];
            let id = after.split_once(BIT).map_or(after, |(id, _)| id);
            (&rest[..at], non_empty(id))
        }
        None => (rest, None),
    };
    let title = before_session
        .strip_prefix('“')
        .and_then(|quoted| quoted.rfind('”').map(|end| &quoted[..end]))
        .and_then(non_empty);
    Some(OriginThread {
        session_id: session_id.filter(|id| !id.contains(char::is_whitespace)),
        title,
        status: non_empty(status).filter(|status| !status.contains(char::is_whitespace)),
        link_id: non_empty(link_id).filter(|id| !id.contains(char::is_whitespace)),
    })
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A whole user message as an ACP `user_message_chunk`, with its origin in
/// `_meta.rustcode.origin` beside whatever `meta` already holds.
pub(crate) fn user_chunk(
    text: String,
    meta: Option<serde_json::Map<String, Value>>,
) -> agent_client_protocol::schema::v1::ContentChunk {
    use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, TextContent};
    let mut meta = meta.unwrap_or_default();
    let origin = meta
        .get("rustcode")
        .and_then(|v| v.get("origin"))
        .and_then(from_meta)
        .unwrap_or_else(|| of_text(&text))
        .to_meta();
    let rustcode = meta.entry("rustcode").or_insert_with(|| json!({}));
    if !rustcode.is_object() {
        *rustcode = json!({});
    }
    if let Some(rustcode) = rustcode.as_object_mut() {
        rustcode.insert("origin".to_owned(), origin);
    }
    ContentChunk::new(ContentBlock::Text(TextContent::new(text))).meta(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wake as the app writes it (`wakeMessage` in `state/orchestration.ts`).
    const WAKE: &str = "2 delegates finished · 1 failed · Delegate asks: Pick a colour\n\
\n\
Finished:\n\
\n\
- link_id 20261111-aaaa/1 · done · “Review the sidebar rows” · Pi · worked 3m 49s · session 20261111-bbbb · 4 files changed · 12s ago\n\
\x20\x20answer: Done.\n\
\x20\x20- link_id not/1 · done · “a line of its answer” · session not-a-delegate\n\
- link_id 20261111-aaaa/2 · failed · “Fix A · B” · resumed after a restart · session 20261111-cccc\n\
\x20\x20answer: (nothing it said came back)\n\
\n\
Waiting on you (the user answers these; you cannot):\n\
\n\
- link_id 20261111-aaaa/3 · needs_user · “Pick a colour” · Codex · session 20261111-dddd\n\
\n\
No progress while the turn still runs:\n\
\n\
- Delegate “Build the thing” has made no progress for 12m; last tool: none reported. It may be stuck on a command that does not exit; interrupt_session it or message it.\n\
\n\
The rest is in read_session on a child's session id.\n\
\n\
<!-- rustcode:wake -->";

    fn thread(
        session_id: Option<&str>,
        title: &str,
        status: &str,
        link_id: Option<&str>,
    ) -> OriginThread {
        OriginThread {
            session_id: session_id.map(str::to_owned),
            title: Some(title.to_owned()),
            status: Some(status.to_owned()),
            link_id: link_id.map(str::to_owned),
        }
    }

    #[test]
    fn a_typed_message_is_the_users() {
        assert_eq!(
            of_text("Why does the parser test fail?"),
            MessageOrigin::of_kind(OriginKind::User)
        );
        // The heading is the app's only at the start of the message.
        assert_eq!(
            of_text("it said: From thread “x” (agent): hello").kind,
            OriginKind::User
        );
    }

    #[test]
    fn a_delivered_message_names_the_thread_that_sent_it() {
        let origin = of_text("From thread “Ship the delegate tree” (agent):\n\nReview the rules.");
        assert_eq!(origin.kind, OriginKind::Agent);
        assert_eq!(
            origin.threads,
            vec![OriginThread {
                title: Some("Ship the delegate tree".to_owned()),
                ..OriginThread::default()
            }]
        );
        // A task that quotes the wake mark is still the agent's task.
        let quoting = of_text("From thread “a” (agent):\n\nlook for <!-- rustcode:wake --> in it");
        assert_eq!(quoting.kind, OriginKind::Agent);
        // An untitled sender is still an agent.
        let untitled = of_text("From thread “” (agent):\n\nx");
        assert_eq!(untitled.kind, OriginKind::Agent);
        assert!(untitled.threads.is_empty());
    }

    #[test]
    fn structured_origin_survives_history_and_overrides_text() {
        let mut message =
            ChatMessage::new("user", "From thread “same title” (agent):\n\nsome task");
        message.origin = Some(
            json!({"kind": "agent", "threads": [{"sessionId": "sender-2", "linkId": "sender-2/1"}]}),
        );
        let saved = serde_json::to_string(&message).unwrap();
        let restored: ChatMessage = serde_json::from_str(&saved).unwrap();
        let origin = of_message(&restored).unwrap();
        assert_eq!(origin.threads[0].session_id.as_deref(), Some("sender-2"));
        assert_eq!(origin.threads[0].link_id.as_deref(), Some("sender-2/1"));
        let meta =
            serde_json::Map::from_iter([("rustcode".into(), json!({"origin": restored.origin}))]);
        let chunk = user_chunk(restored.content, Some(meta));
        assert_eq!(
            chunk.meta.unwrap()["rustcode"]["origin"]["threads"][0]["sessionId"],
            "sender-2"
        );
    }

    #[test]
    fn a_delivered_message_with_ids_names_the_sender_by_id() {
        let origin = of_text(
            "From thread “Ship it” (agent):\nsession 20261113-aaaa · link_id 20261113-aaaa/2\n\nReview the rules.",
        );
        assert_eq!(origin.kind, OriginKind::Agent);
        assert_eq!(
            origin.threads,
            vec![OriginThread {
                session_id: Some("20261113-aaaa".to_owned()),
                title: Some("Ship it".to_owned()),
                link_id: Some("20261113-aaaa/2".to_owned()),
                status: None,
            }]
        );
        // A one-way message has no link.
        let one_way = of_text("From thread “Ship it” (agent):\nsession 20261113-aaaa\n\nFYI.");
        assert_eq!(
            one_way.threads[0].session_id.as_deref(),
            Some("20261113-aaaa")
        );
        assert_eq!(one_way.threads[0].link_id, None);
        // A body that starts with the word after the blank line is not a sender line.
        let body = of_text("From thread “a” (agent):\n\nsession notes are below");
        assert_eq!(body.threads[0].session_id, None);
        // The heading still reads on its own, so the title survives for older readers.
        assert_eq!(
            delivered_title("From thread “Ship it” (agent):\nsession x\n\nbody"),
            Some("Ship it")
        );
    }

    #[test]
    fn a_wake_names_every_delegate_it_reports() {
        let origin = of_text(WAKE);
        assert_eq!(origin.kind, OriginKind::Delegate);
        assert_eq!(
            origin.threads,
            vec![
                thread(
                    Some("20261111-bbbb"),
                    "Review the sidebar rows",
                    "done",
                    Some("20261111-aaaa/1"),
                ),
                thread(
                    Some("20261111-cccc"),
                    "Fix A · B",
                    "failed",
                    Some("20261111-aaaa/2"),
                ),
                thread(
                    Some("20261111-dddd"),
                    "Pick a colour",
                    "needs_user",
                    Some("20261111-aaaa/3"),
                ),
                thread(None, "Build the thing", "stalled", None),
            ]
        );
    }

    #[test]
    fn a_wake_is_counted_by_what_it_reports() {
        let fields = wake_event_fields(WAKE).unwrap();
        assert_eq!(fields["delegates"], 4);
        assert_eq!(fields["finished"], 2);
        assert_eq!(fields["failed"], 1);
        assert_eq!(fields["asking"], 1);
        assert_eq!(fields["stalled"], 1);
        // A wake in wording this release does not know is still one wake, of no known delegates.
        let unknown = wake_event_fields("Delegate finished: x\n\n<!-- rustcode:wake -->").unwrap();
        assert_eq!(unknown["delegates"], 0);
        assert_eq!(wake_event_fields("Why does the parser test fail?"), None);
        assert_eq!(wake_event_fields("From thread “a” (agent):\n\nhello"), None);
    }

    #[test]
    fn a_wake_in_wording_this_release_does_not_know_is_still_a_wake() {
        let origin = of_text("Delegate finished: something\n\n<!-- rustcode:wake -->");
        assert_eq!(origin, MessageOrigin::of_kind(OriginKind::Delegate));
    }

    #[test]
    fn the_apps_continue_prompt_is_the_systems() {
        let origin = of_text("Continue where you left off.\n\n<!-- rustcode:continued -->");
        assert_eq!(origin, MessageOrigin::of_kind(OriginKind::System));
    }

    #[test]
    fn only_a_user_request_has_an_origin() {
        assert_eq!(
            of_message(&ChatMessage::new("user", "hello")).map(|origin| origin.kind),
            Some(OriginKind::User)
        );
        assert_eq!(of_message(&ChatMessage::new("assistant", "hello")), None);
        assert_eq!(
            of_message(&ChatMessage::new("user", "<tool_result>ok</tool_result>")),
            None
        );
    }

    #[test]
    fn the_acp_meta_carries_the_kind_and_the_threads() {
        assert_eq!(
            of_text("hello").to_meta(),
            json!({ "kind": "user" }),
            "no empty list"
        );
        assert_eq!(
            of_text(WAKE).to_meta()["threads"][0],
            json!({
                "sessionId": "20261111-bbbb",
                "title": "Review the sidebar rows",
                "status": "done",
                "linkId": "20261111-aaaa/1",
            })
        );
        assert_eq!(
            of_text(WAKE).to_meta()["threads"][3],
            json!({ "title": "Build the thing", "status": "stalled" })
        );
    }

    #[test]
    fn a_user_chunk_keeps_the_meta_it_was_given() {
        let chunk = user_chunk(
            "From thread “Parent” (agent):\n\ndo it".to_owned(),
            Some(serde_json::Map::from_iter([(
                "rustcode".to_owned(),
                json!({ "at": "2026-11-11T10:00:00Z" }),
            )])),
        );
        let meta = chunk.meta.expect("meta");
        assert_eq!(meta["rustcode"]["at"], "2026-11-11T10:00:00Z");
        assert_eq!(meta["rustcode"]["origin"]["kind"], "agent");
        assert_eq!(meta["rustcode"]["origin"]["threads"][0]["title"], "Parent");

        let plain = user_chunk("hello".to_owned(), None);
        assert_eq!(
            plain.meta.expect("meta")["rustcode"]["origin"],
            json!({ "kind": "user" })
        );
    }
}
