//! The silence deadline of a harness turn, against a fake ACP agent the test
//! drives step by step and paused time (`watchdog.rs`).

use super::*;
use crate::harness::HarnessKind;
use crate::harness::rpc::{Incoming, RpcError, RpcPeer};
use crate::harness::session::test_support::{harness, pipe};
use crate::harness::session::{
    Adapter, ClientSink, ExternalSession, PermissionFuture, StartOptions,
};
use crate::harness::store::HarnessRecord;
use agent_client_protocol::schema::v1::{
    ContentBlock, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, StopReason, TextContent,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// Not shared with `acp_child_tests`: a harness's learned capabilities live
// in one place per id.
const NAME: &str = "quiet-acp";
const TEN_MINUTES: Duration = Duration::from_secs(10 * 60);

/// What the test tells the fake agent to do next.
enum Step {
    /// Send one `session/update` to the client.
    Update(Value),
    /// Send a tool-call approval request and wait for the answer off to the side.
    AskPermission,
    /// Answer the running prompt.
    Finish,
}

/// The fake ACP agent: it answers the handshake at once and a prompt only when
/// the test says so, so a turn stays silent for exactly as long as the test
/// wants.
async fn fake_agent(
    peer: Arc<RpcPeer>,
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    mut steps: mpsc::UnboundedReceiver<Step>,
    seen: mpsc::UnboundedSender<String>,
) {
    let mut prompt: Option<Value> = None;
    loop {
        tokio::select! {
            Some(message) = incoming.recv() => match message {
                Incoming::Request { id, method, .. } => {
                    let _ = seen.send(method.clone());
                    match method.as_str() {
                        "initialize" => {
                            peer.respond(id, Ok(json!({
                                "protocolVersion": 1,
                                "agentCapabilities": {"loadSession": true},
                            })));
                        }
                        "session/new" | "session/load" => {
                            peer.respond(id, Ok(json!({"sessionId": "child-1"})));
                        }
                        // A turn that never answers on its own.
                        "session/prompt" => prompt = Some(id),
                        other => {
                            peer.respond(id, Err(RpcError::method_not_found(other)));
                        }
                    }
                }
                // A turn the user stopped stops at once; a turn the test wants
                // stopped otherwise is answered by `Step::Finish`.
                Incoming::Notification { method, .. } => {
                    if method == "session/cancel"
                        && let Some(id) = prompt.take()
                    {
                        peer.respond(id, Ok(json!({"stopReason": "cancelled"})));
                    }
                }
                Incoming::Barrier(done) => {
                    let _ = done.send(());
                }
            },
            Some(step) = steps.recv() => match step {
                Step::Update(update) => peer.notify(
                    "session/update",
                    json!({"sessionId": "child-1", "update": update}),
                ),
                Step::AskPermission => {
                    let peer = Arc::clone(&peer);
                    // Asked off the side: the agent's loop keeps its turn open.
                    tokio::spawn(async move {
                        let _ = peer
                            .request(
                                "session/request_permission",
                                json!({
                                    "sessionId": "child-1",
                                    "toolCall": {"toolCallId": "t1", "title": "Run the tests"},
                                    "options": [
                                        {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                        {"optionId": "reject", "name": "Reject", "kind": "reject_once"}],
                                }),
                            )
                            .await;
                    });
                }
                Step::Finish => {
                    if let Some(id) = prompt.take() {
                        peer.respond(id, Ok(json!({"stopReason": "end_turn"})));
                    }
                }
            },
            else => break,
        }
    }
}

/// A sink that holds every permission request until the test answers it.
struct HoldSink {
    updates: mpsc::UnboundedSender<SessionUpdate>,
    asks: mpsc::UnboundedSender<oneshot::Sender<RequestPermissionResponse>>,
}

impl ClientSink for HoldSink {
    fn update(&self, update: SessionUpdate) {
        let _ = self.updates.send(update);
    }

    fn request_permission(&self, _request: RequestPermissionRequest) -> PermissionFuture {
        let (answer, held) = oneshot::channel();
        let _ = self.asks.send(answer);
        Box::pin(async move {
            held.await.unwrap_or_else(|_| {
                RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
            })
        })
    }
}

/// A thread on the fake harness, with the test's own silence deadline.
struct Fixture {
    session: Arc<ExternalSession>,
    steps: mpsc::UnboundedSender<Step>,
    seen: mpsc::UnboundedReceiver<String>,
    updates: mpsc::UnboundedReceiver<SessionUpdate>,
    asks: mpsc::UnboundedReceiver<oneshot::Sender<RequestPermissionResponse>>,
}

impl Fixture {
    /// `silence` is the deadline to run with; `None` turns it off, as a
    /// configured 0 does.
    async fn start(silence: Option<Duration>) -> Self {
        let (updates, updates_rx) = mpsc::unbounded_channel();
        let (asks, asks_rx) = mpsc::unbounded_channel();
        let sink = Arc::new(HoldSink { updates, asks });
        let mut record = HarnessRecord::new(NAME, std::env::temp_dir());
        // A started thread records its harness session, which the next start resumes.
        record.native_session_id = Some("child-1".to_owned());
        let session = ExternalSession::new(
            "silent-thread".to_owned(),
            harness(NAME, HarnessKind::Acp),
            record,
            Vec::new(),
            sink,
            false,
        );
        session.set_silence_timeout(silence);
        let (steps, seen) = connect(&session).await;
        Self {
            session,
            steps,
            seen,
            updates: updates_rx,
            asks: asks_rx,
        }
    }

    /// A replacement harness, started the way the next message starts one.
    async fn restart(&mut self) {
        let (steps, seen) = connect(&self.session).await;
        self.steps = steps;
        self.seen = seen;
    }

    fn run(&self, text: &str) -> JoinHandle<Result<StopReason, String>> {
        let session = Arc::clone(&self.session);
        let prompt = vec![ContentBlock::Text(TextContent::new(text.to_owned()))];
        tokio::spawn(async move { session.prompt(prompt).await })
    }

    /// Wait for `count` requests to wait on the user: an answer reaches the
    /// clock off the sink's own task.
    async fn waiting_requests(&self, count: usize) {
        for _ in 0..100 {
            if self.session.waiting_requests() == count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(
            self.session.waiting_requests(),
            count,
            "expected {count} requests waiting on the user"
        );
    }

    fn step(&self, step: Step) {
        self.steps.send(step).expect("the fake agent is gone");
    }

    /// Wait until the fake agent has the turn's prompt.
    async fn prompted(&mut self) {
        while let Some(method) = self.seen.recv().await {
            if method == "session/prompt" {
                return;
            }
        }
        panic!("the fake agent stopped");
    }

    /// Wait until the client got the update of `kind`, which means the engine
    /// counted it as output.
    async fn update(&mut self, kind: &str) -> Value {
        loop {
            let Some(update) = self.updates.recv().await else {
                panic!("the thread stopped");
            };
            let value = serde_json::to_value(&update).unwrap();
            if value["sessionUpdate"] == kind {
                return value;
            }
        }
    }
}

/// Start a fake harness on the thread, as a first start and a restart both do.
async fn connect(
    session: &Arc<ExternalSession>,
) -> (mpsc::UnboundedSender<Step>, mpsc::UnboundedReceiver<String>) {
    let harness = harness(NAME, HarnessKind::Acp);
    let ((ours, our_incoming), (theirs, their_incoming)) = pipe();
    let (steps, step_rx) = mpsc::unbounded_channel();
    let (seen, seen_rx) = mpsc::unbounded_channel();
    tokio::spawn(fake_agent(theirs, their_incoming, step_rx, seen));
    let adapter = Adapter::connect(
        &harness,
        ours,
        our_incoming,
        None,
        &std::env::temp_dir(),
        StartOptions::default(),
        session.output(),
    )
    .await
    .expect("the fake agent starts");
    session.attach(adapter).await;
    (steps, seen_rx)
}

fn tool_call(id: &str, status: &str) -> Step {
    Step::Update(json!({
        "sessionUpdate": "tool_call", "toolCallId": id, "title": "Run the tests",
        "kind": "execute", "status": status,
    }))
}

fn tool_done(id: &str) -> Step {
    Step::Update(
        json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed"}),
    )
}

#[tokio::test(start_paused = true)]
async fn a_silent_turn_fails_and_its_harness_is_torn_down() {
    let mut fixture = Fixture::start(Some(TEN_MINUTES)).await;
    let turn = fixture.run("write the notes");
    fixture.prompted().await;
    let error = turn.await.unwrap().unwrap_err();
    assert_eq!(
        error,
        "No output from quiet-acp for 10 minutes. Send a message to try again."
    );
    assert!(
        !fixture.session.has_adapter().await,
        "the silent harness was torn down, so the next message starts a new one"
    );
    assert_eq!(
        fixture.session.record().native_session_id.as_deref(),
        Some("child-1"),
        "the harness's session is kept, so the next start resumes it"
    );
}

#[tokio::test(start_paused = true)]
async fn a_tool_in_flight_holds_the_deadline_for_an_hour() {
    let mut fixture = Fixture::start(Some(TEN_MINUTES)).await;
    let turn = fixture.run("run the tests");
    fixture.prompted().await;
    fixture.step(tool_call("t1", "in_progress"));
    fixture.update("tool_call").await;

    tokio::time::advance(Duration::from_secs(60 * 60)).await;
    assert!(
        !turn.is_finished(),
        "a tool that runs for an hour is not silence"
    );

    fixture.step(tool_done("t1"));
    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));
}

#[tokio::test(start_paused = true)]
async fn a_permission_waiting_on_the_user_never_times_out() {
    let mut fixture = Fixture::start(Some(TEN_MINUTES)).await;
    let turn = fixture.run("delete the build directory");
    fixture.prompted().await;
    fixture.step(tool_call("t1", "pending"));
    fixture.update("tool_call").await;
    fixture.step(Step::AskPermission);
    let answer = fixture.asks.recv().await.expect("the user is asked");
    // Only the approval is left open: the tool itself is finished.
    fixture.step(tool_done("t1"));
    fixture.update("tool_call_update").await;

    tokio::time::advance(Duration::from_secs(60 * 60)).await;
    assert!(
        !turn.is_finished(),
        "a turn waiting on the user never times out"
    );

    answer.send(allowed()).unwrap();
    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));
}

/// A permission left open by a turn that ended is still the user's: answering
/// it must not be counted as answering the next turn's.
#[tokio::test(start_paused = true)]
async fn a_permission_left_open_by_the_last_turn_holds_the_next_ones_deadline() {
    let mut fixture = Fixture::start(Some(TEN_MINUTES)).await;
    // Turn 1 asks for an approval and the user stops the turn with it open.
    let first = fixture.run("first question");
    fixture.prompted().await;
    fixture.step(tool_call("t1", "pending"));
    fixture.update("tool_call").await;
    fixture.step(Step::AskPermission);
    let first_ask = fixture.asks.recv().await.expect("the user is asked");
    fixture.session.cancel();
    assert_eq!(first.await.unwrap(), Ok(StopReason::Cancelled));

    // Turn 2 asks for its own, so two requests wait on the user.
    let second = fixture.run("second question");
    fixture.prompted().await;
    fixture.step(Step::AskPermission);
    let second_ask = fixture.asks.recv().await.expect("the user is asked again");

    assert_eq!(
        fixture.session.waiting_requests(),
        2,
        "the card the last turn left and this turn's both wait on the user"
    );

    // Answering the old card leaves the new one open.
    first_ask.send(allowed()).unwrap();
    fixture.waiting_requests(1).await;
    tokio::time::advance(Duration::from_secs(2 * 60 * 60)).await;
    assert!(
        !second.is_finished(),
        "the second request still waits on the user, so this turn never times out"
    );

    // Answering the last one lets the deadline run again.
    second_ask.send(allowed()).unwrap();
    fixture.waiting_requests(0).await;
    tokio::time::advance(Duration::from_secs(11 * 60)).await;
    let error = second.await.unwrap().unwrap_err();
    assert_eq!(
        error,
        "No output from quiet-acp for 10 minutes. Send a message to try again."
    );
}

fn allowed() -> RequestPermissionResponse {
    RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new("allow"),
    ))
}

#[tokio::test(start_paused = true)]
async fn an_event_restarts_the_deadline() {
    let mut fixture = Fixture::start(Some(TEN_MINUTES)).await;
    let turn = fixture.run("think about it");
    fixture.prompted().await;
    tokio::time::advance(Duration::from_secs(9 * 60)).await;
    assert!(!turn.is_finished(), "9 minutes is not yet silence");

    // A thought is output like anything else.
    fixture.step(Step::Update(json!({
        "sessionUpdate": "agent_thought_chunk",
        "content": {"type": "text", "text": "Still thinking."},
    })));
    fixture.update("agent_thought_chunk").await;

    tokio::time::advance(Duration::from_secs(9 * 60)).await;
    assert!(
        !turn.is_finished(),
        "the thought restarted the clock, so 18 minutes in all is still fine"
    );

    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));
}

#[tokio::test(start_paused = true)]
async fn an_update_the_engine_cannot_read_still_counts_as_output() {
    let mut fixture = Fixture::start(Some(TEN_MINUTES)).await;
    let turn = fixture.run("think about it");
    fixture.prompted().await;
    tokio::time::advance(Duration::from_secs(9 * 60)).await;
    let before = fixture.session.last_output_at().expect("the turn began");

    // Nothing RustCode knows, so it becomes no update of its own: the agent
    // is nonetheless talking, which a heartbeat tells the deadline.
    fixture.step(Step::Update(json!({"sessionUpdate": "from_the_future"})));
    for _ in 0..100 {
        if fixture.session.last_output_at() != Some(before) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_ne!(
        fixture.session.last_output_at(),
        Some(before),
        "an update the engine cannot read is still output"
    );

    tokio::time::advance(Duration::from_secs(9 * 60)).await;
    assert!(
        !turn.is_finished(),
        "that message restarted the deadline, so 18 minutes in all is fine"
    );
    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));
}

#[tokio::test(start_paused = true)]
async fn the_deadline_restarts_when_the_last_tool_finishes() {
    let mut fixture = Fixture::start(Some(TEN_MINUTES)).await;
    let turn = fixture.run("run the tests");
    fixture.prompted().await;
    fixture.step(tool_call("t1", "in_progress"));
    fixture.update("tool_call").await;

    tokio::time::advance(Duration::from_secs(30 * 60)).await;
    fixture.step(tool_done("t1"));
    fixture.update("tool_call_update").await;

    tokio::time::advance(Duration::from_secs(9 * 60)).await;
    assert!(
        !turn.is_finished(),
        "the clock starts at the last tool's completion"
    );
    // Nothing more. The deadline measured from that completion is what ends it.
    let error = turn.await.unwrap().unwrap_err();
    assert_eq!(
        error,
        "No output from quiet-acp for 10 minutes. Send a message to try again."
    );
}

#[tokio::test(start_paused = true)]
async fn a_zero_timeout_turns_the_deadline_off() {
    let mut fixture = Fixture::start(None).await;
    let turn = fixture.run("run the tests");
    fixture.prompted().await;
    tokio::time::advance(Duration::from_secs(3 * 60 * 60)).await;
    assert!(
        !turn.is_finished(),
        "with the deadline off a silent turn keeps waiting"
    );
    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));
}

#[tokio::test(start_paused = true)]
async fn the_next_message_starts_a_new_harness_and_keeps_the_conversation() {
    let mut fixture = Fixture::start(Some(TEN_MINUTES)).await;
    let turn = fixture.run("first question");
    fixture.prompted().await;
    assert!(turn.await.unwrap().is_err(), "the silent turn failed");
    assert!(!fixture.session.has_adapter().await);

    // The engine starts a new harness for the next message, resuming the
    // session it recorded (in a test, an already attached fake one).
    fixture.restart().await;
    let turn = fixture.run("second question");
    fixture.prompted().await;
    fixture.step(Step::Update(json!({
        "sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": "Back."},
    })));
    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));

    let history = fixture.session.history();
    assert_eq!(
        history.first().map(|message| message.content.as_str()),
        Some("first question"),
        "the conversation is intact"
    );
    assert!(
        history.iter().any(|message| message.content == "Back."),
        "the new harness answered into the same thread: {history:?}"
    );
    assert_eq!(
        fixture.session.record().native_session_id.as_deref(),
        Some("child-1")
    );
}

#[test]
fn a_deadline_counts_a_tool_only_until_it_finishes() {
    let progress = Progress::new();
    progress.begin_turn();
    assert!(!progress.blocked());
    progress.observe(&session_update(json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Build", "status": "in_progress",
    })));
    assert!(progress.blocked(), "the tool is running");
    progress.observe(&session_update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
    })));
    assert!(!progress.blocked(), "the tool finished");
    // An update without a status says nothing about the tool.
    progress.observe(&session_update(json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1",
        "content": [{"type": "text", "text": "output"}],
    })));
    assert!(!progress.blocked());
}

#[test]
fn a_new_turn_and_a_new_harness_forget_what_the_last_one_left() {
    let progress = Progress::new();
    progress.begin_turn();
    progress.observe(&session_update(json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Build", "status": "in_progress",
    })));
    assert!(progress.blocked());
    progress.begin_turn();
    assert!(!progress.blocked(), "a new turn starts clean");

    progress.activity(&super::super::background::Activity {
        state: "waiting",
        background: 1,
    });
    assert!(progress.blocked(), "a background command is work");
    progress.begin_turn();
    assert!(
        progress.blocked(),
        "background work outlives the turn that started it"
    );
    progress.reset();
    assert!(!progress.blocked(), "a new harness has no background work");
    progress.activity(&super::super::background::Activity {
        state: "idle",
        background: 0,
    });
    assert!(!progress.blocked());
}

#[test]
fn a_permission_holds_the_deadline_until_it_is_answered() {
    let progress = Progress::new();
    progress.begin_turn();
    let waiting = progress.waiting();
    assert!(progress.blocked());
    // A new turn and a replaced harness do not answer a card the user still has
    // open: only that request's own guard gives its count back.
    progress.begin_turn();
    assert!(progress.blocked());
    progress.reset();
    assert!(progress.blocked());
    drop(waiting);
    assert!(!progress.blocked());
}

fn session_update(value: Value) -> SessionUpdate {
    serde_json::from_value(value).expect("a known update")
}
