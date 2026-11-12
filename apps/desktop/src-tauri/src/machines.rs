//! Settings › Machines: the other computers this app is paired with.
//!
//! The webview cannot dial a machine itself (the gateway refuses browser
//! `Origin`s, and the pairing token is in the operating system credential
//! store), so this module owns the connection to each saved machine: one
//! supervisor per machine that connects over the first route that answers,
//! retries with jittered backoff for as long as the machine is saved — the
//! route that worked last first, and at once when this computer's network
//! changes, it wakes from sleep, the app comes to the front, a probe finds a
//! route that answers, or an agent asks (`machines_retry`) — and relays the machine's ACP
//! frames to the webview over a Tauri Channel in the same shape the local
//! engine uses
//! (`engine.rs`). `rustcode-client` holds the registry, the tokens, the
//! pairing flow and the WebSocket; nothing here hands a token to the webview.
//!
//! Names follow docs/terminology.md: a **machine** is a computer, Settings ›
//! Machines lists them, and the server is what runs on each one.

use crate::engine::EngineEvent;
use anyhow::Error;
use rustcode_client::machine::{machine_from_pairing, normalize_code, Connector};
use rustcode_client::tokens::{self as token_store, STORE_TIMEOUT};
use rustcode_client::{
    backoff_delay, browse_async, Approved, Cause, ClientKind, DiscoveredMachine, Machine,
    MachineRegistry, OverlayProvider, ProbeOutcome, Route, RouteProbe, TokenStore, Unreachable,
};
use serde::Serialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;
use tokio::sync::Notify;

/// How long Bonjour listens when Settings › Machines asks for the machines on
/// this network.
pub const DISCOVERY: Duration = Duration::from_secs(4);

/// A connection that stayed up this long resets the backoff: a machine that
/// worked and then dropped is tried again after a second, not after a minute.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);

/// How often a machine that is waiting looks at this computer's own addresses,
/// so a network change (a VPN up, another Wi-Fi) is acted on in seconds instead
/// of at the end of the backoff.
pub const NETWORK_POLL: Duration = Duration::from_secs(5);

/// Where a machine stands, as the page shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MachineState {
    /// A connection is being made now.
    Connecting,
    Connected,
    /// Not reachable; the supervisor tries again after `retryInSecs`.
    Offline,
    /// A route answered, but the machine would not serve this computer: its
    /// server is not running, it speaks another protocol version, it does not
    /// allow this device. `reason` says which; the supervisor keeps trying,
    /// since most of these end on the machine's side.
    Unavailable,
    /// The machine refused this computer's pairing: it has to be paired again.
    NeedsPairing,
    /// Not connected and not trying: the app has not asked yet, or was told to stop.
    Stopped,
}

/// One way to reach a machine, as the page shows it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteView {
    /// `lan`, `tailscale`, `vpn` or `public`.
    kind: &'static str,
    address: String,
    /// `netbird`, `tailscale` or `zerotier` when the machine named the overlay
    /// the address is on; the app labels the route with it.
    provider: Option<&'static str>,
    /// The network itself encrypts the traffic (Tailscale, a VPN); a plain LAN
    /// route does not.
    encrypted: bool,
    /// What the last test of this route found, or none if it was never tested.
    probe: Option<ProbeView>,
}

impl RouteView {
    fn of(route: &Route, machine: &Machine) -> Self {
        Self {
            kind: route.kind.as_str(),
            address: route.address.clone(),
            provider: route.provider.map(OverlayProvider::as_str),
            encrypted: route.kind.is_encrypted(),
            probe: machine.probe_for(&route.address).map(ProbeView::of),
        }
    }
}

/// The last test of one route, as the page shows it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeView {
    /// `reachable`, `refused`, `timeout` or `unreachable`.
    outcome: &'static str,
    /// Milliseconds to connect, or to be turned away; none for a timeout.
    latency_ms: Option<u64>,
    detail: Option<String>,
    /// RFC 3339.
    at: String,
}

impl ProbeView {
    fn of(probe: &RouteProbe) -> Self {
        Self {
            outcome: probe.outcome.as_str(),
            latency_ms: probe.latency_ms,
            detail: probe.detail.clone(),
            at: probe.at.to_rfc3339(),
        }
    }
}

/// A saved machine and its live state.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineView {
    /// The machine's `host_id`, which is what every command takes.
    id: String,
    /// New threads are not started here from this app.
    paused: bool,
    /// The name the machine reports.
    name: String,
    /// The name this app shows for it, or none while the reported one is shown.
    display_name: Option<String>,
    /// The glyph this app draws, or none for the platform's own.
    icon: Option<String>,
    /// The series tint of that glyph.
    icon_tint: Option<String>,
    /// `macos`, `windows`, `linux`, or `unknown` for a machine that does not
    /// report one yet.
    platform: String,
    /// The RustCode version it reported when it last answered, or none from a
    /// machine that has not said. The row compares it with this app's version.
    version: Option<String>,
    /// Every route saved for it, in the order they are tried.
    routes: Vec<RouteView>,
    /// The route this connection came up over, or the one it will try first.
    route: Option<RouteView>,
    state: MachineState,
    /// Why it is not connected, as a word to branch on (`unreachable`,
    /// `rejected_token`, `version_mismatch`, `not_allowed`,
    /// `server_not_running`, `busy`, `rejected`, `dropped`), or none while it
    /// is connected or has not tried yet.
    reason: Option<&'static str>,
    /// Why it is not connected, in words a person can act on.
    detail: Option<String>,
    /// What to check when it is reachable one way and not another, e.g. the
    /// overlay route does not answer while the LAN does.
    hint: Option<String>,
    /// Seconds until the next attempt, while it waits.
    retry_in_secs: Option<u64>,
    /// RFC 3339, as the registry stores it.
    added_at: String,
    last_connected_at: Option<String>,
}

/// What `machines_discover` answers: the rows the Add sheet lists, and what to
/// say when the local search failed (the overlay rows are there either way).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub rows: Vec<DiscoveredView>,
    pub notice: Option<String>,
}

/// A machine the Add sheet lists without an address typed: one Bonjour found
/// on this network, or one whose handshake reported an overlay address this
/// app saved for it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredView {
    name: String,
    /// `bonjour` for a machine announcing itself here, `overlay` for a saved
    /// machine reached at an address its own handshake reported.
    source: &'static str,
    /// The address to dial: a LAN one when there is one.
    address: Option<String>,
    addresses: Vec<String>,
    gateway_id: Option<String>,
    /// `macos`, `windows`, `linux`, or none for a machine that has not said.
    platform: Option<String>,
    /// The RustCode version it reported, when it is a saved machine that has
    /// answered: an older one is what the row says to update.
    version: Option<String>,
    /// `lan`, `netbird`, `tailscale`, `zerotier`, `vpn` or `public`.
    route_kind: String,
    /// What to check when one route answers and another does not (#3402).
    hint: Option<String>,
    /// Whether this app already has it.
    saved: bool,
}

/// What the app receives while it asks a machine to connect.
#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PairingEvent {
    /// The request is in front of the owner of that machine. `askId` is the
    /// page's own name for it, which `machines_cancel` and `machines_pair` take.
    Waiting { ask_id: String, host: String },
    /// The owner accepted; the code the machine now shows is what pairs it.
    Approved {
        ask_id: String,
        code_length: u32,
        expires_in_secs: u64,
    },
    /// The owner denied the request.
    Refused { message: String },
}

/// An accepted connection request waiting for the code.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskView {
    ask_id: String,
    code_length: u32,
    expires_in_secs: u64,
}

/// An accepted request and what the page asked for, kept until the code
/// arrives. The socket inside `approved` is the connection the code travels
/// on, so it must stay open between the two commands.
struct PendingApproval {
    approved: Approved,
    address: String,
    name: Option<String>,
}

/// One machine's supervisor.
struct Live {
    /// Sends one JSON-RPC line to the machine. Kept across reconnects, so a
    /// line the app writes while the machine is being reached is not lost.
    outbox: Option<tokio::sync::mpsc::Sender<String>>,
    /// Aborted when the app asks for the machine to be connected again or
    /// disconnected; dropping it closes the socket.
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    state: MachineState,
    route: Option<Route>,
    /// The route that answered in the last attempt that failed: a machine that
    /// refused, or whose server is gone, is shown where it was reached rather
    /// than on the first saved route.
    answered: Option<Route>,
    /// Why the last attempt failed.
    cause: Option<Cause>,
    detail: Option<String>,
    /// What to check when one kind of route answers and another does not,
    /// e.g. a NetBird address blocked while the LAN works (#3202).
    hint: Option<String>,
    retry_in_secs: Option<u64>,
    /// Ends the supervisor's backoff early: the app came to the front, a probe
    /// found a route that answers, or an agent asked. Kept across restarts.
    wake: Arc<Notify>,
}

impl Live {
    fn new() -> Self {
        Self {
            outbox: None,
            task: None,
            state: MachineState::Stopped,
            route: None,
            answered: None,
            cause: None,
            detail: None,
            hint: None,
            retry_in_secs: None,
            wake: Arc::new(Notify::new()),
        }
    }

    /// Whether the supervisor is waiting out a backoff, which a wake ends.
    fn waiting(&self) -> bool {
        self.task.is_some()
            && matches!(
                self.state,
                MachineState::Offline | MachineState::Unavailable
            )
    }
}

/// The app's machines: the supervisors it runs and the connection requests
/// whose code the user is about to type.
#[derive(Default)]
pub struct Machines {
    live: Mutex<HashMap<String, Live>>,
    approved: Mutex<HashMap<String, PendingApproval>>,
    /// The asks in flight, by the page's own id for them. Cancelling one wakes
    /// the command waiting for the owner's answer, which drops its socket.
    asks: Mutex<HashMap<String, Arc<Notify>>>,
    /// The token store, opened once: reaching the credential store on every
    /// retry would prompt for it again and again.
    tokens: Mutex<Option<Arc<TokenStore>>>,
}

/// The configuration directory the engine and `rustcode machines` use, so the
/// app and the command line see one set of saved machines.
fn config_dir() -> Result<PathBuf, String> {
    rustcode_client::config::config_dir().map_err(chain)
}

/// An error with its context chain, for a message the page shows.
fn chain(error: anyhow::Error) -> String {
    format!("{error:#}")
}

impl Machines {
    /// The machine token store, opened once. Every call that reaches it blocks on
    /// the operating system's IPC (a Secret Service prompt, a keychain unlock),
    /// so it runs off the runtime with a deadline: a store that never answers is
    /// a message to show, not a command that never returns.
    async fn tokens(self: &Arc<Self>, config_dir: &Path) -> Result<Arc<TokenStore>, String> {
        if let Some(tokens) = self.tokens.lock().unwrap().as_ref() {
            return Ok(Arc::clone(tokens));
        }
        let tokens = Arc::new(
            rustcode_client::tokens::open_bounded(config_dir, STORE_TIMEOUT)
                .await
                .map_err(chain)?,
        );
        *self.tokens.lock().unwrap() = Some(Arc::clone(&tokens));
        Ok(tokens)
    }

    fn connector(config_dir: &Path) -> Result<Connector, String> {
        Connector::new(config_dir, ClientKind::Desktop).map_err(chain)
    }

    /// The saved machines, oldest first, each with its live state.
    fn list(&self) -> Result<Vec<MachineView>, String> {
        let config_dir = config_dir()?;
        let registry = MachineRegistry::load(&config_dir).map_err(chain)?;
        let live = self.live.lock().unwrap();
        Ok(registry
            .ordered()
            .into_iter()
            .map(|machine| view(machine, live.get(&machine.host_id)))
            .collect())
    }

    /// Wake the supervisors waiting out a backoff: one machine's, or every
    /// machine's when `id` is none. Answers the ids that were woken; a machine
    /// that is connected, connecting, stopped or needs pairing is left alone.
    fn retry(&self, id: Option<&str>) -> Vec<String> {
        let live = self.live.lock().unwrap();
        live.iter()
            .filter(|(key, entry)| id.is_none_or(|id| id == key.as_str()) && entry.waiting())
            .map(|(key, entry)| {
                entry.wake.notify_one();
                key.clone()
            })
            .collect()
    }

    /// Start (or restart) one machine's supervisor. The webview calls this
    /// with a channel of its own, so a page that reloads gets a fresh one.
    fn connect(self: &Arc<Self>, id: &str, channel: Channel<EngineEvent>) -> Result<(), String> {
        let config_dir = config_dir()?;
        let registry = MachineRegistry::load(&config_dir).map_err(chain)?;
        let machine = registry
            .get(id)
            .ok_or("That machine is no longer saved.")?
            .clone();
        let (outbox, inbox) = tokio::sync::mpsc::channel::<String>(256);
        let wake = {
            let mut live = self.live.lock().unwrap();
            let entry = live.entry(id.to_owned()).or_insert_with(Live::new);
            if let Some(task) = entry.task.take() {
                task.abort();
            }
            entry.outbox = Some(outbox);
            entry.state = MachineState::Connecting;
            entry.route = None;
            entry.answered = None;
            entry.cause = None;
            entry.detail = None;
            entry.hint = None;
            entry.retry_in_secs = None;
            // A fresh wake: a permit left by a wake nobody waited for must not
            // cut the new supervisor's first backoff short.
            entry.wake = Arc::new(Notify::new());
            Arc::clone(&entry.wake)
        };
        let machines = Arc::clone(self);
        let target = id.to_owned();
        let task = tauri::async_runtime::spawn({
            let target = target.clone();
            async move {
                supervise(machines, target, machine, channel, inbox, wake).await;
            }
        });
        if let Some(entry) = self.live.lock().unwrap().get_mut(&target) {
            entry.task = Some(task);
        }
        Ok(())
    }
}

fn view(machine: &Machine, live: Option<&Live>) -> MachineView {
    MachineView {
        id: machine.host_id.clone(),
        paused: machine.paused,
        name: machine.name.clone(),
        display_name: machine.display_name.clone(),
        icon: machine.icon.clone(),
        icon_tint: machine.icon_tint.clone(),
        platform: machine.platform.clone(),
        version: machine.version.clone(),
        routes: machine
            .routes
            .iter()
            .map(|route| RouteView::of(route, machine))
            .collect(),
        // The route in use; else the one that answered last time it was
        // tried; else the one it last connected over. A Windows machine
        // reachable over NetBird was shown on a LAN address that timed out.
        route: live
            .and_then(|live| live.route.as_ref().or(live.answered.as_ref()))
            .or_else(|| machine.last_working_route())
            .map(|route| RouteView::of(route, machine)),
        state: live.map_or(MachineState::Stopped, |live| live.state),
        reason: live
            .filter(|live| live.state != MachineState::Connected)
            .and_then(|live| live.cause)
            .map(Cause::as_str),
        detail: live.and_then(|live| live.detail.clone()),
        hint: live.and_then(|live| live.hint.clone()),
        retry_in_secs: live.and_then(|live| live.retry_in_secs),
        added_at: machine.added_at.to_rfc3339(),
        last_connected_at: machine.last_connected_at.map(|at| at.to_rfc3339()),
    }
}

// ---------------------------------------------------------------- backoff

/// Cheap jitter without a dependency: the sub-second clock, spread over one.
fn jitter() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

/// This computer's own interface addresses, which is what a network change
/// shows up in: a VPN's address appears or goes away, another Wi-Fi hands out
/// a lease (#3436).
///
/// Loopback and link-local addresses are left out: neither is a network
/// another machine can be reached over.
fn local_addresses() -> Vec<IpAddr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    interfaces
        .into_iter()
        .filter(|interface| interface.is_oper_up() && !interface.is_loopback())
        .map(|interface| interface.ip())
        .filter(|address| {
            !address.is_loopback()
                && !address.is_unspecified()
                && !match address {
                    IpAddr::V4(ip) => ip.is_link_local(),
                    IpAddr::V6(ip) => ip.is_unicast_link_local(),
                }
        })
        .collect()
}

// ---------------------------------------------------------------- supervisor

/// How one attempt ended.
enum End {
    /// The machine closed the channel, or this side did.
    Closed,
    /// The machine could not be reached, or the ACP channel could not open.
    /// The route is the one that answered, when one did: the handshake went
    /// through, or the machine refused it.
    Failed(Error, Option<Route>),
    /// The machine is no longer saved; nobody is waiting for it.
    Removed,
}

/// Keep one machine connected: connect, relay, and try again with backoff.
///
/// A machine that refused this computer's pairing is not retried: nothing
/// changes until it is paired again, so the page says that instead of
/// spending attempts.
async fn supervise(
    machines: Arc<Machines>,
    id: String,
    machine: Machine,
    channel: Channel<EngineEvent>,
    mut inbox: tokio::sync::mpsc::Receiver<String>,
    wake: Arc<Notify>,
) {
    let config_dir = match config_dir() {
        Ok(dir) => dir,
        Err(error) => {
            set_state(
                &machines,
                &id,
                MachineState::NeedsPairing,
                Some(error.clone()),
                None,
            );
            emit_closed(&channel, &error);
            return;
        }
    };
    let mut attempt: u32 = 0;
    loop {
        set_state(&machines, &id, MachineState::Connecting, None, None);
        let started = Instant::now();
        // Why this attempt did not stay up, in the words the row shows. The
        // countdown is the chip (`retryInSecs`), never the detail, so a reason
        // is not replaced by "Retrying in N s" (#3341).
        let (cause, answered, reason) =
            match relay_once(&machines, &config_dir, &id, &channel, &mut inbox).await {
                End::Removed => return,
                End::Closed => (
                    Cause::Dropped,
                    None,
                    "The machine closed the connection.".to_owned(),
                ),
                End::Failed(error, answered) => {
                    let message = format!("{error:#}");
                    // The route it answered on last, as saved now: a
                    // connection since the supervisor started may have moved it.
                    let last_route = MachineRegistry::load(&config_dir)
                        .ok()
                        .and_then(|registry| registry.get(&id).and_then(|m| m.last_route.clone()))
                        .or_else(|| machine.last_route.clone());
                    if needs_pairing(&error, last_route.as_deref()) {
                        set_failure(&machines, &id, Cause::RejectedToken, answered);
                        set_state(
                            &machines,
                            &id,
                            MachineState::NeedsPairing,
                            Some(message),
                            None,
                        );
                        emit_closed(&channel, "The machine's pairing is no longer valid.");
                        return;
                    }
                    let cause = attempt_cause(&error, answered.is_some());
                    emit_log(
                        &channel,
                        format!(
                            "desktop: {} is not connected ({}): {message}",
                            machine.name,
                            cause.as_str()
                        ),
                    );
                    (cause, answered, message)
                }
            };
        // A connection that worked for a while deserves a fresh schedule.
        if started.elapsed() >= STABLE_AFTER {
            attempt = 0;
        }
        let delay = backoff_delay(attempt, jitter());
        attempt = attempt.saturating_add(1);
        let secs = delay.as_secs().max(1);
        set_failure(&machines, &id, cause, answered);
        // A machine that answered and would not serve is not "offline": the
        // row and `list_machines` say what it said.
        let state = if cause.reachable() {
            MachineState::Unavailable
        } else {
            MachineState::Offline
        };
        set_state(&machines, &id, state, Some(reason), Some(secs));
        emit_closed(&channel, &format!("Waiting {secs} s before trying again."));
        wait_out(&machines, &id, &channel, &wake, delay).await;
    }
}

/// Why one attempt failed. A failure after the handshake went through that
/// names no refusal is a connection that came up and then ended, not a
/// machine that did not answer.
fn attempt_cause(error: &Error, answered: bool) -> Cause {
    match Cause::of(error) {
        Cause::Unreachable if answered => Cause::Dropped,
        cause => cause,
    }
}

/// Record why the last attempt failed and which route answered in it.
fn set_failure(machines: &Arc<Machines>, id: &str, cause: Cause, answered: Option<Route>) {
    let mut live = machines.live.lock().unwrap();
    if let Some(entry) = live.get_mut(id) {
        entry.cause = Some(cause);
        // A route that answered once stays the one shown until another does:
        // a later timeout on every route says nothing about where it was.
        if answered.is_some() {
            entry.answered = answered;
        }
    }
}

/// Wait out the backoff, ending the wait as soon as this computer's own
/// network changes: a machine that went away because the VPN dropped is
/// reachable again the moment it is back, without waiting out the minute
/// (#3436). The wait is spent in [`NETWORK_POLL`] slices so a change is
/// noticed in seconds. It also ends when this computer wakes from sleep, and
/// when `wake` fires: the app came to the front, a probe found a route that
/// answers, or an agent asked for the machine.
async fn wait_out(
    machines: &Arc<Machines>,
    id: &str,
    channel: &Channel<EngineEvent>,
    wake: &Notify,
    delay: Duration,
) {
    let before = local_addresses();
    let mut left = delay;
    while !left.is_zero() {
        let slice = left.min(NETWORK_POLL);
        let (started, wall) = (Instant::now(), SystemTime::now());
        tokio::select! {
            () = tokio::time::sleep(slice) => {}
            () = wake.notified() => {
                emit_log(channel, "desktop: trying the machine again now, as asked.".to_owned());
                set_state(machines, id, MachineState::Connecting, None, None);
                return;
            }
        }
        left = left.saturating_sub(slice);
        let walled = SystemTime::now().duration_since(wall).unwrap_or_default();
        if rustcode_client::slept(started.elapsed(), walled) {
            emit_log(
                channel,
                "desktop: this computer woke from sleep; trying the machine again now.".to_owned(),
            );
            set_state(machines, id, MachineState::Connecting, None, None);
            return;
        }
        if rustcode_client::network_changed(&before, &local_addresses()) {
            emit_log(
                channel,
                "desktop: this computer's network changed; trying the machine again now."
                    .to_owned(),
            );
            set_state(machines, id, MachineState::Connecting, None, None);
            return;
        }
    }
}

/// One attempt: authenticate, open ACP, then relay frames until the
/// connection ends.
async fn relay_once(
    machines: &Arc<Machines>,
    config_dir: &Path,
    id: &str,
    channel: &Channel<EngineEvent>,
    inbox: &mut tokio::sync::mpsc::Receiver<String>,
) -> End {
    // Read again every attempt: a machine removed while we waited is gone, and
    // a route learned elsewhere is picked up on the next try.
    let machine = match MachineRegistry::load(config_dir) {
        Ok(registry) => match registry.get(id) {
            Some(machine) => machine.clone(),
            None => return End::Removed,
        },
        Err(error) => return End::Failed(error, None),
    };
    let tokens = match machines.tokens(config_dir).await {
        Ok(tokens) => tokens,
        Err(error) => return End::Failed(anyhow::anyhow!(error), None),
    };
    let connector = match Machines::connector(config_dir) {
        Ok(connector) => connector,
        Err(error) => return End::Failed(anyhow::anyhow!(error), None),
    };
    // The route the machine answered on last time is tried first after a
    // drop; the classes still decide most of the race, so a laptop that comes
    // home dials the LAN before the overlay (#3436).
    let mut machine = machine.clone();
    machine.routes =
        rustcode_client::prefer_last_working(&machine.routes, machine.last_route.as_deref());
    let connected = match connector.connect_raced(&machine, &tokens).await {
        Ok(connected) => connected,
        Err(error) => {
            // A route that refused the handshake answered: show that one.
            let answered = error
                .downcast_ref::<Unreachable>()
                .and_then(|lost| lost.answered.as_deref())
                .and_then(|address| machine.routes.iter().find(|route| route.address == address))
                .cloned();
            return End::Failed(error, answered);
        }
    };
    let route = connected.route.clone();
    let hint = connected.hint(&machine.routes, &rustcode_client::local_addresses());
    let failed = connected.failed.clone();
    let learned = connected.learned.clone();
    let session = connected.session;
    let version = session.welcome().version.clone();
    let platform = session.welcome().platform.clone();
    if let Ok(mut registry) = MachineRegistry::load(config_dir) {
        let _ = registry.mark_connected(&machine.host_id, &route.address, chrono::Utc::now());
        if !learned.is_empty() {
            let _ = registry.learn_routes(&machine.host_id, &learned);
        }
        if version.is_some() {
            let _ = registry.set_version(&machine.host_id, version.as_deref());
        }
        // A machine paired while its gateway reported no platform learns it
        // on the next connection (#3418).
        if platform.is_some() {
            let _ = registry.set_platform(&machine.host_id, platform.as_deref());
        }
    }
    let mut acp = match session.acp().await {
        Ok(acp) => acp,
        // The handshake went through on this route, so the machine is there:
        // a gateway whose server is gone refuses ACP here.
        Err(error) => return End::Failed(error, Some(route)),
    };
    set_connected(machines, id, &route);
    set_hint(machines, id, hint);
    // A LAN win stops the race before the overlay attempts start. Ask about
    // them in the background, so the row can say why the overlay is not used
    // without holding the connection up.
    schedule_overlay_probe(
        machines,
        config_dir,
        id,
        machine.clone(),
        route.clone(),
        failed,
    );
    let _ = channel.send(EngineEvent::Open {
        engine: format!("{} ({})", machine.name, route.address),
    });
    // Liveness: a link the network dropped without a close is noticed by its
    // silence, and checked at once after a wake from sleep or a network
    // change by a ping the machine has to answer.
    let mut addresses = local_addresses();
    let mut check = tokio::time::interval(NETWORK_POLL);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut clocks = (Instant::now(), SystemTime::now());
    // A ping in flight: when it went out, and when it must have been answered.
    let mut pinged: Option<(tokio::time::Instant, tokio::time::Instant)> = None;
    loop {
        // One step per pass: read the machine's next frame, send a line the
        // webview wrote, or check the link. The read is cancelled when another
        // branch goes first, so the frame branch is polled first and a frame
        // that is already there is never dropped.
        enum Step {
            Frame(Result<Option<serde_json::Value>, Error>),
            Line(Option<String>),
            Check,
        }
        let step = {
            let read = acp.next();
            tokio::pin!(read);
            tokio::select! {
                biased;
                frame = &mut read => Step::Frame(frame),
                line = inbox.recv() => Step::Line(line),
                _ = check.tick() => Step::Check,
            }
        };
        match step {
            Step::Check => {
                let now = tokio::time::Instant::now();
                let heard = acp.last_heard();
                if now.duration_since(heard) >= rustcode_client::SILENCE_LIMIT {
                    return End::Failed(
                        anyhow::anyhow!(
                            "the machine sent nothing for {} s; the connection is gone",
                            rustcode_client::SILENCE_LIMIT.as_secs()
                        ),
                        Some(route.clone()),
                    );
                }
                if let Some((sent, due)) = pinged {
                    if heard >= sent {
                        pinged = None;
                    } else if now >= due {
                        return End::Failed(
                            anyhow::anyhow!(
                                "the machine did not answer a ping within {} s; the connection is gone",
                                rustcode_client::answer_deadline(&route).as_secs()
                            ),
                            Some(route.clone()),
                        );
                    }
                }
                let (started, wall) = clocks;
                clocks = (Instant::now(), SystemTime::now());
                let walled = clocks.1.duration_since(wall).unwrap_or_default();
                let woke = rustcode_client::slept(started.elapsed(), walled);
                let current = local_addresses();
                let moved = rustcode_client::network_changed(&addresses, &current);
                addresses = current;
                if (woke || moved) && pinged.is_none() {
                    if let Err(error) = acp.ping().await {
                        return End::Failed(error, Some(route.clone()));
                    }
                    let sent = tokio::time::Instant::now();
                    pinged = Some((sent, sent + rustcode_client::answer_deadline(&route)));
                    // Check again as soon as the answer is due, not at the
                    // next poll.
                    check.reset_after(rustcode_client::answer_deadline(&route));
                }
            }
            Step::Frame(Ok(Some(frame))) => {
                if channel
                    .send(EngineEvent::Line {
                        line: frame.to_string(),
                    })
                    .is_err()
                {
                    return End::Closed;
                }
            }
            Step::Frame(Ok(None)) => return End::Closed,
            Step::Frame(Err(error)) => return End::Failed(error, Some(route.clone())),
            Step::Line(Some(line)) => {
                // A line the webview wrote; only JSON reaches a machine.
                let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if let Err(error) = acp.send(frame).await {
                    return End::Failed(error, Some(route.clone()));
                }
            }
            Step::Line(None) => return End::Closed,
        }
    }
}

/// Whether pairing again is what would fix this failure, for a machine whose
/// last working route is `last_route`.
///
/// A plain `unauthorized` from another route is not believed: a stale LAN
/// address that another RustCode computer now answers on refuses every device
/// it does not know, and stopping there would leave a machine that is only
/// asleep at *Needs pairing* for good. That failure keeps being retried.
pub fn needs_pairing(error: &Error, last_route: Option<&str>) -> bool {
    if let Some(lost) = error
        .downcast_ref::<Unreachable>()
        .filter(|lost| lost.refused.is_some())
    {
        return lost.needs_new_pairing_at(last_route);
    }
    // The token lookup and the identity check (a device record another
    // pairing replaced, a token that is gone) report as plain errors.
    Cause::of(error) == Cause::RejectedToken
}

fn set_connected(machines: &Arc<Machines>, id: &str, route: &Route) {
    let mut live = machines.live.lock().unwrap();
    if let Some(entry) = live.get_mut(id) {
        entry.state = MachineState::Connected;
        entry.route = Some(route.clone());
        entry.answered = None;
        entry.cause = None;
        entry.detail = None;
        entry.hint = None;
        entry.retry_in_secs = None;
    }
}

/// Show a hint on a connected machine, unless the connection moved on.
fn set_hint(machines: &Arc<Machines>, id: &str, hint: Option<String>) {
    let mut live = machines.live.lock().unwrap();
    if let Some(entry) = live.get_mut(id) {
        if entry.state == MachineState::Connected {
            entry.hint = hint;
        }
    }
}

/// Ask the machine's overlay routes whether they answer, after it connected
/// over another route, and put the answer in its status. The probe runs on a
/// task of its own: it is a diagnostic, never a reason to wait.
fn schedule_overlay_probe(
    machines: &Arc<Machines>,
    config_dir: &Path,
    id: &str,
    machine: Machine,
    route: Route,
    failed: Vec<rustcode_client::RouteFailure>,
) {
    let has_overlay = machine.routes.iter().any(|saved| {
        matches!(
            saved.kind,
            rustcode_client::RouteKind::Vpn | rustcode_client::RouteKind::Tailscale
        ) && saved.address != route.address
    });
    if !has_overlay {
        return;
    }
    let machines = Arc::clone(machines);
    let config_dir = config_dir.to_path_buf();
    let id = id.to_owned();
    tauri::async_runtime::spawn(async move {
        let Ok(connector) = Machines::connector(&config_dir) else {
            return;
        };
        let mut failures = failed;
        failures.extend(connector.probe_overlay_routes(&machine, &route).await);
        let hint = rustcode_client::RouteReport {
            routes: &machine.routes,
            in_use: &route,
            failures: &failures,
            local: &rustcode_client::local_addresses(),
        }
        .hint();
        let mut live = machines.live.lock().unwrap();
        let still_connected = live.get(&id).is_some_and(|entry| {
            entry.state == MachineState::Connected
                && entry
                    .route
                    .as_ref()
                    .is_some_and(|in_use| in_use.address == route.address)
        });
        if still_connected {
            if let Some(entry) = live.get_mut(&id) {
                entry.hint = hint;
            }
        }
    });
}

fn set_state(
    machines: &Arc<Machines>,
    id: &str,
    state: MachineState,
    detail: Option<String>,
    retry_in_secs: Option<u64>,
) {
    let mut live = machines.live.lock().unwrap();
    if let Some(entry) = live.get_mut(id) {
        entry.state = state;
        entry.detail = detail;
        entry.retry_in_secs = retry_in_secs;
        if state != MachineState::Connected {
            entry.route = None;
        }
    }
}

fn emit_closed(channel: &Channel<EngineEvent>, reason: &str) {
    let _ = channel.send(EngineEvent::Closed {
        reason: reason.to_owned(),
    });
}

fn emit_log(channel: &Channel<EngineEvent>, line: String) {
    let _ = channel.send(EngineEvent::Log { line });
}

// ---------------------------------------------------------------- commands

#[tauri::command(async)]
pub fn machines_list(
    machines: tauri::State<'_, Arc<Machines>>,
) -> Result<Vec<MachineView>, String> {
    machines.list()
}

/// The machines the Add sheet can list without an address typed: those
/// Bonjour finds on this network (a few seconds), and the saved machines whose
/// own handshake reported an overlay address, which no browse can hear.
///
/// The page de-duplicates the two by host id; one machine found both ways is
/// one row, with what either source knows of it.
#[tauri::command]
pub async fn machines_discover(
    machines: tauri::State<'_, Arc<Machines>>,
) -> Result<Discovery, String> {
    let saved = machines.list().unwrap_or_default();
    let mut notice = None;
    let mut views: Vec<DiscoveredView> = match browse_async(DISCOVERY).await {
        Ok(found) => {
            let this = ThisComputer::read();
            found
                .iter()
                .filter(|machine| !this.is(machine))
                .map(|machine| discovered(machine, &saved))
                .collect()
        }
        // A network without multicast (a container, a firewall) cannot browse,
        // but the overlay rows come from this machine's own registry: the sheet
        // lists them and says what failed, instead of showing nothing.
        Err(error) => {
            notice = Some(format!(
                "Could not look for machines on this network: {error}"
            ));
            Vec::new()
        }
    };
    views.extend(overlay_peers(&saved, &views));
    Ok(Discovery {
        rows: views,
        notice,
    })
}

/// How this computer is told apart from another one in a browse: Bonjour
/// announces the gateway this computer runs too, and the Add sheet must not
/// offer to connect it to itself.
struct ThisComputer {
    /// The gateway id this computer presents (its own gateway's, when it runs one).
    gateway_id: Option<String>,
    /// The port its gateway listens on.
    port: u16,
    /// Whether an address belongs to one of this computer's interfaces.
    owns: fn(IpAddr) -> bool,
}

impl ThisComputer {
    fn read() -> Self {
        let dir = config_dir().ok();
        Self {
            gateway_id: dir
                .as_deref()
                .and_then(|dir| rustcode_client::machine::Identity::load(dir).ok())
                .map(|identity| identity.host_id),
            port: dir
                .as_deref()
                .and_then(gateway_port)
                .unwrap_or(rustcode_client::DEFAULT_PORT),
            owns: is_local_address,
        }
    }

    /// The same gateway id, or one of this computer's own addresses on its
    /// gateway port (an announcement that carries no id, or an old one).
    fn is(&self, machine: &DiscoveredMachine) -> bool {
        if let (Some(own), Some(found)) = (&self.gateway_id, &machine.gateway_id) {
            if own == found {
                return true;
            }
        }
        machine.port == self.port
            && machine
                .addresses
                .iter()
                .any(|address| (self.owns)(*address))
    }
}

/// The port the local gateway listens on, from its registration file.
fn gateway_port(config_dir: &Path) -> Option<u16> {
    let bytes = std::fs::read(config_dir.join("remote/registration.json")).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let listen = value.get("listen_address")?.as_str()?;
    listen
        .parse::<SocketAddr>()
        .ok()
        .map(|address| address.port())
}

/// Whether `address` is one of this computer's own: only an address on a local
/// interface can be bound.
fn is_local_address(address: IpAddr) -> bool {
    address.is_loopback() || std::net::UdpSocket::bind((address, 0)).is_ok()
}

/// What one Bonjour record says, plus what this app already knows of the
/// machine when it is saved.
fn discovered(machine: &DiscoveredMachine, saved: &[MachineView]) -> DiscoveredView {
    let known = machine
        .gateway_id
        .as_ref()
        .and_then(|id| saved.iter().find(|view| &view.id == id));
    let route = machine.address().map(Route::new);
    DiscoveredView {
        name: machine.name.clone(),
        source: "bonjour",
        address: route.as_ref().map(|route| route.address.clone()),
        addresses: machine.addresses.iter().map(ToString::to_string).collect(),
        gateway_id: machine.gateway_id.clone(),
        platform: known.map(|view| view.platform.clone()),
        version: known.and_then(|view| view.version.clone()),
        route_kind: route_kind_name(route.as_ref()),
        hint: known.and_then(|view| view.hint.clone()),
        saved: known.is_some(),
    }
}

/// A saved machine reached at an address its handshake reported — the overlay
/// peers a browse on the LAN cannot hear. One row per machine, at the first
/// route that is not a LAN address.
fn overlay_peers(saved: &[MachineView], found: &[DiscoveredView]) -> Vec<DiscoveredView> {
    saved
        .iter()
        .filter(|view| {
            !found
                .iter()
                .any(|row| row.gateway_id.as_deref() == Some(&view.id))
        })
        .filter_map(|view| {
            let route = view
                .routes
                .iter()
                .find(|route| route.kind != "lan")
                .or_else(|| view.route.as_ref().filter(|route| route.kind != "lan"))?;
            Some(DiscoveredView {
                name: view.name.clone(),
                source: "overlay",
                address: Some(route.address.clone()),
                addresses: vec![route.address.clone()],
                gateway_id: Some(view.id.clone()),
                platform: Some(view.platform.clone()),
                version: view.version.clone(),
                route_kind: route_route_kind_name(route),
                hint: view.hint.clone(),
                saved: true,
            })
        })
        .collect()
}

/// The kind a route is shown as: the overlay's own name when the machine
/// named it (`netbird`), else `lan`, `tailscale`, `vpn` or `public`.
fn route_kind_name(route: Option<&Route>) -> String {
    match route {
        Some(route) => route
            .provider
            .map_or(route.kind.as_str(), OverlayProvider::as_str)
            .to_owned(),
        None => "lan".to_owned(),
    }
}

fn route_route_kind_name(route: &RouteView) -> String {
    route.provider.unwrap_or(route.kind).to_owned()
}

/// Ask `address` to let this app in (`Settings › Machines › Add machine`).
///
/// `ask_id` is the page's own name for the request: it is registered before
/// anything is dialled, so `machines_cancel` can drop the socket whether the
/// owner has answered yet or not. The owner answers on that machine; the
/// accepted request waits here for its code. A denial, a request nobody
/// answered or a cancel comes back as this command's error, in words the page
/// shows where "Waiting for ‹host›…" was.
#[tauri::command]
pub async fn machines_ask(
    machines: tauri::State<'_, Arc<Machines>>,
    ask_id: String,
    address: String,
    name: Option<String>,
    on_event: Channel<PairingEvent>,
) -> Result<AskView, String> {
    ensure_ask_id(&ask_id)?;
    let cancel = Arc::new(Notify::new());
    machines
        .asks
        .lock()
        .unwrap()
        .insert(ask_id.clone(), Arc::clone(&cancel));
    let question = ask_machine(&machines, &ask_id, &address, name, &on_event, &cancel).await;
    // The ask is over either way; `pair` keeps the accepted one under its own
    // entry, so this only drops the cancel signal.
    machines.asks.lock().unwrap().remove(&ask_id);
    question
}

async fn ask_machine(
    machines: &Arc<Machines>,
    ask_id: &str,
    address: &str,
    name: Option<String>,
    on_event: &Channel<PairingEvent>,
    cancel: &Arc<Notify>,
) -> Result<AskView, String> {
    let config_dir = config_dir()?;
    let connector = Machines::connector(&config_dir)?;
    // Open the credential store before anything is dialled: a store that cannot
    // answer would otherwise surface only after the other machine's owner has
    // accepted the request and enrolled this computer — the pairing would look
    // hung, and the machine there would list a device that never arrives (#3325).
    machines.tokens(&config_dir).await?;
    let label = name.clone().unwrap_or_else(|| address.to_owned());
    let ask = connector
        .ask(address)
        .await
        .map_err(|error| format!("{error:#}"))?;
    on_event
        .send(PairingEvent::Waiting {
            ask_id: ask_id.to_owned(),
            host: label.clone(),
        })
        .ok();
    // The wait is dropped (and its socket with it) the moment the page
    // cancels, instead of holding a request nobody wants for two minutes.
    let answer = {
        let waiting = ask.wait(rustcode_client::machine::APPROVAL_TIMEOUT);
        tokio::pin!(waiting);
        tokio::select! {
            answer = &mut waiting => answer,
            () = cancel.notified() => {
                return Err("The request was cancelled.".to_owned());
            }
        }
    };
    let approved = match answer {
        Ok(Some(approved)) => approved,
        Ok(None) => {
            let message =
                format!("{label} did not answer the request. Ask again, or accept it on that machine first.");
            let _ = on_event.send(PairingEvent::Refused {
                message: message.clone(),
            });
            return Err(message);
        }
        Err(error) => {
            let message = format!("{error:#}");
            let _ = on_event.send(PairingEvent::Refused {
                message: message.clone(),
            });
            return Err(message);
        }
    };
    // Cancelled in the instant between the answer and this line: the cancel
    // took the entry, so the socket is dropped rather than kept for a code
    // nobody will type.
    if !machines.asks.lock().unwrap().contains_key(ask_id) {
        return Err("The request was cancelled.".to_owned());
    }
    let view = AskView {
        ask_id: ask_id.to_owned(),
        code_length: approved.code_length(),
        expires_in_secs: approved.expires_in().as_secs(),
    };
    on_event
        .send(PairingEvent::Approved {
            ask_id: ask_id.to_owned(),
            code_length: view.code_length,
            expires_in_secs: view.expires_in_secs,
        })
        .ok();
    machines.approved.lock().unwrap().insert(
        ask_id.to_owned(),
        PendingApproval {
            approved,
            address: address.to_owned(),
            name,
        },
    );
    Ok(view)
}

/// An ask id travels back and forth as a command argument and a map key: it is
/// short, printable and never empty.
fn ensure_ask_id(ask_id: &str) -> Result<(), String> {
    let usable = !ask_id.is_empty()
        && ask_id.len() <= 64
        && ask_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if usable {
        Ok(())
    } else {
        Err("The page sent an ask id that is not one. Start the request again.".to_owned())
    }
}

/// The code the machine shows, typed here: the machine is saved, its token
/// kept, and its supervisor can start.
#[tauri::command]
pub async fn machines_pair(
    machines: tauri::State<'_, Arc<Machines>>,
    ask_id: String,
    code: String,
) -> Result<MachineView, String> {
    let pending = machines
        .approved
        .lock()
        .unwrap()
        .remove(&ask_id)
        .ok_or("That connection request is over. Ask the machine to connect again.")?;
    let code = normalize_code(&code);
    let expected = pending.approved.code_length() as usize;
    if code.len() != expected {
        return Err(format!(
            "A pairing code is exactly {expected} digits. Ask the machine again for a new code."
        ));
    }
    let paired = pending
        .approved
        .pair(&code)
        .await
        .map_err(|error| format!("The code was not accepted: {error:#}"))?;
    let welcome = paired.session.welcome().clone();
    let machine = machine_from_pairing(&welcome, Route::new(pending.address), pending.name);
    let config_dir = config_dir()?;
    // The token first: a saved machine without one would only ever say "pair
    // again", while a token whose machine was not saved is unreachable. The
    // store was opened before the request was sent, so this is the write, and a
    // write that does not land in time says so instead of waiting for ever.
    let tokens = machines.tokens(&config_dir).await?;
    token_store::set_bounded(
        Arc::clone(&tokens),
        machine.host_id.clone(),
        paired.secret.clone(),
        STORE_TIMEOUT,
    )
    .await
    .map_err(|error| {
        format!(
            "{} accepted this computer, but its pairing token could not be stored: {error:#}. \
             It now lists this computer as a device; pair again once the store answers, or remove \
             this computer on {} first.",
            machine.name, machine.name
        )
    })?;
    let mut registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    if let Err(error) = registry.insert(machine.clone()) {
        let _ = tokens.remove(&machine.host_id);
        return Err(format!(
            "The machine is paired, but could not be saved: {error:#}"
        ));
    }
    let _ = registry.mark_connected(
        &machine.host_id,
        &machine.routes[0].address,
        chrono::Utc::now(),
    );
    let connecting = Live {
        state: MachineState::Connecting,
        ..Live::new()
    };
    Ok(view(&machine, Some(&connecting)))
}

/// Stop waiting for a machine's owner, at any point of the ask: a request
/// still waiting wakes up and drops its socket; an accepted one waiting for
/// its code is dropped here.
#[tauri::command(async)]
pub fn machines_cancel(
    machines: tauri::State<'_, Arc<Machines>>,
    ask_id: String,
) -> Result<(), String> {
    if let Some(cancel) = machines.asks.lock().unwrap().remove(&ask_id) {
        // A permit is kept for a waiter that has not reached its select yet.
        cancel.notify_one();
    }
    machines.approved.lock().unwrap().remove(&ask_id);
    Ok(())
}

/// Forget a machine and its token. It still lists this computer until it is
/// revoked there.
///
/// The credential goes first: a machine whose token could not be deleted stays
/// listed, with a plain reason, instead of becoming a row that can never pair
/// again. (A machine left without a token is re-paired, which works.)
#[tauri::command]
pub async fn machines_remove(
    machines: tauri::State<'_, Arc<Machines>>,
    id: String,
) -> Result<(), String> {
    let config_dir = config_dir()?;
    let mut registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    let machine = registry
        .get(&id)
        .ok_or("That machine is no longer saved.")?
        .clone();
    let tokens = machines.tokens(&config_dir).await?;
    let host_id = machine.host_id.clone();
    token_store::bounded("deleting the pairing token", STORE_TIMEOUT, move || {
        tokens.remove(&host_id)
    })
    .await
    .map_err(|error| {
        format!(
            "Could not delete {}'s pairing token: {error:#}",
            machine.name
        )
    })?;
    if let Err(error) = registry.remove(&machine.host_id) {
        // The token is gone, so the row cannot connect until it is paired
        // again; say that rather than leaving a row that silently fails.
        return Err(format!(
            "{}'s pairing token was deleted, but the machine could not be removed: {error:#}. \
             Pair with it again, or remove it once more.",
            machine.name
        ));
    }
    if let Some(mut entry) = machines.live.lock().unwrap().remove(&machine.host_id) {
        if let Some(task) = entry.task.take() {
            task.abort();
        }
    }
    Ok(())
}

/// Rename a machine here: a local display name, never sent to that machine.
/// `None` or a blank name shows the name the machine reports again.
#[tauri::command(async)]
pub fn machines_set_display_name(id: String, name: Option<String>) -> Result<(), String> {
    let config_dir = config_dir()?;
    let mut registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    registry
        .set_display_name(&id, name.as_deref())
        .map_err(chain)
}

/// Choose the glyph and series tint this app draws for a machine. `icon` of
/// `None` returns to the platform's own glyph.
#[tauri::command(async)]
pub fn machines_set_icon(
    id: String,
    icon: Option<String>,
    tint: Option<String>,
) -> Result<(), String> {
    let config_dir = config_dir()?;
    let mut registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    registry
        .set_icon(&id, icon.as_deref(), tint.as_deref())
        .map_err(chain)
}

/// Pause or resume new threads on a machine: Run on refuses a paused one,
/// while its connection and its existing threads stay as they are.
#[tauri::command(async)]
pub fn machines_set_paused(id: String, paused: bool) -> Result<(), String> {
    let config_dir = config_dir()?;
    let mut registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    registry.set_paused(&id, paused).map_err(chain)
}

/// Test every route of one machine (all of them when `id` is none) with a plain
/// connect, save what each found, and answer the refreshed machines. The
/// connect carries no token, so a route that is down costs only its timeout.
///
/// A machine that is waiting out its backoff while one of its routes answered
/// is tried again at once: a probe that finds the machine is the best reason
/// there is to connect to it.
#[tauri::command(async)]
pub async fn machines_probe(
    machines: tauri::State<'_, Arc<Machines>>,
    id: Option<String>,
) -> Result<Vec<MachineView>, String> {
    let config_dir = config_dir()?;
    let registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    let targets: Vec<Machine> = match &id {
        Some(id) => vec![registry
            .get(id)
            .ok_or("That machine is no longer saved.")?
            .clone()],
        None => registry.machines().to_vec(),
    };
    // One task per machine, so a machine that is down costs its timeout once,
    // not once per machine.
    let tasks: Vec<_> = targets
        .iter()
        .map(|machine| {
            let machine = machine.clone();
            let config_dir = config_dir.clone();
            tauri::async_runtime::spawn(async move {
                let connector = Machines::connector(&config_dir)?;
                Ok::<_, String>(connector.probe_routes(&machine).await)
            })
        })
        .collect();
    let mut registry = registry;
    for (machine, task) in targets.iter().zip(tasks) {
        let probes = task.await.map_err(|error| error.to_string())??;
        let answered = probes
            .iter()
            .any(|probe| probe.outcome == ProbeOutcome::Reachable);
        registry
            .set_probes(&machine.host_id, probes)
            .map_err(chain)?;
        if answered {
            machines.retry(Some(&machine.host_id));
        }
    }
    machines.list()
}

/// Try machines again now instead of at the end of their backoff: one machine,
/// or every machine that is waiting when `id` is none. The app calls it when
/// its window comes to the front or the network comes back, and
/// `list_machines` when an agent asks for a machine. Answers the ids that were
/// woken.
#[tauri::command(async)]
pub fn machines_retry(
    machines: tauri::State<'_, Arc<Machines>>,
    id: Option<String>,
) -> Result<Vec<String>, String> {
    Ok(machines.retry(id.as_deref()))
}

/// Connect to one machine and relay its ACP to this page.
#[tauri::command(async)]
pub fn machine_connect(
    machines: tauri::State<'_, Arc<Machines>>,
    id: String,
    on_event: Channel<EngineEvent>,
) -> Result<(), String> {
    machines.inner().connect(&id, on_event)
}

/// One ACP line for a machine, exactly as the webview writes it to the local
/// engine.
#[tauri::command(async)]
pub fn machine_send(
    machines: tauri::State<'_, Arc<Machines>>,
    id: String,
    line: String,
) -> Result<(), String> {
    let live = machines.live.lock().unwrap();
    let entry = live
        .get(&id)
        .ok_or("That machine is no longer connected.")?;
    let outbox = entry
        .outbox
        .as_ref()
        .ok_or("That machine is not connected yet.")?;
    outbox
        .try_send(line)
        .map_err(|_| "The machine is not keeping up; the line was dropped.".to_string())
}

/// Stop trying to reach one machine. Nothing is forgotten: connecting again
/// starts a fresh supervisor.
#[tauri::command(async)]
pub fn machine_disconnect(
    machines: tauri::State<'_, Arc<Machines>>,
    id: String,
) -> Result<(), String> {
    let mut live = machines.live.lock().unwrap();
    if let Some(entry) = live.get_mut(&id) {
        if let Some(task) = entry.task.take() {
            task.abort();
        }
        entry.outbox = None;
        entry.state = MachineState::Stopped;
        entry.route = None;
        entry.answered = None;
        entry.cause = None;
        entry.detail = None;
        entry.hint = None;
        entry.retry_in_secs = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcode_client::{ErrorCode, RouteKind};

    fn machine() -> Machine {
        Machine::new(
            "gw-8e1f4b7a5c2d4e6f",
            "Lars's MacBook Pro",
            "macos",
            "device-2f1a",
            Route::new("192.168.1.20:17879"),
            chrono::Utc::now(),
        )
    }

    /// A saved machine as the page already has it: a LAN route, the NetBird
    /// address its handshake reported, and the version it runs.
    fn saved_view(id: &str) -> MachineView {
        let mut saved = machine();
        saved.host_id = id.to_owned();
        saved.version = Some("0.96.0".to_owned());
        let mut overlay = Route::with_kind(RouteKind::Vpn, "100.90.16.237:17879");
        overlay.provider = Some(OverlayProvider::Netbird);
        saved.routes.push(overlay);
        MachineView {
            hint: Some("Not reachable over NetBird (100.90.16.237:17879).".to_owned()),
            ..view(&saved, None)
        }
    }

    /// A refusal that pairing again fixes reads as "pairing expired"; a
    /// timeout reads as offline, which the supervisor retries.
    #[test]
    fn only_a_refused_pairing_stops_the_retries() {
        let revoked = Error::new(Unreachable {
            machine: "Studio".into(),
            attempts: vec!["192.168.1.20:17879: unauthorized".into()],
            refused: Some(ErrorCode::Revoked),
            answered: Some("192.168.1.20:17879".into()),
        });
        assert!(needs_pairing(&revoked, Some("10.0.0.9:17879")));
        let replaced =
            Error::msg("Studio replaced this computer's device record; pair with it again");
        assert!(needs_pairing(&replaced, None));
        let unreachable = Error::new(Unreachable {
            machine: "Studio".into(),
            attempts: vec!["192.168.1.20:17879: connection refused".into()],
            refused: None,
            answered: None,
        });
        assert!(!needs_pairing(&unreachable, None));
        assert!(!needs_pairing(
            &Error::msg("no frame from the machine within 5s"),
            None
        ));

        // A stale LAN address another RustCode computer answers on refuses
        // with `unauthorized`; the machine itself was last reached over
        // NetBird and only timed out. That is retried, not unpaired.
        let stranger = Error::new(Unreachable {
            machine: "Studio".into(),
            attempts: vec![
                "192.168.1.20:17879: unauthorized".into(),
                "100.90.16.237:17879: timed out".into(),
            ],
            refused: Some(ErrorCode::Unauthorized),
            answered: Some("192.168.1.20:17879".into()),
        });
        assert!(!needs_pairing(&stranger, Some("100.90.16.237:17879")));
        // From the route it answered on last, `unauthorized` is believed, and
        // so it is when no route is on record yet.
        assert!(needs_pairing(&stranger, Some("192.168.1.20:17879")));
        assert!(needs_pairing(&stranger, None));
    }

    /// The page reads these names: camelCase fields, and a state per word it
    /// switches on.
    #[test]
    fn a_machine_view_is_what_the_page_reads() {
        let mut saved = machine();
        saved.platform = "unknown".into();
        let live = Live {
            state: MachineState::Offline,
            detail: Some("Not connected. Retrying in 4 s.".into()),
            retry_in_secs: Some(4),
            ..Live::new()
        };
        let json = serde_json::to_value(view(&saved, Some(&live))).unwrap();
        assert_eq!(json["id"], "gw-8e1f4b7a5c2d4e6f");
        assert_eq!(json["name"], "Lars's MacBook Pro");
        assert_eq!(json["platform"], "unknown");
        assert_eq!(json["state"], "offline");
        assert_eq!(json["retryInSecs"], 4);
        assert_eq!(json["detail"], "Not connected. Retrying in 4 s.");
        assert!(json.get("main").is_none(), "every machine is a peer");
        assert_eq!(json["routes"][0]["kind"], "lan");
        assert_eq!(json["routes"][0]["address"], "192.168.1.20:17879");
        assert_eq!(json["routes"][0]["encrypted"], false);
        // Offline, the row still says which route it will try.
        assert_eq!(json["route"]["address"], "192.168.1.20:17879");
    }

    /// A machine whose gateway answered on NetBird but has no server behind
    /// it is "unavailable" with the reason, on the route that answered, not
    /// "offline" on the LAN address that timed out (Lagos, 2026-11-12).
    #[test]
    fn a_machine_that_answered_and_would_not_serve_says_why_and_where() {
        let mut saved = machine();
        let netbird = Route::with_kind(RouteKind::Vpn, "100.90.27.158:17879");
        saved.routes.push(netbird.clone());
        let refusal = Error::new(rustcode_client::Refusal {
            step: "acp_open".into(),
            code: ErrorCode::Internal,
            message: "the relay to this machine's engine could not be opened".into(),
            retry_after_secs: None,
        });
        assert!(!needs_pairing(&refusal, None), "a dead server is retried");
        let cause = attempt_cause(&refusal, true);
        assert_eq!(cause, Cause::ServerNotRunning);
        let live = Live {
            state: MachineState::Unavailable,
            answered: Some(netbird),
            cause: Some(cause),
            detail: Some(format!("{refusal:#}")),
            retry_in_secs: Some(8),
            ..Live::new()
        };
        let json = serde_json::to_value(view(&saved, Some(&live))).unwrap();
        assert_eq!(json["state"], "unavailable");
        assert_eq!(json["reason"], "server_not_running");
        assert_eq!(json["route"]["address"], "100.90.27.158:17879");
        assert!(json["detail"]
            .as_str()
            .unwrap()
            .contains("no RustCode engine is running there"));

        // Nothing answered this time: the route it last connected over is
        // shown, not the first saved one.
        saved.last_route = Some("100.90.27.158:17879".into());
        let away = Live {
            state: MachineState::Offline,
            cause: Some(Cause::Unreachable),
            ..Live::new()
        };
        let json = serde_json::to_value(view(&saved, Some(&away))).unwrap();
        assert_eq!(json["reason"], "unreachable");
        assert_eq!(json["route"]["address"], "100.90.27.158:17879");

        // A connection that came up and then ended without a refusal dropped.
        assert_eq!(
            attempt_cause(&Error::msg("the connection to the machine failed"), true),
            Cause::Dropped
        );
        assert_eq!(
            attempt_cause(&Error::msg("connection refused"), false),
            Cause::Unreachable
        );
    }

    /// A wake reaches only a supervisor that is waiting out its backoff, and
    /// ends that wait.
    #[tokio::test]
    async fn a_retry_wakes_only_the_machines_that_wait() {
        let machines = Machines::default();
        let parked = |state| Live {
            state,
            task: Some(tauri::async_runtime::spawn(std::future::pending::<()>())),
            ..Live::new()
        };
        {
            let mut live = machines.live.lock().unwrap();
            live.insert("away".into(), parked(MachineState::Offline));
            live.insert("refused".into(), parked(MachineState::Unavailable));
            live.insert("up".into(), parked(MachineState::Connected));
            live.insert("unpaired".into(), parked(MachineState::NeedsPairing));
            // Never started: there is no supervisor to wake.
            live.insert(
                "stopped".into(),
                Live {
                    state: MachineState::Offline,
                    ..Live::new()
                },
            );
        }
        let mut woken = machines.retry(None);
        woken.sort();
        assert_eq!(woken, ["away", "refused"]);
        assert_eq!(machines.retry(Some("up")), Vec::<String>::new());
        assert_eq!(machines.retry(Some("away")), ["away"]);
        // The permit is there for the waiting supervisor to take.
        let wake = Arc::clone(&machines.live.lock().unwrap()["away"].wake);
        tokio::time::timeout(Duration::from_secs(1), wake.notified())
            .await
            .expect("the wake reaches the wait");
        for entry in machines.live.lock().unwrap().values_mut() {
            if let Some(task) = entry.task.take() {
                task.abort();
            }
        }
    }

    /// A route carries what its last test found, and none before a test.
    #[test]
    fn a_machine_view_shows_each_routes_last_test() {
        let mut saved = machine();
        saved
            .routes
            .push(Route::with_kind(RouteKind::Vpn, "100.90.16.237"));
        saved.probes.push(rustcode_client::RouteProbe {
            address: "192.168.1.20:17879".to_owned(),
            outcome: rustcode_client::ProbeOutcome::Reachable,
            latency_ms: Some(4),
            detail: None,
            at: chrono::Utc::now(),
        });
        let json = serde_json::to_value(view(&saved, None)).unwrap();
        assert_eq!(json["routes"][0]["probe"]["outcome"], "reachable");
        assert_eq!(json["routes"][0]["probe"]["latencyMs"], 4);
        assert!(json["routes"][1]["probe"].is_null());
        // The route it will try carries the same result.
        assert_eq!(json["route"]["probe"]["outcome"], "reachable");
    }

    /// A machine that is connected over an overlay says so, and the page
    /// labels that route encrypted.
    #[test]
    fn a_connected_machine_names_the_route_in_use() {
        let mut saved = machine();
        saved.routes.push(Route::with_kind(
            RouteKind::Tailscale,
            "mac.tail1234.ts.net:17879",
        ));
        let live = Live {
            state: MachineState::Connected,
            route: Some(Route::with_kind(
                RouteKind::Tailscale,
                "mac.tail1234.ts.net:17879",
            )),
            ..Live::new()
        };
        let view = view(&saved, Some(&live));
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["state"], "connected");
        assert_eq!(json["route"]["kind"], "tailscale");
        assert_eq!(json["route"]["encrypted"], true);
        assert_eq!(json["routes"].as_array().unwrap().len(), 2);
        // Every state word the page switches on.
        for (state, word) in [
            (MachineState::Connecting, "connecting"),
            (MachineState::Connected, "connected"),
            (MachineState::Offline, "offline"),
            (MachineState::Unavailable, "unavailable"),
            (MachineState::NeedsPairing, "needsPairing"),
            (MachineState::Stopped, "stopped"),
        ] {
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::Value::from(word),
                "{state:?}"
            );
        }
    }

    /// A machine reachable over the LAN whose overlay route does not answer
    /// carries the words to act on: the same hint `rustcode machines list`
    /// prints (#3202).
    #[test]
    fn a_machine_reachable_one_way_says_why_the_other_route_is_not_used() {
        use rustcode_client::{OverlayProvider, RouteFailure, RouteReport};
        let mut saved = machine();
        let mut overlay = Route::with_kind(RouteKind::Vpn, "100.90.16.237:17879");
        overlay.provider = Some(OverlayProvider::Netbird);
        saved.routes.push(overlay.clone());
        let in_use = Route::new("192.168.1.20:17879");
        // This computer is on NetBird, so its route to the machine can answer.
        let local: Vec<std::net::IpAddr> = ["192.168.1.20", "100.92.13.44"]
            .iter()
            .map(|address| address.parse().unwrap())
            .collect();
        let hint = RouteReport {
            routes: &saved.routes,
            in_use: &in_use,
            failures: &[RouteFailure {
                route: overlay,
                reason: "connection timed out".into(),
                refusal: None,
            }],
            local: &local,
        }
        .hint()
        .expect("a hint");
        let live = Live {
            state: MachineState::Connected,
            route: Some(in_use),
            hint: Some(hint.clone()),
            ..Live::new()
        };
        let json = serde_json::to_value(view(&saved, Some(&live))).unwrap();
        assert_eq!(json["state"], "connected");
        assert_eq!(json["hint"], hint);
        assert_eq!(json["routes"][1]["kind"], "vpn");
    }

    /// The Add sheet lists a saved machine's overlay address, which no browse
    /// on the LAN can hear: its own handshake is where the address came from.
    #[test]
    fn the_add_sheet_lists_the_overlay_address_a_handshake_reported() {
        let peers = overlay_peers(&[saved_view("gw-8e1f4b7a5c2d4e6f")], &[]);
        assert_eq!(peers.len(), 1, "one row per machine");
        let json = serde_json::to_value(&peers[0]).unwrap();
        assert_eq!(json["source"], "overlay");
        assert_eq!(json["name"], "Lars's MacBook Pro");
        assert_eq!(json["address"], "100.90.16.237:17879");
        assert_eq!(
            json["routeKind"], "netbird",
            "the product the machine named, not the generic VPN"
        );
        assert_eq!(json["platform"], "macos");
        assert_eq!(json["version"], "0.96.0");
        assert_eq!(json["saved"], true);
        assert_eq!(
            json["hint"],
            "Not reachable over NetBird (100.90.16.237:17879)."
        );
        // Bonjour found it too: the LAN row is the one, not a second opinion.
        let record = DiscoveredMachine {
            name: "Lars's MacBook Pro".into(),
            port: 17879,
            addresses: vec!["192.168.1.20".parse().unwrap()],
            gateway_id: Some("gw-8e1f4b7a5c2d4e6f".into()),
            protocol: Some("1".into()),
            caps: vec!["desktops".into()],
        };
        let known = [saved_view("gw-8e1f4b7a5c2d4e6f")];
        assert!(
            overlay_peers(&known, &[discovered(&record, &known)]).is_empty(),
            "the page merges both sources by host id"
        );
        let bonjour = serde_json::to_value(discovered(&record, &known)).unwrap();
        assert_eq!(bonjour["source"], "bonjour");
        assert_eq!(bonjour["address"], "192.168.1.20:17879");
        assert_eq!(bonjour["routeKind"], "lan");
        // What the saved record knows travels with the Bonjour row.
        assert_eq!(bonjour["version"], "0.96.0");
        assert_eq!(bonjour["platform"], "macos");
        assert_eq!(bonjour["saved"], true);
    }

    fn record(gateway_id: Option<&str>, address: &str, port: u16) -> DiscoveredMachine {
        DiscoveredMachine {
            name: "This Mac".into(),
            port,
            addresses: vec![address.parse().unwrap()],
            gateway_id: gateway_id.map(Into::into),
            protocol: Some("1".into()),
            caps: vec!["desktops".into()],
        }
    }

    /// Bonjour hears this computer's own gateway too: it is not a machine to
    /// ask to connect (#3432).
    #[test]
    fn this_computer_is_left_out_of_the_machines_found() {
        let this = ThisComputer {
            gateway_id: Some("gw-own".into()),
            port: 17879,
            owns: |address| address == "192.168.1.5".parse::<IpAddr>().unwrap(),
        };
        // By gateway id, whatever address it announced.
        assert!(this.is(&record(Some("gw-own"), "192.168.1.99", 17879)));
        // By address: one of this computer's own, on its gateway port.
        assert!(this.is(&record(None, "192.168.1.5", 17879)));
        assert!(this.is(&record(Some("gw-other"), "192.168.1.5", 17879)));
        // Another computer, and another gateway on this one's address.
        assert!(!this.is(&record(Some("gw-other"), "192.168.1.20", 17879)));
        assert!(!this.is(&record(Some("gw-other"), "192.168.1.5", 18000)));
    }

    /// A machine with nothing but a LAN route is not offered as an overlay
    /// peer: the browse is what finds it.
    #[test]
    fn a_lan_only_machine_is_not_an_overlay_peer() {
        assert!(overlay_peers(&[view(&machine(), None)], &[]).is_empty());
    }

    /// What the page receives while a machine is asked: a tagged event the
    /// webview can switch on, with `camelCase` fields.
    #[test]
    fn pairing_events_are_tagged_for_the_page() {
        assert_eq!(
            serde_json::to_string(&PairingEvent::Waiting {
                ask_id: "ask-8f1c2d3e".into(),
                host: "Lars's MacBook Pro".into()
            })
            .unwrap(),
            r#"{"type":"waiting","askId":"ask-8f1c2d3e","host":"Lars's MacBook Pro"}"#
        );
        assert_eq!(
            serde_json::to_string(&PairingEvent::Approved {
                ask_id: "ask-8f1c2d3e".into(),
                code_length: 8,
                expires_in_secs: 120,
            })
            .unwrap(),
            r#"{"type":"approved","askId":"ask-8f1c2d3e","codeLength":8,"expiresInSecs":120}"#
        );
        assert_eq!(
            serde_json::to_string(&PairingEvent::Refused {
                message: "denied".into()
            })
            .unwrap(),
            r#"{"type":"refused","message":"denied"}"#
        );
        assert_eq!(
            serde_json::to_string(&AskView {
                ask_id: "ask-8f1c".into(),
                code_length: 8,
                expires_in_secs: 120,
            })
            .unwrap(),
            r#"{"askId":"ask-8f1c","codeLength":8,"expiresInSecs":120}"#
        );
    }

    /// The ask id is the page's name for a request, used as a map key and a
    /// command argument: anything a map or a log should not hold is refused.
    #[test]
    fn an_ask_id_is_short_and_printable() {
        for usable in ["ask-1", "8f1c2d3e4a5b6c7d", "ask-mg3kq1-1_2"] {
            assert!(ensure_ask_id(usable).is_ok(), "{usable}");
        }
        for refused in ["", "ask 1", "ask\n1", "jänk", &"a".repeat(65)] {
            let error = ensure_ask_id(refused).unwrap_err();
            assert!(error.contains("ask id"), "{refused:?} gave {error}");
        }
    }

    /// A cancelled ask is refused, whether the page cancels while the owner is
    /// deciding (which wakes the wait) or right after the answer arrives.
    #[tokio::test]
    async fn a_cancelled_ask_never_keeps_a_socket() {
        let machines = Arc::new(Machines::default());
        let cancel = Arc::new(Notify::new());
        machines
            .asks
            .lock()
            .unwrap()
            .insert("ask-1".to_owned(), Arc::clone(&cancel));
        // The page cancels: the signal is taken and the waiter woken.
        assert!(machines.asks.lock().unwrap().remove("ask-1").is_some());
        cancel.notify_one();
        // `notified()` completes on the stored permit, so a select already
        // registered or not yet reached both see it.
        tokio::time::timeout(Duration::from_secs(1), cancel.notified())
            .await
            .expect("the cancel wakes a waiter");
        // Nothing was stored for a code nobody will type.
        assert!(machines.approved.lock().unwrap().get("ask-1").is_none());
        assert!(machines.asks.lock().unwrap().is_empty());
    }

    /// A discovered machine offers its LAN address and says whether it is
    /// already saved, so the page does not offer it twice.
    #[test]
    fn a_discovered_machine_says_whether_it_is_saved() {
        let found = DiscoveredMachine {
            name: "Studio".into(),
            port: 17879,
            addresses: vec!["192.168.1.20".parse().unwrap()],
            gateway_id: Some("gw-8e1f4b7a".into()),
            protocol: Some("1".into()),
            caps: vec!["desktops".into()],
        };
        let view = discovered(&found, &[saved_view("gw-8e1f4b7a")]);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["name"], "Studio");
        assert_eq!(json["address"], "192.168.1.20:17879");
        assert_eq!(json["source"], "bonjour");
        assert_eq!(json["routeKind"], "lan");
        assert_eq!(json["saved"], true);
        assert_eq!(
            serde_json::to_value(discovered(&found, &[])).unwrap()["saved"],
            false
        );
    }
}
