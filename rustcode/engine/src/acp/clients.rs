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
use std::time::Instant;

use agent_client_protocol::schema::v1::{
    ClientCapabilities, ContentBlock, SessionInfoUpdate, SessionNotification, SessionUpdate,
    ToolCallId, ToolCallStatus,
};
use agent_client_protocol::{
    Client, ConnectionTo, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    UntypedMessage,
};
use serde_json::json;

use super::cursor::{LogMark, Position, UpdateLog, rustcode_meta};

#[path = "clients_approval.rs"]
mod approval;

#[cfg(test)]
#[path = "clients_approval_tests.rs"]
mod approval_tests;

/// A connection's id, unique for the life of the engine.
pub(crate) type ClientId = u64;

/// What one client advertised in `initialize`. Read when a session is
/// created or loaded, so a session's prompt never changes under it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ClientInfo {
    /// The `rustcode-viz` level it renders (`_meta.rustcode.visualizations`).
    pub(crate) visualization_level: u8,
    /// `_meta.rustcode.smartUi` (#2484).
    pub(crate) smart_ui: rustcode_core::SmartUi,
    /// `_meta.rustcode.preview` (#2138): its sessions get `localhost_preview`.
    pub(crate) preview: bool,
    /// It answers desktop control.
    pub(crate) desktop: bool,
    /// It renders `smart_ui_preview` calls (`smartUi >= 2`).
    pub(crate) smart_ui_preview: bool,
    /// The orchestration version it hosts (#2637); 0 for none.
    pub(crate) orchestration: u8,
    /// It hosts no orchestration but asks this engine to serve the tools it
    /// can itself (`_meta.rustcode.engineOrchestration`): an app connected to
    /// a paired machine, whose threads' tools are that machine's to answer.
    pub(crate) engine_orchestration: bool,
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
            engine_orchestration: crate::orchestration::client_meta_engine_hosted(meta),
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

    /// Whether this engine serves a session's orchestration tools itself
    /// (#3422): the client hosts none, and either no app is attached or the
    /// client asked for it. An app that turned the tools off keeps them off.
    pub(crate) fn engine_hosts_orchestration(&self, clients: &SessionClients) -> bool {
        self.orchestration == 0 && (self.engine_orchestration || !clients.app_attached())
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
    /// `_rustcode/workflows/changed`, after a workflow list request.
    Workflows,
    /// `_rustcode/settings/changed`, after `_rustcode/settings/get` or `/set`.
    Settings,
    /// `_rustcode/outside/changed`, after `_rustcode/outside/list`.
    Outside,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Waiting,
    Writing,
    Tool,
    Approval,
}

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/session/turnPhase")]
#[serde(rename_all = "camelCase")]
struct TurnPhaseNotification {
    session_id: String,
    phase: Option<Phase>,
    activity_age_ms: Option<u64>,
    /// How long the turn has run, so a window that joins mid-turn shows its real elapsed time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn_age_ms: Option<u64>,
}

struct TurnTiming {
    started: Instant,
    changed: Instant,
    last_activity: Option<(Instant, chrono::DateTime<chrono::Utc>)>,
    phase: Phase,
    durations: [u64; 4],
    tools: HashSet<ToolCallId>,
    approvals: usize,
    writing: bool,
}

fn millis(duration: std::time::Duration) -> u64 {
    duration.as_millis() as u64
}

impl TurnTiming {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            changed: now,
            last_activity: None,
            phase: Phase::Waiting,
            durations: [0; 4],
            tools: HashSet::new(),
            approvals: 0,
            writing: false,
        }
    }

    fn notification(&self, session_id: &str) -> TurnPhaseNotification {
        TurnPhaseNotification {
            session_id: session_id.to_owned(),
            phase: Some(self.phase),
            activity_age_ms: Some(millis(
                self.last_activity
                    .map_or(self.started, |(at, _)| at)
                    .elapsed(),
            )),
            turn_age_ms: Some(millis(self.started.elapsed())),
        }
    }

    fn record(&mut self, update: &SessionUpdate) -> bool {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) if meaningful_content(&chunk.content) => {
                self.writing = true;
            }
            SessionUpdate::AgentThoughtChunk(chunk) if meaningful_content(&chunk.content) => {
                self.writing = false;
            }
            SessionUpdate::ToolCall(call) => {
                self.tool_status(&call.tool_call_id, Some(call.status));
            }
            SessionUpdate::ToolCallUpdate(call) => {
                self.tool_status(&call.tool_call_id, call.fields.status);
            }
            SessionUpdate::Plan(_) => {}
            _ => return false,
        }
        self.activity();
        true
    }

    fn tool_status(&mut self, id: &ToolCallId, status: Option<ToolCallStatus>) {
        self.writing = false;
        match status {
            Some(ToolCallStatus::Pending | ToolCallStatus::InProgress) => {
                self.tools.insert(id.clone());
            }
            Some(_) => {
                self.tools.remove(id);
            }
            None => {}
        }
    }

    fn activity(&mut self) {
        let now = Instant::now();
        self.last_activity = Some((now, chrono::Utc::now()));
        let phase = if self.approvals > 0 {
            Phase::Approval
        } else if !self.tools.is_empty() {
            Phase::Tool
        } else if self.writing {
            Phase::Writing
        } else {
            Phase::Waiting
        };
        if phase != self.phase {
            self.durations[self.phase as usize] += millis(now.duration_since(self.changed));
            self.changed = now;
            self.phase = phase;
        }
    }

    fn persist(mut self, thread_id: String) {
        self.durations[self.phase as usize] += millis(self.changed.elapsed());
        let [waiting, writing, tool, approval] = self.durations;
        let fields = json!({
            "phase_ms": { "waiting": waiting, "writing": writing, "tool": tool, "approval": approval },
            "last_activity_at": self.last_activity.map(|(_, wall)| wall.to_rfc3339()),
            "elapsed_ms": millis(self.started.elapsed()),
        });
        crate::data_store::record_operational_event(&crate::data_store::OperationalEventRow {
            thread_id,
            turn_id: None,
            event_type: "turn.phase_summary".to_owned(),
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            fields_json: Some(fields.to_string()),
            original_bytes: None,
            captured_bytes: None,
            payload_truncated: false,
        });
    }
}

fn meaningful_content(content: &ContentBlock) -> bool {
    !matches!(content, ContentBlock::Text(text) if text.text.is_empty())
}

#[derive(Default)]
struct Registry {
    next_id: ClientId,
    /// Orders attaches and connects: the newest one answers a request.
    clock: u64,
    entries: HashMap<ClientId, Entry>,
    /// The turn each session runs, if any.
    turns: HashMap<String, Turn>,
    timings: HashMap<String, TurnTiming>,
    /// Bumped whenever a session's turn starts, ends or stops being kept:
    /// a replay built from the history checks nothing moved under it.
    generations: HashMap<String, u64>,
    /// The recent updates of each session, numbered.
    logs: HashMap<String, UpdateLog>,
    /// The last `_rustcode/session/activity` of each session that reported
    /// one, sent again to a client that loads or resumes it: it goes out only
    /// when it changes, so a client that attached later, or was away when it
    /// changed, would otherwise show a working thread as finished.
    activities: HashMap<String, Activity>,
    approval_cancels: HashMap<String, approval::CancelSlot>,
    shutting_down: bool,
}

/// What `_rustcode/session/activity` last said of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Activity {
    state: String,
    background: usize,
}

fn activity_notification(
    session_id: &str,
    activity: &Activity,
) -> super::prompt::SessionActivityNotification {
    super::prompt::SessionActivityNotification {
        session_id: session_id.to_owned(),
        state: activity.state.clone(),
        background: activity.background,
    }
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
        let mut untyped = None;
        for (id, entry) in &mut self.entries {
            let Some(slot) = entry.sessions.get_mut(session_id) else {
                continue;
            };
            if Some(*id) == except {
                slot.queued = cursor;
                continue;
            }
            let message = match &untyped {
                Some(message) => message,
                None => untyped.insert(notification.to_untyped_message()?),
            };
            match send_untyped_notification(&entry.connection, message.clone()) {
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

/// How many connected clients in this process host orchestration (an app):
/// while none does, the engine wakes a parent thread itself (#4382).
static HOSTING_APPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn count_hosting(before: bool, after: bool) {
    use std::sync::atomic::Ordering;
    match (before, after) {
        (false, true) => {
            HOSTING_APPS.fetch_add(1, Ordering::SeqCst);
        }
        (true, false) => {
            let _ = HOSTING_APPS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                Some(n.saturating_sub(1))
            });
        }
        _ => {}
    }
}

/// Whether an app that hosts orchestration is connected to this engine.
pub(crate) fn app_hosts_orchestration() -> bool {
    HOSTING_APPS.load(std::sync::atomic::Ordering::SeqCst) > 0
}

/// Every connected client. Cheap to clone; all clones are the same registry.
#[derive(Clone)]
pub(crate) struct Clients {
    registry: Arc<Mutex<Registry>>,
    attached: Arc<tokio::sync::watch::Sender<u64>>,
}

impl Default for Clients {
    fn default() -> Self {
        let (attached, _) = tokio::sync::watch::channel(0);
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            attached: Arc::new(attached),
        }
    }
}

impl Clients {
    fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn notify_attached(&self) {
        self.attached
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    /// Cancel every approval currently waiting on a client for `session_id`.
    pub(crate) fn cancel_approvals(&self, session_id: &str) {
        if let Some(slot) = self.registry().approval_cancels.remove(session_id) {
            slot.cancel();
        }
    }

    /// End every waiting approval across all sessions on engine shutdown.
    pub(crate) fn shutdown(&self) {
        let mut registry = self.registry();
        registry.shutting_down = true;
        for (_, slot) in registry.approval_cancels.drain() {
            slot.cancel();
        }
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
        let was_hosting = entry.info.orchestration >= 1;
        entry.info = info;
        entry.since = since;
        count_hosting(was_hosting, info.orchestration >= 1);
    }

    /// The connection ended. Its sessions keep running for the others.
    pub(crate) fn disconnect(&self, id: ClientId) {
        if let Some(entry) = self.registry().entries.remove(&id) {
            count_hosting(entry.info.orchestration >= 1, false);
        }
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
        registry.activities.remove(session_id);
        registry.timings.remove(session_id);
        if let Some(slot) = registry.approval_cancels.remove(session_id) {
            slot.cancel();
        }
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

    /// Whether a connected client asked for `topic`.
    pub(crate) fn has_topic(&self, topic: Topic) -> bool {
        self.registry()
            .entries
            .values()
            .any(|entry| entry.topics.contains(&topic))
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

    /// Tell every client that lists threads and does not have `session_id`: a
    /// thread's name is kept in a thread list, not only in the chat it was
    /// opened in. Sent outside the logged stream, so it has no cursor.
    pub(crate) fn publish_thread_title(&self, session_id: &str, update: SessionUpdate) {
        self.send_where(snapshot(session_id, update), |entry| {
            entry.info.has(Capability::Threads) && !entry.sessions.contains_key(session_id)
        });
    }

    fn send_where<N: JsonRpcNotification>(&self, notification: N, to: impl Fn(&Entry) -> bool) {
        let connections = self
            .registry()
            .entries
            .values()
            .filter(|entry| to(entry))
            .map(|entry| entry.connection.clone())
            .collect::<Vec<_>>();
        if connections.is_empty() {
            return;
        }
        let Ok(untyped) = notification.to_untyped_message() else {
            return;
        };
        send_untyped_to_connections(connections, untyped);
    }

    fn attach(&self, id: ClientId, session_id: &str) {
        let mut registry = self.registry();
        let at = registry.tick();
        let queued = registry.log(session_id).head();
        if let Some(entry) = registry.entries.get_mut(&id) {
            entry
                .sessions
                .insert(session_id.to_owned(), Slot { at, queued });
            self.notify_attached();
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

/// Single monomorphized notification send so `ConnectionTo::send_notification`
/// is not instantiated per notification type.
fn send_untyped_notification(
    connection: &ConnectionTo<Client>,
    notification: UntypedMessage,
) -> Result<(), agent_client_protocol::Error> {
    connection.send_notification(notification)
}

fn send_notification_erased<N: JsonRpcNotification>(
    connection: &ConnectionTo<Client>,
    notification: N,
) -> Result<(), agent_client_protocol::Error> {
    send_untyped_notification(connection, notification.to_untyped_message()?)
}

fn send_untyped_to_connections(
    connections: Vec<ConnectionTo<Client>>,
    notification: UntypedMessage,
) {
    for connection in connections {
        let _ = send_untyped_notification(&connection, notification.clone());
    }
}

/// Send `notification` to each of `connections`. It fails only when every
/// connection it was sent to failed; with none it goes nowhere.
fn send_all<N: JsonRpcNotification>(
    connections: Vec<ConnectionTo<Client>>,
    notification: N,
) -> Result<(), agent_client_protocol::Error> {
    if connections.is_empty() {
        return Ok(());
    }
    send_all_untyped(connections, notification.to_untyped_message()?)
}

fn send_all_untyped(
    connections: Vec<ConnectionTo<Client>>,
    notification: UntypedMessage,
) -> Result<(), agent_client_protocol::Error> {
    let mut failure = None;
    let mut delivered = false;
    for connection in connections {
        match send_untyped_notification(&connection, notification.clone()) {
            Ok(()) => delivered = true,
            Err(error) => failure = Some(error),
        }
    }
    match failure {
        Some(error) if !delivered => Err(error),
        _ => Ok(()),
    }
}

async fn send_untyped_request(
    connection: ConnectionTo<Client>,
    request: UntypedMessage,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    connection.send_request(request).block_task().await
}

/// A turn running in one session; dropping it ends the turn's record.
#[must_use = "the turn is recorded until this drops"]
pub(crate) struct LiveTurn {
    clients: Clients,
    session_id: String,
    started: Instant,
}

fn ended_phase(session_id: &str) -> TurnPhaseNotification {
    TurnPhaseNotification {
        session_id: session_id.to_owned(),
        phase: None,
        activity_age_ms: None,
        turn_age_ms: None,
    }
}

impl Drop for LiveTurn {
    fn drop(&mut self) {
        let timing = {
            let mut registry = self.clients.registry();
            if !registry
                .timings
                .get(&self.session_id)
                .is_some_and(|timing| timing.started == self.started)
            {
                return;
            }
            if registry.turns.remove(&self.session_id).is_some() {
                registry.bump(&self.session_id);
            }
            let timing = registry.timings.remove(&self.session_id);
            let _ = send_all(
                audience_of(&registry, &self.session_id),
                ended_phase(&self.session_id),
            );
            timing
        };
        if let Some(timing) = timing {
            timing.persist(self.session_id.clone());
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
            send_notification_erased(&self.connection, reset_marker(session_id))?;
        }
        for update in replay.into_iter().chain(running) {
            send_notification_erased(&self.connection, snapshot(session_id, update))?;
        }
        for (cursor, update) in missed {
            send_notification_erased(&self.connection, notification(session_id, update, cursor))?;
        }
        if let Some(activity) = registry.activities.get(session_id) {
            send_notification_erased(
                &self.connection,
                activity_notification(session_id, activity),
            )?;
        }
        send_notification_erased(
            &self.connection,
            registry.timings.get(session_id).map_or_else(
                || ended_phase(session_id),
                |timing| timing.notification(session_id),
            ),
        )?;
        let at = registry.tick();
        if let Some(entry) = registry.entries.get_mut(&self.id) {
            entry
                .sessions
                .insert(session_id.to_owned(), Slot { at, queued: head });
            self.clients.notify_attached();
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
            send_notification_erased(
                &self.connection,
                notification(session_id, logged.update.clone(), logged.cursor),
            )?;
        }
        if let Some(activity) = registry.activities.get(session_id) {
            send_notification_erased(
                &self.connection,
                activity_notification(session_id, activity),
            )?;
        }
        send_notification_erased(
            &self.connection,
            registry.timings.get(session_id).map_or_else(
                || ended_phase(session_id),
                |timing| timing.notification(session_id),
            ),
        )?;
        let at = registry.tick();
        if let Some(entry) = registry.entries.get_mut(&self.id) {
            entry
                .sessions
                .insert(session_id.to_owned(), Slot { at, queued: head });
            self.clients.notify_attached();
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
        send_notification_erased(&self.connection, snapshot(session_id, update))
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

    /// `_rustcode/session/activity`: what the session does outside a prompt
    /// (`waiting` on background commands, `running` a turn it woke for,
    /// `idle`). Kept, so a client that loads or resumes the session later is
    /// told too, then sent to every connection that has it now.
    pub(crate) fn activity(
        &self,
        state: &str,
        background: usize,
    ) -> Result<(), agent_client_protocol::Error> {
        let activity = Activity {
            state: state.to_owned(),
            background,
        };
        // Recorded and addressed under one lock, as an update is: a client
        // attaching meanwhile gets it either in its load or here, never
        // neither. Sending only queues the message.
        let registry = &mut *self.clients.registry();
        let notification = activity_notification(&self.session_id, &activity);
        registry
            .activities
            .insert(self.session_id.clone(), activity);
        send_all(audience_of(registry, &self.session_id), notification)
    }

    /// A `session/update` of this session. A running turn keeps it for a
    /// client that loads the session later in the turn.
    pub(crate) fn update(&self, update: SessionUpdate) -> Result<(), agent_client_protocol::Error> {
        // A title is kept in every client's thread list, like a status: it is sent to the
        // clients that do not have the session below, outside the logged stream.
        let for_list =
            matches!(&update, SessionUpdate::SessionInfoUpdate(info) if info.title.is_value())
                .then(|| update.clone());
        // Numbered, recorded, addressed and queued under one lock. A client
        // attaching meanwhile gets the update either in its replay or live,
        // never both and never neither; and concurrent producers cannot put
        // their cursors on the wire out of order, which would let a client
        // that drops after the higher one skip the lower one on resume.
        // Sending only queues the message.
        let outcome = {
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
            if let Some(timing) = registry.timings.get_mut(&self.session_id)
                && timing.record(&update)
            {
                let phase = timing.notification(&self.session_id);
                let _ = send_all(audience_of(&registry, &self.session_id), phase);
            }
            registry.deliver(
                &self.session_id,
                cursor,
                None,
                notification(&self.session_id, update, cursor),
            )
        };
        if let Some(update) = for_list {
            self.clients.publish_thread_title(&self.session_id, update);
        }
        outcome
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
        let updates = prompt
            .filter(|prompt| !prompt.is_empty())
            .map(|prompt| {
                SessionUpdate::UserMessageChunk(crate::message_origin::user_chunk(prompt, None))
            })
            .into_iter()
            .collect();
        let mut registry = self.clients.registry();
        registry
            .turns
            .insert(self.session_id.clone(), Turn { base, updates });
        registry.bump(&self.session_id);
        let timing = TurnTiming::new();
        let phase = timing.notification(&self.session_id);
        let started = timing.started;
        registry.timings.insert(self.session_id.clone(), timing);
        let _ = send_all(audience_of(&registry, &self.session_id), phase);
        LiveTurn {
            clients: self.clients.clone(),
            session_id: self.session_id.clone(),
            started,
        }
    }

    /// The connection a request about this session goes to.
    pub(crate) fn requester(&self, need: Option<Capability>) -> Option<ConnectionTo<Client>> {
        self.clients.requester(&self.session_id, need)
    }

    /// Whether an app that lists threads has this session attached: the
    /// clients that host orchestration, and the only ones whose absence means
    /// the engine should answer a call itself (#3422). Another client's app on
    /// the same server does not count: it cannot answer this session's calls.
    pub(crate) fn app_attached(&self) -> bool {
        self.requester(Some(Capability::Threads)).is_some()
    }

    /// Ask an attached client for approval, waiting for one to attach or
    /// reconnect if none is attached or its connection drops before answering.
    pub(crate) async fn request_approval<R: JsonRpcRequest>(
        &self,
        request: R,
    ) -> Result<R::Response, agent_client_protocol::Error> {
        let _approval = self.approval();
        let untyped = request.to_untyped_message()?;
        let method = untyped.method.clone();
        let value = approval::request(self, untyped).await?;
        R::Response::from_value(&method, value)
    }

    fn approval(&self) -> Option<PendingApproval<'_>> {
        let mut registry = self.clients.registry();
        let timing = registry.timings.get_mut(&self.session_id)?;
        timing.approvals += 1;
        timing.activity();
        let started = timing.started;
        let phase = timing.notification(&self.session_id);
        let _ = send_all(audience_of(&registry, &self.session_id), phase);
        Some(PendingApproval {
            session: self,
            started,
        })
    }

    /// Ask the client that most recently attached the session and has `need`.
    /// With no such client the request fails, as a request to a closed
    /// connection did.
    pub(crate) async fn request<R: JsonRpcRequest>(
        &self,
        need: Option<Capability>,
        request: R,
    ) -> Result<R::Response, agent_client_protocol::Error> {
        let untyped = request.to_untyped_message()?;
        let method = untyped.method.clone();
        let value = self.request_untyped(need, untyped).await?;
        R::Response::from_value(&method, value)
    }

    async fn request_untyped(
        &self,
        need: Option<Capability>,
        request: UntypedMessage,
    ) -> Result<serde_json::Value, agent_client_protocol::Error> {
        let Some(connection) = self.requester(need) else {
            return Err(agent_client_protocol::Error::internal_error().data(format!(
                "no connected client has session {}",
                self.session_id
            )));
        };
        send_untyped_request(connection, request).await
    }
}

struct PendingApproval<'a> {
    session: &'a SessionClients,
    started: Instant,
}

impl Drop for PendingApproval<'_> {
    fn drop(&mut self) {
        let id = &self.session.session_id;
        let mut registry = self.session.clients.registry();
        if let Some(timing) = registry.timings.get_mut(id)
            && timing.started == self.started
        {
            timing.approvals -= 1;
            timing.activity();
            let phase = timing.notification(id);
            let _ = send_all(audience_of(&registry, id), phase);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        AgentCapabilities, InitializeRequest, InitializeResponse,
    };
    use agent_client_protocol::{Agent, Channel, Client};

    #[test]
    fn reply_chunks_refresh_activity_but_empty_chunks_do_not() {
        let mut timing = TurnTiming::new();
        assert_eq!(timing.phase, Phase::Waiting);
        assert!(timing.record(&text("first")));
        assert_eq!(timing.phase, Phase::Writing);

        let earlier = Instant::now() - std::time::Duration::from_secs(30);
        timing.last_activity = Some((earlier, chrono::Utc::now()));
        assert!(!timing.record(&text("")));
        assert!(timing.notification("s").activity_age_ms.unwrap() >= 30_000);
        assert!(timing.record(&text("next")));
        assert!(timing.last_activity.unwrap().0 > earlier);
    }

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
        let (peer, notes, _activities, close) = connected_with_activity(clients).await;
        (peer, notes, close)
    }

    /// [`connected`], and every `_rustcode/session/activity` it is sent.
    async fn connected_with_activity(
        clients: &Clients,
    ) -> (
        Peer,
        tokio::sync::mpsc::UnboundedReceiver<SessionNotification>,
        tokio::sync::mpsc::UnboundedReceiver<super::super::prompt::SessionActivityNotification>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (activities_tx, activities) = tokio::sync::mpsc::unbounded_channel();
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
                .on_receive_notification(
                    async move |notification: super::super::prompt::SessionActivityNotification,
                                _connection| {
                        let _ = activities_tx.send(notification);
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
        (
            peer_rx.await.expect("initialized"),
            notes,
            activities,
            close_tx,
        )
    }

    /// A client that loads or resumes a session after its activity changed is
    /// told it too: the activity goes out only on a change, and a thread the
    /// agent woke for showed as finished in an app that attached later.
    #[tokio::test]
    async fn a_late_load_or_resume_is_told_the_last_activity() {
        let clients = Clients::default();
        let sender = clients.session("woken");
        sender.update(text("before")).expect("send");
        let seen = clients.head("woken");
        sender.activity("waiting", 2).expect("recorded");
        sender.activity("running", 1).expect("recorded");

        let (loader, _notes, mut loaded, _close) = connected_with_activity(&clients).await;
        loader
            .attach_open("woken", None, vec![text("history")], false)
            .expect("sends")
            .expect("current");
        let (resumer, _notes, mut resumed, _close) = connected_with_activity(&clients).await;
        resumer
            .resume_open("woken", seen)
            .expect("sends")
            .expect("the cursor is recent");
        for activities in [&mut loaded, &mut resumed] {
            let got = tokio::time::timeout(std::time::Duration::from_secs(10), activities.recv())
                .await
                .expect("arrives")
                .expect("an activity");
            assert_eq!((got.state.as_str(), got.background), ("running", 1));
        }

        // Live changes reach both; a forgotten session tells a new client nothing.
        sender.activity("idle", 0).expect("sent");
        for activities in [&mut loaded, &mut resumed] {
            let got = activities.recv().await.expect("live");
            assert_eq!(got.state, "idle");
        }
        clients.forget_session("woken");
        let (late, _notes, mut late_activities, _close) = connected_with_activity(&clients).await;
        late.attach_open("woken", None, vec![], false)
            .expect("sends")
            .expect("current");
        late.send_snapshot("woken", text("marker")).expect("sends");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(200),
                late_activities.recv()
            )
            .await
            .is_err(),
            "nothing recorded after the session closed"
        );
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
