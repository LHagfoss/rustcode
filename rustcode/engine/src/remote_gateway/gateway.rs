//! The gateway process: listeners, limits and the one state lock.
//!
//! Pairing state, the device set and the live-connection registry sit behind
//! a single mutex. Every security decision is one critical section:
//! redeeming a challenge and enrolling its device, checking a token and
//! registering the connection, forgetting a device and closing its
//! connections. Nothing awaits while the lock is held.

use super::address::{AdvertisedAddress, ListenPlan};
use super::control::{
    CONTROL_TIMEOUT, ConnectionSummary, ControlRequest, ControlResponse, GatewayStatus,
    OfferDetails,
};
use super::devices::{DeviceError, DeviceRecord, DeviceStore, Devices};
use super::handshake::{
    ErrorCode, HANDSHAKE_PROTOCOL_VERSION, HandshakeRequest, HandshakeResponse, PairingMethod,
};
use super::hub::{HubIdentity, SessionHub};
use super::lifecycle::{GatewayRegistration, RemoteLifecycle};
use super::owner_ipc;
use super::pairing::{PairingError, PairingState};
use super::router::{ConnectionControl, DeviceContext, FrameQueue, FrameRouter, FrameSink};
use super::transport;
use crate::daemon::lifecycle::remove_if_exists;
use crate::daemon::protocol::{read_async_frame, write_async_frame};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::{self, File};
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::BufReader;
use tokio::net::{TcpListener, UnixListener, UnixStream};
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
        }
    }
}

pub struct GatewayConfig {
    pub plan: ListenPlan,
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
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length; the last byte stays zero.
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) };
    if status != 0 {
        return None;
    }
    let length = buffer.iter().position(|byte| *byte == 0)?;
    let name = String::from_utf8_lossy(&buffer[..length]);
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
    pub(super) advertised: AdvertisedAddress,
    pub(super) limits: Limits,
    pub(super) router: Arc<dyn FrameRouter>,
    sessions: Option<Arc<SessionHub>>,
    host_name: Option<String>,
    pub(super) shutdown: CancellationToken,
    store: DeviceStore,
    state: Mutex<State>,
    unauthenticated: Mutex<Unauthenticated>,
}

/// One of the bounded slots for sockets that have not authenticated.
pub(super) struct UnauthenticatedSlot {
    shared: Arc<Shared>,
    ip: IpAddr,
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
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .live
            .remove(&self.context.connection_id);
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
        match HandshakeRequest::parse(text)? {
            HandshakeRequest::Pair {
                method,
                secret,
                device_name,
                ..
            } => self.pair(method, secret.expose(), &device_name),
            HandshakeRequest::Authenticate {
                device_id, token, ..
            } => self.authenticate(&device_id, token.expose()),
        }
    }

    fn pair(
        self: &Arc<Self>,
        method: PairingMethod,
        secret: &str,
        device_name: &str,
    ) -> Result<Admitted, HandshakeResponse> {
        let mut state = self.state();
        state
            .pairing
            .redeem(Instant::now(), method, secret)
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
        let (record, token) = devices
            .enroll(device_name)
            .map_err(|error| busy(error.to_string()))?;
        if let Err(error) = self.store.save(&devices) {
            crate::dbg_log!("remote gateway: device store write failed: {error:#}");
            return Err(HandshakeResponse::error(
                ErrorCode::Internal,
                "the host could not store the new device",
            ));
        }
        state.devices = devices;
        let welcome = HandshakeResponse::Paired {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            gateway_id: self.gateway_id.clone(),
            instance_id: self.instance_id.clone(),
            device_id: record.id.clone(),
            device_name: record.name.clone(),
            token,
            host_name: self.host_name.clone(),
        };
        Ok(self.register(&mut state, &record, welcome))
    }

    fn authenticate(
        self: &Arc<Self>,
        device_id: &str,
        token: &str,
    ) -> Result<Admitted, HandshakeResponse> {
        let mut state = self.state();
        let Some(record) = state.devices.verify(device_id, token).cloned() else {
            return Err(HandshakeResponse::error(
                ErrorCode::Unauthorized,
                "unknown device or token; pair this device again",
            ));
        };
        if state.live.len() >= self.limits.max_authenticated {
            return Err(busy("the gateway is at its connection limit"));
        }
        let welcome = HandshakeResponse::Authenticated {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            gateway_id: self.gateway_id.clone(),
            instance_id: self.instance_id.clone(),
            device_id: record.id.clone(),
            device_name: record.name.clone(),
            host_name: self.host_name.clone(),
        };
        Ok(self.register(&mut state, &record, welcome))
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
        match self.store.save(&devices) {
            Ok(()) => state.devices = devices,
            Err(error) => {
                crate::dbg_log!("remote gateway: device store write failed: {error:#}");
            }
        }
    }

    /// Forget a device and close its live connections in one step. The store
    /// is written first: if that fails nothing changes and the caller is told.
    fn revoke(&self, selector: &str) -> Result<(DeviceRecord, usize)> {
        let mut state = self.state();
        let record = state.devices.select(selector)?.clone();
        let mut devices = state.devices.clone();
        devices.remove(&record.id);
        self.store.save(&devices)?;
        state.devices = devices;
        let mut closed = 0;
        state.live.retain(|_, live| {
            if live.device_id != record.id {
                return true;
            }
            live.control.close(ErrorCode::Revoked);
            closed += 1;
            false
        });
        Ok((record, closed))
    }

    fn issue_pairing(&self) -> OfferDetails {
        let offer = self.state().pairing.issue(Instant::now());
        OfferDetails::new(
            &self.advertised,
            &self.gateway_id,
            self.host_name.as_deref(),
            offer,
        )
    }
}

fn busy(message: impl Into<String>) -> HandshakeResponse {
    HandshakeResponse::error(ErrorCode::Busy, message)
}

/// Removes the control socket and registration when the gateway goes away,
/// while still holding the ownership lock.
struct Ownership {
    lifecycle: RemoteLifecycle,
    registration: GatewayRegistration,
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
            == Some(&self.registration)
        {
            let _ = remove_if_exists(&self.lifecycle.registration_path());
        }
    }
}

pub struct Gateway {
    listener: TcpListener,
    control: UnixListener,
    owners: UnixListener,
    shared: Arc<Shared>,
    registration: GatewayRegistration,
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
        let devices = store.load()?;
        let listener = TcpListener::bind(SocketAddr::new(config.plan.bind, config.port)).await?;
        let local = listener.local_addr()?;
        let advertised = config.plan.advertise.with_port(local.port());
        let registration = GatewayRegistration::current(
            lifecycle.socket_path(),
            lifecycle.owner_socket_path(),
            gateway_id.clone(),
            local.to_string(),
            advertised.to_string(),
        )?;
        let control = UnixListener::bind(lifecycle.socket_path()).with_context(|| {
            format!(
                "failed to bind control socket {}",
                lifecycle.socket_path().display()
            )
        })?;
        let ownership = Ownership {
            lifecycle: lifecycle.clone(),
            registration: registration.clone(),
            _lock: lock,
        };
        fs::set_permissions(lifecycle.socket_path(), fs::Permissions::from_mode(0o600))?;
        let owners = UnixListener::bind(lifecycle.owner_socket_path()).with_context(|| {
            format!(
                "failed to bind owner socket {}",
                lifecycle.owner_socket_path().display()
            )
        })?;
        fs::set_permissions(
            lifecycle.owner_socket_path(),
            fs::Permissions::from_mode(0o600),
        )?;
        if let Some(sessions) = &config.sessions {
            sessions.bind_identity(HubIdentity {
                gateway_id: gateway_id.clone(),
                instance_id: registration.instance_id.clone(),
                advertised_address: advertised.to_string(),
                loopback_only: config.plan.bind.is_loopback(),
            });
        }
        registration.publish(&lifecycle.registration_path())?;
        let shared = Arc::new(Shared {
            gateway_id,
            instance_id: registration.instance_id.clone(),
            advertised,
            limits: config.limits,
            router: config.router,
            sessions: config.sessions,
            host_name: host_name(),
            shutdown: CancellationToken::new(),
            store,
            state: Mutex::new(State {
                pairing: PairingState::default(),
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
            registration,
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
    pub fn advertised_address(&self) -> &AdvertisedAddress {
        &self.shared.advertised
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
            listener,
            control,
            owners,
            shared,
            registration,
            started,
            _ownership,
        } = self;
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                _ = shared.shutdown.cancelled() => break,
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
                    if let Ok((stream, _)) = accepted {
                        tasks.spawn(serve_control(
                            Arc::clone(&shared),
                            registration.clone(),
                            started,
                            stream,
                        ));
                    }
                }
                accepted = owners.accept() => {
                    // Without a session registry there is nobody to register
                    // with: the connection is dropped.
                    if let (Ok((stream, _)), Some(sessions)) = (accepted, &shared.sessions) {
                        tasks.spawn(owner_ipc::serve_owner(
                            Arc::clone(sessions),
                            stream,
                            shared.shutdown.clone(),
                        ));
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

async fn serve_control(
    shared: Arc<Shared>,
    registration: GatewayRegistration,
    started: Instant,
    stream: UnixStream,
) {
    let exchange = async {
        let (reader, mut writer) = stream.into_split();
        let request: ControlRequest = read_async_frame(&mut BufReader::new(reader)).await?;
        let mut stop = false;
        let response = match request {
            ControlRequest::Status => ControlResponse::Status {
                status: status(&shared, &registration, started),
            },
            ControlRequest::Pair => ControlResponse::Offer {
                offer: shared.issue_pairing(),
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
            ControlRequest::Shutdown { instance_id } => {
                if instance_id == registration.instance_id {
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

fn status(shared: &Shared, registration: &GatewayRegistration, started: Instant) -> GatewayStatus {
    let state = shared.state();
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
        pairing_open: state.pairing.is_open(Instant::now()),
        loopback_only: shared.advertised.host().is_loopback(),
        sessions: shared.sessions.as_ref().map(|sessions| sessions.summary()),
    }
}

#[cfg(test)]
mod tests;
