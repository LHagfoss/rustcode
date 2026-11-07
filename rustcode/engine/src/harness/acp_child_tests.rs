//! The ACP adapter against a fake ACP agent in process.

use super::*;
use crate::harness::HarnessKind;
use crate::harness::session::test_support::{drain, harness, kinds, pipe, thread};
use crate::harness::session::{Adapter, StartOptions};
use agent_client_protocol::schema::v1::TextContent;

fn options(effort: &str) -> Value {
    options_for("sonnet", effort)
}

/// The models the fake agent lists, in its own order.
const CATALOG: [&str; 5] = ["sonnet", "opus", "haiku", "titan", "nova"];

/// The thought levels `model` offers, in the agent's own (unordered) order.
fn efforts_of(model: &str) -> Value {
    match model {
        "opus" => json!([
            {"value": "max", "name": "Max"},
            {"value": "off", "name": "Off"},
            {"value": "default", "name": "Default"}
        ]),
        "haiku" => json!([]),
        "titan" => json!([
            {"value": "low", "name": "Low"},
            {"value": "minimal", "name": "Minimal"}
        ]),
        "nova" => json!([
            {"value": "xhigh", "name": "xHigh"},
            {"value": "high", "name": "High"}
        ]),
        _ => json!([
            {"value": "high", "name": "High"},
            {"value": "medium", "name": "Medium"},
            {"value": "low", "name": "Low"}
        ]),
    }
}

/// The level `model` starts on.
fn starts_at(model: &str) -> &'static str {
    match model {
        "opus" => "default",
        "haiku" => "",
        "titan" => "low",
        "nova" => "high",
        _ => "medium",
    }
}

/// The fake agent's options with `model` running on `effort`, as an ACP agent
/// reports the thought levels of the model the session runs.
fn options_for(model: &str, effort: &str) -> Value {
    let efforts = efforts_of(model);
    let models = CATALOG
        .iter()
        .map(|id| {
            let name = format!("{}{}", id[..1].to_uppercase(), &id[1..]);
            json!({"value": id, "name": name})
        })
        .collect::<Vec<_>>();
    let mut options = vec![json!(
        {"id": "model", "name": "Model", "category": "model", "type": "select",
         "currentValue": model, "options": models}
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
    // The model the session runs: a real agent reports the thought levels of
    // that model, so an effort change answers with its list, not another's.
    let mut model = "sonnet".to_owned();
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
                    model = value.to_owned();
                    options_for(value, starts_at(value))
                } else {
                    options_for(&model, value)
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
    start_named("fake-acp", fake, choice, options).await
}

/// A thread on the fake agent under `id`, so tests that read what the engine
/// learned for a harness do not race each other.
async fn start_named(
    id: &str,
    fake: Fake,
    choice: Option<&str>,
    options: StartOptions,
) -> (
    crate::harness::session::test_support::Thread,
    mpsc::UnboundedReceiver<(String, Value)>,
) {
    let harness = harness(id, HarnessKind::Acp);
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
    adapter_named("fake-acp-listing", fake).await
}

async fn adapter_named(
    id: &str,
    fake: Fake,
) -> (Adapter, mpsc::UnboundedReceiver<(String, Value)>) {
    let harness = harness(id, HarnessKind::Acp);
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

/// The `model` config option switches a listing (or a thread) made.
fn model_switches(seen: &mut mpsc::UnboundedReceiver<(String, Value)>) -> Vec<Value> {
    requests(seen)
        .into_iter()
        .filter(|(method, params)| {
            method == "session/set_config_option" && params["configId"] == "model"
        })
        .map(|(_, params)| params["value"].clone())
        .collect()
}

fn listed(models: &[HarnessModel]) -> Vec<(&str, Vec<String>, Option<String>, bool)> {
    models
        .iter()
        .map(|model| {
            (
                model.id.as_str(),
                model.efforts.clone(),
                model.default_effort.clone(),
                model.efforts_unknown,
            )
        })
        .collect()
}

fn levels(levels: &[&str]) -> Vec<String> {
    levels.iter().map(|level| (*level).to_owned()).collect()
}

#[tokio::test]
async fn the_models_a_listing_probes_get_the_efforts_they_offer() {
    let (adapter, mut seen) = adapter_with(Fake::default()).await;
    let models = adapter.probed_models(&[], &[]).await;
    // ACP reports the thought levels of the model the session runs, so each
    // other model is selected to read its own; each set is ordered from no
    // thinking to the most, `default` first.
    assert_eq!(
        listed(&models),
        [
            (
                "sonnet",
                levels(&["low", "medium", "high"]),
                Some("medium".into()),
                false
            ),
            (
                "opus",
                levels(&["default", "off", "max"]),
                Some("default".into()),
                false
            ),
            ("haiku", Vec::new(), None, false),
            (
                "titan",
                levels(&["minimal", "low"]),
                Some("low".into()),
                false
            ),
            (
                "nova",
                levels(&["high", "xhigh"]),
                Some("high".into()),
                false
            ),
        ]
    );
    // Each model was selected to read its own levels, then the session's went back.
    assert_eq!(
        model_switches(&mut seen),
        [
            json!("opus"),
            json!("haiku"),
            json!("titan"),
            json!("nova"),
            json!("sonnet")
        ]
    );
}

#[tokio::test]
async fn a_model_a_listing_did_not_reach_carries_the_union_and_says_unknown() {
    let (adapter, mut seen) = adapter_with(Fake::default()).await;
    let Adapter::Acp(child) = &adapter else {
        unreachable!("an ACP agent")
    };
    let models = child.probe_models(&[], &[], 2, PROBE_BUDGET).await;
    // `sonnet` (the model the session runs) and the two the cap allowed
    // (`opus`, `haiku`) are known; the rest carry the union of those, marked
    // unknown, so a picker never shows one model's list as another's; their
    // default is the level the harness runs with, not their own.
    let union = levels(&["default", "off", "low", "medium", "high", "max"]);
    assert_eq!(
        listed(&models),
        [
            (
                "sonnet",
                levels(&["low", "medium", "high"]),
                Some("medium".into()),
                false
            ),
            (
                "opus",
                levels(&["default", "off", "max"]),
                Some("default".into()),
                false
            ),
            ("haiku", Vec::new(), None, false),
            ("titan", union.clone(), Some("medium".into()), true),
            ("nova", union, Some("medium".into()), true),
        ]
    );
    // The cap stopped the listing after two models, then the session's model went back.
    assert_eq!(
        model_switches(&mut seen),
        [json!("opus"), json!("haiku"), json!("sonnet")]
    );
}

#[tokio::test]
async fn a_listing_with_no_time_left_probes_nothing() {
    let (adapter, mut seen) = adapter_with(Fake::default()).await;
    let Adapter::Acp(child) = &adapter else {
        unreachable!("an ACP agent")
    };
    let models = child
        .probe_models(&[], &[], PROBE_LIMIT, std::time::Duration::ZERO)
        .await;
    // Only the model the session runs is known; every other model carries its
    // levels as the union and says they are not its own.
    let union = levels(&["low", "medium", "high"]);
    assert_eq!(
        listed(&models),
        [
            (
                "sonnet",
                levels(&["low", "medium", "high"]),
                Some("medium".into()),
                false
            ),
            ("opus", union.clone(), Some("medium".into()), true),
            ("haiku", union.clone(), Some("medium".into()), true),
            ("titan", union.clone(), Some("medium".into()), true),
            ("nova", union, Some("medium".into()), true),
        ]
    );
    assert!(
        model_switches(&mut seen).is_empty(),
        "the budget stopped every probe before it started"
    );
}

#[tokio::test]
async fn a_model_a_listing_already_knew_is_not_probed_again() {
    let (adapter, mut seen) = adapter_with(Fake::default()).await;
    let Adapter::Acp(child) = &adapter else {
        unreachable!("an ACP agent")
    };
    let known = [
        HarnessModel {
            id: "nova".into(),
            name: "Nova".into(),
            efforts: levels(&["high", "xhigh"]),
            default_effort: Some("xhigh".into()),
            efforts_unknown: false,
        },
        HarnessModel {
            id: "haiku".into(),
            name: "Haiku".into(),
            efforts: Vec::new(),
            default_effort: None,
            efforts_unknown: false,
        },
    ];
    let models = child
        .probe_models(&known, &["nova".to_owned()], 2, PROBE_BUDGET)
        .await;
    // What a listing or a thread learned is kept, so `nova` (also a model a
    // setting names) and `haiku` cost no round trip; the cap went to others.
    let nova = models.iter().find(|model| model.id == "nova").unwrap();
    assert_eq!(nova.efforts, levels(&["high", "xhigh"]));
    assert_eq!(nova.default_effort.as_deref(), Some("xhigh"));
    let haiku = models.iter().find(|model| model.id == "haiku").unwrap();
    assert!(haiku.efforts.is_empty() && !haiku.efforts_unknown);
    let switches = model_switches(&mut seen);
    assert!(!switches.contains(&json!("nova")));
    assert!(!switches.contains(&json!("haiku")));
    assert_eq!(
        switches.len(),
        3,
        "two probes, then the session's model back"
    );
}

#[tokio::test]
async fn a_thread_start_keeps_the_efforts_of_the_model_it_runs() {
    let id = "fake-acp-lazy-start";
    crate::harness::forget_learned(id);
    let (thread, _seen) = start_named(
        id,
        Fake::default(),
        None,
        StartOptions {
            model: Some("nova".into()),
            effort: Some("medium".into()),
            ..StartOptions::default()
        },
    )
    .await;
    // `nova` takes high and xhigh, so `medium` went out as the closest, `high`.
    let options = serde_json::to_value(thread.session.config_options().await).unwrap();
    assert_eq!(options[0]["currentValue"], "nova");
    assert_eq!(options[1]["currentValue"], "high");
    let learned = crate::harness::learned_for(id).models.unwrap();
    let nova = learned.iter().find(|model| model.id == "nova").unwrap();
    assert_eq!(nova.efforts, levels(&["high", "xhigh"]));
    assert_eq!(nova.default_effort.as_deref(), Some("high"));
    assert!(!nova.efforts_unknown);
    // The model it did not run stays unknown, with the union as its list.
    let titan = learned.iter().find(|model| model.id == "titan").unwrap();
    assert!(titan.efforts_unknown);
    assert_eq!(titan.efforts, levels(&["low", "medium", "high", "xhigh"]));
}

#[tokio::test]
async fn switching_a_thread_to_a_model_keeps_that_models_efforts() {
    let id = "fake-acp-lazy-switch";
    crate::harness::forget_learned(id);
    let (thread, _seen) = start_named(id, Fake::default(), None, StartOptions::default()).await;
    let learned = crate::harness::learned_for(id).models.unwrap();
    let titan = learned.iter().find(|model| model.id == "titan").unwrap();
    assert!(titan.efforts_unknown, "nothing has run on it yet");

    thread
        .session
        .set_config("model", json!("titan"))
        .await
        .expect("the model switches");
    let learned = crate::harness::learned_for(id).models.unwrap();
    let titan = learned.iter().find(|model| model.id == "titan").unwrap();
    assert_eq!(titan.efforts, levels(&["minimal", "low"]));
    assert_eq!(titan.default_effort.as_deref(), Some("low"));
    assert!(!titan.efforts_unknown);
}

#[tokio::test]
async fn an_effort_the_model_does_not_offer_goes_out_as_the_closest_it_does() {
    let (thread, mut seen) = start(None, StartOptions::default()).await;
    // `sonnet` offers low, medium and high; `xhigh` is not one of them.
    thread
        .session
        .set_config("effort", json!("xhigh"))
        .await
        .expect("the effort is clamped to high");
    let sent = requests(&mut seen)
        .into_iter()
        .filter(|(method, params)| {
            method == "session/set_config_option" && params["configId"] == "thinking"
        })
        .map(|(_, params)| params["value"].clone())
        .collect::<Vec<_>>();
    assert_eq!(sent, [json!("high")]);
    assert_eq!(thread.session.record().effort.as_deref(), Some("high"));
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

/// The skills note is an input of the thread (`StartOptions::skills_note`), so
/// a test never reads the real home directory, and an agent that takes a
/// client system prompt gets exactly what the thread computed.
#[tokio::test]
async fn the_threads_skills_note_rides_in_the_client_system_prompt() {
    let note = "# Skills\nRustCode keeps this machine's skills here: `/skills`.\n\n";
    let (_thread, mut seen) = start(
        None,
        StartOptions {
            skills_note: Some(note.to_owned()),
            ..StartOptions::default()
        },
    )
    .await;
    let requests = requests(&mut seen);
    let new = requests
        .iter()
        .find(|(method, _)| method == "session/new")
        .unwrap();
    let prompt = new.1["systemPrompt"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(prompt.contains(note), "{prompt}");
    assert!(
        prompt.ends_with(&crate::tools::schema::harness_output_style(
            crate::harness::client_smart_ui()
        )),
        "the note comes before the reply style"
    );
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
