//! The ACP adapter against a fake ACP agent in process.

use super::*;
use crate::harness::HarnessKind;
use crate::harness::session::test_support::{drain, harness, kinds, pipe, thread};
use crate::harness::session::{Adapter, StartOptions};
use agent_client_protocol::schema::v1::TextContent;

fn options(effort: &str) -> Value {
    json!([
        {"id": "model", "name": "Model", "category": "model", "type": "select",
         "currentValue": "sonnet",
         "options": [{"value": "sonnet", "name": "Sonnet"}, {"value": "opus", "name": "Opus"}]},
        {"id": "thinking", "name": "Thinking", "category": "thought_level", "type": "select",
         "currentValue": effort,
         "options": [{"value": "low", "name": "Low"}, {"value": "medium", "name": "Medium"},
                     {"value": "high", "name": "High"}]},
        {"id": "mode", "name": "Mode", "type": "select", "currentValue": "default",
         "options": [{"value": "default", "name": "Default"}]}
    ])
}

/// A fake ACP agent. Every request and notification it gets is reported on
/// `seen`; `stubborn` makes it ignore `session/cancel`.
async fn fake_agent(
    peer: Arc<RpcPeer>,
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    seen: mpsc::UnboundedSender<(String, Value)>,
) {
    let update = |session: &str, update: Value| json!({"sessionId": session, "update": update});
    let mut cancelled = Vec::new();
    while let Some(message) = incoming.recv().await {
        let (id, method, params) = match message {
            Incoming::Request { id, method, params } => (Some(id), method, params),
            Incoming::Notification { method, params } => (None, method, params),
            Incoming::Barrier(done) => {
                let _ = done.send(());
                continue;
            }
        };
        let _ = seen.send((method.clone(), params.clone()));
        let Some(id) = id else {
            if method == "session/cancel" {
                for id in cancelled.drain(..) {
                    peer.respond(id, Ok(json!({"stopReason": "cancelled"})));
                }
            }
            continue;
        };
        match method.as_str() {
            "initialize" => peer.respond(
                id,
                Ok(json!({"protocolVersion": 1, "agentCapabilities": {
                    "loadSession": true, "promptCapabilities": {"image": true}}})),
            ),
            "session/new" => peer.respond(
                id,
                Ok(json!({"sessionId": "child-1", "configOptions": options("medium")})),
            ),
            "session/load" => {
                // A real agent replays the history before answering.
                peer.notify(
                    "session/update",
                    update(
                        "child-1",
                        json!({"sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "old reply"}}),
                    ),
                );
                peer.respond(id, Ok(json!({"configOptions": options("medium")})));
            }
            "session/set_config_option" => peer.respond(
                id,
                Ok(json!({"configOptions": options(params["value"].as_str().unwrap_or("low"))})),
            ),
            "session/prompt" => {
                let text = params["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if text == "wait" {
                    cancelled.push(id);
                    continue;
                }
                peer.notify(
                    "session/update",
                    update(
                        "child-1",
                        json!({"sessionUpdate": "agent_thought_chunk",
                        "content": {"type": "text", "text": "hmm"}}),
                    ),
                );
                peer.notify(
                    "session/update",
                    update(
                        "child-1",
                        json!({"sessionUpdate": "tool_call", "toolCallId": "t1",
                        "title": "Write notes.md", "kind": "edit", "status": "pending"}),
                    ),
                );
                let permission = peer
                    .request(
                        "session/request_permission",
                        json!({"sessionId": "child-1",
                            "toolCall": {"toolCallId": "t1", "title": "Write notes.md"},
                            "options": [
                                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}),
                    )
                    .await;
                let _ = seen.send((
                    "permission answer".into(),
                    permission.unwrap_or(Value::Null),
                ));
                let read = peer
                    .request(
                        "fs/read_text_file",
                        json!({"sessionId": "child-1", "path": "/etc/hosts"}),
                    )
                    .await;
                let _ = seen.send((
                    "fs answer".into(),
                    json!(read.err().map(|error| error.code)),
                ));
                peer.notify(
                    "session/update",
                    update(
                        "child-1",
                        json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
                        "status": "completed"}),
                    ),
                );
                peer.notify(
                    "session/update",
                    update(
                        "child-1",
                        json!({"sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "Wrote it."}}),
                    ),
                );
                peer.notify(
                    "session/update",
                    update(
                        "child-1",
                        json!({"sessionUpdate": "usage_update", "used": 900, "size": 200000}),
                    ),
                );
                peer.respond(id, Ok(json!({"stopReason": "end_turn"})));
            }
            other => peer.respond(id, Err(RpcError::method_not_found(other))),
        }
    }
}

async fn start(
    choice: Option<&str>,
    options: StartOptions,
) -> (
    crate::harness::session::test_support::Thread,
    mpsc::UnboundedReceiver<(String, Value)>,
) {
    let harness = harness("fake-acp", HarnessKind::Acp);
    let ((ours, our_incoming), (theirs, their_incoming)) = pipe();
    let (seen_tx, seen) = mpsc::unbounded_channel();
    tokio::spawn(fake_agent(theirs, their_incoming, seen_tx));
    let thread = thread(&harness, choice);
    let adapter = Adapter::connect(
        &harness,
        ours,
        our_incoming,
        None,
        &std::env::temp_dir(),
        options,
        thread.session.output(),
    )
    .await
    .expect("the fake agent starts");
    thread.session.attach(adapter).await;
    (thread, seen)
}

fn requests(seen: &mut mpsc::UnboundedReceiver<(String, Value)>) -> Vec<(String, Value)> {
    let mut all = Vec::new();
    while let Ok(entry) = seen.try_recv() {
        all.push(entry);
    }
    all
}

fn text(text: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Text(TextContent::new(text))]
}

#[tokio::test]
async fn updates_and_permissions_are_relayed() {
    let (mut thread, mut seen) = start(Some("allow"), StartOptions::default()).await;
    assert_eq!(
        thread.session.prompt(text("write notes")).await,
        Ok(StopReason::EndTurn)
    );
    let updates = drain(&mut thread.updates);
    assert_eq!(
        kinds(&updates),
        [
            "agent_thought_chunk",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk",
            "usage_update"
        ]
    );
    assert_eq!(updates[3]["content"]["text"], "Wrote it.");
    assert_eq!(updates[4]["used"], 900);
    assert_eq!(
        thread.session.record().usage.as_ref(),
        Some(&updates[4]),
        "the agent's usage is kept for session/load"
    );

    let history = thread.session.history();
    let roles = history
        .iter()
        .map(|message| message.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
    assert_eq!(history[1].content, "<think>hmm</think>");
    let replayed = crate::harness::transcript::replay(&history, None)
        .into_iter()
        .map(|update| serde_json::to_value(update).unwrap())
        .collect::<Vec<_>>();
    let call = replayed
        .iter()
        .find(|update| update["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_eq!(call["title"], "Write notes.md");
    assert_eq!(call["kind"], "edit");
    assert_eq!(call["status"], "completed");

    let permission = thread.permissions.try_recv().unwrap();
    assert_eq!(
        permission.tool_call.fields.title.as_deref(),
        Some("Write notes.md")
    );
    let requests = requests(&mut seen);
    let answer = requests
        .iter()
        .find(|(method, _)| method == "permission answer")
        .unwrap();
    assert_eq!(
        answer.1,
        json!({"outcome": {"outcome": "selected", "optionId": "allow"}})
    );
    let fs = requests
        .iter()
        .find(|(method, _)| method == "fs answer")
        .unwrap();
    assert_eq!(fs.1, json!(-32601), "file system requests are refused");
    let initialize = requests
        .iter()
        .find(|(method, _)| method == "initialize")
        .unwrap();
    assert_eq!(
        initialize.1["clientCapabilities"],
        json!({"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false})
    );
    let prompt = requests
        .iter()
        .find(|(method, _)| method == "session/prompt")
        .unwrap();
    assert_eq!(prompt.1["sessionId"], "child-1");
}

#[tokio::test]
async fn model_and_thought_level_become_model_and_effort() {
    let (thread, mut seen) = start(
        None,
        StartOptions {
            effort: Some("high".into()),
            ..StartOptions::default()
        },
    )
    .await;
    let options = serde_json::to_value(thread.session.config_options().await).unwrap();
    let ids = options
        .as_array()
        .unwrap()
        .iter()
        .map(|option| option["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["model", "effort", "mode"]);
    assert_eq!(
        options[1]["currentValue"], "high",
        "the start effort applied"
    );
    let set = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "session/set_config_option")
        .unwrap();
    assert_eq!(set.1["configId"], "thinking");

    let options = thread
        .session
        .set_config("effort", json!("low"))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&options).unwrap()[1]["currentValue"],
        "low"
    );
    assert!(thread.session.set_config("nope", json!("x")).await.is_err());
    assert_eq!(thread.session.record().effort.as_deref(), Some("low"));
    assert_eq!(thread.session.record().model.as_deref(), Some("sonnet"));
}

#[tokio::test]
async fn cancel_reaches_the_agent() {
    let (thread, mut seen) = start(None, StartOptions::default()).await;
    let session = Arc::clone(&thread.session);
    let turn = tokio::spawn(async move { session.prompt(text("wait")).await });
    for _ in 0..100 {
        if requests(&mut seen)
            .iter()
            .any(|(method, _)| method == "session/prompt")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    thread.session.cancel();
    assert_eq!(turn.await.unwrap(), Ok(StopReason::Cancelled));
}

#[tokio::test]
async fn resuming_loads_the_session_without_replaying_it_twice() {
    let (mut thread, mut seen) = start(
        None,
        StartOptions {
            resume: Some("child-1".into()),
            ..StartOptions::default()
        },
    )
    .await;
    let requests = requests(&mut seen);
    assert!(requests.iter().any(|(method, _)| method == "session/load"));
    assert!(!requests.iter().any(|(method, _)| method == "session/new"));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        drain(&mut thread.updates).is_empty(),
        "the agent's replay is not sent again"
    );
    assert!(crate::harness::learned_for("fake-acp").resume == Some(true));
}

#[test]
fn legacy_model_state_becomes_a_model_option() {
    let option = legacy_model_option(Some(&json!({
        "currentModelId": "a",
        "availableModels": [{"modelId": "a", "name": "A"}, {"modelId": "b", "name": "B"}]
    })))
    .unwrap();
    let option = serde_json::to_value(option).unwrap();
    assert_eq!(option["id"], "model");
    assert_eq!(option["currentValue"], "a");
    assert_eq!(option["options"][1]["value"], "b");
}

fn orchestrator() -> crate::orchestration::hub::Launch {
    crate::orchestration::hub::Launch::new(
        "/opt/rustcode/bin/rustcode".into(),
        "thread-1",
        std::path::Path::new("/tmp/run/orchestrator-1.sock"),
        "secret",
    )
}

#[tokio::test]
async fn the_orchestrator_is_added_to_the_child_session() {
    let (_thread, mut seen) = start(
        None,
        StartOptions {
            orchestrator: Some(orchestrator()),
            ..StartOptions::default()
        },
    )
    .await;
    let new = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "session/new")
        .expect("session/new was sent")
        .1;
    assert_eq!(
        new["mcpServers"],
        json!([{
            "name": "rustcode",
            "command": "/opt/rustcode/bin/rustcode",
            "args": ["mcp-orchestrator", "--session", "thread-1",
                     "--socket", "/tmp/run/orchestrator-1.sock", "--token", "secret"],
            "env": [],
        }])
    );

    let (_thread, mut seen) = start(None, StartOptions::default()).await;
    let new = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "session/new")
        .unwrap()
        .1;
    assert_eq!(new["mcpServers"], json!([]), "no orchestrator, no servers");
}
