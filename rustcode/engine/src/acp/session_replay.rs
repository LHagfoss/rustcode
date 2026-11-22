//! `_rustcode/session/replay`: the transcript `session/load` would replay,
//! without opening the thread. Another computer's app reads it to keep a
//! complete copy of this machine's threads (docs/data-store.md › Thread
//! mirror). Nothing is attached, started or closed, so a thread someone has
//! open here, in another app or a terminal, is not touched; the answer is
//! what its history file holds.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/session/replay", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplaySessionRequest {
    pub session_id: String,
}

/// `{sessionId, cwd, title, updates: [..]}`: each update as a
/// `session/update` notification carries it.
pub(crate) fn replay_session(session_id: &str) -> Result<Value, agent_client_protocol::Error> {
    let invalid = |message: String| agent_client_protocol::Error::invalid_params().data(message);
    if !super::valid_acp_session_id(session_id) {
        return Err(invalid("invalid session id".to_owned()));
    }
    let workspace = crate::sessions::load_session_workspace(session_id).map(|w| w.cwd);
    // A thread that ran on an external harness replays its recorded
    // transcript, as `harnesses::load_session` does.
    if let Some(record) = crate::harness::store::load_for_restore(session_id).map_err(invalid)? {
        let history = crate::sessions::load_session_history_direct(session_id);
        let cwd = workspace.unwrap_or_else(|| record.cwd.clone());
        let updates = crate::harness::transcript::replay(&history, Some(&cwd));
        return Ok(json!({
            "sessionId": session_id,
            "cwd": cwd,
            "title": record.title,
            "updates": updates,
        }));
    }
    let (meta, history) = crate::sessions::load_session_by_id(session_id)
        .ok_or_else(|| invalid(format!("unknown session: {session_id}")))?;
    let updates = super::session_load::replay_history_updates(&history, workspace.as_deref());
    Ok(json!({
        "sessionId": session_id,
        "cwd": workspace,
        "title": meta.title,
        "updates": updates,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_an_invalid_or_unknown_id() {
        assert!(replay_session("../escape").is_err());
        assert!(replay_session("no-such-thread-0000").is_err());
    }
}
