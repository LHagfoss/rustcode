//! The live session registry and router (issue #1909, routing half).
//!
//! One [`SessionHub`] sits between the two kinds of connection a gateway
//! holds: authenticated devices, whose frames arrive through
//! [`FrameRouter`], and session owners, whose frames arrive from the owner
//! socket ([`super::owner_ipc`]). It keeps the registry of shared sessions,
//! serves the session list and subscriptions itself, and forwards every other
//! operation to the owner of the session it names.
//!
//! Everything lives behind one mutex and nothing here waits: frames towards a
//! device go through its bounded [`FrameSink`], frames towards an owner
//! through a bounded queue, and whatever does not fit is retried from
//! [`SessionHub::tick`]. A device that cannot keep up is resynchronised or
//! closed; it never holds back an owner or another device.
//!
//! Ordering. An owner numbers its events; the hub keeps the last snapshot it
//! was given and, in a byte-bounded ring, every event after it. A subscriber
//! is a cursor into that ring. Attaching, resuming and catching up are the
//! same step: under the lock, pick the position, answer, and deliver from
//! there, so no event can fall between the snapshot and the subscription or
//! be delivered on both sides of it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::owner_ipc::{GatewayFrame, OwnerFrame};
use super::router::{DeviceContext, FrameRouter, FrameSink};
use crate::remote::owner::GatewayLinkStatus;
use crate::remote::{
    MAX_REMOTE_FRAME_BYTES, OwnerHealth, REMOTE_PROTOCOL_VERSION, ReceiptState, RemoteError,
    RemoteErrorCode, RemoteEvent, RemoteEventFrame, RemoteFrame, RemoteOperation, RemoteRequest,
    RemoteResponse, RemoteResult, RemoteSessionInfo, RemoteSessionsFrame, RemoteSnapshot,
    ResumeCursor, ResyncReason, SessionCloseReason, SessionRegistration, decode_request,
};

/// Resource bounds of one hub. The defaults are the production values; tests
/// shrink them to reach each bound quickly.
#[derive(Debug, Clone, Copy)]
pub struct HubLimits {
    /// Shared sessions at once.
    pub max_sessions: usize,
    /// Encoded bytes of events kept per session for replay.
    pub replay_bytes: usize,
    /// Frames queued towards one owner before requests are refused.
    pub owner_queue: usize,
    /// Requests one connection may have waiting on owners.
    pub max_in_flight: usize,
    /// Slots of a device's queue kept free for responses: events wait
    /// rather than use them.
    pub reserved_slots: usize,
    /// How long an owner may take to answer a request.
    pub request_timeout: Duration,
    /// How long an attach may wait for the owner's snapshot.
    pub attach_timeout: Duration,
    /// Owner silence after which the session is listed as unresponsive.
    pub owner_unresponsive: Duration,
    /// Owner silence after which the registration is dropped.
    pub owner_timeout: Duration,
    /// How often [`SessionHub::tick`] runs.
    pub tick: Duration,
}

impl Default for HubLimits {
    fn default() -> Self {
        Self {
            max_sessions: 32,
            replay_bytes: 4 * 1024 * 1024,
            owner_queue: 64,
            max_in_flight: 16,
            reserved_slots: 8,
            request_timeout: Duration::from_secs(30),
            attach_timeout: Duration::from_secs(10),
            owner_unresponsive: Duration::from_secs(15),
            owner_timeout: Duration::from_secs(45),
            tick: Duration::from_millis(50),
        }
    }
}

/// What the gateway process knows about itself once it is bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubIdentity {
    /// Stable for the host's configuration directory.
    pub gateway_id: String,
    /// Changes on every gateway start; event cursors are scoped to it.
    pub instance_id: String,
    pub advertised_address: String,
    pub loopback_only: bool,
}

/// One row of `rustcode remote status`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub attached_devices: Vec<String>,
}

/// Handle of one registered owner, as [`SessionHub::register_owner`] returns it.
pub struct OwnerRegistration {
    pub owner_id: u64,
    /// First frame to write to the owner.
    pub registered: GatewayFrame,
    /// Every later frame for the owner.
    pub frames: mpsc::Receiver<GatewayFrame>,
}

/// Events after the cached snapshot, oldest first, contiguous by sequence.
struct ReplayRing {
    events: VecDeque<Arc<str>>,
    bytes: usize,
    /// Sequence of the event before the oldest one kept.
    floor: u64,
}

impl ReplayRing {
    fn starting_after(floor: u64) -> Self {
        Self {
            events: VecDeque::new(),
            bytes: 0,
            floor,
        }
    }

    fn push(&mut self, frame: Arc<str>, limit: usize) {
        self.bytes += frame.len();
        self.events.push_back(frame);
        // The newest event always stays, whatever its size.
        while self.bytes > limit && self.events.len() > 1 {
            let evicted = self.events.pop_front().expect("ring is not empty");
            self.bytes -= evicted.len();
            self.floor += 1;
        }
    }

    fn get(&self, sequence: u64) -> Option<&Arc<str>> {
        let index = sequence.checked_sub(self.floor + 1)?;
        self.events.get(usize::try_from(index).ok()?)
    }
}

/// An `attach_session` that has not been answered yet.
struct AttachWait {
    request_id: String,
    deadline: Instant,
    /// Why the cursor it carried could not be replayed.
    resync: Option<ResyncReason>,
}

struct Subscriber {
    device_name: String,
    sink: FrameSink,
    /// Sequence of the next event to deliver. `None` while the subscriber
    /// needs a snapshot first.
    next: Option<u64>,
    /// `resync_required` to send before that snapshot.
    notice: Option<ResyncReason>,
    attach: Option<AttachWait>,
}

struct Pending {
    connection_id: u64,
    sink: FrameSink,
    request_id: String,
    mutation: bool,
    deadline: Instant,
    /// Set when this is one leg of a `get_request_status` lookup.
    lookup: Option<u64>,
}

/// A `get_request_status` asked of one or several owners.
struct Lookup {
    sink: FrameSink,
    request_id: String,
    target_request_id: String,
    remaining: usize,
}

struct Session {
    owner_id: u64,
    epoch: u64,
    info: RemoteSessionInfo,
    to_owner: mpsc::Sender<GatewayFrame>,
    /// State at `cache.sequence`; `None` after events were lost, until the
    /// owner's resync snapshot arrives.
    cache: Option<Box<RemoteSnapshot>>,
    ring: ReplayRing,
    /// Sequence of the last event the owner produced, as far as known.
    last: u64,
    snapshot_requested: Option<Instant>,
    subscribers: HashMap<u64, Subscriber>,
    pending: HashMap<u64, Pending>,
    last_heard: Instant,
    status_sent: Option<GatewayLinkStatus>,
    /// A subscriber is waiting for room in its queue.
    backlog: bool,
}

struct Connection {
    device: DeviceContext,
    sink: FrameSink,
    list_subscribed: bool,
    list_dirty: bool,
    in_flight: usize,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Session>,
    connections: HashMap<u64, Connection>,
    lookups: HashMap<u64, Lookup>,
    next_id: u64,
}

pub struct SessionHub {
    limits: HubLimits,
    identity: OnceLock<HubIdentity>,
    state: Mutex<State>,
}

fn encode(frame: &RemoteFrame) -> String {
    serde_json::to_string(frame).expect("remote frames always serialize")
}

fn error(code: RemoteErrorCode, message: &str) -> RemoteResult {
    RemoteResult::Error(RemoteError::new(code, message))
}

/// Queue a response. A full queue closes the device as a slow consumer,
/// which is the foundation's policy for anything that is not an event.
fn respond(sink: &FrameSink, response: RemoteResponse) {
    let mut frame = encode(&RemoteFrame::Response(response.clone()));
    if frame.len() > MAX_REMOTE_FRAME_BYTES {
        let mut refusal = RemoteResponse::error(
            response.request_id,
            RemoteError::new(
                RemoteErrorCode::Internal,
                "the answer does not fit in one frame",
            ),
        );
        // What the owner did with a mutation stays true.
        refusal.receipt = response.receipt;
        frame = encode(&RemoteFrame::Response(refusal));
    }
    let _ = sink.send(frame);
}

fn reply(sink: &FrameSink, request_id: &str, result: RemoteResult) {
    respond(sink, RemoteResponse::new(request_id, result));
}

/// Answer for a request no owner will decide. Mutations carry a receipt:
/// `rejected` when nothing was forwarded, `unknown` when the owner may have
/// applied it.
fn refuse(
    sink: &FrameSink,
    request_id: &str,
    mutation: bool,
    code: RemoteErrorCode,
    message: &str,
) {
    let mut response = RemoteResponse::error(request_id, RemoteError::new(code, message));
    if mutation {
        response.receipt = Some(if code == RemoteErrorCode::OwnerUnavailable {
            ReceiptState::Unknown
        } else {
            ReceiptState::Rejected
        });
    }
    respond(sink, response);
}

impl Session {
    fn event_frame(&self, session_id: &str, sequence: u64, event: RemoteEvent) -> String {
        encode(&RemoteFrame::Event(RemoteEventFrame {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            session_id: session_id.to_owned(),
            registration_epoch: self.epoch,
            sequence,
            generation: self.cache.as_ref().map_or(0, |cache| cache.generation),
            event,
        }))
    }

    /// The cached snapshot, if every event after it is still in the ring.
    fn usable_cache(&self) -> Option<&RemoteSnapshot> {
        self.cache
            .as_deref()
            .filter(|cache| self.ring.floor <= cache.sequence)
    }

    /// Whether a client that applied everything up to `last_sequence` can be
    /// brought up to date from the ring alone.
    fn replayable(&self, cursor: &ResumeCursor) -> Result<(), ResyncReason> {
        let last_sequence = cursor.last_sequence;
        let Some(cache) = self.cache.as_deref() else {
            return Err(ResyncReason::SequenceGap);
        };
        if last_sequence < self.ring.floor {
            return Err(ResyncReason::Lagged);
        }
        // At the snapshot watermark, sequence alone cannot distinguish two
        // different cuts. A matching identity proves the client has this cut.
        let at_current_snapshot = last_sequence == cache.sequence
            && cursor.snapshot_id.is_some()
            && cursor.snapshot_id == cache.snapshot_id;
        if last_sequence < cache.sequence
            || (last_sequence == cache.sequence && !at_current_snapshot)
            || last_sequence > self.last
        {
            return Err(ResyncReason::SequenceGap);
        }
        Ok(())
    }

    fn request_snapshot(&mut self, now: Instant) {
        const RETRY: Duration = Duration::from_secs(2);
        if self
            .snapshot_requested
            .is_some_and(|asked| now.duration_since(asked) < RETRY)
        {
            return;
        }
        if self
            .to_owner
            .try_send(GatewayFrame::SnapshotRequested)
            .is_ok()
        {
            self.snapshot_requested = Some(now);
        }
    }

    /// Deliver to one subscriber whatever it is due and its queue has room
    /// for. Returns false when the subscription ended.
    fn serve(
        &mut self,
        session_id: &str,
        connection_id: u64,
        identity: &HubIdentity,
        limits: &HubLimits,
        now: Instant,
    ) -> bool {
        let Some(mut subscriber) = self.subscribers.remove(&connection_id) else {
            return false;
        };
        let room = |subscriber: &Subscriber, frames: usize| {
            subscriber.sink.free_slots() >= limits.reserved_slots + frames
        };
        let mut alive = !subscriber.sink.is_closed();
        while alive {
            match subscriber.next {
                None => {
                    if self.usable_cache().is_none() {
                        self.request_snapshot(now);
                        break;
                    }
                    if !room(&subscriber, 2) {
                        self.backlog = true;
                        break;
                    }
                    let cache = self.usable_cache().expect("checked above");
                    let watermark = cache.sequence;
                    let snapshot = Box::new(cache.clone());
                    let frame = if let Some(attach) = subscriber.attach.take() {
                        subscriber.notice = None;
                        let attached = fits(encode(&RemoteFrame::Response(RemoteResponse::new(
                            attach.request_id.clone(),
                            RemoteResult::Attached {
                                gateway_id: identity.gateway_id.clone(),
                                instance_id: Some(identity.instance_id.clone()),
                                resync: attach.resync,
                                snapshot,
                            },
                        ))));
                        match attached {
                            Some(frame) => frame,
                            None => {
                                alive = false;
                                encode(&RemoteFrame::Response(RemoteResponse::error(
                                    attach.request_id,
                                    RemoteError::new(
                                        RemoteErrorCode::Internal,
                                        "the session snapshot does not fit in one frame",
                                    ),
                                )))
                            }
                        }
                    } else {
                        if let Some(reason) = subscriber.notice.take() {
                            let notice = self.event_frame(
                                session_id,
                                watermark,
                                RemoteEvent::ResyncRequired { reason },
                            );
                            alive &= subscriber.sink.send(notice).is_ok();
                        }
                        match fits(self.event_frame(
                            session_id,
                            watermark,
                            RemoteEvent::Snapshot { snapshot },
                        )) {
                            Some(frame) => frame,
                            // The client was told to resync and attaches again.
                            None => {
                                alive = false;
                                break;
                            }
                        }
                    };
                    alive &= subscriber.sink.send(frame).is_ok();
                    subscriber.next = Some(watermark + 1);
                }
                Some(next) if next > self.last => break,
                Some(next) => match self.ring.get(next) {
                    Some(frame) => {
                        if !room(&subscriber, 1) {
                            self.backlog = true;
                            break;
                        }
                        alive &= subscriber.sink.send(frame.to_string()).is_ok();
                        subscriber.next = Some(next + 1);
                    }
                    // The ring moved past this subscriber.
                    None => {
                        subscriber.next = None;
                        subscriber.notice = Some(ResyncReason::Lagged);
                    }
                },
            }
        }
        if alive {
            self.subscribers.insert(connection_id, subscriber);
        }
        alive
    }

    fn serve_all(&mut self, session_id: &str, identity: &HubIdentity, limits: &HubLimits) {
        let now = Instant::now();
        self.backlog = false;
        let connections: Vec<u64> = self.subscribers.keys().copied().collect();
        for connection_id in connections {
            self.serve(session_id, connection_id, identity, limits, now);
        }
    }

    /// Every subscriber needs a snapshot before anything else.
    fn invalidate_subscribers(&mut self, reason: Option<ResyncReason>) {
        for subscriber in self.subscribers.values_mut() {
            if subscriber.next.take().is_some() || subscriber.notice.is_some() {
                subscriber.notice = reason.or(subscriber.notice);
            }
        }
    }

    fn attached_devices(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .subscribers
            .values()
            .map(|subscriber| subscriber.device_name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// The frame, unless it is larger than one frame may be.
fn fits(frame: String) -> Option<String> {
    (frame.len() <= MAX_REMOTE_FRAME_BYTES).then_some(frame)
}

impl SessionHub {
    /// A hub and the task that drives its timers. Call inside a runtime.
    pub fn start(limits: HubLimits) -> Arc<Self> {
        let hub = Arc::new(Self {
            limits,
            identity: OnceLock::new(),
            state: Mutex::new(State::default()),
        });
        let weak: Weak<Self> = Arc::downgrade(&hub);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(limits.tick);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                match weak.upgrade() {
                    Some(hub) => hub.tick(),
                    None => return,
                }
            }
        });
        hub
    }

    /// Record who this gateway is. Called once, when its listeners are bound.
    pub fn bind_identity(&self, identity: HubIdentity) {
        let _ = self.identity.set(identity);
    }

    fn identity(&self) -> &HubIdentity {
        static UNBOUND: OnceLock<HubIdentity> = OnceLock::new();
        self.identity.get().unwrap_or_else(|| {
            UNBOUND.get_or_init(|| HubIdentity {
                gateway_id: String::new(),
                instance_id: String::new(),
                advertised_address: String::new(),
                loopback_only: true,
            })
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("remote session hub is poisoned")
    }

    /// Shared sessions and the devices attached to each.
    pub fn session_info(&self, id: &str) -> Option<RemoteSessionInfo> {
        self.state()
            .sessions
            .get(id)
            .map(|session| session.info.clone())
    }

    pub fn summary(&self) -> Vec<SessionSummary> {
        let state = self.state();
        let mut sessions: Vec<SessionSummary> = state
            .sessions
            .iter()
            .map(|(session_id, session)| SessionSummary {
                session_id: session_id.clone(),
                title: session.info.title.clone(),
                attached_devices: session.attached_devices(),
            })
            .collect();
        sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        sessions
    }

    // ---- owners ---------------------------------------------------------

    /// Take a registration from an owner connection. `Err` is the reason to
    /// tell the owner; nothing was registered.
    pub fn register_owner(
        &self,
        registration: SessionRegistration,
        mut snapshot: Box<RemoteSnapshot>,
    ) -> Result<OwnerRegistration, String> {
        let mut state = self.state();
        if state.sessions.contains_key(&registration.session_id) {
            return Err("another terminal is already sharing this session".to_owned());
        }
        if state.sessions.len() >= self.limits.max_sessions {
            return Err("the gateway is at its limit of shared sessions".to_owned());
        }
        if registration.session_id.is_empty() || registration.session_id.len() > 256 {
            return Err("the session identifier is not acceptable".to_owned());
        }
        state.next_id += 1;
        let owner_id = state.next_id;
        let (to_owner, frames) = mpsc::channel(self.limits.owner_queue.max(1));
        let info = owned_info(&registration, snapshot.session.clone(), OwnerHealth::Live);
        snapshot.session = info.clone();
        snapshot.snapshot_id = Some(uuid::Uuid::new_v4().to_string());
        let mut session = Session {
            owner_id,
            epoch: registration.registration_epoch,
            info,
            to_owner,
            ring: ReplayRing::starting_after(snapshot.sequence),
            last: snapshot.sequence,
            cache: Some(snapshot),
            snapshot_requested: None,
            subscribers: HashMap::new(),
            pending: HashMap::new(),
            last_heard: Instant::now(),
            status_sent: None,
            backlog: false,
        };
        let status = self.link_status(&state, &session);
        session.status_sent = Some(status.clone());
        state.sessions.insert(registration.session_id, session);
        self.push_sessions(&mut state);
        let identity = self.identity();
        Ok(OwnerRegistration {
            owner_id,
            registered: GatewayFrame::Registered {
                gateway_id: identity.gateway_id.clone(),
                instance_id: identity.instance_id.clone(),
                status,
            },
            frames,
        })
    }

    /// One frame from a registered owner. Returns false when the
    /// registration is over and the connection should close.
    pub fn owner_frame(&self, owner_id: u64, frame: OwnerFrame) -> bool {
        let mut guard = self.state();
        let state = &mut *guard;
        let Some((session_id, session)) = state
            .sessions
            .iter_mut()
            .find(|(_, session)| session.owner_id == owner_id)
        else {
            return false;
        };
        let session_id = session_id.clone();
        let was_unresponsive = session.info.health != OwnerHealth::Live;
        session.last_heard = Instant::now();
        session.info.health = OwnerHealth::Live;
        if let Some(cache) = session.cache.as_mut() {
            if cache.session.health != OwnerHealth::Live {
                cache.snapshot_id = Some(uuid::Uuid::new_v4().to_string());
            }
            cache.session.health = OwnerHealth::Live;
        }
        let mut list_changed = was_unresponsive;
        match frame {
            // A second registration on one connection is not a thing.
            OwnerFrame::Register { .. } => {}
            OwnerFrame::Ping => {
                let _ = session.to_owner.try_send(GatewayFrame::Pong);
            }
            OwnerFrame::Event { frame } => {
                if frame.session_id != session_id || frame.registration_epoch != session.epoch {
                    return true;
                }
                let sequence = frame.sequence;
                if sequence <= session.last {
                    return true;
                }
                let encoded = encode(&RemoteFrame::Event(frame));
                let contiguous = sequence == session.last + 1 && session.cache.is_some();
                session.last = sequence;
                if contiguous && encoded.len() <= MAX_REMOTE_FRAME_BYTES {
                    session
                        .ring
                        .push(Arc::from(encoded), self.limits.replay_bytes);
                } else {
                    // Events were lost on the way here. Nothing held is
                    // complete any more; the owner's resync snapshot (asked
                    // for below, in case it is not already on its way)
                    // restores every subscriber.
                    session.cache = None;
                    session.ring = ReplayRing::starting_after(sequence);
                    session.invalidate_subscribers(Some(ResyncReason::SequenceGap));
                    session.request_snapshot(Instant::now());
                }
                session.serve_all(&session_id, self.identity(), &self.limits);
            }
            OwnerFrame::Snapshot {
                mut snapshot,
                resync,
            } => {
                if snapshot.sequence < session.last {
                    return true;
                }
                let lost = snapshot.sequence > session.last || session.cache.is_none();
                let registration = SessionRegistration {
                    session_id: session_id.clone(),
                    registration_epoch: session.epoch,
                };
                let info = owned_info(&registration, snapshot.session.clone(), OwnerHealth::Live);
                list_changed |= info != session.info;
                session.info = info.clone();
                snapshot.session = info;
                snapshot.snapshot_id = Some(uuid::Uuid::new_v4().to_string());
                session.last = snapshot.sequence;
                session.ring = ReplayRing::starting_after(snapshot.sequence);
                session.cache = Some(snapshot);
                session.snapshot_requested = None;
                session
                    .invalidate_subscribers(resync.or(lost.then_some(ResyncReason::SequenceGap)));
                session.serve_all(&session_id, self.identity(), &self.limits);
            }
            OwnerFrame::SessionInfo { info } => {
                let registration = SessionRegistration {
                    session_id: session_id.clone(),
                    registration_epoch: session.epoch,
                };
                let info = owned_info(&registration, info, OwnerHealth::Live);
                list_changed |= info != session.info;
                if let Some(cache) = session.cache.as_mut() {
                    if cache.session != info {
                        cache.snapshot_id = Some(uuid::Uuid::new_v4().to_string());
                    }
                    cache.session = info.clone();
                }
                session.info = info;
            }
            OwnerFrame::Reply { id, response } => {
                if let Some(pending) = session.pending.remove(&id) {
                    self.finish(state, pending, Some(response));
                }
            }
            OwnerFrame::Unregister { reason } => {
                self.close_session(state, &session_id, reason);
                return false;
            }
        }
        if list_changed {
            self.push_sessions(state);
        }
        true
    }

    /// The owner's connection ended. A registration that was not closed with
    /// a reason ends as `owner_exited`.
    pub fn owner_gone(&self, owner_id: u64) {
        let mut state = self.state();
        let session_id = state
            .sessions
            .iter()
            .find(|(_, session)| session.owner_id == owner_id)
            .map(|(session_id, _)| session_id.clone());
        if let Some(session_id) = session_id {
            self.close_session(&mut state, &session_id, SessionCloseReason::OwnerExited);
        }
    }

    /// Remove a session at once: tell its subscribers, answer what is still
    /// waiting on its owner, and update the list.
    fn close_session(&self, state: &mut State, session_id: &str, reason: SessionCloseReason) {
        let Some(mut session) = state.sessions.remove(session_id) else {
            return;
        };
        let closed = session.event_frame(
            session_id,
            session.last,
            RemoteEvent::SessionClosed { reason },
        );
        for (_, subscriber) in session.subscribers.drain() {
            match subscriber.attach {
                Some(attach) => reply(
                    &subscriber.sink,
                    &attach.request_id,
                    error(
                        RemoteErrorCode::StaleSession,
                        "the session stopped being shared",
                    ),
                ),
                // Unconditional: a device with no room left is closed as a
                // slow consumer rather than left believing the session lives.
                None => {
                    let _ = subscriber.sink.send(closed.clone());
                }
            }
        }
        let pending: Vec<Pending> = session
            .pending
            .drain()
            .map(|(_, pending)| pending)
            .collect();
        for pending in pending {
            self.finish(state, pending, None);
        }
        self.push_sessions(state);
    }

    /// Deliver the outcome of a forwarded request. `None` means the owner
    /// never answered.
    fn finish(&self, state: &mut State, pending: Pending, response: Option<RemoteResponse>) {
        if let Some(connection) = state.connections.get_mut(&pending.connection_id) {
            connection.in_flight = connection.in_flight.saturating_sub(1);
        }
        if let Some(lookup_id) = pending.lookup {
            let known = response.and_then(|response| match response.result {
                RemoteResult::RequestStatus {
                    receipt, result, ..
                } if receipt != ReceiptState::Unknown => Some((receipt, result)),
                _ => None,
            });
            let Some(lookup) = state.lookups.get_mut(&lookup_id) else {
                return;
            };
            lookup.remaining = lookup.remaining.saturating_sub(1);
            if known.is_none() && lookup.remaining > 0 {
                return;
            }
            let lookup = state.lookups.remove(&lookup_id).expect("lookup exists");
            let (receipt, result) = known.unwrap_or((ReceiptState::Unknown, None));
            reply(
                &lookup.sink,
                &lookup.request_id,
                RemoteResult::RequestStatus {
                    target_request_id: lookup.target_request_id,
                    receipt,
                    result,
                },
            );
            return;
        }
        match response {
            Some(mut response) => {
                response.request_id = pending.request_id;
                respond(&pending.sink, response);
            }
            None => refuse(
                &pending.sink,
                &pending.request_id,
                pending.mutation,
                RemoteErrorCode::OwnerUnavailable,
                "the session owner did not answer",
            ),
        }
    }

    fn link_status(&self, state: &State, session: &Session) -> GatewayLinkStatus {
        let identity = self.identity();
        let mut connected: Vec<String> = state
            .connections
            .values()
            .map(|connection| connection.device.device_name.clone())
            .collect();
        connected.sort();
        connected.dedup();
        GatewayLinkStatus {
            connected: true,
            advertised_address: identity.advertised_address.clone(),
            loopback_only: identity.loopback_only,
            attached_devices: session.attached_devices(),
            connected_devices: connected,
        }
    }

    // ---- devices --------------------------------------------------------

    fn sessions_of(state: &State) -> Vec<RemoteSessionInfo> {
        let mut sessions: Vec<RemoteSessionInfo> = state
            .sessions
            .values()
            .map(|session| session.info.clone())
            .collect();
        sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        sessions
    }

    fn sessions_frame(&self, state: &State) -> String {
        let identity = self.identity();
        encode(&RemoteFrame::Sessions(RemoteSessionsFrame {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            gateway_id: identity.gateway_id.clone(),
            instance_id: Some(identity.instance_id.clone()),
            sessions: Self::sessions_of(state),
        }))
    }

    /// Send the list to every subscribed device that has room; the others
    /// get it from [`SessionHub::tick`]. The list is state, so only the
    /// latest one matters.
    fn push_sessions(&self, state: &mut State) {
        let frame = self.sessions_frame(state);
        for connection in state.connections.values_mut() {
            if !connection.list_subscribed {
                continue;
            }
            if connection.sink.free_slots() > self.limits.reserved_slots {
                connection.list_dirty = connection.sink.send(frame.clone()).is_err();
            } else {
                connection.list_dirty = true;
            }
        }
    }

    fn attach(
        &self,
        state: &mut State,
        device: &DeviceContext,
        sink: &FrameSink,
        request: &RemoteRequest,
        resume: Option<&ResumeCursor>,
    ) {
        let identity = self.identity();
        let (Some(session_id), Some(epoch)) = (&request.session_id, request.registration_epoch)
        else {
            unreachable!("decode_request requires the session envelope");
        };
        let Some(session) = state.sessions.get_mut(session_id) else {
            return reply(
                sink,
                &request.request_id,
                error(RemoteErrorCode::NotFound, "no such shared session"),
            );
        };
        if session.epoch != epoch {
            return reply(
                sink,
                &request.request_id,
                error(
                    RemoteErrorCode::StaleSession,
                    "the session is shared under another registration",
                ),
            );
        }
        // Attaching again replaces the subscription this connection holds.
        session.subscribers.remove(&device.connection_id);
        let mut subscriber = Subscriber {
            device_name: device.device_name.clone(),
            sink: sink.clone(),
            next: None,
            notice: None,
            attach: None,
        };
        let replay = resume.map(|cursor| {
            if cursor.gateway_id != identity.gateway_id
                || cursor
                    .instance_id
                    .as_ref()
                    .is_some_and(|instance| *instance != identity.instance_id)
            {
                Err(ResyncReason::GatewayRestarted)
            } else {
                session.replayable(cursor)
            }
        });
        match (resume, replay) {
            (Some(cursor), Some(Ok(()))) => {
                reply(
                    sink,
                    &request.request_id,
                    RemoteResult::Resumed {
                        gateway_id: identity.gateway_id.clone(),
                        instance_id: Some(identity.instance_id.clone()),
                        next_sequence: cursor.last_sequence + 1,
                    },
                );
                subscriber.next = Some(cursor.last_sequence + 1);
            }
            (_, replay) => {
                subscriber.attach = Some(AttachWait {
                    request_id: request.request_id.clone(),
                    deadline: Instant::now() + self.limits.attach_timeout,
                    resync: replay.and_then(Result::err),
                });
            }
        }
        session.subscribers.insert(device.connection_id, subscriber);
        session.serve(
            session_id,
            device.connection_id,
            identity,
            &self.limits,
            Instant::now(),
        );
    }

    /// Hand a request to the owner of the session it names.
    fn forward(
        &self,
        state: &mut State,
        device: &DeviceContext,
        sink: &FrameSink,
        request: RemoteRequest,
    ) {
        let mutation = request.operation.is_mutation();
        let request_id = request.request_id.clone();
        let refuse = |code, message: &str| refuse(sink, &request_id, mutation, code, message);
        let in_flight = state
            .connections
            .get(&device.connection_id)
            .map_or(0, |connection| connection.in_flight);
        if in_flight >= self.limits.max_in_flight {
            return refuse(
                RemoteErrorCode::RateLimited,
                "too many requests are waiting on session owners",
            );
        }
        let Some(session) = request
            .session_id
            .as_ref()
            .and_then(|session_id| state.sessions.get_mut(session_id))
        else {
            return refuse(RemoteErrorCode::NotFound, "no such shared session");
        };
        if Some(session.epoch) != request.registration_epoch {
            return refuse(
                RemoteErrorCode::StaleSession,
                "the session is shared under another registration",
            );
        }
        state.next_id += 1;
        let id = state.next_id;
        let frame = GatewayFrame::Request {
            id,
            device_id: device.device_id.clone(),
            request,
        };
        if session.to_owner.try_send(frame).is_err() {
            return refuse(
                RemoteErrorCode::RateLimited,
                "the session owner is not keeping up; nothing was applied",
            );
        }
        session.pending.insert(
            id,
            Pending {
                connection_id: device.connection_id,
                sink: sink.clone(),
                request_id,
                mutation,
                deadline: Instant::now() + self.limits.request_timeout,
                lookup: None,
            },
        );
        if let Some(connection) = state.connections.get_mut(&device.connection_id) {
            connection.in_flight += 1;
        }
    }

    /// `get_request_status`: ask the owner that holds the receipt. With a
    /// session on the envelope that is one owner; without, every live owner
    /// is asked and the first that knows the request answers.
    fn lookup(
        &self,
        state: &mut State,
        device: &DeviceContext,
        sink: &FrameSink,
        request: RemoteRequest,
        target_request_id: String,
    ) {
        let targets: Vec<String> = match (&request.session_id, request.registration_epoch) {
            (Some(session_id), epoch) => state
                .sessions
                .get(session_id)
                .filter(|session| epoch.is_none_or(|epoch| epoch == session.epoch))
                .map(|_| session_id.clone())
                .into_iter()
                .collect(),
            (None, _) => state.sessions.keys().cloned().collect(),
        };
        state.next_id += 1;
        let lookup_id = state.next_id;
        let mut asked = 0;
        for session_id in targets {
            let session = state
                .sessions
                .get_mut(&session_id)
                .expect("target was just listed");
            state.next_id += 1;
            let id = state.next_id;
            let mut leg = request.clone();
            leg.session_id = Some(session_id);
            leg.registration_epoch = Some(session.epoch);
            let frame = GatewayFrame::Request {
                id,
                device_id: device.device_id.clone(),
                request: leg,
            };
            if session.to_owner.try_send(frame).is_err() {
                continue;
            }
            session.pending.insert(
                id,
                Pending {
                    connection_id: device.connection_id,
                    sink: sink.clone(),
                    request_id: request.request_id.clone(),
                    mutation: false,
                    deadline: Instant::now() + self.limits.request_timeout,
                    lookup: Some(lookup_id),
                },
            );
            asked += 1;
        }
        if asked == 0 {
            // No live owner could hold the receipt.
            return reply(
                sink,
                &request.request_id,
                RemoteResult::RequestStatus {
                    target_request_id,
                    receipt: ReceiptState::Unknown,
                    result: None,
                },
            );
        }
        if let Some(connection) = state.connections.get_mut(&device.connection_id) {
            connection.in_flight += asked;
        }
        state.lookups.insert(
            lookup_id,
            Lookup {
                sink: sink.clone(),
                request_id: request.request_id,
                target_request_id,
                remaining: asked,
            },
        );
    }

    // ---- timers ---------------------------------------------------------

    /// Everything that depends on time or on a queue having drained.
    pub fn tick(&self) {
        let now = Instant::now();
        let mut guard = self.state();
        let state = &mut *guard;
        let mut timed_out = Vec::new();
        let mut silent = Vec::new();
        let mut list_changed = false;
        for (session_id, session) in state.sessions.iter_mut() {
            let quiet = now.duration_since(session.last_heard);
            if quiet >= self.limits.owner_timeout {
                silent.push(session_id.clone());
                continue;
            }
            if quiet >= self.limits.owner_unresponsive && session.info.health == OwnerHealth::Live {
                session.info.health = OwnerHealth::Unresponsive;
                if let Some(cache) = session.cache.as_mut() {
                    cache.session.health = OwnerHealth::Unresponsive;
                    cache.snapshot_id = Some(uuid::Uuid::new_v4().to_string());
                }
                list_changed = true;
            }
            let expired: Vec<u64> = session
                .pending
                .iter()
                .filter(|(_, pending)| pending.deadline <= now)
                .map(|(id, _)| *id)
                .collect();
            timed_out.extend(
                expired
                    .into_iter()
                    .filter_map(|id| session.pending.remove(&id)),
            );
            session.subscribers.retain(|_, subscriber| {
                if subscriber.sink.is_closed() {
                    return false;
                }
                match &subscriber.attach {
                    Some(attach) if attach.deadline <= now => {
                        reply(
                            &subscriber.sink,
                            &attach.request_id,
                            error(
                                RemoteErrorCode::OwnerUnavailable,
                                "the session owner did not provide a snapshot",
                            ),
                        );
                        false
                    }
                    _ => true,
                }
            });
            let waiting = session
                .subscribers
                .values()
                .any(|subscriber| subscriber.next.is_none());
            if session.backlog || waiting {
                session.serve_all(session_id, self.identity(), &self.limits);
            }
        }
        for pending in timed_out {
            self.finish(state, pending, None);
        }
        for session_id in silent {
            if let Some(session) = state.sessions.get(&session_id) {
                let _ = session.to_owner.try_send(GatewayFrame::Closed {
                    reason: "the gateway heard nothing from this terminal for too long".to_owned(),
                });
            }
            self.close_session(state, &session_id, SessionCloseReason::OwnerExited);
        }
        if list_changed {
            self.push_sessions(state);
        } else if state.connections.values().any(|c| c.list_dirty) {
            let frame = self.sessions_frame(state);
            for connection in state.connections.values_mut() {
                if connection.list_dirty
                    && connection.sink.free_slots() > self.limits.reserved_slots
                {
                    connection.list_dirty = connection.sink.send(frame.clone()).is_err();
                }
            }
        }
        // Tell each owner who is attached, when that changed.
        let statuses: Vec<(String, GatewayLinkStatus)> = state
            .sessions
            .iter()
            .map(|(session_id, session)| (session_id.clone(), self.link_status(state, session)))
            .collect();
        for (session_id, status) in statuses {
            let session = state.sessions.get_mut(&session_id).expect("just listed");
            if session.status_sent.as_ref() != Some(&status)
                && session
                    .to_owner
                    .try_send(GatewayFrame::Status {
                        status: status.clone(),
                    })
                    .is_ok()
            {
                session.status_sent = Some(status);
            }
        }
    }
}

/// The list row of a registration: whatever the owner says about itself, it
/// cannot name another session or epoch, and health is the gateway's call.
fn owned_info(
    registration: &SessionRegistration,
    mut info: RemoteSessionInfo,
    health: OwnerHealth,
) -> RemoteSessionInfo {
    info.session_id = registration.session_id.clone();
    info.registration_epoch = registration.registration_epoch;
    info.health = health;
    info
}

impl FrameRouter for SessionHub {
    fn connected(&self, device: &DeviceContext, sink: &FrameSink) {
        self.state().connections.insert(
            device.connection_id,
            Connection {
                device: device.clone(),
                sink: sink.clone(),
                list_subscribed: false,
                list_dirty: false,
                in_flight: 0,
            },
        );
    }

    fn frame(&self, device: &DeviceContext, frame: &str, sink: &FrameSink) {
        let request = match decode_request(frame) {
            Ok(request) => request,
            Err(rejection) => return respond(sink, *rejection),
        };
        let mut guard = self.state();
        let state = &mut *guard;
        match request.operation.clone() {
            RemoteOperation::ListSessions | RemoteOperation::SubscribeSessions => {
                let subscribed = request.operation == RemoteOperation::SubscribeSessions;
                if let Some(connection) = state.connections.get_mut(&device.connection_id) {
                    connection.list_subscribed |= subscribed;
                    connection.list_dirty = false;
                }
                let identity = self.identity();
                reply(
                    sink,
                    &request.request_id,
                    RemoteResult::Sessions {
                        gateway_id: identity.gateway_id.clone(),
                        instance_id: Some(identity.instance_id.clone()),
                        sessions: Self::sessions_of(state),
                        subscribed: state
                            .connections
                            .get(&device.connection_id)
                            .is_some_and(|connection| connection.list_subscribed),
                    },
                );
            }
            RemoteOperation::AttachSession { resume } => {
                self.attach(state, device, sink, &request, resume.as_ref());
            }
            RemoteOperation::DetachSession => {
                if let Some(session) = request
                    .session_id
                    .as_ref()
                    .and_then(|session_id| state.sessions.get_mut(session_id))
                {
                    session.subscribers.remove(&device.connection_id);
                }
                reply(sink, &request.request_id, RemoteResult::Detached);
            }
            RemoteOperation::GetRequestStatus { target_request_id } => {
                self.lookup(state, device, sink, request, target_request_id);
            }
            _ => self.forward(state, device, sink, request),
        }
    }

    fn disconnected(&self, device: &DeviceContext) {
        let mut state = self.state();
        state.connections.remove(&device.connection_id);
        for session in state.sessions.values_mut() {
            session.subscribers.remove(&device.connection_id);
        }
        // Requests still waiting on an owner are answered into a closed
        // queue; the owner's receipt is what a reconnecting device asks for.
    }
}

#[cfg(test)]
mod tests;
