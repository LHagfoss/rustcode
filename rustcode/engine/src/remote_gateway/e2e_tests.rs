//! The whole path in one process: a gateway on a loopback port, a session
//! owner that reaches it through the real owner socket and connector, and a
//! WebSocket client that pairs and drives the session.
//!
//! The owner is the real owner-side machinery ([`SessionPublisher`],
//! [`apply_session_mutation`], [`read_session`]) around an `AppState`, run by
//! a small loop that stands in for the terminal's event loop; what a model
//! would do is scripted by the test. Every wait is bounded and every gateway
//! lives in its own temporary configuration directory.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::{WebSocketStream, client_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use super::address::plan_listen;
use super::gateway::{Gateway, GatewayConfig, Limits};
use super::hub::{HubLimits, SessionHub};
use super::lifecycle::RemoteLifecycle;
use super::owner_client::{ClientTiming, GatewayConnector};
use crate::app::{AppState, AppStatus, PendingQuestion};
use crate::controller::AgentUiEvent;
use crate::remote::owner::{
    GatewayLinkStatus, OwnerCommand, OwnerConnector, OwnerMessage, next_registration_epoch,
};
use crate::remote::{
    MAX_REMOTE_FRAME_BYTES, ProjectionLimits, ReceiptState, RemoteResponse, SessionCloseReason,
    SessionPublisher, SessionRegistration, apply_session_mutation, read_session,
};

const WAIT: Duration = Duration::from_secs(15);

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, future)
        .await
        .expect("test step timed out")
}

struct Host {
    dir: tempfile::TempDir,
    lifecycle: RemoteLifecycle,
    running: Option<Running>,
}

struct Running {
    addr: SocketAddr,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Host {
    async fn start() -> Self {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let mut host = Self {
            dir,
            lifecycle,
            running: None,
        };
        host.start_gateway().await;
        host
    }

    async fn start_gateway(&mut self) {
        let hub = SessionHub::start(HubLimits::default());
        let config = || GatewayConfig {
            plan: plan_listen("127.0.0.1", None, &[]).unwrap(),
            port: 0,
            router: hub.clone(),
            sessions: Some(hub.clone()),
            limits: Limits::default(),
        };
        // The previous gateway's lock is released a moment after it stops.
        let gateway = bounded(async {
            loop {
                match Gateway::bind(&self.lifecycle, config()).await {
                    Ok(gateway) => break gateway,
                    Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        })
        .await;
        self.running = Some(Running {
            addr: gateway.local_addr(),
            shutdown: gateway.shutdown_token(),
            task: tokio::spawn(gateway.run()),
        });
    }

    async fn stop_gateway(&mut self) {
        let running = self.running.take().expect("a gateway is running");
        running.shutdown.cancel();
        bounded(running.task).await.unwrap().unwrap();
    }

    fn addr(&self) -> SocketAddr {
        self.running.as_ref().expect("a gateway is running").addr
    }

    fn connector(&self) -> GatewayConnector {
        GatewayConnector::discover(self.lifecycle.clone()).with_timing(ClientTiming {
            heartbeat: Duration::from_millis(200),
            ..ClientTiming::default()
        })
    }
}

/// The terminal's side: the state of one session and the loop that shares it.
struct Terminal {
    registration: SessionRegistration,
    state: Arc<Mutex<AppState>>,
    /// Agent events the "model" produced, applied to `state` under its lock.
    observed: std::sync::mpsc::Sender<AgentUiEvent>,
    control: tokio::sync::mpsc::UnboundedSender<SessionCloseReason>,
    gateway: tokio::sync::watch::Receiver<Option<GatewayLinkStatus>>,
    closed: tokio::sync::watch::Receiver<Option<String>>,
    task: tokio::task::JoinHandle<()>,
}

impl Terminal {
    /// What `/remote` does: open a link for the session on screen and
    /// register it.
    async fn share(connector: &GatewayConnector, session_id: &str) -> Self {
        let mut state = AppState::new();
        state.active_session_id = session_id.to_owned();
        state.input_buffer = "half-typed terminal draft".to_owned();
        let state = Arc::new(Mutex::new(state));
        let registration = SessionRegistration {
            session_id: session_id.to_owned(),
            registration_epoch: next_registration_epoch(),
        };
        let mut link = connector
            .clone()
            .connect(&registration)
            .expect("a link is handed back at once");
        let (mut publisher, register) = {
            let state = state.lock().await;
            SessionPublisher::register(&state, registration.clone(), false)
        };
        link.outbound
            .try_send(register)
            .expect("register is queued");

        let (observed, events) = std::sync::mpsc::channel();
        let (control, mut off) = tokio::sync::mpsc::unbounded_channel();
        let (gateway_tx, gateway) = tokio::sync::watch::channel(None);
        let (closed_tx, closed) = tokio::sync::watch::channel(None);
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut cancel = CancellationToken::new();
            let limits = ProjectionLimits::default();
            loop {
                if let Ok(reason) = off.try_recv() {
                    let _ = link.outbound.try_send(OwnerMessage::Unregister { reason });
                    return;
                }
                loop {
                    match link.commands.try_recv() {
                        Ok(OwnerCommand::Request { request, reply }) => {
                            let registration = publisher.registration().clone();
                            let response = if request.operation.is_mutation() {
                                match apply_session_mutation(
                                    &shared,
                                    &mut cancel,
                                    &registration,
                                    &request,
                                )
                                .await
                                {
                                    Ok(mutation) => {
                                        RemoteResponse::new(request.request_id, mutation.result)
                                            .with_receipt(ReceiptState::Applied)
                                    }
                                    Err(error) => RemoteResponse::error(request.request_id, error)
                                        .with_receipt(ReceiptState::Rejected),
                                }
                            } else {
                                let state = shared.lock().await;
                                match read_session(&state, &registration, &request, &limits) {
                                    Ok(result) => RemoteResponse::new(request.request_id, result),
                                    Err(error) => RemoteResponse::error(request.request_id, error),
                                }
                            };
                            reply.send(response);
                        }
                        Ok(OwnerCommand::SnapshotRequested) => publisher.request_snapshot(),
                        Ok(OwnerCommand::Status(status)) => {
                            let _ = gateway_tx.send(Some(status));
                        }
                        Ok(OwnerCommand::Closed { reason }) => {
                            let _ = closed_tx.send(Some(reason));
                            return;
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            let _ = closed_tx.send(Some("the link closed".to_owned()));
                            return;
                        }
                    }
                }
                let messages = {
                    let state = shared.lock().await;
                    while let Ok(event) = events.try_recv() {
                        publisher.observe(&event);
                    }
                    let room = link.outbound.capacity() * 2 >= link.outbound.max_capacity();
                    publisher.publish(&state, room)
                };
                for message in messages {
                    if link.outbound.try_send(message).is_err() {
                        publisher.mark_lagging();
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        Self {
            registration,
            state,
            observed,
            control,
            gateway,
            closed,
            task,
        }
    }

    /// The gateway accepted the registration.
    async fn registered(&mut self) {
        bounded(
            self.gateway
                .wait_for(|status| status.as_ref().is_some_and(|status| status.connected)),
        )
        .await
        .expect("the terminal is running");
    }

    async fn devices(&mut self, attached: &[&str]) -> GatewayLinkStatus {
        bounded(self.gateway.wait_for(|status| {
            status
                .as_ref()
                .is_some_and(|status| status.connected && status.attached_devices == attached)
        }))
        .await
        .expect("the terminal is running")
        .clone()
        .expect("checked above")
    }

    /// Start the turn for the prompt at the head of the queue, the way the
    /// orchestrator would, and return its turn ID.
    async fn start_turn(&self) -> String {
        let mut state = self.state.lock().await;
        let prompt = state.pending_queue.remove(0);
        let turn_id = state.begin_turn_identity();
        state.status = AppStatus::Streaming;
        self.observed
            .send(AgentUiEvent::PromptStarted {
                prompt,
                timing: None,
            })
            .unwrap();
        turn_id
    }

    async fn stream(&self, text: &str) {
        self.state.lock().await.append_current_response(text);
    }

    async fn ask(&self, text: &str, options: &[&str]) -> tokio::sync::oneshot::Receiver<String> {
        let mut state = self.state.lock().await;
        let question = PendingQuestion::new(
            text.to_owned(),
            options.iter().map(|option| (*option).to_owned()).collect(),
            false,
        );
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(question);
        let (answer, answered) = tokio::sync::oneshot::channel();
        state.question_response = Some(answer);
        answered
    }

    async fn finish_turn(&self, turn_id: &str, content: &str) {
        let mut state = self.state.lock().await;
        state.end_turn_identity(turn_id);
        state.clear_current_response();
        state.enter_idle();
        self.observed
            .send(AgentUiEvent::TurnFinished {
                content: content.to_owned(),
                completed: true,
                timing: None,
            })
            .unwrap();
    }

    /// `/remote off`.
    async fn off(self) {
        self.control
            .send(SessionCloseReason::SharingDisabled)
            .unwrap();
        bounded(self.task).await.unwrap();
    }

    /// The terminal process dies.
    async fn exit(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

struct Phone {
    socket: WebSocketStream<TcpStream>,
    requests: u64,
    /// Frames read while waiting for a response.
    backlog: Vec<Value>,
}

impl Phone {
    async fn connect(addr: SocketAddr, hello: Value) -> (Self, Value) {
        let stream = bounded(TcpStream::connect(addr)).await.unwrap();
        let (socket, _) = bounded(client_async(format!("ws://{addr}/"), stream))
            .await
            .expect("websocket upgrade");
        let mut phone = Self {
            socket,
            requests: 0,
            backlog: Vec::new(),
        };
        phone.send(hello).await;
        let welcome = phone.read().await.expect("a handshake answer");
        (phone, welcome)
    }

    async fn pair(host: &Host, name: &str) -> (Self, Value) {
        let offer = bounded(host.lifecycle.pair()).await.unwrap();
        let (phone, paired) = Self::connect(
            host.addr(),
            json!({
                "type": "pair",
                "protocol_version": 1,
                "method": "code",
                "secret": offer.code.expose(),
                "device_name": name,
            }),
        )
        .await;
        assert_eq!(paired["type"], "paired", "{paired}");
        (phone, paired)
    }

    async fn authenticate(host: &Host, paired: &Value) -> (Self, Value) {
        let (phone, welcome) = Self::connect(
            host.addr(),
            json!({
                "type": "authenticate",
                "protocol_version": 1,
                "device_id": paired["device_id"],
                "token": paired["token"],
            }),
        )
        .await;
        assert_eq!(welcome["type"], "authenticated", "{welcome}");
        (phone, welcome)
    }

    async fn send(&mut self, value: Value) {
        bounded(self.socket.send(Message::text(value.to_string())))
            .await
            .expect("send frame");
    }

    /// The next text frame, or `None` when the connection ended.
    async fn read(&mut self) -> Option<Value> {
        loop {
            match bounded(self.socket.next()).await {
                Some(Ok(Message::Text(text))) => {
                    assert!(text.len() <= MAX_REMOTE_FRAME_BYTES, "oversized frame");
                    return Some(serde_json::from_str(text.as_str()).expect("JSON frame"));
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return None,
            }
        }
    }

    /// Send a request and return its response; everything else that arrives
    /// first is kept for [`Phone::event`].
    async fn request(&mut self, session: Option<&SessionRegistration>, operation: Value) -> Value {
        self.requests += 1;
        let request_id = format!("req-{}", self.requests);
        self.request_as(&request_id, session, operation).await
    }

    async fn request_as(
        &mut self,
        request_id: &str,
        session: Option<&SessionRegistration>,
        operation: Value,
    ) -> Value {
        let mut frame = json!({
            "protocol_version": 1,
            "request_id": request_id,
            "operation": operation,
        });
        if let Some(session) = session {
            frame["session_id"] = json!(session.session_id);
            frame["registration_epoch"] = json!(session.registration_epoch);
        }
        self.send(frame).await;
        loop {
            let frame = self.read().await.expect("the connection stays open");
            if frame["kind"] == "response" && frame["request_id"] == request_id {
                return frame;
            }
            self.backlog.push(frame);
        }
    }

    /// The next frame for which `wanted` holds, skipping the others.
    async fn frame(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.backlog.iter().position(&wanted) {
            return self.backlog.remove(index);
        }
        loop {
            let frame = self.read().await.expect("the connection stays open");
            if wanted(&frame) {
                return frame;
            }
        }
    }

    async fn event(&mut self, kind: &str) -> Value {
        self.frame(|frame| frame["kind"] == "event" && frame["event"]["type"] == kind)
            .await
    }
}

/// What a client keeps per attached session: the text of the running turn
/// and the cursor, with the contract's ordering rule enforced.
#[derive(Default)]
struct View {
    sequence: u64,
    text: String,
}

impl View {
    fn snapshot(&mut self, snapshot: &Value) {
        self.sequence = snapshot["sequence"].as_u64().unwrap();
        self.text = snapshot["turn"]["live_response"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
    }

    fn apply(&mut self, frame: &Value) {
        let sequence = frame["sequence"].as_u64().unwrap();
        match frame["event"]["type"].as_str().unwrap() {
            "snapshot" => self.snapshot(&frame["event"]["snapshot"]),
            "resync_required" | "session_closed" => {}
            kind => {
                assert_eq!(sequence, self.sequence + 1, "gap before {frame}");
                self.sequence = sequence;
                match kind {
                    "turn_started" => self.text.clear(),
                    "text_delta" => self.text.push_str(frame["event"]["text"].as_str().unwrap()),
                    _ => {}
                }
            }
        }
    }

    /// Apply frames until one of type `kind` was applied; returns it.
    async fn until(&mut self, phone: &mut Phone, kind: &str) -> Value {
        loop {
            let frame = phone.frame(|frame| frame["kind"] == "event").await;
            self.apply(&frame);
            if frame["event"]["type"] == kind {
                return frame;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_resumes_the_current_snapshot_and_exposes_machine_readable_history() {
    let host = Host::start().await;
    let connector = host.connector();
    let mut terminal = Terminal::share(&connector, "session-client-followups").await;
    terminal.registered().await;
    let (mut phone, paired) = Phone::pair(&host, "Cursor phone").await;
    let attached = phone
        .request(
            Some(&terminal.registration),
            json!({"type": "attach_session"}),
        )
        .await;
    let snapshot = &attached["result"]["snapshot"];
    assert_eq!(snapshot["session"]["title"], "");
    assert!(snapshot["snapshot_id"].is_string());
    let cursor = json!({"gateway_id": paired["gateway_id"], "instance_id": paired["instance_id"],
        "last_sequence": snapshot["sequence"], "snapshot_id": snapshot["snapshot_id"]});
    drop(phone);
    let (mut phone, _) = Phone::authenticate(&host, &paired).await;
    let resumed = phone
        .request(
            Some(&terminal.registration),
            json!({"type": "attach_session", "resume": cursor}),
        )
        .await;
    assert_eq!(resumed["result"]["type"], "resumed", "{resumed}");
    terminal
        .state
        .lock()
        .await
        .history
        .push(crate::app::ChatMessage::new("user", "Fresh timestamp"));
    let history = phone
        .request(
            Some(&terminal.registration),
            json!({"type": "get_history", "limit": 10}),
        )
        .await;
    let timestamp = history["result"]["messages"][0]["timestamp"]
        .as_str()
        .unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(timestamp).is_ok(),
        "{timestamp}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_remote_off_preserves_the_reason_with_gateway_status_in_flight() {
    let host = Host::start().await;
    let connector = host.connector();
    let (mut phone, _) = Phone::pair(&host, "Teardown phone").await;
    for cycle in 0..20 {
        let mut terminal = Terminal::share(&connector, "session-off-race").await;
        terminal.registered().await;
        let attached = phone
            .request(
                Some(&terminal.registration),
                json!({"type": "attach_session"}),
            )
            .await;
        assert_eq!(attached["result"]["type"], "attached");
        // Attaching queues a gateway status to the owner. Disable sharing
        // immediately, while both directions of the owner socket are busy.
        terminal.off().await;
        let closed = phone.event("session_closed").await;
        assert_eq!(
            closed["event"]["reason"], "sharing_disabled",
            "cycle {cycle}: {closed}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paired_device_drives_a_shared_session_end_to_end() {
    let host = Host::start().await;
    let connector = host.connector();

    // Nothing is shared yet: a device learns nothing before it authenticates,
    // and an authenticated one sees an empty list.
    let (mut stranger, refused) = Phone::connect(
        host.addr(),
        json!({"protocol_version": 1, "request_id": "r", "operation": {"type": "list_sessions"}}),
    )
    .await;
    assert_eq!(refused["type"], "error");
    assert_eq!(refused["code"], "invalid_frame");
    assert!(stranger.read().await.is_none(), "the socket is closed");

    let mut terminal = Terminal::share(&connector, "session-shared").await;
    terminal.registered().await;
    let session = terminal.registration.clone();
    // A second terminal runs a session it never shared.
    let private = Arc::new(Mutex::new(AppState::new()));
    private.lock().await.active_session_id = "session-private".to_owned();

    let (mut stranger, refused) = Phone::connect(
        host.addr(),
        json!({"type": "authenticate", "protocol_version": 1, "device_id": "0011223344556677", "token": "guess"}),
    )
    .await;
    assert_eq!(refused["code"], "unauthorized");
    assert!(!refused.to_string().contains("session-shared"));
    assert!(stranger.read().await.is_none());

    // Pair, then come back with the device token.
    let (mut pairing, paired) = Phone::pair(&host, "Test iPhone").await;
    assert!(paired["host_name"].is_string(), "{paired}");
    assert_eq!(paired["device_name"], "Test iPhone");
    let listed = pairing
        .request(None, json!({"type": "list_sessions"}))
        .await;
    assert_eq!(listed["result"]["sessions"].as_array().unwrap().len(), 1);
    drop(pairing);
    let (mut phone, welcome) = Phone::authenticate(&host, &paired).await;
    assert_eq!(welcome["gateway_id"], paired["gateway_id"]);
    assert_eq!(welcome["instance_id"], paired["instance_id"]);

    let listed = phone
        .request(None, json!({"type": "subscribe_sessions"}))
        .await;
    assert_eq!(listed["result"]["subscribed"], true);
    assert_eq!(listed["result"]["gateway_id"], paired["gateway_id"]);
    assert_eq!(listed["result"]["instance_id"], paired["instance_id"]);
    let sessions = listed["result"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "only the shared session is listed");
    assert_eq!(sessions[0]["session_id"], "session-shared");
    assert_eq!(
        sessions[0]["registration_epoch"],
        session.registration_epoch
    );
    assert_eq!(sessions[0]["activity"], "idle");

    let attached = phone
        .request(Some(&session), json!({"type": "attach_session"}))
        .await;
    assert_eq!(attached["result"]["type"], "attached", "{attached}");
    let mut view = View::default();
    view.snapshot(&attached["result"]["snapshot"]);
    let status = terminal.devices(&["Test iPhone"]).await;
    assert_eq!(status.connected_devices, ["Test iPhone"]);
    assert!(status.loopback_only);
    assert_eq!(status.advertised_address, host.addr().to_string());

    // A remote prompt is never a command: nothing is queued, the approval
    // state is untouched, and the answer says so.
    for prompt in ["/exit", "/yolo on", " /remote off", "/new"] {
        let refused = phone
            .request(
                Some(&session),
                json!({"type": "submit_prompt", "prompt": prompt}),
            )
            .await;
        assert_eq!(refused["result"]["code"], "unsupported_operation");
        assert_eq!(refused["receipt"], "rejected");
    }
    {
        let state = terminal.state.lock().await;
        assert!(state.pending_queue.is_empty());
        assert_eq!(state.active_session_id, "session-shared");
        assert_eq!(state.status, AppStatus::Idle);
    }
    // Nor can a frame name an operation the plan does not list.
    for operation in [
        json!({"type": "run_slash_command", "command": "/exit"}),
        json!({"type": "set_approval_mode", "mode": "auto"}),
    ] {
        let refused = phone.request(Some(&session), operation).await;
        assert_eq!(refused["result"]["code"], "unsupported_operation");
    }

    // Submit a prompt. The same request sent twice is applied once and
    // answered twice with the same outcome; another payload under that ID
    // is refused.
    let submit = json!({"type": "submit_prompt", "prompt": "run the tests"});
    let accepted = phone
        .request_as("submit-1", Some(&session), submit.clone())
        .await;
    assert_eq!(accepted["receipt"], "applied", "{accepted}");
    assert_eq!(accepted["result"]["disposition"], "started");
    let repeated = phone.request_as("submit-1", Some(&session), submit).await;
    assert_eq!(repeated, accepted);
    let conflict = phone
        .request_as(
            "submit-1",
            Some(&session),
            json!({"type": "submit_prompt", "prompt": "delete the tests"}),
        )
        .await;
    assert_eq!(conflict["result"]["code"], "request_conflict");
    {
        let state = terminal.state.lock().await;
        assert_eq!(state.pending_queue, ["run the tests"]);
        // The terminal's own draft is where the user left it.
        assert_eq!(state.input_buffer, "half-typed terminal draft");
    }

    // The turn runs in the terminal; the phone follows it live.
    let turn_id = terminal.start_turn().await;
    terminal.stream("Running ").await;
    let started = view.until(&mut phone, "turn_started").await;
    assert_eq!(started["event"]["turn_id"], turn_id.as_str());
    terminal.stream("cargo test. ").await;

    // The model asks; the phone answers the question by its identity.
    let answered = terminal.ask("Include ignored tests?", &["Yes", "No"]).await;
    let question = view.until(&mut phone, "question_requested").await;
    let question_id = question["event"]["question"]["question_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let answer = phone
        .request(
            Some(&session),
            json!({"type": "answer_question", "question_id": question_id, "answer": {"type": "selected", "options": ["No"]}}),
        )
        .await;
    assert_eq!(answer["receipt"], "applied", "{answer}");
    assert_eq!(bounded(answered).await.unwrap(), "User selected: No");
    // A second answer to the same question is late.
    let late = phone
        .request(
            Some(&session),
            json!({"type": "answer_question", "question_id": question_id, "answer": {"type": "custom", "text": "maybe"}}),
        )
        .await;
    assert_eq!(late["result"]["code"], "stale_question");
    assert_eq!(late["receipt"], "rejected");
    view.until(&mut phone, "question_resolved").await;
    assert_eq!(view.text, "Running cargo test. ");

    // The connection drops mid-turn. Work continues in the terminal.
    let cursor = view.sequence;
    drop(phone);
    terminal.devices(&[]).await;
    terminal.state.lock().await.status = AppStatus::Streaming;
    terminal.stream("All 12 passed.").await;
    // Reconnect mid-turn and resume by cursor: only what was missed is
    // replayed. Finishing the turn changes settings availability and may
    // publish a replacement snapshot, which intentionally resets replay.
    let (mut phone, welcome) = Phone::authenticate(&host, &paired).await;
    let resumed = phone
        .request(
            Some(&session),
            json!({"type": "attach_session", "resume": {
                "gateway_id": welcome["gateway_id"],
                "instance_id": welcome["instance_id"],
                "last_sequence": cursor,
            }}),
        )
        .await;
    assert_eq!(resumed["result"]["type"], "resumed", "{resumed}");
    assert_eq!(resumed["result"]["next_sequence"], cursor + 1);
    terminal
        .finish_turn(&turn_id, "Running cargo test. All 12 passed.")
        .await;
    let finished = view.until(&mut phone, "turn_finished").await;
    assert_eq!(finished["event"]["turn_id"], turn_id.as_str());
    assert_eq!(view.text, "Running cargo test. All 12 passed.");

    // The response to the first submit could have been lost with the old
    // connection: asking returns the original outcome and applies nothing.
    let status = phone
        .request(
            None,
            json!({"type": "get_request_status", "target_request_id": "submit-1"}),
        )
        .await;
    assert_eq!(status["result"]["receipt"], "applied", "{status}");
    assert_eq!(status["result"]["result"], accepted["result"]);
    assert!(terminal.state.lock().await.pending_queue.is_empty());

    // History and content come from the owner, bounded.
    let history = phone
        .request(Some(&session), json!({"type": "get_history", "limit": 20}))
        .await;
    assert_eq!(history["result"]["type"], "history", "{history}");

    // `/remote off`: the session disappears for the device at once.
    let listed = phone
        .request(None, json!({"type": "subscribe_sessions"}))
        .await;
    assert_eq!(listed["result"]["sessions"][0]["activity"], "idle");
    terminal.off().await;
    let closed = phone.event("session_closed").await;
    assert_eq!(closed["event"]["reason"], "sharing_disabled");
    let emptied = phone
        .frame(|frame| frame["kind"] == "sessions" && frame["sessions"] == json!([]))
        .await;
    assert_eq!(emptied["gateway_id"], paired["gateway_id"]);
    let gone = phone
        .request(
            Some(&session),
            json!({"type": "submit_prompt", "prompt": "anyone there?"}),
        )
        .await;
    assert_eq!(gone["result"]["code"], "not_found");

    // Revoking the device closes its connection and its token.
    let device_id = paired["device_id"].as_str().unwrap();
    bounded(host.lifecycle.revoke(device_id)).await.unwrap();
    let revoked = phone.frame(|frame| frame["type"] == "error").await;
    assert_eq!(revoked["code"], "revoked");
    assert!(phone.read().await.is_none());
    let (_, refused) = Phone::connect(
        host.addr(),
        json!({"type": "authenticate", "protocol_version": 1, "device_id": device_id, "token": paired["token"]}),
    )
    .await;
    assert_eq!(refused["code"], "unauthorized");
    drop(host.dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_restart_keeps_the_registration_and_its_receipts() {
    let mut host = Host::start().await;
    let connector = host.connector();
    let mut terminal = Terminal::share(&connector, "session-restart").await;
    terminal.registered().await;
    let session = terminal.registration.clone();

    let (mut phone, paired) = Phone::pair(&host, "phone").await;
    let attached = phone
        .request(Some(&session), json!({"type": "attach_session"}))
        .await;
    let mut view = View::default();
    view.snapshot(&attached["result"]["snapshot"]);
    let accepted = phone
        .request_as(
            "before-restart",
            Some(&session),
            json!({"type": "submit_prompt", "prompt": "long job"}),
        )
        .await;
    assert_eq!(accepted["receipt"], "applied");
    let turn_id = terminal.start_turn().await;
    terminal.stream("working ").await;
    view.until(&mut phone, "text_delta").await;
    let cursor = json!({
        "gateway_id": paired["gateway_id"],
        "instance_id": paired["instance_id"],
        "last_sequence": view.sequence,
    });

    // The gateway goes away. The device is told why its connection closes;
    // the terminal keeps running its turn and its registration.
    host.stop_gateway().await;
    let goodbye = phone.frame(|frame| frame["type"] == "error").await;
    assert_eq!(goodbye["code"], "shutting_down");
    bounded(
        terminal
            .gateway
            .wait_for(|status| status.as_ref().is_some_and(|status| !status.connected)),
    )
    .await
    .unwrap();
    terminal.stream("still working ").await;

    // A new gateway instance on the same configuration directory: the
    // terminal finds it and registers again under the same epoch.
    host.start_gateway().await;
    terminal.registered().await;
    let (mut phone, welcome) = Phone::authenticate(&host, &paired).await;
    assert_eq!(welcome["gateway_id"], paired["gateway_id"]);
    assert_ne!(welcome["instance_id"], paired["instance_id"]);
    let listed = phone.request(None, json!({"type": "list_sessions"})).await;
    assert_eq!(
        listed["result"]["sessions"][0]["registration_epoch"],
        session.registration_epoch
    );

    // The old cursor belongs to the old instance: the gateway says so and
    // hands out current state instead of replaying.
    let attached = phone
        .request(
            Some(&session),
            json!({"type": "attach_session", "resume": cursor}),
        )
        .await;
    assert_eq!(attached["result"]["type"], "attached", "{attached}");
    assert_eq!(attached["result"]["resync"], "gateway_restarted");
    view.snapshot(&attached["result"]["snapshot"]);
    terminal.stream("done").await;
    terminal
        .finish_turn(&turn_id, "working still working done")
        .await;
    view.until(&mut phone, "turn_finished").await;
    assert_eq!(view.text, "working still working done");

    // The receipt survived in the terminal: the original outcome comes
    // back, and repeating the request does not run the prompt again.
    let status = phone
        .request(
            Some(&session),
            json!({"type": "get_request_status", "target_request_id": "before-restart"}),
        )
        .await;
    assert_eq!(status["result"]["receipt"], "applied", "{status}");
    let repeated = phone
        .request_as(
            "before-restart",
            Some(&session),
            json!({"type": "submit_prompt", "prompt": "long job"}),
        )
        .await;
    assert_eq!(repeated["result"], accepted["result"]);
    assert_eq!(repeated["receipt"], "applied");
    assert!(terminal.state.lock().await.pending_queue.is_empty());

    // The terminal exits. Its session is gone at once and what it did with
    // a request nobody answered stays unknown; nothing resends it.
    terminal.exit().await;
    let closed = phone.event("session_closed").await;
    assert_eq!(closed["event"]["reason"], "owner_exited");
    for session in [Some(&session), None] {
        let status = phone
            .request(
                session,
                json!({"type": "get_request_status", "target_request_id": "before-restart"}),
            )
            .await;
        assert_eq!(status["result"]["receipt"], "unknown", "{status}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_terminals_share_and_a_third_stays_private() {
    let host = Host::start().await;
    let connector = host.connector();
    let mut first = Terminal::share(&connector, "session-one").await;
    let mut second = Terminal::share(&connector, "session-two").await;
    first.registered().await;
    second.registered().await;

    // A terminal that shares a session someone else already shares is told
    // so and shares nothing.
    let mut duplicate = Terminal::share(&connector, "session-one").await;
    let reason = bounded(duplicate.closed.wait_for(|reason| reason.is_some()))
        .await
        .unwrap()
        .clone()
        .unwrap();
    assert!(reason.contains("already sharing"), "{reason}");

    let (mut phone, _) = Phone::pair(&host, "phone").await;
    let listed = phone.request(None, json!({"type": "list_sessions"})).await;
    let ids: Vec<&str> = listed["result"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["session-one", "session-two"]);

    // Each prompt lands in the terminal that owns its session.
    for (terminal, text) in [(&second, "for two"), (&first, "for one")] {
        let accepted = phone
            .request(
                Some(&terminal.registration),
                json!({"type": "submit_prompt", "prompt": text}),
            )
            .await;
        assert_eq!(accepted["receipt"], "applied", "{accepted}");
    }
    assert_eq!(first.state.lock().await.pending_queue, ["for one"]);
    assert_eq!(second.state.lock().await.pending_queue, ["for two"]);

    // The terminal moves to another session: the old registration ends and
    // the new identity is not shared by anything it did before.
    let old = second.registration.clone();
    second
        .control
        .send(SessionCloseReason::SessionChanged)
        .unwrap();
    bounded(second.task).await.unwrap();
    let listed = phone.request(None, json!({"type": "list_sessions"})).await;
    assert_eq!(listed["result"]["sessions"].as_array().unwrap().len(), 1);
    let refused = phone
        .request(Some(&old), json!({"type": "attach_session"}))
        .await;
    assert_eq!(refused["result"]["code"], "not_found");
    first.off().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_output_stays_within_frame_limits_and_rebuilds_completely() {
    let host = Host::start().await;
    let connector = host.connector();
    let mut terminal = Terminal::share(&connector, "session-large").await;
    terminal.registered().await;
    let session = terminal.registration.clone();
    let (mut phone, _) = Phone::pair(&host, "phone").await;
    // A second device attaches and reads nothing for the whole run.
    let (mut idle, _) = Phone::pair(&host, "idle").await;
    idle.request(Some(&session), json!({"type": "attach_session"}))
        .await;

    // A long transcript lands in the session; the attached device is handed
    // the new state as a snapshot.
    phone
        .request(Some(&session), json!({"type": "attach_session"}))
        .await;
    {
        let mut state = terminal.state.lock().await;
        for index in 0..400 {
            state.history.push(crate::app::ChatMessage::new(
                if index % 2 == 0 { "user" } else { "assistant" },
                format!("message {index}: {}", "lorem ipsum ".repeat(500)),
            ));
        }
    }
    let event = phone
        .frame(|frame| {
            frame["event"]["type"] == "snapshot"
                && frame["event"]["snapshot"]["history_cursor"].is_string()
        })
        .await;
    let snapshot = &event["event"]["snapshot"];
    let mut view = View::default();
    view.snapshot(snapshot);

    // Page back through all of it; every page is one bounded frame
    // (`Phone::read` checks each) and the pages rebuild the transcript.
    let mut messages: Vec<Value> = snapshot["transcript"].as_array().unwrap().clone();
    let mut cursor = snapshot["history_cursor"].clone();
    let mut pages = 0;
    while cursor.is_string() {
        let page = phone
            .request(
                Some(&session),
                json!({"type": "get_history", "cursor": cursor, "limit": 50}),
            )
            .await;
        assert_eq!(page["result"]["type"], "history", "{page}");
        let mut older = page["result"]["messages"].as_array().unwrap().clone();
        older.extend(messages);
        messages = older;
        cursor = page["result"]["next_cursor"].clone();
        pages += 1;
        assert!(pages < 400, "paging does not terminate");
    }
    assert!(pages > 1, "the transcript did not fit one frame");
    assert_eq!(messages.len(), 400);
    // A message cut to fit its frame is fetched in chunks, completely.
    let first = &messages[0];
    assert_eq!(first["content"]["truncated"], true, "{first}");
    let mut text = String::new();
    {
        let content_id = first["content"]["content_id"].clone();
        let mut offset = json!(0);
        while offset.is_u64() {
            let chunk = phone
                .request(
                    Some(&session),
                    json!({"type": "get_content", "content_id": content_id, "offset": offset, "max_bytes": 4096}),
                )
                .await;
            assert_eq!(chunk["result"]["type"], "content", "{chunk}");
            text.push_str(chunk["result"]["text"].as_str().unwrap());
            offset = chunk["result"]["next_offset"].clone();
        }
    }
    assert_eq!(text, format!("message 0: {}", "lorem ipsum ".repeat(500)));

    // Then a turn that streams far more than a frame holds.
    let accepted = phone
        .request(
            Some(&session),
            json!({"type": "submit_prompt", "prompt": "print a lot"}),
        )
        .await;
    assert_eq!(accepted["receipt"], "applied");
    let turn_id = terminal.start_turn().await;
    let chunk = "0123456789abcdef".repeat(256);
    let mut expected = String::new();
    for _ in 0..150 {
        terminal.stream(&chunk).await;
        expected.push_str(&chunk);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(expected.len() > 2 * MAX_REMOTE_FRAME_BYTES);
    terminal.finish_turn(&turn_id, &expected).await;
    view.until(&mut phone, "turn_finished").await;
    // The reading device has the tail in order; it stays responsive although
    // the other one never read a frame.
    assert!(expected.ends_with(&view.text) && !view.text.is_empty());
    let listed = phone.request(None, json!({"type": "list_sessions"})).await;
    assert_eq!(listed["result"]["sessions"].as_array().unwrap().len(), 1);
    terminal.off().await;
}
