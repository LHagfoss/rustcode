//! A machine's side of the relay: the registration it keeps and the control
//! connection that answers each dial with a fresh pipe.
//!
//! `rc machines relay join <link>` saves the link and a key of the
//! machine's own in `relay-link.json` (private: the link carries the secret
//! that admits members). The gateway reads it, registers over TLS pinned to
//! the relay's certificate, tells the relay which device keys may dial it,
//! and hands every pipe the relay asks for to its own connection handling,
//! where the device's pinned TLS and sign-in run as on any other route. It
//! reconnects with a backoff while the relay is down and lets go when the
//! file goes away.

use super::pin::{pinned_connector, server_name};
use super::protocol::{
    JoinLink, PROTOCOL_VERSION, Reply, Request, Role, read_frame, signed_message, write_frame,
};
use crate::daemon::private_files::{publish_private_json, read_private_json, remove_if_exists};
use anyhow::{Context, Result, bail};
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// How often a machine with no relay looks for a registration, and how often
/// a registered one rereads it and its allowed keys.
const CHECK: Duration = Duration::from_secs(30);
const REFRESH: Duration = Duration::from_secs(5);
/// The relay pings every 25 s; this long without a word is a dead link.
const SILENCE: Duration = Duration::from_secs(80);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FIRST_RETRY: Duration = Duration::from_secs(5);
const LAST_RETRY: Duration = Duration::from_secs(300);
const MAX_LINK_BYTES: u64 = 16 * 1024;

/// `relay-link.json`: the relay this machine registers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLink {
    /// The join link `rc remote relay` printed, secret included.
    pub link: String,
    /// This machine's relay key, unpadded base64url of its PKCS#8 document.
    /// The relay binds the machine id to it on first registration, so it is
    /// kept when the machine joins another relay.
    key: String,
}

impl RelayLink {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("relay-link.json")
    }

    /// The saved registration, or `None` when this machine has none.
    pub fn load(config_dir: &Path) -> Result<Option<Self>> {
        match read_private_json::<Self>(&Self::path(config_dir), MAX_LINK_BYTES) {
            Ok(link) => Ok(Some(link)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Save `link` as the relay to register with, keeping the key of an
    /// earlier registration.
    pub fn join(config_dir: &Path, link: &JoinLink) -> Result<Self> {
        let key = match Self::load(config_dir)? {
            Some(saved) => saved.key,
            None => {
                let document =
                    Ed25519KeyPair::generate_pkcs8(&aws_lc_rs::rand::SystemRandom::new()).map_err(
                        |_| anyhow::anyhow!("could not generate this machine's relay key"),
                    )?;
                URL_SAFE_NO_PAD.encode(document.as_ref())
            }
        };
        let saved = Self {
            link: link.to_string(),
            key,
        };
        publish_private_json(&Self::path(config_dir), &saved)?;
        Ok(saved)
    }

    /// Forget the registration. False when there was none.
    pub fn leave(config_dir: &Path) -> Result<bool> {
        let path = Self::path(config_dir);
        let existed = path.exists();
        remove_if_exists(&path)?;
        Ok(existed)
    }

    pub fn join_link(&self) -> Result<JoinLink> {
        JoinLink::parse(&self.link)
    }

    fn key_pair(&self) -> Result<Ed25519KeyPair> {
        let document = URL_SAFE_NO_PAD
            .decode(&self.key)
            .context("this machine's relay key is not readable")?;
        Ed25519KeyPair::from_pkcs8(&document)
            .map_err(|_| anyhow::anyhow!("this machine's relay key is not a valid key"))
    }
}

/// What the member needs from the gateway it runs in.
pub trait Host: Send + Sync + 'static {
    /// The public keys of the devices that may dial this machine.
    fn allowed_keys(&self) -> Vec<String>;
    /// Serve `pipe` as a connection that arrived through the relay.
    fn deliver(&self, pipe: TcpStream);
    /// The route devices dial while registered, or `None` once not.
    fn set_route(&self, route: Option<String>);
}

/// Keep `machine_id` registered with the relay in `config_dir`'s
/// `relay-link.json` until `stop` is cancelled.
pub async fn run<H: Host>(
    config_dir: PathBuf,
    machine_id: String,
    host: Arc<H>,
    stop: CancellationToken,
) {
    let mut retry = FIRST_RETRY;
    loop {
        let wait = match RelayLink::load(&config_dir) {
            Ok(None) => CHECK,
            Err(error) => {
                crate::dbg_log!("relay link: {error:#}");
                CHECK
            }
            Ok(Some(saved)) => {
                let outcome = link_once(&config_dir, &saved, &machine_id, &host, &stop).await;
                host.set_route(None);
                match outcome {
                    Ok(()) => {
                        retry = FIRST_RETRY;
                        Duration::ZERO
                    }
                    Err(error) => {
                        crate::dbg_log!(
                            "relay link: {error:#}; trying again in {}s",
                            retry.as_secs()
                        );
                        let wait = retry;
                        retry = (retry * 2).min(LAST_RETRY);
                        wait
                    }
                }
            }
        };
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// Register once and serve until the link drops, the saved registration
/// changes or `stop` is cancelled. `Ok` means the caller should reread the
/// registration at once.
async fn link_once<H: Host>(
    config_dir: &Path,
    saved: &RelayLink,
    machine_id: &str,
    host: &Arc<H>,
    stop: &CancellationToken,
) -> Result<()> {
    let link = saved.join_link()?;
    let key = saved.key_pair()?;
    let address = link.address();
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address))
        .await
        .with_context(|| format!("the relay at {address} did not answer in time"))?
        .with_context(|| format!("could not reach the relay at {address}"))?;
    let _ = tcp.set_nodelay(true);
    let tls = tokio::time::timeout(
        CONNECT_TIMEOUT,
        pinned_connector(&link.fingerprint)?.connect(server_name(), tcp),
    )
    .await
    .context("the relay did not finish the TLS handshake in time")?
    .context("the relay did not answer with the certificate its link names")?;
    let (read, mut writer) = tokio::io::split(tls);
    let mut reader = BufReader::new(read);
    let register = Request::Register {
        version: PROTOCOL_VERSION,
        secret: link.secret.clone(),
        machine_id: machine_id.to_owned(),
        public_key: URL_SAFE_NO_PAD.encode(key.public_key().as_ref()),
    };
    write_frame(&mut writer, &register).await?;
    let (challenge, relay_id) = match next(&mut reader).await? {
        Reply::Challenge {
            challenge,
            relay_id,
        } => (challenge, relay_id),
        other => bail!("{}", refusal(other)),
    };
    let message = signed_message(Role::Member, &relay_id, &challenge, machine_id);
    let prove = Request::Prove {
        signature: URL_SAFE_NO_PAD.encode(key.sign(&message).as_ref()),
    };
    write_frame(&mut writer, &prove).await?;
    match next(&mut reader).await? {
        Reply::Registered { .. } => {}
        other => bail!("{}", refusal(other)),
    }
    let mut allowed = host.allowed_keys();
    write_frame(
        &mut writer,
        &Request::AllowedKeys {
            keys: allowed.clone(),
        },
    )
    .await?;
    host.set_route(Some(link.route_address()));
    crate::dbg_log!("relay link: registered with {address}");

    let (frames, mut received) = mpsc::channel::<Reply>(16);
    let reading = tokio::spawn(async move {
        while let Ok(Some(frame)) = read_frame::<Reply, _>(&mut reader).await {
            if frames.send(frame).await.is_err() {
                break;
            }
        }
    });
    let mut refresh = tokio::time::interval(REFRESH);
    refresh.tick().await;
    let mut heard = tokio::time::Instant::now();
    let outcome = loop {
        tokio::select! {
            _ = stop.cancelled() => break Ok(()),
            frame = received.recv() => {
                heard = tokio::time::Instant::now();
                match frame {
                    None => break Err(anyhow::anyhow!("the relay closed the link")),
                    Some(Reply::Ping) => {
                        if let Err(error) = write_frame(&mut writer, &Request::Pong).await {
                            break Err(error);
                        }
                    }
                    Some(Reply::Open { pipe }) => {
                        tokio::spawn(open_pipe(address.clone(), pipe, Arc::clone(host)));
                    }
                    Some(Reply::Error { message, .. }) => break Err(anyhow::anyhow!("the relay ended the link: {message}")),
                    Some(_) => {}
                }
            }
            _ = refresh.tick() => {
                if heard.elapsed() > SILENCE {
                    break Err(anyhow::anyhow!("the relay went quiet"));
                }
                if RelayLink::load(config_dir).ok().flatten().as_ref() != Some(saved) {
                    break Ok(());
                }
                let now = host.allowed_keys();
                if now != allowed {
                    allowed = now;
                    let keys = Request::AllowedKeys { keys: allowed.clone() };
                    if let Err(error) = write_frame(&mut writer, &keys).await {
                        break Err(error);
                    }
                }
            }
        }
    };
    reading.abort();
    outcome
}

/// Open the pipe named `pipe` and hand it over once the relay has it.
async fn open_pipe<H: Host>(address: String, pipe: String, host: Arc<H>) {
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address)).await;
    let Ok(Ok(mut stream)) = connected else {
        crate::dbg_log!("relay link: could not open a pipe to {address}");
        return;
    };
    let _ = stream.set_nodelay(true);
    if write_frame(&mut stream, &Request::Pipe { pipe })
        .await
        .is_ok()
    {
        host.deliver(stream);
    }
}

async fn next<R>(reader: &mut R) -> Result<Reply>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    tokio::time::timeout(CONNECT_TIMEOUT, read_frame::<Reply, _>(reader))
        .await
        .context("the relay did not answer in time")??
        .context("the relay closed the connection")
}

fn refusal(reply: Reply) -> String {
    match reply {
        Reply::Error { message, .. } => format!("the relay refused this machine: {message}"),
        other => format!("the relay answered {other:?} out of turn"),
    }
}
