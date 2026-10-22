//! Steering a running ACP turn (#2255): `_rustcode/steer` hands the turn a
//! message to read at its next safe boundary, after the tool calls it is
//! running, instead of waiting in the client's queue for the next turn. It
//! uses the same `pending_steers` as the terminal's steering.
//!
//! The turn only takes steers while it streams with nothing waiting on the
//! user; otherwise the request answers `accepted: false` and the client
//! queues the message instead. A steer the turn ended before reading is
//! handed back in the prompt response's `_meta.rustcode.unusedSteers`, so the
//! client can send it as the next turn and nothing is lost.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const STEER_CAPABILITY_VERSION: u64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/steer", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SteerRequest {
    pub session_id: String,
    pub text: String,
}

/// Mark the session's ACP turn as one that takes steers.
pub(crate) fn begin(state: &mut crate::app::AppState, session_id: &str) {
    state.active_turn_steerable_session =
        (state.active_session_id == session_id).then(|| session_id.to_owned());
}

/// The turn is over: it takes no more steers, and those it never read are
/// returned for the client to send next.
pub(crate) fn end(state: &mut crate::app::AppState, session_id: &str) -> Vec<String> {
    if state.active_turn_steerable_session.as_deref() == Some(session_id) {
        state.active_turn_steerable_session = None;
    }
    let (unused, rest) = std::mem::take(&mut state.pending_steers)
        .into_iter()
        .partition::<Vec<_>, _>(|steer| steer.session_id == session_id);
    state.pending_steers = rest;
    unused.into_iter().map(|steer| steer.text).collect()
}

/// Accept `text` as a steer for the running turn, or refuse it.
pub(crate) fn accept(state: &mut crate::app::AppState, text: &str) -> Value {
    let accepted =
        crate::app::submit_detached_prompt(state, text, crate::app::DraftSubmitMode::Steer)
            == crate::app::SubmitOutcome::Steered;
    json!({ "accepted": accepted })
}

/// The prompt response's `_meta` for steers the turn did not read.
pub(crate) fn unused_meta(unused: Vec<String>) -> Option<serde_json::Map<String, Value>> {
    (!unused.is_empty()).then(|| {
        serde_json::Map::from_iter([("rustcode".to_owned(), json!({ "unusedSteers": unused }))])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppState, AppStatus};

    fn streaming() -> (AppState, String) {
        let mut state = AppState::new();
        let session = state.active_session_id.clone();
        state.status = AppStatus::Streaming;
        begin(&mut state, &session);
        (state, session)
    }

    #[test]
    fn steerable_turn_takes_steers_and_hands_back_unread_ones() {
        let (mut state, session) = streaming();
        assert_eq!(
            accept(&mut state, "  change course "),
            json!({"accepted": true})
        );
        assert_eq!(state.pending_steers[0].text, "change course");
        assert_eq!(accept(&mut state, "   "), json!({"accepted": false}));

        assert_eq!(end(&mut state, &session), ["change course"]);
        assert!(state.pending_steers.is_empty());
        assert_eq!(state.active_turn_steerable_session, None);
        assert_eq!(accept(&mut state, "too late"), json!({"accepted": false}));
    }

    #[test]
    fn turn_waiting_on_the_user_refuses_steers() {
        let (mut state, _) = streaming();
        state.status = AppStatus::Idle;
        assert_eq!(accept(&mut state, "later"), json!({"accepted": false}));
    }

    #[test]
    fn another_session_turn_is_not_marked() {
        let mut state = AppState::new();
        begin(&mut state, "someone-else");
        assert_eq!(state.active_turn_steerable_session, None);
    }

    #[test]
    fn unused_steers_ride_in_the_response_meta() {
        assert_eq!(unused_meta(Vec::new()), None);
        assert_eq!(
            Value::Object(unused_meta(vec!["a".into()]).unwrap()),
            json!({"rustcode": {"unusedSteers": ["a"]}})
        );
    }

    #[test]
    fn request_uses_camel_case() {
        let request: SteerRequest =
            serde_json::from_value(json!({"sessionId": "s", "text": "t"})).unwrap();
        assert_eq!(
            (request.session_id.as_str(), request.text.as_str()),
            ("s", "t")
        );
    }
}
