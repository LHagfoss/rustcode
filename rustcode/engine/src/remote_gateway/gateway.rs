//! The gateway process: listeners, limits and the one state lock.
//!
//! Pairing state, the device set and the live-connection registry sit behind
//! a single mutex. Every security decision is one critical section:
//! redeeming a challenge and enrolling its device, checking a token and
//! registering the connection, forgetting a device and closing its
//! connections. Nothing awaits while the lock is held.

use super::address::{
    AdvertisedAddress, Candidate, HostRoute, ListenPlan, alternate_addresses, candidate_addresses,
    host_routes,
};
use super::connect::{ConnectRequests, OpenError, Outcome, RequestEnd, RequestState};
use super::control::{
    CONTROL_TIMEOUT, ConnectAnswer, ConnectRequestSummary, ConnectionSummary, ControlRequest,
    ControlResponse, DeviceSummary, EndedChallenge, EndedConnectRequest, GatewayStatus,
    OfferDetails, PairingStatus,
};
use super::devices::{
    Acknowledgement, AuthenticateError, Credential, DeviceError, DeviceProfile, DeviceRecord,
    DeviceStore, Devices, RevocationNotes, apply_revocation_notes,
};
use super::handshake::{
    ErrorCode, HANDSHAKE_PROTOCOL_VERSION, HandshakeRequest, HandshakeResponse, PairingMethod,
};
use super::hub::{HubIdentity, SessionHub};
use super::lifecycle::{GatewayRegistration, RemoteLifecycle};
use super::local_socket::{LocalListener, LocalStream};
use super::owner_ipc;
use super::pairing::{PairingError, PairingState};
use super::router::{ConnectionControl, DeviceContext, FrameQueue, FrameRouter, FrameSink};
use super::transport;
use crate::daemon::private_files::remove_if_exists;
use crate::daemon::protocol::{read_async_frame, write_async_frame};
use crate::remote::host_capability;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Largest text frame accepted from a device, before or after authentication.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Resource bounds of one gateway. The defaults are the production values;
/// tests shrink them to exercise each bound quickly.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// From TCP accept to a complete first frame, WebSocket upgrade included.
    pub handshake_timeout: Duration,
    /// Sockets that have not authenticated yet, across all peers.
    pub max_unauthenticated: usize,
    /// The share of those one peer address may hold.
    pub max_unauthenticated_per_ip: usize,
    /// Authenticated connections, across all devices.
    pub max_authenticated: usize,
    /// Frames queued for one device before it is dropped as a slow consumer.
    pub outbound_queue: usize,
    /// How long one frame may take to reach a device's socket.
    pub write_timeout: Duration,
    pub ping_interval: Duration,
    /// Silence, pongs included, after which a connection is closed.
    pub idle_timeout: Duration,
    /// How often the listen plan is resolved again; see [`GatewayConfig::replan`].
    pub replan_interval: Duration,
    /// How long a connection request waits for the owner's answer.
    pub connect_request_lifetime: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(10),
            max_unauthenticated: 16,
            max_unauthenticated_per_ip: 4,
            max_authenticated: 32,
            outbound_queue: 64,
            write_timeout: Duration::from_secs(10),
            ping_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(90),
            replan_interval: Duration::from_secs(5),
            connect_request_lifetime: super::connect::REQUEST_LIFETIME,
        }
    }
}

/// Resolves the listen plan again from the machine's current addresses.
/// `None` means there is nothing better to move to right now.
pub type Replan = Arc<dyn Fn() -> Option<ListenPlan> + Send + Sync>;

pub struct GatewayConfig {
    pub plan: ListenPlan,
    /// Asked every [`Limits::replan_interval`]. When its answer differs from
    /// the plan in use (DHCP handed out another address, Wi-Fi came up after
    /// the gateway did), the listener moves there and pairing details, status
    /// and the registration follow. `None` keeps `plan` for good.
    pub replan: Option<Replan>,
    pub port: u16,
    pub router: Arc<dyn FrameRouter>,
    /// The session registry behind `router`, when it is one: session owners
    /// that connect to the owner socket register with it. `None` leaves the
    /// owner socket closed.
    pub sessions: Option<Arc<SessionHub>>,
    pub limits: Limits,
}

/// This machine's name as its user knows it, for a device to show next to
/// the address. Printable characters only, and short.
pub(super) fn host_name() -> Option<String> {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(read_host_name).clone()
}

fn read_host_name() -> Option<String> {
    // macOS often takes its BSD host name from reverse DNS, which names the
    // ISP connection rather than the Mac; the computer name is the user's.
    #[cfg(target_os = "macos")]
    if let Some(name) = crate::platform::command("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| clean_host_name(&String::from_utf8_lossy(&output.stdout)))
    {
        return Some(name);
    }
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its length; the last byte stays zero.
        let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) };
        if status != 0 {
            return None;
        }
        let length = buffer.iter().position(|byte| *byte == 0)?;
        clean_host_name(&String::from_utf8_lossy(&buffer[..length]))
    }
    // The computer name the user set in Settings › System › About.
    #[cfg(not(unix))]
    clean_host_name(&sysinfo::System::host_name()?)
}

fn clean_host_name(name: &str) -> Option<String> {
    let name: String = name
        .trim()
        .trim_end_matches(".local")
        .chars()
        .filter(|character| !character.is_control())
        .take(64)
        .collect();
    (!name.is_empty()).then_some(name)
}

struct State {
    pairing: PairingState,
    connect: ConnectRequests,
    devices: Devices,
    live: HashMap<u64, Live>,
    next_connection: u64,
}

struct Live {
    device_id: String,
    device_name: String,
    control: Arc<ConnectionControl>,
}

#[derive(Default)]
struct Unauthenticated {
    total: usize,
    per_ip: HashMap<IpAddr, usize>,
}

pub(super) struct Shared {
    pub(super) gateway_id: String,
    pub(super) instance_id: String,
    /// The address pairing details name. Replaced when the listener moves.
    advertised: tokio::sync::watch::Sender<AdvertisedAddress>,
    /// The local address the listener is bound to, for interface-scoped Bonjour.
    bind: tokio::sync::watch::Sender<IpAddr>,
    registration: Arc<Mutex<GatewayRegistration>>,
    pub(super) lifecycle: RemoteLifecycle,
    pub(super) limits: Limits,
    pub(super) router: Arc<dyn FrameRouter>,
    sessions: Option<Arc<SessionHub>>,
    host_name: Option<String>,
    /// How the listen plan was asked for; it does not change while the
    /// gateway runs, only the addresses it resolves to do.
    automatic_bind: bool,
    fixed_advertise: bool,
    pub(super) shutdown: CancellationToken,
    store: DeviceStore,
    /// Revocation notes a failed store write left, applied at startup.
    notes: RevocationNotes,
    state: Mutex<State>,
    unauthenticated: Mutex<Unauthenticated>,
}

/// One of the bounded slots for sockets that have not authenticated.
pub(super) struct UnauthenticatedSlot {
    shared: Arc<Shared>,
    ip: IpAddr,
}

impl UnauthenticatedSlot {
    /// The peer's address, canonical.
    pub(super) fn ip(&self) -> IpAddr {
        self.ip
    }
}

impl Drop for UnauthenticatedSlot {
    fn drop(&mut self) {
        let mut slots = self
            .shared
            .unauthenticated
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slots.total = slots.total.saturating_sub(1);
        if let Some(count) = slots.per_ip.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                slots.per_ip.remove(&self.ip);
            }
        }
    }
}

/// An authenticated connection's entry in the live registry.
pub(super) struct LiveConnection {
    shared: Arc<Shared>,
    pub(super) context: DeviceContext,
    pub(super) control: Arc<ConnectionControl>,
}

impl Drop for LiveConnection {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.live.remove(&self.context.connection_id);
        // Last seen is when the device was last connected, not when it
        // connected: a phone attached all afternoon was seen at its end.
        self.shared
            .mark_seen(&mut state, &self.context.device_id, chrono::Utc::now());
    }
}

/// A socket's open connection request. Dropping it ends the request as
/// withdrawn, unless something ended it first, and destroys a challenge
/// still bound to it: nobody else may redeem that.
pub(super) struct ConnectTicket {
    shared: Arc<Shared>,
    pub(super) id: String,
    pub(super) expires_at: Instant,
    pub(super) outcome: tokio::sync::watch::Receiver<Outcome>,
}

impl Drop for ConnectTicket {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.pairing.withdraw_bound(&self.id);
        state
            .connect
            .finish(Instant::now(), &self.id, RequestEnd::Withdrawn, None);
    }
}

/// A device that passed the handshake, with the frame to tell it so.
pub(super) struct Admitted {
    pub(super) connection: LiveConnection,
    pub(super) welcome: HandshakeResponse,
    pub(super) sink: FrameSink,
    pub(super) queue: FrameQueue,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        // A panic under this lock leaves security state half-updated; refuse
        // to keep serving on top of it.
        self.state.lock().expect("remote gateway state is poisoned")
    }

    /// Reserve a slot for a newly accepted socket, or refuse it.
    pub(super) fn admit_socket(self: &Arc<Self>, ip: IpAddr) -> Option<UnauthenticatedSlot> {
        let ip = ip.to_canonical();
        let mut slots = self
            .unauthenticated
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slots.total >= self.limits.max_unauthenticated
            || slots.per_ip.get(&ip).copied().unwrap_or(0) >= self.limits.max_unauthenticated_per_ip
        {
            return None;
        }
        slots.total += 1;
        *slots.per_ip.entry(ip).or_insert(0) += 1;
        Some(UnauthenticatedSlot {
            shared: Arc::clone(self),
            ip,
        })
    }

    /// Decide a first frame. On success the connection is already in the
    /// live registry, so a revocation that follows cannot miss it.
    pub(super) fn admit_device(
        self: &Arc<Self>,
        text: &str,
    ) -> Result<Admitted, HandshakeResponse> {
        let request = HandshakeRequest::parse(text)?;
        let profile = request.device_profile();
        let rotate = request.wants_token_rotation();
        match request {
            // The transport keeps a socket that asks to connect open
            // ([`Self::open_connect_request`]); it never reaches here.
            HandshakeRequest::ConnectRequest { .. } => Err(HandshakeResponse::error(
                ErrorCode::InvalidFrame,
                "a connection request is only valid as the first frame",
            )),
            HandshakeRequest::Pair {
                method,
                secret,
                device_name,
                install_id,
                ..
            } => self.pair(
                None,
                method,
                secret.expose(),
                &device_name,
                install_id.as_deref(),
                profile,
                rotate,
            ),
            HandshakeRequest::Authenticate {
                device_id,
                token,
                refresh_token,
                access_token,
                ..
            } => {
                let credential = access_token
                    .as_ref()
                    .map(|access| Credential::Access(access.expose()))
                    .or_else(|| {
                        refresh_token
                            .as_ref()
                            .or(token.as_ref())
                            .map(|refresh| Credential::Refresh(refresh.expose()))
                    });
                match credential {
                    Some(credential) => self.authenticate(&device_id, credential),
                    None => Err(HandshakeResponse::error(
                        ErrorCode::InvalidFrame,
                        "authenticate needs a token, refresh_token or access_token",
                    )),
                }
            }
            // An acknowledgement retires a token on an authenticated
            // connection; as a first frame it means nothing.
            HandshakeRequest::TokenAck(_) => Err(HandshakeResponse::error(
                ErrorCode::InvalidFrame,
                "token_ack is only valid after authenticating",
            )),
        }
    }

    /// The second frame of a socket whose connection request was accepted:
    /// a `pair` frame presenting the code bound to that request.
    pub(super) fn admit_connected(
        self: &Arc<Self>,
        request: &str,
        text: &str,
    ) -> Result<Admitted, HandshakeResponse> {
        let parsed = HandshakeRequest::parse(text)?;
        let profile = parsed.device_profile();
        let rotate = parsed.wants_token_rotation();
        let HandshakeRequest::Pair {
            method,
            secret,
            device_name,
            install_id,
            ..
        } = parsed
        else {
            return Err(HandshakeResponse::error(
                ErrorCode::InvalidFrame,
                "after connect_approved the only frame is a pair request with the code",
            ));
        };
        let admitted = self.pair(
            Some(request),
            method,
            secret.expose(),
            &device_name,
            install_id.as_deref(),
            profile,
            rotate,
        );
        let mut state = self.state();
        let now = Instant::now();
        match &admitted {
            Ok(admitted) => {
                let device_id = admitted.connection.context.device_id.clone();
                state
                    .connect
                    .finish(now, request, RequestEnd::Paired, Some(device_id));
            }
            Err(_) => {
                // One code per accepted request: the socket closes, and the
                // challenge it was shown for goes with it.
                state.pairing.withdraw_bound(request);
                state.connect.finish(now, request, RequestEnd::Failed, None);
            }
        }
        admitted
    }

    /// Open a connection request for a socket that sent `connect_request`.
    pub(super) fn open_connect_request(
        self: &Arc<Self>,
        ip: IpAddr,
        device_name: &str,
        profile: DeviceProfile,
        install_id: Option<&str>,
    ) -> Result<ConnectTicket, HandshakeResponse> {
        let now = Instant::now();
        let mut state = self.state();
        self.expire_connect_requests(&mut state, now);
        if let Some(replaced) = state.connect.replaceable(ip, install_id) {
            state.pairing.withdraw_bound(&replaced);
            state
                .connect
                .finish(now, &replaced, RequestEnd::Replaced, None);
        }
        let opened = state
            .connect
            .open(
                now,
                self.limits.connect_request_lifetime,
                ip,
                device_name,
                profile,
                install_id,
            )
            .map_err(|error| match error {
                OpenError::Busy => HandshakeResponse::error(
                    ErrorCode::ConnectBusy,
                    "the host has too many connection requests open; try again shortly",
                ),
                OpenError::RateLimited { retry_after } => HandshakeResponse::Error {
                    code: ErrorCode::ConnectRateLimited,
                    message: "the host denied this device a moment ago".into(),
                    retry_after_secs: Some(retry_after.as_secs().max(1)),
                },
            })?;
        Ok(ConnectTicket {
            shared: Arc::clone(self),
            id: opened.id,
            expires_at: opened.expires_at,
            outcome: opened.outcome,
        })
    }

    /// End a request whose time ran out, unless it was approved meanwhile
    /// and its challenge still runs. True when it ended.
    pub(super) fn expire_connect_request(&self, id: &str) -> bool {
        let now = Instant::now();
        let mut state = self.state();
        let Some(request) = state.connect.get(id) else {
            return true;
        };
        if request.expires_at > now {
            return false;
        }
        state.pairing.withdraw_bound(id);
        state.connect.finish(now, id, RequestEnd::Expired, None);
        true
    }

    fn expire_connect_requests(&self, state: &mut State, now: Instant) {
        for id in state.connect.expired(now) {
            state.pairing.withdraw_bound(&id);
            state.connect.finish(now, &id, RequestEnd::Expired, None);
        }
    }

    /// The owner's answer to a connection request. Accepting replaces any
    /// outstanding challenge, the QR code's or another request's, with one
    /// bound to this request.
    fn answer_connect_request(&self, id: &str, accept: bool) -> Result<ConnectAnswer, String> {
        let now = Instant::now();
        let mut state = self.state();
        self.expire_connect_requests(&mut state, now);
        let Some(request) = state.connect.get(id) else {
            return Err("that connection request is no longer open".into());
        };
        if request.state != RequestState::Pending {
            return Err("that connection request was already answered".into());
        }
        let device_name = request.device_name.clone();
        if !accept {
            state.pairing.withdraw_bound(id);
            state.connect.finish(now, id, RequestEnd::Denied, None);
            return Ok(ConnectAnswer {
                id: id.to_owned(),
                device_name,
                instance_id: self.instance_id.clone(),
                accepted: false,
                code: None,
                expires_in_secs: None,
                locked_for_secs: None,
            });
        }
        if let Some(retry_after) = state.pairing.lockout(now) {
            return Err(format!(
                "pairing is locked for {} seconds after too many failed codes",
                retry_after.as_secs().max(1)
            ));
        }
        replace_approved(&mut state, now);
        let offer = state.pairing.issue_bound(now, Some(id.to_owned()));
        state.connect.approve(now, id, now + offer.lifetime);
        Ok(ConnectAnswer {
            id: id.to_owned(),
            device_name,
            instance_id: self.instance_id.clone(),
            accepted: true,
            code: Some(offer.code),
            expires_in_secs: Some(offer.lifetime.as_secs()),
            locked_for_secs: state.pairing.lockout(now).map(|left| left.as_secs().max(1)),
        })
    }

    /// Redeem the outstanding challenge and enroll the device. `request`
    /// names the connection request whose socket presents it, if any; the
    /// device is then recorded as that request described it, which is what
    /// the owner accepted, rather than as its `pair` frame says. `rotate`
    /// enrolls a device that asked for rotating tokens instead of a static
    /// one.
    #[allow(clippy::too_many_arguments)]
    fn pair(
        self: &Arc<Self>,
        request: Option<&str>,
        method: PairingMethod,
        secret: &str,
        device_name: &str,
        install_id: Option<&str>,
        profile: DeviceProfile,
        rotate: bool,
    ) -> Result<Admitted, HandshakeResponse> {
        let mut state = self.state();
        let profile = request
            .and_then(|id| state.connect.get(id))
            .map_or(profile, |request| request.profile.clone());
        state
            .pairing
            .redeem_as(Instant::now(), request, method, secret)
            .map_err(|error| match error {
                PairingError::Failed => HandshakeResponse::error(
                    ErrorCode::PairingFailed,
                    "pairing failed; request a new pairing code on the host",
                ),
                PairingError::RateLimited { retry_after } => HandshakeResponse::Error {
                    code: ErrorCode::RateLimited,
                    message: "too many failed pairing attempts on this host".into(),
                    retry_after_secs: Some(retry_after.as_secs().max(1)),
                },
            })?;
        // The challenge is spent from here on, whatever happens next.
        if state.live.len() >= self.limits.max_authenticated {
            return Err(busy("the gateway is at its connection limit"));
        }
        let mut devices = state.devices.clone();
        let mut enrolled = if rotate {
            devices.enroll_rotating(device_name, install_id, &profile)
        } else {
            devices.enroll_as(device_name, install_id, &profile)
        }
        .map_err(|error| busy(error.to_string()))?;
        // Only the owner-approved, socket-bound desktop request grants ACP.
        // A standalone short code, regardless of its claimed kind, never does.
        if request.is_some() && profile.kind == super::devices::DeviceKind::Desktop {
            enrolled.record = devices.set_acp(&enrolled.record.id, true).unwrap();
        }
        if let Err(error) = self.persist(&mut state, devices) {
            crate::dbg_log!("remote gateway: device store write failed: {error:#}");
            return Err(HandshakeResponse::error(
                ErrorCode::Internal,
                "the host could not store the new device",
            ));
        }
        let record = enrolled.record;
        let issued = enrolled.issued;
        // The same install paired again: its old token is gone, and so is
        // anything still connected with it.
        if let Some(replaced) = enrolled.replaced {
            close_device(&mut state, &replaced);
        }
        let welcome = HandshakeResponse::Paired {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            gateway_id: self.gateway_id.clone(),
            instance_id: self.instance_id.clone(),
            device_id: record.id.clone(),
            device_name: record.name.clone(),
            token: enrolled.token,
            host_name: self.host_name.clone(),
            expires_at: record.expires_at().map(rfc3339),
            refresh_token: issued.as_ref().map(|issued| issued.refresh_token.clone()),
            access_token: issued.as_ref().map(|issued| issued.access_token.clone()),
            access_expires_at: issued
                .as_ref()
                .map(|issued| rfc3339(issued.access_expires_at)),
            wire_names: None,
            routes: routes_now(self),
        };
        Ok(self.register(&mut state, &record, welcome))
    }

    /// Accept an `authenticate`: a static token, a rotating refresh token or an
    /// access token. A replayed refresh token revokes the whole family, live
    /// connections included, and is reported as such.
    fn authenticate(
        self: &Arc<Self>,
        device_id: &str,
        credential: Credential<'_>,
    ) -> Result<Admitted, HandshakeResponse> {
        let mut state = self.state();
        let now = chrono::Utc::now();
        if state.live.len() >= self.limits.max_authenticated {
            return Err(busy("the gateway is at its connection limit"));
        }
        let device = state
            .devices
            .list()
            .iter()
            .find(|record| record.id == device_id);
        let device_name = device.map(|record| record.name.clone());
        let family_id = device
            .and_then(|record| record.family_id())
            .map(str::to_owned);
        let mut devices = state.devices.clone();
        let accepted = match devices.accept(device_id, credential, now) {
            Ok(accepted) => accepted,
            Err(error @ (AuthenticateError::Replayed | AuthenticateError::Exhausted)) => {
                // The family is gone from `devices`: write that down before
                // anything is closed or answered, close whatever is still live
                // with it, and tell the owner. A removal that could not be
                // written is still refused, and reported as not persisted.
                let persisted = self.revoke_family(&mut state, devices, device_id);
                let closed = close_device(&mut state, device_id);
                report_family_revocation(
                    device_id,
                    device_name.as_deref(),
                    family_id.as_deref(),
                    closed,
                    error,
                    persisted,
                );
                return Err(match error {
                    AuthenticateError::Replayed => HandshakeResponse::error(
                        ErrorCode::TokenReplayed,
                        "a copy of this device's key was used; the host removed it. Pair again if this was you",
                    ),
                    _ => HandshakeResponse::error(
                        ErrorCode::Unauthorized,
                        "this pairing rotated more tokens than the host keeps history for and was removed; pair again",
                    ),
                });
            }
            Err(AuthenticateError::AccessExpired) => {
                return Err(HandshakeResponse::error(
                    ErrorCode::AccessExpired,
                    "the access token is no longer valid; authenticate with the refresh token",
                ));
            }
            Err(AuthenticateError::Unauthorized) => {
                return Err(HandshakeResponse::error(
                    ErrorCode::Unauthorized,
                    "unknown device or token; pair this device again",
                ));
            }
        };
        if accepted.issued.is_some() {
            // A rotation is on disk before its token is handed out, so the
            // device it reaches is never one the host forgot.
            if let Err(error) = self.persist(&mut state, devices) {
                crate::dbg_log!("remote gateway: device store write failed: {error:#}");
                return Err(HandshakeResponse::error(
                    ErrorCode::Internal,
                    "the host could not store the rotated token",
                ));
            }
        } else {
            // An access token reconnected: nothing rotated, but the record it
            // confirmed is the one to keep.
            state.devices = devices;
        }
        let record = accepted.record;
        self.mark_seen(&mut state, &record.id, now);
        let issued = accepted.issued;
        let welcome = HandshakeResponse::Authenticated {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            gateway_id: self.gateway_id.clone(),
            instance_id: self.instance_id.clone(),
            device_id: record.id.clone(),
            device_name: record.name.clone(),
            host_name: self.host_name.clone(),
            expires_at: record.expires_at().map(rfc3339),
            refresh_token: issued.as_ref().map(|issued| issued.refresh_token.clone()),
            access_token: issued.as_ref().map(|issued| issued.access_token.clone()),
            access_expires_at: issued
                .as_ref()
                .map(|issued| issued.access_expires_at)
                .or(accepted.access_expires_at)
                .map(rfc3339),
            rotation_id: issued
                .as_ref()
                .and_then(|issued| issued.rotation_id.clone()),
            wire_names: None,
            routes: routes_now(self),
        };
        Ok(self.register(&mut state, &record, welcome))
    }

    /// The device stored the refresh token from the `authenticated` frame that
    /// named `rotation_id`: the token before it may be retired for good. An
    /// acknowledgement of a rotation another connection replaced changes
    /// nothing, so a late or duplicated ack is harmless.
    pub(super) fn acknowledge_rotation(self: &Arc<Self>, device_id: &str, rotation_id: &str) {
        let mut state = self.state();
        let mut devices = state.devices.clone();
        let record = state
            .devices
            .list()
            .iter()
            .find(|record| record.id == device_id);
        let device_name = record.map(|record| record.name.clone());
        let family_id = record
            .and_then(|record| record.family_id())
            .map(str::to_owned);
        match devices.acknowledge(device_id, rotation_id) {
            Acknowledgement::Ignored => {}
            Acknowledgement::Retired => match self.persist(&mut state, devices) {
                Ok(()) => {}
                Err(error) => {
                    crate::dbg_log!("remote gateway: device store write failed: {error:#}");
                }
            },
            // The history was full: the family is removed rather than left
            // with a token it can no longer recognize.
            Acknowledgement::Exhausted => {
                let persisted = self.revoke_family(&mut state, devices, device_id);
                let closed = close_device(&mut state, device_id);
                report_family_revocation(
                    device_id,
                    device_name.as_deref(),
                    family_id.as_deref(),
                    closed,
                    AuthenticateError::Exhausted,
                    persisted,
                );
            }
        }
    }

    /// Write the device set, then drop the revocation notes a failed write left
    /// behind: with the store on disk it is the truth again.
    fn persist(&self, state: &mut State, devices: Devices) -> Result<()> {
        self.store.save(&devices)?;
        state.devices = devices;
        if let Err(error) = self.notes.clear() {
            crate::dbg_log!("remote gateway: could not clear the revocation notes: {error:#}");
        }
        Ok(())
    }

    /// Remove a family for good, durably before anything is reported. The store is
    /// written first; when that fails, a note next to it records the revocation for
    /// the next start. Either way the family is gone from memory, so it is refused
    /// for the rest of this run; a removal that reached no disk is retried in the
    /// background. Returns whether the removal reached the disk.
    fn revoke_family(
        self: &Arc<Self>,
        state: &mut State,
        devices: Devices,
        device_id: &str,
    ) -> bool {
        let persisted = match self.store.save(&devices) {
            Ok(()) => true,
            Err(error) => {
                crate::dbg_log!(
                    "remote gateway: ERROR: device store write failed while revoking {device_id}: {error:#}"
                );
                match self.notes.add(device_id) {
                    Ok(()) => {
                        crate::dbg_log!(
                            "remote gateway: noted the revocation of {device_id} for the next start"
                        );
                        true
                    }
                    Err(note_error) => {
                        crate::dbg_log!(
                            "remote gateway: ERROR: could not write the revocation note for {device_id} either ({note_error:#}); it is refused in memory only until a retry gets through"
                        );
                        false
                    }
                }
            }
        };
        state.devices = devices;
        if !persisted {
            self.retry_family_revocation(device_id);
        }
        persisted
    }

    /// Keep trying to write a revocation that reached no disk, with exponential
    /// backoff. The family is already refused in memory; this only narrows the
    /// window in which a restart could bring it back. The note goes first, since it
    /// survives a store that cannot be written.
    fn retry_family_revocation(self: &Arc<Self>, device_id: &str) {
        const ATTEMPTS: u32 = 6;
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            crate::dbg_log!(
                "remote gateway: ERROR: no runtime to retry the revocation of {device_id}; a restart could bring it back"
            );
            return;
        };
        let shared = Arc::clone(self);
        let device_id = device_id.to_owned();
        handle.spawn(async move {
        for attempt in 0..ATTEMPTS {
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            let mut state = shared.state();
            let devices = state.devices.clone();
            let noted = shared.notes.add(&device_id);
            match shared.store.save(&devices) {
                Ok(()) => {
                    state.devices = devices;
                    if let Err(error) = shared.notes.clear() {
                        crate::dbg_log!(
                            "remote gateway: could not clear the revocation notes: {error:#}"
                        );
                    }
                    crate::dbg_log!(
                        "remote gateway: wrote the revocation of {device_id} on retry {}",
                        attempt + 1
                    );
                    crate::logger::operational_event(
                        "remote.revocation_written",
                        serde_json::json!({
                            "device_id": device_id,
                            "persisted": true,
                            "attempts": attempt + 1,
                        }),
                    );
                    return;
                }
                Err(error) => {
                    crate::dbg_log!(
                        "remote gateway: ERROR: retry {} of the revocation write for {device_id} failed: {error:#}",
                        attempt + 1
                    );
                    if let Err(note_error) = noted {
                        crate::dbg_log!(
                            "remote gateway: ERROR: the revocation note for {device_id} failed too: {note_error:#}"
                        );
                    }
                }
            }
        }
        crate::dbg_log!(
            "remote gateway: ERROR: gave up writing the revocation of {device_id}; it is refused in memory only and a restart could bring it back"
        );
    });
    }

    /// Record that `device_id` was connected at `now`, within
    /// [`super::devices::LAST_SEEN_RESOLUTION`]. Last seen is for the
    /// owner's device list; failing to store it never keeps a device out.
    fn mark_seen(&self, state: &mut State, device_id: &str, now: chrono::DateTime<chrono::Utc>) {
        let mut devices = state.devices.clone();
        if devices.mark_seen(device_id, now)
            && let Err(error) = self.persist(state, devices)
        {
            crate::dbg_log!("remote gateway: device store write failed: {error:#}");
        }
    }

    fn register(
        self: &Arc<Self>,
        state: &mut State,
        record: &DeviceRecord,
        welcome: HandshakeResponse,
    ) -> Admitted {
        let connection_id = state.next_connection;
        state.next_connection += 1;
        let control = Arc::new(ConnectionControl::new(self.shutdown.child_token()));
        state.live.insert(
            connection_id,
            Live {
                device_id: record.id.clone(),
                device_name: record.name.clone(),
                control: Arc::clone(&control),
            },
        );
        let (sink, queue) = FrameSink::new(self.limits.outbound_queue, Arc::clone(&control));
        Admitted {
            connection: LiveConnection {
                shared: Arc::clone(self),
                context: DeviceContext {
                    connection_id,
                    device_id: record.id.clone(),
                    device_name: record.name.clone(),
                },
                control,
            },
            welcome,
            sink,
            queue,
        }
    }

    /// Drop a device whose token never reached it (the `paired` frame could
    /// not be written), so no unusable entry is left behind.
    pub(super) fn forget_undelivered(&self, device_id: &str) {
        let mut state = self.state();
        let mut devices = state.devices.clone();
        if devices.remove(device_id).is_none() {
            return;
        }
        if let Err(error) = self.persist(&mut state, devices) {
            crate::dbg_log!("remote gateway: device store write failed: {error:#}");
        }
    }

    /// Forget a device and close its live connections in one step. The store
    /// is written first: if that fails nothing changes and the caller is told.
    fn revoke(&self, selector: &str) -> Result<(DeviceRecord, usize)> {
        let mut state = self.state();
        let record = state.devices.select(selector)?.clone();
        let mut devices = state.devices.clone();
        devices.remove(&record.id);
        self.persist(&mut state, devices)?;
        let closed = close_device(&mut state, &record.id);
        Ok((record, closed))
    }

    pub(super) fn acp_allowed(&self, id: &str) -> bool {
        self.state()
            .devices
            .select(id)
            .is_ok_and(|record| record.acp)
    }

    fn set_acp(&self, selector: &str, allow: bool) -> Result<()> {
        let mut state = self.state();
        let mut devices = state.devices.clone();
        let record = devices.set_acp(selector, allow)?;
        self.persist(&mut state, devices)?;
        if !allow {
            close_device(&mut state, &record.id);
        }
        Ok(())
    }

    fn issue_pairing(&self) -> OfferDetails {
        // Read the interfaces before taking the state lock.
        let advertised = self.advertised.borrow().clone();
        let alternates =
            alternate_addresses(*self.bind.borrow(), &advertised, &candidate_addresses());
        let now = Instant::now();
        let mut state = self.state();
        replace_approved(&mut state, now);
        let offer = state.pairing.issue(now);
        let mut details = OfferDetails::with_alternates(
            &advertised,
            &self.gateway_id,
            self.host_name.as_deref(),
            alternates,
            offer,
        );
        details.instance_id = Some(self.instance_id.clone());
        details.challenge = state.pairing.last_serial();
        details.locked_for_secs = state.pairing.lockout(now).map(|left| left.as_secs().max(1));
        details
    }
}

impl Shared {
    /// Follow a changed listen plan. A new bind address gets a new listener
    /// on the same port, returned to replace the old one; connections on the
    /// old address stay until it stops carrying them. Either way pairing
    /// details, status, the registration and the session owners learn the
    /// address a device should dial now.
    async fn move_listener(
        &self,
        listener: &TcpListener,
        current: &ListenPlan,
        next: &ListenPlan,
    ) -> Result<Option<TcpListener>> {
        let port = listener.local_addr()?.port();
        let moved = if next.bind == current.bind {
            None
        } else {
            Some(TcpListener::bind(SocketAddr::new(next.bind, port)).await?)
        };
        let local = match &moved {
            Some(moved) => moved.local_addr()?,
            None => listener.local_addr()?,
        };
        let advertised = next.advertise.clone().with_port(port);
        {
            let mut registration = self
                .registration
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            registration.listen_address = local.to_string();
            registration.advertised_address = advertised.to_string();
            registration.publish(&self.lifecycle.registration_path())?;
        }
        if let Some(sessions) = &self.sessions {
            sessions.set_advertised_address(advertised.to_string(), next.bind.is_loopback());
        }
        println!("address changed: listening on {local}, advertised as {advertised}");
        self.advertised.send_replace(advertised);
        self.bind.send_replace(next.bind);
        Ok(moved)
    }
}

/// Tell the owner that a retired refresh token was presented and its whole
/// family revoked: in the gateway log and as an operational event, next to
/// what `rustcode remote revoke` reports.
fn report_family_revocation(
    device_id: &str,
    device_name: Option<&str>,
    family_id: Option<&str>,
    closed_connections: usize,
    reason: AuthenticateError,
    persisted: bool,
) {
    let name = device_name.unwrap_or("a paired device");
    let (event, why) = match reason {
        AuthenticateError::Exhausted => (
            "remote.pairing_retired",
            "rotated more refresh tokens than the host keeps history for",
        ),
        _ => ("remote.token_replayed", "presented a retired refresh token"),
    };
    let durability = if persisted {
        "the revocation is on disk"
    } else {
        "the revocation could not be written and is not on disk"
    };
    crate::dbg_log!(
        "remote gateway: {name} ({device_id}) {why}; the pairing was revoked ({durability}) and {closed_connections} live connection(s) closed"
    );
    crate::logger::operational_event(
        event,
        serde_json::json!({
            "device_id": device_id,
            "device_name": name,
            "family_id": family_id,
            "revoked": true,
            "persisted": persisted,
            "closed_connections": closed_connections,
        }),
    );
}

/// Close every live connection of `device_id` as revoked; how many there were.
fn close_device(state: &mut State, device_id: &str) -> usize {
    let mut closed = 0;
    state.live.retain(|_, live| {
        if live.device_id != device_id {
            return true;
        }
        live.control.close(ErrorCode::Revoked);
        closed += 1;
        false
    });
    closed
}

/// End the accepted connection request, if there is one, because a new
/// challenge is about to replace the one issued for it. Its socket is told
/// the code expired.
fn replace_approved(state: &mut State, now: Instant) {
    if let Some(id) = state.connect.approved().map(|request| request.id.clone()) {
        state.connect.finish(now, &id, RequestEnd::Replaced, None);
    }
}

/// RFC 3339 in UTC, seconds precision, as every handshake timestamp is sent.
fn rfc3339(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The addresses this host answers on right now, for a handshake and for
/// `remote status`: the advertised one, the machine's LAN and mesh addresses,
/// and its `.local` name. A device keeps them as routes, so it can reconnect
/// over another network after pairing once.
fn routes_now(shared: &Shared) -> Vec<HostRoute> {
    let advertised = shared.advertised.borrow().clone();
    let label = super::discovery::host_label(&shared.gateway_id);
    host_routes(
        &advertised,
        &candidate_addresses(),
        Some(label.trim_end_matches('.')),
    )
}

fn busy(message: impl Into<String>) -> HandshakeResponse {
    HandshakeResponse::error(ErrorCode::Busy, message)
}

/// Removes the control socket and registration when the gateway goes away,
/// while still holding the ownership lock.
struct Ownership {
    lifecycle: RemoteLifecycle,
    registration: Arc<Mutex<GatewayRegistration>>,
    _lock: File,
}

impl Drop for Ownership {
    fn drop(&mut self) {
        // Socket first: stop() waits for the registration to disappear and
        // must not return while the socket is still on disk.
        let _ = remove_if_exists(&self.lifecycle.socket_path());
        let _ = remove_if_exists(&self.lifecycle.owner_socket_path());
        if GatewayRegistration::read(&self.lifecycle.registration_path())
            .ok()
            .as_ref()
            == Some(
                &*self
                    .registration
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            )
        {
            let _ = remove_if_exists(&self.lifecycle.registration_path());
        }
    }
}

pub struct Gateway {
    listener: TcpListener,
    control: LocalListener,
    owners: LocalListener,
    shared: Arc<Shared>,
    plan: ListenPlan,
    replan: Option<Replan>,
    started: Instant,
    _ownership: Ownership,
}

impl Gateway {
    /// Become the gateway for `lifecycle`'s directory: take the ownership
    /// lock, load the paired devices, bind both listeners and publish the
    /// registration. Fails if another gateway is running.
    pub async fn bind(lifecycle: &RemoteLifecycle, config: GatewayConfig) -> Result<Self> {
        let lock = lifecycle.claim()?;
        let gateway_id = lifecycle.gateway_id(&lock)?;
        let store = lifecycle.device_store();
        let notes = lifecycle.revocation_notes();
        // Notes a failed revocation write left must be read and applied before
        // anything is served; an unreadable one stops the gateway instead.
        let devices = apply_revocation_notes(&store, &notes, store.load()?)?;
        let listener = TcpListener::bind(SocketAddr::new(config.plan.bind, config.port)).await?;
        let local = listener.local_addr()?;
        let advertised = config.plan.advertise.clone().with_port(local.port());
        let registration = GatewayRegistration::current(
            lifecycle.socket_path(),
            lifecycle.owner_socket_path(),
            gateway_id.clone(),
            local.to_string(),
            advertised.to_string(),
        )?;
        let control = LocalListener::bind(&lifecycle.socket_path()).with_context(|| {
            format!(
                "failed to bind control socket {}",
                lifecycle.socket_path().display()
            )
        })?;
        let instance_id = registration.instance_id.clone();
        let registration = Arc::new(Mutex::new(registration));
        let ownership = Ownership {
            lifecycle: lifecycle.clone(),
            registration: Arc::clone(&registration),
            _lock: lock,
        };
        let owners = LocalListener::bind(&lifecycle.owner_socket_path()).with_context(|| {
            format!(
                "failed to bind owner socket {}",
                lifecycle.owner_socket_path().display()
            )
        })?;
        if let Some(sessions) = &config.sessions {
            sessions.advertise_capabilities(&[
                host_capability::CONNECT_REQUESTS,
                host_capability::DESKTOPS,
                host_capability::TOKEN_ROTATION,
            ]);
            sessions.bind_identity(HubIdentity {
                gateway_id: gateway_id.clone(),
                instance_id: instance_id.clone(),
                advertised_address: advertised.to_string(),
                loopback_only: config.plan.bind.is_loopback(),
            });
        }
        registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .publish(&lifecycle.registration_path())?;
        let shared = Arc::new(Shared {
            gateway_id,
            instance_id,
            advertised: tokio::sync::watch::Sender::new(advertised),
            bind: tokio::sync::watch::Sender::new(config.plan.bind),
            registration,
            lifecycle: lifecycle.clone(),
            limits: config.limits,
            router: config.router,
            sessions: config.sessions,
            host_name: host_name(),
            automatic_bind: config.plan.automatic_bind,
            fixed_advertise: config.plan.fixed_advertise,
            shutdown: CancellationToken::new(),
            store,
            notes,
            state: Mutex::new(State {
                pairing: PairingState::default(),
                connect: ConnectRequests::default(),
                devices,
                live: HashMap::new(),
                next_connection: 1,
            }),
            unauthenticated: Mutex::new(Unauthenticated::default()),
        });
        Ok(Self {
            listener,
            control,
            owners,
            shared,
            plan: config.plan,
            replan: config.replan,
            started: Instant::now(),
            _ownership: ownership,
        })
    }

    /// The bound WebSocket listener address.
    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound listener has an address")
    }

    /// The address pairing details tell a device to dial.
    pub fn advertised_address(&self) -> AdvertisedAddress {
        self.shared.advertised.borrow().clone()
    }

    /// Follows the advertised address as the listener moves.
    pub fn advertised_changes(&self) -> tokio::sync::watch::Receiver<AdvertisedAddress> {
        self.shared.advertised.subscribe()
    }

    /// Follows the listener when it moves between network interfaces.
    pub fn listen_address_changes(&self) -> tokio::sync::watch::Receiver<IpAddr> {
        self.shared.bind.subscribe()
    }

    pub fn gateway_id(&self) -> &str {
        &self.shared.gateway_id
    }

    /// Cancel this to stop [`Gateway::run`] and close every connection.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shared.shutdown.clone()
    }

    /// Serve until shut down. Dropping the future also closes every
    /// connection and removes the socket and registration.
    pub async fn run(self) -> Result<()> {
        let Gateway {
            mut listener,
            control,
            owners,
            shared,
            mut plan,
            replan,
            started,
            _ownership,
        } = self;
        let mut tasks = JoinSet::new();
        let mut replan_due = tokio::time::interval(shared.limits.replan_interval);
        replan_due.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shared.shutdown.cancelled() => break,
                _ = replan_due.tick(), if replan.is_some() => {
                    let next = replan.as_ref().and_then(|replan| replan());
                    if let Some(next) = next.filter(|next| *next != plan) {
                        match shared.move_listener(&listener, &plan, &next).await {
                            Ok(Some(moved)) => {
                                listener = moved;
                                plan = next;
                            }
                            Ok(None) => plan = next,
                            // Usually the new address is not assignable yet;
                            // the next round tries again.
                            Err(error) => {
                                crate::dbg_log!("remote gateway: could not move to {}: {error:#}", next.bind);
                            }
                        }
                    }
                }
                accepted = listener.accept() => match accepted {
                    Ok((socket, peer)) => {
                        // Over the limit: drop the socket without reading a byte.
                        if let Some(slot) = shared.admit_socket(peer.ip()) {
                            tasks.spawn(transport::serve_connection(
                                Arc::clone(&shared),
                                socket,
                                slot,
                            ));
                        }
                    }
                    Err(error) => {
                        // Typically descriptor exhaustion; do not spin on it.
                        crate::dbg_log!("remote gateway: accept failed: {error}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                },
                accepted = control.accept() => {
                    if let Ok(incoming) = accepted {
                        let shared = Arc::clone(&shared);
                        tasks.spawn(async move {
                            if let Some(stream) = incoming.verify().await {
                                serve_control(shared, started, stream).await;
                            }
                        });
                    }
                }
                accepted = owners.accept() => {
                    // Without a session registry there is nobody to register
                    // with: the connection is dropped.
                    if let (Ok(incoming), Some(sessions)) = (accepted, &shared.sessions) {
                        let sessions = Arc::clone(sessions);
                        let shutdown = shared.shutdown.clone();
                        tasks.spawn(async move {
                            if let Some(stream) = incoming.verify().await {
                                owner_ipc::serve_owner(sessions, stream, shutdown).await;
                            }
                        });
                    }
                }
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
        // Every connection watches a child of the shutdown token; give each a
        // moment to send its goodbye, then drop whatever is left.
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while tasks.join_next().await.is_some() {}
        })
        .await;
        tasks.shutdown().await;
        Ok(())
    }
}

async fn serve_control(shared: Arc<Shared>, started: Instant, stream: LocalStream) {
    let exchange = async {
        let (reader, mut writer) = stream.into_split();
        let request: ControlRequest = read_async_frame(&mut BufReader::new(reader)).await?;
        let mut stop = false;
        let response = match request {
            ControlRequest::Status => ControlResponse::Status {
                status: status(&shared, started),
            },
            ControlRequest::Pair => ControlResponse::Offer {
                offer: shared.issue_pairing(),
            },
            ControlRequest::SetAcp { device, allow } => match shared.set_acp(&device, allow) {
                Ok(()) => ControlResponse::Ack,
                Err(error) => ControlResponse::Error {
                    message: format!("{error:#}"),
                },
            },
            ControlRequest::Revoke { device } => match shared.revoke(&device) {
                Ok((record, closed_connections)) => ControlResponse::Revoked {
                    device_id: record.id,
                    device_name: record.name,
                    closed_connections,
                },
                Err(error) => ControlResponse::Error {
                    message: match error.downcast_ref::<DeviceError>() {
                        Some(error) => error.to_string(),
                        None => format!("{error:#}"),
                    },
                },
            },
            ControlRequest::AnswerConnectRequest { id, accept } => {
                match shared.answer_connect_request(&id, accept) {
                    Ok(answer) => ControlResponse::ConnectAnswered { answer },
                    Err(message) => ControlResponse::Error { message },
                }
            }
            ControlRequest::Shutdown { instance_id } => {
                if instance_id == shared.instance_id {
                    stop = true;
                    ControlResponse::Ack
                } else {
                    ControlResponse::Error {
                        message: "remote gateway instance changed".into(),
                    }
                }
            }
        };
        write_async_frame(&mut writer, &response).await?;
        anyhow::Ok(stop)
    };
    if let Ok(Ok(true)) = tokio::time::timeout(CONTROL_TIMEOUT, exchange).await {
        shared.shutdown.cancel();
    }
}

fn status(shared: &Shared, started: Instant) -> GatewayStatus {
    let registration = shared
        .registration
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let sessions = shared.sessions.as_ref().map(|sessions| sessions.summary());
    // Owners hold their lease for as long as they live; one that is not in
    // the session list is alive without being shared.
    let unregistered_owners = super::workspace::live_owner_sessions(&shared.lifecycle)
        .into_iter()
        .filter(|id| {
            sessions
                .as_ref()
                .is_none_or(|sessions| sessions.iter().all(|session| session.session_id != *id))
        })
        .collect();
    let now = Instant::now();
    let mut state = shared.state();
    shared.expire_connect_requests(&mut state, now);
    let connect_requests = state
        .connect
        .list()
        .iter()
        .map(|request| ConnectRequestSummary {
            id: request.id.clone(),
            device_name: request.device_name.clone(),
            device_kind: request.profile.kind,
            platform: request.profile.platform.clone(),
            remote_addr: request.remote_addr.to_string(),
            age_secs: now.saturating_duration_since(request.created).as_secs(),
            expires_in_secs: request.expires_at.saturating_duration_since(now).as_secs(),
            state: request.state,
        })
        .collect();
    let ended_connect_requests = state
        .connect
        .ended()
        .map(|ended| EndedConnectRequest {
            id: ended.id.clone(),
            device_name: ended.device_name.clone(),
            end: ended.end,
            device_id: ended.device_id.clone(),
            ended_secs_ago: now.saturating_duration_since(ended.at).as_secs(),
        })
        .collect();
    let mut connections: Vec<(u64, ConnectionSummary)> = state
        .live
        .iter()
        .map(|(id, live)| {
            (
                *id,
                ConnectionSummary {
                    device_id: live.device_id.clone(),
                    device_name: live.device_name.clone(),
                },
            )
        })
        .collect();
    connections.sort_by_key(|(id, _)| *id);
    let (expires_in, attempts_left) = state.pairing.open_challenge(now).unzip();
    GatewayStatus {
        version: registration.version.clone(),
        pid: registration.pid,
        process_start_time: registration.process_start_time,
        instance_id: registration.instance_id.clone(),
        gateway_id: registration.gateway_id.clone(),
        uptime_seconds: started.elapsed().as_secs(),
        listen_address: registration.listen_address.clone(),
        advertised_address: registration.advertised_address.clone(),
        paired_devices: state.devices.list().len(),
        connections: connections.into_iter().map(|(_, c)| c).collect(),
        pairing_open: state.pairing.is_open(now),
        loopback_only: shared.advertised.borrow().host().is_loopback(),
        sessions,
        candidates: candidate_addresses()
            .into_iter()
            .map(|Candidate { interface, address }| format!("{address}  ({interface})"))
            .collect(),
        routes: routes_now(shared),
        devices: state
            .devices
            .list()
            .iter()
            .map(DeviceSummary::from)
            .collect(),
        pairing: Some(PairingStatus {
            expires_in_secs: expires_in.map(|left| left.as_secs()),
            attempts_left,
            locked_for_secs: state.pairing.lockout(now).map(|left| left.as_secs().max(1)),
            challenge: state.pairing.open_serial(now),
            last_challenge: state.pairing.last_serial(),
            ended: state
                .pairing
                .last_ended()
                .map(|(challenge, end)| EndedChallenge { challenge, end }),
        }),
        unregistered_owners,
        log_path: Some(shared.lifecycle.log_path()),
        owner_log_directory: Some(shared.lifecycle.owners_directory()),
        connect_requests,
        ended_connect_requests,
        automatic_bind: Some(shared.automatic_bind),
        fixed_advertise: Some(shared.fixed_advertise),
    }
}

// End-to-end through Unix sockets and Unix permissions.
#[cfg(all(test, unix))]
mod tests;
