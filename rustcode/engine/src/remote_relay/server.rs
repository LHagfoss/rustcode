//! The relay server: member registration, dials and blind pipes.
//!
//! One port serves everything. A connection whose first byte starts a TLS
//! record is a member's control connection; one that starts with `{` is a
//! dial from a device or a pipe a member opens in answer to one; anything
//! else is the HTTP entry of [`http_entry`], for a client that only has
//! WebSocket and `fetch`. Once a dial is matched with its pipe the relay
//! copies bytes both ways and reads none of them.

pub(super) mod http_entry;

use super::protocol::{
    ErrorCode, PROTOCOL_VERSION, Reply, Request, Role, read_frame, signed_message, write_frame,
};
use super::state::{RelayState, random_token};
use crate::remote_gateway::device_key::{normalize_public_key, verify};
use crate::remote_gateway::tls::TlsIdentity;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// The bounds that keep one member or client from exhausting the relay.
#[derive(Debug, Clone)]
pub struct Limits {
    /// From accept to a registered member or a spliced pipe.
    pub handshake_timeout: Duration,
    /// How long a dial waits for the member to open its pipe.
    pub pipe_wait: Duration,
    /// A spliced pipe that carries no byte for this long is closed.
    pub idle_timeout: Duration,
    pub ping_interval: Duration,
    /// A member heard from neither frame nor pong for this long is dropped.
    pub member_silence: Duration,
    pub max_pipes_per_machine: usize,
    pub max_unauthenticated: usize,
    pub max_allowed_keys: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(10),
            pipe_wait: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(15 * 60),
            ping_interval: Duration::from_secs(25),
            member_silence: Duration::from_secs(80),
            max_pipes_per_machine: 32,
            max_unauthenticated: 256,
            max_allowed_keys: 256,
        }
    }
}

/// The first byte of a TLS handshake record.
const TLS_HANDSHAKE_RECORD: u8 = 0x16;

/// How long one frame may take to leave.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// One side of a plain connection, with any bytes read past its last frame
/// still buffered.
type Halves = (BufReader<OwnedReadHalf>, OwnedWriteHalf);

pub struct Relay {
    state: Mutex<RelayState>,
    state_path: Option<PathBuf>,
    tls: TlsIdentity,
    limits: Limits,
    members: Mutex<HashMap<String, Member>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Halves>>>,
    challenges: http_entry::Challenges,
    sessions: AtomicU64,
    unauthenticated: Arc<Semaphore>,
    shutdown: CancellationToken,
}

struct Member {
    session: u64,
    open: mpsc::Sender<String>,
    allowed: HashSet<String>,
    pipes: Arc<AtomicUsize>,
}

/// Counts one live pipe against its machine until dropped.
struct PipeSlot(Arc<AtomicUsize>);

impl Drop for PipeSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Relay {
    pub fn new(
        state: RelayState,
        state_path: Option<PathBuf>,
        tls: TlsIdentity,
        limits: Limits,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            state_path,
            tls,
            challenges: http_entry::Challenges::new(limits.max_unauthenticated),
            unauthenticated: Arc::new(Semaphore::new(limits.max_unauthenticated)),
            limits,
            members: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            sessions: AtomicU64::new(0),
            shutdown: CancellationToken::new(),
        })
    }

    pub fn fingerprint(&self) -> &str {
        self.tls.fingerprint()
    }

    pub fn relay_id(&self) -> String {
        self.lock_state().relay_id.clone()
    }

    pub fn secret(&self) -> String {
        self.lock_state().secret.clone()
    }

    /// Cancelled to stop accepting and close every connection.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// How many machines are registered now.
    pub fn online(&self) -> usize {
        self.lock_members().len()
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, RelayState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_members(&self) -> std::sync::MutexGuard<'_, HashMap<String, Member>> {
        self.members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<Halves>>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Accept connections on `listener` until the relay shuts down.
    pub async fn run(self: Arc<Self>, listener: TcpListener) {
        loop {
            let socket = tokio::select! {
                _ = self.shutdown.cancelled() => return,
                accepted = listener.accept() => match accepted {
                    Ok((socket, _)) => socket,
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                },
            };
            let Ok(permit) = Arc::clone(&self.unauthenticated).try_acquire_owned() else {
                continue;
            };
            let relay = Arc::clone(&self);
            tokio::spawn(async move {
                tokio::select! {
                    _ = relay.shutdown.cancelled() => {}
                    _ = Arc::clone(&relay).handle(socket, permit) => {}
                }
            });
        }
    }

    async fn handle(self: Arc<Self>, socket: TcpStream, permit: OwnedSemaphorePermit) {
        let _ = socket.set_nodelay(true);
        let deadline = tokio::time::Instant::now() + self.limits.handshake_timeout;
        let mut first = [0u8; 1];
        let peeked = tokio::time::timeout_at(deadline, socket.peek(&mut first)).await;
        if !matches!(peeked, Ok(Ok(1))) {
            return;
        }
        if first[0] == TLS_HANDSHAKE_RECORD {
            let accepted =
                tokio::time::timeout_at(deadline, self.tls.acceptor().accept(socket)).await;
            if let Ok(Ok(stream)) = accepted {
                let (read, write) = tokio::io::split(stream);
                self.member(BufReader::new(read), write, deadline, permit)
                    .await;
            }
            return;
        }
        let (read, mut write) = socket.into_split();
        let mut reader = BufReader::new(read);
        if first[0] != b'{' {
            self.http_entry((reader, write), deadline, permit).await;
            return;
        }
        let frame = tokio::time::timeout_at(deadline, read_frame::<Request, _>(&mut reader)).await;
        match frame {
            Ok(Ok(Some(Request::Dial {
                version,
                machine_id,
                public_key,
            }))) => {
                self.dial(
                    (reader, write),
                    version,
                    &machine_id,
                    &public_key,
                    deadline,
                    permit,
                )
                .await;
            }
            Ok(Ok(Some(Request::Pipe { pipe }))) => {
                drop(permit);
                let waiting = self.lock_pending().remove(&pipe);
                if let Some(waiting) = waiting {
                    let _ = waiting.send((reader, write));
                }
            }
            Ok(Ok(Some(Request::Register { .. }))) => {
                send(
                    &mut write,
                    &Reply::error(
                        ErrorCode::BadRequest,
                        "a machine registers over TLS pinned to the relay's certificate",
                    ),
                )
                .await;
            }
            Ok(Ok(Some(_))) | Ok(Err(_)) => {
                send(
                    &mut write,
                    &Reply::error(ErrorCode::BadRequest, "expected a dial or a pipe"),
                )
                .await;
            }
            Ok(Ok(None)) | Err(_) => {}
        }
    }

    fn secret_matches(&self, offered: &str) -> bool {
        let expected = Sha256::digest(self.lock_state().secret.as_bytes());
        let offered = Sha256::digest(offered.as_bytes());
        expected
            .iter()
            .zip(offered.iter())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
    }

    /// Issue a challenge and check that `public_key` signed it.
    async fn prove<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
        role: Role,
        machine_id: &str,
        public_key: &str,
        deadline: tokio::time::Instant,
    ) -> bool
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let challenge = random_token(32);
        let challenge_frame = Reply::Challenge {
            challenge: challenge.clone(),
            relay_id: self.relay_id(),
        };
        if !send(writer, &challenge_frame).await {
            return false;
        }
        let message = signed_message(role, &self.relay_id(), &challenge, machine_id);
        match tokio::time::timeout_at(deadline, read_frame::<Request, _>(reader)).await {
            Ok(Ok(Some(Request::Prove { signature })))
                if verify(public_key, &message, &signature) =>
            {
                true
            }
            Ok(Ok(Some(_))) => {
                send(
                    writer,
                    &Reply::error(
                        ErrorCode::Unauthorized,
                        "the signature does not match the key",
                    ),
                )
                .await;
                false
            }
            Err(_) => {
                send(
                    writer,
                    &Reply::error(ErrorCode::Timeout, "no proof arrived in time"),
                )
                .await;
                false
            }
            Ok(_) => false,
        }
    }

    async fn member<R, W>(
        self: Arc<Self>,
        mut reader: BufReader<R>,
        mut writer: W,
        deadline: tokio::time::Instant,
        permit: OwnedSemaphorePermit,
    ) where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin,
    {
        let frame = tokio::time::timeout_at(deadline, read_frame::<Request, _>(&mut reader)).await;
        let Ok(Ok(Some(Request::Register {
            version,
            secret,
            machine_id,
            public_key,
        }))) = frame
        else {
            send(
                &mut writer,
                &Reply::error(ErrorCode::BadRequest, "expected a registration"),
            )
            .await;
            return;
        };
        if version != PROTOCOL_VERSION {
            let message = format!("this relay speaks relay protocol {PROTOCOL_VERSION}");
            send(
                &mut writer,
                &Reply::error(ErrorCode::UnsupportedVersion, message),
            )
            .await;
            return;
        }
        if !self.secret_matches(&secret) {
            send(
                &mut writer,
                &Reply::error(ErrorCode::Unauthorized, "the relay secret is wrong"),
            )
            .await;
            return;
        }
        let Some(public_key) = normalize_public_key(&public_key) else {
            send(
                &mut writer,
                &Reply::error(ErrorCode::BadRequest, "the public key is not one"),
            )
            .await;
            return;
        };
        if machine_id.is_empty() || machine_id.len() > 128 {
            send(
                &mut writer,
                &Reply::error(ErrorCode::BadRequest, "the machine id is not one"),
            )
            .await;
            return;
        }
        if self.key_conflicts(&machine_id, &public_key) {
            send(&mut writer, &key_mismatch()).await;
            return;
        }
        if !self
            .prove(
                &mut reader,
                &mut writer,
                Role::Member,
                &machine_id,
                &public_key,
                deadline,
            )
            .await
        {
            return;
        }
        if !self.remember(&machine_id, &public_key) {
            send(&mut writer, &key_mismatch()).await;
            return;
        }
        let session = self.sessions.fetch_add(1, Ordering::Relaxed);
        let (open, mut opened) = mpsc::channel::<String>(self.limits.max_pipes_per_machine);
        self.lock_members().insert(
            machine_id.clone(),
            Member {
                session,
                open,
                allowed: HashSet::new(),
                pipes: Arc::new(AtomicUsize::new(0)),
            },
        );
        drop(permit);
        let registered = Reply::Registered {
            relay_id: self.relay_id(),
        };
        if send(&mut writer, &registered).await {
            self.serve_member(reader, &mut writer, &machine_id, session, &mut opened)
                .await;
        }
        let mut members = self.lock_members();
        if members
            .get(&machine_id)
            .is_some_and(|member| member.session == session)
        {
            members.remove(&machine_id);
        }
    }

    async fn serve_member<R, W>(
        &self,
        mut reader: BufReader<R>,
        writer: &mut W,
        machine_id: &str,
        session: u64,
        opened: &mut mpsc::Receiver<String>,
    ) where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin,
    {
        let (frames, mut received) = mpsc::channel::<Request>(16);
        let reading = tokio::spawn(async move {
            while let Ok(Some(frame)) = read_frame::<Request, _>(&mut reader).await {
                if frames.send(frame).await.is_err() {
                    break;
                }
            }
        });
        let mut ping = tokio::time::interval(self.limits.ping_interval);
        ping.tick().await;
        let mut last_heard = Instant::now();
        loop {
            tokio::select! {
                pipe = opened.recv() => match pipe {
                    Some(pipe) => {
                        if !send(writer, &Reply::Open { pipe }).await {
                            break;
                        }
                    }
                    None => break,
                },
                frame = received.recv() => {
                    let Some(frame) = frame else { break };
                    last_heard = Instant::now();
                    if let Request::AllowedKeys { keys } = frame {
                        let keys: HashSet<String> = keys
                            .iter()
                            .filter_map(|key| normalize_public_key(key))
                            .take(self.limits.max_allowed_keys)
                            .collect();
                        let mut members = self.lock_members();
                        if let Some(member) = members.get_mut(machine_id)
                            && member.session == session
                        {
                            member.allowed = keys;
                        }
                    }
                }
                _ = ping.tick() => {
                    if last_heard.elapsed() > self.limits.member_silence
                        || !send(writer, &Reply::Ping).await
                    {
                        break;
                    }
                }
            }
        }
        reading.abort();
    }

    /// Whether `machine_id` first registered with another key.
    fn key_conflicts(&self, machine_id: &str, public_key: &str) -> bool {
        self.lock_state()
            .members
            .get(machine_id)
            .is_some_and(|known| known != public_key)
    }

    /// Bind `machine_id` to `public_key` on first registration. False when
    /// it is already bound to another key or the binding could not be saved.
    fn remember(&self, machine_id: &str, public_key: &str) -> bool {
        let mut state = self.lock_state();
        match state.members.get(machine_id) {
            Some(known) => known == public_key,
            None => {
                state
                    .members
                    .insert(machine_id.to_owned(), public_key.to_owned());
                match &self.state_path {
                    Some(path) => state.save(path).is_ok(),
                    None => true,
                }
            }
        }
    }

    async fn dial(
        &self,
        client: Halves,
        version: u32,
        machine_id: &str,
        public_key: &str,
        deadline: tokio::time::Instant,
        permit: OwnedSemaphorePermit,
    ) {
        let (mut reader, mut writer) = client;
        if version != PROTOCOL_VERSION {
            let message = format!("this relay speaks relay protocol {PROTOCOL_VERSION}");
            send(
                &mut writer,
                &Reply::error(ErrorCode::UnsupportedVersion, message),
            )
            .await;
            return;
        }
        let Some(public_key) = normalize_public_key(public_key) else {
            send(
                &mut writer,
                &Reply::error(ErrorCode::BadRequest, "the public key is not one"),
            )
            .await;
            return;
        };
        if !self.lock_members().contains_key(machine_id) {
            send(&mut writer, &offline()).await;
            return;
        }
        if !self
            .prove(
                &mut reader,
                &mut writer,
                Role::Client,
                machine_id,
                &public_key,
                deadline,
            )
            .await
        {
            return;
        }
        let (member, slot) = match self.open_pipe(machine_id, &public_key).await {
            Ok(opened) => opened,
            Err(refusal) => {
                send(&mut writer, &refusal).await;
                return;
            }
        };
        drop(permit);
        if !send(&mut writer, &Reply::Connected).await {
            return;
        }
        splice(
            (reader, writer),
            member,
            self.limits.idle_timeout,
            self.shutdown.clone(),
        )
        .await;
        drop(slot);
    }

    /// Ask the member of `machine_id` for a pipe for the device `public_key`
    /// and wait for it to arrive.
    async fn open_pipe(
        &self,
        machine_id: &str,
        public_key: &str,
    ) -> Result<(Halves, PipeSlot), Reply> {
        let pipe = random_token(32);
        let (deliver, delivered) = oneshot::channel();
        self.lock_pending().insert(pipe.clone(), deliver);
        let slot = match self.request_pipe(machine_id, public_key, &pipe) {
            Ok(slot) => slot,
            Err(refusal) => {
                self.lock_pending().remove(&pipe);
                return Err(refusal);
            }
        };
        match tokio::time::timeout(self.limits.pipe_wait, delivered).await {
            Ok(Ok(member)) => Ok((member, slot)),
            _ => {
                self.lock_pending().remove(&pipe);
                Err(Reply::error(
                    ErrorCode::Timeout,
                    "the machine did not open the pipe in time",
                ))
            }
        }
    }

    /// Ask the member for a pipe named `pipe`, counting it against the
    /// machine's cap.
    fn request_pipe(
        &self,
        machine_id: &str,
        public_key: &str,
        pipe: &str,
    ) -> Result<PipeSlot, Reply> {
        let members = self.lock_members();
        let Some(member) = members.get(machine_id) else {
            return Err(offline());
        };
        if !member.allowed.contains(public_key) {
            return Err(Reply::error(
                ErrorCode::NotAllowed,
                "this device may not reach that machine; pair it with the machine first",
            ));
        }
        if member.pipes.fetch_add(1, Ordering::AcqRel) >= self.limits.max_pipes_per_machine {
            member.pipes.fetch_sub(1, Ordering::AcqRel);
            return Err(busy());
        }
        let slot = PipeSlot(Arc::clone(&member.pipes));
        if member.open.try_send(pipe.to_owned()).is_err() {
            return Err(busy());
        }
        Ok(slot)
    }

    #[cfg(test)]
    pub(super) fn allows(&self, machine_id: &str, public_key: &str) -> bool {
        self.lock_members()
            .get(machine_id)
            .is_some_and(|member| member.allowed.contains(public_key))
    }
}

fn offline() -> Reply {
    Reply::error(
        ErrorCode::MachineOffline,
        "no machine with that id is registered with this relay now",
    )
}

fn busy() -> Reply {
    Reply::error(
        ErrorCode::Busy,
        "the machine has too many pipes open; try again shortly",
    )
}

fn key_mismatch() -> Reply {
    Reply::error(
        ErrorCode::KeyMismatch,
        "that machine id registered with another key",
    )
}

/// Send `reply`, giving up on a peer that does not read it in time.
async fn send<W: AsyncWrite + Unpin>(writer: &mut W, reply: &Reply) -> bool {
    matches!(
        tokio::time::timeout(WRITE_TIMEOUT, write_frame(writer, reply)).await,
        Ok(Ok(()))
    )
}

/// Copy bytes both ways until both sides close, the pipe stays quiet for
/// `idle`, or the relay shuts down.
async fn splice(client: Halves, member: Halves, idle: Duration, shutdown: CancellationToken) {
    let started = Instant::now();
    let last = AtomicU64::new(0);
    let (client_read, client_write) = client;
    let (member_read, member_write) = member;
    let pumps = async {
        tokio::join!(
            pump(client_read, member_write, &last, started),
            pump(member_read, client_write, &last, started),
        )
    };
    let tick = (idle / 4).max(Duration::from_millis(10));
    let watchdog = async {
        loop {
            tokio::time::sleep(tick).await;
            let heard = Duration::from_millis(last.load(Ordering::Relaxed));
            if started.elapsed().saturating_sub(heard) >= idle {
                break;
            }
        }
    };
    tokio::select! {
        _ = pumps => {}
        _ = watchdog => {}
        _ = shutdown.cancelled() => {}
    }
}

async fn pump<R, W>(mut reader: R, mut writer: W, last: &AtomicU64, started: Instant)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if writer.write_all(&buffer[..read]).await.is_err() {
                    break;
                }
                let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                last.store(elapsed, Ordering::Relaxed);
            }
        }
    }
    let _ = writer.shutdown().await;
}
