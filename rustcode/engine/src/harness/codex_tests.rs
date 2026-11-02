//! The Codex adapter against a fake `codex app-server` in process.

use super::*;
use crate::harness::HarnessKind;
use crate::harness::session::test_support::{drain, harness, kinds, pipe, thread};
use crate::harness::session::{Adapter, StartOptions};
use agent_client_protocol::schema::v1::TextContent;

#[derive(Clone, Copy, PartialEq)]
enum Script {
    /// Stream a reply with a thought, an approved command and a plan.
    Reply,
    /// Wait for `turn/interrupt`.
    UntilInterrupted,
    /// Ignore `turn/interrupt`.
    Stubborn,
    /// Fail the turn.
    Fail,
}

/// A fake app-server. Every request it gets is reported on `seen`.
async fn fake_codex(
    peer: Arc<RpcPeer>,
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    script: Script,
    seen: mpsc::UnboundedSender<(String, Value)>,
) {
    let mut turn = 0;
    while let Some(message) = incoming.recv().await {
        let Incoming::Request { id, method, params } = message else {
            continue;
        };
        let _ = seen.send((method.clone(), params.clone()));
        match method.as_str() {
            "initialize" => peer.respond(id, Ok(json!({"userAgent": "fake", "codexHome": "/tmp", "platformFamily": "unix", "platformOs": "macos"}))),
            "model/list" => peer.respond(
                id,
                Ok(json!({
                    "data": [
                        {"id": "gpt-6-luna", "model": "gpt-6-luna", "displayName": "GPT-6-Luna",
                         "hidden": false, "isDefault": false, "defaultReasoningEffort": "medium",
                         "supportedReasoningEfforts": [
                            {"reasoningEffort": "low", "description": ""},
                            {"reasoningEffort": "medium", "description": ""},
                            {"reasoningEffort": "high", "description": ""}]},
                        {"id": "gpt-6.1-sol", "model": "gpt-6.1-sol", "displayName": "GPT-6.1-Sol",
                         "hidden": false, "isDefault": true, "defaultReasoningEffort": "low",
                         "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": ""}]},
                        {"id": "secret", "model": "secret", "displayName": "Hidden", "hidden": true,
                         "isDefault": false, "defaultReasoningEffort": "low", "supportedReasoningEfforts": []}
                    ],
                    "nextCursor": null
                })),
            ),
            "thread/start" | "thread/resume" => peer.respond(
                id,
                Ok(json!({"thread": {"id": "thr_1"}, "model": params["model"], "reasoningEffort": null})),
            ),
            "turn/start" => {
                turn += 1;
                let turn_id = format!("turn_{turn}");
                peer.respond(id, Ok(json!({"turn": {"id": turn_id, "status": "inProgress", "items": []}})));
                let note = |method: &str, params: Value| peer.notify(method, params);
                match script {
                    Script::Reply => {
                        let base = json!({"threadId": "thr_1", "turnId": turn_id});
                        let with = |extra: Value| {
                            let mut params = base.clone();
                            for (key, value) in extra.as_object().unwrap() {
                                params[key] = value.clone();
                            }
                            params
                        };
                        note("item/reasoning/summaryTextDelta", with(json!({"itemId": "r1", "delta": "Thinking", "summaryIndex": 0})));
                        note("item/agentMessage/delta", with(json!({"itemId": "m1", "delta": "Hel"})));
                        note("item/agentMessage/delta", with(json!({"itemId": "m1", "delta": "lo"})));
                        note("item/completed", with(json!({"item": {"type": "agentMessage", "id": "m1", "text": "Hello"}})));
                        note("item/started", with(json!({"item": {"type": "commandExecution", "id": "cmd1", "command": "ls", "cwd": "/tmp", "status": "inProgress"}})));
                        let decision = peer
                            .request(
                                "item/commandExecution/requestApproval",
                                with(json!({"itemId": "cmd1", "command": "ls", "kind": "command", "startedAtMs": 0})),
                            )
                            .await;
                        let _ = seen.send(("decision".into(), decision.unwrap_or(Value::Null)));
                        note("item/completed", with(json!({"item": {"type": "commandExecution", "id": "cmd1", "command": "ls", "status": "completed", "exitCode": 0, "aggregatedOutput": "a\nb"}})));
                        note("turn/plan/updated", with(json!({"explanation": null, "plan": [{"step": "Look", "status": "completed"}, {"step": "Answer", "status": "inProgress"}]})));
                        note("item/completed", with(json!({"item": {"type": "agentMessage", "id": "m2", "text": "Done."}})));
                        note("turn/completed", json!({"threadId": "thr_1", "turn": {"id": turn_id, "status": "completed", "items": []}}));
                    }
                    Script::Fail => {
                        note("error", json!({"threadId": "thr_1", "turnId": turn_id, "willRetry": false, "error": {"message": "usage limit reached"}}));
                        note("turn/completed", json!({"threadId": "thr_1", "turn": {"id": turn_id, "status": "failed", "error": null, "items": []}}));
                    }
                    Script::UntilInterrupted | Script::Stubborn => {}
                }
            }
            "turn/interrupt" => {
                peer.respond(id, Ok(json!({})));
                if script == Script::UntilInterrupted {
                    peer.notify("turn/completed", json!({"threadId": "thr_1", "turn": {"id": params["turnId"], "status": "interrupted", "items": []}}));
                }
            }
            "turn/steer" => peer.respond(id, Ok(json!({"turnId": params["expectedTurnId"]}))),
            other => peer.respond(id, Err(RpcError::method_not_found(other))),
        }
    }
}

async fn start(
    script: Script,
    choice: Option<&str>,
    options: StartOptions,
) -> (
    crate::harness::session::test_support::Thread,
    mpsc::UnboundedReceiver<(String, Value)>,
) {
    let harness = harness("codex", HarnessKind::Codex);
    let ((ours, our_incoming), (theirs, their_incoming)) = pipe();
    let (seen_tx, seen) = mpsc::unbounded_channel();
    tokio::spawn(fake_codex(theirs, their_incoming, script, seen_tx));
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
    .expect("the fake app-server starts");
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
async fn a_turn_streams_as_acp_updates_and_approvals_reach_the_client() {
    let (mut thread, mut seen) = start(
        Script::Reply,
        Some("accept"),
        StartOptions {
            model: Some("gpt-6-luna".into()),
            effort: Some("low".into()),
            ..StartOptions::default()
        },
    )
    .await;
    let stop = thread.session.prompt(text("hi")).await.unwrap();
    assert_eq!(stop, StopReason::EndTurn);

    let updates = drain(&mut thread.updates);
    assert_eq!(
        kinds(&updates),
        [
            "agent_thought_chunk",
            "agent_message_chunk",
            "agent_message_chunk",
            "tool_call",
            "tool_call_update",
            "plan",
            "agent_message_chunk",
        ]
    );
    assert_eq!(updates[0]["content"]["text"], "Thinking");
    assert_eq!(updates[1]["content"]["text"], "Hel");
    assert_eq!(updates[3]["toolCallId"], "cmd1");
    assert_eq!(updates[3]["kind"], "execute");
    assert_eq!(updates[3]["status"], "in_progress");
    assert_eq!(updates[4]["status"], "completed");
    assert_eq!(updates[4]["rawOutput"]["exitCode"], 0);
    assert_eq!(updates[5]["entries"][1]["status"], "in_progress");
    assert_eq!(
        updates[6]["content"]["text"], "\n\nDone.",
        "a second message is set apart from the first"
    );

    let permission = thread.permissions.try_recv().unwrap();
    assert_eq!(permission.tool_call.fields.title.as_deref(), Some("ls"));
    let options = permission
        .options
        .iter()
        .map(|option| option.option_id.0.to_string())
        .collect::<Vec<_>>();
    assert_eq!(options, ["accept", "acceptForSession", "decline"]);

    let requests = requests(&mut seen);
    let turn = requests
        .iter()
        .find(|(method, _)| method == "turn/start")
        .unwrap();
    assert_eq!(turn.1["model"], "gpt-6-luna");
    assert_eq!(turn.1["effort"], "low");
    assert_eq!(
        turn.1["input"][0],
        json!({"type": "text", "text": "hi", "text_elements": []})
    );
    let decision = requests
        .iter()
        .find(|(method, _)| method == "decision")
        .unwrap();
    assert_eq!(decision.1, json!({"decision": "accept"}));
}

#[tokio::test]
async fn a_cancelled_approval_cancels_the_codex_request() {
    let (thread, mut seen) = start(Script::Reply, None, StartOptions::default()).await;
    thread.session.prompt(text("hi")).await.unwrap();
    let decision = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "decision")
        .unwrap();
    assert_eq!(decision.1, json!({"decision": "cancel"}));
}

#[tokio::test]
async fn models_and_effort_become_config_options() {
    let (thread, _seen) = start(Script::Reply, None, StartOptions::default()).await;
    let options = serde_json::to_value(thread.session.config_options().await).unwrap();
    assert_eq!(options[0]["id"], "model");
    assert_eq!(
        options[0]["currentValue"], "gpt-6.1-sol",
        "the default model"
    );
    assert_eq!(
        options[0]["options"].as_array().unwrap().len(),
        2,
        "hidden models stay hidden"
    );
    assert_eq!(options[1]["id"], "effort");

    let options = thread
        .session
        .set_config("model", json!("gpt-6-luna"))
        .await
        .unwrap();
    let options = serde_json::to_value(options).unwrap();
    assert_eq!(options[0]["currentValue"], "gpt-6-luna");
    assert_eq!(options[1]["currentValue"], "medium");
    assert_eq!(options[1]["options"].as_array().unwrap().len(), 3);
    assert!(
        thread
            .session
            .set_config("effort", json!("ultra"))
            .await
            .is_err()
    );
    assert!(
        thread
            .session
            .set_config("model", json!("nope"))
            .await
            .is_err()
    );
    thread
        .session
        .set_config("effort", json!("high"))
        .await
        .unwrap();
    assert_eq!(
        thread.session.record().effort.as_deref(),
        Some("high"),
        "the thread records its choice"
    );
}

#[tokio::test]
async fn cancel_interrupts_the_turn() {
    let (thread, mut seen) = start(Script::UntilInterrupted, None, StartOptions::default()).await;
    let session = Arc::clone(&thread.session);
    let turn = tokio::spawn(async move { session.prompt(text("long task")).await });
    for _ in 0..100 {
        if requests(&mut seen)
            .iter()
            .any(|(method, _)| method == "turn/start")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    thread.session.cancel();
    assert_eq!(turn.await.unwrap(), Ok(StopReason::Cancelled));
}

#[tokio::test(start_paused = true)]
async fn a_harness_that_ignores_cancel_is_killed_after_the_grace_period() {
    let (thread, _seen) = start(Script::Stubborn, None, StartOptions::default()).await;
    let session = Arc::clone(&thread.session);
    let turn = tokio::spawn(async move { session.prompt(text("long task")).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    thread.session.cancel();
    assert_eq!(turn.await.unwrap(), Ok(StopReason::Cancelled));
}

#[tokio::test]
async fn a_failed_turn_is_a_failed_prompt_with_the_reason() {
    let (thread, _seen) = start(Script::Fail, None, StartOptions::default()).await;
    let error = thread.session.prompt(text("hi")).await.unwrap_err();
    assert_eq!(error, "Codex: usage limit reached");
}

#[tokio::test]
async fn steer_adds_input_to_the_running_turn() {
    let (thread, mut seen) = start(Script::UntilInterrupted, None, StartOptions::default()).await;
    assert!(!thread.session.steer("too early").await, "no turn yet");
    let session = Arc::clone(&thread.session);
    let turn = tokio::spawn(async move { session.prompt(text("long task")).await });
    let mut steered = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if thread.session.steer("also check tests").await {
            steered = true;
            break;
        }
    }
    assert!(steered);
    let steer = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "turn/steer")
        .unwrap();
    assert_eq!(steer.1["input"][0]["text"], "also check tests");
    assert_eq!(steer.1["expectedTurnId"], "turn_1");
    thread.session.cancel();
    turn.await.unwrap().unwrap();
}

#[test]
fn images_and_links_become_codex_input() {
    use agent_client_protocol::schema::v1::{ImageContent, ResourceLink};
    let items = input_items(&[
        ContentBlock::Image(ImageContent::new("QUJD", "image/png")),
        ContentBlock::ResourceLink(ResourceLink::new("main.rs", "file:///repo/main.rs")),
    ]);
    assert_eq!(
        items,
        [
            json!({"type": "image", "url": "data:image/png;base64,QUJD"}),
            json!({"type": "text", "text": "/repo/main.rs", "text_elements": []}),
        ]
    );
}

#[test]
fn file_changes_become_edit_cards() {
    let item = json!({"type": "fileChange", "id": "p1", "status": "completed",
        "changes": [{"path": "src/lib.rs", "kind": {"type": "update", "move_path": null}, "diff": "@@ -1 +1 @@\n-a\n+b"}]});
    let call = serde_json::to_value(tool_call_for(&item).unwrap()).unwrap();
    assert_eq!(call["title"], "Edit src/lib.rs");
    assert_eq!(call["kind"], "edit");
    assert_eq!(call["locations"][0]["path"], "src/lib.rs");
    let update = serde_json::to_value(tool_update_for(&item).unwrap()).unwrap();
    assert_eq!(update["status"], "completed");
    assert!(
        update["content"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("+b")
    );
    let declined =
        json!({"type": "commandExecution", "id": "c", "status": "declined", "exitCode": null});
    let update = serde_json::to_value(tool_update_for(&declined).unwrap()).unwrap();
    assert_eq!(update["status"], "failed");
}
