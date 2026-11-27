//! `_rustcode/browser/*`: drive the thread's shared engine Chrome from the
//! desktop Preview panel (screencast frames, navigation, viewport emulation,
//! input forwarding, and sanitized element picking — ADR 0012).

use agent_client_protocol::UntypedMessage;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

use super::clients::ClientId;
use crate::computer_use::panel::ScreencastFrame;

/// Notification method pushed whenever a new screencast JPEG frame arrives.
pub(crate) const FRAME_NOTIFICATION: &str = "_rustcode/browser/frame";

/// Open a URL in the thread's engine Chrome and start streaming frames.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/browser/open", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenRequest {
    pub session_id: String,
    pub url: String,
}

/// Read current navigation, viewport, inspect-mode, and latest frame state.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/browser/state", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StateRequest {
    pub session_id: String,
}

/// Navigate (`goto`, `reload`, `back`, `forward`) in the thread's browser.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/browser/navigate", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NavigateRequest {
    pub session_id: String,
    pub action: String,
    #[serde(default)]
    pub url: Option<String>,
}

/// Switch emulated responsive viewport dimensions (`desktop`, `tablet`, `mobile`).
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/browser/viewport", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ViewportRequest {
    pub session_id: String,
    pub preset: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// Forward a mouse, wheel, keyboard, or text input event to the thread's browser.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/browser/input", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InputRequest {
    pub session_id: String,
    pub event: Value,
}

/// Toggle CDP `Overlay.setInspectMode` element highlighting in the preview.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/browser/inspect", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectRequest {
    pub session_id: String,
    pub enabled: bool,
}

/// Inspect the DOM element at `(x, y)` and return a sanitized bundle + cropped PNG.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/browser/pick", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PickRequest {
    pub session_id: String,
    pub x: f64,
    pub y: f64,
}

/// Start or stop forwarding live screencast frames as `_rustcode/browser/frame` notifications.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/browser/stream", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StreamRequest {
    pub session_id: String,
    pub active: bool,
}

type StreamKey = (String, ClientId);

#[derive(Default)]
struct StreamRegistry {
    next: u64,
    entries: HashMap<StreamKey, (u64, CancellationToken)>,
}

impl StreamRegistry {
    fn insert(&mut self, key: StreamKey, token: CancellationToken) -> u64 {
        self.next += 1;
        if let Some((_, previous)) = self.entries.insert(key, (self.next, token)) {
            previous.cancel();
        }
        self.next
    }

    fn remove(&mut self, key: &StreamKey) -> usize {
        if let Some((_, token)) = self.entries.remove(key) {
            token.cancel();
        }
        self.watchers(&key.0)
    }

    fn finished(&mut self, key: &StreamKey, generation: u64) -> bool {
        if self
            .entries
            .get(key)
            .is_some_and(|(current, _)| *current == generation)
        {
            self.entries.remove(key);
            self.watchers(&key.0) == 0
        } else {
            false
        }
    }

    fn drop_client(&mut self, client_id: ClientId) -> Vec<String> {
        let mut sessions = Vec::new();
        self.entries.retain(|(session, owner), (_, token)| {
            let keep = *owner != client_id;
            if !keep {
                token.cancel();
                sessions.push(session.clone());
            }
            keep
        });
        sessions.retain(|session| self.watchers(session) == 0);
        sessions.sort();
        sessions
    }

    fn watchers(&self, session_id: &str) -> usize {
        self.entries
            .keys()
            .filter(|(session, _)| session == session_id)
            .count()
    }
}

fn active_streams() -> std::sync::MutexGuard<'static, StreamRegistry> {
    static STREAMS: OnceLock<Mutex<StreamRegistry>> = OnceLock::new();
    STREAMS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn stop_stream_task(session_id: &str, client_id: ClientId) -> usize {
    active_streams().remove(&(session_id.to_owned(), client_id))
}

async fn stop_screencast_if_unwatched(session_id: &str) {
    let watchers = active_streams().watchers(session_id);
    if watchers == 0 {
        let _ = crate::computer_use::run_panel_action(
            session_id,
            "panel_stream",
            json!({ "active": false }),
        )
        .await;
    }
}

/// Cancel every frame stream the closed connection `client_id` subscribed to
/// and stop the panel screencast for any session left without watchers.
pub(super) fn client_closed(client_id: ClientId) {
    let orphaned = active_streams().drop_client(client_id);
    if orphaned.is_empty() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    for session_id in orphaned {
        handle.spawn(async move {
            stop_screencast_if_unwatched(&session_id).await;
        });
    }
}

async fn start_stream_task(
    session_id: String,
    client_id: ClientId,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
) {
    let Some((mut rx, latest)) = crate::computer_use::subscribe_frames(&session_id).await else {
        stop_stream_task(&session_id, client_id);
        return;
    };
    let token = CancellationToken::new();
    let key = (session_id.clone(), client_id);
    let generation = active_streams().insert(key.clone(), token.clone());
    tokio::spawn(async move {
        let send_failed = forward_frames(&session_id, &token, &mut rx, latest, &connection).await;
        let orphaned = active_streams().finished(&key, generation);
        if orphaned && send_failed {
            stop_screencast_if_unwatched(&session_id).await;
        }
    });
}

async fn forward_frames(
    session_id: &str,
    token: &CancellationToken,
    rx: &mut tokio::sync::broadcast::Receiver<ScreencastFrame>,
    latest: Option<ScreencastFrame>,
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
) -> bool {
    if let Some(frame) = latest
        && let Ok(msg) = UntypedMessage::new(
            FRAME_NOTIFICATION,
            json!({ "sessionId": session_id, "frame": frame }),
        )
        && connection.send_notification(msg).is_err()
    {
        return true;
    }
    loop {
        tokio::select! {
            _ = token.cancelled() => return false,
            received = rx.recv() => {
                match received {
                    Ok(frame) => {
                        let Ok(msg) = UntypedMessage::new(
                            FRAME_NOTIFICATION,
                            json!({ "sessionId": session_id, "frame": frame }),
                        ) else {
                            continue;
                        };
                        if connection.send_notification(msg).is_err() {
                            return true;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                }
            }
        }
    }
}

fn format_response(outcome: Result<Value, String>) -> Value {
    match outcome {
        Ok(value) => value,
        Err(message) => json!({ "error": message }),
    }
}

/// Dispatch browser preview panel requests, forwarding everything else to `memory::dispatch`.
pub(super) fn dispatch(
    client_id: ClientId,
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    match method.as_str() {
        "_rustcode/browser/open" => {
            let request: OpenRequest = super::parse_params(params)?;
            let stream_conn = connection.clone();
            connection.spawn(async move {
                let outcome = crate::computer_use::run_panel_action(
                    &request.session_id,
                    "panel_open",
                    json!({ "url": request.url }),
                )
                .await;
                if outcome.is_ok() {
                    start_stream_task(request.session_id, client_id, stream_conn).await;
                }
                responder.respond(format_response(outcome))?;
                Ok(())
            })
        }
        "_rustcode/browser/state" => {
            let request: StateRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let outcome = crate::computer_use::run_panel_action(
                    &request.session_id,
                    "panel_state",
                    json!({}),
                )
                .await;
                responder.respond(format_response(outcome))?;
                Ok(())
            })
        }
        "_rustcode/browser/navigate" => {
            let request: NavigateRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let outcome = crate::computer_use::run_panel_action(
                    &request.session_id,
                    "panel_navigate",
                    json!({ "action": request.action, "url": request.url }),
                )
                .await;
                responder.respond(format_response(outcome))?;
                Ok(())
            })
        }
        "_rustcode/browser/viewport" => {
            let request: ViewportRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let outcome = crate::computer_use::run_panel_action(
                    &request.session_id,
                    "panel_viewport",
                    json!({
                        "preset": request.preset,
                        "width": request.width,
                        "height": request.height,
                    }),
                )
                .await;
                responder.respond(format_response(outcome))?;
                Ok(())
            })
        }
        "_rustcode/browser/input" => {
            let request: InputRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let outcome = crate::computer_use::run_panel_action(
                    &request.session_id,
                    "panel_input",
                    json!({ "event": request.event }),
                )
                .await;
                responder.respond(format_response(outcome))?;
                Ok(())
            })
        }
        "_rustcode/browser/inspect" => {
            let request: InspectRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let outcome = crate::computer_use::run_panel_action(
                    &request.session_id,
                    "panel_inspect",
                    json!({ "enabled": request.enabled }),
                )
                .await;
                responder.respond(format_response(outcome))?;
                Ok(())
            })
        }
        "_rustcode/browser/pick" => {
            let request: PickRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let outcome = crate::computer_use::run_panel_action(
                    &request.session_id,
                    "panel_pick",
                    json!({ "x": request.x, "y": request.y }),
                )
                .await;
                responder.respond(format_response(outcome))?;
                Ok(())
            })
        }
        "_rustcode/browser/stream" => {
            let request: StreamRequest = super::parse_params(params)?;
            let stream_conn = connection.clone();
            connection.spawn(async move {
                let others = if request.active {
                    0
                } else {
                    stop_stream_task(&request.session_id, client_id)
                };
                let (action, args) = if others > 0 {
                    ("panel_state", json!({}))
                } else {
                    ("panel_stream", json!({ "active": request.active }))
                };
                let outcome =
                    crate::computer_use::run_panel_action(&request.session_id, action, args).await;
                if request.active && outcome.is_ok() {
                    start_stream_task(request.session_id, client_id, stream_conn).await;
                }
                responder.respond(format_response(outcome))?;
                Ok(())
            })
        }
        _ => super::memory::dispatch(method, params, responder, connection, unhandled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_payloads_deserialize_camel_case_fields() {
        let open: OpenRequest =
            serde_json::from_str(r#"{"sessionId":"t1","url":"http://localhost:5173"}"#).unwrap();
        assert_eq!(open.session_id, "t1");
        assert_eq!(open.url, "http://localhost:5173");

        let pick: PickRequest =
            serde_json::from_str(r#"{"sessionId":"t1","x":120.5,"y":88.0}"#).unwrap();
        assert_eq!(pick.session_id, "t1");
        assert_eq!(pick.x, 120.5);
        assert_eq!(pick.y, 88.0);
    }

    #[test]
    fn two_clients_stream_one_session_without_cancelling_each_other() {
        let mut streams = StreamRegistry::default();
        let first = CancellationToken::new();
        let second = CancellationToken::new();
        streams.insert(("t1".into(), 1), first.clone());
        streams.insert(("t1".into(), 2), second.clone());
        assert!(!first.is_cancelled());
        assert_eq!(streams.watchers("t1"), 2);

        assert_eq!(streams.remove(&("t1".into(), 1)), 1);
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
        assert_eq!(streams.remove(&("t1".into(), 2)), 0);
        assert!(second.is_cancelled());
    }

    #[test]
    fn restarting_a_client_stream_replaces_only_its_own_task() {
        let mut streams = StreamRegistry::default();
        let old = CancellationToken::new();
        let other = CancellationToken::new();
        let stale = streams.insert(("t1".into(), 1), old.clone());
        streams.insert(("t1".into(), 2), other.clone());
        let fresh = CancellationToken::new();
        let current = streams.insert(("t1".into(), 1), fresh.clone());
        assert!(old.is_cancelled());
        assert!(!other.is_cancelled());

        assert!(!streams.finished(&("t1".into(), 1), stale));
        assert_eq!(streams.watchers("t1"), 2);
        assert!(!streams.finished(&("t1".into(), 1), current));
        assert_eq!(streams.watchers("t1"), 1);
        assert!(!fresh.is_cancelled());
    }

    #[test]
    fn a_closed_client_drops_only_its_own_streams() {
        let mut streams = StreamRegistry::default();
        let gone_a = CancellationToken::new();
        let gone_b = CancellationToken::new();
        let kept = CancellationToken::new();
        streams.insert(("t1".into(), 7), gone_a.clone());
        streams.insert(("t2".into(), 7), gone_b.clone());
        streams.insert(("t1".into(), 8), kept.clone());
        let orphaned = streams.drop_client(7);
        assert_eq!(orphaned, ["t2"]);
        assert!(gone_a.is_cancelled() && gone_b.is_cancelled());
        assert!(!kept.is_cancelled());
        assert_eq!(streams.watchers("t1"), 1);
        assert_eq!(streams.watchers("t2"), 0);
    }

    #[test]
    fn drop_client_reports_sessions_left_without_watchers() {
        let mut streams = StreamRegistry::default();
        streams.insert(("t1".into(), 7), CancellationToken::new());
        streams.insert(("t2".into(), 7), CancellationToken::new());
        streams.insert(("t3".into(), 7), CancellationToken::new());
        streams.insert(("t2".into(), 8), CancellationToken::new());
        streams.insert(("t4".into(), 8), CancellationToken::new());

        assert_eq!(streams.drop_client(7), ["t1", "t3"]);
        assert!(streams.drop_client(7).is_empty());
        assert_eq!(streams.drop_client(8), ["t2", "t4"]);
    }

    #[test]
    fn finished_reports_when_the_last_watcher_exits() {
        let mut streams = StreamRegistry::default();
        let first = streams.insert(("t1".into(), 1), CancellationToken::new());
        let second = streams.insert(("t1".into(), 2), CancellationToken::new());

        assert!(!streams.finished(&("t1".into(), 1), first));
        assert!(streams.finished(&("t1".into(), 2), second));
        assert!(!streams.finished(&("t1".into(), 2), second));
    }
}
