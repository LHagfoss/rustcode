//! The seam between a session owner and the remote gateway (issue #1908).
//!
//! An owner is the runtime that holds a session's `AppState` (the terminal in
//! v1). While it shares its session it holds one [`OwnerLink`]: a bounded
//! queue of [`OwnerMessage`]s towards the gateway and a bounded queue of
//! [`OwnerCommand`]s from it. The gateway's local IPC client implements
//! [`OwnerConnector`] and pumps the matching [`GatewayLink`] over its socket;
//! nothing in this module does I/O.
//!
//! Contract, owner side:
//!
//! - The first message on a link is [`OwnerMessage::Register`]. Dropping the
//!   link ends the registration; [`OwnerMessage::Unregister`] only adds the
//!   reason and may be lost when the queue is full.
//! - Messages are sent with `try_send` only. An owner that finds the queue
//!   full drops events, keeps counting their sequence numbers, and sends an
//!   [`OwnerMessage::Snapshot`] with `resync` set once there is room again.
//! - Every [`OwnerCommand::Request`] is answered exactly once through its
//!   [`OwnerReply`], after the owner applied or rejected it. A reply handle
//!   dropped unanswered means the owner went away: report `owner_unavailable`
//!   with receipt `unknown`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, oneshot};

use super::ops::SessionRegistration;
use super::protocol::{
    RemoteEventFrame, RemoteRequest, RemoteResponse, RemoteSessionInfo, RemoteSnapshot,
    ResyncReason, SessionCloseReason,
};

/// Messages an owner may have queued towards the gateway before it starts
/// dropping events and falls back to a resync snapshot.
pub const OWNER_OUTBOUND_CAPACITY: usize = 256;
/// Commands the gateway may have queued towards an owner. The gateway answers
/// `rate_limited` itself when this is full.
pub const OWNER_COMMAND_CAPACITY: usize = 32;

/// What `/remote` asks the session owner to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharingCommand {
    /// `/remote`: share the session. Repeating it changes nothing.
    Enable,
    /// `/remote status`
    Status,
    /// `/remote off`: stop sharing; a running turn is left alone.
    Off,
}

impl SharingCommand {
    /// Parse the words after `/remote`. `Err` is the usage text.
    pub fn parse(arguments: &[&str]) -> Result<Self, String> {
        match arguments {
            [] => Ok(Self::Enable),
            ["status"] => Ok(Self::Status),
            ["off"] => Ok(Self::Off),
            _ => Err("Usage:\n  /remote  Share this session with a paired device\n  /remote status  Show whether this session is shared\n  /remote off  Stop sharing this session".to_owned()),
        }
    }
}

/// Owner → gateway, in the order the owner applied the underlying changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerMessage {
    /// Opens the registration. `snapshot.session` is the session-list row and
    /// `snapshot.sequence` (0) the watermark the first event follows.
    Register {
        registration: SessionRegistration,
        snapshot: Box<RemoteSnapshot>,
    },
    /// One applied update. `sequence` increases by one per event the owner
    /// produced, including events it had to drop: a gap means events were
    /// lost and a resync snapshot follows.
    Event(RemoteEventFrame),
    /// Authoritative state at `snapshot.sequence`: every event up to that
    /// sequence is contained in it and every later event follows it on this
    /// link. `resync` is set when subscribers must discard what they hold
    /// (events were dropped, or the transcript was rewritten); it is `None`
    /// for an answer to [`OwnerCommand::SnapshotRequested`] and for a change
    /// no event describes.
    Snapshot {
        snapshot: Box<RemoteSnapshot>,
        resync: Option<ResyncReason>,
    },
    /// The session-list row changed. Carries no sequence.
    SessionInfo(RemoteSessionInfo),
    /// Sharing ended; the link closes after this message.
    Unregister { reason: SessionCloseReason },
}

/// What the gateway reports about a registration, for `/remote status`.
/// Device names are labels a device chose; no credential is ever part of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GatewayLinkStatus {
    /// False while the link is trying to reach a gateway again.
    pub connected: bool,
    /// `host:port` a device dials.
    pub advertised_address: String,
    /// The gateway listens on loopback only: no other machine can reach it.
    pub loopback_only: bool,
    /// Devices attached to this session.
    pub attached_devices: Vec<String>,
    /// Devices connected to the gateway, attached to this session or not.
    pub connected_devices: Vec<String>,
}

/// Gateway → owner.
#[derive(Debug)]
pub enum OwnerCommand {
    /// A session operation from an authenticated device: a mutation,
    /// `get_history` or `get_content`. The owner validates the session ID,
    /// the registration epoch and every identity the operation names at the
    /// point it applies it.
    Request {
        request: RemoteRequest,
        reply: OwnerReply,
    },
    /// The gateway needs current state (a device attached and replay is not
    /// possible). Answered by an [`OwnerMessage::Snapshot`] on the link.
    SnapshotRequested,
    /// The gateway's view of the registration changed: it was accepted, a
    /// device attached or left, or the gateway became unreachable. Sent
    /// without waiting; only the latest one matters.
    Status(GatewayLinkStatus),
    /// The gateway ended the registration, for example because another live
    /// owner already shares this session. The owner stops sharing.
    Closed { reason: String },
}

/// Reply handle for one [`OwnerCommand::Request`].
#[derive(Debug)]
pub struct OwnerReply(oneshot::Sender<RemoteResponse>);

impl OwnerReply {
    pub fn channel() -> (Self, oneshot::Receiver<RemoteResponse>) {
        let (sender, receiver) = oneshot::channel();
        (Self(sender), receiver)
    }

    /// Deliver the owner's decision. Never blocks; a gateway that stopped
    /// waiting is not an error for the owner.
    pub fn send(self, response: RemoteResponse) {
        let _ = self.0.send(response);
    }
}

/// The owner's end of a registration.
#[derive(Debug)]
pub struct OwnerLink {
    pub outbound: mpsc::Sender<OwnerMessage>,
    pub commands: mpsc::Receiver<OwnerCommand>,
}

/// The gateway's end of a registration: what an [`OwnerConnector`] pumps.
#[derive(Debug)]
pub struct GatewayLink {
    pub messages: mpsc::Receiver<OwnerMessage>,
    pub commands: mpsc::Sender<OwnerCommand>,
}

impl GatewayLink {
    /// Queue `request` for the owner without waiting. `Err` returns the
    /// request when the command queue is full or the owner is gone; nothing
    /// was applied in either case.
    pub fn try_request(
        &self,
        request: RemoteRequest,
    ) -> Result<oneshot::Receiver<RemoteResponse>, RemoteRequest> {
        let (reply, response) = OwnerReply::channel();
        match self
            .commands
            .try_send(OwnerCommand::Request { request, reply })
        {
            Ok(()) => Ok(response),
            Err(
                mpsc::error::TrySendError::Full(command)
                | mpsc::error::TrySendError::Closed(command),
            ) => match command {
                OwnerCommand::Request { request, .. } => Err(request),
                _ => unreachable!("the rejected command is the request just built"),
            },
        }
    }
}

/// Both ends of one registration's queues.
pub fn owner_link_pair(
    outbound_capacity: usize,
    command_capacity: usize,
) -> (OwnerLink, GatewayLink) {
    let (outbound, messages) = mpsc::channel(outbound_capacity.max(1));
    let (commands_tx, commands_rx) = mpsc::channel(command_capacity.max(1));
    (
        OwnerLink {
            outbound,
            commands: commands_rx,
        },
        GatewayLink {
            messages,
            commands: commands_tx,
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerLinkError {
    /// No gateway is running, or this build has none to connect to.
    NoGateway,
    /// A gateway exists but refused or could not take the registration.
    Unavailable(String),
}

impl std::fmt::Display for OwnerLinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGateway => f.write_str("no remote gateway is available"),
            Self::Unavailable(detail) => write!(f, "the remote gateway is unavailable: {detail}"),
        }
    }
}

impl std::error::Error for OwnerLinkError {}

/// Opens a link for one registration. Called from the owner's event loop, so
/// it must return without waiting: an implementation that has to dial a
/// socket hands back the link at once, pumps it from its own task, and
/// reports a later failure with [`OwnerCommand::Closed`].
pub trait OwnerConnector: Send {
    fn connect(&mut self, registration: &SessionRegistration) -> Result<OwnerLink, OwnerLinkError>;

    /// The configuration directory of the gateway this connector reaches,
    /// which is where pairing details for `/remote` are asked for. `None`
    /// when there is no such gateway (in-memory links, unsupported
    /// platforms): `/remote` then shows no pairing details.
    fn gateway_directory(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// The connector of a build without a gateway: sharing cannot start.
#[derive(Debug, Default)]
pub struct NoGatewayConnector;

impl OwnerConnector for NoGatewayConnector {
    fn connect(
        &mut self,
        _registration: &SessionRegistration,
    ) -> Result<OwnerLink, OwnerLinkError> {
        Err(OwnerLinkError::NoGateway)
    }
}

/// The connector a session owner uses at runtime: the gateway's local IPC
/// client, which discovers the running gateway or starts one. Platforms
/// without the gateway, and a process without a configuration directory, get
/// [`NoGatewayConnector`].
pub fn default_connector() -> Box<dyn OwnerConnector> {
    #[cfg(unix)]
    if let Some(connector) = crate::remote_gateway::owner_client::GatewayConnector::for_this_host()
    {
        return Box::new(connector);
    }
    Box::new(NoGatewayConnector)
}

/// A connector whose gateway is the caller, for tests and in-process use.
#[derive(Debug)]
pub struct InMemoryConnector {
    accepted: std::sync::mpsc::Sender<(SessionRegistration, GatewayLink)>,
    outbound_capacity: usize,
    command_capacity: usize,
}

/// Receives the gateway end of every link an [`InMemoryConnector`] opened.
#[derive(Debug)]
pub struct InMemoryGateway {
    accepted: std::sync::mpsc::Receiver<(SessionRegistration, GatewayLink)>,
}

impl InMemoryConnector {
    pub fn new(outbound_capacity: usize, command_capacity: usize) -> (Self, InMemoryGateway) {
        let (sender, receiver) = std::sync::mpsc::channel();
        (
            Self {
                accepted: sender,
                outbound_capacity,
                command_capacity,
            },
            InMemoryGateway { accepted: receiver },
        )
    }
}

impl OwnerConnector for InMemoryConnector {
    fn connect(&mut self, registration: &SessionRegistration) -> Result<OwnerLink, OwnerLinkError> {
        let (owner, gateway) = owner_link_pair(self.outbound_capacity, self.command_capacity);
        self.accepted
            .send((registration.clone(), gateway))
            .map_err(|_| OwnerLinkError::NoGateway)?;
        Ok(owner)
    }
}

impl InMemoryGateway {
    /// The next link opened since the last call, if any.
    pub fn accept(&self) -> Option<(SessionRegistration, GatewayLink)> {
        self.accepted.try_recv().ok()
    }
}

/// A registration epoch for a new enable: wall-clock milliseconds, forced to
/// increase within the process. A session shared again, by this process or
/// by the one that resumes it later, never reuses an epoch a client may
/// still hold.
pub fn next_registration_epoch() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(last + 1);
        match LAST.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(observed) => last = observed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::protocol::{
        REMOTE_PROTOCOL_VERSION, ReceiptState, RemoteOperation, RemoteResult,
    };

    fn registration() -> SessionRegistration {
        SessionRegistration {
            session_id: "owner-link-session".to_owned(),
            registration_epoch: next_registration_epoch(),
        }
    }

    fn request(request_id: &str) -> RemoteRequest {
        RemoteRequest {
            authenticated_device_id: None,
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            session_id: Some("owner-link-session".to_owned()),
            registration_epoch: Some(1),
            operation: RemoteOperation::DetachSession,
        }
    }

    #[test]
    fn sharing_commands_parse_their_arguments() {
        assert_eq!(SharingCommand::parse(&[]), Ok(SharingCommand::Enable));
        assert_eq!(
            SharingCommand::parse(&["status"]),
            Ok(SharingCommand::Status)
        );
        assert_eq!(SharingCommand::parse(&["off"]), Ok(SharingCommand::Off));
        for unknown in [&["on"][..], &["off", "now"][..], &["serve"][..]] {
            let usage = SharingCommand::parse(unknown).expect_err("unknown arguments");
            assert!(usage.contains("/remote off"), "unexpected usage: {usage}");
        }
    }

    #[test]
    fn registration_epochs_never_repeat() {
        let first = next_registration_epoch();
        let second = next_registration_epoch();
        let third = next_registration_epoch();
        assert!(first < second && second < third);
    }

    #[test]
    fn a_build_without_a_gateway_cannot_open_a_link() {
        let error = NoGatewayConnector
            .connect(&registration())
            .expect_err("there is no gateway to connect to");
        assert_eq!(error, OwnerLinkError::NoGateway);
        assert_eq!(error.to_string(), "no remote gateway is available");
    }

    #[tokio::test]
    async fn in_memory_links_carry_requests_and_their_replies() {
        let (mut connector, gateway) = InMemoryConnector::new(4, 1);
        let registration = registration();
        let mut owner = connector.connect(&registration).expect("link opens");
        let (accepted, link) = gateway.accept().expect("the gateway sees the link");
        assert_eq!(accepted, registration);
        assert!(gateway.accept().is_none());

        let response = link.try_request(request("r-1")).expect("queued");
        // The command queue is bounded: the second request comes straight back.
        let refused = link.try_request(request("r-2")).expect_err("queue is full");
        assert_eq!(refused.request_id, "r-2");

        let Some(OwnerCommand::Request { request, reply }) = owner.commands.recv().await else {
            panic!("the owner receives the queued request");
        };
        reply.send(
            RemoteResponse::new(request.request_id, RemoteResult::Detached)
                .with_receipt(ReceiptState::Applied),
        );
        let response = response.await.expect("the owner replied");
        assert_eq!(response.request_id, "r-1");
        assert_eq!(response.receipt, Some(ReceiptState::Applied));
    }

    #[tokio::test]
    async fn a_dropped_reply_handle_reports_an_absent_owner() {
        let (owner, link) = owner_link_pair(1, 1);
        let response = link.try_request(request("r-1")).expect("queued");
        drop(owner);
        assert!(response.await.is_err());
        assert!(link.try_request(request("r-2")).is_err());
    }
}
