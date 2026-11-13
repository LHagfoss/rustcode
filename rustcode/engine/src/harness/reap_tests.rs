//! The idle reaper (`session.rs`) against a fake clock — paused time — and a
//! fake harness process the test drives step by step.
//!
//! Each thread here has the silence deadline off, so only the reaper's own
//! timer moves: an hour of "nothing" is silence the watchdog ignores and the
//! reaper counts.

use super::*;
use crate::harness::HarnessKind;
use crate::harness::pressure::Pressure;
use crate::harness::rpc::{Incoming, RpcError, RpcPeer};
use crate::harness::session::test_support::{harness, pipe};
use crate::harness::store::HarnessRecord;
use agent_client_protocol::schema::v1::{
    ContentBlock, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, StopReason, TextContent,
};
use serde_json::Value;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Its own id: a harness's learned capabilities live in one place per id.
const NAME: &str = "reap-acp";
const NATIVE: &str = "child-1";

/// What the test tells the fake agent to do next.
enum Step {
    /// Send one `session/update` to the client.
    Update(Value),
    /// Send a tool-call approval request and wait for the answer off to the side.
    AskPermission,
    /// Answer the running prompt.
    Finish,
}

/// The fake ACP agent: it answers the handshake at once and holds a prompt
/// until the test says `Finish`, so a turn stays running for as long as the
/// test wants.
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
                            peer.respond(id, Ok(json!({"sessionId": NATIVE})));
                        }
                        "session/prompt" => prompt = Some(id),
                        other => {
                            peer.respond(id, Err(RpcError::method_not_found(other)));
                        }
                    }
                }
                Incoming::Notification { .. } => {}
                Incoming::Barrier(done) => {
                    let _ = done.send(());
                }
            },
            Some(step) = steps.recv() => match step {
                Step::Update(update) => peer.notify(
                    "session/update",
                    json!({"sessionId": NATIVE, "update": update}),
                ),
                Step::AskPermission => {
                    let peer = Arc::clone(&peer);
                    // Asked off the side: the agent's loop keeps its turn open.
                    tokio::spawn(async move {
                        let _ = peer
                            .request(
                                "session/request_permission",
                                json!({
                                    "sessionId": NATIVE,
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

/// One thread on the fake harness, with a running fake process.
struct Fixture {
    id: String,
    steps: mpsc::UnboundedSender<Step>,
    seen: mpsc::UnboundedReceiver<String>,
    updates: mpsc::UnboundedReceiver<SessionUpdate>,
    asks: mpsc::UnboundedReceiver<oneshot::Sender<RequestPermissionResponse>>,
    session: Arc<ExternalSession>,
}

impl Fixture {
    /// A thread whose record names a harness session to resume, a model and an
    /// effort, so a reap has something to keep.
    async fn start(id: &str) -> Self {
        let (updates, updates_rx) = mpsc::unbounded_channel();
        let (asks, asks_rx) = mpsc::unbounded_channel();
        let sink = Arc::new(HoldSink { updates, asks });
        let mut record = HarnessRecord::new(NAME, std::env::temp_dir());
        record.native_session_id = Some(NATIVE.to_owned());
        record.model = Some("model-x".to_owned());
        record.effort = Some("high".to_owned());
        let session = ExternalSession::new(
            id.to_owned(),
            harness(NAME, HarnessKind::Acp),
            record,
            Vec::new(),
            sink,
            false,
        );
        session.set_silence_timeout(None);
        let (steps, seen) = connect(&session).await;
        Self {
            id: id.to_owned(),
            steps,
            seen,
            updates: updates_rx,
            asks: asks_rx,
            session,
        }
    }

    fn run(&self, text: &str) -> JoinHandle<Result<StopReason, String>> {
        let session = Arc::clone(&self.session);
        let prompt = vec![ContentBlock::Text(TextContent::new(text.to_owned()))];
        tokio::spawn(async move { session.prompt(prompt).await })
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

    /// Wait until the client got the update of `kind`.
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

    /// Wait until `count` requests wait on the user.
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

    fn sessions(&self) -> Vec<Arc<ExternalSession>> {
        vec![Arc::clone(&self.session)]
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

fn allowed() -> RequestPermissionResponse {
    RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new("allow"),
    ))
}

#[tokio::test(start_paused = true)]
async fn an_idle_process_is_stopped_after_the_timeout() {
    let fixture = Fixture::start("idle-thread").await;
    let session = Arc::clone(&fixture.session);
    let timeout = Some(DEFAULT_IDLE_TIMEOUT);

    // 29 minutes of nothing is not enough.
    tokio::time::advance(DEFAULT_IDLE_TIMEOUT - Duration::from_secs(60)).await;
    assert!(
        reap_once(&fixture.sessions(), timeout, None, Pressure::Unknown).is_empty(),
        "the process is still inside its idle timeout"
    );
    assert!(session.has_adapter().await, "still running");
    assert!(!session.sleeping(), "not asleep yet");

    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(
        reap_once(&fixture.sessions(), timeout, None, Pressure::Unknown),
        vec![fixture.id.clone()]
    );
    assert!(!session.has_adapter().await, "the process is gone");
    assert!(session.sleeping(), "asleep, which is not crashed");

    // Nothing was lost: the record still resumes the harness's own session.
    let record = session.record();
    assert_eq!(record.native_session_id.as_deref(), Some(NATIVE));
    assert_eq!(record.model.as_deref(), Some("model-x"));
    assert_eq!(record.effort.as_deref(), Some("high"));
}

#[tokio::test(start_paused = true)]
async fn a_running_turn_is_never_reaped() {
    let mut fixture = Fixture::start("busy-thread").await;
    let turn = fixture.run("write the notes");
    fixture.prompted().await;

    tokio::time::advance(Duration::from_secs(3 * 60 * 60)).await;
    assert!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Unknown
        )
        .is_empty(),
        "a turn that runs for hours is not idle"
    );
    assert!(fixture.session.has_adapter().await);
    assert!(!turn.is_finished());

    // Once it ends, the thread is idle again.
    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));
    tokio::time::advance(DEFAULT_IDLE_TIMEOUT + Duration::from_secs(60)).await;
    assert_eq!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Unknown
        ),
        vec![fixture.id.clone()]
    );
}

#[tokio::test(start_paused = true)]
async fn a_thread_waiting_on_the_user_is_never_reaped() {
    let mut fixture = Fixture::start("asking-thread").await;
    let turn = fixture.run("delete the build directory");
    fixture.prompted().await;
    fixture.step(Step::Update(json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run the tests",
        "kind": "execute", "status": "pending",
    })));
    fixture.update("tool_call").await;
    fixture.step(Step::AskPermission);
    let answer = fixture.asks.recv().await.expect("the user is asked");

    tokio::time::advance(Duration::from_secs(2 * 60 * 60)).await;
    assert!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Unknown
        )
        .is_empty(),
        "a card waiting on the user keeps the process"
    );
    assert!(fixture.session.has_adapter().await);
    assert!(!turn.is_finished());

    // Answering it and finishing the turn makes the thread idle again.
    answer.send(allowed()).unwrap();
    fixture.waiting_requests(0).await;
    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));
    tokio::time::advance(DEFAULT_IDLE_TIMEOUT + Duration::from_secs(60)).await;
    assert_eq!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Unknown
        ),
        vec![fixture.id.clone()]
    );
}

#[tokio::test(start_paused = true)]
async fn an_update_from_the_harness_renews_the_timer() {
    let mut fixture = Fixture::start("active-thread").await;
    tokio::time::advance(Duration::from_secs(25 * 60)).await;
    assert!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Unknown
        )
        .is_empty()
    );

    // A thought is activity like anything else.
    fixture.step(Step::Update(json!({
        "sessionUpdate": "agent_thought_chunk",
        "content": {"type": "text", "text": "Still here."},
    })));
    fixture.update("agent_thought_chunk").await;

    tokio::time::advance(Duration::from_secs(10 * 60)).await;
    assert!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Unknown
        )
        .is_empty(),
        "10 minutes since the thought, not 35"
    );

    tokio::time::advance(Duration::from_secs(25 * 60)).await;
    assert_eq!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Unknown
        ),
        vec![fixture.id.clone()]
    );
}

#[tokio::test(start_paused = true)]
async fn a_disabled_reaper_keeps_every_process() {
    let fixture = Fixture::start("kept-thread").await;
    tokio::time::advance(Duration::from_secs(24 * 60 * 60)).await;
    assert!(
        reap_once(&fixture.sessions(), None, None, Pressure::Unknown).is_empty(),
        "a timeout of 0 and no cap stop nothing"
    );
    assert!(fixture.session.has_adapter().await);
}

#[tokio::test(start_paused = true)]
async fn the_least_recently_used_processes_are_stopped_above_the_cap_under_pressure() {
    let first = Fixture::start("first-thread").await;
    tokio::time::advance(Duration::from_secs(60)).await;
    let second = Fixture::start("second-thread").await;
    tokio::time::advance(Duration::from_secs(60)).await;
    let third = Fixture::start("third-thread").await;
    let sessions = [
        first.session.clone(),
        second.session.clone(),
        third.session.clone(),
    ];

    // Well inside the idle timeout, but three processes against a cap of two
    // and a machine short of memory.
    assert_eq!(
        reap_once(
            &sessions,
            Some(DEFAULT_IDLE_TIMEOUT),
            Some(2),
            Pressure::High
        ),
        vec![first.id.clone()],
        "the least recently used one goes first"
    );
    assert!(first.session.sleeping());
    assert!(!second.session.sleeping());
    assert!(!third.session.sleeping());

    // At the cap, nothing else goes.
    assert!(
        reap_once(
            &sessions,
            Some(DEFAULT_IDLE_TIMEOUT),
            Some(2),
            Pressure::High
        )
        .is_empty()
    );
    assert!(second.session.has_adapter().await);
    assert!(third.session.has_adapter().await);

    // A lower cap takes the least recently used of what runs.
    assert_eq!(
        reap_once(
            &sessions,
            Some(DEFAULT_IDLE_TIMEOUT),
            Some(1),
            Pressure::High
        ),
        vec![second.id.clone()]
    );
    assert!(third.session.has_adapter().await, "the newest one stays");
}

#[tokio::test(start_paused = true)]
async fn the_cap_alone_spares_a_recently_used_process_while_memory_is_plentiful() {
    let first = Fixture::start("recent-first").await;
    tokio::time::advance(Duration::from_secs(60)).await;
    let second = Fixture::start("recent-second").await;
    let sessions = [first.session.clone(), second.session.clone()];

    // Eight minutes of idle time and one process over the cap: the case the
    // reaper used to evict, losing the session's state and prompt cache.
    tokio::time::advance(Duration::from_secs(8 * 60)).await;
    assert!(
        reap_once(
            &sessions,
            Some(DEFAULT_IDLE_TIMEOUT),
            Some(1),
            Pressure::Low
        )
        .is_empty(),
        "under 30 minutes idle with memory to spare, the cap leaves it running"
    );
    assert!(first.session.has_adapter().await);
    assert!(second.session.has_adapter().await);

    // The same pass with no reading of memory keeps the old cap.
    assert_eq!(
        reap_once(
            &sessions,
            Some(DEFAULT_IDLE_TIMEOUT),
            Some(1),
            Pressure::Unknown
        ),
        vec![first.id.clone()]
    );
}

#[tokio::test(start_paused = true)]
async fn the_cap_stops_the_least_recently_used_once_idle_for_half_an_hour() {
    let first = Fixture::start("old-first").await;
    tokio::time::advance(Duration::from_secs(60)).await;
    let second = Fixture::start("old-second").await;
    let sessions = [first.session.clone(), second.session.clone()];

    // No idle timeout (0), so only the cap acts; memory is plentiful.
    tokio::time::advance(CAP_MIN_IDLE + Duration::from_secs(120)).await;
    assert_eq!(
        reap_once(&sessions, None, Some(1), Pressure::Low),
        vec![first.id.clone()],
        "idle for 30 minutes, the oldest goes down to the cap"
    );
    assert!(
        second.session.has_adapter().await,
        "at the cap, the other stays"
    );
}

#[tokio::test(start_paused = true)]
async fn a_process_with_a_tool_call_left_open_is_still_reaped() {
    let mut fixture = Fixture::start("open-tool-thread").await;
    let turn = fixture.run("start the build");
    fixture.prompted().await;
    fixture.step(Step::Update(json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run the build",
        "kind": "execute", "status": "in_progress",
    })));
    fixture.update("tool_call").await;
    fixture.step(Step::Finish);
    assert_eq!(turn.await.unwrap(), Ok(StopReason::EndTurn));
    assert_eq!(
        fixture.session.progress.open_tools(),
        1,
        "the finished turn left a call open: work a stop would cut off"
    );

    tokio::time::advance(DEFAULT_IDLE_TIMEOUT + Duration::from_secs(60)).await;
    assert_eq!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Low
        ),
        vec![fixture.id.clone()]
    );
    assert_eq!(fixture.session.progress.open_tools(), 0);
}

#[test]
fn the_eviction_choice() {
    let minutes = |n: u64| Duration::from_secs(n * 60);
    let timeout = Some(minutes(30));
    let cap = Some(2);
    let pick = eviction;

    // The idle timeout always applies, whatever the memory.
    assert_eq!(
        pick(minutes(31), 1, timeout, cap, Pressure::Low),
        Some(Eviction::Idle)
    );
    // Under the cap, nothing else stops a process.
    assert_eq!(pick(minutes(29), 2, timeout, cap, Pressure::High), None);
    // Over the cap, memory plentiful: only a half-hour idle process goes.
    assert_eq!(pick(minutes(8), 3, timeout, cap, Pressure::Low), None);
    assert_eq!(
        pick(minutes(30), 3, None, cap, Pressure::Low),
        Some(Eviction::Cap)
    );
    // Over the cap and short of memory: at once, as the memory.
    assert_eq!(
        pick(minutes(8), 3, timeout, cap, Pressure::High),
        Some(Eviction::Memory)
    );
    // No reading of memory: the old cap.
    assert_eq!(
        pick(minutes(8), 3, timeout, cap, Pressure::Unknown),
        Some(Eviction::Cap)
    );
    // Both turned off: nothing stops.
    assert_eq!(pick(minutes(600), 50, None, None, Pressure::High), None);
}

#[tokio::test(start_paused = true)]
async fn a_reaped_thread_starts_again_and_resumes_its_recorded_session() {
    let fixture = Fixture::start("resumed-thread").await;
    let session = Arc::clone(&fixture.session);
    tokio::time::advance(DEFAULT_IDLE_TIMEOUT + Duration::from_secs(60)).await;
    assert_eq!(
        reap_once(
            &fixture.sessions(),
            Some(DEFAULT_IDLE_TIMEOUT),
            None,
            Pressure::Unknown
        ),
        vec![fixture.id.clone()]
    );

    // The next start resumes the harness's own session, with the account, model
    // and effort the thread ran with.
    let options = session
        .resume_options(&session.record())
        .expect("the recorded start needs no account of its own");
    assert_eq!(options.resume.as_deref(), Some(NATIVE));
    assert_eq!(options.model.as_deref(), Some("model-x"));
    assert_eq!(options.effort.as_deref(), Some("high"));

    // And the next prompt starts a new process rather than reporting a reaped
    // or closed thread. The fake harness has no program on PATH, so the start
    // itself is what fails here.
    let prompt = vec![ContentBlock::Text(TextContent::new("again".to_owned()))];
    assert_eq!(
        session.prompt(prompt).await.unwrap_err(),
        format!("{NAME} not found on PATH")
    );
}
