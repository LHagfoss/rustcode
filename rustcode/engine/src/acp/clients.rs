//! The ACP clients connected to this engine and what each one can do.
//!
//! One engine serves several clients at once (the desktop app over stdio
//! today; a local socket and other machines next, see `docs/desktop.md`).
//! Each connection keeps its own [`ClientInfo`] from its `initialize`, and
//! the sessions it created or loaded:
//!
//! - a session's updates go to the connections that have it ([`SessionClients`]);
//! - a request to a client (an approval, a question, a preview, the desktop,
//!   orchestration) goes to the connection that most recently attached the
//!   session and has the capability;
//! - thread status and other list notifications go to every connection.
//!
//! What runs (sessions, turns, harness processes) belongs to the engine, not
//! to a connection: closing one only forgets it here.
//!
//! While a turn runs, what it sent is kept here too ([`LiveTurn`]): a client
//! that loads the session mid-turn gets the transcript from before the turn,
//! then what the turn sent so far, then the rest live ([`Peer::attach_open`]).
//!
//! Every update is numbered and the latest ones are kept per session
//! ([`UpdateLog`]), so a client that comes back with the cursor of the last
//! one it saw gets only what it missed ([`Peer::resume_open`]); see
//! [`super::cursor`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use agent_client_protocol::schema::v1::{
    ClientCapabilities, SessionInfoUpdate, SessionNotification, SessionUpdate,
};
use agent_client_protocol::{Client, ConnectionTo, JsonRpcNotification, JsonRpcRequest};
use serde_json::json;

use super::cursor::{LogMark, Position, UpdateLog, rustcode_meta};

/// A connection's id, unique for the life of the engine.
pub(crate) type ClientId = u64;

/// What one client advertised in `initialize`. Read when a session is
/// created or loaded, so a session's prompt never changes under it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ClientInfo {
    /// The `rustcode-viz` level it renders (`_meta.rustcode.visualizations`).
    pub(crate) visualization_level: u8,
    /// `_meta.rustcode.smartUi` (#2484).
    pub(crate) smart_ui: crate::app::SmartUi,
    /// `_meta.rustcode.preview` (#2138): its sessions get `localhost_preview`.
    pub(crate) preview: bool,
    /// It answers desktop control.
    pub(crate) desktop: bool,
    /// It renders `smart_ui_preview` calls (`smartUi >= 2`).
    pub(crate) smart_ui_preview: bool,
    /// The orchestration version it hosts (#2637); 0 for none.
    pub(crate) orchestration: u8,
    /// Discord Rich Presence (#2179).
    pub(crate) presence: bool,
    /// It lists a thread the engine starts for a paired device
    /// (`_meta.rustcode.threads`).
    pub(crate) shows_threads: bool,
}

impl ClientInfo {
    pub(crate) fn from_capabilities(capabilities: &ClientCapabilities) -> Self {
        let meta = capabilities.meta.as_ref();
        Self {
            visualization_level: super::client_visualization_level(capabilities),
            smart_ui: super::client_smart_ui(capabilities),
            preview: crate::preview::client_meta_supports_preview(meta),
            desktop: crate::desktop_control::supported(meta),
            smart_ui_preview: crate::smart_ui::client_meta_supports_preview(meta),
            orchestration: crate::orchestration::client_meta_version(meta),
            presence: super::presence::client_meta_wants_presence(meta),
            shows_threads: super::client_shows_threads(meta),
        }
    }

    pub(crate) fn has(&self, capability: Capability) -> bool {
        match capability {
            Capability::Preview => self.preview,
            Capability::Desktop => self.desktop,
            Capability::SmartUiPreview => self.smart_ui_preview,
            Capability::Orchestration => self.orchestration >= 1,
            Capability::Threads => self.shows_threads,
        }
    }

    pub(crate) fn display(&self) -> crate::harness::ClientDisplay {
        crate::harness::ClientDisplay {
            visualization_level: self.visualization_level,
            smart_ui: self.smart_ui,
        }
    }
}

/// A request only some clients answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capability {
    Preview,
    Desktop,
    SmartUiPreview,
    Orchestration,
    Threads,
}

/// Notifications a client asks for by making a request first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Topic {
    /// `_rustcode/themes/changed`, after `_rustcode/themes`.
    Themes,
    /// `_rustcode/pulls/watch`, after a watch request.
    PullWatches,
}

/// One session as one connection has it.
#[derive(Clone, Copy)]
struct Slot {
    /// When it attached, on the registry's clock.
    at: u64,
    /// The cursor of the newest logged update queued to this connection (or,
    /// for the sender of a prompt, that its view already shows). Every update
    /// up to it was queued in order, so it is what the connection can be told
    /// it has.
    queued: u64,
}

struct Entry {
    connection: ConnectionTo<Client>,
    info: ClientInfo,
    /// When it initialized, on the registry's clock.
    since: u64,
    /// The sessions it created or loaded.
    sessions: HashMap<String, Slot>,
    topics: HashSet<Topic>,
}

/// Most updates a running turn keeps for a client that loads it mid-turn.
/// Text streams as many small chunks; consecutive ones are joined, so this is
/// reached only by a turn of thousands of tool calls. Past it the turn is
/// replayed from the history instead.
const LIVE_TURN_UPDATES: usize = 20_000;

/// The turn running in a session, for a client that loads it mid-turn.
struct Turn {
    /// How many history messages the session had when the turn began.
    base: usize,
    /// What the turn sent since, its prompt first.
    updates: Vec<SessionUpdate>,
}

impl Turn {
    fn record(&mut self, update: &SessionUpdate) {
        if let Some(last) = self.updates.last_mut()
            && join_text(last, update)
        {
            return;
        }
        self.updates.push(update.clone());
    }
}

/// Append `next` to `last` when both are text of the same message.
fn join_text(last: &mut SessionUpdate, next: &SessionUpdate) -> bool {
    use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk};
    fn text_of(chunk: &ContentChunk) -> Option<&str> {
        match &chunk.content {
            ContentBlock::Text(text)
                if chunk.meta.is_none() && text.meta.is_none() && text.annotations.is_none() =>
            {
                Some(&text.text)
            }
            _ => None,
        }
    }
    let (last, next) = match (last, next) {
        (SessionUpdate::AgentMessageChunk(last), SessionUpdate::AgentMessageChunk(next))
        | (SessionUpdate::AgentThoughtChunk(last), SessionUpdate::AgentThoughtChunk(next)) => {
            (last, next)
        }
        _ => return false,
    };
    if last.message_id != next.message_id || text_of(last).is_none() {
        return false;
    }
    let Some(more) = text_of(next) else {
        return false;
    };
    if let ContentBlock::Text(text) = &mut last.content {
        text.text.push_str(more);
    }
    true
}

#[derive(Default)]
struct Registry {
    next_id: ClientId,
    /// Orders attaches and connects: the newest one answers a request.
    clock: u64,
    entries: HashMap<ClientId, Entry>,
    /// The turn each session runs, if any.
    turns: HashMap<String, Turn>,
    /// Bumped whenever a session's turn starts, ends or stops being kept:
    /// a replay built from the history checks nothing moved under it.
    generations: HashMap<String, u64>,
    /// The recent updates of each session, numbered.
    logs: HashMap<String, UpdateLog>,
}

/// `update` as a notification carrying `cursor`: a logged update, one a
/// client can resume after.
fn notification(session_id: &str, update: SessionUpdate, cursor: u64) -> SessionNotification {
    let mut notification = SessionNotification::new(session_id.to_owned(), update);
    notification.meta = Some(rustcode_meta(None, "cursor", json!(cursor)));
    notification
}

/// `update` sent to bring one client's view up to date (the transcript on a
/// load): it is not in the log, so it has no cursor. A client that is cut off
/// halfway through a load has no cursor to resume from, and loads again.
fn snapshot(session_id: &str, update: SessionUpdate) -> SessionNotification {
    SessionNotification::new(session_id.to_owned(), update)
}

impl Registry {
    fn log(&mut self, session_id: &str) -> &mut UpdateLog {
        self.logs
            .entry(session_id.to_owned())
            .or_insert_with(UpdateLog::new)
    }

    /// Queue `notification`, the logged update `cursor`, to each connection
    /// that has `session_id` (`except` already shows it). Called with the
    /// update numbered under the same lock, so the order on the wire is the
    /// order of the cursors; sending only queues the message. It fails only
    /// when every connection it was sent to failed.
    fn deliver(
        &mut self,
        session_id: &str,
        cursor: u64,
        except: Option<ClientId>,
        notification: SessionNotification,
    ) -> Result<(), agent_client_protocol::Error> {
        let mut failure = None;
        let mut delivered = false;
        for (id, entry) in &mut self.entries {
            let Some(slot) = entry.sessions.get_mut(session_id) else {
                continue;
            };
            if Some(*id) == except {
                slot.queued = cursor;
                continue;
            }
            match entry.connection.send_notification(notification.clone()) {
                Ok(()) => {
                    slot.queued = cursor;
                    delivered = true;
                }
                Err(error) => failure = Some(error),
            }
        }
        match failure {
            Some(error) if !delivered => Err(error),
            _ => Ok(()),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn bump(&mut self, session_id: &str) {
        *self.generations.entry(session_id.to_owned()).or_default() += 1;
    }

    fn generation(&self, session_id: &str) -> u64 {
        self.generations
            .get(session_id)
            .copied()
            .unwrap_or_default()
    }

    fn entry(&mut self, id: ClientId, connection: &ConnectionTo<Client>) -> &mut Entry {
        let since = self.tick();
        self.entries.entry(id).or_insert_with(|| Entry {
            connection: connection.clone(),
            info: ClientInfo::default(),
            since,
            sessions: HashMap::new(),
            topics: HashSet::new(),
        })
    }
}

/// Every connected client. Cheap to clone; all clones are the same registry.
#[derive(Clone, Default)]
pub(crate) struct Clients(Arc<Mutex<Registry>>);

impl Clients {
    fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// An id for a connection about to start.
    pub(crate) fn new_id(&self) -> ClientId {
        let mut registry = self.registry();
        registry.next_id += 1;
        registry.next_id
    }

    /// The connection `id`, as a request handler sees it.
    pub(crate) fn peer(&self, id: ClientId, connection: &ConnectionTo<Client>) -> Peer {
        let info = self.info(id);
        Peer {
            clients: self.clone(),
            id,
            connection: connection.clone(),
            info,
        }
    }

    /// `initialize`: the connection is here with these capabilities.
    pub(crate) fn connect(
        &self,
        id: ClientId,
        connection: &ConnectionTo<Client>,
        info: ClientInfo,
    ) {
        let mut registry = self.registry();
        let since = registry.tick();
        let entry = registry.entry(id, connection);
        entry.info = info;
        entry.since = since;
    }

    /// The connection ended. Its sessions keep running for the others.
    pub(crate) fn disconnect(&self, id: ClientId) {
        self.registry().entries.remove(&id);
    }

    /// What `id` advertised; nothing for one that never initialized.
    pub(crate) fn info(&self, id: ClientId) -> ClientInfo {
        self.registry()
            .entries
            .get(&id)
            .map(|entry| entry.info)
            .unwrap_or_default()
    }

    /// Whether any connected client advertised what `wants` looks for.
    pub(crate) fn any(&self, wants: impl Fn(&ClientInfo) -> bool) -> bool {
        self.registry()
            .entries
            .values()
            .any(|entry| wants(&entry.info))
    }

    /// How many clients are connected.
    pub(crate) fn count(&self) -> usize {
        self.registry().entries.len()
    }

    /// The newest connected client with `capability`.
    pub(crate) fn newest_with(&self, capability: Capability) -> Option<Peer> {
        let registry = self.registry();
        registry
            .entries
            .iter()
            .filter(|(_, entry)| entry.info.has(capability))
            .max_by_key(|(_, entry)| entry.since)
            .map(|(id, entry)| Peer {
                clients: self.clone(),
                id: *id,
                connection: entry.connection.clone(),
                info: entry.info,
            })
    }

    /// Where `session_id`'s updates go.
    pub(crate) fn session(&self, session_id: &str) -> SessionClients {
        SessionClients {
            clients: self.clone(),
            session_id: session_id.to_owned(),
        }
    }

    /// The session closed: no connection has it any more.
    pub(crate) fn forget_session(&self, session_id: &str) {
        let mut registry = self.registry();
        for entry in registry.entries.values_mut() {
            entry.sessions.remove(session_id);
        }
        registry.turns.remove(session_id);
        registry.generations.remove(session_id);
        registry.logs.remove(session_id);
    }

    /// The history of `session_id` changed without an update saying so (a
    /// rewind): no client resumes from a cursor it got before.
    pub(crate) fn invalidate_cursors(&self, session_id: &str) {
        let mut registry = self.registry();
        if let Some(log) = registry.logs.get_mut(session_id) {
            log.invalidate();
        }
    }

    /// The cursor of the newest update of `session_id` (tests).
    #[cfg(test)]
    pub(crate) fn head(&self, session_id: &str) -> u64 {
        self.registry().log(session_id).head()
    }

    /// Where `session_id`'s log is now: a load of a session that is not open
    /// yet takes it before it reads the history, and [`Peer::attach_closed`]
    /// sends everything logged after it. It makes no log of its own, so a
    /// load that fails before the session opens (an unknown thread, a cwd
    /// that does not match) leaves nothing in the registry for the server's
    /// life.
    pub(crate) fn load_mark(&self, session_id: &str) -> LogMark {
        LogMark(self.registry().logs.get(session_id).map(UpdateLog::head))
    }

    /// Where `session_id`'s turn is: a generation that changes whenever a
    /// turn starts or ends, and while one runs, how many history messages
    /// came before it. A replay built from those is current for as long as
    /// the generation is.
    pub(crate) fn turn_marker(&self, session_id: &str) -> (u64, Option<usize>) {
        let registry = self.registry();
        (
            registry.generation(session_id),
            registry.turns.get(session_id).map(|turn| turn.base),
        )
    }

    /// How many threads have a turn running, built-in or on another harness.
    pub(crate) fn running_turns(&self) -> usize {
        self.registry().turns.len()
    }

    /// Whether a turn runs in `session_id` (tests).
    #[cfg(test)]
    pub(crate) fn turn_running(&self, session_id: &str) -> bool {
        self.registry().turns.contains_key(session_id)
    }

    /// `id` also gets the notifications of `topic`.
    pub(crate) fn subscribe(&self, id: ClientId, topic: Topic) {
        if let Some(entry) = self.registry().entries.get_mut(&id) {
            entry.topics.insert(topic);
        }
    }

    /// Tell every connected client.
    pub(crate) fn broadcast<N: JsonRpcNotification>(&self, notification: N) {
        self.send_where(notification, |_| true);
    }

    /// Tell every client with `capability`.
    pub(crate) fn broadcast_to<N: JsonRpcNotification>(
        &self,
        capability: Capability,
        notification: N,
    ) {
        self.send_where(notification, |entry| entry.info.has(capability));
    }

    /// Tell every client that asked for `topic`.
    pub(crate) fn publish<N: JsonRpcNotification>(&self, topic: Topic, notification: N) {
        self.send_where(notification, |entry| entry.topics.contains(&topic));
    }

    fn send_where<N: JsonRpcNotification>(&self, notification: N, to: impl Fn(&Entry) -> bool) {
        let connections = self
            .registry()
            .entries
            .values()
            .filter(|entry| to(entry))
            .map(|entry| entry.connection.clone())
            .collect::<Vec<_>>();
        for connection in connections {
            let _ = connection.send_notification(notification.clone());
        }
    }

    fn attach(&self, id: ClientId, session_id: &str) {
        let mut registry = self.registry();
        let at = registry.tick();
        let queued = registry.log(session_id).head();
        if let Some(entry) = registry.entries.get_mut(&id) {
            entry
                .sessions
                .insert(session_id.to_owned(), Slot { at, queued });
        }
    }

    /// The connections that have `session_id`.
    fn audience(&self, session_id: &str) -> Vec<ConnectionTo<Client>> {
        audience_of(&self.registry(), session_id)
    }

    /// The connection that most recently attached `session_id` and has `need`.
    fn requester(
        &self,
        session_id: &str,
        need: Option<Capability>,
    ) -> Option<ConnectionTo<Client>> {
        self.registry()
            .entries
            .values()
            .filter(|entry| need.is_none_or(|need| entry.info.has(need)))
            .filter_map(|entry| Some((entry.sessions.get(session_id)?.at, entry)))
            .max_by_key(|(at, _)| *at)
            .map(|(_, entry)| entry.connection.clone())
    }
}

fn audience_of(registry: &Registry, session_id: &str) -> Vec<ConnectionTo<Client>> {
    registry
        .entries
        .values()
        .filter(|entry| entry.sessions.contains_key(session_id))
        .map(|entry| entry.connection.clone())
        .collect()
}

/// Send `notification` to each of `connections`. It fails only when every
/// connection it was sent to failed; with none it goes nowhere.
fn send_all<N: JsonRpcNotification>(
    connections: Vec<ConnectionTo<Client>>,
    notification: N,
) -> Result<(), agent_client_protocol::Error> {
    let mut failure = None;
    let mut delivered = false;
    for connection in connections {
        match connection.send_notification(notification.clone()) {
            Ok(()) => delivered = true,
            Err(error) => failure = Some(error),
        }
    }
    match failure {
        Some(error) if !delivered => Err(error),
        _ => Ok(()),
    }
}

/// A turn running in one session; dropping it ends the turn's record.
#[must_use = "the turn is recorded until this drops"]
pub(crate) struct LiveTurn {
    clients: Clients,
    session_id: String,
}

impl Drop for LiveTurn {
    fn drop(&mut self) {
        let mut registry = self.clients.registry();
        if registry.turns.remove(&self.session_id).is_some() {
            registry.bump(&self.session_id);
        }
    }
}

/// One connection, as the handler of a request it sent sees it.
#[derive(Clone)]
pub(crate) struct Peer {
    clients: Clients,
    id: ClientId,
    connection: ConnectionTo<Client>,
    info: ClientInfo,
}

impl Peer {
    pub(crate) fn id(&self) -> ClientId {
        self.id
    }

    pub(crate) fn connection(&self) -> &ConnectionTo<Client> {
        &self.connection
    }

    pub(crate) fn clients(&self) -> &Clients {
        &self.clients
    }

    /// What this client advertised in `initialize`.
    pub(crate) fn info(&self) -> ClientInfo {
        self.info
    }

    /// This client created or loaded `session_id`: it gets its updates from
    /// now on. Answers where they go.
    pub(crate) fn attach(&self, session_id: &str) -> SessionClients {
        self.clients.attach(self.id, session_id);
        self.clients.session(session_id)
    }

    /// This client loads `session_id`, which is already open, possibly
    /// mid-turn. `replay` is the transcript up to the running turn (the whole
    /// history when none runs), built for `generation` of
    /// [`Clients::turn_marker`]. Sent first, then what the turn sent so far,
    /// then every update live, in that order. With `reset` the client is told
    /// to clear its view before it (it asked to resume and could not). None of
    /// it carries a cursor: the client has one when the load is answered
    /// ([`Peer::position`]), not before.
    ///
    /// `None`, with nothing sent, when a turn started or ended since
    /// `generation`: the replay is stale and has to be built again. `None`
    /// for `generation` skips the check.
    pub(crate) fn attach_open(
        &self,
        session_id: &str,
        generation: Option<u64>,
        replay: Vec<SessionUpdate>,
        reset: bool,
    ) -> Result<Option<Position>, agent_client_protocol::Error> {
        self.attach_replayed(session_id, generation, None, replay, reset)
    }

    /// This client loads `session_id`, which was not open: the engine read
    /// its transcript from disk, then opened it, and `replay` is that
    /// transcript. `base` is [`Clients::load_mark`] from before the
    /// transcript was read. Updates the session logged since (another client loaded it and
    /// ran a turn, a title arrived) are not in the transcript: they are sent
    /// after it, each with its cursor, so the cursor the answer names covers
    /// nothing this connection was not sent.
    ///
    /// `None`, with nothing sent, when the log no longer reaches back to
    /// `base`: the load has to start again.
    pub(crate) fn attach_closed(
        &self,
        session_id: &str,
        base: LogMark,
        replay: Vec<SessionUpdate>,
        reset: bool,
    ) -> Result<Option<Position>, agent_client_protocol::Error> {
        self.attach_replayed(session_id, None, Some(base), replay, reset)
    }

    fn attach_replayed(
        &self,
        session_id: &str,
        generation: Option<u64>,
        base: Option<LogMark>,
        replay: Vec<SessionUpdate>,
        reset: bool,
    ) -> Result<Option<Position>, agent_client_protocol::Error> {
        let mut registry = self.clients.registry();
        if generation.is_some_and(|generation| generation != registry.generation(session_id)) {
            return Ok(None);
        }
        let head = registry.log(session_id).head();
        let position = Position {
            cursor: head,
            reset,
            resumed: false,
        };
        // The log after `base`, if it is all still there.
        let missed = match base {
            None => Vec::new(),
            Some(base) => match registry.log(session_id).since_mark(base) {
                Some(logged) => logged
                    .map(|logged| (logged.cursor, logged.update.clone()))
                    .collect(),
                None => return Ok(None),
            },
        };
        // A client that has gone is not brought back by a late load.
        if !registry.entries.contains_key(&self.id) {
            return Ok(Some(position));
        }
        // Sent under the registry's lock, which every session update takes
        // to number and address itself: nothing reaches this client between
        // the replay and the live updates, and nothing is sent twice.
        // Sending only queues the message. All of it is current as of `head`.
        // With a `base` the log has the running turn's updates too, so they
        // are not sent a second time from the turn's own record.
        let running = match base {
            Some(_) => Vec::new(),
            None => registry
                .turns
                .get(session_id)
                .map(|turn| turn.updates.clone())
                .unwrap_or_default(),
        };
        if reset {
            self.connection
                .send_notification(reset_marker(session_id))?;
        }
        for update in replay.into_iter().chain(running) {
            self.connection
                .send_notification(snapshot(session_id, update))?;
        }
        for (cursor, update) in missed {
            self.connection
                .send_notification(notification(session_id, update, cursor))?;
        }
        let at = registry.tick();
        if let Some(entry) = registry.entries.get_mut(&self.id) {
            entry
                .sessions
                .insert(session_id.to_owned(), Slot { at, queued: head });
        }
        Ok(Some(position))
    }

    /// This client loads `session_id`, which is already open, and has
    /// everything up to `cursor` (`_meta.rustcode.resume`). It is sent what
    /// came after, each update with its own cursor, then every update live:
    /// nothing missed and nothing twice, since the log and the audience
    /// change under one lock.
    ///
    /// `None`, with nothing sent, when the log no longer reaches back to
    /// `cursor`: the caller sends the whole transcript with a reset.
    pub(crate) fn resume_open(
        &self,
        session_id: &str,
        cursor: u64,
    ) -> Result<Option<Position>, agent_client_protocol::Error> {
        let mut registry = self.clients.registry();
        let head = registry.log(session_id).head();
        let position = Position {
            cursor: head,
            reset: false,
            resumed: true,
        };
        // A client that has gone is not brought back by a late load.
        if !registry.entries.contains_key(&self.id) {
            let resumable = registry.log(session_id).since(cursor).is_some();
            return Ok(resumable.then_some(position));
        }
        let Some(missed) = registry.log(session_id).since(cursor) else {
            return Ok(None);
        };
        for logged in missed {
            self.connection.send_notification(notification(
                session_id,
                logged.update.clone(),
                logged.cursor,
            ))?;
        }
        let at = registry.tick();
        if let Some(entry) = registry.entries.get_mut(&self.id) {
            entry
                .sessions
                .insert(session_id.to_owned(), Slot { at, queued: head });
        }
        Ok(Some(position))
    }

    /// Where this client is in `session_id` now, for the answer to its
    /// `session/load`: the cursor of the newest update queued to it, so
    /// everything up to it is on its way, in order, ahead of the answer.
    pub(crate) fn position(&self, session_id: &str, reset: bool) -> Position {
        let mut registry = self.clients.registry();
        let head = registry.log(session_id).head();
        let cursor = registry
            .entries
            .get(&self.id)
            .and_then(|entry| entry.sessions.get(session_id))
            .map_or(head, |slot| slot.queued);
        Position {
            cursor,
            reset,
            resumed: false,
        }
    }

    /// Send this client alone an `update` that brings its view up to date
    /// (not a new event). It has no cursor: it is not in the log.
    pub(crate) fn send_snapshot(
        &self,
        session_id: &str,
        update: SessionUpdate,
    ) -> Result<(), agent_client_protocol::Error> {
        self.connection
            .send_notification(snapshot(session_id, update))
    }
}

/// The update that starts a reset: it changes nothing but carries the flag.
fn reset_marker(session_id: &str) -> SessionNotification {
    let mut marker = snapshot(
        session_id,
        SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new()),
    );
    marker.meta = Some(rustcode_meta(None, "reset", json!(true)));
    marker
}

/// The clients that have one session: where its updates and its requests go.
/// Looked up on every send, so a client that attaches later gets the rest
/// and one that left gets nothing.
#[derive(Clone)]
pub(crate) struct SessionClients {
    clients: Clients,
    session_id: String,
}

impl SessionClients {
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Send `notification` to every connection that has the session. With
    /// none it goes nowhere and the session runs on; it fails only when every
    /// connection it was sent to failed.
    pub(crate) fn notify<N: JsonRpcNotification>(
        &self,
        notification: N,
    ) -> Result<(), agent_client_protocol::Error> {
        send_all(self.clients.audience(&self.session_id), notification)
    }

    /// A `session/update` of this session. A running turn keeps it for a
    /// client that loads the session later in the turn.
    pub(crate) fn update(&self, update: SessionUpdate) -> Result<(), agent_client_protocol::Error> {
        // Numbered, recorded, addressed and queued under one lock. A client
        // attaching meanwhile gets the update either in its replay or live,
        // never both and never neither; and concurrent producers cannot put
        // their cursors on the wire out of order, which would let a client
        // that drops after the higher one skip the lower one on resume.
        // Sending only queues the message.
        let mut registry = self.clients.registry();
        let cursor = registry.log(&self.session_id).push(&update);
        let mut overflow = false;
        if let Some(turn) = registry.turns.get_mut(&self.session_id) {
            turn.record(&update);
            overflow = turn.updates.len() > LIVE_TURN_UPDATES;
        }
        if overflow {
            registry.turns.remove(&self.session_id);
            registry.bump(&self.session_id);
        }
        registry.deliver(
            &self.session_id,
            cursor,
            None,
            notification(&self.session_id, update, cursor),
        )
    }

    /// A `session/update` for every connection with the session except
    /// `except`, which already shows it (the prompt it sent).
    pub(crate) fn update_others(&self, except: ClientId, update: SessionUpdate) {
        // Kept for a client that resumes. The sender does not get it, but
        // its view has it, so its position moves past it.
        let mut registry = self.clients.registry();
        let cursor = registry.log(&self.session_id).push(&update);
        let _ = registry.deliver(
            &self.session_id,
            cursor,
            Some(except),
            notification(&self.session_id, update, cursor),
        );
    }

    /// A turn starts: `base` history messages came before it. What it sends
    /// is kept, `prompt` first, until the returned guard drops.
    pub(crate) fn begin_turn(&self, base: usize, prompt: Option<String>) -> LiveTurn {
        use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, TextContent};
        let updates = prompt
            .filter(|prompt| !prompt.is_empty())
            .map(|prompt| {
                SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(prompt),
                )))
            })
            .into_iter()
            .collect();
        let mut registry = self.clients.registry();
        registry
            .turns
            .insert(self.session_id.clone(), Turn { base, updates });
        registry.bump(&self.session_id);
        LiveTurn {
            clients: self.clients.clone(),
            session_id: self.session_id.clone(),
        }
    }

    /// The connection a request about this session goes to.
    pub(crate) fn requester(&self, need: Option<Capability>) -> Option<ConnectionTo<Client>> {
        self.clients.requester(&self.session_id, need)
    }

    /// Ask the client that most recently attached the session and has `need`.
    /// With no such client the request fails, as a request to a closed
    /// connection did.
    pub(crate) async fn request<R: JsonRpcRequest>(
        &self,
        need: Option<Capability>,
        request: R,
    ) -> Result<R::Response, agent_client_protocol::Error> {
        let Some(connection) = self.requester(need) else {
            return Err(agent_client_protocol::Error::internal_error().data(format!(
                "no connected client has session {}",
                self.session_id
            )));
        };
        connection.send_request(request).block_task().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        AgentCapabilities, InitializeRequest, InitializeResponse,
    };
    use agent_client_protocol::{Agent, Channel, Client};

    #[tokio::test]
    async fn late_attach_and_subscribe_do_not_restore_a_disconnected_client() {
        let clients = Clients::default();
        let id = clients.new_id();
        let (engine, transport) = Channel::duplex();
        let (peer_tx, peer_rx) = tokio::sync::oneshot::channel();
        let peer_tx = Arc::new(std::sync::Mutex::new(Some(peer_tx)));
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();
        let server_clients = clients.clone();

        let server = tokio::spawn(async move {
            Agent
                .builder()
                .on_receive_request(
                    async move |_: InitializeRequest, responder, connection| {
                        let clients = server_clients.clone();
                        let peer_tx = Arc::clone(&peer_tx);
                        clients.connect(
                            id,
                            &connection,
                            ClientInfo {
                                preview: true,
                                ..ClientInfo::default()
                            },
                        );
                        if let Some(peer_tx) = peer_tx
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .take()
                        {
                            let _ = peer_tx.send(clients.peer(id, &connection));
                        }
                        responder.respond(
                            InitializeResponse::new(
                                agent_client_protocol::schema::ProtocolVersion::V1,
                            )
                            .agent_capabilities(AgentCapabilities::new()),
                        )
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(engine)
                .await
        });
        let client = tokio::spawn(async move {
            Client
                .builder()
                .connect_with(transport, async move |connection| {
                    connection
                        .send_request(InitializeRequest::new(
                            agent_client_protocol::schema::ProtocolVersion::V1,
                        ))
                        .block_task()
                        .await?;
                    let _ = close_rx.await;
                    Ok(())
                })
                .await
        });

        let peer = peer_rx.await.expect("initialize captured its peer");
        let _ = close_tx.send(());
        server
            .await
            .expect("server task")
            .expect("server closes cleanly");
        client
            .await
            .expect("client task")
            .expect("client closes cleanly");
        clients.disconnect(id);

        assert!(
            peer.info().preview,
            "a peer keeps its initialized capabilities"
        );
        peer.clients().subscribe(peer.id(), Topic::Themes);
        let session = peer.attach("closed-session");
        assert_eq!(
            clients.count(),
            0,
            "late operations do not recreate the entry"
        );
        assert!(session.requester(Some(Capability::Preview)).is_none());
        assert!(
            session
                .update(SessionUpdate::UserMessageChunk(
                    agent_client_protocol::schema::v1::ContentChunk::new(
                        agent_client_protocol::schema::v1::ContentBlock::Text(
                            agent_client_protocol::schema::v1::TextContent::new("unseen"),
                        ),
                    ),
                ))
                .is_ok()
        );
    }

    /// A connected client of `clients`: its peer, and every update it is sent.
    async fn connected(
        clients: &Clients,
    ) -> (
        Peer,
        tokio::sync::mpsc::UnboundedReceiver<SessionNotification>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let id = clients.new_id();
        let (engine, transport) = Channel::duplex();
        let (peer_tx, peer_rx) = tokio::sync::oneshot::channel();
        let peer_tx = Arc::new(std::sync::Mutex::new(Some(peer_tx)));
        let (notes_tx, notes) = tokio::sync::mpsc::unbounded_channel();
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();
        let server_clients = clients.clone();
        tokio::spawn(async move {
            Agent
                .builder()
                .on_receive_request(
                    async move |_: InitializeRequest, responder, connection| {
                        server_clients.connect(id, &connection, ClientInfo::default());
                        if let Some(peer_tx) = peer_tx
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .take()
                        {
                            let _ = peer_tx.send(server_clients.peer(id, &connection));
                        }
                        responder.respond(
                            InitializeResponse::new(
                                agent_client_protocol::schema::ProtocolVersion::V1,
                            )
                            .agent_capabilities(AgentCapabilities::new()),
                        )
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(engine)
                .await
        });
        tokio::spawn(async move {
            Client
                .builder()
                .on_receive_notification(
                    async move |notification: SessionNotification, _connection| {
                        let _ = notes_tx.send(notification);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async move |connection| {
                    connection
                        .send_request(InitializeRequest::new(
                            agent_client_protocol::schema::ProtocolVersion::V1,
                        ))
                        .block_task()
                        .await?;
                    let _ = close_rx.await;
                    Ok(())
                })
                .await
        });
        (peer_rx.await.expect("initialized"), notes, close_tx)
    }

    fn text(text: &str) -> SessionUpdate {
        use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, TextContent};
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    fn cursor_of(note: &SessionNotification) -> u64 {
        note.meta.as_ref().expect("meta")["rustcode"]["cursor"]
            .as_u64()
            .expect("a cursor")
    }

    /// A client resumes from `cursor` while another thread keeps sending
    /// updates to the session: it gets every update after `cursor` exactly
    /// once and in order, whether each landed before the resume or after it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_resume_racing_live_updates_loses_and_repeats_nothing() {
        const UPDATES: u64 = 1_500;
        for round in 0..25_u64 {
            let clients = Clients::default();
            let session_id = format!("race-{round}");
            let sender = clients.session(&session_id);
            for n in 0..20 {
                sender.update(text(&format!("early {n}"))).expect("send");
            }
            // The client had seen up to here before it dropped off.
            let seen = clients.head(&session_id) - 5;
            let (peer, mut notes, close) = connected(&clients).await;

            let producer = {
                let sender = sender.clone();
                tokio::spawn(async move {
                    for n in 0..UPDATES {
                        sender.update(text(&format!("live {n}"))).expect("send");
                        if n % 7 == 0 {
                            tokio::task::yield_now().await;
                        }
                    }
                })
            };
            // Land the resume at a different point of the stream each round.
            for _ in 0..round * 3 {
                tokio::task::yield_now().await;
            }
            let position = peer
                .resume_open(&session_id, seen)
                .expect("sends")
                .expect("the cursor is recent");
            assert!(position.resumed && !position.reset);
            producer.await.expect("producer");
            let last = clients.head(&session_id);

            let mut cursors = Vec::new();
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                while let Some(note) = notes.recv().await {
                    cursors.push(cursor_of(&note));
                    if cursors.last() == Some(&last) {
                        break;
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("round {round}: stopped at {:?}", cursors.last()));
            assert_eq!(
                cursors,
                (seen + 1..=last).collect::<Vec<_>>(),
                "round {round}: each update after the cursor once, in order"
            );
            let _ = close.send(());
        }
    }

    /// The cursors of what `notes` gets, up to and including `last`.
    async fn cursors_until(
        notes: &mut tokio::sync::mpsc::UnboundedReceiver<SessionNotification>,
        last: u64,
    ) -> Vec<u64> {
        let mut got = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(note) = notes.recv().await {
                got.push(cursor_of(&note));
                if got.last() == Some(&last) {
                    break;
                }
            }
        })
        .await
        .expect("updates arrive");
        got
    }

    /// Two clients resume from different places of one session and each gets
    /// exactly what is after its own.
    #[tokio::test]
    async fn clients_resume_from_their_own_cursors() {
        let clients = Clients::default();
        let sender = clients.session("shared");
        let mut cursors = Vec::new();
        for n in 0..10 {
            sender.update(text(&format!("{n}"))).expect("send");
            cursors.push(clients.head("shared"));
        }
        let (early, mut early_notes, _early_close) = connected(&clients).await;
        let (late, mut late_notes, _late_close) = connected(&clients).await;
        early.resume_open("shared", cursors[2]).unwrap().unwrap();
        late.resume_open("shared", cursors[7]).unwrap().unwrap();
        sender.update(text("live")).expect("send");
        let live = clients.head("shared");
        let early_got = cursors_until(&mut early_notes, live).await;
        let late_got = cursors_until(&mut late_notes, live).await;
        assert_eq!(early_got, [&cursors[3..], &[live]].concat());
        assert_eq!(late_got, [&cursors[8..], &[live]].concat());
    }

    /// A rewind ends every resume from before it: the chat was cut with no
    /// update to say so.
    #[tokio::test]
    async fn a_resume_from_before_a_rewind_is_refused() {
        let clients = Clients::default();
        let sender = clients.session("cut");
        sender.update(text("a")).expect("send");
        let seen = clients.head("cut");
        clients.invalidate_cursors("cut");
        let (peer, ..) = connected(&clients).await;
        assert!(peer.resume_open("cut", seen).unwrap().is_none());
        assert!(
            peer.resume_open("cut", clients.head("cut"))
                .unwrap()
                .is_some()
        );
    }

    /// Producers on several threads (a turn streaming, a title being made,
    /// a terminal finishing) send to one session at once: each client is
    /// handed the cursors in order, with none missing, so the greatest one
    /// it has seen never covers an update it was not sent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_producers_put_cursors_on_the_wire_in_order() {
        const PER_PRODUCER: u64 = 1_500;
        const PRODUCERS: u64 = 4;
        for round in 0..10 {
            let clients = Clients::default();
            let session_id = format!("order-{round}");
            let (peer, mut notes, close) = connected(&clients).await;
            peer.attach(&session_id);
            let start = clients.head(&session_id);
            let producers = (0..PRODUCERS)
                .map(|producer| {
                    let sender = clients.session(&session_id);
                    std::thread::spawn(move || {
                        for n in 0..PER_PRODUCER {
                            let result = if producer == 0 && n % 3 == 0 {
                                // The prompt shown to the others goes the same way.
                                sender.update_others(u64::MAX, text("echo"));
                                Ok(())
                            } else {
                                sender.update(text(&format!("{producer}/{n}")))
                            };
                            result.expect("send");
                        }
                    })
                })
                .collect::<Vec<_>>();
            for producer in producers {
                producer.join().expect("producer");
            }
            let last = clients.head(&session_id);
            let cursors = cursors_until(&mut notes, last).await;
            assert_eq!(
                cursors,
                (start + 1..=last).collect::<Vec<_>>(),
                "round {round}: the wire order is the cursor order"
            );
            // What the load answer would name is what was queued, no more.
            assert_eq!(peer.position(&session_id, false).cursor, last);
            let _ = close.send(());
        }
    }

    /// The load answer names the newest update queued to this connection,
    /// not whatever the session has logged by the time it is built.
    #[tokio::test]
    async fn the_answer_names_what_was_queued_to_the_connection() {
        let clients = Clients::default();
        let sender = clients.session("answer");
        sender.update(text("before")).expect("send");
        let (peer, mut notes, _close) = connected(&clients).await;
        let loaded_at = clients.head("answer");
        let position = peer
            .attach_open("answer", None, vec![text("history")], false)
            .unwrap()
            .unwrap();
        assert_eq!(position.cursor, loaded_at);
        sender.update(text("live")).expect("send");
        sender.update(text("more")).expect("send");
        let queued = clients.head("answer");
        assert_eq!(peer.position("answer", false).cursor, queued);
        // A client that joins the session later is not named updates it
        // was not sent.
        let (other, ..) = connected(&clients).await;
        other.attach("answer");
        sender.update(text("after")).expect("send");
        assert_eq!(peer.position("answer", false).cursor, queued + 1);
        assert_eq!(other.position("answer", false).cursor, queued + 1);
        // The history it was sent to rebuild its view has no cursor.
        assert!(notes.recv().await.expect("history").meta.is_none());
        let seen = cursors_until(&mut notes, queued + 1).await;
        assert_eq!(seen, [queued - 1, queued, queued + 1]);
    }

    /// A session no connection has open is loaded while another connection
    /// keeps sending it updates (a prompt, a title). The loader gets its
    /// reset marker and the whole transcript first, none of it carrying a
    /// cursor, and only then the live updates, every one after the answer's
    /// cursor and none missing. So a load cut off in the middle of the
    /// transcript has no cursor to come back with, and an update is never
    /// sent before the reset that would clear it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_load_racing_a_prompt_gets_the_transcript_before_any_live_update() {
        const LIVE: u64 = 2_000;
        const TRANSCRIPT: usize = 300;
        for round in 0..20 {
            let clients = Clients::default();
            let session_id = format!("closed-{round}");
            let (prompter, _prompter_notes, _prompter_close) = connected(&clients).await;
            prompter.attach(&session_id);
            let sent = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let producer = {
                let sender = clients.session(&session_id);
                let sent = Arc::clone(&sent);
                std::thread::spawn(move || {
                    for n in 0..LIVE {
                        sender.update(text(&format!("live {n}"))).expect("send");
                        sent.store(n + 1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            };
            while sent.load(std::sync::atomic::Ordering::SeqCst) < 20 {
                tokio::task::yield_now().await;
            }

            // It had a cursor from before and asks to resume: reset.
            let (loader, mut notes, close) = connected(&clients).await;
            let transcript = (0..TRANSCRIPT)
                .map(|n| text(&format!("message {n}")))
                .collect::<Vec<_>>();
            let position = loader
                .attach_open(&session_id, None, transcript, true)
                .unwrap()
                .unwrap();
            producer.join().expect("producer");
            let head = clients.head(&session_id);

            let mut prefix = Vec::new();
            while prefix.len() < TRANSCRIPT + 1 {
                prefix.push(notes.recv().await.expect("a note"));
            }
            // A client cut off anywhere in here holds no cursor.
            assert!(
                prefix[0]
                    .meta
                    .as_ref()
                    .is_some_and(|meta| meta["rustcode"]["reset"] == true),
                "round {round}: the reset marker comes first"
            );
            assert!(
                prefix.iter().all(|note| note
                    .meta
                    .as_ref()
                    .is_none_or(|meta| meta["rustcode"]["cursor"].is_null())),
                "round {round}: no live update lands inside the transcript"
            );
            // After it, every live update, in order, from the answer's cursor.
            let live = if position.cursor == head {
                Vec::new()
            } else {
                cursors_until(&mut notes, head).await
            };
            assert_eq!(
                live,
                (position.cursor + 1..=head).collect::<Vec<_>>(),
                "round {round}: nothing lost after the transcript"
            );
            let _ = close.send(());
        }
    }

    /// A closed-session load whose base the log no longer reaches (too much
    /// happened meanwhile, or the history was rewritten) sends nothing and
    /// says so: the cursor it would answer must not cover what it skipped.
    #[tokio::test]
    async fn a_closed_load_the_log_cannot_catch_up_is_refused() {
        use super::super::cursor::LOG_UPDATES;
        let clients = Clients::default();
        let sender = clients.session("busy");
        sender.update(text("before")).expect("send");
        let base = clients.load_mark("busy");
        for n in 0..LOG_UPDATES + 5 {
            sender.update(text(&format!("{n}"))).expect("send");
        }
        let (loader, mut notes, _close) = connected(&clients).await;
        assert!(
            loader
                .attach_closed("busy", base, vec![text("history")], false)
                .unwrap()
                .is_none()
        );
        let base = clients.load_mark("busy");
        sender.update(text("one")).expect("send");
        sender.update(text("two")).expect("send");
        clients.invalidate_cursors("busy");
        assert!(
            loader
                .attach_closed("busy", base, vec![text("history")], false)
                .unwrap()
                .is_none()
        );
        assert!(notes.try_recv().is_err(), "nothing was sent");
        let head = clients.head("busy");
        let position = loader
            .attach_closed(
                "busy",
                clients.load_mark("busy"),
                vec![text("history")],
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(position.cursor, head);
        assert!(notes.recv().await.expect("history").meta.is_none());
    }

    /// Taking the mark of a closed session makes no log: a load that then
    /// fails (an unknown thread, a cwd that does not match) leaves nothing in
    /// the registry. One that goes on gets what the session logged after the
    /// mark, even when the log only began after it.
    #[tokio::test]
    async fn a_load_mark_makes_no_log_and_still_covers_what_came_after() {
        let clients = Clients::default();
        for n in 0..100 {
            let id = format!("unknown-{n}");
            assert_eq!(clients.load_mark(&id), LogMark(None));
        }
        assert!(
            clients.registry().logs.is_empty(),
            "failed loads leave no log behind"
        );

        let mark = clients.load_mark("late");
        // Another client opens the thread and it logs, before this load attaches.
        let sender = clients.session("late");
        sender.update(text("one")).expect("send");
        sender.update(text("two")).expect("send");
        let head = clients.head("late");
        let (loader, mut notes, _close) = connected(&clients).await;
        let position = loader
            .attach_closed("late", mark, vec![text("history")], false)
            .unwrap()
            .expect("the log reaches back to the mark");
        assert_eq!(position.cursor, head);
        assert!(notes.recv().await.expect("history").meta.is_none());
        let one = notes.recv().await.expect("one");
        let two = notes.recv().await.expect("two");
        assert_eq!((cursor_of(&one), cursor_of(&two)), (head - 1, head));
    }

    /// A load cut off halfway has no cursor to resume from, so the next one
    /// rebuilds: nothing it sent to rebuild the view is resumable, and the
    /// cursor the client came with is as stale as it was.
    #[tokio::test]
    async fn a_load_cut_off_halfway_is_rebuilt_not_resumed() {
        use super::super::cursor::LOG_UPDATES;
        let clients = Clients::default();
        let sender = clients.session("cut-off");
        sender.update(text("first")).expect("send");
        let stale = clients.head("cut-off");
        for n in 0..LOG_UPDATES + 5 {
            sender.update(text(&format!("{n}"))).expect("send");
        }
        let transcript = (0..300)
            .map(|n| text(&format!("message {n}")))
            .collect::<Vec<_>>();

        // The client asked to resume with a cursor that is too old; it is
        // sent the reset and the transcript, and is cut off after 150.
        let (first, mut notes, close) = connected(&clients).await;
        assert!(first.resume_open("cut-off", stale).unwrap().is_none());
        let position = first
            .attach_open("cut-off", None, transcript.clone(), true)
            .unwrap()
            .unwrap();
        assert!(position.reset);
        let mut received = Vec::new();
        while received.len() < 151 {
            received.push(notes.recv().await.expect("a note"));
        }
        let _ = close.send(());
        let reset = received[0].meta.as_ref().expect("meta")["rustcode"].clone();
        assert_eq!(reset["reset"], true);
        assert!(
            received.iter().all(|note| note
                .meta
                .as_ref()
                .is_none_or(|meta| meta["rustcode"]["cursor"].is_null())),
            "nothing it sent to rebuild the view can be resumed from"
        );

        // It reconnects with the last cursor it saw: none, or the stale one
        // it had. Either way the engine does not resume it.
        let (second, mut notes, _close) = connected(&clients).await;
        assert!(second.resume_open("cut-off", stale).unwrap().is_none());
        let position = second
            .attach_open("cut-off", None, transcript, true)
            .unwrap()
            .unwrap();
        assert!(position.reset && !position.resumed);
        let mut sent = Vec::new();
        while sent.len() < 301 {
            sent.push(notes.recv().await.expect("a note"));
        }
        assert!(
            sent[0]
                .meta
                .as_ref()
                .is_some_and(|meta| meta["rustcode"]["reset"] == true),
            "the reset marker, then all 300 messages"
        );
    }
}
