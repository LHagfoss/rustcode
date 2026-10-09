//! The terminal's end of the owner socket: the [`OwnerConnector`] behind
//! `/remote`.
//!
//! [`GatewayConnector::connect`] hands the owner its link at once and does
//! everything else on a task of its own: find the running gateway or start
//! one, register, move frames both ways, send heartbeats, and report loss
//! with [`OwnerCommand::Closed`]. The owner's event loop never waits on it.
//!
//! Receipts live here, in the owner's process, for as long as the
//! registration does. That is what lets them outlive the gateway: when the
//! gateway goes away the task keeps the registration, looks for a gateway
//! again and registers under the same epoch with a fresh snapshot, and a
//! device that asks what became of a request still gets the original answer.
//! A mutation is applied at most once per device and request ID; the same ID
//! with another payload is refused, and when the store is full new mutations
//! are refused rather than old receipts forgotten.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use sha2::{Digest, Sha256};
use tokio::io::BufReader;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::lifecycle::RemoteLifecycle;
use super::owner_ipc::{GatewayFrame, OWNER_IPC_VERSION, OwnerFrame};
use crate::daemon::protocol::{read_async_frame_with_buffer, write_async_frame};
use crate::remote::owner::{
    GatewayLink, GatewayLinkStatus, OWNER_COMMAND_CAPACITY, OWNER_OUTBOUND_CAPACITY, OwnerCommand,
    OwnerConnector, OwnerLink, OwnerLinkError, OwnerMessage, owner_link_pair,
};
use crate::remote::{
    ReceiptState, RemoteError, RemoteErrorCode, RemoteOperation, RemoteRequest, RemoteResponse,
    RemoteResult, RemoteSnapshot, SessionRegistration,
};

/// Mutation receipts one registration keeps. Past this, new mutations are
/// refused with `receipt_capacity`.
pub const RECEIPT_CAPACITY: usize = 1024;

/// Duplicates of one in-flight mutation that wait for its outcome.
const MAX_WAITERS: usize = 4;

/// Timing of one connector. The defaults are the production values.
#[derive(Debug, Clone, Copy)]
pub struct ClientTiming {
    pub heartbeat: Duration,
    /// Gateway silence after which the connection counts as lost.
    pub gateway_timeout: Duration,
    pub write_timeout: Duration,
    /// How long a lost gateway is looked for before sharing stops.
    pub reconnect_window: Duration,
    pub receipt_capacity: usize,
}

impl Default for ClientTiming {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(5),
            gateway_timeout: Duration::from_secs(20),
            write_timeout: Duration::from_secs(10),
            reconnect_window: Duration::from_secs(120),
            receipt_capacity: RECEIPT_CAPACITY,
        }
    }
}

/// How `/remote` starts a gateway when none is running: this program's own
/// `remote serve`, detached, on the address the user configured.
#[derive(Debug, Clone)]
pub struct Launcher {
    pub program: PathBuf,
    pub config_directory: PathBuf,
    pub bind: Option<String>,
    pub port: Option<u16>,
    pub advertise: Option<String>,
}

impl Launcher {
    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .env("RUSTCODE_CONFIG_DIR", &self.config_directory)
            .args(["remote", "serve"]);
        if let Some(bind) = &self.bind {
            command.args(["--bind", bind]);
        }
        if let Some(port) = self.port {
            command.args(["--port", &port.to_string()]);
        }
        if let Some(advertise) = &self.advertise {
            command.args(["--advertise", advertise]);
        }
        command
    }
}

/// Connects session owners to the gateway of one configuration directory.
#[derive(Clone)]
pub struct GatewayConnector {
    lifecycle: RemoteLifecycle,
    launcher: Option<Launcher>,
    timing: ClientTiming,
}

impl GatewayConnector {
    /// A connector that only uses a gateway that is already running.
    pub fn discover(lifecycle: RemoteLifecycle) -> Self {
        Self {
            lifecycle,
            launcher: None,
            timing: ClientTiming::default(),
        }
    }

    pub fn with_launcher(mut self, launcher: Launcher) -> Self {
        self.launcher = Some(launcher);
        self
    }

    pub fn with_timing(mut self, timing: ClientTiming) -> Self {
        self.timing = timing;
        self
    }

    /// The connector of this process: the user's configuration directory,
    /// and this executable to start a gateway with the `[remote]` address
    /// from the user's config. `None` without a configuration directory.
    pub fn for_this_host() -> Option<Self> {
        let config_directory = crate::config::get_config_dir()?;
        let connector = Self::discover(RemoteLifecycle::new(&config_directory));
        let Ok(program) = std::env::current_exe() else {
            return Some(connector);
        };
        // Read when sharing starts, not here: the user may edit the address
        // between two `/remote`.
        Some(connector.with_launcher(Launcher {
            program,
            config_directory,
            bind: None,
            port: None,
            advertise: None,
        }))
    }
}

impl OwnerConnector for GatewayConnector {
    fn connect(&mut self, registration: &SessionRegistration) -> Result<OwnerLink, OwnerLinkError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| OwnerLinkError::Unavailable("no async runtime is running".to_owned()))?;
        let (owner, gateway) = owner_link_pair(OWNER_OUTBOUND_CAPACITY, OWNER_COMMAND_CAPACITY);
        let client = Client {
            connector: self.clone(),
            registration: registration.clone(),
            link: gateway,
            receipts: Receipts::new(self.timing.receipt_capacity),
            in_flight: FuturesUnordered::new(),
            generation: 0,
            status: GatewayLinkStatus::default(),
            status_due: false,
            snapshot_due: false,
        };
        runtime.spawn(client.run());
        Ok(owner)
    }

    fn gateway_directory(&self) -> Option<PathBuf> {
        Some(self.lifecycle.config_directory())
    }
}

type ReceiptKey = (String, String);

enum Receipt {
    /// Handed to the owner, not decided yet. The numbers are the requests
    /// (connection generation, request number) waiting for the outcome.
    InFlight {
        digest: [u8; 32],
        waiters: Vec<(u64, u64)>,
    },
    Done {
        digest: [u8; 32],
        response: RemoteResponse,
    },
}

enum Begin {
    New,
    /// The same request is already with the owner; its outcome answers this
    /// one too.
    Joined,
    Replay(RemoteResponse),
    Conflict,
    Full,
    /// Too many duplicates are already waiting.
    Busy,
}

/// Mutation receipts of one registration, keyed by device and request ID.
struct Receipts {
    capacity: usize,
    entries: HashMap<ReceiptKey, Receipt>,
}

impl Receipts {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
        }
    }

    fn digest(request: &RemoteRequest) -> [u8; 32] {
        let payload = serde_json::to_vec(&(
            &request.session_id,
            request.registration_epoch,
            &request.operation,
        ))
        .expect("requests always serialize");
        Sha256::digest(payload).into()
    }

    fn begin(&mut self, key: &ReceiptKey, digest: [u8; 32], waiter: (u64, u64)) -> Begin {
        let full = self.entries.len() >= self.capacity;
        match self.entries.get_mut(key) {
            Some(Receipt::Done {
                digest: recorded,
                response,
            }) if *recorded == digest => Begin::Replay(response.clone()),
            Some(Receipt::InFlight {
                digest: recorded,
                waiters,
            }) if *recorded == digest => {
                if waiters.len() >= MAX_WAITERS {
                    return Begin::Busy;
                }
                waiters.push(waiter);
                Begin::Joined
            }
            Some(_) => Begin::Conflict,
            None if full => Begin::Full,
            None => {
                self.entries.insert(
                    key.clone(),
                    Receipt::InFlight {
                        digest,
                        waiters: vec![waiter],
                    },
                );
                Begin::New
            }
        }
    }

    /// The owner never took the request: it leaves no receipt.
    fn forget(&mut self, key: &ReceiptKey) {
        self.entries.remove(key);
    }

    /// Record the outcome and return who is waiting for it.
    fn finish(&mut self, key: &ReceiptKey, response: &RemoteResponse) -> Vec<(u64, u64)> {
        match self.entries.remove(key) {
            Some(Receipt::InFlight { digest, waiters }) => {
                self.entries.insert(
                    key.clone(),
                    Receipt::Done {
                        digest,
                        response: response.clone(),
                    },
                );
                waiters
            }
            Some(done) => {
                self.entries.insert(key.clone(), done);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn status(
        &self,
        device_id: &str,
        target_request_id: &str,
    ) -> (ReceiptState, Option<RemoteResult>) {
        match self
            .entries
            .get(&(device_id.to_owned(), target_request_id.to_owned()))
        {
            Some(Receipt::InFlight { .. }) => (ReceiptState::Received, None),
            Some(Receipt::Done { response, .. }) => (
                response.receipt.unwrap_or(ReceiptState::Unknown),
                Some(response.result.clone()),
            ),
            None => (ReceiptState::Unknown, None),
        }
    }
}

/// One request handed to the owner and not answered yet.
struct InFlight {
    /// Set for mutations.
    key: Option<ReceiptKey>,
    waiter: (u64, u64),
    request_id: String,
    response: Option<RemoteResponse>,
}

type InFlightFuture = std::pin::Pin<Box<dyn std::future::Future<Output = InFlight> + Send + Sync>>;

struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    buffer: Vec<u8>,
}

/// Why serving a connection stopped.
enum Ended {
    /// The gateway went away; look for it again.
    Lost,
    /// The owner stopped sharing or went away.
    Owner,
    /// The gateway ended the registration.
    Closed(String),
}

struct Client {
    connector: GatewayConnector,
    registration: SessionRegistration,
    link: GatewayLink,
    receipts: Receipts,
    in_flight: FuturesUnordered<InFlightFuture>,
    /// Counts gateway connections, so an answer is never written to a
    /// connection other than the one that asked.
    generation: u64,
    status: GatewayLinkStatus,
    /// The owner's command queue had no room for the latest status.
    status_due: bool,
    snapshot_due: bool,
}

fn rejection(
    request_id: &str,
    mutation: bool,
    code: RemoteErrorCode,
    message: &str,
) -> RemoteResponse {
    let response = RemoteResponse::error(request_id, RemoteError::new(code, message));
    if mutation {
        response.with_receipt(ReceiptState::Rejected)
    } else {
        response
    }
}

impl Client {
    async fn run(mut self) {
        let timing = self.connector.timing;
        // The owner queues its registration right after `connect` returns.
        let Some(OwnerMessage::Register { snapshot, .. }) = self.link.messages.recv().await else {
            return;
        };
        let mut snapshot = Some(snapshot);
        let mut deadline = None;
        loop {
            let connection = match self.establish(snapshot.take(), deadline).await {
                Ok(connection) => connection,
                Err(Ended::Owner) => return,
                Err(Ended::Closed(reason)) => return self.close(reason).await,
                Err(Ended::Lost) => {
                    return self
                        .close("the remote gateway went away and did not come back".to_owned())
                        .await;
                }
            };
            match self.serve(connection).await {
                Ended::Owner => return,
                Ended::Closed(reason) => return self.close(reason).await,
                Ended::Lost => {
                    self.status.connected = false;
                    self.status.attached_devices.clear();
                    self.status.connected_devices.clear();
                    self.send_status();
                    deadline = Some(Instant::now() + timing.reconnect_window);
                }
            }
        }
    }

    async fn close(self, reason: String) {
        // The owner may be busy; dropping the link afterwards ends the
        // registration either way.
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.link.commands.send(OwnerCommand::Closed { reason }),
        )
        .await;
    }

    fn send_status(&mut self) {
        self.status_due = self
            .link
            .commands
            .try_send(OwnerCommand::Status(self.status.clone()))
            .is_err();
    }

    fn request_snapshot(&mut self) {
        self.snapshot_due = self
            .link
            .commands
            .try_send(OwnerCommand::SnapshotRequested)
            .is_err();
    }

    /// Find or start the gateway. The first connection may start one; a
    /// reconnect only looks for one, so a gateway the user stopped is not
    /// brought back behind their back.
    async fn gateway(&self, first: bool) -> Result<(), String> {
        let lifecycle = &self.connector.lifecycle;
        match lifecycle.status().await {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(error) => return Err(format!("{error:#}")),
        }
        let Some(launcher) = self.connector.launcher.as_ref().filter(|_| first) else {
            return Err(
                "no remote gateway is running; start one with `rustcode remote serve`".to_owned(),
            );
        };
        let mut launcher = launcher.clone();
        if launcher.bind.is_none() && launcher.advertise.is_none() && launcher.port.is_none() {
            let (_, _, config) = crate::config::load_config();
            launcher.bind = config.remote.bind;
            launcher.port = config.remote.port;
            launcher.advertise = config.remote.advertise;
        }
        lifecycle
            .start(launcher.command())
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:#}"))
    }

    /// Open a connection and register on it. `snapshot` is the one that
    /// opened the registration; a reconnect (`deadline` set) asks the owner
    /// for a current one instead, so the gateway starts from the state the
    /// owner is in now.
    async fn establish(
        &mut self,
        snapshot: Option<Box<RemoteSnapshot>>,
        deadline: Option<Instant>,
    ) -> Result<Connection, Ended> {
        let first = deadline.is_none();
        let mut backoff = Duration::from_millis(100);
        let stream = loop {
            let attempt = async {
                self.gateway(first).await?;
                let path = self.connector.lifecycle.owner_socket_path();
                tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(path))
                    .await
                    .map_err(|_| "the gateway did not accept the connection".to_owned())?
                    .map_err(|error| {
                        format!("the gateway's owner socket is not reachable: {error}")
                    })
            };
            match attempt.await {
                Ok(stream) => break stream,
                Err(reason) if first => return Err(Ended::Closed(reason)),
                Err(_) => {}
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(Ended::Lost);
            }
            self.idle(backoff).await?;
            backoff = (backoff * 2).min(Duration::from_secs(2));
        };
        let snapshot = match snapshot {
            Some(snapshot) => snapshot,
            None => self.current_snapshot().await?,
        };
        self.generation += 1;
        let (reader, writer) = stream.into_split();
        let mut connection = Connection {
            reader: BufReader::new(reader),
            writer,
            buffer: Vec::new(),
        };
        let register = OwnerFrame::Register {
            ipc_version: OWNER_IPC_VERSION,
            registration: self.registration.clone(),
            snapshot,
        };
        if !self.write(&mut connection, &register).await {
            return Err(self.retry_or_fail(first, "the gateway closed the connection"));
        }
        let answer = tokio::time::timeout(
            Duration::from_secs(5),
            read_async_frame_with_buffer::<_, GatewayFrame>(
                &mut connection.reader,
                &mut connection.buffer,
            ),
        )
        .await;
        match answer {
            Ok(Ok(GatewayFrame::Registered { status, .. })) => {
                self.status = status;
                self.send_status();
                Ok(connection)
            }
            Ok(Ok(GatewayFrame::Closed { reason })) => Err(Ended::Closed(reason)),
            _ => Err(self.retry_or_fail(first, "the gateway did not accept the registration")),
        }
    }

    fn retry_or_fail(&self, first: bool, reason: &str) -> Ended {
        if first {
            Ended::Closed(reason.to_owned())
        } else {
            Ended::Lost
        }
    }

    /// Wait without a gateway: discard what the owner publishes (the
    /// snapshot of the next registration contains it) and keep recording the
    /// outcome of requests the owner still holds.
    async fn idle(&mut self, duration: Duration) -> Result<(), Ended> {
        let sleep = tokio::time::sleep(duration);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => return Ok(()),
                message = self.link.messages.recv() => match message {
                    None | Some(OwnerMessage::Unregister { .. }) => return Err(Ended::Owner),
                    Some(_) => {}
                },
                Some(done) = self.in_flight.next(), if !self.in_flight.is_empty() => {
                    self.record(done);
                }
            }
        }
    }

    /// Ask the owner for its current state and wait for it.
    async fn current_snapshot(&mut self) -> Result<Box<RemoteSnapshot>, Ended> {
        self.request_snapshot();
        let mut retry = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                message = self.link.messages.recv() => match message {
                    None | Some(OwnerMessage::Unregister { .. }) => return Err(Ended::Owner),
                    Some(OwnerMessage::Snapshot { snapshot, .. }) => return Ok(snapshot),
                    Some(_) => {}
                },
                Some(done) = self.in_flight.next(), if !self.in_flight.is_empty() => {
                    self.record(done);
                }
                _ = retry.tick() => {
                    if self.snapshot_due {
                        self.request_snapshot();
                    }
                }
            }
        }
    }

    async fn write(&self, connection: &mut Connection, frame: &OwnerFrame) -> bool {
        matches!(
            tokio::time::timeout(
                self.connector.timing.write_timeout,
                write_async_frame(&mut connection.writer, frame),
            )
            .await,
            Ok(Ok(()))
        )
    }

    /// Record the outcome of a request and return the answers to write on
    /// the current connection.
    fn record(&mut self, done: InFlight) -> Vec<OwnerFrame> {
        let mutation = done.key.is_some();
        let response = done.response.unwrap_or_else(|| {
            // The owner dropped the request unanswered: it is going away.
            let response = RemoteResponse::error(
                done.request_id.clone(),
                RemoteError::new(
                    RemoteErrorCode::OwnerUnavailable,
                    "the session owner did not answer",
                ),
            );
            if mutation {
                response.with_receipt(ReceiptState::Unknown)
            } else {
                response
            }
        });
        let waiters = match &done.key {
            Some(key) => self.receipts.finish(key, &response),
            None => vec![done.waiter],
        };
        waiters
            .into_iter()
            .filter(|(generation, _)| *generation == self.generation)
            .map(|(_, id)| OwnerFrame::Reply {
                id,
                response: response.clone(),
            })
            .collect()
    }

    /// Decide one request from the gateway. Returns the answer when there is
    /// one already.
    fn accept(&mut self, id: u64, device_id: String, request: RemoteRequest) -> Option<OwnerFrame> {
        let answer = |response| Some(OwnerFrame::Reply { id, response });
        let request_id = request.request_id.clone();
        if let RemoteOperation::GetRequestStatus { target_request_id } = &request.operation {
            let (receipt, result) = self.receipts.status(&device_id, target_request_id);
            return answer(RemoteResponse::new(
                request_id,
                RemoteResult::RequestStatus {
                    target_request_id: target_request_id.clone(),
                    receipt,
                    result: result.map(Box::new),
                },
            ));
        }
        let mutation = request.operation.is_mutation();
        let waiter = (self.generation, id);
        let key = mutation.then(|| (device_id, request_id.clone()));
        if let Some(key) = &key {
            match self.receipts.begin(key, Receipts::digest(&request), waiter) {
                Begin::New => {}
                Begin::Joined => return None,
                Begin::Replay(response) => return answer(response),
                Begin::Conflict => {
                    return answer(rejection(
                        &request_id,
                        true,
                        RemoteErrorCode::RequestConflict,
                        "this request_id was already used with a different payload",
                    ));
                }
                Begin::Full => {
                    return answer(rejection(
                        &request_id,
                        true,
                        RemoteErrorCode::ReceiptCapacity,
                        "the session owner cannot record another receipt; nothing was applied",
                    ));
                }
                Begin::Busy => {
                    return answer(rejection(
                        &request_id,
                        true,
                        RemoteErrorCode::RateLimited,
                        "this request is already being applied",
                    ));
                }
            }
        }
        match self.link.try_request(request) {
            Ok(response) => {
                self.in_flight
                    .push(Box::pin(wait(key, waiter, request_id, response)));
                None
            }
            Err(_) => {
                // Nothing reached the owner, so the ID stays free to retry.
                if let Some(key) = &key {
                    self.receipts.forget(key);
                }
                answer(rejection(
                    &request_id,
                    mutation,
                    RemoteErrorCode::RateLimited,
                    "the session owner is busy; nothing was applied",
                ))
            }
        }
    }

    /// Write the answers the owner has already given. Used before the
    /// registration ends, so a request rejected on the way out is told so.
    async fn flush_answers(&mut self, connection: &mut Connection) {
        use futures_util::FutureExt;
        while let Some(Some(done)) = self.in_flight.next().now_or_never() {
            for frame in self.record(done) {
                if !self.write(connection, &frame).await {
                    return;
                }
            }
        }
    }

    async fn serve(&mut self, mut connection: Connection) -> Ended {
        let timing = self.connector.timing;
        let mut heartbeat =
            tokio::time::interval_at(Instant::now() + timing.heartbeat, timing.heartbeat);
        let mut last_heard = Instant::now();
        loop {
            tokio::select! {
                message = self.link.messages.recv() => {
                    let frame = match message {
                        None => {
                            self.flush_answers(&mut connection).await;
                            return Ended::Owner;
                        }
                        Some(OwnerMessage::Unregister { reason }) => {
                            self.flush_answers(&mut connection).await;
                            if self.write(&mut connection, &OwnerFrame::Unregister { reason }).await {
                                // Keep the read half alive until the gateway processes
                                // Unregister and closes. It may still be writing a
                                // queued status: an early socket close makes that
                                // write fail before it reads our explicit reason.
                                let _ = tokio::time::timeout(timing.write_timeout, async {
                                    while let Ok(frame) = read_async_frame_with_buffer::<_, GatewayFrame>(
                                        &mut connection.reader,
                                        &mut connection.buffer,
                                    ).await {
                                        if matches!(frame, GatewayFrame::Closed { .. }) {
                                            break;
                                        }
                                    }
                                }).await;
                            }
                            return Ended::Owner;
                        }
                        Some(OwnerMessage::Register { .. }) => continue,
                        Some(OwnerMessage::Event(frame)) => OwnerFrame::Event { frame },
                        Some(OwnerMessage::Snapshot { snapshot, resync }) => {
                            OwnerFrame::Snapshot { snapshot, resync }
                        }
                        Some(OwnerMessage::SessionInfo(info)) => OwnerFrame::SessionInfo { info },
                    };
                    if !self.write(&mut connection, &frame).await {
                        return Ended::Lost;
                    }
                }
                // The partial frame lives in the connection, so cancelling
                // this read for another branch loses nothing.
                inbound = read_async_frame_with_buffer::<_, GatewayFrame>(
                    &mut connection.reader,
                    &mut connection.buffer,
                ) => {
                    let Ok(frame) = inbound else {
                        return Ended::Lost;
                    };
                    last_heard = Instant::now();
                    match frame {
                        GatewayFrame::Request { id, device_id, request } => {
                            if let Some(answer) = self.accept(id, device_id, request)
                                && !self.write(&mut connection, &answer).await
                            {
                                return Ended::Lost;
                            }
                        }
                        GatewayFrame::SnapshotRequested => self.request_snapshot(),
                        GatewayFrame::Status { status } => {
                            self.status = status;
                            self.send_status();
                        }
                        GatewayFrame::Closed { reason } => return Ended::Closed(reason),
                        GatewayFrame::Registered { .. } | GatewayFrame::Pong => {}
                    }
                }
                Some(done) = self.in_flight.next(), if !self.in_flight.is_empty() => {
                    for frame in self.record(done) {
                        if !self.write(&mut connection, &frame).await {
                            return Ended::Lost;
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    if last_heard.elapsed() >= timing.gateway_timeout
                        || !self.write(&mut connection, &OwnerFrame::Ping).await
                    {
                        return Ended::Lost;
                    }
                    if self.status_due {
                        self.send_status();
                    }
                    if self.snapshot_due {
                        self.request_snapshot();
                    }
                }
            }
        }
    }
}

async fn wait(
    key: Option<ReceiptKey>,
    waiter: (u64, u64),
    request_id: String,
    response: oneshot::Receiver<RemoteResponse>,
) -> InFlight {
    InFlight {
        key,
        waiter,
        request_id,
        response: response.await.ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::REMOTE_PROTOCOL_VERSION;

    #[tokio::test]
    async fn unregister_keeps_the_socket_open_until_the_gateway_reads_the_reason() {
        let directory = tempfile::tempdir().unwrap();
        let (owner, gateway) = owner_link_pair(8, 8);
        let mut client = Client {
            connector: GatewayConnector::discover(RemoteLifecycle::new(directory.path())),
            registration: SessionRegistration {
                session_id: "s".into(),
                registration_epoch: 1,
            },
            link: gateway,
            receipts: Receipts::new(8),
            in_flight: FuturesUnordered::new(),
            generation: 0,
            status: GatewayLinkStatus::default(),
            status_due: false,
            snapshot_due: false,
        };
        let (socket, peer) = UnixStream::pair().unwrap();
        let (reader, writer) = socket.into_split();
        let mut task = tokio::spawn(async move {
            client
                .serve(Connection {
                    reader: BufReader::new(reader),
                    writer,
                    buffer: Vec::new(),
                })
                .await
        });
        owner
            .outbound
            .send(OwnerMessage::Unregister {
                reason: crate::remote::SessionCloseReason::SharingDisabled,
            })
            .await
            .unwrap();
        drop(owner);
        let (reader, mut writer) = peer.into_split();
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        let frame: OwnerFrame = read_async_frame_with_buffer(&mut reader, &mut buffer)
            .await
            .unwrap();
        assert!(matches!(
            frame,
            OwnerFrame::Unregister {
                reason: crate::remote::SessionCloseReason::SharingDisabled
            }
        ));
        // The gateway can still have an outbound status queued before it
        // processes Unregister. Closing now makes that write fail and ends
        // the registration as owner_exited instead of sharing_disabled.
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut task)
                .await
                .is_err()
        );
        write_async_frame(&mut writer, &GatewayFrame::Pong)
            .await
            .unwrap();
        drop(reader);
        drop(writer);
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            Ended::Owner
        ));
    }

    fn prompt(request_id: &str, text: &str) -> RemoteRequest {
        RemoteRequest {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            session_id: Some("s".to_owned()),
            registration_epoch: Some(1),
            operation: RemoteOperation::SubmitPrompt {
                prompt: text.to_owned(),
            },
        }
    }

    fn key(device: &str, request_id: &str) -> ReceiptKey {
        (device.to_owned(), request_id.to_owned())
    }

    #[test]
    fn receipts_deduplicate_by_device_and_request_id_with_a_payload_digest() {
        let mut receipts = Receipts::new(8);
        let first = Receipts::digest(&prompt("r-1", "run the tests"));
        assert!(matches!(
            receipts.begin(&key("phone", "r-1"), first, (1, 1)),
            Begin::New
        ));
        assert_eq!(receipts.status("phone", "r-1").0, ReceiptState::Received);
        // The same request again while the owner decides: it joins.
        assert!(matches!(
            receipts.begin(&key("phone", "r-1"), first, (1, 2)),
            Begin::Joined
        ));
        // Same ID, other payload.
        let other = Receipts::digest(&prompt("r-1", "delete everything"));
        assert!(matches!(
            receipts.begin(&key("phone", "r-1"), other, (1, 3)),
            Begin::Conflict
        ));
        // Another device may use the same ID.
        assert!(matches!(
            receipts.begin(&key("tablet", "r-1"), other, (1, 4)),
            Begin::New
        ));

        let applied = RemoteResponse::new(
            "r-1",
            RemoteResult::PromptAccepted {
                disposition: crate::remote::PromptDisposition::Started,
            },
        )
        .with_receipt(ReceiptState::Applied);
        assert_eq!(
            receipts.finish(&key("phone", "r-1"), &applied),
            [(1, 1), (1, 2)]
        );
        assert!(matches!(
            receipts.begin(&key("phone", "r-1"), first, (2, 9)),
            Begin::Replay(response) if response == applied
        ));
        assert!(matches!(
            receipts.begin(&key("phone", "r-1"), other, (2, 10)),
            Begin::Conflict
        ));
        let (receipt, result) = receipts.status("phone", "r-1");
        assert_eq!(receipt, ReceiptState::Applied);
        assert_eq!(result, Some(applied.result));
        assert_eq!(
            receipts.status("phone", "never-sent"),
            (ReceiptState::Unknown, None)
        );
    }

    #[test]
    fn a_full_store_refuses_new_mutations_and_keeps_every_receipt() {
        let mut receipts = Receipts::new(2);
        let digest = Receipts::digest(&prompt("r", "p"));
        for id in ["r-1", "r-2"] {
            assert!(matches!(
                receipts.begin(&key("phone", id), digest, (1, 1)),
                Begin::New
            ));
        }
        assert!(matches!(
            receipts.begin(&key("phone", "r-3"), digest, (1, 1)),
            Begin::Full
        ));
        // Known IDs still resolve; nothing was evicted to make room.
        assert!(matches!(
            receipts.begin(&key("phone", "r-1"), digest, (1, 2)),
            Begin::Joined
        ));
        // A request the owner never took leaves no receipt behind.
        receipts.forget(&key("phone", "r-2"));
        assert!(matches!(
            receipts.begin(&key("phone", "r-3"), digest, (1, 1)),
            Begin::New
        ));
    }
}
