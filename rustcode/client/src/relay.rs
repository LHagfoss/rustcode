//! Relay routes: reach a machine through a relay it is registered with
//! ([ADR 0013](../../../docs/adr/0013-machine-relay-and-nat-traversal.md)).
//!
//! A [`Bridge`] listens on this computer's loopback, on a port the system
//! picks. For each socket it accepts it dials the relay, asks for a pipe to
//! the machine, proves with this install's device key that the machine
//! allows it, and then copies bytes both ways. The WebSocket code dials the
//! bridge as it dials a LAN address, with the machine's TLS pin, so the
//! gateway's own TLS and sign-in run end to end and the relay reads none of
//! it. A bridge for a machine without a pin is never opened.
//!
//! The frames and the signed bytes must match the relay byte for byte
//! (`rustcode/engine/src/remote_relay/protocol.rs`): one JSON object per
//! line, and `rustcode-relay-v1`, `client`, the relay id, the challenge and
//! the machine id, each followed by a zero byte.

use crate::device_key::DeviceKey;
use crate::registry::{Machine, Route, RouteKind};
use crate::tokens::TokenStore;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// The scheme of a relay route, `rustcode-relay://host:port`.
pub const SCHEME: &str = "rustcode-relay";

/// The relay protocol version this client speaks.
const PROTOCOL_VERSION: u32 = 1;
const SIGNATURE_DOMAIN: &str = "rustcode-relay-v1";
const MAX_FRAME_BYTES: u64 = 64 * 1024;
/// From the relay's accept to a spliced pipe: the relay waits up to 10 s
/// for the machine to open its side.
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);

/// The `host:port` of a relay route, or `None` for any other address.
pub fn route_target(address: &str) -> Option<&str> {
    let target = address.trim().strip_prefix(SCHEME)?.strip_prefix("://")?;
    (!target.is_empty() && !target.contains(['?', '/'])).then_some(target)
}

/// The bytes signed to dial `machine_id` through the relay `relay_id`.
pub fn signed_message(relay_id: &str, challenge: &str, machine_id: &str) -> Vec<u8> {
    let mut message = Vec::new();
    for part in [SIGNATURE_DOMAIN, "client", relay_id, challenge, machine_id] {
        message.extend_from_slice(part.as_bytes());
        message.push(0);
    }
    message
}

/// A loopback address that reaches the machine through its relay, for as
/// long as this value lives.
pub struct Bridge {
    address: String,
    relay: String,
    failure: Arc<Mutex<Option<String>>>,
    /// Dropped with the bridge, which ends the accept loop.
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Bridge {
    /// Start listening for the relay route `route` to `machine_id`. Nothing
    /// is dialled until a socket connects.
    pub async fn open(route: &str, machine_id: &str, key: Arc<DeviceKey>) -> Result<Self> {
        let relay = route_target(route)
            .with_context(|| format!("{route} is not a relay route"))?
            .to_owned();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?.to_string();
        let failure = Arc::new(Mutex::new(None));
        let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
        let accepted_failure = Arc::clone(&failure);
        let target = relay.clone();
        let machine_id = machine_id.to_owned();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        tokio::spawn(carry(
                            socket,
                            target.clone(),
                            machine_id.clone(),
                            Arc::clone(&key),
                            Arc::clone(&accepted_failure),
                        ));
                    }
                }
            }
        });
        Ok(Self {
            address,
            relay,
            failure,
            _stop: stop,
        })
    }

    /// The `127.0.0.1:<port>` to dial.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The failure of a dial over this bridge, with the relay's own reason
    /// when it gave one.
    pub async fn explain(&self, error: impl std::fmt::Display) -> String {
        for _ in 0..10 {
            if let Some(failure) = self.failure.lock().unwrap().clone() {
                return failure;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        format!("{error} (through the relay at {})", self.relay)
    }
}

/// The loopback bridge a route that is not a socket address is dialled
/// through: `ssh` for an SSH route, the relay for a relay route.
pub enum RouteBridge {
    Ssh(crate::tunnel::Bridge),
    Relay(Bridge),
}

impl RouteBridge {
    /// The bridge `route` to `machine` needs, or `None` when it is dialled
    /// directly. A relay route needs the machine's TLS pin, so the relay
    /// can never read what crosses it, and this install's device key.
    pub async fn open(
        route: &Route,
        machine: &Machine,
        tokens: &TokenStore,
    ) -> Result<Option<Self>> {
        if let Some(ssh) = &route.ssh {
            return Ok(Some(Self::Ssh(crate::tunnel::Bridge::open(ssh).await?)));
        }
        if route.kind != RouteKind::Relay {
            return Ok(None);
        }
        ensure!(
            machine.tls_fingerprint.is_some(),
            "{} has not given this computer its certificate yet, which a relay route needs; \
             connect to it once on another route",
            machine.display()
        );
        let key = DeviceKey::load(tokens)?.with_context(|| {
            format!(
                "this computer has no key {} knows yet, which a relay route needs; connect to \
                 it once on another route",
                machine.display()
            )
        })?;
        let bridge = Bridge::open(&route.address, &machine.host_id, Arc::new(key)).await?;
        Ok(Some(Self::Relay(bridge)))
    }

    /// The `127.0.0.1:<port>` to dial.
    pub fn address(&self) -> &str {
        match self {
            Self::Ssh(bridge) => bridge.address(),
            Self::Relay(bridge) => bridge.address(),
        }
    }

    /// The failure of a dial over this bridge, with SSH's or the relay's
    /// own reason when it gave one.
    pub async fn explain(&self, error: impl std::fmt::Display) -> String {
        match self {
            Self::Ssh(bridge) => bridge.explain(error).await,
            Self::Relay(bridge) => bridge.explain(error).await,
        }
    }
}

/// One accepted socket: dial the relay and copy both ways until either side
/// ends.
async fn carry(
    mut socket: TcpStream,
    relay: String,
    machine_id: String,
    key: Arc<DeviceKey>,
    failure: Arc<Mutex<Option<String>>>,
) {
    match tokio::time::timeout(DIAL_TIMEOUT, dial(&relay, &machine_id, &key)).await {
        Ok(Ok(mut pipe)) => {
            let _ = socket.set_nodelay(true);
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut pipe).await;
        }
        Ok(Err(error)) => {
            *failure.lock().unwrap() = Some(format!("{error:#} (relay at {relay})"));
        }
        Err(_) => {
            *failure.lock().unwrap() =
                Some(format!("the relay at {relay} did not open a pipe in time"));
        }
    }
}

/// Ask `relay` for a pipe to `machine_id` and return it once spliced.
async fn dial(relay: &str, machine_id: &str, key: &DeviceKey) -> Result<BufReader<TcpStream>> {
    let socket = TcpStream::connect(relay)
        .await
        .with_context(|| format!("could not reach the relay at {relay}"))?;
    let _ = socket.set_nodelay(true);
    let mut stream = BufReader::new(socket);
    let request = json!({
        "type": "dial",
        "version": PROTOCOL_VERSION,
        "machine_id": machine_id,
        "public_key": key.public_key(),
    });
    write_frame(&mut stream, &request).await?;
    let reply = read_frame(&mut stream).await?;
    let (Some(challenge), Some(relay_id)) = (
        field(&reply, "challenge", "challenge"),
        field(&reply, "challenge", "relay_id"),
    ) else {
        bail!("{}", refusal(&reply));
    };
    let signature = key.sign_message(&signed_message(&relay_id, &challenge, machine_id));
    write_frame(
        &mut stream,
        &json!({"type": "prove", "signature": signature}),
    )
    .await?;
    let reply = read_frame(&mut stream).await?;
    if reply.get("type").and_then(Value::as_str) != Some("connected") {
        bail!("{}", refusal(&reply));
    }
    Ok(stream)
}

fn field(frame: &Value, kind: &str, name: &str) -> Option<String> {
    (frame.get("type")?.as_str()? == kind)
        .then(|| frame.get(name)?.as_str().map(str::to_owned))
        .flatten()
}

fn refusal(frame: &Value) -> String {
    match frame.get("message").and_then(Value::as_str) {
        Some(message) => format!("the relay refused: {message}"),
        None => "the relay answered out of turn".to_owned(),
    }
}

async fn write_frame(stream: &mut BufReader<TcpStream>, frame: &Value) -> Result<()> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    stream.get_mut().write_all(&line).await?;
    stream.get_mut().flush().await?;
    Ok(())
}

/// The next line, reading no byte past it: what follows is the machine's.
async fn read_frame(stream: &mut BufReader<TcpStream>) -> Result<Value> {
    let mut line = Vec::new();
    let read = (&mut *stream)
        .take(MAX_FRAME_BYTES)
        .read_until(b'\n', &mut line)
        .await
        .context("could not read from the relay")?;
    if read == 0 || line.last() != Some(&b'\n') {
        bail!("the relay closed the connection");
    }
    serde_json::from_slice(&line).context("the relay sent something that is not a relay frame")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_relay_route_names_a_relay() {
        assert_eq!(
            route_target("rustcode-relay://relay.example.com:17880"),
            Some("relay.example.com:17880")
        );
        assert_eq!(
            route_target("rustcode-relay://[2001:db8::1]:17880"),
            Some("[2001:db8::1]:17880")
        );
        assert_eq!(route_target("rustcode-relay://h:1?secret=x"), None);
        assert_eq!(route_target("192.168.1.20:17879"), None);
        assert_eq!(route_target("rustcode-relay://"), None);
    }

    #[test]
    fn the_signed_parts_cannot_shift_into_each_other() {
        assert_ne!(
            signed_message("ab", "c", "d"),
            signed_message("a", "bc", "d")
        );
        assert!(signed_message("r", "c", "m").starts_with(b"rustcode-relay-v1\0client\0"));
    }
}
