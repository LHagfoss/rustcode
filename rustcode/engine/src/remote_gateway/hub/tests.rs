//! The registry and router on their own: scripted owners on one side,
//! device queues on the other, no sockets. Nothing here sleeps; time-driven
//! behaviour is reached with short limits and explicit [`SessionHub::tick`].

use super::*;
use crate::remote::{PromptDisposition, RemoteAttention, RemoteOmitted, SessionActivity};
use crate::remote_gateway::router::FrameQueue;
use serde_json::{Value, json};

const GATEWAY: &str = "gw-test";
const INSTANCE: &str = "instance-1";

fn limits() -> HubLimits {
    HubLimits {
        // Ticks are explicit in these tests.
        tick: Duration::from_secs(3600),
        ..HubLimits::default()
    }
}

fn hub(limits: HubLimits) -> Arc<SessionHub> {
    let hub = SessionHub::start(limits);
    hub.bind_identity(HubIdentity {
        gateway_id: GATEWAY.to_owned(),
        instance_id: INSTANCE.to_owned(),
        advertised_address: "192.168.1.20:17879".to_owned(),
        loopback_only: false,
    });
    hub
}

fn info(session_id: &str, epoch: u64) -> RemoteSessionInfo {
    RemoteSessionInfo {
        session_id: session_id.to_owned(),
        registration_epoch: epoch,
        title: format!("title of {session_id}"),
        workspace: None,
        model: "model".to_owned(),
        turn_count: None,
        activity: SessionActivity::Idle,
        attention: RemoteAttention {
            approval: false,
            question: false,
        },
        health: OwnerHealth::Live,
    }
}

fn snapshot(session_id: &str, epoch: u64, sequence: u64) -> Box<RemoteSnapshot> {
    Box::new(RemoteSnapshot {
        settings: None,
        snapshot_id: None,
        session: info(session_id, epoch),
        sequence,
        generation: 0,
        turn: None,
        transcript: Vec::new(),
        history_revision: "rev".to_owned(),
        history_cursor: None,
        pending_question: None,
        pending_approval: None,
        pending_prompts: Vec::new(),
        subagents: Vec::new(),
        background_tasks: Vec::new(),
        omitted: RemoteOmitted::default(),
        last_turn: None,
    })
}

/// A scripted session owner: what the owner socket would feed the hub.
struct Owner {
    hub: Arc<SessionHub>,
    id: u64,
    session_id: String,
    epoch: u64,
    sequence: u64,
    frames: mpsc::Receiver<GatewayFrame>,
}

impl Owner {
    fn register(hub: &Arc<SessionHub>, session_id: &str, epoch: u64) -> Self {
        Self::register_at(hub, session_id, epoch, 0)
    }

    fn register_at(hub: &Arc<SessionHub>, session_id: &str, epoch: u64, sequence: u64) -> Self {
        let registration = hub
            .register_owner(
                SessionRegistration {
                    session_id: session_id.to_owned(),
                    registration_epoch: epoch,
                },
                snapshot(session_id, epoch, sequence),
            )
            .expect("the registration is accepted");
        assert!(matches!(
            registration.registered,
            GatewayFrame::Registered { .. }
        ));
        Self {
            hub: Arc::clone(hub),
            id: registration.owner_id,
            session_id: session_id.to_owned(),
            epoch,
            sequence,
            frames: registration.frames,
        }
    }

    fn frame(&self, sequence: u64, text: &str) -> OwnerFrame {
        OwnerFrame::Event {
            frame: RemoteEventFrame {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                session_id: self.session_id.clone(),
                registration_epoch: self.epoch,
                sequence,
                generation: 0,
                event: RemoteEvent::TextDelta {
                    text: text.to_owned(),
                    timing: None,
                    thought_time_ms: None,
                    thought_tokens: None,
                    thought_tokens_estimated: None,
                },
            },
        }
    }

    /// Publish the next event.
    fn text(&mut self, text: &str) {
        self.sequence += 1;
        assert!(
            self.hub
                .owner_frame(self.id, self.frame(self.sequence, text))
        );
    }

    /// Produce an event that never reaches the gateway.
    fn lose(&mut self) {
        self.sequence += 1;
    }

    fn snapshot(&self, resync: Option<ResyncReason>) {
        assert!(self.hub.owner_frame(
            self.id,
            OwnerFrame::Snapshot {
                snapshot: snapshot(&self.session_id, self.epoch, self.sequence),
                resync,
            },
        ));
    }

    fn next(&mut self) -> Option<GatewayFrame> {
        self.frames.try_recv().ok()
    }

    /// The next request the gateway forwarded, skipping status frames.
    fn request(&mut self) -> (u64, String, RemoteRequest) {
        loop {
            match self.next().expect("a request was forwarded") {
                GatewayFrame::Request {
                    id,
                    device_id,
                    request,
                } => return (id, device_id, request),
                GatewayFrame::Status { .. } | GatewayFrame::Pong => {}
                other => panic!("expected a request, got {other:?}"),
            }
        }
    }

    fn reply(&self, id: u64, response: RemoteResponse) {
        assert!(
            self.hub
                .owner_frame(self.id, OwnerFrame::Reply { id, response })
        );
    }
}

struct Device {
    hub: Arc<SessionHub>,
    context: DeviceContext,
    sink: FrameSink,
    queue: FrameQueue,
    requests: u64,
}

impl Device {
    fn connect(hub: &Arc<SessionHub>, connection_id: u64, name: &str) -> Self {
        Self::connect_with(hub, connection_id, name, 64)
    }

    fn connect_with(hub: &Arc<SessionHub>, connection_id: u64, name: &str, queue: usize) -> Self {
        let (sink, queue) = FrameSink::detached(queue);
        let context = DeviceContext {
            connection_id,
            device_id: format!("device-{name}"),
            device_name: name.to_owned(),
        };
        hub.connected(&context, &sink);
        Self {
            hub: Arc::clone(hub),
            context,
            sink,
            queue,
            requests: 0,
        }
    }

    /// Send one request and return its `request_id`.
    fn send(&mut self, session: Option<(&str, u64)>, operation: Value) -> String {
        self.requests += 1;
        let request_id = format!("{}-{}", self.context.device_name, self.requests);
        self.send_as(&request_id, session, operation);
        request_id
    }

    fn send_as(&self, request_id: &str, session: Option<(&str, u64)>, operation: Value) {
        let mut frame = json!({
            "protocol_version": 1,
            "request_id": request_id,
            "operation": operation,
        });
        if let Some((session_id, epoch)) = session {
            frame["session_id"] = json!(session_id);
            frame["registration_epoch"] = json!(epoch);
        }
        self.hub
            .frame(&self.context, &frame.to_string(), &self.sink);
    }

    fn next(&mut self) -> Option<Value> {
        let frame = self.queue.try_recv().ok()?;
        assert!(
            frame.len() <= MAX_REMOTE_FRAME_BYTES,
            "a frame of {} bytes was queued",
            frame.len()
        );
        Some(serde_json::from_str(&frame).expect("frames are JSON"))
    }

    fn expect(&mut self) -> Value {
        self.next().expect("a frame is queued")
    }

    fn drain(&mut self) -> Vec<Value> {
        std::iter::from_fn(|| self.next()).collect()
    }

    /// The response to `request_id`, which must be the next frame.
    fn response(&mut self, request_id: &str) -> Value {
        let frame = self.expect();
        assert_eq!(frame["kind"], "response", "{frame}");
        assert_eq!(frame["request_id"], request_id, "{frame}");
        frame
    }

    fn attach(&mut self, session_id: &str, epoch: u64) -> Value {
        let id = self.send(Some((session_id, epoch)), json!({"type": "attach_session"}));
        self.response(&id)
    }
}

/// `(sequence, text)` of the text deltas in `frames`.
fn deltas(frames: &[Value]) -> Vec<(u64, String)> {
    frames
        .iter()
        .filter(|frame| frame["event"]["type"] == "text_delta")
        .map(|frame| {
            (
                frame["sequence"].as_u64().unwrap(),
                frame["event"]["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn accepted(request_id: &str) -> RemoteResponse {
    RemoteResponse::new(
        request_id,
        RemoteResult::PromptAccepted {
            disposition: PromptDisposition::Started,
        },
    )
    .with_receipt(ReceiptState::Applied)
}

#[tokio::test]
async fn two_shared_sessions_are_listed_and_commands_reach_their_own_owner() {
    let hub = hub(limits());
    let mut first = Owner::register(&hub, "session-a", 11);
    let mut second = Owner::register(&hub, "session-b", 22);
    // A third terminal is running but never ran `/remote`: nothing of it
    // exists here.
    let mut phone = Device::connect(&hub, 1, "phone");

    let id = phone.send(None, json!({"type": "list_sessions"}));
    let listed = phone.response(&id);
    assert_eq!(listed["result"]["type"], "sessions");
    assert_eq!(listed["result"]["gateway_id"], GATEWAY);
    assert_eq!(listed["result"]["instance_id"], INSTANCE);
    assert_eq!(listed["result"]["subscribed"], false);
    let sessions: Vec<(&str, u64)> = listed["result"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["session_id"].as_str().unwrap(),
                s["registration_epoch"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(sessions, [("session-a", 11), ("session-b", 22)]);

    let to_b = phone.send(
        Some(("session-b", 22)),
        json!({"type": "submit_prompt", "prompt": "for b"}),
    );
    let to_a = phone.send(
        Some(("session-a", 11)),
        json!({"type": "submit_prompt", "prompt": "for a"}),
    );
    let (id_b, device, request_b) = second.request();
    assert_eq!(device, "device-phone");
    assert_eq!(
        request_b.operation,
        RemoteOperation::SubmitPrompt {
            prompt: "for b".to_owned()
        }
    );
    let (id_a, _, request_a) = first.request();
    assert_eq!(
        request_a.operation,
        RemoteOperation::SubmitPrompt {
            prompt: "for a".to_owned()
        }
    );
    assert!(first.next().is_none() && second.next().is_none());

    // Each owner's answer goes back under the request that asked.
    first.reply(id_a, accepted(&to_a));
    let answer = phone.response(&to_a);
    assert_eq!(answer["receipt"], "applied");
    second.reply(
        id_b,
        RemoteResponse::error(
            &to_b,
            RemoteError::new(RemoteErrorCode::Busy, "the session is running a turn"),
        )
        .with_receipt(ReceiptState::Rejected),
    );
    let answer = phone.response(&to_b);
    assert_eq!(answer["receipt"], "rejected");
    assert_eq!(answer["result"]["code"], "busy");

    // The private session cannot be reached by guessing its identifier.
    let id = phone.send(
        Some(("session-private", 1)),
        json!({"type": "submit_prompt", "prompt": "hello"}),
    );
    let refused = phone.response(&id);
    assert_eq!(refused["result"]["code"], "not_found");
    assert_eq!(refused["receipt"], "rejected");
    let id = phone.send(
        Some(("session-private", 1)),
        json!({"type": "attach_session"}),
    );
    assert_eq!(phone.response(&id)["result"]["code"], "not_found");
}

#[tokio::test]
async fn a_second_live_owner_for_one_session_is_refused() {
    let hub = hub(limits());
    let first = Owner::register(&hub, "session-a", 1);
    let refused = hub
        .register_owner(
            SessionRegistration {
                session_id: "session-a".to_owned(),
                registration_epoch: 2,
            },
            snapshot("session-a", 2, 0),
        )
        .err()
        .expect("the first owner keeps the session");
    assert!(refused.contains("already sharing"), "{refused}");
    // The first registration is untouched.
    assert_eq!(hub.summary().len(), 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    assert_eq!(phone.attach("session-a", 1)["result"]["type"], "attached");

    // Once it is gone the session can be shared again.
    hub.owner_gone(first.id);
    Owner::register(&hub, "session-a", 2);
}

#[tokio::test]
async fn an_owner_cannot_publish_under_another_identity() {
    let hub = hub(limits());
    let owner = Owner::register(&hub, "session-a", 1);
    let _other = Owner::register(&hub, "session-b", 1);
    // The row an owner sends is pinned to its own registration.
    assert!(hub.owner_frame(
        owner.id,
        OwnerFrame::SessionInfo {
            info: RemoteSessionInfo {
                title: "renamed".to_owned(),
                ..info("session-b", 99)
            },
        },
    ));
    let mut phone = Device::connect(&hub, 1, "phone");
    let id = phone.send(None, json!({"type": "list_sessions"}));
    let listed = phone.response(&id);
    let sessions = listed["result"]["sessions"].as_array().unwrap();
    assert_eq!(sessions[0]["session_id"], "session-a");
    assert_eq!(sessions[0]["registration_epoch"], 1);
    assert_eq!(sessions[0]["title"], "renamed");
    assert_eq!(sessions[1]["title"], "title of session-b");
}

#[tokio::test]
async fn attaching_is_one_cut_of_snapshot_and_later_events() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    owner.text("one ");
    owner.text("two ");

    let mut phone = Device::connect(&hub, 1, "phone");
    let attached = phone.attach("session-a", 1);
    assert_eq!(attached["result"]["type"], "attached");
    assert_eq!(attached["result"]["instance_id"], INSTANCE);
    assert!(attached["result"].get("resync").is_none());
    // The snapshot the gateway holds is at 0; what followed it is replayed
    // once, in order, and the owner is not asked for anything.
    assert_eq!(attached["result"]["snapshot"]["sequence"], 0);
    owner.text("three");
    assert_eq!(
        deltas(&phone.drain()),
        [
            (1, "one ".to_owned()),
            (2, "two ".to_owned()),
            (3, "three".to_owned())
        ]
    );
    assert!(owner.next().is_none());
}

#[tokio::test]
async fn a_snapshot_that_races_a_delta_loses_and_repeats_nothing() {
    let hub = hub(HubLimits {
        // Room for about one event: the cached snapshot goes stale at once.
        replay_bytes: 200,
        ..limits()
    });
    let mut owner = Owner::register(&hub, "session-a", 1);
    owner.text("alpha ");
    owner.text("beta ");
    owner.text("gamma ");

    let mut phone = Device::connect(&hub, 1, "phone");
    let id = phone.send(Some(("session-a", 1)), json!({"type": "attach_session"}));
    // The gateway cannot build the state itself: it asks the owner.
    assert!(phone.next().is_none());
    assert_eq!(owner.next(), Some(GatewayFrame::SnapshotRequested));

    // A delta is published while the snapshot is on its way. The snapshot
    // the owner then sends contains it.
    owner.text("delta ");
    assert!(phone.next().is_none(), "nothing precedes the snapshot");
    owner.snapshot(None);
    let attached = phone.response(&id);
    assert_eq!(attached["result"]["snapshot"]["sequence"], 4);
    owner.text("epsilon");
    // Only what follows the watermark is delivered: the racing delta is in
    // the snapshot and not repeated; nothing after it is missing.
    assert_eq!(deltas(&phone.drain()), [(5, "epsilon".to_owned())]);
}

#[tokio::test]
async fn a_cursor_resumes_only_a_contiguous_range_of_the_same_instance() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    for text in ["a", "b", "c", "d"] {
        owner.text(text);
    }
    let resume = |cursor: Value| json!({"type": "attach_session", "resume": cursor});

    // Applied up to 2 before the connection dropped.
    let mut phone = Device::connect(&hub, 1, "phone");
    let id = phone.send(
        Some(("session-a", 1)),
        resume(json!({"gateway_id": GATEWAY, "instance_id": INSTANCE, "last_sequence": 2})),
    );
    let resumed = phone.response(&id);
    assert_eq!(resumed["result"]["type"], "resumed");
    assert_eq!(resumed["result"]["next_sequence"], 3);
    owner.text("e");
    assert_eq!(
        deltas(&phone.drain()),
        [
            (3, "c".to_owned()),
            (4, "d".to_owned()),
            (5, "e".to_owned())
        ]
    );

    // A cursor without the instance is decided from the sequence alone.
    let mut tablet = Device::connect(&hub, 2, "tablet");
    let id = tablet.send(
        Some(("session-a", 1)),
        resume(json!({"gateway_id": GATEWAY, "last_sequence": 5})),
    );
    assert_eq!(tablet.response(&id)["result"]["type"], "resumed");
    assert!(tablet.next().is_none());

    // A cursor from another gateway instance, or another gateway, is never
    // replayed, even when the sequence would fit.
    for cursor in [
        json!({"gateway_id": GATEWAY, "instance_id": "instance-0", "last_sequence": 3}),
        json!({"gateway_id": "another-gateway", "last_sequence": 3}),
    ] {
        let mut device = Device::connect(&hub, 3, "old");
        let id = device.send(Some(("session-a", 1)), resume(cursor));
        let attached = device.response(&id);
        assert_eq!(attached["result"]["type"], "attached", "{attached}");
        assert_eq!(attached["result"]["resync"], "gateway_restarted");
        assert_eq!(attached["result"]["snapshot"]["sequence"], 0);
        assert_eq!(deltas(&device.drain()).len(), 5);
        hub.disconnected(&device.context);
    }

    // A cursor at or before the snapshot the gateway holds may predate it,
    // and one ahead of the owner is nonsense: both get a snapshot.
    for last_sequence in [0, 9] {
        let mut device = Device::connect(&hub, 4, "odd");
        let id = device.send(
            Some(("session-a", 1)),
            resume(json!({"gateway_id": GATEWAY, "instance_id": INSTANCE, "last_sequence": last_sequence})),
        );
        let attached = device.response(&id);
        assert_eq!(attached["result"]["type"], "attached");
        assert_eq!(attached["result"]["resync"], "sequence_gap");
        hub.disconnected(&device.context);
    }

    // Another registration epoch is another session as far as cursors go.
    let id = phone.send(
        Some(("session-a", 7)),
        resume(json!({"gateway_id": GATEWAY, "last_sequence": 2})),
    );
    assert_eq!(phone.response(&id)["result"]["code"], "stale_session");
}

#[tokio::test]
async fn replay_retains_terminal_turn_timing_and_outcome() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    owner.text("before reconnect");
    let timing: crate::remote::protocol::RemoteTurnTiming = serde_json::from_value(json!({
        "turn_id": "turn:replay", "started_at": "2026-10-09T18:41:00+02:00",
        "ended_at": "2026-10-09T18:42:00+02:00", "elapsed_work_ms": 32000, "outcome": "completed"
    }))
    .unwrap();
    assert!(hub.owner_frame(
        owner.id,
        OwnerFrame::Event {
            frame: RemoteEventFrame {
                protocol_version: 1,
                session_id: "session-a".to_owned(),
                registration_epoch: 1,
                sequence: 2,
                generation: 0,
                event: RemoteEvent::TurnFinished {
                    turn_id: Some(timing.turn_id.clone()),
                    timing: Some(timing.clone()),
                }
            }
        }
    ));
    let mut phone = Device::connect(&hub, 1, "reconnected-phone");
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "attach_session",
        "resume": {"gateway_id": GATEWAY, "instance_id": INSTANCE, "last_sequence": 1}}),
    );
    assert_eq!(phone.response(&id)["result"]["type"], "resumed");
    let frames = phone.drain();
    let replayed = frames
        .iter()
        .find(|frame| frame["event"]["type"] == "turn_finished")
        .unwrap();
    assert_eq!(replayed["sequence"], 2);
    assert_eq!(
        replayed["event"]["timing"],
        serde_json::to_value(timing).unwrap()
    );
}

#[tokio::test]
async fn current_snapshot_cursor_resumes_and_rejects_a_replaced_snapshot_at_the_same_sequence() {
    let hub = hub(limits());
    let owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    let attached = phone.attach("session-a", 1);
    let snapshot_id = attached["result"]["snapshot"]["snapshot_id"].clone();
    let cursor = json!({"gateway_id": GATEWAY, "instance_id": INSTANCE, "last_sequence": 0, "snapshot_id": snapshot_id});
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "attach_session", "resume": cursor}),
    );
    assert_eq!(phone.response(&id)["result"]["type"], "resumed");
    assert!(phone.next().is_none());
    owner.snapshot(None);
    phone.drain();
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "attach_session", "resume": cursor}),
    );
    let response = phone.response(&id);
    assert_eq!(response["result"]["type"], "attached");
    assert_eq!(response["result"]["resync"], "sequence_gap");
    assert_ne!(response["result"]["snapshot"]["snapshot_id"], snapshot_id);
}

#[tokio::test]
async fn a_cursor_behind_the_replay_ring_gets_a_fresh_snapshot() {
    let hub = hub(HubLimits {
        replay_bytes: 400,
        ..limits()
    });
    let mut owner = Owner::register(&hub, "session-a", 1);
    for index in 0..20 {
        owner.text(&format!("chunk {index} "));
    }
    let mut phone = Device::connect(&hub, 1, "phone");
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "attach_session", "resume": {"gateway_id": GATEWAY, "instance_id": INSTANCE, "last_sequence": 3}}),
    );
    assert!(phone.next().is_none());
    assert_eq!(owner.next(), Some(GatewayFrame::SnapshotRequested));
    owner.snapshot(None);
    let attached = phone.response(&id);
    assert_eq!(attached["result"]["type"], "attached");
    assert_eq!(attached["result"]["resync"], "lagged");
    assert_eq!(attached["result"]["snapshot"]["sequence"], 20);
    assert!(phone.next().is_none());
}

#[tokio::test]
async fn events_lost_before_the_gateway_force_a_resync_for_every_subscriber() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    phone.attach("session-a", 1);
    owner.text("kept ");
    assert_eq!(deltas(&phone.drain()), [(1, "kept ".to_owned())]);

    // The owner's queue overflowed: two events never left the terminal.
    owner.lose();
    owner.lose();
    owner.text("after the gap");
    // An event that cannot be applied is not forwarded.
    assert!(phone.next().is_none());
    // A device that attaches now cannot be given a state either.
    let mut tablet = Device::connect(&hub, 2, "tablet");
    let pending = tablet.send(Some(("session-a", 1)), json!({"type": "attach_session"}));
    assert!(tablet.next().is_none());

    owner.snapshot(Some(ResyncReason::Lagged));
    let frames = phone.drain();
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert_eq!(frames[0]["event"]["type"], "resync_required");
    assert_eq!(frames[0]["event"]["reason"], "lagged");
    assert_eq!(frames[1]["event"]["type"], "snapshot");
    assert_eq!(frames[1]["event"]["snapshot"]["sequence"], 4);
    assert_eq!(frames[1]["sequence"], 4);
    let attached = tablet.response(&pending);
    assert_eq!(attached["result"]["snapshot"]["sequence"], 4);

    owner.text("live again");
    assert_eq!(deltas(&phone.drain()), [(5, "live again".to_owned())]);
    assert_eq!(deltas(&tablet.drain()), [(5, "live again".to_owned())]);
}

#[tokio::test]
async fn a_change_no_event_describes_reaches_subscribers_as_a_snapshot() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    owner.text("x");
    let mut phone = Device::connect(&hub, 1, "phone");
    phone.attach("session-a", 1);
    phone.drain();

    owner.snapshot(None);
    let frames = phone.drain();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["event"]["type"], "snapshot");
    assert_eq!(frames[0]["sequence"], 1);
    // A cursor taken before that snapshot must not skip it.
    let mut tablet = Device::connect(&hub, 2, "tablet");
    let id = tablet.send(
        Some(("session-a", 1)),
        json!({"type": "attach_session", "resume": {"gateway_id": GATEWAY, "instance_id": INSTANCE, "last_sequence": 1}}),
    );
    assert_eq!(tablet.response(&id)["result"]["type"], "attached");
}

#[tokio::test]
async fn a_slow_device_is_held_back_without_stalling_the_owner_or_other_devices() {
    let hub = hub(HubLimits {
        replay_bytes: 4_000,
        reserved_slots: 2,
        ..limits()
    });
    let mut owner = Owner::register(&hub, "session-a", 1);
    // The slow device has room for four frames and reads nothing.
    let mut slow = Device::connect_with(&hub, 1, "slow", 4);
    let mut fast = Device::connect_with(&hub, 2, "fast", 512);
    slow.attach("session-a", 1);
    fast.attach("session-a", 1);

    for index in 0..200 {
        owner.text(&format!("delta {index} "));
        // The reader keeps up; every event is in its queue at once.
        assert_eq!(deltas(&fast.drain()).len(), 1, "event {index}");
    }
    // The slow device was never pushed over its queue: it is still
    // connected, holds a bounded backlog and kept room for a response.
    assert!(!slow.sink.is_closed());
    assert!(slow.sink.free_slots() >= 2);
    let id = slow.send(None, json!({"type": "list_sessions"}));
    let held: Vec<Value> = slow.drain();
    assert!(held.iter().any(|frame| frame["request_id"] == id.as_str()));
    let seen = deltas(&held);
    assert!(seen.len() <= 2, "{seen:?}");

    // Once it reads again it is told that it fell behind, and gets state
    // rather than a silent hole.
    hub.tick();
    assert_eq!(owner.next(), Some(GatewayFrame::SnapshotRequested));
    owner.snapshot(None);
    let frames = slow.drain();
    assert_eq!(frames[0]["event"]["type"], "resync_required", "{frames:?}");
    assert_eq!(frames[0]["event"]["reason"], "lagged");
    assert_eq!(frames[1]["event"]["type"], "snapshot");
    assert_eq!(frames[1]["event"]["snapshot"]["sequence"], 200);
    owner.text("next");
    assert_eq!(deltas(&slow.drain()), [(201, "next".to_owned())]);
}

#[tokio::test]
async fn a_device_that_never_reads_is_closed_rather_than_buffered() {
    let hub = hub(limits());
    let _owner = Owner::register(&hub, "session-a", 1);
    let stuck = Device::connect_with(&hub, 1, "stuck", 4);
    // Responses are not optional: a device that lets them pile up overflows
    // its own queue and is closed as a slow consumer.
    for index in 0..8 {
        stuck.send_as(
            &format!("r-{index}"),
            None,
            json!({"type": "list_sessions"}),
        );
    }
    assert!(stuck.sink.is_closed());
}

#[tokio::test]
async fn an_oversized_event_is_never_forwarded() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    phone.attach("session-a", 1);
    owner.text(&"x".repeat(MAX_REMOTE_FRAME_BYTES));
    assert!(phone.next().is_none());
    // It counts as lost: the owner is asked for state.
    assert_eq!(owner.next(), Some(GatewayFrame::SnapshotRequested));
    owner.snapshot(None);
    let frames = phone.drain();
    assert_eq!(frames[0]["event"]["type"], "resync_required");
    assert_eq!(frames[1]["event"]["type"], "snapshot");
}

#[tokio::test]
async fn reads_pass_through_to_the_owner_and_back() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "get_content", "content_id": "c-1", "offset": 0, "max_bytes": 1000}),
    );
    let (ipc, _, request) = owner.request();
    assert!(matches!(
        request.operation,
        RemoteOperation::GetContent { .. }
    ));
    owner.reply(
        ipc,
        RemoteResponse::new(
            // Whatever the owner echoes, the device gets its own ID back.
            "something-else",
            RemoteResult::Content(crate::remote::RemoteContentChunk {
                content_id: "c-1".to_owned(),
                offset: 0,
                text: "chunk".to_owned(),
                total_bytes: 5,
                next_offset: None,
            }),
        ),
    );
    let answer = phone.response(&id);
    assert_eq!(answer["result"]["type"], "content");
    assert!(answer.get("receipt").is_none());

    // An answer too large for a frame becomes an error, not a broken frame.
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "get_history", "limit": 50}),
    );
    let (ipc, _, _) = owner.request();
    owner.reply(
        ipc,
        RemoteResponse::new(
            &id,
            RemoteResult::Content(crate::remote::RemoteContentChunk {
                content_id: "c".to_owned(),
                offset: 0,
                text: "y".repeat(MAX_REMOTE_FRAME_BYTES),
                total_bytes: 0,
                next_offset: None,
            }),
        ),
    );
    assert_eq!(phone.response(&id)["result"]["code"], "internal");
}

#[tokio::test]
async fn every_reply_after_authentication_is_a_correlated_response() {
    let hub = hub(limits());
    let mut phone = Device::connect(&hub, 1, "phone");
    for (frame, request_id, code) in [
        ("not json".to_owned(), "", "invalid_request"),
        (
            json!({"protocol_version": 9, "request_id": "v", "operation": {"type": "x"}})
                .to_string(),
            "v",
            "incompatible_version",
        ),
        (
            json!({"protocol_version": 1, "request_id": "s", "operation": {"type": "run_slash_command", "command": "/exit"}})
                .to_string(),
            "s",
            "unsupported_operation",
        ),
        (
            json!({"protocol_version": 1, "request_id": "m", "operation": {"type": "cancel_turn", "turn_id": "t"}})
                .to_string(),
            "m",
            "invalid_request",
        ),
    ] {
        hub.frame(&phone.context, &frame, &phone.sink);
        let answer = phone.expect();
        assert_eq!(answer["kind"], "response", "{answer}");
        assert_eq!(answer["request_id"], request_id, "{answer}");
        assert_eq!(answer["result"]["code"], code, "{answer}");
    }
}

#[tokio::test]
async fn owner_exit_leaves_mutations_unknown_and_removes_the_session_at_once() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    let mut watcher = Device::connect(&hub, 2, "watcher");
    let id = watcher.send(None, json!({"type": "subscribe_sessions"}));
    assert_eq!(watcher.response(&id)["result"]["subscribed"], true);
    phone.attach("session-a", 1);

    let mutation = phone.send(
        Some(("session-a", 1)),
        json!({"type": "submit_prompt", "prompt": "did this run?"}),
    );
    let read = phone.send(
        Some(("session-a", 1)),
        json!({"type": "get_history", "limit": 10}),
    );
    owner.request();
    owner.request();
    // The terminal dies with both in its hands.
    hub.owner_gone(owner.id);

    let frames = phone.drain();
    let closed = frames
        .iter()
        .find(|frame| frame["event"]["type"] == "session_closed")
        .expect("subscribers are told");
    assert_eq!(closed["event"]["reason"], "owner_exited");
    let answer = |id: &str| {
        frames
            .iter()
            .find(|frame| frame["request_id"] == id)
            .unwrap_or_else(|| panic!("{id} was answered"))
    };
    // The prompt may or may not have run: the gateway does not guess and
    // nothing resends it.
    assert_eq!(answer(&mutation)["result"]["code"], "owner_unavailable");
    assert_eq!(answer(&mutation)["receipt"], "unknown");
    assert_eq!(answer(&read)["result"]["code"], "owner_unavailable");
    assert!(answer(&read).get("receipt").is_none());

    let pushed = watcher.expect();
    assert_eq!(pushed["kind"], "sessions");
    assert_eq!(pushed["sessions"], json!([]));

    // Asking later gives the same honest answer, with or without a session.
    for session in [Some(("session-a", 1)), None] {
        let id = phone.send(
            session,
            json!({"type": "get_request_status", "target_request_id": mutation}),
        );
        let status = phone.response(&id);
        assert_eq!(status["result"]["type"], "request_status");
        assert_eq!(status["result"]["receipt"], "unknown");
        assert!(status["result"].get("result").is_none());
    }
    // And the old registration is simply gone.
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "submit_prompt", "prompt": "again"}),
    );
    assert_eq!(phone.response(&id)["result"]["code"], "not_found");
}

#[tokio::test]
async fn an_owner_that_does_not_answer_in_time_leaves_the_mutation_unknown() {
    let hub = hub(HubLimits {
        request_timeout: Duration::ZERO,
        ..limits()
    });
    let mut owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "submit_prompt", "prompt": "slow"}),
    );
    let (ipc, _, _) = owner.request();
    hub.tick();
    let answer = phone.response(&id);
    assert_eq!(answer["result"]["code"], "owner_unavailable");
    assert_eq!(answer["receipt"], "unknown");
    // The late answer is not delivered a second time.
    owner.reply(ipc, accepted(&id));
    assert!(phone.next().is_none());
}

#[tokio::test]
async fn request_status_finds_the_owner_that_holds_the_receipt() {
    let hub = hub(limits());
    let mut first = Owner::register(&hub, "session-a", 1);
    let mut second = Owner::register(&hub, "session-b", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    let unknown = |request_id: &str| {
        RemoteResponse::new(
            request_id,
            RemoteResult::RequestStatus {
                target_request_id: "lost-1".to_owned(),
                receipt: ReceiptState::Unknown,
                result: None,
            },
        )
    };

    // No session on the envelope: every live owner is asked, under its own
    // registration, and the one that knows the request answers.
    let id = phone.send(
        None,
        json!({"type": "get_request_status", "target_request_id": "lost-1"}),
    );
    let (ipc_a, device, leg_a) = first.request();
    let (ipc_b, _, leg_b) = second.request();
    assert_eq!(device, "device-phone");
    assert_eq!(leg_a.session_id.as_deref(), Some("session-a"));
    assert_eq!(leg_b.session_id.as_deref(), Some("session-b"));
    first.reply(ipc_a, unknown(&id));
    assert!(
        phone.next().is_none(),
        "one owner not knowing is not an answer"
    );
    second.reply(
        ipc_b,
        RemoteResponse::new(
            &id,
            RemoteResult::RequestStatus {
                target_request_id: "lost-1".to_owned(),
                receipt: ReceiptState::Applied,
                result: Some(Box::new(accepted("lost-1").result)),
            },
        ),
    );
    let status = phone.response(&id);
    assert_eq!(status["result"]["receipt"], "applied");
    assert_eq!(status["result"]["result"]["type"], "prompt_accepted");

    // With a session, only that owner is asked; an ID nobody saw is unknown.
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "get_request_status", "target_request_id": "lost-1"}),
    );
    let (ipc_a, _, _) = first.request();
    assert!(second.next().is_none());
    first.reply(ipc_a, unknown(&id));
    let status = phone.response(&id);
    assert_eq!(status["result"]["receipt"], "unknown");
    assert_eq!(status["result"]["target_request_id"], "lost-1");
}

#[tokio::test]
async fn off_ends_the_registration_and_a_new_one_is_a_new_identity() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    phone.attach("session-a", 1);
    owner.text("before off");
    phone.drain();

    // `/remote off`. The owner has already rejected what it held; the
    // gateway drops the registration with the reason it was given.
    assert!(!hub.owner_frame(
        owner.id,
        OwnerFrame::Unregister {
            reason: SessionCloseReason::SharingDisabled,
        },
    ));
    let closed = phone.expect();
    assert_eq!(closed["event"]["type"], "session_closed");
    assert_eq!(closed["event"]["reason"], "sharing_disabled");
    assert!(hub.summary().is_empty());
    // A command that was on its way is refused: nothing routes to it.
    let id = phone.send(
        Some(("session-a", 1)),
        json!({"type": "submit_prompt", "prompt": "too late"}),
    );
    assert_eq!(phone.response(&id)["result"]["code"], "not_found");

    // Shared again, the session has a new epoch. The old subscription did
    // not survive and the old epoch is refused everywhere.
    let mut again = Owner::register(&hub, "session-a", 2);
    again.text("after re-share");
    assert!(phone.next().is_none(), "access is not inherited");
    for operation in [
        json!({"type": "submit_prompt", "prompt": "old epoch"}),
        json!({"type": "attach_session"}),
        json!({"type": "get_history", "limit": 5}),
    ] {
        let id = phone.send(Some(("session-a", 1)), operation);
        assert_eq!(phone.response(&id)["result"]["code"], "stale_session");
    }
    assert!(again.next().is_none());
}

#[tokio::test]
async fn a_disconnected_or_revoked_device_loses_its_subscriptions() {
    let hub = hub(limits());
    let mut owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    let mut tablet = Device::connect(&hub, 2, "tablet");
    phone.attach("session-a", 1);
    tablet.attach("session-a", 1);
    assert_eq!(hub.summary()[0].attached_devices, ["phone", "tablet"]);
    hub.tick();
    assert!(matches!(
        owner.next(),
        Some(GatewayFrame::Status { status }) if status.attached_devices == ["phone", "tablet"]
            && status.connected_devices == ["phone", "tablet"]
            && status.advertised_address == "192.168.1.20:17879"
    ));

    // Revocation closes the connection; the transport reports it here.
    hub.disconnected(&phone.context);
    owner.text("only the tablet sees this");
    assert!(phone.next().is_none());
    assert_eq!(deltas(&tablet.drain()).len(), 1);
    assert_eq!(hub.summary()[0].attached_devices, ["tablet"]);
    hub.tick();
    assert!(matches!(
        owner.next(),
        Some(GatewayFrame::Status { status }) if status.attached_devices == ["tablet"]
    ));

    // Detaching is the polite version of the same thing.
    let id = tablet.send(Some(("session-a", 1)), json!({"type": "detach_session"}));
    assert_eq!(tablet.response(&id)["result"]["type"], "detached");
    owner.text("nobody sees this");
    assert!(tablet.next().is_none());
}

#[tokio::test]
async fn a_silent_owner_is_listed_unresponsive_and_then_dropped() {
    let hub = hub(HubLimits {
        owner_unresponsive: Duration::ZERO,
        owner_timeout: Duration::from_secs(3600),
        ..limits()
    });
    let mut owner = Owner::register(&hub, "session-a", 1);
    let mut phone = Device::connect(&hub, 1, "phone");
    let id = phone.send(None, json!({"type": "subscribe_sessions"}));
    assert_eq!(
        phone.response(&id)["result"]["sessions"][0]["health"],
        "live"
    );
    hub.tick();
    assert_eq!(phone.expect()["sessions"][0]["health"], "unresponsive");
    // Any frame from the owner, a heartbeat included, makes it live again.
    assert!(hub.owner_frame(owner.id, OwnerFrame::Ping));
    assert_eq!(phone.expect()["sessions"][0]["health"], "live");
    assert!(std::iter::from_fn(|| owner.next()).any(|frame| frame == GatewayFrame::Pong));

    let dropping = self::hub(HubLimits {
        owner_timeout: Duration::ZERO,
        ..limits()
    });
    let mut silent = Owner::register(&dropping, "session-b", 1);
    dropping.tick();
    assert!(dropping.summary().is_empty());
    assert!(
        std::iter::from_fn(|| silent.next())
            .any(|frame| matches!(frame, GatewayFrame::Closed { .. }))
    );
}

#[tokio::test]
async fn limits_on_sessions_and_waiting_requests_are_enforced() {
    let hub = hub(HubLimits {
        max_sessions: 1,
        max_in_flight: 2,
        owner_queue: 64,
        ..limits()
    });
    let mut owner = Owner::register(&hub, "session-a", 1);
    assert!(
        hub.register_owner(
            SessionRegistration {
                session_id: "session-b".to_owned(),
                registration_epoch: 1,
            },
            snapshot("session-b", 1, 0),
        )
        .is_err()
    );
    let mut phone = Device::connect(&hub, 1, "phone");
    let prompt = |text: &str| json!({"type": "submit_prompt", "prompt": text});
    let first = phone.send(Some(("session-a", 1)), prompt("one"));
    phone.send(Some(("session-a", 1)), prompt("two"));
    let third = phone.send(Some(("session-a", 1)), prompt("three"));
    let refused = phone.response(&third);
    assert_eq!(refused["result"]["code"], "rate_limited");
    assert_eq!(refused["receipt"], "rejected");
    // An answer frees its slot.
    let (ipc, _, _) = owner.request();
    owner.reply(ipc, accepted(&first));
    phone.response(&first);
    let fourth = phone.send(Some(("session-a", 1)), prompt("four"));
    assert!(phone.next().is_none(), "{fourth} was forwarded");
}
