//! The machine client: what a `rustcode` on this computer does with another
//! machine's gateway.
//!
//! Three steps, in the order the protocol allows them:
//!
//! 1. [`Connector::ask`] sends `connect_request`. The machine's owner answers
//!    on the machine; this end waits on the same socket ([`Ask`]).
//! 2. [`Approved::pair`] sends the eight-digit code and receives the device
//!    token, shown once ([`Paired`]). A machine that takes
//!    [`CAPABILITY_TOKEN_ROTATION`] hands out a refresh token and a short-lived
//!    access token instead of the static token.
//! 3. [`Connector::authenticate`] presents those tokens on later connections,
//!    after which the socket speaks ACP ([`Session::acp`]).
//!
//! A rotating pairing reconnects with its access token while the machine still
//! accepts it, and with the refresh token otherwise. Every accepted refresh
//! token is replaced: the new one is written to [`TokenStore`] *before* the
//! rotation is acknowledged, so a crash between the two leaves the previous
//! token valid as the retry the machine makes idempotent.
//!
//! A machine can be reached by several routes. [`Connector::connect`] tries
//! them in the saved order and takes the first that authenticates as the
//! machine we expect; the route racing, learning and moving back between
//! routes are the machine network's step 1.8.

use crate::protocol::{
    CAPABILITY_TOKEN_ROTATION, DEVICE_KIND_CLI, DEVICE_KIND_DESKTOP, HANDSHAKE_PROTOCOL_VERSION,
    HandshakeRequest, HandshakeResponse, PairingMethod, Secret,
};
use crate::registry::{Machine, Route, may_send_secrets, plain_ws_refusal, write_private_json};
use crate::tokens::{Access, TokenStore};
use crate::transport::{AcpChannel, Connection};
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// File holding this machine's own identifier under `machines/`.
const HOST_ID_FILE: &str = "host-id";
/// File holding this install's identifier under `machines/`.
const INSTALL_ID_FILE: &str = "install-id";

/// How long the owner has to answer a connection request. The gateway's own
/// request lifetime is two minutes; this leaves room for the answer's last
/// moment to arrive.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(150);

/// How long one route may take to authenticate before the next is tried.
pub const ROUTE_TIMEOUT: Duration = Duration::from_secs(5);

/// What kind of client this is. A label for the owner, and what the machine
/// grants: ACP access follows an accepted request from a [`ClientKind::Desktop`],
/// while a [`ClientKind::Cli`] device is allowed the session protocol until the
/// owner grants it (`rustcode remote devices allow <device>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    Desktop,
    Cli,
}

impl ClientKind {
    /// The `device_kind` on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => DEVICE_KIND_DESKTOP,
            Self::Cli => DEVICE_KIND_CLI,
        }
    }
}

/// What this machine runs: `macos`, `windows`, `linux`, … lowercase, as the
/// gateway stores it.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "unknown"
    }
}

/// The identity this computer presents to other machines.
pub struct Identity {
    /// A label the owner sees: the computer's name where there is one.
    pub device_name: String,
    /// This computer's stable identifier. It is the local gateway's
    /// `gateway_id` when this profile runs one, so a machine that paired with
    /// this computer and this computer's own gateway agree on one identity.
    pub host_id: String,
    /// One random identifier per install, so pairing again replaces this
    /// install's device record instead of adding another.
    pub install_id: String,
}

impl Identity {
    /// Read or create this profile's identity under `config_dir`:
    /// `remote/gateway.json` first, then `machines/host-id`.
    pub fn load(config_dir: &Path) -> Result<Self> {
        Ok(Self {
            device_name: default_device_name(),
            host_id: host_id_from_gateway(config_dir).unwrap_or(host_id_from_file(config_dir)?),
            install_id: install_id(config_dir)?,
        })
    }
}

/// `remote/gateway.json`, the identity the local remote gateway keeps. It is
/// read, never created here: the gateway creates it under its own lock.
fn host_id_from_gateway(config_dir: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct GatewayIdentity {
        gateway_id: String,
    }
    let bytes = std::fs::read(config_dir.join("remote/gateway.json")).ok()?;
    let identity: GatewayIdentity = serde_json::from_slice(&bytes).ok()?;
    usable_id(&identity.gateway_id).then_some(identity.gateway_id)
}

fn host_id_from_file(config_dir: &Path) -> Result<String> {
    read_or_create_id(&crate::config::machines_dir(config_dir).join(HOST_ID_FILE))
}

#[derive(Serialize, Deserialize)]
struct StoredId {
    id: String,
}

fn install_id_path(config_dir: &Path) -> PathBuf {
    crate::config::machines_dir(config_dir).join(INSTALL_ID_FILE)
}

/// This profile's install identifier, created on first use.
///
/// It is what keeps one `RUSTCODE_CONFIG_DIR` profile's keychain entries (see
/// [`crate::tokens`]) and its device records on other machines apart from
/// another profile's: the token store names every entry by it.
pub fn install_id(config_dir: &Path) -> Result<String> {
    read_or_create_id(&install_id_path(config_dir))
}

/// Read a stored identifier, or create one and keep it.
fn read_or_create_id(path: &Path) -> Result<String> {
    if let Ok(bytes) = std::fs::read(path)
        && let Ok(stored) = serde_json::from_slice::<StoredId>(&bytes)
        && usable_id(&stored.id)
    {
        return Ok(stored.id);
    }
    let id = random_id();
    write_private_json(path, &StoredId { id: id.clone() })?;
    Ok(id)
}

/// The gateway accepts an identifier of 16 to 128 printable ASCII characters
/// and ignores anything else, so only that shape is ever stored or sent.
fn usable_id(value: &str) -> bool {
    (16..=128).contains(&value.len()) && value.chars().all(|character| character.is_ascii_graphic())
}

/// A random 128-bit identifier as lowercase hex, the shape the engine uses for
/// gateway and instance identifiers.
pub fn random_id() -> String {
    let random: [u8; 16] = rand::random();
    hex::encode(random)
}

/// This computer's name as the owner of another machine should see it. The
/// shell exports `HOSTNAME`; Windows has `COMPUTERNAME`; otherwise the label
/// says what this is.
fn default_device_name() -> String {
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|name| name.trim().trim_end_matches(".local").to_owned())
        .filter(|name| !name.is_empty());
    match name {
        Some(name) => name.chars().filter(|c| !c.is_control()).take(64).collect(),
        None => format!("RustCode on {}", platform()),
    }
}

/// One client, with the identity and kind it pairs and authenticates as.
pub struct Connector {
    pub kind: ClientKind,
    pub identity: Identity,
    /// Per-step timeout for connecting and for each handshake frame.
    pub timeout: Duration,
    /// Per-route deadline used when choosing a route. Short: a route that does
    /// not answer is skipped rather than waited on.
    pub route_timeout: Duration,
}

impl Connector {
    pub fn new(config_dir: &Path, kind: ClientKind) -> Result<Self> {
        Ok(Self {
            kind,
            identity: Identity::load(config_dir)?,
            timeout: crate::transport::CONNECT_TIMEOUT,
            route_timeout: ROUTE_TIMEOUT,
        })
    }

    /// Ask `address` to let this client pair: send `connect_request` and wait
    /// for the machine to put it in front of its owner.
    ///
    /// A public address is refused before anything is dialled: the same socket
    /// carries the code and then the token, and plain `ws` would put both on
    /// the network in the clear (#2651).
    pub async fn ask(&self, address: &str) -> Result<Ask> {
        if !may_send_secrets(address) {
            return Err(plain_ws_refusal(address));
        }
        let mut connection = Connection::open_with_timeout(address, self.route_timeout).await?;
        let request = HandshakeRequest::ConnectRequest {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            device_name: self.identity.device_name.clone(),
            platform: platform().to_owned(),
            install_id: Some(self.identity.install_id.clone()),
            device_kind: Some(self.kind.as_str().to_owned()),
            host_id: Some(self.identity.host_id.clone()),
        };
        connection.send_handshake(&request).await?;
        let answer = connection.next_handshake(self.timeout).await?;
        match answer {
            HandshakeResponse::ConnectPending {
                request_id,
                expires_in_secs,
            } => Ok(Ask {
                connection,
                connect: request,
                request_id,
                expires_in: Duration::from_secs(expires_in_secs),
            }),
            other => Err(handshake_failure("connect_request", other)),
        }
    }

    /// Authenticate with a saved machine's tokens on `address`.
    ///
    /// The tokens are never sent to a public address on plain `ws`: that would
    /// hand them to the network before the machine could prove who it is. See
    /// [`crate::registry::may_send_secrets`] and #2651.
    ///
    /// A rotating device reconnects with the access token in memory while the
    /// machine still takes it, rotating nothing; otherwise it presents the
    /// durable refresh token, and the answer replaces it. The new refresh token
    /// is stored before the rotation is acknowledged ([`crate::tokens`]). A
    /// gateway that predates rotating tokens does not know the `refresh_token`
    /// field and answers `invalid_frame`; the static form saved by an older
    /// pairing is tried once after that.
    pub async fn authenticate(
        &self,
        address: &str,
        machine: &Machine,
        tokens: &TokenStore,
    ) -> Result<Session> {
        if !may_send_secrets(address) {
            return Err(plain_ws_refusal(address));
        }
        let stored = tokens.get(&machine.host_id)?.with_context(|| {
            format!("{} has no pairing token; pair with it again", machine.name)
        })?;

        // 1. The access token from the last connection, while it is valid. A
        //    reconnect inside its 15 minutes rotates nothing.
        if let Some(access) = tokens
            .access(&machine.host_id)
            .filter(|access| access.is_valid_at(Utc::now()))
        {
            let (connection, answer) = self
                .exchange(address, &authenticate_access(machine, access.token()))
                .await?;
            match answer {
                HandshakeResponse::Error {
                    code: crate::protocol::ErrorCode::AccessExpired,
                    ..
                } => {
                    // Expired, replaced or minted before a restart: the refresh
                    // token is the one that never expires. Fall through once,
                    // on this route.
                    tokens.forget_access(&machine.host_id);
                }
                HandshakeResponse::Authenticated { .. } => {
                    return self
                        .accept(machine, tokens, connection, answer, Some(access))
                        .await;
                }
                other => return Err(handshake_failure("authenticate", other)),
            }
        }

        // 2. The durable token. A rotating device names the capability again
        //    on this frame; the machine tied the pairing to it at `pair` time.
        //    The same frame is how a static pairing of a gateway this client
        //    paired with before rotation keeps working: the machine answers a
        //    refresh credential for a non-rotating record with that record.
        let (connection, answer) = self
            .exchange(address, &authenticate_refresh(machine, &stored))
            .await?;
        if let HandshakeResponse::Error {
            code: crate::protocol::ErrorCode::InvalidFrame,
            ..
        } = answer
        {
            // A gateway from before rotating tokens does not know
            // `refresh_token`; the static form is what it reads.
            let (connection, answer) = self
                .exchange(address, &authenticate_static(machine, &stored))
                .await?;
            return match answer {
                HandshakeResponse::Authenticated { .. } => {
                    self.accept(machine, tokens, connection, answer, None).await
                }
                other => Err(handshake_failure("authenticate", other)),
            };
        }
        match answer {
            HandshakeResponse::Authenticated { .. } => {
                self.accept(machine, tokens, connection, answer, None).await
            }
            other => Err(handshake_failure("authenticate", other)),
        }
    }

    /// Send one handshake frame on a fresh socket and read the answer. Error
    /// frames come back as the frame, not as an error, so the caller can read
    /// the code and try the next credential.
    async fn exchange(
        &self,
        address: &str,
        frame: &HandshakeRequest,
    ) -> Result<(Connection, HandshakeResponse)> {
        let mut connection = Connection::open_with_timeout(address, self.route_timeout).await?;
        connection.send_handshake(frame).await?;
        let answer = connection.next_handshake(self.timeout).await?;
        Ok((connection, answer))
    }

    /// Turn an accepted `authenticated` frame into a [`Session`], storing what
    /// the machine handed out. The new refresh token is durable before the
    /// rotation is acknowledged; the acknowledgement goes on the same socket,
    /// which is still speaking remote protocol frames.
    async fn accept(
        &self,
        machine: &Machine,
        tokens: &TokenStore,
        mut connection: Connection,
        answer: HandshakeResponse,
        presented: Option<Access>,
    ) -> Result<Session> {
        let HandshakeResponse::Authenticated {
            gateway_id,
            instance_id,
            device_id,
            device_name,
            host_name,
            expires_at,
            refresh_token,
            access_token,
            access_expires_at,
            rotation_id,
            wire_names,
            ..
        } = answer
        else {
            return Err(handshake_failure("authenticate", answer));
        };
        if let Some(refresh) = &refresh_token {
            // Durable first: until this write lands, the answer is not
            // acknowledged and the token before it stays valid at the machine.
            tokens.set(&machine.host_id, refresh).with_context(|| {
                format!(
                    "{} replaced this computer's refresh token, but it could not be stored, \
                     so the rotation was not acknowledged; the token this computer already has \
                     stays valid until a later connection stores the new one",
                    machine.name
                )
            })?;
        }
        let access = match (access_token, parse_expiry(access_expires_at.as_deref())) {
            // A rotation hands out a new access token and its expiry.
            (Some(token), Some(expires_at)) => Some(Access::new(token, expires_at)),
            // An access reconnect hands out only the expiry: the token this
            // client presented keeps working until then.
            (None, Some(expires_at)) => {
                presented.map(|access| Access::new(access.token().clone(), expires_at))
            }
            (Some(_), None) | (None, None) => None,
        };
        match &access {
            Some(access) => tokens.keep_access(&machine.host_id, access.clone()),
            // Nothing to time: do not present it again.
            None => tokens.forget_access(&machine.host_id),
        }
        if let (Some(rotation_id), true) = (rotation_id, refresh_token.is_some()) {
            connection
                .send_handshake(&HandshakeRequest::TokenAck { rotation_id })
                .await
                .context("acknowledge the rotated refresh token")?;
        }
        Ok(Session {
            connection,
            welcome: Welcome {
                gateway_id,
                instance_id,
                device_id,
                device_name,
                host_name,
                expires_at,
                wire_names,
            },
            access,
        })
    }

    /// Connect to `machine` over the first route that authenticates as it.
    ///
    /// A route is accepted only when the machine behind it reports the
    /// `host_id` the machine was saved under, so a stale address that now
    /// belongs to another computer is skipped instead of trusted.
    pub async fn connect(
        &self,
        machine: &Machine,
        tokens: &TokenStore,
    ) -> Result<(Session, Route)> {
        ensure!(
            !machine.routes.is_empty(),
            "{} has no route to try; add one",
            machine.name
        );
        // A public route would carry the tokens in the clear; it is not tried
        // at all, and a machine reachable only that way says why (#2651).
        let routes: Vec<&Route> = machine
            .routes
            .iter()
            .filter(|route| may_send_secrets(&route.address))
            .collect();
        if routes.is_empty() {
            return Err(plain_ws_refusal(&machine.routes[0].address));
        }
        let mut attempts = Vec::new();
        let mut refused = None;
        for route in routes {
            let result = match self.authenticate(&route.address, machine, tokens).await {
                Ok(session) => session.check(machine).map(|()| session),
                Err(error) => Err(error),
            };
            match result {
                Ok(session) => return Ok((session, route.clone())),
                Err(error) => {
                    if let Some(refusal) = error.downcast_ref::<Refusal>() {
                        refused = Some(refusal.code);
                        if refusal.code.revoked_pairing() {
                            // The machine revoked this pairing (or a token it
                            // retired came back): the saved token is worthless,
                            // and keeping it would only present it again.
                            let _ = tokens.remove(&machine.host_id);
                            attempts.push(format!("{}: {error:#}", route.address));
                            break;
                        }
                    }
                    attempts.push(format!("{}: {error:#}", route.address));
                }
            }
        }
        Err(anyhow::Error::new(Unreachable {
            machine: machine.name.clone(),
            attempts,
            refused,
        }))
    }

    /// Connect and read the titles of the machine's threads, as ACP's
    /// `session/list` reports them. This is the one read that proves ACP
    /// works end to end; the thread list itself belongs to step 1.9.
    pub async fn thread_titles(
        &self,
        machine: &Machine,
        tokens: &TokenStore,
    ) -> Result<Vec<String>> {
        let (session, _route) = self.connect(machine, tokens).await?;
        let mut acp = session.acp().await?;
        thread_titles(&mut acp).await
    }
}

/// A connection request waiting for the machine's owner.
pub struct Ask {
    connection: Connection,
    /// The `connect_request` sent, repeated in the `pair` frame the gateway
    /// expects after `connect_approved`.
    connect: HandshakeRequest,
    request_id: String,
    expires_in: Duration,
}

impl std::fmt::Debug for Ask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Ask")
            .field("request_id", &self.request_id)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

impl Ask {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// How long the owner has to answer.
    pub fn expires_in(&self) -> Duration {
        self.expires_in
    }

    /// Wait for the owner's answer. `None` means nobody answered in time.
    pub async fn wait(self, timeout: Duration) -> Result<Option<Approved>> {
        let mut connection = self.connection;
        let answer = match connection.next_handshake(timeout).await {
            Ok(answer) => answer,
            // The machine dropped the socket when the request ran out.
            Err(_) if timeout >= self.expires_in => return Ok(None),
            Err(error) => return Err(error),
        };
        match answer {
            HandshakeResponse::ConnectApproved {
                code_length,
                expires_in_secs,
                ..
            } => Ok(Some(Approved {
                connection,
                connect: self.connect,
                code_length,
                expires_in: Duration::from_secs(expires_in_secs),
            })),
            HandshakeResponse::ConnectDenied => bail!("the machine's owner denied the request"),
            HandshakeResponse::ConnectExpired => Ok(None),
            other => Err(handshake_failure("connect_request", other)),
        }
    }
}

/// An accepted connection request, waiting for the code.
pub struct Approved {
    connection: Connection,
    connect: HandshakeRequest,
    code_length: u32,
    expires_in: Duration,
}

impl std::fmt::Debug for Approved {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Approved")
            .field("code_length", &self.code_length)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

impl Approved {
    /// How many digits the code has, as the machine said.
    pub fn code_length(&self) -> u32 {
        self.code_length
    }

    /// How long the code is accepted for.
    pub fn expires_in(&self) -> Duration {
        self.expires_in
    }

    /// Present the code the machine shows. `code` may be typed with a
    /// separator (`4821-9034`) or without; the gateway compares digits.
    pub async fn pair(mut self, code: &str) -> Result<Paired> {
        let code = normalize_code(code);
        ensure!(
            !code.is_empty(),
            "enter the {} digits the machine shows",
            self.code_length
        );
        let HandshakeRequest::ConnectRequest {
            device_name,
            platform,
            install_id,
            device_kind,
            host_id,
            ..
        } = &self.connect
        else {
            bail!("a `pair` frame follows a `connect_request`");
        };
        let request = HandshakeRequest::Pair {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            method: PairingMethod::Code,
            secret: Secret::new(code),
            device_name: device_name.clone(),
            install_id: install_id.clone(),
            wire_names: None,
            device_kind: device_kind.clone(),
            platform: Some(platform.clone()),
            host_id: host_id.clone(),
            // Ask for a refresh token instead of the static token: the pairing
            // then lasts until it is revoked, and reconnect does not expire.
            capabilities: Some(vec![CAPABILITY_TOKEN_ROTATION.to_owned()]),
        };
        self.connection.send_handshake(&request).await?;
        let answer = self
            .connection
            .next_handshake(crate::transport::CONNECT_TIMEOUT)
            .await?;
        match answer {
            HandshakeResponse::Paired {
                gateway_id,
                instance_id,
                device_id,
                device_name,
                token,
                host_name,
                expires_at,
                refresh_token,
                access_token,
                access_expires_at,
                wire_names,
                ..
            } => {
                // A gateway that takes the capability sends a refresh token;
                // an older one ignores it and sends the static token, which is
                // presented as the refresh token from then on.
                let secret = refresh_token.or(token).context(
                    "the machine accepted the pairing but sent no device token; pair again",
                )?;
                let access = access_token
                    .zip(parse_expiry(access_expires_at.as_deref()))
                    .map(|(token, expires_at)| Access::new(token, expires_at));
                Ok(Paired {
                    secret,
                    access,
                    session: Session {
                        connection: self.connection,
                        welcome: Welcome {
                            gateway_id,
                            instance_id,
                            device_id,
                            device_name,
                            host_name,
                            expires_at,
                            wire_names,
                        },
                        access: None,
                    },
                })
            }
            other => Err(handshake_failure("pair", other)),
        }
    }
}

/// A successful pairing: the durable token, and the authenticated connection.
pub struct Paired {
    /// What [`TokenStore::set`] keeps: the refresh token of a machine that
    /// rotates its tokens, or the static token of one that does not.
    pub secret: Secret,
    /// The access token this pairing was given, in memory only. It reconnects
    /// without rotating anything until it expires.
    pub access: Option<Access>,
    pub session: Session,
}

impl Paired {
    /// Keep what the pairing handed out: the durable token through
    /// [`TokenStore::set`], then the access token in memory for the next
    /// [`Connector::connect`].
    pub fn keep(&self, tokens: &TokenStore, host_id: &str) -> Result<()> {
        tokens.set(host_id, &self.secret)?;
        if let Some(access) = &self.access {
            tokens.keep_access(host_id, access.clone());
        }
        Ok(())
    }

    /// The access token the machine minted for this connection, if it handed
    /// one out.
    pub fn access(&self) -> Option<&Access> {
        self.access.as_ref()
    }
}

impl std::fmt::Debug for Paired {
    /// Never the token: a pairing credential in a log is a pairing credential
    /// in a log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Paired")
            .field("secret", &"<redacted>")
            .field("rotates", &self.access.is_some())
            .field("session", &self.session)
            .finish()
    }
}

/// The machine's answer when a pairing is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    /// The machine's stable identifier: what a [`Machine`] is keyed by.
    pub gateway_id: String,
    /// The machine's gateway process; changes on every gateway start.
    pub instance_id: String,
    /// This client's device record on the machine.
    pub device_id: String,
    pub device_name: String,
    /// The machine's own name, for display: a label, not an identity.
    pub host_name: Option<String>,
    pub expires_at: Option<String>,
    pub wire_names: Option<String>,
}

impl Welcome {
    /// The name to show for the machine.
    pub fn machine_name(&self) -> String {
        self.host_name
            .clone()
            .unwrap_or_else(|| self.gateway_id.clone())
    }
}

/// An authenticated connection to one machine.
pub struct Session {
    connection: Connection,
    welcome: Welcome,
    /// The access token this connection presented or was given, in memory only.
    access: Option<Access>,
}

impl std::fmt::Debug for Session {
    /// The socket stays out of it; the machine and the device identify it.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("gateway_id", &self.welcome.gateway_id)
            .field("device_id", &self.welcome.device_id)
            .field("access", &self.access)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// The access token this connection reconnects with, until it expires.
    pub fn access(&self) -> Option<&Access> {
        self.access.as_ref()
    }

    /// Check that this address really is the machine we saved: same `host_id`,
    /// and the device record the token belongs to.
    pub fn check(&self, machine: &Machine) -> Result<()> {
        if self.welcome.gateway_id != machine.host_id {
            bail!(
                "that address answers as {} ({}), not {}",
                self.welcome.machine_name(),
                self.welcome.gateway_id,
                machine.name
            );
        }
        if self.welcome.device_id != machine.device_id {
            bail!(
                "{} replaced this computer's device record; pair with it again",
                machine.name
            );
        }
        Ok(())
    }

    /// Switch the connection to ACP.
    pub async fn acp(self) -> Result<AcpChannel> {
        self.connection.acp_open().await
    }
}

/// The titles of the machine's threads, newest first as `session/list` lists
/// them. One ACP round trip: `initialize`, then `session/list`.
pub async fn thread_titles(channel: &mut AcpChannel) -> Result<Vec<String>> {
    channel
        .request(1, "initialize", serde_json::json!({"protocolVersion": 1}))
        .await
        .context("the machine refused the ACP handshake")?;
    let answer = channel
        .request(2, "session/list", serde_json::json!({}))
        .await
        .context("the machine refused session/list")?;
    let sessions = answer["result"]["sessions"]
        .as_array()
        .context("session/list answered without a session list")?;
    Ok(sessions
        .iter()
        .filter_map(|session| session["title"].as_str().map(str::to_owned))
        .collect())
}

/// Turn a failure frame into an error a person can act on.
fn handshake_failure(step: &str, answer: HandshakeResponse) -> anyhow::Error {
    match answer {
        HandshakeResponse::Error {
            code,
            message,
            retry_after_secs,
        } => anyhow::Error::new(Refusal {
            step: step.to_owned(),
            code,
            message,
            retry_after_secs,
        }),
        other => anyhow::anyhow!("the machine answered {step} with {other:?}"),
    }
}

/// The `authenticate` frame that reconnects with an access token: no
/// capability, because nothing is asked for and nothing rotates.
fn authenticate_access(machine: &Machine, token: &Secret) -> HandshakeRequest {
    HandshakeRequest::Authenticate {
        protocol_version: HANDSHAKE_PROTOCOL_VERSION,
        device_id: machine.device_id.clone(),
        token: None,
        refresh_token: None,
        access_token: Some(token.clone()),
        wire_names: None,
        capabilities: None,
    }
}

/// The `authenticate` frame that exchanges the durable token for a new pair.
/// It is the refresh token of a rotating device, and the static token of a
/// pairing the machine does not rotate.
fn authenticate_refresh(machine: &Machine, token: &Secret) -> HandshakeRequest {
    HandshakeRequest::Authenticate {
        protocol_version: HANDSHAKE_PROTOCOL_VERSION,
        device_id: machine.device_id.clone(),
        token: None,
        refresh_token: Some(token.clone()),
        access_token: None,
        wire_names: None,
        capabilities: Some(vec![CAPABILITY_TOKEN_ROTATION.to_owned()]),
    }
}

/// The pre-rotation form of `authenticate`: the token under `token` and no
/// capability, which a gateway from before rotating tokens understands.
fn authenticate_static(machine: &Machine, token: &Secret) -> HandshakeRequest {
    HandshakeRequest::Authenticate {
        protocol_version: HANDSHAKE_PROTOCOL_VERSION,
        device_id: machine.device_id.clone(),
        token: Some(token.clone()),
        refresh_token: None,
        access_token: None,
        wire_names: None,
        capabilities: None,
    }
}

/// An RFC 3339 UTC instant from the machine, or `None` when it is absent or
/// not one: an access token whose expiry cannot be read is not kept, and the
/// next connection presents the refresh token instead.
fn parse_expiry(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?;
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|instant| instant.with_timezone(&Utc))
}

/// The machine answered and refused: the gateway's error code, as a type, so
/// a caller can tell "pair again" from "offline" without reading a message.
#[derive(Debug, Clone)]
pub struct Refusal {
    /// The step that was refused: `authenticate`, `connect_request`, `pair`.
    pub step: String,
    pub code: crate::protocol::ErrorCode,
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hint = if self.code.revoked_pairing() {
            "; pairing revoked — pair with the machine again"
        } else if self.code.needs_new_pairing() {
            "; pair with the machine again"
        } else if matches!(self.code, crate::protocol::ErrorCode::UnauthorizedAccess) {
            "; on that machine, run `rustcode remote devices allow <device>`"
        } else {
            ""
        };
        write!(
            formatter,
            "the machine refused {} ({}): {}{}{hint}",
            self.step,
            self.code,
            self.message,
            self.retry_after_secs
                .map(|seconds| format!(" (retry in {seconds}s)"))
                .unwrap_or_default(),
        )
    }
}

impl std::error::Error for Refusal {}

/// Every route failed. `refused` carries the gateway code that a new pairing
/// would fix, when one of them refused the token rather than not answering.
#[derive(Debug, Clone)]
pub struct Unreachable {
    pub machine: String,
    /// One line per route: the address and what happened.
    pub attempts: Vec<String>,
    pub refused: Option<crate::protocol::ErrorCode>,
}

impl Unreachable {
    /// Whether pairing again is what would fix this.
    pub fn needs_new_pairing(&self) -> bool {
        self.refused.is_some_and(|code| code.needs_new_pairing())
    }
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "could not reach {}: {}",
            self.machine,
            self.attempts.join("; ")
        )
    }
}

impl std::error::Error for Unreachable {}

/// Digits only: a code typed as `4821-9034` is the same code as `48219034`.
pub fn normalize_code(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

/// A machine record built from a pairing: the route it paired over, the
/// machine's name and the device record the token belongs to.
pub fn machine_from_pairing(welcome: &Welcome, route: Route, name: Option<String>) -> Machine {
    Machine::new(
        &welcome.gateway_id,
        name.unwrap_or_else(|| welcome.machine_name()),
        // The gateway does not report its platform yet (step 1.7 adds it);
        // `unknown` is honest, and a later pairing fills it in.
        "unknown",
        &welcome.device_id,
        route,
        chrono::Utc::now(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_dir() -> tempfile::TempDir {
        tempfile::tempdir_in("/tmp").unwrap()
    }

    /// A public address never receives the code or the token: the guard runs
    /// before a socket is opened, so a black-holed address fails at once
    /// instead of after a connect timeout.
    #[tokio::test]
    async fn a_public_address_is_refused_before_any_socket_is_opened() {
        let dir = config_dir();
        let connector = Connector::new(dir.path(), ClientKind::Cli).unwrap();
        let public = "203.0.113.9:17879";
        let error = connector.ask(public).await.unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("#2651"), "{message}");
        assert!(message.contains(public), "{message}");

        let machine = Machine::new(
            "gw-a",
            "A",
            "macos",
            "device-1",
            Route::with_kind(crate::registry::RouteKind::Public, public),
            chrono::Utc::now(),
        );
        let token = Secret::new("token");
        let tokens = TokenStore::in_file(dir.path()).unwrap();
        tokens.set(&machine.host_id, &token).unwrap();
        let error = connector
            .authenticate(public, &machine, &tokens)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("#2651"), "{error:#}");

        let error = connector.connect(&machine, &tokens).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("#2651"),
            "a machine reachable only over a public plain-ws route says why: {error:#}"
        );
    }

    /// The guard is about the address, not the saved label: a route saved as
    /// `public` but pointing at a LAN address is still dialled.
    #[tokio::test]
    async fn a_lan_address_is_not_refused_for_a_public_label() {
        let dir = config_dir();
        let connector = Connector::new(dir.path(), ClientKind::Cli).unwrap();
        let machine = Machine::new(
            "gw-a",
            "A",
            "macos",
            "device-1",
            Route::with_kind(crate::registry::RouteKind::Public, "127.0.0.1:1"),
            chrono::Utc::now(),
        );
        let tokens = TokenStore::in_file(dir.path()).unwrap();
        tokens.set(&machine.host_id, &Secret::new("token")).unwrap();
        let error = connector
            .authenticate("127.0.0.1:1", &machine, &tokens)
            .await
            .unwrap_err();
        assert!(
            !format!("{error:#}").contains("#2651"),
            "a loopback address is not a public one: {error:#}"
        );
    }

    #[test]
    fn a_code_is_compared_as_digits() {
        assert_eq!(normalize_code("4821-9034"), "48219034");
        assert_eq!(normalize_code(" 4821 9034 "), "48219034");
        assert_eq!(normalize_code("48219034"), "48219034");
        assert_eq!(normalize_code("nothing"), "");
    }

    /// The three `authenticate` frames are the shapes the contract documents:
    /// an access token asks for nothing, a refresh token names the capability,
    /// and the pre-rotation form puts the token under `token`.
    #[test]
    fn the_authenticate_frames_are_the_ones_the_contract_documents() {
        let machine = Machine::new(
            "gw-a",
            "A",
            "macos",
            "0011223344556677",
            Route::new("10.0.0.2"),
            chrono::Utc::now(),
        );
        let expected = |credential: &str, capability: bool| {
            let mut frame = serde_json::json!({
                "type": "authenticate",
                "protocol_version": 1,
                "device_id": "0011223344556677",
            });
            frame[credential] = serde_json::json!("the-token");
            if capability {
                frame["capabilities"] = serde_json::json!([CAPABILITY_TOKEN_ROTATION]);
            }
            frame
        };
        let token = Secret::new("the-token");
        assert_eq!(
            serde_json::to_value(authenticate_access(&machine, &token)).unwrap(),
            expected("access_token", false)
        );
        assert_eq!(
            serde_json::to_value(authenticate_refresh(&machine, &token)).unwrap(),
            expected("refresh_token", true)
        );
        assert_eq!(
            serde_json::to_value(authenticate_static(&machine, &token)).unwrap(),
            expected("token", false)
        );
    }

    /// Only a real future instant is an expiry: an absent or unreadable one
    /// means the access token is not kept, and the refresh token reconnects.
    #[test]
    fn an_access_expiry_is_read_leniently() {
        assert!(parse_expiry(None).is_none());
        assert!(parse_expiry(Some("soon")).is_none());
        assert!(parse_expiry(Some("2026-08-22")).is_none());
        let instant = parse_expiry(Some("2026-08-22T09:15:00Z")).expect("RFC 3339");
        assert_eq!(instant.to_rfc3339(), "2026-08-22T09:15:00+00:00");
        let access = Access::new(Secret::new("a1"), instant);
        assert!(access.is_valid_at(instant - chrono::Duration::seconds(1)));
        assert!(!access.is_valid_at(instant));
        assert!(!format!("{access:?}").contains("a1"));
    }

    #[test]
    fn the_identity_is_created_once_and_reused() {
        let dir = config_dir();
        let first = Identity::load(dir.path()).unwrap();
        assert!(usable_id(&first.host_id), "{}", first.host_id);
        assert_eq!(first.host_id.len(), 32);
        assert!(usable_id(&first.install_id));
        assert!(!first.device_name.is_empty());
        let second = Identity::load(dir.path()).unwrap();
        assert_eq!(second.host_id, first.host_id);
        assert_eq!(second.install_id, first.install_id);
    }

    /// The identity of the local gateway is this computer's identity, so a
    /// machine this one pairs with and this one's own gateway agree.
    #[test]
    fn the_local_gateway_identity_is_preferred() {
        let dir = config_dir();
        std::fs::create_dir_all(dir.path().join("remote")).unwrap();
        std::fs::write(
            dir.path().join("remote/gateway.json"),
            br#"{"gateway_id":"gw-0123456789abcdef"}"#,
        )
        .unwrap();
        let identity = Identity::load(dir.path()).unwrap();
        assert_eq!(identity.host_id, "gw-0123456789abcdef");
        // A malformed or unusable one falls back to this crate's own file.
        std::fs::write(dir.path().join("remote/gateway.json"), b"{}").unwrap();
        let identity = Identity::load(dir.path()).unwrap();
        assert_eq!(identity.host_id.len(), 32);
    }

    #[cfg(unix)]
    #[test]
    fn identity_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = config_dir();
        Identity::load(dir.path()).unwrap();
        for name in ["host-id", "install-id"] {
            let path = crate::config::machines_dir(dir.path()).join(name);
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{name}");
        }
    }

    #[test]
    fn a_pairing_builds_the_machine_record() {
        let welcome = Welcome {
            gateway_id: "gw-8e1f4b7a".into(),
            instance_id: "inst-1".into(),
            device_id: "device-9".into(),
            device_name: "box".into(),
            host_name: Some("Studio".into()),
            expires_at: None,
            wire_names: None,
        };
        let machine = machine_from_pairing(&welcome, Route::new("192.168.1.20:17879"), None);
        assert_eq!(machine.host_id, "gw-8e1f4b7a");
        assert_eq!(machine.name, "Studio");
        assert_eq!(machine.platform, "unknown");
        assert_eq!(machine.device_id, "device-9");
        // An explicit name wins over the machine's own.
        let named = machine_from_pairing(
            &welcome,
            Route::new("192.168.1.20:17879"),
            Some("the Mac".into()),
        );
        assert_eq!(named.name, "the Mac");
    }

    #[test]
    fn a_session_must_belong_to_the_saved_machine() {
        let welcome = Welcome {
            gateway_id: "gw-a".into(),
            instance_id: "inst-1".into(),
            device_id: "device-9".into(),
            device_name: "box".into(),
            host_name: None,
            expires_at: None,
            wire_names: None,
        };
        let mut machine = Machine::new(
            "gw-a",
            "Studio",
            "macos",
            "device-9",
            Route::new("10.0.0.2"),
            chrono::Utc::now(),
        );
        let check = |welcome: &Welcome, machine: &Machine| -> Result<()> {
            // `Session::check` without a socket behind it.
            if welcome.gateway_id != machine.host_id {
                bail!("that address answers as {}", welcome.machine_name());
            }
            if welcome.device_id != machine.device_id {
                bail!("{} replaced this computer's device record", machine.name);
            }
            Ok(())
        };
        assert!(check(&welcome, &machine).is_ok());
        machine.device_id = "device-other".into();
        assert!(check(&welcome, &machine).is_err());
        machine.device_id = "device-9".into();
        machine.host_id = "gw-b".into();
        assert!(check(&welcome, &machine).is_err());
    }
}
