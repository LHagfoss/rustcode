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

#[derive(Default)]
struct Registry {
    next_id: ClientId,
    /// Orders attaches and connects: the newest one answers a request.
    clock: u64,
    entries: HashMap<ClientId, Entry>,
}

impl Registry {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
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
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
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
        for entry in self.registry().entries.values_mut() {
            entry.sessions.remove(session_id);
        }
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
        self.registry()
            .entries
            .values()
            .filter(|entry| entry.sessions.contains_key(session_id))
            .map(|entry| entry.connection.clone())
            .collect()
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
        let mut failure = None;
        let mut delivered = false;
        for connection in self.clients.audience(&self.session_id) {
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

    /// A `session/update` of this session.
    pub(crate) fn update(&self, update: SessionUpdate) -> Result<(), agent_client_protocol::Error> {
        self.notify(SessionNotification::new(self.session_id.clone(), update))
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
