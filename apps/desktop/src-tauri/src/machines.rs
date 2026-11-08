//! Settings › Machines: the other computers this app is paired with.
//!
//! The webview cannot dial a machine itself (the gateway refuses browser
//! `Origin`s, and the pairing token is in the operating system credential
//! store), so this module owns the connection to each saved machine: one
//! supervisor per machine that connects over the first route that answers,
//! retries with jittered backoff, and relays the machine's ACP frames to the
//! webview over a Tauri Channel in the same shape the local engine uses
//! (`engine.rs`). `rustcode-client` holds the registry, the tokens, the
//! pairing flow and the WebSocket; nothing here hands a token to the webview.
//!
//! Names follow docs/terminology.md: a **machine** is a computer, Settings ›
//! Machines lists them, and the server is what runs on each one.

use crate::engine::EngineEvent;
use anyhow::Error;
use rustcode_client::machine::{machine_from_pairing, normalize_code, Connector};
use rustcode_client::{
    browse_async, Approved, ClientKind, DiscoveredMachine, Machine, MachineRegistry, Route,
    TokenStore, Unreachable,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;

/// How long Bonjour listens when Settings › Machines asks for the machines on
/// this network.
pub const DISCOVERY: Duration = Duration::from_secs(4);

/// First wait before a failed machine is tried again, and the longest one.
pub const BACKOFF_START: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(300);

/// A connection that stayed up this long resets the backoff: a machine that
/// worked and then dropped is tried again after a second, not after five
/// minutes.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);

/// Where a machine stands, as the page shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MachineState {
    /// A connection is being made now.
    Connecting,
    Connected,
    /// Not reachable; the supervisor tries again after `retryInSecs`.
    Offline,
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
    /// The network itself encrypts the traffic (Tailscale, a VPN); a plain LAN
    /// route does not.
    encrypted: bool,
}

impl RouteView {
    fn of(route: &Route) -> Self {
        Self {
            kind: route.kind.as_str(),
            address: route.address.clone(),
            encrypted: route.kind.is_encrypted(),
        }
    }
}

/// A saved machine and its live state.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineView {
    /// The machine's `host_id`, which is what every command takes.
    id: String,
    name: String,
    /// `macos`, `windows`, `linux`, or `unknown` for a machine that does not
    /// report one yet.
    platform: String,
    main: bool,
    /// Every route saved for it, in the order they are tried.
    routes: Vec<RouteView>,
    /// The route this connection came up over, or the one it will try first.
    route: Option<RouteView>,
    state: MachineState,
    /// Why it is not connected, in words a person can act on.
    detail: Option<String>,
    /// Seconds until the next attempt, while it waits.
    retry_in_secs: Option<u64>,
    /// RFC 3339, as the registry stores it.
    added_at: String,
    last_connected_at: Option<String>,
}

/// A machine Bonjour found on this network.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredView {
    name: String,
    /// The address to dial: a LAN one when there is one.
    address: Option<String>,
    addresses: Vec<String>,
    gateway_id: Option<String>,
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
    /// The request is in front of the owner of that machine. Its id names it
    /// for `machines_cancel`.
    Waiting { request_id: String, host: String },
    /// The owner accepted; the code the machine now shows is what pairs it.
    Approved {
        request_id: String,
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
    request_id: String,
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
    detail: Option<String>,
    retry_in_secs: Option<u64>,
}

impl Live {
    fn new() -> Self {
        Self {
            outbox: None,
            task: None,
            state: MachineState::Stopped,
            route: None,
            detail: None,
            retry_in_secs: None,
        }
    }
}

/// The app's machines: the supervisors it runs and the connection requests
/// whose code the user is about to type.
#[derive(Default)]
pub struct Machines {
    live: Mutex<HashMap<String, Live>>,
    approved: Mutex<HashMap<String, PendingApproval>>,
    /// Requests the page cancelled. A request id arrives before the owner
    /// answers, so the answer can still be dropped when it comes.
    cancelled: Mutex<Vec<String>>,
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
    fn tokens(&self, config_dir: &Path) -> Result<Arc<TokenStore>, String> {
        if let Some(tokens) = self.tokens.lock().unwrap().as_ref() {
            return Ok(Arc::clone(tokens));
        }
        let tokens = Arc::new(TokenStore::open(config_dir).map_err(chain)?);
        *self.tokens.lock().unwrap() = Some(Arc::clone(&tokens));
        Ok(tokens)
    }

    fn connector(config_dir: &Path) -> Result<Connector, String> {
        Connector::new(config_dir, ClientKind::Desktop).map_err(chain)
    }

    /// The saved machines, main first, each with its live state.
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
        {
            let mut live = self.live.lock().unwrap();
            let entry = live.entry(id.to_owned()).or_insert_with(Live::new);
            if let Some(task) = entry.task.take() {
                task.abort();
            }
            entry.outbox = Some(outbox);
            entry.state = MachineState::Connecting;
            entry.route = None;
            entry.detail = None;
            entry.retry_in_secs = None;
        }
        let machines = Arc::clone(self);
        let target = id.to_owned();
        let task = tauri::async_runtime::spawn({
            let target = target.clone();
            async move {
                supervise(machines, target, machine, channel, inbox).await;
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
        name: machine.name.clone(),
        platform: machine.platform.clone(),
        main: machine.main,
        routes: machine.routes.iter().map(RouteView::of).collect(),
        route: live
            .and_then(|live| live.route.as_ref())
            .map(RouteView::of)
            .or_else(|| machine.preferred_route().map(RouteView::of)),
        state: live.map_or(MachineState::Stopped, |live| live.state),
        detail: live.and_then(|live| live.detail.clone()),
        retry_in_secs: live.and_then(|live| live.retry_in_secs),
        added_at: machine.added_at.to_rfc3339(),
        last_connected_at: machine.last_connected_at.map(|at| at.to_rfc3339()),
    }
}

// ---------------------------------------------------------------- backoff

/// The longest wait after `attempt` failures: 1, 2, 4, 8, 16, 32, 64, 128,
/// 256, then five minutes. `attempt` counts the failures so far.
pub fn backoff_cap(attempt: u32) -> Duration {
    let seconds = BACKOFF_START
        .as_secs()
        .saturating_mul(1u64.checked_shl(attempt.min(16)).unwrap_or(u64::MAX / 2));
    Duration::from_secs(seconds).min(BACKOFF_MAX)
}

/// The wait before the next attempt: at most [`backoff_cap`] and at least
/// half of it, spread by `jitter` in `0.0..1.0`, so several machines that lost
/// the same network do not come back in the same second.
pub fn backoff_delay(attempt: u32, jitter: f64) -> Duration {
    let cap = backoff_cap(attempt);
    let half = cap.as_secs_f64() / 2.0;
    Duration::from_secs_f64(half + half * jitter.clamp(0.0, 1.0))
}

/// Cheap jitter without a dependency: the sub-second clock, spread over one.
fn jitter() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

// ---------------------------------------------------------------- supervisor

/// How one attempt ended.
enum End {
    /// The machine closed the channel, or this side did.
    Closed,
    /// The machine could not be reached, or the ACP channel could not open.
    Failed(Error),
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
        match relay_once(&machines, &config_dir, &id, &channel, &mut inbox).await {
            End::Removed => return,
            End::Closed => {}
            End::Failed(error) => {
                let message = format!("{error:#}");
                if needs_pairing(&error) {
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
                emit_log(
                    &channel,
                    format!("desktop: {} is not reachable: {message}", machine.name),
                );
            }
        }
        // A connection that worked for a while deserves a fresh schedule.
        if started.elapsed() >= STABLE_AFTER {
            attempt = 0;
        }
        let delay = backoff_delay(attempt, jitter());
        attempt = attempt.saturating_add(1);
        let secs = delay.as_secs().max(1);
        set_state(
            &machines,
            &id,
            MachineState::Offline,
            Some(format!("Not connected. Retrying in {secs} s.")),
            Some(secs),
        );
        emit_closed(&channel, &format!("Waiting {secs} s before trying again."));
        tokio::time::sleep(delay).await;
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
        Err(error) => return End::Failed(error),
    };
    let tokens = match machines.tokens(config_dir) {
        Ok(tokens) => tokens,
        Err(error) => return End::Failed(anyhow::anyhow!(error)),
    };
    let connector = match Machines::connector(config_dir) {
        Ok(connector) => connector,
        Err(error) => return End::Failed(anyhow::anyhow!(error)),
    };
    let (session, route) = match connector.connect(&machine, &tokens).await {
        Ok(connected) => connected,
        Err(error) => return End::Failed(error),
    };
    if let Ok(mut registry) = MachineRegistry::load(config_dir) {
        let _ = registry.mark_connected(&machine.host_id, chrono::Utc::now());
    }
    let mut acp = match session.acp().await {
        Ok(acp) => acp,
        Err(error) => return End::Failed(error),
    };
    set_connected(machines, id, &route);
    let _ = channel.send(EngineEvent::Open {
        engine: format!("{} ({})", machine.name, route.address),
    });
    loop {
        // One step per pass: read the machine's next frame, or send a line the
        // webview wrote. The read is cancelled when a line goes first, so the
        // frame branch is polled first and a frame that is already there is
        // never dropped.
        enum Step {
            Frame(Result<Option<serde_json::Value>, Error>),
            Line(Option<String>),
        }
        let step = {
            let read = acp.next();
            tokio::pin!(read);
            tokio::select! {
                biased;
                frame = &mut read => Step::Frame(frame),
                line = inbox.recv() => Step::Line(line),
            }
        };
        match step {
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
            Step::Frame(Err(error)) => return End::Failed(error),
            Step::Line(Some(line)) => {
                // A line the webview wrote; only JSON reaches a machine.
                let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if let Err(error) = acp.send(frame).await {
                    return End::Failed(error);
                }
            }
            Step::Line(None) => return End::Closed,
        }
    }
}

/// Whether pairing again is what would fix this failure.
pub fn needs_pairing(error: &Error) -> bool {
    if error
        .downcast_ref::<Unreachable>()
        .is_some_and(Unreachable::needs_new_pairing)
    {
        return true;
    }
    // `Session::check` and the token lookup report a device record another
    // pairing replaced, or a token that is gone, as plain errors.
    let text = format!("{error:#}");
    text.contains("pair with it again")
        || text.contains("replaced this computer's device record")
        || text.contains("no pairing token")
}

fn set_connected(machines: &Arc<Machines>, id: &str, route: &Route) {
    let mut live = machines.live.lock().unwrap();
    if let Some(entry) = live.get_mut(id) {
        entry.state = MachineState::Connected;
        entry.route = Some(route.clone());
        entry.detail = None;
        entry.retry_in_secs = None;
    }
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

/// The machines on this network that take a desktop client. Bonjour, so it
/// takes a few seconds.
#[tauri::command]
pub async fn machines_discover(
    machines: tauri::State<'_, Arc<Machines>>,
) -> Result<Vec<DiscoveredView>, String> {
    let saved: Vec<String> = machines
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|machine| machine.id)
        .collect();
    let found = browse_async(DISCOVERY)
        .await
        .map_err(|error| format!("Could not look for machines on this network: {error}"))?;
    Ok(found
        .iter()
        .map(|machine| discovered(machine, &saved))
        .collect())
}

fn discovered(machine: &DiscoveredMachine, saved: &[String]) -> DiscoveredView {
    DiscoveredView {
        name: machine.name.clone(),
        address: machine.address(),
        addresses: machine.addresses.iter().map(ToString::to_string).collect(),
        gateway_id: machine.gateway_id.clone(),
        saved: machine
            .gateway_id
            .as_ref()
            .is_some_and(|id| saved.iter().any(|saved| saved == id)),
    }
}

/// Ask `address` to let this app in (`Settings › Machines › Add machine`).
///
/// The owner answers on that machine; the accepted request waits here for its
/// code. A denial or a request nobody answered comes back as this command's
/// error, in words the page shows where "Waiting for ‹host›…" was.
#[tauri::command]
pub async fn machines_ask(
    machines: tauri::State<'_, Arc<Machines>>,
    address: String,
    name: Option<String>,
    on_event: Channel<PairingEvent>,
) -> Result<AskView, String> {
    let config_dir = config_dir()?;
    let connector = Machines::connector(&config_dir)?;
    let label = name.clone().unwrap_or_else(|| address.clone());
    let ask = connector
        .ask(&address)
        .await
        .map_err(|error| format!("{error:#}"))?;
    let request_id = ask.request_id().to_owned();
    on_event
        .send(PairingEvent::Waiting {
            request_id: request_id.clone(),
            host: label.clone(),
        })
        .ok();
    let approved = match ask.wait(rustcode_client::machine::APPROVAL_TIMEOUT).await {
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
    // Cancelled while the owner was deciding: drop the socket instead of
    // keeping it open for a code nobody will type.
    if machines.cancelled.lock().unwrap().contains(&request_id) {
        return Err("The request was cancelled.".to_owned());
    }
    let answer = AskView {
        request_id: request_id.clone(),
        code_length: approved.code_length(),
        expires_in_secs: approved.expires_in().as_secs(),
    };
    on_event
        .send(PairingEvent::Approved {
            request_id: request_id.clone(),
            code_length: answer.code_length,
            expires_in_secs: answer.expires_in_secs,
        })
        .ok();
    machines.approved.lock().unwrap().insert(
        request_id,
        PendingApproval {
            approved,
            address,
            name,
        },
    );
    Ok(answer)
}

/// The code the machine shows, typed here: the machine is saved, its token
/// kept, and its supervisor can start.
#[tauri::command]
pub async fn machines_pair(
    machines: tauri::State<'_, Arc<Machines>>,
    request_id: String,
    code: String,
) -> Result<MachineView, String> {
    let pending = machines
        .approved
        .lock()
        .unwrap()
        .remove(&request_id)
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
    // again", while a token whose machine was not saved is unreachable.
    let tokens = machines.tokens(&config_dir)?;
    tokens
        .set(&machine.host_id, &paired.secret)
        .map_err(|error| {
            format!("The machine is paired, but its token could not be stored: {error:#}")
        })?;
    let mut registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    if let Err(error) = registry.insert(machine.clone()) {
        let _ = tokens.remove(&machine.host_id);
        return Err(format!(
            "The machine is paired, but could not be saved: {error:#}"
        ));
    }
    let _ = registry.mark_connected(&machine.host_id, chrono::Utc::now());
    let connecting = Live {
        state: MachineState::Connecting,
        ..Live::new()
    };
    Ok(view(&machine, Some(&connecting)))
}

/// Stop waiting for a machine's owner. A request that was already accepted
/// has its socket dropped here; a request still waiting for an answer is
/// dropped when it arrives.
#[tauri::command(async)]
pub fn machines_cancel(
    machines: tauri::State<'_, Arc<Machines>>,
    request_id: String,
) -> Result<(), String> {
    machines.approved.lock().unwrap().remove(&request_id);
    let mut cancelled = machines.cancelled.lock().unwrap();
    cancelled.push(request_id);
    // Only the open requests matter; a long session cannot grow this without bound.
    let excess = cancelled.len().saturating_sub(16);
    cancelled.drain(..excess);
    Ok(())
}

/// Forget a machine and its token. It still lists this computer until it is
/// revoked there.
#[tauri::command(async)]
pub fn machines_remove(
    machines: tauri::State<'_, Arc<Machines>>,
    id: String,
) -> Result<(), String> {
    let config_dir = config_dir()?;
    let mut registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    let machine = registry
        .get(&id)
        .ok_or("That machine is no longer saved.")?
        .clone();
    if let Some(mut entry) = machines.live.lock().unwrap().remove(&machine.host_id) {
        if let Some(task) = entry.task.take() {
            task.abort();
        }
    }
    registry.remove(&machine.host_id).map_err(chain)?;
    // The record is gone, which is what was asked for; a token that cannot be
    // deleted is unusable without it, so it is not an error.
    if let Ok(tokens) = machines.tokens(&config_dir) {
        let _ = tokens.remove(&machine.host_id);
    }
    Ok(())
}

/// Make `id` the main machine: listed first, and the default **Run on**.
#[tauri::command(async)]
pub fn machines_set_main(id: String) -> Result<(), String> {
    let config_dir = config_dir()?;
    let mut registry = MachineRegistry::load(&config_dir).map_err(chain)?;
    registry.set_main(&id).map_err(chain)
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
        entry.detail = None;
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

    /// The schedule the page's "Retrying in N s" comes from: 1, 2, 4, 8, 16,
    /// 32, 64, 128, 256, then five minutes, and it never goes back down.
    #[test]
    fn the_backoff_doubles_to_five_minutes_and_stops() {
        let caps: Vec<u64> = (0..12)
            .map(|attempt| backoff_cap(attempt).as_secs())
            .collect();
        assert_eq!(
            caps,
            [1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300, 300],
            "a long outage waits five minutes between attempts"
        );
        for attempt in 1..12 {
            assert!(backoff_cap(attempt) >= backoff_cap(attempt - 1));
        }
        // Nothing overflows however long the outage lasts.
        assert_eq!(backoff_cap(u32::MAX), BACKOFF_MAX);
    }

    /// Jitter spreads machines that lost the same network over the second
    /// half of the window, never below the first half and never past the cap.
    #[test]
    fn jitter_stays_between_half_the_cap_and_the_cap() {
        for attempt in 0..10 {
            let cap = backoff_cap(attempt);
            assert_eq!(backoff_delay(attempt, 0.0), cap / 2);
            assert_eq!(backoff_delay(attempt, 1.0), cap);
            for step in 0..10 {
                let delay = backoff_delay(attempt, f64::from(step) / 10.0);
                assert!(delay >= cap / 2 && delay <= cap, "{delay:?} for {attempt}");
            }
            // A jitter outside 0..1 cannot push it out of the window.
            assert_eq!(backoff_delay(attempt, -1.0), backoff_delay(attempt, 0.0));
            assert_eq!(backoff_delay(attempt, 4.0), backoff_delay(attempt, 1.0));
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
        });
        assert!(needs_pairing(&revoked));
        let replaced =
            Error::msg("Studio replaced this computer's device record; pair with it again");
        assert!(needs_pairing(&replaced));
        let unreachable = Error::new(Unreachable {
            machine: "Studio".into(),
            attempts: vec!["192.168.1.20:17879: connection refused".into()],
            refused: None,
        });
        assert!(!needs_pairing(&unreachable));
        assert!(!needs_pairing(&Error::msg(
            "no frame from the machine within 5s"
        )));
    }

    /// The page reads these names: camelCase fields, and a state per word it
    /// switches on.
    #[test]
    fn a_machine_view_is_what_the_page_reads() {
        let mut saved = machine();
        saved.platform = "unknown".into();
        saved.main = true;
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
        assert_eq!(
            json["main"], true,
            "the first machine saved is the main one"
        );
        assert_eq!(json["routes"][0]["kind"], "lan");
        assert_eq!(json["routes"][0]["address"], "192.168.1.20:17879");
        assert_eq!(json["routes"][0]["encrypted"], false);
        // Offline, the row still says which route it will try.
        assert_eq!(json["route"]["address"], "192.168.1.20:17879");
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

    /// What the page receives while a machine is asked: a tagged event the
    /// webview can switch on, with `camelCase` fields.
    #[test]
    fn pairing_events_are_tagged_for_the_page() {
        assert_eq!(
            serde_json::to_string(&PairingEvent::Waiting {
                request_id: "8f1c2d3e4a5b6c7d".into(),
                host: "Lars's MacBook Pro".into()
            })
            .unwrap(),
            r#"{"type":"waiting","requestId":"8f1c2d3e4a5b6c7d","host":"Lars's MacBook Pro"}"#
        );
        assert_eq!(
            serde_json::to_string(&PairingEvent::Approved {
                request_id: "8f1c2d3e4a5b6c7d".into(),
                code_length: 8,
                expires_in_secs: 120,
            })
            .unwrap(),
            r#"{"type":"approved","requestId":"8f1c2d3e4a5b6c7d","codeLength":8,"expiresInSecs":120}"#
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
                request_id: "8f1c".into(),
                code_length: 8,
                expires_in_secs: 120,
            })
            .unwrap(),
            r#"{"requestId":"8f1c","codeLength":8,"expiresInSecs":120}"#
        );
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
        let view = discovered(&found, &["gw-8e1f4b7a".to_string()]);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["name"], "Studio");
        assert_eq!(json["address"], "192.168.1.20:17879");
        assert_eq!(json["saved"], true);
        assert_eq!(
            serde_json::to_value(discovered(&found, &[])).unwrap()["saved"],
            false
        );
    }
}
