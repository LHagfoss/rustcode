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
    /// Report token usage, then finish.
    Usage,
    /// Ask the user two questions, then finish.
    AskUser,
    /// Codex compacts on its own mid-turn, then answers.
    AutoCompact,
    /// Start two of Codex's own subagents, let one end its turn, then wait.
    Subagents,
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
                    Script::Usage => {
                        note("thread/tokenUsage/updated", json!({"threadId": "thr_1", "turnId": turn_id, "tokenUsage": {
                            "total": {"totalTokens": 1500, "inputTokens": 1200, "cachedInputTokens": 800, "outputTokens": 300, "reasoningOutputTokens": 100},
                            "last": {"totalTokens": 700, "inputTokens": 600, "cachedInputTokens": 400, "outputTokens": 100, "reasoningOutputTokens": 50},
                            "modelContextWindow": 272000}}));
                        note("turn/completed", json!({"threadId": "thr_1", "turn": {"id": turn_id, "status": "completed", "items": []}}));
                    }
                    Script::AskUser => {
                        let answer = peer
                            .request(
                                "item/tool/requestUserInput",
                                json!({"threadId": "thr_1", "turnId": turn_id, "itemId": "ask1", "isBlocking": true,
                                    "questions": [
                                        {"id": "db", "header": "Database", "question": "Which database?",
                                         "options": [{"label": "Postgres", "description": "Relational"},
                                                     {"label": "SQLite", "description": "Embedded"}]},
                                        {"id": "tests", "header": "Tests", "question": "Add tests?",
                                         "options": [{"label": "No", "description": ""},
                                                     {"label": "Yes", "description": ""}]}]}),
                            )
                            .await;
                        let _ = seen.send(("answer".into(), answer.unwrap_or_else(|error| json!(error.code))));
                        note("turn/completed", json!({"threadId": "thr_1", "turn": {"id": turn_id, "status": "completed", "items": []}}));
                    }
                    Script::AutoCompact => {
                        let base = json!({"threadId": "thr_1", "turnId": turn_id});
                        let mut compacted = base.clone();
                        compacted["item"] = json!({"type": "contextCompaction", "id": "c1"});
                        note("item/completed", compacted);
                        let mut message = base.clone();
                        message["item"] = json!({"type": "agentMessage", "id": "m1", "text": "Answer"});
                        note("item/completed", message);
                        note("turn/completed", json!({"threadId": "thr_1", "turn": {"id": turn_id, "status": "completed", "items": []}}));
                    }
                    Script::Subagents => {
                        let base = json!({"threadId": "thr_1", "turnId": turn_id});
                        let with = |extra: Value| {
                            let mut params = base.clone();
                            for (key, value) in extra.as_object().unwrap() {
                                params[key] = value.clone();
                            }
                            params
                        };
                        let spawn = |status: &str, states: Value| {
                            json!({"item": {"type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent",
                                "prompt": "Write pytest coverage for calc.py", "receiverThreadIds": ["child1", "child2"],
                                "senderThreadId": "thr_1", "model": "gpt-6.1-sol", "status": status,
                                "agentsStates": states}})
                        };
                        note("item/started", with(spawn("inProgress", json!({}))));
                        let running = json!({"child1": {"status": "running"}, "child2": {"status": "running"}});
                        note("item/completed", with(spawn("completed", running)));
                        // A subagent's own command and turn: neither ends the thread's turn.
                        note("item/started", json!({"threadId": "child1", "turnId": "turn_c1",
                            "item": {"type": "commandExecution", "id": "cmd7", "command": "pytest", "status": "inProgress"}}));
                        note("item/completed", json!({"threadId": "child1", "turnId": "turn_c1",
                            "item": {"type": "commandExecution", "id": "cmd7", "command": "pytest", "status": "completed", "exitCode": 0, "aggregatedOutput": "2 passed"}}));
                        note("turn/completed", json!({"threadId": "child1", "turn": {"id": "turn_c1", "status": "completed", "items": []}}));
                        let wait = |status: &str, states: Value| {
                            json!({"item": {"type": "collabAgentToolCall", "id": "c2", "tool": "wait",
                                "receiverThreadIds": ["child1", "child2"], "senderThreadId": "thr_1",
                                "status": status, "agentsStates": states}})
                        };
                        note("item/started", with(wait("inProgress", json!({}))));
                        let done = json!({
                            "child1": {"status": "completed", "message": "Added 4 tests"},
                            "child2": {"status": "errored", "message": "calc.py has no parser"}});
                        note("item/completed", with(wait("completed", done)));
                        note("turn/completed", json!({"threadId": "thr_1", "turn": {"id": turn_id, "status": "completed", "items": []}}));
                    }
                    Script::UntilInterrupted | Script::Stubborn => {}
                }
            }
            "thread/compact/start" => {
                peer.respond(id, Ok(json!({})));
                // No turn/completed: the adapter must end the wait on the item itself.
                peer.notify("item/started", json!({"threadId": "thr_1", "turnId": "turn_c", "item": {"type": "contextCompaction", "id": "c9"}}));
                peer.notify("item/completed", json!({"threadId": "thr_1", "turnId": "turn_c", "item": {"type": "contextCompaction", "id": "c9"}}));
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
    assert!(
        !thread
            .session
            .steer(&[agent_client_protocol::schema::v1::ContentBlock::from(
                "too early"
            )])
            .await,
        "no turn yet"
    );
    let session = Arc::clone(&thread.session);
    let turn = tokio::spawn(async move { session.prompt(text("long task")).await });
    let mut steered = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if thread
            .session
            .steer(&[agent_client_protocol::schema::v1::ContentBlock::from(
                "also check tests",
            )])
            .await
        {
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

#[tokio::test]
async fn the_transcript_records_tool_calls_and_plans_for_replay() {
    let (thread, _seen) = start(Script::Reply, Some("accept"), StartOptions::default()).await;
    thread.session.prompt(text("hi")).await.unwrap();
    let history = thread.session.history();
    let roles = history
        .iter()
        .map(|message| message.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, ["user", "assistant", "tool", "tool", "assistant"]);
    assert_eq!(history[1].tool_calls[0].name, "ls");
    let record = history[2].tool_result.as_ref().unwrap();
    assert_eq!(record.tool_name, "execute");
    assert!(record.success && !record.pending);
    assert_eq!(history[2].content, "a\nb");

    let replayed = crate::harness::transcript::replay(&history, None)
        .into_iter()
        .map(|update| serde_json::to_value(update).unwrap())
        .collect::<Vec<_>>();
    let kinds = kinds(&replayed);
    let call = kinds.iter().position(|kind| kind == "tool_call").unwrap();
    let plan = kinds.iter().position(|kind| kind == "plan").unwrap();
    assert_eq!(kinds[0], "user_message_chunk");
    assert!(
        kinds[1..call]
            .iter()
            .any(|kind| kind == "agent_message_chunk")
    );
    assert!(call < plan && plan < kinds.len() - 1);
    assert_eq!(kinds.last().unwrap(), "agent_message_chunk");
    let call = &replayed[call];
    assert_eq!(call["toolCallId"], "cmd1");
    assert_eq!(call["title"], "ls");
    assert_eq!(call["kind"], "execute");
    assert_eq!(call["status"], "completed");
    assert_eq!(call["rawInput"]["command"], "ls");
    assert_eq!(call["content"][0]["content"]["text"], "a\nb");
    assert_eq!(replayed[plan]["entries"][0]["status"], "completed");
    assert_eq!(replayed[plan]["entries"][1]["status"], "in_progress");
    assert_eq!(replayed[plan]["entries"][1]["content"], "Answer");
}

#[tokio::test]
async fn token_usage_becomes_a_usage_update_and_is_kept() {
    let (mut thread, _seen) = start(Script::Usage, None, StartOptions::default()).await;
    thread.session.prompt(text("hi")).await.unwrap();
    let updates = drain(&mut thread.updates);
    assert_eq!(kinds(&updates), ["usage_update"]);
    let usage = &updates[0];
    assert_eq!(usage["used"], 600);
    assert_eq!(usage["size"], 272000);
    let meta = &usage["_meta"]["rustcode"];
    assert_eq!(meta["kind"], "usage");
    assert_eq!(meta["version"], crate::acp::usage::USAGE_META_VERSION);
    assert_eq!(meta["inputTokens"], 1200);
    assert_eq!(meta["outputTokens"], 300);
    assert_eq!(meta["cachedTokens"], 800);
    assert_eq!(
        thread.session.record().usage.as_ref(),
        Some(usage),
        "the thread keeps its latest usage for session/load"
    );
}

#[tokio::test]
async fn user_input_requests_are_asked_as_questions() {
    let (mut thread, mut seen) = start(
        Script::AskUser,
        Some("question_option_1"),
        StartOptions::default(),
    )
    .await;
    thread.session.prompt(text("set it up")).await.unwrap();
    let first = thread.permissions.try_recv().unwrap();
    let second = thread.permissions.try_recv().unwrap();
    let first = serde_json::to_value(first).unwrap();
    assert_eq!(first["_meta"]["rustcode"]["kind"], "question");
    assert_eq!(first["toolCall"]["rawInput"]["question"], "Which database?");
    assert_eq!(
        first["toolCall"]["rawInput"]["options"],
        json!(["Postgres", "SQLite"])
    );
    assert_eq!(
        serde_json::to_value(second).unwrap()["toolCall"]["title"],
        "Tests: Add tests?"
    );
    let answer = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "answer")
        .unwrap();
    assert_eq!(
        answer.1,
        json!({"answers": {"db": {"answers": ["SQLite"]}, "tests": {"answers": ["Yes"]}}})
    );
}

#[tokio::test]
async fn a_dismissed_question_fails_the_user_input_request() {
    let (thread, mut seen) = start(Script::AskUser, None, StartOptions::default()).await;
    thread.session.prompt(text("set it up")).await.unwrap();
    let answer = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "answer")
        .unwrap();
    assert_eq!(answer.1, json!(-32000));
}

#[tokio::test]
async fn the_orchestrator_is_added_to_the_codex_thread_config() {
    let launch = crate::orchestration::hub::Launch::new(
        "/opt/rustcode/bin/rustcode".into(),
        "thread-1",
        std::path::Path::new("/tmp/run/orchestrator-1.sock"),
        "secret",
    );
    let (_thread, mut seen) = start(
        Script::Reply,
        None,
        StartOptions {
            orchestrator: Some(launch),
            ..StartOptions::default()
        },
    )
    .await;
    let started = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "thread/start")
        .expect("thread/start was sent")
        .1;
    let server = &started["config"]["mcp_servers.rustcode"];
    assert_eq!(server["command"], "/opt/rustcode/bin/rustcode");
    assert_eq!(server["args"][0], "mcp-orchestrator");
    assert_eq!(server["env"]["RUSTCODE_ORCHESTRATOR_TOKEN"], "secret");
    assert!(
        !server["args"].to_string().contains("secret"),
        "the token never appears in the process arguments"
    );

    let (_thread, mut seen) = start(Script::Reply, None, StartOptions::default()).await;
    let started = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "thread/start")
        .unwrap()
        .1;
    assert!(started.get("config").is_none());
    assert_eq!(
        (&started["sandbox"], &started["approvalPolicy"]),
        (&json!("workspace-write"), &json!("on-request")),
        "Codex writes in the workspace and asks RustCode for more, trusted folder or not"
    );
}

#[tokio::test]
async fn the_workspaces_mcp_servers_are_thread_config() {
    let (_thread, mut seen) = start(
        Script::Reply,
        None,
        StartOptions {
            mcp: vec![
                crate::config::McpServerConfig {
                    name: "files".to_owned(),
                    command: "npx".to_owned(),
                    args: vec!["-y".to_owned(), "@example/files-mcp".to_owned()],
                    env: std::collections::HashMap::from([("API_KEY".to_owned(), "k".to_owned())]),
                    url: None,
                    headers: std::collections::HashMap::new(),
                    client_id: None,
                    enabled: true,
                    always_include: false,
                    timeout_secs: None,
                },
                crate::config::McpServerConfig {
                    name: "api".to_owned(),
                    command: String::new(),
                    args: Vec::new(),
                    env: std::collections::HashMap::new(),
                    url: Some("https://mcp.example.com/mcp".to_owned()),
                    headers: std::collections::HashMap::from([(
                        "Authorization".to_owned(),
                        "Bearer t".to_owned(),
                    )]),
                    client_id: None,
                    enabled: true,
                    always_include: false,
                    timeout_secs: None,
                },
            ],
            ..StartOptions::default()
        },
    )
    .await;
    let started = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "thread/start")
        .expect("thread/start was sent")
        .1;
    let config = &started["config"];
    assert_eq!(config["mcp_servers.files"]["command"], "npx");
    assert_eq!(
        config["mcp_servers.files"]["args"],
        json!(["-y", "@example/files-mcp"])
    );
    assert_eq!(config["mcp_servers.files"]["env"]["API_KEY"], "k");
    assert_eq!(
        config["mcp_servers.api"]["url"],
        "https://mcp.example.com/mcp"
    );
    assert_eq!(
        config["mcp_servers.api"]["http_headers"]["Authorization"],
        "Bearer t"
    );
    // Codex never sees RustCode's root prompt; where it runs and the reply
    // style ride along as developer instructions (docs/output-style.md).
    let instructions = started["developerInstructions"].as_str().unwrap();
    assert!(
        instructions.starts_with("<runtime_info>In case you're asked: you are running in RustCode")
    );
    assert!(
        instructions.ends_with(&crate::tools::schema::harness_output_style(
            crate::harness::client_smart_ui()
        ))
    );
}

#[tokio::test]
async fn compact_runs_codex_compaction_and_shows_a_compaction_card() {
    let (mut thread, mut seen) = start(Script::Reply, None, StartOptions::default()).await;
    let stop = thread.session.prompt(text("/compact")).await.unwrap();
    assert_eq!(stop, StopReason::EndTurn);
    let requests = requests(&mut seen);
    assert!(
        requests
            .iter()
            .any(|(method, _)| method == "thread/compact/start")
    );
    assert!(
        !requests.iter().any(|(method, _)| method == "turn/start"),
        "/compact is not sent to the model"
    );
    let updates = drain(&mut thread.updates);
    let card = updates
        .iter()
        .find(|update| update["_meta"]["rustcode"]["kind"] == "compaction")
        .expect("a compaction card");
    assert_eq!(card["sessionUpdate"], "agent_message_chunk");
}

#[tokio::test]
async fn codex_auto_compaction_inside_a_turn_does_not_end_the_turn() {
    let (mut thread, _seen) = start(Script::AutoCompact, None, StartOptions::default()).await;
    let stop = thread.session.prompt(text("go on")).await.unwrap();
    assert_eq!(stop, StopReason::EndTurn);
    let updates = drain(&mut thread.updates);
    assert!(
        updates
            .iter()
            .any(|update| update["_meta"]["rustcode"]["kind"] == "compaction")
    );
    assert!(
        updates
            .iter()
            .any(|update| update["content"]["text"] == "Answer"),
        "the reply after the compaction still arrives in the same turn"
    );
}

fn tool_approval(server: &str) -> Value {
    json!({
        "threadId": "t1",
        "serverName": server,
        "mode": "form",
        "message": format!("Allow the {server} MCP server to run tool \"fan_out\"?"),
        "requestedSchema": {"type": "object", "properties": {}},
        "_meta": {"codex_approval_kind": "mcp_tool_call"},
    })
}

#[test]
fn tool_approvals_for_our_own_server_are_accepted() {
    assert!(matches!(
        elicitation(&tool_approval("rustcode")),
        Elicitation::Accept
    ));
}

#[test]
fn tool_approvals_for_other_servers_ask_the_user() {
    let Elicitation::Ask(request) = elicitation(&tool_approval("github")) else {
        panic!("expected a permission request");
    };
    assert_eq!(
        request.tool_call.fields.title.as_deref(),
        Some("Allow the github MCP server to run tool \"fan_out\"?")
    );
    let ids: Vec<_> = request
        .options
        .iter()
        .map(|option| option.option_id.0.to_string())
        .collect();
    assert_eq!(ids, ["accept", "acceptForSession", "decline"]);
    assert_eq!(
        elicitation_reply("acceptForSession")["_meta"]["persist"],
        "session"
    );
    assert_eq!(elicitation_reply("")["action"], "decline");
}

#[test]
fn forms_with_fields_and_urls_are_declined() {
    let mut form = tool_approval("github");
    form["requestedSchema"]["properties"] = json!({"name": {"type": "string"}});
    assert!(matches!(elicitation(&form), Elicitation::Decline));
    let url = json!({"threadId": "t1", "serverName": "rustcode", "mode": "url",
        "elicitationId": "e1", "message": "Sign in", "url": "https://example.com"});
    assert!(matches!(elicitation(&url), Elicitation::Decline));
}

/// A `spawnAgent` item as the app-server records it (protocol v2: one item per
/// collab call, `agentsStates` keyed by the receiver's thread).
fn spawn_item() -> Value {
    json!({
        "type": "collabAgentToolCall",
        "id": "c1",
        "tool": "spawnAgent",
        "prompt": "\nWrite pytest coverage for calc.py\nThen stop.",
        "receiverThreadIds": ["t2", "t3"],
        "senderThreadId": "t1",
        "model": "gpt-6.1-sol",
        "reasoningEffort": "low",
        "status": "inProgress",
        "agentsStates": {},
    })
}

/// A `wait` on two agents that then finished, one of them with an error.
fn wait_item(status: &str) -> Value {
    let states = if status == "completed" {
        json!({
            "t2": {"status": "completed", "message": "Added 4 tests; all pass."},
            "t3": {"status": "errored", "message": "calc.py has no parser to test."},
        })
    } else {
        json!({"t2": {"status": "running"}, "t3": {"status": "pendingInit"}})
    };
    json!({
        "type": "collabAgentToolCall",
        "id": "c2",
        "tool": "wait",
        "receiverThreadIds": ["t2", "t3"],
        "senderThreadId": "t1",
        "status": status,
        "agentsStates": states,
    })
}

/// The updates one notification makes, as the JSON a client receives.
fn notified(state: &Mutex<State>, method: &str, params: Value) -> Vec<Value> {
    translate(state, method, &params)
        .into_iter()
        .map(|update| serde_json::to_value(update).expect("an update"))
        .collect()
}

fn item_event(thread: &str, item: Value) -> Value {
    json!({"threadId": thread, "turnId": "u1", "item": item})
}

/// The updates on the row of the agent in `thread`.
fn agent_rows<'a>(updates: &'a [Value], thread: &str) -> Vec<&'a Value> {
    let id = format!("c1:{thread}");
    updates
        .iter()
        .filter(|update| update["toolCallId"] == id.as_str())
        .collect()
}

#[test]
fn a_spawned_agent_is_a_row_of_its_own() {
    let state = Mutex::new(State::default());
    let updates = notified(&state, "item/started", item_event("t1", spawn_item()));
    // One row per agent, and no step saying the same thing again.
    assert_eq!(updates.len(), 2);
    for (update, thread) in updates.iter().zip(["t2", "t3"]) {
        assert_eq!(update["sessionUpdate"], "tool_call");
        assert_eq!(update["toolCallId"], format!("c1:{thread}").as_str());
        assert_eq!(update["status"], "in_progress");
        assert_eq!(
            update["title"],
            "Started agent: Write pytest coverage for calc.py"
        );
        // The fields the client's subagent row and its stack read.
        assert_eq!(update["_meta"]["claudeCode"]["toolName"], "Agent");
        assert_eq!(
            update["rawInput"]["description"],
            "Write pytest coverage for calc.py"
        );
        assert_eq!(update["rawInput"]["prompt"], spawn_item()["prompt"]);
        assert_eq!(update["rawInput"]["model"], "gpt-6.1-sol");
    }
}

#[test]
fn a_spawn_that_is_reported_twice_makes_one_row() {
    let state = Mutex::new(State::default());
    let spawn = spawn_item();
    let started = item_event("t1", spawn.clone());
    notified(&state, "item/started", started);
    let mut done = spawn.clone();
    done["status"] = json!("completed");
    done["agentsStates"] = json!({
        "t2": {"status": "running"},
        "t3": {"status": "running"},
    });
    let updates = notified(&state, "item/completed", item_event("t1", done));
    // Its agents are running; their rows move, and no new row or step appears.
    assert!(
        updates
            .iter()
            .all(|u| u["sessionUpdate"] == "tool_call_update")
    );
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0]["status"], "in_progress");
    assert_eq!(updates[0]["title"], Value::Null);
}

#[test]
fn waiting_for_agents_is_a_step_and_finishes_their_rows() {
    let state = Mutex::new(State::default());
    notified(&state, "item/started", item_event("t1", spawn_item()));
    let waiting = notified(
        &state,
        "item/started",
        item_event("t1", wait_item("inProgress")),
    );
    // Its agents are running, so only the step is new; their rows move too.
    let step = waiting
        .iter()
        .find(|update| update["toolCallId"] == "c2")
        .expect("a step");
    assert_eq!(step["sessionUpdate"], "tool_call");
    assert_eq!(step["title"], "Waiting for 2 agents");
    assert_eq!(agent_rows(&waiting, "t2")[0]["status"], "in_progress");

    let done = notified(
        &state,
        "item/completed",
        item_event("t1", wait_item("completed")),
    );
    // Both rows end, the finished one says how, and the step counts them.
    let rows = agent_rows(&done, "t2");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status"], "completed");
    assert_eq!(
        rows[0]["title"],
        "Agent finished: Write pytest coverage for calc.py"
    );
    assert_eq!(
        rows[0]["content"][0]["content"]["text"],
        "Added 4 tests; all pass."
    );
    let failed = agent_rows(&done, "t3");
    assert_eq!(failed[0]["status"], "failed");
    assert_eq!(
        failed[0]["title"],
        "Agent failed: Write pytest coverage for calc.py"
    );
    let step = done
        .iter()
        .find(|u| u["toolCallId"] == "c2")
        .expect("a step");
    assert_eq!(step["status"], "completed");
    assert_eq!(step["title"], "2 agents finished");
}

#[test]
fn closing_an_agent_finishes_its_row() {
    let state = Mutex::new(State::default());
    let mut spawn = spawn_item();
    spawn["receiverThreadIds"] = json!(["t2"]);
    notified(&state, "item/started", item_event("t1", spawn));
    let close = json!({
        "type": "collabAgentToolCall",
        "id": "c3",
        "tool": "closeAgent",
        "receiverThreadIds": ["t2"],
        "senderThreadId": "t1",
        "status": "completed",
        "agentsStates": {"t2": {"status": "shutdown"}},
    });
    let updates = notified(&state, "item/completed", item_event("t1", close));
    let rows = agent_rows(&updates, "t2");
    assert_eq!(rows[0]["status"], "completed");
    assert_eq!(
        rows[0]["title"],
        "Agent closed: Write pytest coverage for calc.py"
    );
    let step = updates
        .iter()
        .find(|update| update["toolCallId"] == "c3")
        .expect("a step");
    assert_eq!(
        step["title"],
        "Agent closed: Write pytest coverage for calc.py"
    );
}

#[test]
fn a_messaged_agent_row_runs_again() {
    let state = Mutex::new(State::default());
    let mut spawn = spawn_item();
    spawn["receiverThreadIds"] = json!(["t2"]);
    notified(&state, "item/started", item_event("t1", spawn));
    let mut finished = wait_item("completed");
    finished["receiverThreadIds"] = json!(["t2"]);
    finished["agentsStates"] = json!({"t2": {"status": "completed"}});
    notified(&state, "item/completed", item_event("t1", finished));
    let message = json!({
        "type": "collabAgentToolCall",
        "id": "c4",
        "tool": "sendInput",
        "prompt": "Also cover the parser.",
        "receiverThreadIds": ["t2"],
        "senderThreadId": "t1",
        "status": "inProgress",
        "agentsStates": {"t2": {"status": "running"}},
    });
    let updates = notified(&state, "item/started", item_event("t1", message));
    let rows = agent_rows(&updates, "t2");
    assert_eq!(rows[0]["status"], "in_progress");
    let step = updates
        .iter()
        .find(|update| update["toolCallId"] == "c4")
        .expect("a step");
    assert_eq!(
        step["title"],
        "Messaged agent: Write pytest coverage for calc.py"
    );
}

#[test]
fn an_agents_own_steps_fold_under_its_row() {
    let state = Mutex::new(State::default());
    notified(&state, "item/started", item_event("t1", spawn_item()));
    let command = json!({"type": "commandExecution", "id": "cmd9", "status": "inProgress",
        "command": "pytest tests/"});
    let updates = notified(&state, "item/started", item_event("t3", command.clone()));
    assert_eq!(updates[0]["toolCallId"], "cmd9");
    assert_eq!(
        updates[0]["_meta"]["claudeCode"]["parentToolUseId"],
        "c1:t3"
    );
    let done = notified(
        &state,
        "item/completed",
        item_event("t3", {
            let mut item = command;
            item["status"] = json!("completed");
            item["aggregatedOutput"] = json!("4 passed");
            item
        }),
    );
    assert_eq!(done[0]["_meta"]["claudeCode"]["parentToolUseId"], "c1:t3");
    // A call the thread itself made stays where it is.
    let own = notified(
        &state,
        "item/started",
        item_event(
            "t1",
            json!({"type": "commandExecution", "id": "cmd10", "command": "ls"}),
        ),
    );
    assert_eq!(own[0]["_meta"], Value::Null);
    // What the agent says, plans and spends stays out of the thread itself.
    for (method, params) in [
        (
            "item/agentMessage/delta",
            json!({"threadId": "t3", "itemId": "m9", "delta": "Done."}),
        ),
        (
            "item/reasoning/textDelta",
            json!({"threadId": "t3", "itemId": "r9", "delta": "Thinking"}),
        ),
        (
            "turn/plan/updated",
            json!({"threadId": "t3", "plan": [{"step": "Read", "status": "inProgress"}]}),
        ),
        (
            "thread/tokenUsage/updated",
            json!({"threadId": "t3", "tokenUsage": {"last": {"inputTokens": 10}}}),
        ),
        (
            "item/completed",
            json!({"threadId": "t3", "item": {"type": "agentMessage", "id": "m9", "text": "Done."}}),
        ),
        (
            "item/completed",
            json!({"threadId": "t3", "item": {"type": "contextCompaction", "id": "k9"}}),
        ),
    ] {
        assert!(
            notified(&state, method, params).is_empty(),
            "{method} of a subagent reached the thread"
        );
    }
}

#[test]
fn a_subagents_turn_ending_finishes_its_row_and_not_the_thread_turn() {
    let state = Mutex::new(State::default());
    notified(&state, "item/started", item_event("t1", spawn_item()));
    let (waiter, mut answer) = oneshot::channel();
    lock(&state).waiter = Some(waiter);
    let end = json!({"threadId": "t2", "turn": {"id": "u2", "status": "completed"}});
    let updates = notified(&state, "turn/completed", end.clone());
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["toolCallId"], "c1:t2");
    assert_eq!(updates[0]["status"], "completed");
    assert_eq!(
        updates[0]["title"],
        "Agent finished: Write pytest coverage for calc.py"
    );
    settle(&state, "turn/completed", &end);
    assert!(answer.try_recv().is_err(), "the thread's turn is not over");

    let failed = json!({"threadId": "t3",
        "turn": {"id": "u3", "status": "failed", "error": {"message": "out of context"}}});
    let updates = notified(&state, "turn/completed", failed.clone());
    assert_eq!(updates[0]["status"], "failed");
    assert_eq!(
        updates[0]["content"][0]["content"]["text"],
        "out of context"
    );
    settle(
        &state,
        "error",
        &json!({"threadId": "t3", "error": {"message": "no"}}),
    );
    assert!(answer.try_recv().is_err());

    let own = json!({"threadId": "t1", "turn": {"id": "u1", "status": "completed"}});
    settle(&state, "turn/completed", &own);
    assert!(matches!(answer.try_recv(), Ok(TurnEnd::Completed)));
}

#[test]
fn a_collab_call_without_agents_keeps_its_step() {
    let state = Mutex::new(State::default());
    let list = json!({"type": "collabAgentToolCall", "id": "c5", "tool": "listAgents",
        "receiverThreadIds": [], "senderThreadId": "t1", "status": "inProgress", "agentsStates": {}});
    let updates = notified(&state, "item/started", item_event("t1", list));
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["title"], "Listed agents");
}

#[tokio::test]
async fn codex_subagents_reach_the_client_as_child_rows() {
    let (mut thread, _seen) = start(Script::Subagents, None, StartOptions::default()).await;
    let stop = thread.session.prompt(text("cover this")).await.unwrap();
    // The subagents' own events did not end the thread's turn.
    assert_eq!(stop, StopReason::EndTurn);
    let updates = drain(&mut thread.updates);
    let find = |id: &str| {
        updates
            .iter()
            .filter(|update| update["toolCallId"] == id)
            .collect::<Vec<_>>()
    };
    let row = find("c1:child1");
    assert_eq!(row[0]["sessionUpdate"], "tool_call");
    assert_eq!(row[0]["_meta"]["claudeCode"]["toolName"], "Agent");
    assert_eq!(
        row[0]["rawInput"]["description"],
        "Write pytest coverage for calc.py"
    );
    // The command the subagent ran folds under its row.
    assert_eq!(
        find("cmd7")[0]["_meta"]["claudeCode"]["parentToolUseId"],
        "c1:child1"
    );
    // Its turn ending finished the row, before the wait reported the same.
    assert_eq!(row.last().unwrap()["status"], "completed");
    // The wait is a step of its own, and it says who finished.
    assert_eq!(find("c2")[0]["title"], "Waiting for 2 agents");
    assert_eq!(find("c2")[1]["title"], "2 agents finished");
    let failed = find("c1:child2").last().copied().unwrap();
    assert_eq!(failed["status"], "failed");
    assert_eq!(
        failed["content"][0]["content"]["text"],
        "calc.py has no parser"
    );
}
