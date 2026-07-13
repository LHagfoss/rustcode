use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app::{AppState, ChatMessage};

struct ServerState {
    app: Arc<Mutex<AppState>>,
    token: String,
}

/// Generate a short random session token the user types on their phone.
pub fn gen_token() -> String {
    use rand::RngExt;
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..8)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}

fn token_ok(st: &ServerState, provided: Option<&str>) -> bool {
    provided == Some(st.token.as_str())
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "invalid token").into_response()
}

/// Serve the embedded single-page frontend.
async fn serve_index() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}

/// GET /api/history?token=... -> { history: [{ role, content }] }
async fn get_history(
    State(st): State<Arc<ServerState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !token_ok(&st, q.get("token").map(|s| s.as_str())) {
        return unauthorized();
    }
    let s = st.app.lock().await;
    let history: Vec<Value> = s
        .history
        .iter()
        .filter(|m| matches!(m.role.as_str(), "user" | "assistant" | "tool" | "system"))
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    axum::Json(json!({ "history": history })).into_response()
}

/// GET /api/status?token=... -> { confirmation: null | { tool, preview, bytes } }
async fn get_status(
    State(st): State<Arc<ServerState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !token_ok(&st, q.get("token").map(|s| s.as_str())) {
        return unauthorized();
    }
    let s = st.app.lock().await;
    let confirmation = s.pending_tool_confirmation.as_ref().map(|c| {
        json!({
            "tool": c.tool_name,
            "preview": c.content_preview,
            "bytes": c.content_bytes,
        })
    });
    axum::Json(json!({ "confirmation": confirmation })).into_response()
}

/// POST /api/confirm { token, approved } -> resumes the waiting agent loop.
async fn confirm_tool(
    State(st): State<Arc<ServerState>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let provided = body.get("token").and_then(|v| v.as_str());
    if !token_ok(&st, provided) {
        return unauthorized();
    }
    let approved = body
        .get("approved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut s = st.app.lock().await;
    // Hand the decision to the oneshot the agent loop is awaiting. The receiving
    // side (confirm_and_execute) does the state cleanup and resumes streaming.
    if let Some(tx) = s.tool_confirmation_response.take() {
        let _ = tx.send(approved);
    }
    axum::Json(json!({ "ok": true })).into_response()
}

/// POST /api/send { token, message } -> queued for agent.
async fn send_message(
    State(st): State<Arc<ServerState>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if !token_ok(&st, body.get("token").and_then(|v| v.as_str())) {
        return unauthorized();
    }
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if message.is_empty() {
        return axum::Json(json!({ "ok": false })).into_response();
    }
    st.app.lock().await.pending_queue.push(message);
    axum::Json(json!({ "ok": true })).into_response()
}

/// POST /api/cancel { token } -> cancels the active agent stream.
async fn cancel_agent(
    State(st): State<Arc<ServerState>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if !token_ok(&st, body.get("token").and_then(|v| v.as_str())) {
        return unauthorized();
    }
    let mut s = st.app.lock().await;
    if let Some(token) = &s.cancel_token {
        token.cancel();
    }
    s.status = crate::app::AppStatus::Idle;
    s.pending_queue.clear();
    axum::Json(json!({ "ok": true })).into_response()
}

/// Run the remote-control server until `cancel` fires. Reports bind failures back
/// into the chat as a system message.
pub async fn run_server(
    app: Arc<Mutex<AppState>>,
    host: String,
    port: u16,
    token: String,
    cancel: CancellationToken,
) {
    let addr = format!("{host}:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            let mut s = app.lock().await;
            s.history.push(ChatMessage::new(
                "system",
                format!("remote server failed to bind {addr}: {e}"),
            ));
            s.remote_server = None;
            return;
        }
    };

    let state = Arc::new(ServerState { app, token });
    let router = Router::new()
        .route("/", get(serve_index))
        .route("/api/history", get(get_history))
        .route("/api/status", get(get_status))
        .route("/api/confirm", post(confirm_tool))
        .route("/api/send", post(send_message))
        .route("/api/cancel", post(cancel_agent))
        .with_state(state);

    let _ = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            cancel.cancelled().await;
        })
        .await;
}
