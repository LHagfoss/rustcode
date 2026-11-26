//! `_rustcode/session/replay`: the transcript `session/load` would replay,
//! without opening the thread. Another computer's app reads it to keep a
//! complete copy of this machine's threads (docs/data-store.md › Thread
//! mirror). Nothing is attached, started or closed, so a thread someone has
//! open here, in another app or a terminal, is not touched; the answer is
//! what its history file holds.

use agent_client_protocol::schema::v1::SessionUpdate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Target byte budget for one `_rustcode/session/replay` response, kept well
/// below `remote_gateway::gateway::MAX_FRAME_BYTES` (1 MiB) so replaying a
/// large thread never closes the remote ACP connection.
pub(crate) const MAX_REPLAY_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/session/replay", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplaySessionRequest {
    pub session_id: String,
    #[serde(default)]
    pub cursor: Option<usize>,
}

/// `{sessionId, cwd, title, updates: [..], nextCursor?}`: one page of updates
/// as `session/update` notifications carry them, bounded to `MAX_REPLAY_BYTES`.
pub(crate) fn replay_session(
    session_id: &str,
    cursor: Option<usize>,
) -> Result<Value, agent_client_protocol::Error> {
    let invalid = |message: String| agent_client_protocol::Error::invalid_params().data(message);
    if !super::valid_acp_session_id(session_id) {
        return Err(invalid("invalid session id".to_owned()));
    }
    let workspace = crate::sessions::load_session_workspace(session_id).map(|w| w.cwd);
    // A thread that ran on an external harness replays its recorded
    // transcript, as `harnesses::load_session` does.
    let (cwd, title, updates) = if let Some(record) =
        crate::harness::store::load_for_restore(session_id).map_err(invalid)?
    {
        let history = crate::sessions::load_session_history_direct(session_id);
        let cwd = workspace.unwrap_or_else(|| record.cwd.clone());
        let updates = crate::harness::transcript::replay(&history, Some(&cwd));
        (json!(cwd), json!(record.title), updates)
    } else {
        let (meta, history) = crate::sessions::load_session_by_id(session_id)
            .ok_or_else(|| invalid(format!("unknown session: {session_id}")))?;
        let updates = super::session_load::replay_history_updates(&history, workspace.as_deref());
        (json!(workspace), json!(meta.title), updates)
    };
    let (page, next_cursor) = paginate_updates(updates, cursor, MAX_REPLAY_BYTES);
    let mut body = json!({
        "sessionId": session_id,
        "cwd": cwd,
        "title": title,
        "updates": page,
    });
    if let Some(next_cursor) = next_cursor {
        body["nextCursor"] = json!(next_cursor);
    }
    Ok(body)
}

fn paginate_updates(
    updates: Vec<SessionUpdate>,
    cursor: Option<usize>,
    max_bytes: usize,
) -> (Vec<Value>, Option<usize>) {
    let total = updates.len();
    let start = cursor.unwrap_or(0).min(total);
    let mut page = Vec::new();
    let mut used = 0usize;
    let mut next = start;
    for update in updates.into_iter().skip(start) {
        let Ok(value) = serde_json::to_value(&update) else {
            next += 1;
            continue;
        };
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
        if size > max_bytes {
            if page.is_empty() {
                crate::dbg_log!(
                    "session/replay: skipped oversized update ({size} bytes > {max_bytes})"
                );
                next += 1;
                continue;
            }
            break;
        }
        let added = if page.is_empty() { size } else { size + 1 };
        if !page.is_empty() && used.saturating_add(added) > max_bytes {
            break;
        }
        used = used.saturating_add(added);
        page.push(value);
        next += 1;
    }
    let next_cursor = (next < total).then_some(next);
    (page, next_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, TextContent};

    fn text_update(text: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    #[test]
    fn refuses_an_invalid_or_unknown_id() {
        assert!(replay_session("../escape", None).is_err());
        assert!(replay_session("no-such-thread-0000", None).is_err());
    }

    #[test]
    fn paginates_updates_within_the_byte_budget() {
        let updates = vec![
            text_update(&"a".repeat(80)),
            text_update(&"b".repeat(80)),
            text_update(&"c".repeat(80)),
            text_update(&"d".repeat(80)),
        ];
        let single_size = serde_json::to_vec(&updates[0]).unwrap().len();
        let budget = single_size * 2 + 5;

        let (first, cursor1) = paginate_updates(updates.clone(), None, budget);
        assert_eq!(first.len(), 2);
        assert_eq!(cursor1, Some(2));

        let (second, cursor2) = paginate_updates(updates, cursor1, budget);
        assert_eq!(second.len(), 2);
        assert_eq!(cursor2, None);
    }

    #[test]
    fn skips_a_single_update_larger_than_the_budget_and_keeps_progressing() {
        let updates = vec![
            text_update("first"),
            text_update(&"x".repeat(500)),
            text_update("last"),
        ];
        let budget = 200;

        let (first, cursor1) = paginate_updates(updates.clone(), None, budget);
        assert_eq!(first.len(), 1);
        assert_eq!(cursor1, Some(1));

        let (second, cursor2) = paginate_updates(updates, cursor1, budget);
        assert_eq!(second.len(), 1);
        assert_eq!(cursor2, None);
        assert!(second[0].to_string().contains("last"));
    }
}
