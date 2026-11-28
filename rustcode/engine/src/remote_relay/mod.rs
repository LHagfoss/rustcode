//! A blind, self-hostable relay for machines with no shared network
//! ([ADR 0013](../../../../docs/adr/0013-machine-relay-and-nat-traversal.md)).
//!
//! A machine keeps one outbound control connection to the relay, over TLS
//! pinned to the relay's own certificate and admitted by the relay's secret.
//! A paired device dials the relay for that machine, proves with its device
//! key that the machine allows it, and is spliced to a fresh pipe the machine
//! opens. The gateway's pinned TLS and the device-key challenge then run end
//! to end through the pipe, so the relay never holds a key that could read
//! the traffic. It sees which device reaches which machine, when, and how
//! many bytes.

pub mod member;
pub mod pin;
pub mod protocol;
pub mod server;
pub mod state;

#[cfg(test)]
mod tests;

use crate::daemon::private_files::ensure_private_directory;
use crate::remote_gateway::tls::TlsIdentity;
use anyhow::{Context, Result};
use protocol::JoinLink;
use server::{Limits, Relay};
use state::RelayState;
use std::path::Path;
use tokio::net::TcpListener;

/// The port `rc remote relay` listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 17880;

/// Run a relay in the foreground on `bind:port` until interrupted, keeping
/// its state in `relay/` under `config_dir`.
pub async fn serve(
    config_dir: &Path,
    bind: &str,
    port: u16,
    advertise: Option<&str>,
) -> Result<()> {
    let directory = config_dir.join("relay");
    ensure_private_directory(&directory, "relay")?;
    let state_path = RelayState::path(config_dir);
    let state = RelayState::load_or_create(&state_path)?;
    let tls = TlsIdentity::load_or_create(&directory.join("tls.json"))?;
    let listener = TcpListener::bind((bind, port))
        .await
        .with_context(|| format!("could not listen on {bind}:{port}"))?;
    let relay = Relay::new(state, Some(state_path), tls, Limits::default());
    let unspecified = matches!(bind, "0.0.0.0" | "::" | "[::]");
    let host = advertise
        .map(str::to_owned)
        .unwrap_or_else(|| bind.trim_matches(['[', ']']).to_owned());
    let link = JoinLink {
        host,
        port,
        fingerprint: relay.fingerprint().to_owned(),
        secret: relay.secret(),
    };
    println!("RustCode relay listening on {bind}:{port}");
    println!("  relay id     {}", relay.relay_id());
    println!("  fingerprint  {}", relay.fingerprint());
    println!();
    println!(
        "Register a machine with this link. It holds the secret that admits machines; keep it private:"
    );
    println!("  {link}");
    if unspecified && advertise.is_none() {
        println!();
        println!(
            "The link names {bind}, which no machine can dial: pass --advertise with this relay's public name or IP."
        );
    }
    println!();
    println!(
        "The relay sees which device reaches which machine, when, and how many bytes. Prompts, transcripts, files and tokens travel inside the machine's own pinned TLS and stay unreadable here."
    );
    let stop = relay.shutdown_token();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            stop.cancel();
        }
    });
    relay.run(listener).await;
    Ok(())
}
