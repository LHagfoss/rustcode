//! The ACP adapter against a fake ACP agent in process.

use super::*;
use crate::harness::HarnessKind;
use crate::harness::session::test_support::{drain, harness, kinds, pipe, thread};
use crate::harness::session::{Adapter, StartOptions};
use agent_client_protocol::schema::v1::TextContent;

fn options(effort: &str) -> Value {
    options_for("sonnet", effort)
}

/// The fake agent's options with `model` running on `effort`: `opus` offers a
/// different set than `sonnet`, and `haiku` takes no effort at all, as an ACP
/// agent reports the thought levels of the model the session runs.
fn options_for(model: &str, effort: &str) -> Value {
    let efforts = match model {
        "opus" => json!([
            {"value": "max", "name": "Max"},
            {"value": "off", "name": "Off"},
            {"value": "default", "name": "Default"}
        ]),
        "haiku" => json!([]),
        _ => json!([
            {"value": "high", "name": "High"},
            {"value": "medium", "name": "Medium"},
            {"value": "low", "name": "Low"}
        ]),
    };
    let mut options = vec![json!(
        {"id": "model", "name": "Model", "category": "model", "type": "select",
         "currentValue": model,
         "options": [{"value": "sonnet", "name": "Sonnet"},
                     {"value": "opus", "name": "Opus"},
                     {"value": "haiku", "name": "Haiku"}]}
    )];
    if !efforts.as_array().is_some_and(Vec::is_empty) {
        options.push(json!(
            {"id": "thinking", "name": "Thinking", "category": "thought_level", "type": "select",
             "currentValue": effort, "options": efforts}
        ));
    }
    options.push(json!(
        {"id": "mode", "name": "Mode", "type": "select", "currentValue": "default",
         "options": [{"value": "default", "name": "Default"}]}
    ));
    Value::Array(options)
}

/// What the fake agent advertises and does.
#[derive(Clone, Copy)]
struct Fake {
    /// `sessionCapabilities.systemPrompt`: takes RustCode's text appended to
    /// its own system prompt, as `claude-agent-acp` does.
    system_prompt: bool,
    /// Echo the prompt it was given back as `user_message_chunk`, as pi-acp
    /// and OpenCode do.
    echo: bool,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            system_prompt: true,
            echo: false,
        }
    }
}

/// A fake ACP agent. Every request and notification it gets is reported on
/// `seen`; `stubborn` makes it ignore `session/cancel`.
async fn fake_agent(
    peer: Arc<RpcPeer>,
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    seen: mpsc::UnboundedSender<(String, Value)>,
    fake: Fake,
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
            "initialize" => {
                let mut capabilities = json!({
                    "loadSession": true, "promptCapabilities": {"image": true},
                    "mcpCapabilities": {"http": true},
                });
                if fake.system_prompt {
                    capabilities["sessionCapabilities"] = json!({"systemPrompt": {}});
                }
                peer.respond(
                    id,
                    Ok(json!({"protocolVersion": 1, "agentCapabilities": capabilities,
                              "_meta": {"steering": {"supported": true}}})),
                );
            }
            // As claude-agent-acp: injected into a running turn, else the
            // client should send a prompt.
            "_session/steering" => peer.respond(
                id,
                Ok(json!({"outcome": if cancelled.is_empty() { "promptRequired" } else { "injected" }})),
            ),
            "session/new" => {
                peer.respond(
                    id,
                    Ok(json!({"sessionId": "child-1", "configOptions": options("medium")})),
                );
                // Like Pi, a startup banner outside any turn. It must not reach the thread.
                peer.notify(
                    "session/update",
                    update(
                        "child-1",
                        json!({"sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "banner"}}),
                    ),
                );
            }
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
            "session/set_config_option" => {
                let value = params["value"].as_str().unwrap_or("low");
                // Another model, as a real agent answers: its own efforts, and
                // the level it starts on.
                let answer = if params["configId"] == "model" {
                    let effort = if value == "opus" { "default" } else { "medium" };
                    options_for(value, effort)
                } else {
                    options_for("sonnet", value)
                };
                peer.respond(id, Ok(json!({"configOptions": answer})))
            }
            "session/prompt" => {
                if fake.echo && let Some(echoed) = params["prompt"][0]["text"].as_str() {
                    peer.notify(
                        "session/update",
                        update(
                            "child-1",
                            json!({"sessionUpdate": "user_message_chunk",
                            "content": {"type": "text", "text": echoed}}),
                        ),
                    );
                }
                let last = params["prompt"].as_array().and_then(|blocks| blocks.last());
                let text = last
                    .and_then(|block| block["text"].as_str())
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
    start_with(Fake::default(), choice, options).await
}

async fn start_with(
    fake: Fake,
    choice: Option<&str>,
    options: StartOptions,
) -> (
    crate::harness::session::test_support::Thread,
    mpsc::UnboundedReceiver<(String, Value)>,
) {
    let harness = harness("fake-acp", HarnessKind::Acp);
    let ((ours, our_incoming), (theirs, their_incoming)) = pipe();
    let (seen_tx, seen) = mpsc::unbounded_channel();
    tokio::spawn(fake_agent(theirs, their_incoming, seen_tx, fake));
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

/// The adapter of a session on the fake agent, for what a listing does before
/// any thread owns it.
async fn adapter_with(fake: Fake) -> (Adapter, mpsc::UnboundedReceiver<(String, Value)>) {
    let harness = harness("fake-acp-listing", HarnessKind::Acp);
    let ((ours, our_incoming), (theirs, their_incoming)) = pipe();
    let (seen_tx, seen) = mpsc::unbounded_channel();
    tokio::spawn(fake_agent(theirs, their_incoming, seen_tx, fake));
    let thread = thread(&harness, None);
    let adapter = Adapter::connect(
        &harness,
        ours,
        our_incoming,
        None,
        &std::env::temp_dir(),
        StartOptions::default(),
        thread.session.output(),
    )
    .await
    .expect("the fake agent starts");
    (adapter, seen)
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
        json!({
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
            "_meta": {"jetbrains": {"air": {"version": 1, "capabilities": ["asyncTasks"]}}},
        })
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
async fn every_model_is_listed_with_the_efforts_it_offers() {
    let (adapter, mut seen) = adapter_with(Fake::default()).await;
    let models = adapter.probed_models().await;
    let listed = models
        .iter()
        .map(|model| {
            (
                model.id.as_str(),
                model.efforts.clone(),
                model.default_effort.clone(),
            )
        })
        .collect::<Vec<_>>();
    // ACP reports the thought levels of the model the session runs, so `opus`
    // and `haiku` are read by selecting them; each set is ordered from no
    // thinking to the most, `default` first.
    assert_eq!(
        listed,
        [
            (
                "sonnet",
                vec!["low".to_owned(), "medium".into(), "high".into()],
                Some("medium".into())
            ),
            (
                "opus",
                vec!["default".to_owned(), "off".into(), "max".into()],
                Some("default".into())
            ),
            ("haiku", Vec::new(), None),
        ]
    );
    // Each model was selected to read its own levels, then the session's went back.
    let switched = requests(&mut seen)
        .into_iter()
        .filter(|(method, params)| {
            method == "session/set_config_option" && params["configId"] == "model"
        })
        .map(|(_, params)| params["value"].clone())
        .collect::<Vec<_>>();
    assert_eq!(switched, [json!("opus"), json!("haiku"), json!("sonnet")]);
}

#[tokio::test]
async fn a_model_taking_no_effort_offers_none() {
    let (adapter, _seen) = adapter_with(Fake::default()).await;
    let models = adapter.probed_models().await;
    let haiku = models.iter().find(|model| model.id == "haiku").unwrap();
    assert!(haiku.efforts.is_empty());
    assert_eq!(haiku.default_effort, None);
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
                     "--socket", "/tmp/run/orchestrator-1.sock"],
            "env": [
                { "name": "RUSTCODE_ORCHESTRATOR_TOKEN", "value": "secret" },
                { "name": "RUSTCODE_ORCHESTRATION_VERSION",
                  "value": crate::orchestration::VERSION.to_string() },
            ],
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

#[tokio::test]
async fn the_workspaces_mcp_servers_go_to_the_child_session() {
    let (_thread, mut seen) = start(
        None,
        StartOptions {
            orchestrator: Some(orchestrator()),
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
                    url: Some("https://mcp.example.com/mcp".to_owned()),
                    headers: std::collections::HashMap::from([(
                        "Authorization".to_owned(),
                        "Bearer t".to_owned(),
                    )]),
                    command: String::new(),
                    args: Vec::new(),
                    env: std::collections::HashMap::new(),
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
    let new = requests(&mut seen)
        .into_iter()
        .find(|(method, _)| method == "session/new")
        .expect("session/new was sent")
        .1;
    let servers = new["mcpServers"].as_array().expect("a list of servers");
    assert_eq!(
        servers.len(),
        3,
        "the orchestrator, then the workspace's two"
    );
    assert_eq!(servers[1]["name"], "files");
    assert_eq!(servers[1]["command"], "npx");
    assert_eq!(servers[1]["args"], json!(["-y", "@example/files-mcp"]));
    assert_eq!(
        servers[1]["env"],
        json!([{ "name": "API_KEY", "value": "k" }]),
        "a stdio server's environment, named and valued"
    );
    assert_eq!(servers[2]["type"], "http");
    assert_eq!(servers[2]["url"], "https://mcp.example.com/mcp");
    assert_eq!(
        servers[2]["headers"],
        json!([{ "name": "Authorization", "value": "Bearer t" }])
    );
}

#[tokio::test]
async fn steering_adds_a_message_to_the_running_turn() {
    let (thread, mut seen) = start(None, StartOptions::default()).await;
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
    let turn = tokio::spawn(async move { session.prompt(text("wait")).await });
    let mut steered = false;
    for _ in 0..100 {
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
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(steered);
    let steer = requests(&mut seen)
        .into_iter()
        .rfind(|(method, _)| method == "_session/steering")
        .expect("a steering request");
    assert_eq!(steer.1["sessionId"], "child-1");
    assert_eq!(steer.1["prompt"][0]["text"], "also check tests");
    assert_eq!(
        steer.1["_meta"]["steering"]["idleBehavior"],
        "promptRequired"
    );
    let harness = harness("fake-acp", HarnessKind::Acp);
    assert_eq!(crate::harness::capabilities(&harness).steer, "live");
    thread.session.cancel();
    let _ = turn.await;
}

#[test]
fn remote_servers_need_the_childs_http_support_and_the_orchestrator_keeps_its_name() {
    let server = |name: &str, url: Option<&str>| crate::config::McpServerConfig {
        name: name.to_owned(),
        command: if url.is_some() {
            String::new()
        } else {
            "npx".to_owned()
        },
        args: Vec::new(),
        env: std::collections::HashMap::new(),
        url: url.map(str::to_owned),
        headers: std::collections::HashMap::new(),
        client_id: None,
        enabled: true,
        always_include: false,
        timeout_secs: None,
    };
    let options = StartOptions {
        orchestrator: Some(orchestrator()),
        mcp: vec![
            server("rustcode", None),
            server("files", None),
            server("api", Some("https://mcp.example.com/mcp")),
        ],
        ..StartOptions::default()
    };
    let names = |value: Value| -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|server| server["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        names(super::mcp_servers(&options, true)),
        ["rustcode", "files", "api"]
    );
    assert_eq!(
        names(super::mcp_servers(&options, false)),
        ["rustcode", "files"],
        "no HTTP support, no remote server"
    );
    let server_list = super::mcp_servers(&options, true);
    assert_eq!(
        server_list[0]["command"], "/opt/rustcode/bin/rustcode",
        "the orchestrator, not the user's server of the same name"
    );
}

/// The `session/prompt` blocks the fake agent was sent, in order.
fn prompts(seen: &mut mpsc::UnboundedReceiver<(String, Value)>) -> Vec<Vec<Value>> {
    requests(seen)
        .into_iter()
        .filter(|(method, _)| method == "session/prompt")
        .map(|(_, params)| params["prompt"].as_array().cloned().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn an_agent_without_a_system_prompt_gets_the_setup_block_on_its_first_prompt() {
    let fake = Fake {
        system_prompt: false,
        echo: false,
    };
    let (thread, mut seen) = start_with(fake, Some("allow"), StartOptions::default()).await;
    assert_eq!(
        thread.session.prompt(text("first")).await,
        Ok(StopReason::EndTurn)
    );
    assert_eq!(
        thread.session.prompt(text("second")).await,
        Ok(StopReason::EndTurn)
    );
    let sent = prompts(&mut seen);
    assert_eq!(sent.len(), 2);
    // Once, at the head of the first prompt, as its own text block.
    assert_eq!(sent[0].len(), 2, "{:?}", sent[0]);
    let reminder = sent[0][0]["text"].as_str().unwrap();
    assert!(reminder.starts_with("<rustcode_runtime>"), "{reminder}");
    assert!(
        reminder.contains("running in RustCode through the fake-acp harness"),
        "{reminder}"
    );
    assert!(reminder.trim_end().ends_with("</rustcode_runtime>"));
    assert_eq!(
        sent[0][1]["text"], "first",
        "the user's own words follow it"
    );
    // Not again, and nothing else changes.
    assert_eq!(sent[1].len(), 1, "{:?}", sent[1]);
    assert_eq!(sent[1][0]["text"], "second");
    // The thread's history is the conversation, not the setup text.
    let history = thread.session.history();
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content, "first");
    assert!(
        !serde_json::to_string(&history)
            .unwrap()
            .contains("<rustcode_runtime>")
    );
}

#[tokio::test]
async fn a_loaded_session_gets_no_setup_block() {
    let fake = Fake {
        system_prompt: false,
        echo: false,
    };
    let (thread, mut seen) = start_with(
        fake,
        None,
        StartOptions {
            resume: Some("child-1".to_owned()),
            ..StartOptions::default()
        },
    )
    .await;
    assert_eq!(
        thread.session.prompt(text("carry on")).await,
        Ok(StopReason::EndTurn)
    );
    let requests = requests(&mut seen);
    assert!(
        requests.iter().any(|(method, _)| method == "session/load"),
        "the session was loaded, not created"
    );
    let sent = requests
        .into_iter()
        .filter(|(method, _)| method == "session/prompt")
        .map(|(_, params)| params["prompt"].as_array().cloned().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(sent[0].len(), 1, "{:?}", sent[0]);
    assert_eq!(sent[0][0]["text"], "carry on");
}

#[tokio::test]
async fn an_agent_with_a_system_prompt_gets_no_block() {
    let (thread, mut seen) = start(Some("allow"), StartOptions::default()).await;
    assert_eq!(
        thread.session.prompt(text("first")).await,
        Ok(StopReason::EndTurn)
    );
    let requests = requests(&mut seen);
    let new = requests
        .iter()
        .find(|(method, _)| method == "session/new")
        .unwrap();
    assert!(
        new.1["systemPrompt"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("running in RustCode through the fake-acp harness"),
        "{:?}",
        new.1["systemPrompt"]
    );
    let sent = requests
        .iter()
        .filter(|(method, _)| method == "session/prompt")
        .map(|(_, params)| params["prompt"].as_array().cloned().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(sent[0].len(), 1, "no block, no double delivery");
}

#[tokio::test]
async fn the_agents_echo_of_the_setup_block_is_not_a_user_message() {
    let fake = Fake {
        system_prompt: false,
        echo: true,
    };
    let (mut thread, mut seen) = start_with(fake, Some("allow"), StartOptions::default()).await;
    assert_eq!(
        thread.session.prompt(text("first")).await,
        Ok(StopReason::EndTurn)
    );
    let updates = drain(&mut thread.updates);
    let echoed = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "user_message_chunk")
        .collect::<Vec<_>>();
    assert!(echoed.is_empty(), "{echoed:?}");
    // Everything else the turn produced still arrives.
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
    // And what the agent was sent still began with the block.
    let sent = prompts(&mut seen);
    assert!(
        sent[0][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("<rustcode_runtime>")
    );
}

#[test]
fn the_echo_filter_takes_the_block_and_keeps_the_user_text() {
    let whole = crate::tools::schema::runtime_reminder("setup text");
    let strip = |text: &str, pending: &mut String| super::strip_runtime_echo(text, pending, &whole);

    // The whole block in one chunk, alone: nothing to show.
    let mut pending = String::new();
    assert_eq!(strip(&whole, &mut pending), None);
    assert!(pending.is_empty());
    // Ordinary user content is untouched, whatever chunk it arrives in.
    assert_eq!(
        strip("fix the build", &mut pending).as_deref(),
        Some("fix the build")
    );
    assert!(pending.is_empty());
    // Split anywhere in the block, even inside a tag: the pieces go, the text
    // after the block stays, and no half tag reaches the thread.
    for split in [1, 10, 18, 19, 64, whole.len() - 1] {
        let mut pending = String::new();
        let (head, tail) = whole.split_at(split);
        assert_eq!(strip(head, &mut pending), None, "split at {split}");
        let kept = strip(&format!("{tail}fix the build"), &mut pending);
        assert_eq!(kept.as_deref(), Some("fix the build"), "split at {split}");
        assert!(pending.is_empty(), "split at {split}");
    }
    // A block the agent quoted in the middle of its own user message: only the
    // block goes.
    let mut pending = String::new();
    assert_eq!(
        strip(&format!("before {whole} after"), &mut pending).as_deref(),
        Some("before  after")
    );
}
