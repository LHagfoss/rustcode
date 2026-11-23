//! End-to-end agent turns through the real binary, with no model and no
//! network. The test is the client: it speaks ACP to `rc acp`, which runs the
//! thread on a scripted fake ACP agent (a Python script, the same shape as
//! `claude-agent-acp` or `opencode acp`) registered as a custom harness. A turn
//! crosses every layer a user's turn does: the client connection, the engine's
//! harness proxy, the child process, approvals and the thread store.
//!
//! Every case gets a private `RUSTCODE_CONFIG_DIR`, `HOME` and workspace under
//! the system temp dir, removed on drop, so the developer's real threads,
//! settings and sign-ins are never read or written.

#![cfg(unix)]

use serde_json::{Value, json};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

const BINARY: &str = env!("CARGO_BIN_EXE_rc");
/// Upper bound for one message from the engine; a turn takes milliseconds.
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// The fake agent. A prompt containing "please edit" asks to edit a file and
/// finishes on the answer; "wait for cancel" streams a line and ends only on
/// `session/cancel`; anything else is echoed back.
const FAKE_AGENT: &str = r##"
import json, sys

SESSION = "fake-session"

def send(message):
    sys.stdout.write(json.dumps(dict(jsonrpc="2.0", **message)) + "\n")
    sys.stdout.flush()

def update(body):
    send({"method": "session/update", "params": {"sessionId": SESSION, "update": body}})

def say(text):
    update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}})

asked = {}
waiting = []
next_request = 1000
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    ident = message.get("id")
    if method is None:
        prompt_id = asked.pop(ident, None)
        if prompt_id is None:
            continue
        outcome = (message.get("result") or {}).get("outcome") or {}
        allowed = outcome.get("outcome") == "selected" and outcome.get("optionId") == "allow"
        update({"sessionUpdate": "tool_call_update", "toolCallId": "edit-1",
                "status": "completed" if allowed else "failed"})
        say("Edited the file." if allowed else "Edit declined.")
        send({"id": prompt_id, "result": {"stopReason": "end_turn"}})
    elif method == "session/cancel":
        for prompt_id in waiting:
            send({"id": prompt_id, "result": {"stopReason": "cancelled"}})
        waiting.clear()
    elif method == "initialize":
        send({"id": ident, "result": {"protocolVersion": 1, "agentCapabilities": {}, "authMethods": []}})
    elif method == "session/new":
        send({"id": ident, "result": {"sessionId": SESSION}})
    elif method == "session/prompt":
        text = "".join(block.get("text", "") for block in message["params"]["prompt"])
        if "please edit" in text:
            say("Reading the request. ")
            update({"sessionUpdate": "tool_call", "toolCallId": "edit-1", "title": "Edit app.ts",
                    "kind": "edit", "status": "pending"})
            next_request += 1
            asked[next_request] = ident
            send({"id": next_request, "method": "session/request_permission", "params": {
                "sessionId": SESSION,
                "toolCall": {"toolCallId": "edit-1", "title": "Edit app.ts", "kind": "edit"},
                "options": [
                    {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                    {"optionId": "deny", "name": "Deny", "kind": "reject_once"},
                ],
            }})
        elif "wait for cancel" in text:
            say("Working on it.")
            waiting.append(ident)
        else:
            say("Echo: " + text)
            send({"id": ident, "result": {"stopReason": "end_turn"}})
    elif ident is not None:
        send({"id": ident, "result": {}})
"##;

/// `rc acp` in a sandbox, with the fake agent registered as harness `fake`.
struct Engine {
    root: PathBuf,
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: u64,
    /// Every notification and engine request seen so far, in order.
    seen: Vec<Value>,
}

impl Engine {
    fn start(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rustcode-agent-e2e-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["config", "home", "work"] {
            std::fs::create_dir_all(root.join(dir)).expect("create sandbox dir");
        }
        let agent = root.join("fake_agent.py");
        std::fs::write(&agent, FAKE_AGENT).expect("write the fake agent");
        let agent_path = Value::String(agent.display().to_string());
        let config = format!(
            "[[harnesses]]\nid = \"fake\"\nname = \"Fake agent\"\nkind = \"acp\"\ncommand = [\"python3\", \"-I\", {agent_path}]\n"
        );
        std::fs::write(root.join("config").join("config.toml"), config).expect("write config");
        let log = File::create(root.join("engine.log")).expect("create the engine log");

        let mut child = Command::new(BINARY)
            .arg("acp")
            .current_dir(root.join("work"))
            .env("RUSTCODE_CONFIG_DIR", root.join("config"))
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("home"))
            .env("RUSTCODE_SHELL_PROBE", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn rc acp");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (sender, messages) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            root,
            child,
            stdin,
            messages,
            next_id: 0,
            seen: Vec::new(),
        }
    }

    fn work_dir(&self) -> PathBuf {
        self.root.join("work")
    }

    /// The tail of the engine's stderr, for a failure message.
    fn log(&self) -> String {
        let text = std::fs::read_to_string(self.root.join("engine.log")).unwrap_or_default();
        let start = text.len().saturating_sub(4000);
        let start = (start..text.len())
            .find(|&index| text.is_char_boundary(index))
            .unwrap_or(text.len());
        format!("--- rc acp stderr (tail) ---\n{}", &text[start..])
    }

    fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
            .expect("write to rc acp");
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn next(&mut self, waiting_for: &str) -> Value {
        match self.messages.recv_timeout(MESSAGE_TIMEOUT) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => panic!(
                "no message from rc acp within {MESSAGE_TIMEOUT:?} while waiting for {waiting_for}\n{}",
                self.log()
            ),
            Err(RecvTimeoutError::Disconnected) => panic!(
                "rc acp exited while waiting for {waiting_for}\n{}",
                self.log()
            ),
        }
    }

    /// Waits for the response to request `id`, answering every request the
    /// engine sends meanwhile with `answer`.
    fn response(&mut self, id: u64, mut answer: impl FnMut(&Value) -> Value) -> Value {
        let waiting_for = format!("the response to request {id}");
        loop {
            let message = self.next(&waiting_for);
            let is_request = message.get("method").is_some();
            match message.get("id").cloned() {
                Some(got) if !is_request && got == json!(id) => {
                    assert!(
                        message.get("error").is_none(),
                        "request {id} failed: {message}\n{}",
                        self.log()
                    );
                    return message["result"].clone();
                }
                Some(request) if is_request => {
                    let result = answer(&message);
                    self.send(json!({"jsonrpc": "2.0", "id": request, "result": result}));
                    self.seen.push(message);
                }
                _ => self.seen.push(message),
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        self.response(id, |request| {
            panic!("unexpected request from the engine: {request}")
        })
    }

    /// Reads messages until one matches `predicate`.
    fn until(&mut self, what: &str, predicate: impl Fn(&Value) -> bool) {
        loop {
            let message = self.next(what);
            let hit = predicate(&message);
            self.seen.push(message);
            if hit {
                return;
            }
        }
    }

    fn initialize(&mut self) {
        let result = self.call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        );
        assert_eq!(result["protocolVersion"], 1, "initialize: {result}");
    }

    /// A new thread on the fake harness; returns its session id.
    fn new_fake_thread(&mut self) -> String {
        let cwd = self.work_dir();
        let result = self.call(
            "session/new",
            json!({
                "cwd": cwd,
                "mcpServers": [],
                "_meta": {"rustcode": {"harness": "fake"}},
            }),
        );
        assert_eq!(
            result["_meta"]["rustcode"]["harness"]["id"], "fake",
            "the thread should run on the fake harness: {result}"
        );
        result["sessionId"]
            .as_str()
            .expect("session/new returns a sessionId")
            .to_owned()
    }

    fn prompt(&mut self, session: &str, text: &str) -> u64 {
        self.request(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
        )
    }

    /// The `session/update` bodies of `session` seen so far.
    fn updates(&self, session: &str) -> Vec<&Value> {
        self.seen
            .iter()
            .filter(|message| {
                message["method"] == "session/update" && message["params"]["sessionId"] == session
            })
            .map(|message| &message["params"]["update"])
            .collect()
    }

    /// The agent's streamed text for `session`, in order.
    fn agent_text(&self, session: &str) -> String {
        self.updates(session)
            .into_iter()
            .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
            .filter_map(|update| update["content"]["text"].as_str())
            .collect()
    }

    fn tool_status(&self, session: &str, tool_call: &str) -> Option<String> {
        self.updates(session)
            .into_iter()
            .rev()
            .filter(|update| {
                update["sessionUpdate"] == "tool_call_update" && update["toolCallId"] == tool_call
            })
            .find_map(|update| update["status"].as_str())
            .map(str::to_owned)
    }

    /// The thread's directory in the store, wherever the store files it.
    fn thread_dir(&self, session: &str) -> PathBuf {
        find_dir(&self.root.join("config").join("threads"), session)
            .unwrap_or_else(|| panic!("no stored thread directory for {session}\n{}", self.log()))
    }

    fn stored_json(&self, session: &str, file: &str) -> Value {
        let path = self.thread_dir(session).join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn find_dir(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if entry.file_name() == name {
            return Some(path);
        }
        if let Some(found) = find_dir(&path, name) {
            return Some(found);
        }
    }
    None
}

fn choose(option: &'static str) -> impl FnMut(&Value) -> Value {
    move |request: &Value| {
        assert_eq!(
            request["method"], "session/request_permission",
            "only an approval is expected: {request}"
        );
        let options = request["params"]["options"]
            .as_array()
            .expect("an approval lists its options");
        assert!(
            options
                .iter()
                .any(|candidate| candidate["optionId"] == option),
            "the approval should offer {option}: {request}"
        );
        json!({"outcome": {"outcome": "selected", "optionId": option}})
    }
}

#[test]
fn an_approved_edit_finishes_the_turn_and_is_stored() {
    let mut engine = Engine::start("approve");
    engine.initialize();
    let session = engine.new_fake_thread();

    let prompt = engine.prompt(&session, "please edit app.ts");
    let result = engine.response(prompt, choose("allow"));

    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert!(
        engine
            .seen
            .iter()
            .any(|message| message["method"] == "session/request_permission"),
        "the approval should reach the client"
    );
    assert_eq!(
        engine.agent_text(&session),
        "Reading the request. Edited the file."
    );
    assert_eq!(
        engine.tool_status(&session, "edit-1").as_deref(),
        Some("completed")
    );

    let harness = engine.stored_json(&session, "harness.json");
    assert_eq!(harness["harness"], "fake", "{harness}");
    let history = engine.stored_json(&session, "history.json");
    let history = history.as_array().expect("history is a list of messages");
    let first = history.first().expect("the stored turn has messages");
    assert_eq!(first["role"], "user", "{first}");
    assert!(
        first["content"]
            .as_str()
            .is_some_and(|text| text.contains("please edit app.ts")),
        "{first}"
    );
    let last = history.last().expect("the stored turn has messages");
    assert_eq!(last["role"], "assistant", "{last}");
    assert_eq!(last["content"], "Edited the file.", "{last}");
}

#[test]
fn a_declined_edit_reaches_the_agent() {
    let mut engine = Engine::start("decline");
    engine.initialize();
    let session = engine.new_fake_thread();

    let prompt = engine.prompt(&session, "please edit app.ts");
    let result = engine.response(prompt, choose("deny"));

    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert_eq!(
        engine.agent_text(&session),
        "Reading the request. Edit declined."
    );
    assert_eq!(
        engine.tool_status(&session, "edit-1").as_deref(),
        Some("failed")
    );
}

#[test]
fn a_cancelled_turn_stops_and_the_thread_takes_the_next_message() {
    let mut engine = Engine::start("cancel");
    engine.initialize();
    let session = engine.new_fake_thread();

    let prompt = engine.prompt(&session, "wait for cancel");
    engine.until("the agent to start working", |message| {
        message["method"] == "session/update"
            && message["params"]["update"]["content"]["text"] == "Working on it."
    });
    engine.notify("session/cancel", json!({"sessionId": session}));
    let result = engine.response(prompt, |request| {
        panic!("unexpected request from the engine: {request}")
    });
    assert_eq!(result["stopReason"], "cancelled", "{result}");

    let next = engine.prompt(&session, "say hello");
    let result = engine.response(next, |request| {
        panic!("unexpected request from the engine: {request}")
    });
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert!(
        engine.agent_text(&session).contains("Echo: say hello"),
        "{}",
        engine.agent_text(&session)
    );
}
