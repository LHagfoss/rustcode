//! `_rustcode/browser/*`: drive the thread's shared engine Chrome from the
//! desktop Preview panel (screencast frames, navigation, viewport emulation,
//! input forwarding, and sanitized element picking — ADR 0012).

use agent_client_protocol::UntypedMessage;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

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

type StreamMap = Mutex<HashMap<String, CancellationToken>>;

fn active_streams() -> &'static StreamMap {
    static STREAMS: OnceLock<StreamMap> = OnceLock::new();
    STREAMS.get_or_init(Default::default)
}

fn stop_stream_task(session_id: &str) {
    if let Some(token) = active_streams()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id)
    {
        token.cancel();
    }
}

async fn start_stream_task(
    session_id: String,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
) {
    stop_stream_task(&session_id);
    let Some((mut rx, latest)) = crate::computer_use::subscribe_frames(&session_id).await else {
        return;
    };
    let token = CancellationToken::new();
    active_streams()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.clone(), token.clone());
    tokio::spawn(async move {
        if let Some(frame) = latest
            && let Ok(msg) = UntypedMessage::new(
                FRAME_NOTIFICATION,
                json!({ "sessionId": session_id, "frame": frame }),
            )
            && connection.send_notification(msg).is_err()
        {
            return;
        }
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
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
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
}

fn format_response(outcome: Result<Value, String>) -> Value {
    match outcome {
        Ok(value) => value,
        Err(message) => json!({ "error": message }),
    }
}

/// Dispatch browser preview panel requests, forwarding everything else to `memory::dispatch`.
pub(super) fn dispatch(
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
                    start_stream_task(request.session_id, stream_conn).await;
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
                if !request.active {
                    stop_stream_task(&request.session_id);
                }
                let outcome = crate::computer_use::run_panel_action(
                    &request.session_id,
                    "panel_stream",
                    json!({ "active": request.active }),
                )
                .await;
                if request.active && outcome.is_ok() {
                    start_stream_task(request.session_id, stream_conn).await;
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
}
