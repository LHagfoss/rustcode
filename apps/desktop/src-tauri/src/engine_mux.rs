//! JSON-RPC multiplexer between one local engine process and multiple app windows.
//!
//! Each desktop window owns its own `AcpConnection` with independent numeric request
//! IDs starting at 1. This multiplexer rewrites outgoing client request IDs to unique
//! engine-scoped integers, restores each window's original ID on the matching response,
//! caches the current engine generation's `initialize` result for additional windows,
//! scopes `session/load` history replay notifications to the loading window(s),
//! broadcasts live notifications and permission prompts, deduplicates permission
//! responses, and routes orchestration requests to the primary window.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Canonical key for a JSON-RPC request ID (`number` or `string`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RpcIdKey {
    Int(i64),
    Uint(u64),
    Str(String),
}

impl RpcIdKey {
    /// Extracts a canonical key from a JSON-RPC `id` field.
    pub fn from_value(value: &Value) -> Option<Self> {
        if let Some(n) = value.as_u64() {
            return Some(Self::Uint(n));
        }
        if let Some(n) = value.as_i64() {
            return Some(Self::Int(n));
        }
        value.as_str().map(|s| Self::Str(s.to_owned()))
    }
}

#[derive(Debug, Clone)]
struct PendingRequest {
    window: String,
    original_id: Value,
    is_initialize: bool,
    loading_session: Option<String>,
}

/// Action the engine supervisor should take for one line sent by a window.
#[derive(Debug, Clone, PartialEq)]
pub enum OutboundDecision {
    /// Write this JSON-RPC line to engine stdin.
    Forward(String),
    /// Reply directly to `window` without writing to engine stdin.
    ReplyLocal { window: String, line: String },
    /// An `initialize` request is already in flight; the window was queued.
    QueueInit,
    /// Duplicate response to an engine request another window already answered.
    Drop,
    /// Write this permission response to engine stdin and notify other windows.
    ForwardAndNotifyPermissionResolved { line: String, rpc_id: Value },
}

/// Action the engine supervisor should take for one line printed by engine stdout.
#[derive(Debug, Clone, PartialEq)]
pub enum InboundDecision {
    /// Broadcast this line to every attached window.
    Broadcast(String),
    /// Deliver specific `(window_label, line)` frames.
    Deliver(Vec<(String, String)>),
}

/// Stateful JSON-RPC multiplexer for one running engine generation.
#[derive(Debug, Default)]
pub struct RpcMux {
    next_rpc_id: u64,
    pending_requests: HashMap<u64, PendingRequest>,
    initialize_result: Option<Value>,
    initialize_in_flight: bool,
    initialize_waiters: Vec<(String, Value)>,
    loading_sessions: HashMap<String, Vec<String>>,
    in_flight_server_requests: HashMap<RpcIdKey, String>,
}

impl RpcMux {
    /// Resets all routing tables when a new engine process starts.
    pub fn reset(&mut self) {
        self.next_rpc_id = 0;
        self.pending_requests.clear();
        self.initialize_result = None;
        self.initialize_in_flight = false;
        self.initialize_waiters.clear();
        self.loading_sessions.clear();
        self.in_flight_server_requests.clear();
    }

    /// Drops pending entries belonging to a closed window.
    pub fn detach_window(&mut self, window: &str) {
        self.pending_requests
            .retain(|_, pending| pending.window != window);
        self.initialize_waiters.retain(|(label, _)| label != window);
        self.loading_sessions.retain(|_, windows| {
            windows.retain(|label| label != window);
            !windows.is_empty()
        });
    }

    /// Processes a JSON-RPC line sent from `window` toward the engine.
    pub fn outbound(&mut self, window: &str, line: &str) -> OutboundDecision {
        let Ok(mut value) = serde_json::from_str::<Value>(line) else {
            return OutboundDecision::Forward(line.to_owned());
        };
        let Some(object) = value.as_object_mut() else {
            return OutboundDecision::Forward(line.to_owned());
        };

        let method = object
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let original_id = object.get("id").cloned();

        match (method, original_id) {
            (Some(method), Some(orig_id)) if !orig_id.is_null() => {
                if method == "initialize" {
                    if let Some(cached) = &self.initialize_result {
                        let reply = json!({
                            "jsonrpc": "2.0",
                            "id": orig_id,
                            "result": cached,
                        });
                        return OutboundDecision::ReplyLocal {
                            window: window.to_owned(),
                            line: reply.to_string(),
                        };
                    }
                    if self.initialize_in_flight {
                        self.initialize_waiters.push((window.to_owned(), orig_id));
                        return OutboundDecision::QueueInit;
                    }
                    self.initialize_in_flight = true;
                }

                let loading_session = if method == "session/load" {
                    object
                        .get("params")
                        .and_then(Value::as_object)
                        .and_then(|params| params.get("sessionId"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                };

                if let Some(session_id) = &loading_session {
                    self.loading_sessions
                        .entry(session_id.clone())
                        .or_default()
                        .push(window.to_owned());
                }

                self.next_rpc_id = self.next_rpc_id.saturating_add(1);
                let engine_id = self.next_rpc_id;
                object.insert("id".to_owned(), json!(engine_id));
                self.pending_requests.insert(
                    engine_id,
                    PendingRequest {
                        window: window.to_owned(),
                        original_id: orig_id,
                        is_initialize: method == "initialize",
                        loading_session,
                    },
                );
                OutboundDecision::Forward(value.to_string())
            }
            (Some(_), _) => OutboundDecision::Forward(line.to_owned()),
            (None, Some(rpc_id)) => {
                let Some(key) = RpcIdKey::from_value(&rpc_id) else {
                    return OutboundDecision::Forward(line.to_owned());
                };
                match self.in_flight_server_requests.remove(&key) {
                    Some(method) if method == "session/request_permission" => {
                        OutboundDecision::ForwardAndNotifyPermissionResolved {
                            line: line.to_owned(),
                            rpc_id,
                        }
                    }
                    Some(_) => OutboundDecision::Forward(line.to_owned()),
                    None => OutboundDecision::Drop,
                }
            }
            (None, None) => OutboundDecision::Forward(line.to_owned()),
        }
    }

    /// Processes a JSON-RPC line printed by the engine toward attached windows.
    pub fn inbound(&mut self, line: &str, primary_window: Option<&str>) -> InboundDecision {
        let Ok(mut value) = serde_json::from_str::<Value>(line) else {
            return InboundDecision::Broadcast(line.to_owned());
        };
        let Some(object) = value.as_object_mut() else {
            return InboundDecision::Broadcast(line.to_owned());
        };

        let method = object
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = object.get("id").cloned();

        match (method, id) {
            (None, Some(id_val)) => {
                let Some(engine_id) = id_val.as_u64() else {
                    return InboundDecision::Broadcast(line.to_owned());
                };
                let Some(pending) = self.pending_requests.remove(&engine_id) else {
                    return InboundDecision::Deliver(Vec::new());
                };

                if let Some(session_id) = &pending.loading_session {
                    self.remove_loading_window(session_id, &pending.window);
                }

                let mut deliveries = Vec::new();
                if pending.is_initialize {
                    self.initialize_in_flight = false;
                    let error = object.get("error").cloned();
                    let result = object.get("result").cloned();
                    if let (None, Some(res)) = (&error, &result) {
                        self.initialize_result = Some(res.clone());
                    }
                    object.insert("id".to_owned(), pending.original_id);
                    deliveries.push((pending.window, value.to_string()));
                    let waiters = std::mem::take(&mut self.initialize_waiters);
                    for (waiter_window, waiter_id) in waiters {
                        let waiter_frame = if let Some(err) = &error {
                            json!({
                                "jsonrpc": "2.0",
                                "id": waiter_id,
                                "error": err,
                            })
                        } else {
                            json!({
                                "jsonrpc": "2.0",
                                "id": waiter_id,
                                "result": result.clone().unwrap_or(Value::Null),
                            })
                        };
                        deliveries.push((waiter_window, waiter_frame.to_string()));
                    }
                    return InboundDecision::Deliver(deliveries);
                }

                object.insert("id".to_owned(), pending.original_id);
                deliveries.push((pending.window, value.to_string()));
                InboundDecision::Deliver(deliveries)
            }
            (Some(method), Some(id_val)) if !id_val.is_null() => {
                if let Some(key) = RpcIdKey::from_value(&id_val) {
                    self.in_flight_server_requests.insert(key, method.clone());
                }
                if method == "session/request_permission" {
                    InboundDecision::Broadcast(line.to_owned())
                } else if let Some(primary) = primary_window {
                    InboundDecision::Deliver(vec![(primary.to_owned(), line.to_owned())])
                } else {
                    InboundDecision::Broadcast(line.to_owned())
                }
            }
            (Some(method), _) => {
                if method == "session/update" {
                    let session_id = object
                        .get("params")
                        .and_then(Value::as_object)
                        .and_then(|params| params.get("sessionId"))
                        .and_then(Value::as_str);
                    if let Some(loading_windows) =
                        session_id.and_then(|id| self.loading_sessions.get(id))
                    {
                        let mut seen = HashSet::new();
                        let deliveries = loading_windows
                            .iter()
                            .filter(|label| seen.insert((*label).clone()))
                            .map(|label| (label.clone(), line.to_owned()))
                            .collect();
                        return InboundDecision::Deliver(deliveries);
                    }
                }
                InboundDecision::Broadcast(line.to_owned())
            }
            (None, None) => InboundDecision::Broadcast(line.to_owned()),
        }
    }

    fn remove_loading_window(&mut self, session_id: &str, window: &str) {
        if let Some(windows) = self.loading_sessions.get_mut(session_id) {
            if let Some(pos) = windows.iter().position(|label| label == window) {
                windows.remove(pos);
            }
            if windows.is_empty() {
                self.loading_sessions.remove(session_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaps_overlapping_ids_and_routes_responses_to_each_window() {
        let mut mux = RpcMux::default();

        let out_a = mux.outbound(
            "main",
            r#"{"jsonrpc":"2.0","id":1,"method":"session/list","params":{}}"#,
        );
        let out_b = mux.outbound(
            "window-2",
            r#"{"jsonrpc":"2.0","id":1,"method":"session/list","params":{}}"#,
        );

        let line_a = match out_a {
            OutboundDecision::Forward(l) => l,
            other => panic!("expected Forward, got {other:?}"),
        };
        let line_b = match out_b {
            OutboundDecision::Forward(l) => l,
            other => panic!("expected Forward, got {other:?}"),
        };

        let val_a: Value = serde_json::from_str(&line_a).unwrap();
        let val_b: Value = serde_json::from_str(&line_b).unwrap();
        assert_ne!(val_a["id"], val_b["id"]);

        let id_a = val_a["id"].as_u64().unwrap();
        let id_b = val_b["id"].as_u64().unwrap();

        let resp_b = mux.inbound(
            &format!(r#"{{"jsonrpc":"2.0","id":{id_b},"result":{{"from":"b"}}}}"#),
            Some("main"),
        );
        match resp_b {
            InboundDecision::Deliver(list) => {
                assert_eq!(list.len(), 1);
                assert_eq!(list[0].0, "window-2");
                let parsed: Value = serde_json::from_str(&list[0].1).unwrap();
                assert_eq!(parsed["id"], json!(1));
                assert_eq!(parsed["result"]["from"], json!("b"));
            }
            other => panic!("expected Deliver, got {other:?}"),
        }

        let resp_a = mux.inbound(
            &format!(r#"{{"jsonrpc":"2.0","id":{id_a},"result":{{"from":"a"}}}}"#),
            Some("main"),
        );
        match resp_a {
            InboundDecision::Deliver(list) => {
                assert_eq!(list.len(), 1);
                assert_eq!(list[0].0, "main");
                let parsed: Value = serde_json::from_str(&list[0].1).unwrap();
                assert_eq!(parsed["id"], json!(1));
                assert_eq!(parsed["result"]["from"], json!("a"));
            }
            other => panic!("expected Deliver, got {other:?}"),
        }
    }

    #[test]
    fn caches_initialize_result_and_queues_concurrent_init() {
        let mut mux = RpcMux::default();

        let first = mux.outbound(
            "main",
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        let engine_id = match first {
            OutboundDecision::Forward(line) => serde_json::from_str::<Value>(&line).unwrap()["id"]
                .as_u64()
                .unwrap(),
            other => panic!("expected Forward, got {other:?}"),
        };

        assert_eq!(
            mux.outbound(
                "window-2",
                r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}"#,
            ),
            OutboundDecision::QueueInit
        );

        let inbound = mux.inbound(
            &format!(r#"{{"jsonrpc":"2.0","id":{engine_id},"result":{{"version":"1"}}}}"#),
            Some("main"),
        );
        match inbound {
            InboundDecision::Deliver(list) => {
                assert_eq!(list.len(), 2);
                assert_eq!(list[0].0, "main");
                assert_eq!(list[1].0, "window-2");
                let second: Value = serde_json::from_str(&list[1].1).unwrap();
                assert_eq!(second["id"], json!(7));
                assert_eq!(second["result"]["version"], json!("1"));
            }
            other => panic!("expected Deliver, got {other:?}"),
        }

        let third = mux.outbound(
            "window-3",
            r#"{"jsonrpc":"2.0","id":3,"method":"initialize","params":{}}"#,
        );
        match third {
            OutboundDecision::ReplyLocal { window, line } => {
                assert_eq!(window, "window-3");
                let parsed: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(parsed["id"], json!(3));
                assert_eq!(parsed["result"]["version"], json!("1"));
            }
            other => panic!("expected ReplyLocal, got {other:?}"),
        }
    }

    #[test]
    fn scopes_session_load_replay_updates_to_loading_window() {
        let mut mux = RpcMux::default();

        let load = mux.outbound(
            "window-2",
            r#"{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"thread-1"}}"#,
        );
        let load_id = match load {
            OutboundDecision::Forward(line) => serde_json::from_str::<Value>(&line).unwrap()["id"]
                .as_u64()
                .unwrap(),
            other => panic!("expected Forward, got {other:?}"),
        };

        let replay = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"thread-1","update":{"text":"hi"}}}"#;
        assert_eq!(
            mux.inbound(replay, Some("main")),
            InboundDecision::Deliver(vec![("window-2".to_owned(), replay.to_owned())])
        );

        let other_thread = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"thread-2","update":{"text":"live"}}}"#;
        assert_eq!(
            mux.inbound(other_thread, Some("main")),
            InboundDecision::Broadcast(other_thread.to_owned())
        );

        let _ = mux.inbound(
            &format!(r#"{{"jsonrpc":"2.0","id":{load_id},"result":{{}}}}"#),
            Some("main"),
        );

        assert_eq!(
            mux.inbound(replay, Some("main")),
            InboundDecision::Broadcast(replay.to_owned())
        );
    }

    #[test]
    fn routes_orchestration_to_primary_and_deduplicates_permission_answers() {
        let mut mux = RpcMux::default();

        let orch = r#"{"jsonrpc":"2.0","id":99,"method":"_rustcode/orchestration","params":{}}"#;
        assert_eq!(
            mux.inbound(orch, Some("main")),
            InboundDecision::Deliver(vec![("main".to_owned(), orch.to_owned())])
        );

        let perm =
            r#"{"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{}}"#;
        assert_eq!(
            mux.inbound(perm, Some("main")),
            InboundDecision::Broadcast(perm.to_owned())
        );

        let answer = r#"{"jsonrpc":"2.0","id":100,"result":{"outcome":"selected"}}"#;
        assert_eq!(
            mux.outbound("window-2", answer),
            OutboundDecision::ForwardAndNotifyPermissionResolved {
                line: answer.to_owned(),
                rpc_id: json!(100),
            }
        );
        assert_eq!(mux.outbound("main", answer), OutboundDecision::Drop);
    }

    #[test]
    fn detach_window_drops_pending_entries_and_loading_state() {
        let mut mux = RpcMux::default();

        let load = mux.outbound(
            "window-2",
            r#"{"jsonrpc":"2.0","id":5,"method":"session/load","params":{"sessionId":"s1"}}"#,
        );
        let load_id = match load {
            OutboundDecision::Forward(line) => serde_json::from_str::<Value>(&line).unwrap()["id"]
                .as_u64()
                .unwrap(),
            other => panic!("expected Forward, got {other:?}"),
        };

        mux.detach_window("window-2");

        let update = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{}}}"#;
        assert_eq!(
            mux.inbound(update, Some("main")),
            InboundDecision::Broadcast(update.to_owned())
        );
        assert_eq!(
            mux.inbound(
                &format!(r#"{{"jsonrpc":"2.0","id":{load_id},"result":{{}}}}"#),
                Some("main"),
            ),
            InboundDecision::Deliver(Vec::new())
        );
    }
}
