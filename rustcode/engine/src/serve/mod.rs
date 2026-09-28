//! `rustcode serve` transport for remote frontends (issue #1443, experimental).
//!
//! A TCP server driving exactly one [`ControllerHandle`] session worker —
//! the same seam the desktop uses. Framing reuses the daemon protocol
//! (newline-delimited JSON, `MAX_FRAME_BYTES`); the message set beside it
//! is session-oriented and never overloads `DaemonRequest`.
//!
//! MVP operations: auth handshake, session list, prompt submit, event
//! stream (snapshots + turn updates), question answers, approval batches,
//! cancel. Everything else (multi-session routing, TLS, reconnect resume)
//! is explicitly future work.

pub mod protocol;
pub mod server;

pub use protocol::{SERVE_PROTOCOL_VERSION, ServeRequest, ServeResponse};
pub use server::{ServeOptions, resolve_bind, serve};

use std::net::SocketAddr;
use std::path::PathBuf;

/// Random per-launch token, printed once for the remote operator.
pub fn generate_token() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// Run `serve` until the listener errors or the process exits. Prints the
/// address and the per-launch token (once) for the remote operator.
pub async fn run(
    bind: &str,
    port: u16,
    token: Option<String>,
    allow_remote: bool,
    workspace: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let ip = resolve_bind(bind, allow_remote)?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let addr = listener.local_addr()?;
    let token = token.unwrap_or_else(generate_token);
    println!("rustcode serve (experimental) listening on {addr}");
    println!("token: {token}");
    println!("workspace: {}", workspace.display());
    serve(listener, ServeOptions { token, workspace })
        .await
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    Ok(())
}
