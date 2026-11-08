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

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use agent_client_protocol::schema::v1::{ClientCapabilities, SessionNotification, SessionUpdate};
use agent_client_protocol::{Client, ConnectionTo, JsonRpcNotification, JsonRpcRequest};

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
    /// `_rustcode/pulls/watch/event`, after a watch request.
    PullWatches,
}

struct Entry {
    connection: ConnectionTo<Client>,
    info: ClientInfo,
    /// When it initialized, on the registry's clock.
    since: u64,
    /// The sessions it created or loaded, each with when it did.
    sessions: HashMap<String, u64>,
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
}

impl Registry {
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
        Peer {
            clients: self.clone(),
            id,
            connection: connection.clone(),
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
            .map(|(id, entry)| self.peer(*id, &entry.connection))
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

    /// Whether a turn runs in `session_id` (tests).
    #[cfg(test)]
    pub(crate) fn turn_running(&self, session_id: &str) -> bool {
        self.registry().turns.contains_key(session_id)
    }

    /// `id` also gets the notifications of `topic`.
    pub(crate) fn subscribe(&self, id: ClientId, connection: &ConnectionTo<Client>, topic: Topic) {
        self.registry().entry(id, connection).topics.insert(topic);
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

    fn attach(&self, id: ClientId, connection: &ConnectionTo<Client>, session_id: &str) {
        let mut registry = self.registry();
        let at = registry.tick();
        registry
            .entry(id, connection)
            .sessions
            .insert(session_id.to_owned(), at);
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
            .filter_map(|entry| Some((entry.sessions.get(session_id)?, entry)))
            .max_by_key(|(at, _)| **at)
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
        self.clients.info(self.id)
    }

    /// This client created or loaded `session_id`: it gets its updates from
    /// now on. Answers where they go.
    pub(crate) fn attach(&self, session_id: &str) -> SessionClients {
        self.clients.attach(self.id, &self.connection, session_id);
        self.clients.session(session_id)
    }

    /// This client loads `session_id`, which is already open, possibly
    /// mid-turn. `replay` is the transcript up to the running turn (the whole
    /// history when none runs), built for `generation` of
    /// [`Clients::turn_marker`]. Sent first, then what the turn sent so far,
    /// then every update live, in that order.
    ///
    /// `false`, with nothing sent, when a turn started or ended since
    /// `generation`: the replay is stale and has to be built again. `None`
    /// skips the check.
    pub(crate) fn attach_open(
        &self,
        session_id: &str,
        generation: Option<u64>,
        replay: Vec<SessionUpdate>,
    ) -> Result<bool, agent_client_protocol::Error> {
        let mut registry = self.clients.registry();
        if generation.is_some_and(|generation| generation != registry.generation(session_id)) {
            return Ok(false);
        }
        // Sent under the registry's lock, which every session update takes
        // to find its audience: nothing reaches this client between the
        // replay and the live updates, and nothing is sent twice. Sending
        // only queues the message.
        let running = registry
            .turns
            .get(session_id)
            .map(|turn| turn.updates.clone())
            .unwrap_or_default();
        for update in replay.into_iter().chain(running) {
            self.connection
                .send_notification(SessionNotification::new(session_id.to_owned(), update))?;
        }
        let at = registry.tick();
        registry
            .entry(self.id, &self.connection)
            .sessions
            .insert(session_id.to_owned(), at);
        Ok(true)
    }
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
        let audience = {
            // Recorded and addressed under one lock: a client attaching
            // meanwhile gets the update either in its replay or live, never
            // both and never neither.
            let mut registry = self.clients.registry();
            let mut overflow = false;
            if let Some(turn) = registry.turns.get_mut(&self.session_id) {
                turn.record(&update);
                overflow = turn.updates.len() > LIVE_TURN_UPDATES;
            }
            if overflow {
                registry.turns.remove(&self.session_id);
                registry.bump(&self.session_id);
            }
            audience_of(&registry, &self.session_id)
        };
        send_all(
            audience,
            SessionNotification::new(self.session_id.clone(), update),
        )
    }

    /// A `session/update` for every connection with the session except
    /// `except`, which already shows it (the prompt it sent).
    pub(crate) fn update_others(&self, except: ClientId, update: SessionUpdate) {
        let audience = self
            .clients
            .registry()
            .entries
            .iter()
            .filter(|(id, entry)| **id != except && entry.sessions.contains_key(&self.session_id))
            .map(|(_, entry)| entry.connection.clone())
            .collect::<Vec<_>>();
        let _ = send_all(
            audience,
            SessionNotification::new(self.session_id.clone(), update),
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
