//! The terminal's side of `/remote` (issue #1908): shares the session this
//! terminal is running over an [`OwnerLink`] and applies the commands that
//! arrive on it.
//!
//! The event loop runs [`RemoteBridge::tick`] once per iteration, after it has
//! applied that iteration's agent events. Everything remote therefore happens
//! on the loop's own task, between two pieces of terminal input: a remote
//! command and a key press are never applied concurrently. The link is only
//! ever used with `try_send`/`try_recv`, and never while the state is locked.

use super::*;
use rustcode::remote::owner::{
    OwnerCommand, OwnerConnector, OwnerLink, OwnerLinkError, OwnerMessage, OwnerReply,
    SharingCommand, default_connector, next_registration_epoch,
};
use rustcode::remote::{
    OwnerFollowUp, ProjectionLimits, ReceiptState, RemoteError, RemoteErrorCode, RemoteRequest,
    RemoteResponse, SessionCloseReason, SessionPublisher, SessionRegistration,
    apply_session_mutation, read_session,
};
use tokio::sync::mpsc::error::{TryRecvError, TrySendError};

/// Remote commands applied in one loop iteration, so a burst from a device
/// cannot hold back the terminal's own input and rendering.
const COMMANDS_PER_TICK: usize = 8;

const PANEL_TITLE: &str = "Remote";

struct Share {
    publisher: SessionPublisher,
    link: OwnerLink,
}

/// What the loop lends the bridge for one iteration.
pub(super) struct RemotePump<'a> {
    pub(super) app_state: &'a Arc<Mutex<AppState>>,
    pub(super) client: &'a reqwest::Client,
    pub(super) cancel_token: &'a mut CancellationToken,
    pub(super) agent_ui_event_sender: &'a AgentUiEventSender,
    /// No terminal input is waiting in the loop's queue. Remote commands wait
    /// for this, so an answer the terminal already produced is applied before
    /// a remote one that arrived after it.
    pub(super) terminal_input_idle: bool,
    pub(super) needs_redraw: &'a mut bool,
}

pub(crate) struct RemoteBridge {
    connector: Box<dyn OwnerConnector>,
    share: Option<Share>,
    /// A turn's start was applied and its end not yet. Tracked while nothing
    /// is shared too, so a registration made mid-turn publishes that end.
    turn_stream_open: bool,
}

impl RemoteBridge {
    pub(super) fn new() -> Self {
        Self::with_connector(default_connector())
    }

    pub(super) fn with_connector(connector: Box<dyn OwnerConnector>) -> Self {
        Self {
            connector,
            share: None,
            turn_stream_open: false,
        }
    }

    #[cfg(test)]
    pub(super) fn registration(&self) -> Option<&SessionRegistration> {
        self.share
            .as_ref()
            .map(|share| share.publisher.registration())
    }

    /// Note an agent event the loop has applied this iteration.
    pub(super) fn observe(&mut self, event: &AgentUiEvent) {
        match event {
            AgentUiEvent::PromptStarted { .. } => self.turn_stream_open = true,
            AgentUiEvent::TurnFinished { .. } | AgentUiEvent::Cancelled { .. } => {
                self.turn_stream_open = false;
            }
            _ => {}
        }
        if let Some(share) = self.share.as_mut() {
            share.publisher.observe(event);
        }
    }

    /// One pass: run a `/remote` command if the user entered one, apply
    /// waiting remote commands, then publish what changed.
    pub(super) async fn tick(&mut self, command: Option<SharingCommand>, pump: RemotePump<'_>) {
        if let Some(command) = command {
            self.run_command(command, pump.app_state).await;
            *pump.needs_redraw = true;
        }
        if self.share.is_some() {
            self.pump(pump).await;
        }
    }

    async fn run_command(&mut self, command: SharingCommand, app_state: &Arc<Mutex<AppState>>) {
        let report = match command {
            SharingCommand::Enable => self.enable(app_state).await,
            SharingCommand::Status => self.status(),
            SharingCommand::Off => {
                if self.close(SessionCloseReason::SharingDisabled) {
                    "Remote sharing is off. This session is private again; a running turn is not affected."
                        .to_owned()
                } else {
                    "This session is not shared.".to_owned()
                }
            }
        };
        app_state
            .lock()
            .await
            .show_command_panel(PANEL_TITLE, report);
    }

    /// Share the session on screen. Repeating it changes nothing.
    async fn enable(&mut self, app_state: &Arc<Mutex<AppState>>) -> String {
        if self.share.is_some() {
            return self.status();
        }
        let registration = SessionRegistration {
            session_id: app_state.lock().await.active_session_id.clone(),
            registration_epoch: next_registration_epoch(),
        };
        // Opening the link is the connector's business; the state is not
        // locked while it runs.
        let link = match self.connector.connect(&registration) {
            Ok(link) => link,
            Err(error @ OwnerLinkError::NoGateway) => {
                return format!(
                    "Remote sharing is not available: {error}.\n\
                     This session stays private. `/remote` is experimental and this build has no gateway to connect to yet."
                );
            }
            Err(error) => {
                return format!(
                    "Remote sharing could not start: {error}.\nThis session stays private."
                );
            }
        };
        let (publisher, register) = {
            let state = app_state.lock().await;
            if state.active_session_id != registration.session_id {
                return "Remote sharing could not start: the session changed. Run /remote again."
                    .to_owned();
            }
            SessionPublisher::register(&state, registration, self.turn_stream_open)
        };
        if link.outbound.try_send(register).is_err() {
            return "Remote sharing could not start: the gateway link closed.\nThis session stays private."
                .to_owned();
        }
        self.share = Some(Share { publisher, link });
        self.status()
    }

    fn status(&self) -> String {
        let Some(share) = self.share.as_ref() else {
            return "This session is private. Run /remote to share it with a paired device."
                .to_owned();
        };
        let registration = share.publisher.registration();
        format!(
            "This session is shared with the remote gateway.\n\
             Session  {}\n\
             Epoch  {}\n\
             Published  {} updates{}\n\
             Run /remote off to stop sharing. Switching to another session stops it too.",
            registration.session_id,
            registration.registration_epoch,
            share.publisher.sequence(),
            if share.publisher.is_lagging() {
                " (the gateway is behind; a resync is pending)"
            } else {
                ""
            },
        )
    }

    /// End the registration, if there is one: reject what is still queued on
    /// it, tell the gateway why, and drop the link.
    pub(super) fn close(&mut self, reason: SessionCloseReason) -> bool {
        let Some(mut share) = self.share.take() else {
            return false;
        };
        while let Ok(command) = share.link.commands.try_recv() {
            if let OwnerCommand::Request { request, reply } = command {
                reject_stale(request, reply);
            }
        }
        // Dropping the link is what ends the registration; the reason is a
        // courtesy that a full queue may lose.
        let _ = share
            .link
            .outbound
            .try_send(OwnerMessage::Unregister { reason });
        true
    }

    async fn pump(&mut self, pump: RemotePump<'_>) {
        let RemotePump {
            app_state,
            client,
            cancel_token,
            agent_ui_event_sender,
            terminal_input_idle,
            needs_redraw,
        } = pump;
        let Some(share) = self.share.as_mut() else {
            return;
        };

        let mut closed = None;
        for _ in 0..COMMANDS_PER_TICK {
            if !terminal_input_idle {
                break;
            }
            match share.link.commands.try_recv() {
                Ok(OwnerCommand::Request { request, reply }) => {
                    let registration = share.publisher.registration();
                    let response = if request.operation.is_mutation() {
                        apply_mutation(
                            registration,
                            &request,
                            app_state,
                            client,
                            cancel_token,
                            agent_ui_event_sender,
                        )
                        .await
                    } else {
                        let state = app_state.lock().await;
                        let limits = ProjectionLimits::default();
                        match read_session(&state, registration, &request, &limits) {
                            Ok(result) => RemoteResponse::new(request.request_id, result),
                            Err(error) => RemoteResponse::error(request.request_id, error),
                        }
                    };
                    reply.send(response);
                    *needs_redraw = true;
                }
                Ok(OwnerCommand::SnapshotRequested) => share.publisher.request_snapshot(),
                Ok(OwnerCommand::Closed { reason }) => {
                    closed = Some(format!("Remote sharing stopped: {reason}"));
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    closed = Some("Remote sharing stopped: the gateway link closed".to_owned());
                    break;
                }
            }
        }

        if closed.is_none() {
            // A lagging link resyncs once half of its queue has drained, so a
            // slow gateway is not handed a snapshot for every freed slot.
            let room_for_resync =
                share.link.outbound.capacity() * 2 >= share.link.outbound.max_capacity();
            let messages = {
                let state = app_state.lock().await;
                share
                    .publisher
                    .owns(&state)
                    .then(|| share.publisher.publish(&state, room_for_resync))
            };
            match messages {
                // The terminal moved to another session. That identity was
                // never shared: nothing of it is published under the old
                // registration, and it stays private until `/remote` runs.
                None => {
                    self.close(SessionCloseReason::SessionChanged);
                    closed = Some(
                        "Remote sharing stopped: the session changed. Run /remote to share this one."
                            .to_owned(),
                    );
                }
                Some(messages) => {
                    for message in messages {
                        match share.link.outbound.try_send(message) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                share.publisher.mark_lagging();
                                break;
                            }
                            Err(TrySendError::Closed(_)) => {
                                closed = Some(
                                    "Remote sharing stopped: the gateway link closed".to_owned(),
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }

        if let Some(notice) = closed {
            self.close(SessionCloseReason::OwnerExited);
            app_state.lock().await.set_transient_notice(notice);
            *needs_redraw = true;
        }
    }
}

/// Apply one remote mutation on the loop's task, with the loop's own cancel
/// token, and finish it the way the terminal finishes its own.
async fn apply_mutation(
    registration: &SessionRegistration,
    request: &RemoteRequest,
    app_state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel_token: &mut CancellationToken,
    agent_ui_event_sender: &AgentUiEventSender,
) -> RemoteResponse {
    match apply_session_mutation(app_state, cancel_token, registration, request).await {
        Ok(mutation) => {
            match mutation.follow_up {
                OwnerFollowUp::None => {}
                // The queue lease makes this the same single worker a prompt
                // typed in the terminal would start.
                OwnerFollowUp::StartTurn => {
                    spawn_observed_orchestrator(
                        client.clone(),
                        Arc::clone(app_state),
                        cancel_token.clone(),
                        agent_ui_event_sender.clone(),
                    )
                    .await;
                }
                OwnerFollowUp::FinishCancel => {
                    rustcode::app::stop_turn_keeping_draft(app_state, cancel_token).await;
                }
            }
            RemoteResponse::new(request.request_id.clone(), mutation.result)
                .with_receipt(ReceiptState::Applied)
        }
        Err(error) => RemoteResponse::error(request.request_id.clone(), error)
            .with_receipt(ReceiptState::Rejected),
    }
}

fn reject_stale(request: RemoteRequest, reply: OwnerReply) {
    let response = RemoteResponse::error(
        request.request_id,
        RemoteError::new(
            RemoteErrorCode::StaleSession,
            "the session is no longer shared under this registration",
        ),
    );
    reply.send(if request.operation.is_mutation() {
        response.with_receipt(ReceiptState::Rejected)
    } else {
        response
    });
}

/// Apply an approval decision made in the terminal, unless the batch it was
/// made for is gone. A remote device may have resolved it while the key press
/// was queued; applying the late decision anyway would let a stale deny
/// cancel the turn the other answer allowed to continue.
pub(super) async fn apply_terminal_approval(
    app_state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    decision: rustcode::app::ApprovalDecision,
) -> bool {
    let pending = app_state.lock().await.tool_confirmation_response.is_some();
    if pending {
        apply_approval_decision(app_state, cancel_token, decision).await;
    } else {
        app_state
            .lock()
            .await
            .set_transient_notice("That approval was already resolved");
    }
    pending
}

/// As [`apply_terminal_approval`], for an answer to a question.
pub(super) async fn apply_terminal_answer(
    app_state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    answer: rustcode::app::QuestionAnswer,
) -> bool {
    let pending = app_state.lock().await.question_response.is_some();
    if pending {
        apply_question_answer(app_state, cancel_token, answer).await;
    } else {
        app_state
            .lock()
            .await
            .set_transient_notice("That question was already answered");
    }
    pending
}

#[cfg(test)]
impl AppRuntime {
    /// What one iteration of [`AppRuntime::run`] does for remote sharing and
    /// the prompt queue, without a terminal.
    async fn run_remote_iteration(&mut self) {
        let command = self.app_state.lock().await.remote_command.take();
        while let Ok(event) = self.agent_ui_event_receiver.try_recv() {
            self.remote.observe(&event);
        }
        self.remote
            .tick(
                command,
                RemotePump {
                    app_state: &self.app_state,
                    client: &self.client,
                    cancel_token: &mut self.current_cancel_token,
                    agent_ui_event_sender: &self.agent_ui_event_sender,
                    terminal_input_idle: self.app_event_receiver.is_empty(),
                    needs_redraw: &mut self.needs_redraw,
                },
            )
            .await;
        spawn_observed_orchestrator(
            self.client.clone(),
            Arc::clone(&self.app_state),
            self.current_cancel_token.clone(),
            self.agent_ui_event_sender.clone(),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcode::app::{DraftSubmitMode, PendingQuestion, QuestionAnswer};
    use rustcode::controller::ApprovalChoice;
    use rustcode::remote::owner::{GatewayLink, InMemoryConnector, InMemoryGateway};
    use rustcode::remote::{
        PromptDisposition, REMOTE_PROTOCOL_VERSION, RemoteAnswer, RemoteEvent, RemoteOperation,
        RemoteResult, RemoteSnapshot, ResyncReason,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const PROVIDER_TEXT: &str = "hello from provider";
    const WAIT: Duration = Duration::from_secs(15);

    /// A local streaming provider that counts the requests it receives. Every
    /// response sends its text, then stays open until `release` is set.
    struct FakeProvider {
        endpoint: String,
        requests: Arc<AtomicUsize>,
        release: tokio::sync::watch::Sender<bool>,
    }

    impl FakeProvider {
        async fn start(held: bool) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind local provider");
            let endpoint = format!(
                "http://{}/v1/chat/completions",
                listener.local_addr().expect("provider address")
            );
            let requests = Arc::new(AtomicUsize::new(0));
            let (release, released) = tokio::sync::watch::channel(!held);
            let counter = Arc::clone(&requests);
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(serve_stream(socket, released.clone()));
                }
            });
            Self {
                endpoint,
                requests,
                release,
            }
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }

        fn release(&self) {
            let _ = self.release.send(true);
        }

        fn state(&self) -> AppState {
            use rustcode::config::{ApiProtocol, ModelProfile};
            let mut state = AppState::new();
            state.api_base_url = self.endpoint.clone();
            state.model_name = "remote-bridge-test".to_owned();
            state.config.models = vec![ModelProfile {
                name: state.model_name.clone(),
                url: self.endpoint.clone(),
                model: state.model_name.clone(),
                api_protocol: Some(ApiProtocol::ChatCompletions),
                context_window: Some(8_192),
                ..ModelProfile::default()
            }];
            state.record_function_calling_support(&self.endpoint, false);
            state
        }
    }

    async fn serve_stream(
        mut socket: tokio::net::TcpStream,
        mut released: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let Ok(read) = socket.read(&mut buffer).await else {
                return;
            };
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        let first = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{PROVIDER_TEXT}\"}}}}]}}\n\n"
        );
        let last = concat!(
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        if socket
            .write_all(format!("{head}{:X}\r\n{first}\r\n", first.len()).as_bytes())
            .await
            .is_err()
        {
            return;
        }
        if released.wait_for(|released| *released).await.is_err() {
            return;
        }
        let _ = socket
            .write_all(format!("{:X}\r\n{last}\r\n0\r\n\r\n", last.len()).as_bytes())
            .await;
    }

    /// A runtime whose `/remote` connects to an in-memory gateway.
    fn shared_runtime(state: AppState, outbound_capacity: usize) -> (AppRuntime, InMemoryGateway) {
        let (connector, gateway) = InMemoryConnector::new(outbound_capacity, 4);
        let mut runtime = AppRuntime::for_test(state);
        runtime.remote = RemoteBridge::with_connector(Box::new(connector));
        (runtime, gateway)
    }

    /// Enter `line` in the composer the way the terminal does.
    async fn enter(runtime: &mut AppRuntime, line: &str) {
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = line.to_owned();
            state.cursor_position = line.len();
        }
        rustcode::app::handle_enter_with_ui_events(
            &runtime.app_state,
            &runtime.client,
            &mut runtime.current_cancel_token,
            runtime.agent_ui_event_sender.clone(),
            &|| Vec::new(),
        )
        .await;
    }

    /// Run `/remote` and return the link the gateway was handed, with the
    /// snapshot that opened it.
    async fn share(
        runtime: &mut AppRuntime,
        gateway: &InMemoryGateway,
    ) -> (SessionRegistration, GatewayLink, Box<RemoteSnapshot>) {
        enter(runtime, "/remote").await;
        runtime.run_remote_iteration().await;
        let (registration, mut link) = gateway.accept().expect("/remote opened a link");
        let Ok(OwnerMessage::Register {
            registration: registered,
            snapshot,
        }) = link.messages.try_recv()
        else {
            panic!("a link opens with its registration");
        };
        assert_eq!(registered, registration);
        (registration, link, snapshot)
    }

    fn request(registration: &SessionRegistration, operation: RemoteOperation) -> RemoteRequest {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        RemoteRequest {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: format!("req-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            session_id: Some(registration.session_id.clone()),
            registration_epoch: Some(registration.registration_epoch),
            operation,
        }
    }

    /// Send `operation` and run the loop once: the owner's decision.
    async fn send(
        runtime: &mut AppRuntime,
        link: &GatewayLink,
        registration: &SessionRegistration,
        operation: RemoteOperation,
    ) -> RemoteResponse {
        let response = link
            .try_request(request(registration, operation))
            .expect("the command queue has room");
        runtime.run_remote_iteration().await;
        response.await.expect("the owner replied")
    }

    fn rejection(response: &RemoteResponse) -> RemoteErrorCode {
        assert_eq!(response.receipt, Some(ReceiptState::Rejected));
        match &response.result {
            RemoteResult::Error(error) => error.code,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    fn submit(prompt: &str) -> RemoteOperation {
        RemoteOperation::SubmitPrompt {
            prompt: prompt.to_owned(),
        }
    }

    /// Run the loop until `done` holds for the messages received so far.
    async fn publish_until(
        runtime: &mut AppRuntime,
        link: &mut GatewayLink,
        received: &mut Vec<OwnerMessage>,
        done: impl Fn(&[OwnerMessage]) -> bool,
    ) {
        tokio::time::timeout(WAIT, async {
            loop {
                runtime.run_remote_iteration().await;
                while let Ok(message) = link.messages.try_recv() {
                    received.push(message);
                }
                if done(received) {
                    break;
                }
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the bridge published what the test waits for");
    }

    fn events(messages: &[OwnerMessage]) -> Vec<(u64, RemoteEvent)> {
        messages
            .iter()
            .filter_map(|message| match message {
                OwnerMessage::Event(frame) => Some((frame.sequence, frame.event.clone())),
                _ => None,
            })
            .collect()
    }

    fn turn_ended(messages: &[OwnerMessage]) -> bool {
        events(messages)
            .iter()
            .any(|(_, event)| matches!(event, RemoteEvent::TurnFinished { .. }))
    }

    async fn panel(runtime: &AppRuntime) -> String {
        runtime
            .app_state()
            .await
            .command_panel
            .as_ref()
            .map(|panel| panel.content.clone())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn remote_without_a_gateway_reports_it_and_shares_nothing() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        enter(&mut runtime, "/remote").await;
        runtime.run_remote_iteration().await;
        let report = panel(&runtime).await;
        assert!(
            report.contains("not available") && report.contains("no remote gateway is available"),
            "unexpected report: {report}"
        );
        assert!(report.contains("stays private"));
        assert!(runtime.remote.registration().is_none());

        enter(&mut runtime, "/remote status").await;
        runtime.run_remote_iteration().await;
        assert!(panel(&runtime).await.contains("This session is private"));

        enter(&mut runtime, "/remote off").await;
        runtime.run_remote_iteration().await;
        assert!(panel(&runtime).await.contains("not shared"));

        enter(&mut runtime, "/remote everywhere").await;
        assert!(panel(&runtime).await.starts_with("Usage:"));
        assert!(runtime.app_state().await.remote_command.is_none());
    }

    #[tokio::test]
    async fn repeating_remote_keeps_the_same_registration() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 16);
        let (registration, _link, snapshot) = share(&mut runtime, &gateway).await;
        assert_eq!(snapshot.sequence, 0);
        assert_eq!(snapshot.session.session_id, registration.session_id);

        enter(&mut runtime, "/remote").await;
        runtime.run_remote_iteration().await;
        assert!(gateway.accept().is_none(), "no second link is opened");
        assert_eq!(runtime.remote.registration(), Some(&registration));
        assert!(panel(&runtime).await.contains("This session is shared"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attaching_during_generation_starts_no_second_turn() {
        let provider = FakeProvider::start(true).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 256);
        enter(&mut runtime, "say hello").await;
        tokio::time::timeout(WAIT, async {
            while runtime.app_state().await.current_response.is_empty() {
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the terminal's turn is streaming");
        let (session_id, turn_id) = {
            let state = runtime.app_state().await;
            (
                state.active_session_id.clone(),
                state.active_turn_id.clone().expect("a running turn"),
            )
        };

        let (registration, mut link, snapshot) = share(&mut runtime, &gateway).await;
        assert_eq!(registration.session_id, session_id);
        let turn = snapshot.turn.as_ref().expect("the snapshot shows the turn");
        assert_eq!(turn.turn_id, turn_id);
        assert_eq!(turn.live_response.text, PROVIDER_TEXT);
        {
            let state = runtime.app_state().await;
            assert_eq!(state.active_session_id, session_id);
            assert_eq!(state.active_turn_id.as_deref(), Some(turn_id.as_str()));
            assert!(state.orchestrator_running);
        }
        assert_eq!(provider.requests(), 1);

        provider.release();
        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, turn_ended).await;
        let published = events(&received);
        // The text the snapshot already held is not sent again, and the turn
        // that ends is the one the snapshot named.
        assert!(
            !published
                .iter()
                .any(|(_, event)| matches!(event, RemoteEvent::TextDelta { .. })),
            "unexpected text after the snapshot: {published:?}"
        );
        assert_eq!(
            published.last().map(|(_, event)| event),
            Some(&RemoteEvent::TurnFinished {
                turn_id: Some(turn_id)
            })
        );
        for (index, (sequence, _)) in published.iter().enumerate() {
            assert_eq!(*sequence, index as u64 + 1);
        }
        assert_eq!(provider.requests(), 1, "one provider request in total");
        assert_eq!(runtime.app_state().await.active_session_id, session_id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_remote_prompt_runs_once_and_leaves_the_terminal_draft_alone() {
        let provider = FakeProvider::start(false).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 256);
        // An attachment is part of the draft text: a pasted image path.
        let draft = "look at /tmp/screenshot.png and";
        let draft_of = |state: &AppState| {
            (
                state.input_buffer.clone(),
                state.cursor_position,
                state.draft_submit_mode,
                state.selected_subagent_id,
                state.input_history.clone(),
            )
        };
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = draft.to_owned();
            state.cursor_position = 7;
            state.draft_submit_mode = DraftSubmitMode::Queue;
        }
        let before = draft_of(&*runtime.app_state().await);
        // `/remote` is entered through the composer, so it replaces the draft
        // like any command; restore it to what the user was typing.
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = draft.to_owned();
            state.cursor_position = 7;
            state.draft_submit_mode = DraftSubmitMode::Queue;
            state.input_history.clear();
        }

        // Remote text is never a command, and refusing it changes nothing.
        let refused = send(&mut runtime, &link, &registration, submit("/exit")).await;
        assert_eq!(rejection(&refused), RemoteErrorCode::UnsupportedOperation);
        assert_eq!(provider.requests(), 0);

        let accepted = send(&mut runtime, &link, &registration, submit("from the phone")).await;
        assert_eq!(accepted.receipt, Some(ReceiptState::Applied));
        assert_eq!(
            accepted.result,
            RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Started
            }
        );
        assert_eq!(draft_of(&*runtime.app_state().await), before);

        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, turn_ended).await;
        let published = events(&received);
        assert!(matches!(
            published.first(),
            Some((_, RemoteEvent::TurnStarted { turn_id: Some(_), prompt })) if prompt.text == "from the phone"
        ));
        let text: String = published
            .iter()
            .filter_map(|(_, event)| match event {
                RemoteEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, PROVIDER_TEXT);

        let state = runtime.app_state().await;
        assert_eq!(provider.requests(), 1, "the prompt ran exactly once");
        assert_eq!(
            state
                .history
                .iter()
                .filter(|message| message.role == "user" && message.content == "from the phone")
                .count(),
            1
        );
        assert_eq!(draft_of(&state), before);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_queue_and_steer_follow_the_running_turn_rules() {
        let provider = FakeProvider::start(true).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 256);
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;
        let queue = || RemoteOperation::Queue {
            prompt: "afterwards".to_owned(),
        };
        let steer = || RemoteOperation::Steer {
            prompt: "change course".to_owned(),
        };

        // Idle: there is nothing to queue behind or to steer.
        let idle_queue = send(&mut runtime, &link, &registration, queue()).await;
        assert_eq!(rejection(&idle_queue), RemoteErrorCode::NotRunning);
        let idle_steer = send(&mut runtime, &link, &registration, steer()).await;
        assert_eq!(rejection(&idle_steer), RemoteErrorCode::NotRunning);

        enter(&mut runtime, "terminal prompt").await;
        tokio::time::timeout(WAIT, async {
            while runtime.app_state().await.current_response.is_empty() {
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the terminal's turn is streaming");
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = "terminal draft".to_owned();
            state.cursor_position = 3;
        }

        // Running: a new prompt is refused, a steer is accepted only by a
        // turn that takes steering, and a queued prompt waits its turn.
        let busy = send(&mut runtime, &link, &registration, submit("now")).await;
        assert_eq!(rejection(&busy), RemoteErrorCode::Busy);
        let steerable = runtime.app_state().await.can_accept_steer();
        let steered = send(&mut runtime, &link, &registration, steer()).await;
        if steerable {
            assert_eq!(steered.receipt, Some(ReceiptState::Applied));
        } else {
            assert_eq!(rejection(&steered), RemoteErrorCode::UnsupportedOperation);
            assert!(runtime.app_state().await.pending_steers.is_empty());
        }
        let queued = send(&mut runtime, &link, &registration, queue()).await;
        assert_eq!(
            queued.result,
            RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Queued
            }
        );
        {
            let state = runtime.app_state().await;
            assert_eq!(state.pending_queue, ["afterwards"]);
            assert_eq!(state.input_buffer, "terminal draft");
            assert_eq!(state.cursor_position, 3);
        }
        assert_eq!(provider.requests(), 1, "queueing starts nothing");

        provider.release();
        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, |messages| {
            events(messages)
                .iter()
                .filter(|(_, event)| matches!(event, RemoteEvent::TurnFinished { .. }))
                .count()
                == 2
        })
        .await;
        let state = runtime.app_state().await;
        let prompts: Vec<&str> = state
            .history
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.as_str())
            .collect();
        let expected: &[&str] = if steerable {
            &["terminal prompt", "change course", "afterwards"]
        } else {
            &["terminal prompt", "afterwards"]
        };
        assert_eq!(prompts, expected);
        assert_eq!(state.input_buffer, "terminal draft");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_remote_cancel_stops_only_the_turn_it_names() {
        let provider = FakeProvider::start(true).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 256);
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;
        let cancel = |turn_id: &str| RemoteOperation::CancelTurn {
            turn_id: turn_id.to_owned(),
        };
        let streaming_turn = |runtime: &AppRuntime| {
            let state = Arc::clone(&runtime.app_state);
            async move {
                tokio::time::timeout(WAIT, async {
                    loop {
                        let state = state.lock().await;
                        if !state.current_response.is_empty()
                            && let Some(turn_id) = state.active_turn_id.clone()
                        {
                            break turn_id;
                        }
                        drop(state);
                        tokio::time::sleep(EVENT_POLL_INTERVAL).await;
                    }
                })
                .await
                .expect("a turn is streaming")
            }
        };

        enter(&mut runtime, "first").await;
        let first = streaming_turn(&runtime).await;
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = "terminal draft".to_owned();
            state.cursor_position = 5;
        }
        let stale = send(&mut runtime, &link, &registration, cancel("turn:0:0")).await;
        assert_eq!(rejection(&stale), RemoteErrorCode::StaleTurn);
        assert!(runtime.app_state().await.orchestrator_running);

        let cancelled = send(&mut runtime, &link, &registration, cancel(&first)).await;
        assert_eq!(cancelled.receipt, Some(ReceiptState::Applied));
        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, |messages| {
            events(messages).iter().any(|(_, event)| {
                matches!(event, RemoteEvent::TurnCancelled { turn_id } if turn_id.as_deref() == Some(first.as_str()))
            })
        })
        .await;
        {
            let state = runtime.app_state().await;
            assert_eq!(state.input_buffer, "terminal draft");
            assert_eq!(state.cursor_position, 5);
            assert_eq!(state.active_turn_id, None);
        }

        // The terminal stops the next turn itself. A remote cancel that still
        // names it is stale and leaves the replacement token alone.
        tokio::time::timeout(WAIT, async {
            while runtime.app_state().await.orchestrator_running {
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the cancelled turn unwinds");
        enter(&mut runtime, "second").await;
        let second = streaming_turn(&runtime).await;
        runtime
            .handle_event(AppEvent::CancelActiveTurn)
            .await
            .expect("Esc is handled");
        let token = runtime.current_cancel_token.clone();
        let late = send(&mut runtime, &link, &registration, cancel(&second)).await;
        assert_eq!(rejection(&late), RemoteErrorCode::StaleTurn);
        assert!(!token.is_cancelled());
    }

    #[tokio::test]
    async fn a_full_outbound_queue_resyncs_without_stalling_the_terminal() {
        // Room for the registration and two more messages; nothing reads.
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 3);
        let (_, mut link, _) = share(&mut runtime, &gateway).await;
        let ask = |runtime: &AppRuntime, text: &str| {
            let question = PendingQuestion::new(text.to_owned(), vec!["Yes".to_owned()], false);
            let id = question.id.clone();
            let state = Arc::clone(&runtime.app_state);
            async move {
                state.lock().await.pending_question = Some(question);
                id
            }
        };

        let mut last_question = String::new();
        for round in 0..40 {
            last_question = ask(&runtime, &format!("Question {round}?")).await;
            tokio::time::timeout(Duration::from_secs(1), runtime.run_remote_iteration())
                .await
                .expect("a full link never blocks the loop");
        }
        // The terminal still takes its own input meanwhile.
        runtime
            .handle_event(AppEvent::RequestDraw)
            .await
            .expect("terminal input is handled");

        let mut delivered = Vec::new();
        while let Ok(message) = link.messages.try_recv() {
            delivered.push(message);
        }
        assert_eq!(delivered.len(), 3, "the queue held its bound");
        let last_delivered = events(&delivered)
            .last()
            .map_or(0, |(sequence, _)| *sequence);

        // With room again the owner resyncs: no event in between, then a
        // snapshot that says so and whose watermark shows the gap.
        runtime.run_remote_iteration().await;
        let Ok(OwnerMessage::Snapshot { snapshot, resync }) = link.messages.try_recv() else {
            panic!("the owner resyncs with a snapshot");
        };
        assert_eq!(resync, Some(ResyncReason::Lagged));
        assert!(snapshot.sequence > last_delivered + 1);
        assert_eq!(
            snapshot
                .pending_question
                .as_ref()
                .map(|question| question.question_id.as_str()),
            Some(last_question.as_str())
        );
        assert!(link.messages.try_recv().is_err());

        // Events resume right after the snapshot's watermark.
        let next_question = ask(&runtime, "One more?").await;
        runtime.run_remote_iteration().await;
        let mut resumed = Vec::new();
        while let Ok(message) = link.messages.try_recv() {
            resumed.push(message);
        }
        let resumed = events(&resumed);
        assert_eq!(resumed[0].0, snapshot.sequence + 1);
        assert!(matches!(
            &resumed[1],
            (_, RemoteEvent::QuestionRequested { question }) if question.question_id == next_question
        ));
    }

    #[tokio::test]
    async fn remote_off_drops_the_registration_and_rejects_what_was_queued() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 16);
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;

        // The command reached the owner's queue before `/remote off` ran.
        let queued = link
            .try_request(request(&registration, submit("too late")))
            .expect("queued");
        enter(&mut runtime, "/remote off").await;
        runtime.run_remote_iteration().await;

        let response = queued.await.expect("the owner replied");
        assert_eq!(rejection(&response), RemoteErrorCode::StaleSession);
        assert!(runtime.remote.registration().is_none());
        assert!(runtime.app_state().await.pending_queue.is_empty());
        assert!(panel(&runtime).await.contains("Remote sharing is off"));
        assert_eq!(
            link.messages.recv().await,
            Some(OwnerMessage::Unregister {
                reason: SessionCloseReason::SharingDisabled
            })
        );
        assert_eq!(link.messages.recv().await, None, "the link is closed");
        assert!(
            link.try_request(request(&registration, submit("x")))
                .is_err()
        );

        // Sharing again is a new registration; the old epoch stays dead.
        let (again, new_link, _) = share(&mut runtime, &gateway).await;
        assert_eq!(again.session_id, registration.session_id);
        assert!(again.registration_epoch > registration.registration_epoch);
        let old = new_link
            .try_request(request(&registration, submit("old epoch")))
            .expect("queued");
        runtime.run_remote_iteration().await;
        assert_eq!(
            rejection(&old.await.expect("the owner replied")),
            RemoteErrorCode::StaleSession
        );
        assert!(runtime.app_state().await.pending_queue.is_empty());
    }

    #[tokio::test]
    async fn a_session_switch_drops_the_registration_and_stays_private() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 16);
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;

        let queued = link
            .try_request(request(&registration, submit("for the old session")))
            .expect("queued");
        runtime
            .handle_event(AppEvent::NewSession)
            .await
            .expect("the terminal starts a new session");
        let new_session = runtime.app_state().await.active_session_id.clone();
        assert_ne!(new_session, registration.session_id);
        runtime.run_remote_iteration().await;

        assert_eq!(
            rejection(&queued.await.expect("the owner replied")),
            RemoteErrorCode::StaleSession
        );
        assert!(runtime.app_state().await.pending_queue.is_empty());
        assert!(runtime.remote.registration().is_none());
        // Nothing about the new session went out under the old registration.
        assert_eq!(
            link.messages.recv().await,
            Some(OwnerMessage::Unregister {
                reason: SessionCloseReason::SessionChanged
            })
        );
        assert_eq!(link.messages.recv().await, None);

        // Activity in the new session publishes nowhere until it is shared.
        runtime
            .app_state
            .lock()
            .await
            .history
            .push(ChatMessage::new("user", "private"));
        runtime.run_remote_iteration().await;
        assert!(gateway.accept().is_none());
        enter(&mut runtime, "/remote status").await;
        runtime.run_remote_iteration().await;
        assert!(panel(&runtime).await.contains("This session is private"));

        let (again, _link, snapshot) = share(&mut runtime, &gateway).await;
        assert_eq!(again.session_id, new_session);
        assert_eq!(snapshot.session.session_id, new_session);
        assert!(again.registration_epoch > registration.registration_epoch);
    }

    fn pending_question(state: &mut AppState) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let question = PendingQuestion::new(
            "Proceed?".to_owned(),
            vec!["Yes".to_owned(), "No".to_owned()],
            false,
        );
        let id = question.id.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(question);
        state.question_response = Some(tx);
        (id, rx)
    }

    fn answer(question_id: &str, option: &str) -> RemoteOperation {
        RemoteOperation::AnswerQuestion {
            question_id: question_id.to_owned(),
            answer: RemoteAnswer::Selected {
                options: vec![option.to_owned()],
            },
        }
    }

    #[tokio::test]
    async fn the_first_answer_to_a_question_wins_whichever_side_gives_it() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 64);
        let (registration, link, _) = share(&mut runtime, &gateway).await;

        // The remote device answers first; the terminal's answer is late.
        let (question_id, mut answered) = pending_question(&mut *runtime.app_state.lock().await);
        let remote = send(
            &mut runtime,
            &link,
            &registration,
            answer(&question_id, "Yes"),
        )
        .await;
        assert_eq!(remote.receipt, Some(ReceiptState::Applied));
        let token = runtime.current_cancel_token.clone();
        for late in [
            QuestionAnswer::Selected("No".to_owned()),
            QuestionAnswer::Cancelled,
        ] {
            runtime
                .handle_event(AppEvent::AnswerQuestion(late))
                .await
                .expect("the terminal event is handled");
        }
        assert_eq!(answered.try_recv().as_deref(), Ok("User selected: Yes"));
        assert!(!token.is_cancelled(), "a late cancel stops nothing");
        assert_eq!(
            runtime.app_state().await.active_transient_notice(),
            Some("That question was already answered")
        );

        // The terminal answered first: its key press is already queued when
        // the remote answer arrives, so the remote one is the stale one.
        let (question_id, mut answered) = pending_question(&mut *runtime.app_state.lock().await);
        let _ = runtime
            .app_event_sender
            .send(AppEvent::AnswerQuestion(QuestionAnswer::Selected(
                "No".to_owned(),
            )));
        let remote = link
            .try_request(request(&registration, answer(&question_id, "Yes")))
            .expect("queued");
        runtime.run_remote_iteration().await;
        assert!(answered.try_recv().is_err(), "the remote answer waits");
        let terminal = runtime
            .app_event_receiver
            .try_recv()
            .expect("the terminal's answer is queued");
        runtime
            .handle_event(terminal)
            .await
            .expect("the terminal event is handled");
        runtime.run_remote_iteration().await;
        assert_eq!(answered.try_recv().as_deref(), Ok("User selected: No"));
        assert_eq!(
            rejection(&remote.await.expect("the owner replied")),
            RemoteErrorCode::StaleQuestion
        );
    }

    #[tokio::test]
    async fn a_late_terminal_denial_cannot_undo_a_remote_approval() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 64);
        let (registration, link, _) = share(&mut runtime, &gateway).await;
        let (tx, mut decided) = tokio::sync::oneshot::channel();
        {
            let mut state = runtime.app_state.lock().await;
            state.status = AppStatus::AwaitingToolConfirmation;
            state.pending_tool_confirmation = Some(Vec::new());
            state.pending_approval_batch_id = Some("batch-1".to_owned());
            state.tool_confirmation_response = Some(tx);
        }
        let resolve = |choice| RemoteOperation::ResolveApproval {
            batch_id: "batch-1".to_owned(),
            choice,
        };

        let approved = send(
            &mut runtime,
            &link,
            &registration,
            resolve(ApprovalChoice::Approve),
        )
        .await;
        assert_eq!(approved.receipt, Some(ReceiptState::Applied));
        let token = runtime.current_cancel_token.clone();
        runtime
            .handle_event(AppEvent::ApprovalDecision(
                rustcode::app::ApprovalDecision::Deny,
            ))
            .await
            .expect("the terminal event is handled");

        assert_eq!(
            decided.try_recv(),
            Ok(rustcode::app::ToolConfirmationResponse::Approve)
        );
        assert!(!token.is_cancelled(), "the approved turn keeps running");
        let second = send(
            &mut runtime,
            &link,
            &registration,
            resolve(ApprovalChoice::Deny),
        )
        .await;
        assert_eq!(rejection(&second), RemoteErrorCode::StaleApproval);
    }
}
