//! `_rustcode/server/status`: what `rustcode server status` and
//! `restart --when-idle` ask a running server. The caller need not have sent
//! `initialize`, and then it is not counted among the clients.

use super::AcpServer;

/// Answers [`status`].
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/server/status", response = serde_json::Value)]
pub(crate) struct ServerStatusRequest {}

/// The running server: its version and pid, how many clients are connected
/// (those that initialized) and how many threads have a turn running.
pub(crate) fn status(server: &AcpServer) -> serde_json::Value {
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "clients": server.clients.count(),
        "running_turns": server.clients.running_turns(),
    })
}
