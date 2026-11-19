//! The hub role (#3957, `docs/architecture/hub-plan.md` phase 5).
//!
//! A hub is an ordinary machine with `"hub": true` in `<config>/hub.json`
//! (`rustcode machines hub on`). Another machine joins it with
//! `rustcode machines hub join <machine>`: that machine's gateway keeps an
//! outbound WebSocket to the hub's gateway, the device connection it already
//! has as a paired desktop or command line, and switches it to the hub link
//! with a `hub_join` frame. The hub can then reach it without any inbound
//! port on the member.
//!
//! An outside agent signed in on the hub that names a member in `machine`
//! has its call checked by the hub's own gate first, then sent to the member
//! as a `hub_call`. The member answers it through its own outside-agent gate,
//! at the access it granted the hub (`hub_result`), so a member is never
//! reached above the ceiling it set, whatever the agent may do on the hub.
//! Threads stay on the machine that runs them; nothing is copied to the hub.
//!
//! While a member is offline the hub keeps what can wait (`start_session`,
//! `message_session`) in a bounded mailbox and delivers it when the member
//! joins again. Everything else fails at once, naming the machine and when
//! it was last seen. The fleet still talks directly when the hub is down: the
//! hub only adds a route, it replaces none.
//!
//! The frames are the hub's own, between two RustCode gateways, and are not
//! part of the phone's remote protocol contract (`docs/remote-protocol`).

use super::gateway::MAX_FRAME_BYTES;
use crate::daemon::private_files::{publish_private_json, read_private_json};
use crate::orchestration::hub::CallAnswer;
/// The outside-agent access levels, which a membership's ceiling uses.
pub use crate::orchestration::outside::Scope;
use chrono::{DateTime, Utc};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// The role file, next to `machines.json`.
pub const HUB_FILE: &str = "hub.json";
/// What the hub knows about its members, connected or not.
const MEMBERS_FILE: &str = "remote/hub-members.json";
/// Calls waiting for an offline member.
const MAILBOX_FILE: &str = "remote/hub-mailbox.json";
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// The most members a hub remembers; the least recently seen go first.
const MAX_MEMBERS: usize = 64;
/// Calls kept for one offline machine.
pub const MAILBOX_PER_MACHINE: usize = 32;
/// Calls kept for every offline machine together.
pub const MAILBOX_TOTAL: usize = 256;
/// How long a call waits for an offline machine before it is dropped.
pub const MAILBOX_DAYS: i64 = 7;
/// How long the hub waits for a member to answer one call. `wait_thread`
/// waits for a turn, so this is long; the call may still finish there.
pub const HUB_CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// The tools that may wait for an offline member: each one only hands the
/// member work, and the agent is told the work is queued, not done.
pub const QUEUEABLE: [&str; 2] = [
    crate::orchestration::START_SESSION,
    crate::orchestration::MESSAGE_SESSION,
];
/// The largest answer a member sends; a longer one is cut with a note.
const MAX_CONTENT_BYTES: usize = MAX_FRAME_BYTES - 64 * 1024;

// ------------------------------------------------------------ the role file

/// `<config>/hub.json`: whether this machine is the fleet's hub, and the hub
/// this machine joined. Either, both or neither.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubConfig {
    /// This machine is the hub: members may join it.
    #[serde(default)]
    pub hub: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<Membership>,
}

/// The hub this machine keeps a link to, and what the hub may do here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    /// The hub's `host_id`, as `machines.json` saves it.
    pub machine: String,
    /// The hub's name when it was joined, for messages.
    pub name: String,
    /// The ceiling for every call the hub forwards, at the outside-agent
    /// levels. Read-only unless the owner raised it.
    pub access: Scope,
    /// Ask in the app before a forwarded call that changes something.
    #[serde(default = "yes")]
    pub ask: bool,
}

fn yes() -> bool {
    true
}

impl HubConfig {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join(HUB_FILE)
    }

    /// The role file, or the default (no role) when there is none or it does
    /// not parse.
    pub fn load(config_dir: &Path) -> Self {
        let path = Self::path(config_dir);
        if !path.exists() {
            return Self::default();
        }
        read_private_json(&path, 64 * 1024).unwrap_or_default()
    }

    /// The role file of this user's configuration directory.
    pub fn current() -> Self {
        crate::config::get_config_dir()
            .map(|dir| Self::load(&dir))
            .unwrap_or_default()
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        publish_private_json(&Self::path(config_dir), self)
    }
}

// ---------------------------------------------------------------- the frames

/// What a member sends on its link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemberFrame {
    /// Switch this authenticated connection to the hub link. The access and
    /// `ask` are the member's own ceiling, shown on the hub; the member
    /// enforces them.
    HubJoin {
        access: Scope,
        #[serde(default)]
        ask: bool,
    },
    HubResult {
        call_id: u64,
        success: bool,
        content: String,
    },
}

/// What the hub sends on a member's link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HubFrame {
    HubJoined,
    HubCall {
        call_id: u64,
        tool: String,
        arguments: Value,
        /// Who asked on the hub, for the member's audit log and approval card.
        caller: String,
    },
}

impl MemberFrame {
    /// The join frame, when `text` is one.
    pub fn parse_join(text: &str) -> Option<(Scope, bool)> {
        match serde_json::from_str(text).ok()? {
            Self::HubJoin { access, ask } => Some((access, ask)),
            Self::HubResult { .. } => None,
        }
    }
}

/// A member's answer, cut to fit one frame.
pub fn result_frame(call_id: u64, success: bool, mut content: String) -> MemberFrame {
    if content.len() > MAX_CONTENT_BYTES {
        let mut end = MAX_CONTENT_BYTES;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        content.truncate(end);
        content.push_str("\n[cut: the answer was longer than the hub link carries]");
    }
    MemberFrame::HubResult {
        call_id,
        success,
        content,
    }
}

// --------------------------------------------------------------- the members

/// One member as the hub shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubMember {
    pub host_id: String,
    pub name: String,
    /// The ceiling the member said it grants the hub.
    pub access: Scope,
    pub ask: bool,
    pub online: bool,
    /// When the member last joined or left; now while it is online.
    pub last_seen_at: DateTime<Utc>,
    /// Calls waiting for it.
    #[serde(default)]
    pub queued: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnownMember {
    host_id: String,
    name: String,
    access: Scope,
    #[serde(default)]
    ask: bool,
    last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Mail {
    id: String,
    host_id: String,
    tool: String,
    arguments: Value,
    caller: String,
    queued_at: DateTime<Utc>,
}

/// A call on its way to a connected member.
struct Outgoing {
    tool: String,
    arguments: Value,
    caller: String,
    reply: oneshot::Sender<CallAnswer>,
}

struct Link {
    epoch: u64,
    calls: mpsc::Sender<Outgoing>,
}

#[derive(Default)]
struct Inner {
    links: HashMap<String, Link>,
    next_epoch: u64,
}

/// How a forwarded call ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum HubCallAnswer {
    /// `machine` names no member of this hub (or this machine is no hub):
    /// the caller answers it as it would without a hub.
    NotMember,
    Answered {
        success: bool,
        content: String,
    },
}

impl HubCallAnswer {
    fn failure(content: impl Into<String>) -> Self {
        Self::Answered {
            success: false,
            content: content.into(),
        }
    }
}

/// The hub's members: the links of the connected ones, and on disk what it
/// knows about all of them and the calls waiting for the offline ones.
pub struct Members {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

impl Members {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            dir: config_dir.to_owned(),
            inner: Mutex::new(Inner::default()),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether this machine takes members now.
    pub fn is_hub(&self) -> bool {
        HubConfig::load(&self.dir).hub
    }

    fn known(&self) -> Vec<KnownMember> {
        read_private_json(&self.dir.join(MEMBERS_FILE), MAX_FILE_BYTES).unwrap_or_default()
    }

    fn save_known(&self, known: &[KnownMember]) {
        if let Err(error) = publish_private_json(&self.dir.join(MEMBERS_FILE), &known) {
            crate::dbg_log!("hub: members not saved: {error:#}");
        }
    }

    fn mail(&self) -> Vec<Mail> {
        let cutoff = Utc::now() - chrono::Duration::days(MAILBOX_DAYS);
        let mut mail: Vec<Mail> =
            read_private_json(&self.dir.join(MAILBOX_FILE), MAX_FILE_BYTES).unwrap_or_default();
        mail.retain(|item| item.queued_at > cutoff);
        mail
    }

    fn save_mail(&self, mail: &[Mail]) {
        if let Err(error) = publish_private_json(&self.dir.join(MAILBOX_FILE), &mail) {
            crate::dbg_log!("hub: mailbox not saved: {error:#}");
        }
    }

    fn remember(&self, host_id: &str, name: &str, access: Scope, ask: bool) {
        let mut known = self.known();
        known.retain(|member| member.host_id != host_id);
        known.push(KnownMember {
            host_id: host_id.to_owned(),
            name: name.to_owned(),
            access,
            ask,
            last_seen_at: Utc::now(),
        });
        known.sort_by_key(|member| std::cmp::Reverse(member.last_seen_at));
        known.truncate(MAX_MEMBERS);
        self.save_known(&known);
    }

    fn seen(&self, host_id: &str) {
        let mut known = self.known();
        if let Some(member) = known.iter_mut().find(|member| member.host_id == host_id) {
            member.last_seen_at = Utc::now();
            self.save_known(&known);
        }
    }

    /// The member `selector` names: its `host_id`, or a name only one member has.
    fn resolve(&self, selector: &str) -> Result<Option<KnownMember>, String> {
        let selector = selector.trim();
        let known = self.known();
        if let Some(member) = known.iter().find(|member| member.host_id == selector) {
            return Ok(Some(member.clone()));
        }
        let named: Vec<&KnownMember> = known
            .iter()
            .filter(|member| member.name.eq_ignore_ascii_case(selector))
            .collect();
        match named.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some((*one).clone())),
            _ => Err(format!(
                "error: more than one machine on this hub is called {selector}; pass its id from list_machines instead. Nothing was sent."
            )),
        }
    }

    /// Whether `selector` names a member this hub knows, linked or not.
    pub fn knows(&self, selector: &str) -> bool {
        !matches!(self.resolve(selector), Ok(None))
    }

    /// Every member, connected ones first.
    pub fn list(&self) -> Vec<HubMember> {
        let online: Vec<String> = self.inner().links.keys().cloned().collect();
        let mail = self.mail();
        let mut members: Vec<HubMember> = self
            .known()
            .into_iter()
            .map(|member| {
                let online = online.contains(&member.host_id);
                HubMember {
                    queued: mail
                        .iter()
                        .filter(|item| item.host_id == member.host_id)
                        .count(),
                    host_id: member.host_id,
                    name: member.name,
                    access: member.access,
                    ask: member.ask,
                    online,
                    last_seen_at: if online {
                        Utc::now()
                    } else {
                        member.last_seen_at
                    },
                }
            })
            .collect();
        members.sort_by_key(|member| !member.online);
        members
    }

    /// Send one call to the member `selector` names, or queue it while that
    /// member is offline, or refuse it.
    pub async fn call(
        &self,
        selector: &str,
        tool: &str,
        arguments: Value,
        caller: &str,
    ) -> HubCallAnswer {
        if !self.is_hub() {
            return HubCallAnswer::NotMember;
        }
        let member = match self.resolve(selector) {
            Ok(Some(member)) => member,
            Ok(None) => return HubCallAnswer::NotMember,
            Err(message) => return HubCallAnswer::failure(message),
        };
        let link = self
            .inner()
            .links
            .get(&member.host_id)
            .map(|link| link.calls.clone());
        if let Some(calls) = link {
            let (reply, answer) = oneshot::channel();
            let sent = calls
                .send(Outgoing {
                    tool: tool.to_owned(),
                    arguments: arguments.clone(),
                    caller: caller.to_owned(),
                    reply,
                })
                .await;
            if sent.is_ok() {
                return match tokio::time::timeout(HUB_CALL_TIMEOUT, answer).await {
                    Ok(Ok(answer)) => HubCallAnswer::Answered {
                        success: answer.success,
                        content: answer.content,
                    },
                    Ok(Err(_)) => HubCallAnswer::failure(format!(
                        "error: {} went offline before it answered {tool}. It may have run there; check with list_threads on that machine before retrying.",
                        member.name
                    )),
                    Err(_) => HubCallAnswer::failure(format!(
                        "error: {} did not answer {tool} within {} seconds. It may still finish there.",
                        member.name,
                        HUB_CALL_TIMEOUT.as_secs()
                    )),
                };
            }
        }
        self.offline(&member, tool, arguments, caller)
    }

    fn offline(
        &self,
        member: &KnownMember,
        tool: &str,
        mut arguments: Value,
        caller: &str,
    ) -> HubCallAnswer {
        let last_seen = member.last_seen_at.format("%Y-%m-%d %H:%M UTC");
        if !QUEUEABLE.contains(&tool) {
            return HubCallAnswer::failure(format!(
                "error: {} is offline (last seen {last_seen}), and {tool} cannot wait for it. Nothing was sent. Only starting a thread and sending one a message wait on the hub.",
                member.name
            ));
        }
        let mut mail = self.mail();
        let theirs = mail
            .iter()
            .filter(|item| item.host_id == member.host_id)
            .count();
        if theirs >= MAILBOX_PER_MACHINE || mail.len() >= MAILBOX_TOTAL {
            return HubCallAnswer::failure(format!(
                "error: {} is offline (last seen {last_seen}) and the hub already holds as many calls for it as it keeps. Nothing was sent.",
                member.name
            ));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        // Delivered at most once per member process: a redelivery after a
        // link that dropped before the answer replays the first answer.
        if arguments
            .get(crate::orchestration::CLIENT_REQUEST_ID)
            .is_none()
            && let Some(object) = arguments.as_object_mut()
        {
            object.insert(
                crate::orchestration::CLIENT_REQUEST_ID.to_owned(),
                json!(format!("hub-{id}")),
            );
        }
        mail.push(Mail {
            id,
            host_id: member.host_id.clone(),
            tool: tool.to_owned(),
            arguments,
            caller: caller.to_owned(),
            queued_at: Utc::now(),
        });
        self.save_mail(&mail);
        HubCallAnswer::Answered {
            success: true,
            content: json!({
                "queued": true,
                "machine": member.name,
                "last_seen": member.last_seen_at,
                "note": format!(
                    "{} is offline. The hub holds this {tool} and delivers it when the machine connects again, for up to {MAILBOX_DAYS} days. Nothing has run yet.",
                    member.name
                ),
            })
            .to_string(),
        }
    }

    /// Register a member's link, replacing an older one from the same
    /// machine. Its calls arrive on the receiver until [`Self::leave`].
    fn join(
        &self,
        host_id: &str,
        name: &str,
        access: Scope,
        ask: bool,
    ) -> (u64, mpsc::Receiver<Outgoing>) {
        self.remember(host_id, name, access, ask);
        let (calls, receiver) = mpsc::channel(32);
        let mut inner = self.inner();
        inner.next_epoch += 1;
        let epoch = inner.next_epoch;
        inner
            .links
            .insert(host_id.to_owned(), Link { epoch, calls });
        (epoch, receiver)
    }

    fn leave(&self, host_id: &str, epoch: u64) {
        let mut inner = self.inner();
        if inner
            .links
            .get(host_id)
            .is_some_and(|link| link.epoch == epoch)
        {
            inner.links.remove(host_id);
            drop(inner);
            self.seen(host_id);
        }
    }

    fn take_mail(&self, host_id: &str) -> Vec<Mail> {
        self.mail()
            .into_iter()
            .filter(|item| item.host_id == host_id)
            .collect()
    }

    fn delivered(&self, id: &str) {
        let mut mail = self.mail();
        let before = mail.len();
        mail.retain(|item| item.id != id);
        if mail.len() != before {
            self.save_mail(&mail);
        }
    }
}

// ------------------------------------------------------------------ the link

/// Who joined: the device record's identity, never the frame's.
pub(super) struct Joined {
    pub host_id: String,
    pub name: String,
    pub access: Scope,
    pub ask: bool,
}

/// How a link's timing is bounded; the gateway's own [`super::gateway::Limits`].
#[derive(Clone, Copy)]
pub(super) struct LinkTimes {
    pub write_timeout: Duration,
    pub ping_interval: Duration,
    pub idle_timeout: Duration,
}

enum Waiting {
    Caller(oneshot::Sender<CallAnswer>),
    Mail(String),
}

/// Serve one member's link until it closes, the member leaves the hub or
/// the hub role is turned off. Returns when the socket is done with.
pub(super) async fn serve<W, R>(
    members: &Members,
    joined: Joined,
    writer: &mut W,
    reader: &mut R,
    times: LinkTimes,
) where
    W: Sink<Message, Error = WsError> + Unpin,
    R: Stream<Item = Result<Message, WsError>> + Unpin,
{
    let (epoch, mut calls) = members.join(&joined.host_id, &joined.name, joined.access, joined.ask);
    crate::dbg_log!(
        "hub: {} joined at {} access",
        joined.name,
        joined.access.as_str()
    );
    let mut waiting: HashMap<u64, Waiting> = HashMap::new();
    let mut next_call: u64 = 0;
    let send = |frame: &HubFrame| Message::text(serde_json::to_string(frame).unwrap_or_default());
    'link: {
        if write(writer, send(&HubFrame::HubJoined), times.write_timeout)
            .await
            .is_none()
        {
            break 'link;
        }
        for item in members.take_mail(&joined.host_id) {
            next_call += 1;
            waiting.insert(next_call, Waiting::Mail(item.id));
            let frame = HubFrame::HubCall {
                call_id: next_call,
                tool: item.tool,
                arguments: item.arguments,
                caller: item.caller,
            };
            if write(writer, send(&frame), times.write_timeout)
                .await
                .is_none()
            {
                break 'link;
            }
        }
        let mut ping = tokio::time::interval_at(
            tokio::time::Instant::now() + times.ping_interval,
            times.ping_interval,
        );
        let mut last_heard = tokio::time::Instant::now();
        loop {
            tokio::select! {
                call = calls.recv() => {
                    // A newer link from the same machine took over.
                    let Some(call) = call else { break 'link };
                    next_call += 1;
                    let frame = HubFrame::HubCall {
                        call_id: next_call,
                        tool: call.tool,
                        arguments: call.arguments,
                        caller: call.caller,
                    };
                    waiting.insert(next_call, Waiting::Caller(call.reply));
                    if write(writer, send(&frame), times.write_timeout).await.is_none() {
                        break 'link;
                    }
                }
                _ = ping.tick() => {
                    if last_heard.elapsed() >= times.idle_timeout || !members.is_hub() {
                        break 'link;
                    }
                    if write(writer, Message::Ping(Default::default()), times.write_timeout).await.is_none() {
                        break 'link;
                    }
                }
                inbound = reader.next() => match inbound {
                    Some(Ok(Message::Text(text))) => {
                        last_heard = tokio::time::Instant::now();
                        let Ok(MemberFrame::HubResult { call_id, success, content }) =
                            serde_json::from_str::<MemberFrame>(text.as_str())
                        else {
                            continue;
                        };
                        match waiting.remove(&call_id) {
                            Some(Waiting::Caller(reply)) => {
                                let _ = reply.send(CallAnswer { success, content });
                            }
                            Some(Waiting::Mail(id)) => {
                                crate::dbg_log!(
                                    "hub: delivered a queued call to {} ({})",
                                    joined.name,
                                    if success { "ok" } else { "refused" }
                                );
                                members.delivered(&id);
                            }
                            None => {}
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {
                        last_heard = tokio::time::Instant::now();
                    }
                    Some(Ok(Message::Binary(_))) | Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break 'link,
                },
            }
        }
    }
    members.leave(&joined.host_id, epoch);
    crate::dbg_log!("hub: {} left", joined.name);
}

async fn write<W>(writer: &mut W, message: Message, deadline: Duration) -> Option<()>
where
    W: Sink<Message, Error = WsError> + Unpin,
{
    match tokio::time::timeout(deadline, writer.send(message)).await {
        Ok(Ok(())) => Some(()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        HubConfig {
            hub: true,
            member: None,
        }
        .save(dir.path())
        .unwrap();
        dir
    }

    fn times() -> LinkTimes {
        LinkTimes {
            write_timeout: Duration::from_secs(5),
            ping_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(90),
        }
    }

    fn joined(name: &str, access: Scope) -> Joined {
        Joined {
            host_id: format!("host-{name}"),
            name: name.to_owned(),
            access,
            ask: false,
        }
    }

    /// A member at the other end of an in-memory link: what the hub writes
    /// arrives on `from_hub`, and what is pushed on `to_hub` reaches the hub.
    struct Wire {
        from_hub: mpsc::UnboundedReceiver<Message>,
        to_hub: mpsc::UnboundedSender<Result<Message, WsError>>,
    }

    impl Wire {
        async fn frame(&mut self) -> HubFrame {
            loop {
                match tokio::time::timeout(Duration::from_secs(5), self.from_hub.recv())
                    .await
                    .expect("the hub writes in time")
                    .expect("the link is open")
                {
                    Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
                    _ => continue,
                }
            }
        }

        fn answer(&self, call_id: u64, content: &str) {
            let frame = MemberFrame::HubResult {
                call_id,
                success: true,
                content: content.to_owned(),
            };
            self.to_hub
                .send(Ok(Message::text(serde_json::to_string(&frame).unwrap())))
                .unwrap();
        }
    }

    fn link(
        members: std::sync::Arc<Members>,
        joined: Joined,
    ) -> (Wire, tokio::task::JoinHandle<()>) {
        let (hub_out, from_hub) = mpsc::unbounded_channel::<Message>();
        let (to_hub, hub_in) = mpsc::unbounded_channel::<Result<Message, WsError>>();
        let task = tokio::spawn(async move {
            let mut writer = Box::pin(futures_util::sink::unfold(
                hub_out,
                |out: mpsc::UnboundedSender<Message>, message: Message| async move {
                    out.send(message).map_err(|_| WsError::ConnectionClosed)?;
                    Ok::<_, WsError>(out)
                },
            ));
            let mut reader = Box::pin(futures_util::stream::unfold(
                hub_in,
                |mut hub_in| async move { hub_in.recv().await.map(|item| (item, hub_in)) },
            ));
            serve(&members, joined, &mut writer, &mut reader, times()).await;
        });
        (Wire { from_hub, to_hub }, task)
    }

    #[test]
    fn the_role_file_round_trips_and_defaults_to_no_role() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(HubConfig::load(dir.path()), HubConfig::default());
        let config = HubConfig {
            hub: false,
            member: Some(Membership {
                machine: "vps-id".into(),
                name: "vps".into(),
                access: Scope::Delegate,
                ask: true,
            }),
        };
        config.save(dir.path()).unwrap();
        assert_eq!(HubConfig::load(dir.path()), config);
        // A file written by hand without `ask` asks.
        std::fs::write(
            HubConfig::path(dir.path()),
            r#"{"member":{"machine":"a","name":"a","access":"read_only"}}"#,
        )
        .unwrap();
        assert!(HubConfig::load(dir.path()).member.unwrap().ask);
    }

    #[test]
    fn a_join_frame_is_told_from_everything_else() {
        assert_eq!(
            MemberFrame::parse_join(r#"{"type":"hub_join","access":"delegate","ask":true}"#),
            Some((Scope::Delegate, true))
        );
        assert_eq!(MemberFrame::parse_join(r#"{"type":"acp_open"}"#), None);
        assert_eq!(
            MemberFrame::parse_join(
                r#"{"type":"hub_result","call_id":1,"success":true,"content":""}"#
            ),
            None
        );
    }

    #[test]
    fn a_long_answer_is_cut_to_fit_one_frame() {
        let MemberFrame::HubResult { content, .. } =
            result_frame(1, true, "é".repeat(MAX_FRAME_BYTES))
        else {
            unreachable!()
        };
        assert!(content.len() < MAX_FRAME_BYTES);
        assert!(content.ends_with("hub link carries]"));
    }

    #[tokio::test]
    async fn a_call_reaches_a_connected_member_and_its_answer_comes_back() {
        let dir = hub_dir();
        let members = std::sync::Arc::new(Members::new(dir.path()));
        let (mut wire, _task) = link(members.clone(), joined("laptop", Scope::Delegate));
        assert_eq!(wire.frame().await, HubFrame::HubJoined);
        let call = {
            let members = members.clone();
            tokio::spawn(async move {
                members
                    .call("LAPTOP", "list_sessions", json!({}), "ChatGPT")
                    .await
            })
        };
        let HubFrame::HubCall {
            call_id,
            tool,
            caller,
            ..
        } = wire.frame().await
        else {
            panic!("expected a call")
        };
        assert_eq!(
            (tool.as_str(), caller.as_str()),
            ("list_sessions", "ChatGPT")
        );
        wire.answer(call_id, "three threads");
        assert_eq!(
            call.await.unwrap(),
            HubCallAnswer::Answered {
                success: true,
                content: "three threads".into()
            }
        );
        let listed = members.list();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].online);
        assert_eq!(listed[0].access, Scope::Delegate);
    }

    #[tokio::test]
    async fn an_unknown_machine_or_a_machine_that_is_no_hub_is_not_a_member() {
        let dir = hub_dir();
        let members = Members::new(dir.path());
        assert_eq!(
            members
                .call("nobody", "list_sessions", json!({}), "x")
                .await,
            HubCallAnswer::NotMember
        );
        let plain = tempfile::tempdir().unwrap();
        let members = Members::new(plain.path());
        assert_eq!(
            members
                .call("laptop", "list_sessions", json!({}), "x")
                .await,
            HubCallAnswer::NotMember
        );
    }

    #[tokio::test]
    async fn an_offline_member_gets_queued_work_on_its_next_join_and_nothing_else() {
        let dir = hub_dir();
        let members = std::sync::Arc::new(Members::new(dir.path()));
        let (mut wire, task) = link(members.clone(), joined("laptop", Scope::Delegate));
        assert_eq!(wire.frame().await, HubFrame::HubJoined);
        drop(wire);
        task.await.unwrap();
        assert!(!members.list()[0].online);

        // A read cannot wait; a start can, and says it has not run.
        let HubCallAnswer::Answered { success, content } = members
            .call("laptop", "list_sessions", json!({}), "x")
            .await
        else {
            panic!()
        };
        assert!(
            !success && content.contains("laptop is offline"),
            "{content}"
        );
        let HubCallAnswer::Answered { success, content } = members
            .call(
                "laptop",
                crate::orchestration::START_SESSION,
                json!({"prompt": "go"}),
                "ChatGPT",
            )
            .await
        else {
            panic!()
        };
        assert!(success && content.contains("\"queued\":true"), "{content}");
        assert_eq!(members.list()[0].queued, 1);

        // On the next join the call arrives with a retry key, and its answer
        // empties the mailbox.
        let (mut wire, _task) = link(members.clone(), joined("laptop", Scope::Delegate));
        assert_eq!(wire.frame().await, HubFrame::HubJoined);
        let HubFrame::HubCall {
            call_id,
            tool,
            arguments,
            ..
        } = wire.frame().await
        else {
            panic!()
        };
        assert_eq!(tool, crate::orchestration::START_SESSION);
        assert!(
            arguments[crate::orchestration::CLIENT_REQUEST_ID]
                .as_str()
                .unwrap()
                .starts_with("hub-")
        );
        wire.answer(call_id, "{}");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while members.list()[0].queued > 0 {
            assert!(tokio::time::Instant::now() < deadline, "never delivered");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn the_mailbox_is_bounded_per_machine() {
        let dir = hub_dir();
        let members = std::sync::Arc::new(Members::new(dir.path()));
        let (wire, task) = link(members.clone(), joined("laptop", Scope::Delegate));
        drop(wire);
        task.await.unwrap();
        for _ in 0..MAILBOX_PER_MACHINE {
            let answer = members
                .call(
                    "laptop",
                    crate::orchestration::MESSAGE_SESSION,
                    json!({}),
                    "x",
                )
                .await;
            assert!(matches!(
                answer,
                HubCallAnswer::Answered { success: true, .. }
            ));
        }
        let HubCallAnswer::Answered { success, content } = members
            .call(
                "laptop",
                crate::orchestration::MESSAGE_SESSION,
                json!({}),
                "x",
            )
            .await
        else {
            panic!()
        };
        assert!(!success && content.contains("as many calls"), "{content}");
    }

    #[tokio::test]
    async fn a_member_that_drops_mid_call_is_reported_and_a_newer_link_replaces_an_older_one() {
        let dir = hub_dir();
        let members = std::sync::Arc::new(Members::new(dir.path()));
        let (mut first, first_task) = link(members.clone(), joined("laptop", Scope::ReadOnly));
        assert_eq!(first.frame().await, HubFrame::HubJoined);
        let call = {
            let members = members.clone();
            tokio::spawn(
                async move { members.call("laptop", "read_session", json!({}), "x").await },
            )
        };
        let HubFrame::HubCall { .. } = first.frame().await else {
            panic!()
        };
        // The hub loses the member before it answers.
        drop(first);
        first_task.await.unwrap();
        let HubCallAnswer::Answered { success, content } = call.await.unwrap() else {
            panic!()
        };
        assert!(!success && content.contains("went offline"), "{content}");

        let (mut older, older_task) = link(members.clone(), joined("laptop", Scope::ReadOnly));
        assert_eq!(older.frame().await, HubFrame::HubJoined);
        let (mut newer, _newer_task) = link(members.clone(), joined("laptop", Scope::Delegate));
        assert_eq!(newer.frame().await, HubFrame::HubJoined);
        tokio::time::timeout(Duration::from_secs(5), older_task)
            .await
            .expect("the older link ends")
            .unwrap();
        let listed = members.list();
        assert!(listed[0].online, "the newer link is still registered");
        assert_eq!(listed[0].access, Scope::Delegate);
    }
}
