//! `rustcode mcp-orchestrator --session <id> --socket <path>` (token in
//! `RUSTCODE_ORCHESTRATOR_TOKEN`):
//! a stdio MCP server that offers an external harness the orchestration
//! tools (`docs/orchestrator.md`). It holds no state of its own: every
//! `tools/call` goes to the parent engine's [`super::hub`], which checks the
//! session and token and forwards it to the client like a built-in call.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::hub::{CallAnswer, CallRequest};

/// The MCP revisions this server speaks; it answers with the client's when
/// it is one of these.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// How often a pending call reports progress. A call can block for minutes
/// (`wait_session`, `merge_when_green`), and an MCP client stops a call that
/// sends neither an answer nor progress (Claude Code aborts a silent one after
/// about half an hour), so a client that sent a `progressToken` gets this
/// heartbeat until the answer arrives.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);

/// Where calls go: the parent engine's socket, or a stand-in in tests.
pub trait Forward: Send + Sync + 'static {
    fn call(
        &self,
        request: CallRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CallAnswer> + Send + '_>>;
}

/// The parent engine at `socket`.
pub struct SocketForward {
    pub socket: PathBuf,
}

impl Forward for SocketForward {
    fn call(
        &self,
        request: CallRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CallAnswer> + Send + '_>> {
        Box::pin(async move {
            #[cfg(unix)]
            {
                super::hub::call(&self.socket, &request).await
            }
            #[cfg(not(unix))]
            {
                let _ = request;
                CallAnswer {
                    success: false,
                    content: "error: the orchestrator needs a unix socket".to_owned(),
                }
            }
        })
    }
}

/// The session and token every call carries.
#[derive(Debug, Clone)]
pub struct Identity {
    pub session_id: String,
    pub token: String,
}

/// The tools the server lists, as MCP tool entries, for a client of
/// `version`. `smart_ui` adds the block check, when the user has it on. The
/// pull request link tool is listed for every client: this server answers it
/// itself (#3369).
pub fn tools(version: u8, smart_ui: bool) -> Vec<Value> {
    let names = super::tool_names(version).into_iter().chain(
        // Answered by this server itself rather than by the client, so no
        // orchestration version gates it (#3369).
        std::iter::once(crate::pull_watch::LINK_TOOL_NAME)
            .chain(smart_ui.then_some(crate::smart_ui::CHECK_TOOL_NAME)),
    );
    names
        .filter_map(|name| crate::tools::TOOLS.iter().find(|tool| tool.name == name))
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": super::schema_for(tool.name, version, (tool.schema)()),
            })
        })
        .collect()
}

/// Run on stdin/stdout until the harness closes stdin.
pub async fn run(session_id: String, socket: PathBuf, token: String) -> Result<(), String> {
    // Set by the engine from the thread's client (`hub::Launch`): the tools it
    // can answer, and whether its Smart UI blocks run.
    let orchestration = std::env::var(super::hub::ORCHESTRATION_VERSION_ENV)
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .map_or(super::VERSION, |version| version.min(super::VERSION));
    let smart_ui = std::env::var(super::hub::SMART_UI_ENV).is_ok_and(|on| on == "1");
    serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        Identity { session_id, token },
        Arc::new(SocketForward { socket }),
        orchestration,
        smart_ui,
    )
    .await
}

/// Serve newline-delimited JSON-RPC from `input`, answering on `output`.
pub async fn serve<R, W>(
    input: R,
    output: W,
    identity: Identity,
    forward: Arc<dyn Forward>,
    // The client's orchestration version: which tools and hint it can answer.
    orchestration: u8,
    smart_ui: bool,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    serve_every(
        input,
        output,
        identity,
        forward,
        orchestration,
        smart_ui,
        PROGRESS_INTERVAL,
    )
    .await
}

/// [`serve`], with the interval a pending call reports progress at.
async fn serve_every<R, W>(
    input: R,
    mut output: W,
    identity: Identity,
    forward: Arc<dyn Forward>,
    orchestration: u8,
    smart_ui: bool,
    // How often a pending call reports progress; 0 disables it.
    progress_every: Duration,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (sender, mut answers) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(message) = answers.recv().await {
            let mut line = serde_json::to_vec(&message).unwrap_or_default();
            line.push(b'\n');
            if output.write_all(&line).await.is_err() || output.flush().await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(input).lines();
    let mut calls = tokio::task::JoinSet::new();
    // Each call in flight, by its request id, so `notifications/cancelled` ends it.
    let mut in_flight: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
    while let Some(line) = lines.next_line().await.map_err(|error| error.to_string())? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            let _ = sender.send(error(Value::Null, -32700, "parse error"));
            continue;
        };
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if method.is_empty() {
            continue;
        }
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        // A harness that stopped waiting for a call says so; the call goes with
        // it, and dropping it closes its socket, which cancels it in the engine.
        if method == CANCEL_NOTIFICATION {
            if let Some(call) = params.get("requestId").map(request_key) {
                if let Some(handle) = in_flight.remove(&call) {
                    handle.abort();
                }
            }
            continue;
        }
        // Answers to requests we never send.
        let Some(id) = id else {
            continue;
        };
        match method.as_str() {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked
                    .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                let _ = sender.send(result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "rustcode-orchestrator",
                            "title": "RustCode threads",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "instructions": super::prompt_hint(orchestration).trim(),
                    }),
                ));
            }
            "ping" => {
                let _ = sender.send(result(id, json!({})));
            }
            "tools/list" => {
                let _ = sender.send(result(
                    id,
                    json!({ "tools": tools(orchestration, smart_ui) }),
                ));
            }
            "tools/call" => {
                let sender = sender.clone();
                let forward = Arc::clone(&forward);
                let identity = identity.clone();
                let key = request_key(&id);
                in_flight.retain(|_, handle| !handle.is_finished());
                in_flight.insert(
                    key,
                    calls.spawn(async move {
                        let token = progress_token(&params);
                        let mut ticker =
                            token
                                .as_ref()
                                .filter(|_| !progress_every.is_zero())
                                .map(|_| {
                                    let mut ticker = tokio::time::interval(progress_every);
                                    // A slow call must not make the next ticks fire in a burst.
                                    ticker.set_missed_tick_behavior(
                                        tokio::time::MissedTickBehavior::Delay,
                                    );
                                    ticker
                                });
                        let tool = params.get("name").and_then(Value::as_str).unwrap_or("");
                        let message = progress_message(tool, &params);
                        let waiting = call(
                            forward.as_ref(),
                            &identity,
                            &params,
                            orchestration,
                            smart_ui,
                        );
                        tokio::pin!(waiting);
                        let answer = match ticker.as_mut() {
                            Some(ticker) => {
                                // The first tick of an interval is immediate: a call
                                // answered at once is not worth a notification.
                                ticker.tick().await;
                                let mut sent = 0u64;
                                loop {
                                    tokio::select! {
                                        answer = &mut waiting => break answer,
                                        _ = ticker.tick() => {
                                            sent += 1;
                                            if let Some(token) = token.as_ref() {
                                                let _ = sender.send(progress_notification(
                                                    token,
                                                    sent,
                                                    &message,
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            None => waiting.await,
                        };
                        let _ = sender.send(result(
                            id,
                            json!({
                                "content": [{ "type": "text", "text": answer.content }],
                                "isError": !answer.success,
                            }),
                        ));
                    }),
                );
            }
            _ => {
                let _ = sender.send(error(id, -32601, "method not found"));
            }
        }
    }
    // stdin closed: the harness is done with us. Dropping the calls in
    // flight closes their sockets, which cancels them in the engine.
    calls.shutdown().await;
    drop(sender);
    let _ = writer.await;
    Ok(())
}

async fn call(
    forward: &dyn Forward,
    identity: &Identity,
    params: &Value,
    version: u8,
    smart_ui: bool,
) -> CallAnswer {
    let tool = params.get("name").and_then(Value::as_str).unwrap_or("");
    // The block check is answered here: it needs no client and no thread, only
    // the user's Smart UI setting, and it forwards nothing.
    if tool == crate::smart_ui::CHECK_TOOL_NAME {
        if !smart_ui {
            return CallAnswer::failure(format!("error: unknown tool '{tool}'"));
        }
        let args = params
            .get("arguments")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let output = crate::smart_ui::check(&args);
        return CallAnswer {
            success: output.success,
            content: output.content,
        };
    }
    // A thread's pull request link (#3369) is the same store the engine's own
    // checks read, so the server answers it here instead of asking the client:
    // no approval is needed to watch a pull request, and an engine that is not
    // running the app's window still links one.
    if tool == crate::pull_watch::LINK_TOOL_NAME {
        let args = params
            .get("arguments")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let cwd = crate::checkpoints::thread_cwd(&identity.session_id)
            .filter(|cwd| cwd.is_dir())
            .or_else(|| std::env::current_dir().ok());
        let Some(cwd) = cwd else {
            return CallAnswer::failure("error: there is no folder to read the pull request in");
        };
        return match crate::pull_watch::answer(&args, &identity.session_id, &cwd).await {
            Ok(content) => CallAnswer {
                success: true,
                content,
            },
            Err(message) => CallAnswer::failure(format!("error: {message}")),
        };
    }
    if !super::tool_names(version).contains(&tool) {
        return CallAnswer {
            success: false,
            content: format!("error: unknown tool '{tool}'"),
        };
    }
    let args = params
        .get("arguments")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    // Checked here too, so a bad call never leaves the harness's machine
    // without a clear answer.
    if let Err(reason) = super::validate(tool, &args) {
        return CallAnswer {
            success: false,
            content: format!("error: invalid {tool} call: {reason}. Nothing was sent."),
        };
    }
    // An older server never listed `account`, so a harness that sends it gets
    // the same clear refusal the engine would give, not a call on another login.
    if version < super::ACCOUNT_VERSION && super::names_an_account(tool, &args) {
        return CallAnswer {
            success: false,
            content: format!(
                "error: {tool} with account needs a newer RustCode app. Nothing was sent."
            ),
        };
    }
    // The same for a file an older app would drop, instead of attaching it silently
    // nowhere.
    if version < super::ATTACHMENT_VERSION && super::carries_attachments(tool, &args) {
        return CallAnswer {
            success: false,
            content: format!(
                "error: {tool} with attachments needs a newer RustCode app. Nothing was sent."
            ),
        };
    }
    let limit = super::requested_wait(tool, &args).map(|wait| {
        wait + if tool == super::WAIT_SESSION {
            super::WAIT_SERVER_GRACE
        } else {
            super::MERGE_ANSWER_GRACE
        }
    });
    let waiting = forward.call(CallRequest {
        session_id: identity.session_id.clone(),
        token: identity.token.clone(),
        tool: tool.to_owned(),
        args,
    });
    let Some(limit) = limit else {
        return waiting.await;
    };
    // The app answers a wait at its own deadline, but nothing makes it: a slow
    // or suspended app would leave the harness waiting until its own abort.
    // Dropping the forwarded call closes the socket, which ends it in the app.
    match bounded(limit, waiting).await {
        Some(answer) => answer,
        None => CallAnswer {
            success: true,
            content: timed_out_answer(tool),
        },
    }
}

/// How often a bounded call checks the clock that keeps counting in sleep. The
/// runtime's timers stop while the machine sleeps; that clock does not.
const CLOCK_CHECK: Duration = Duration::from_secs(5);

/// Time from an arbitrary point that keeps counting while the machine sleeps
/// and never runs backwards: `CLOCK_MONOTONIC` on macOS (which, unlike
/// `std::time::Instant`'s `CLOCK_UPTIME_RAW`, includes sleep) and
/// `CLOCK_BOOTTIME` on Linux. Elsewhere the wall clock, which can be set back.
fn suspend_aware_now() -> Duration {
    #[cfg(unix)]
    {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const CLOCK: libc::clockid_t = libc::CLOCK_BOOTTIME;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `now` is a valid timespec for the call to fill.
        if unsafe { libc::clock_gettime(CLOCK, &mut now) } == 0 {
            return Duration::new(
                now.tv_sec.max(0) as u64,
                now.tv_nsec.clamp(0, 999_999_999) as u32,
            );
        }
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
}

/// Whether `limit` has passed between two readings of that clock. A reading
/// that went backwards (a wall clock set back) cannot show how long it was, so
/// it counts as passed: a wait that ends early is told to call again, a wait
/// that never ends is aborted by the harness.
fn deadline_passed(started: Duration, now: Duration, limit: Duration) -> bool {
    now.checked_sub(started)
        .is_none_or(|elapsed| elapsed >= limit)
}

/// `future`'s output, or `None` once `limit` has passed on the runtime's timer
/// or on the clock that counts through sleep, whichever comes first.
async fn bounded<T>(limit: Duration, future: impl std::future::Future<Output = T>) -> Option<T> {
    let started = suspend_aware_now();
    tokio::pin!(future);
    let timer = tokio::time::sleep(limit);
    tokio::pin!(timer);
    let mut clock = tokio::time::interval(CLOCK_CHECK.min(limit.max(Duration::from_millis(1))));
    clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            output = &mut future => return Some(output),
            _ = &mut timer => return None,
            _ = clock.tick() => {
                if deadline_passed(started, suspend_aware_now(), limit) {
                    return None;
                }
            }
        }
    }
}

/// What a wait says when the app did not answer in time: the same shape as the
/// app's own timeout, so the caller reads it the same way.
fn timed_out_answer(tool: &str) -> String {
    let answer = if tool == super::MERGE_WHEN_GREEN {
        json!({
            "state": "pending",
            "merged": false,
            "detail": "The app did not answer in time, so nothing is known about the checks. Call thread_status for the pull request, then call merge_when_green again to keep waiting.",
        })
    } else {
        json!({
            "status": "timeout",
            "links": [],
            "note": "The app did not answer in time. Call wait_session again to keep waiting; anything that settles also comes as a wake, so do not poll.",
        })
    };
    answer.to_string()
}

/// The MCP notification a harness sends to stop waiting for a call.
const CANCEL_NOTIFICATION: &str = "notifications/cancelled";
/// The notification that keeps a pending call alive.
const PROGRESS_NOTIFICATION: &str = "notifications/progress";

/// The `progressToken` a `tools/call` carried, when it carried one: only then
/// may the server send progress for it, and only then does a client that
/// listens for progress know the call is still running.
fn progress_token(params: &Value) -> Option<Value> {
    params.get("_meta")?.get("progressToken").cloned()
}

/// What a pending call's progress notification says: the threads it waits on
/// when it named them, else that it is still waiting.
fn progress_message(tool: &str, params: &Value) -> String {
    let args = params.get("arguments");
    match tool {
        super::WAIT_SESSION => {
            let links = args
                .and_then(|args| args.get("link_ids"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
                + args
                    .and_then(|args| args.get("link_id"))
                    .and_then(Value::as_str)
                    .map_or(0, |_| 1);
            match links {
                0 => "still waiting".to_owned(),
                1 => "still waiting on 1 thread".to_owned(),
                n => format!("still waiting on {n} threads"),
            }
        }
        super::MERGE_WHEN_GREEN => args
            .and_then(|args| args.get("session_id"))
            .and_then(Value::as_str)
            .map_or_else(
                || "still waiting for the checks".to_owned(),
                |thread| format!("still waiting for the checks of {thread}"),
            ),
        other => format!("still waiting on {other}"),
    }
}

/// One `notifications/progress`: `progress` must rise with every notification
/// for the same token, so it counts the heartbeats sent for this call.
fn progress_notification(token: &Value, progress: u64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": PROGRESS_NOTIFICATION,
        "params": { "progressToken": token, "progress": progress, "message": message },
    })
}

/// A request id as a map key: `1` and `"1"` are the same call to a harness.
fn request_key(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::hub::Hub;
    use super::*;
    use crate::preview::{PreviewClient, PreviewFuture};
    use tokio::io::AsyncBufReadExt;

    /// The app: answers every call with what it was asked.
    struct EchoApp;

    impl PreviewClient for EchoApp {
        fn request(&self, params: Value) -> PreviewFuture {
            Box::pin(async move { Ok(json!({ "asked": params })) })
        }
    }

    async fn hub() -> (tempfile::TempDir, Arc<Hub>) {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::bind(dir.path().join("run").join("orchestrator-test.sock"))
            .await
            .unwrap();
        (dir, hub)
    }

    /// Drive the server with `messages` and collect its answers by id; one
    /// answer is expected for each message that has an id.
    async fn exchange(identity: Identity, socket: PathBuf, messages: &[Value]) -> Vec<Value> {
        exchange_with(identity, socket, messages, false).await
    }

    async fn exchange_with(
        identity: Identity,
        socket: PathBuf,
        messages: &[Value],
        smart_ui: bool,
    ) -> Vec<Value> {
        exchange_versioned(identity, socket, messages, super::super::VERSION, smart_ui).await
    }

    async fn exchange_versioned(
        identity: Identity,
        socket: PathBuf,
        messages: &[Value],
        orchestration: u8,
        smart_ui: bool,
    ) -> Vec<Value> {
        let expected = messages
            .iter()
            .filter(|message| message.get("id").is_some())
            .count();
        let (mut client, server_side) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_side);
        let server = tokio::spawn(serve(
            server_read,
            server_write,
            identity,
            Arc::new(SocketForward { socket }),
            orchestration,
            smart_ui,
        ));
        let mut input = String::new();
        for message in messages {
            input.push_str(&message.to_string());
            input.push('\n');
        }
        client.write_all(input.as_bytes()).await.unwrap();
        let (read, mut write) = tokio::io::split(client);
        let mut lines = BufReader::new(read).lines();
        let mut answers = Vec::new();
        while answers.len() < expected {
            let line = lines.next_line().await.unwrap().expect("an answer");
            answers.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        write.shutdown().await.unwrap();
        server.await.unwrap().unwrap();
        answers.sort_by_key(|answer| answer["id"].as_i64());
        answers
    }

    #[test]
    fn the_block_check_is_listed_only_while_smart_ui_is_on() {
        let names = |smart_ui| {
            tools(super::super::VERSION, smart_ui)
                .into_iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        assert!(!names(false).contains(&crate::smart_ui::CHECK_TOOL_NAME.to_owned()));
        let with = names(true);
        assert_eq!(
            with.last().map(String::as_str),
            Some(crate::smart_ui::CHECK_TOOL_NAME)
        );
        // Everything else a Smart UI thread gets is what every thread gets.
        assert_eq!(with.len(), names(false).len() + 1);
    }

    #[tokio::test]
    async fn the_block_check_is_answered_by_the_server_without_the_app() {
        // A socket nothing listens on: the check must not need one.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.sock");
        let identity = || Identity {
            session_id: "thread-1".into(),
            token: "t".into(),
        };
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": crate::smart_ui::CHECK_TOOL_NAME,
                                     "arguments": {"html": "<title>X</title><p>hi</p>"}}});
        let answers = exchange_with(identity(), missing.clone(), &[call.clone()], true).await;
        let result = &answers[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("No problems found"), "{text}");
        assert!(text.contains("titled"), "{text}");

        // With Smart UI off the tool does not exist, so its name is unknown.
        let answers = exchange_with(identity(), missing, &[call], false).await;
        assert_eq!(answers[0]["result"]["isError"], true);
        assert!(
            answers[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown tool")
        );
    }

    #[tokio::test]
    async fn a_pull_request_link_is_answered_by_the_server_without_the_app() {
        // A socket nothing listens on: a link does not need the app either.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.sock");
        let identity = || Identity {
            session_id: "thread-1".into(),
            token: "t".into(),
        };
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": crate::pull_watch::LINK_TOOL_NAME,
                                     "arguments": {"action": "status", "number": 9_999_999}}});
        let answers = exchange_with(identity(), missing, &[call], false).await;
        let result = &answers[0]["result"];
        // Either this test config has no such link (a clear refusal) or it has
        // one (its state): both are answered here, never forwarded.
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("has no link to pull request #9999999"),
            "{text}"
        );

        // A bad action is refused with the ones it takes.
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                          "params": {"name": crate::pull_watch::LINK_TOOL_NAME,
                                     "arguments": {"action": "watch"}}});
        let answers =
            exchange_with(identity(), dir.path().join("missing.sock"), &[call], false).await;
        assert_eq!(answers[0]["result"]["isError"], true);
        assert!(
            answers[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("link, status, unlink")
        );
    }

    #[tokio::test]
    async fn tools_list_and_call_round_trip_to_the_app() {
        let (_dir, hub) = hub().await;
        let token = hub.register("thread-1", Arc::new(EchoApp), super::super::VERSION);
        let mode = std::fs::metadata(hub.path()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.permissions().mode() & 0o777, 0o600);

        let answers = exchange(
            Identity {
                session_id: "thread-1".into(),
                token,
            },
            hub.path().to_owned(),
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": {"protocolVersion": "2025-03-26", "capabilities": {}}}),
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                       "params": {"name": "start_session", "arguments": {"prompt": "fix the login"}}}),
                json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                       "params": {"name": "fan_out", "arguments": {"prompt": "x", "harnesses": []}}}),
                json!({"jsonrpc": "2.0", "id": 5, "method": "resources/list"}),
            ],
        )
        .await;
        assert_eq!(answers.len(), 5, "{answers:?}");
        assert_eq!(answers[0]["result"]["protocolVersion"], "2025-03-26");
        assert!(answers[0]["result"]["capabilities"]["tools"].is_object());
        let listed = answers[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let mut expected = super::super::tool_names(super::super::VERSION)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        // The link tool needs no orchestration version, so it is listed last
        // for every client and answered by this server (#3369).
        expected.push(crate::pull_watch::LINK_TOOL_NAME.to_owned());
        assert_eq!(listed, expected);
        assert!(answers[1]["result"]["tools"][0]["inputSchema"]["type"] == "object");

        let called = &answers[2]["result"];
        assert_eq!(called["isError"], false, "{called}");
        let text = called["content"][0]["text"].as_str().unwrap();
        let echoed: Value = serde_json::from_str(text).unwrap();
        // The same `_rustcode/orchestration` params a built-in call sends, and
        // the call id that lets a dropped call be ended on the app's side.
        assert_eq!(echoed["asked"]["tool"], json!(super::super::START_SESSION));
        assert_eq!(echoed["asked"]["args"], json!({"prompt": "fix the login"}));
        assert!(
            echoed["asked"]["callId"]
                .as_str()
                .is_some_and(|call| !call.is_empty()),
            "{echoed}"
        );

        assert_eq!(answers[3]["result"]["isError"], true);
        assert!(
            answers[3]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Nothing was sent")
        );
        assert_eq!(answers[4]["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn a_version_four_server_refuses_an_account_before_forwarding_it() {
        let (_dir, hub) = hub().await;
        let token = hub.register("thread-1", Arc::new(EchoApp), 4);
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": "start_session",
                                     "arguments": {"prompt": "go", "account": "work"}}});
        let answers = exchange_versioned(
            Identity {
                session_id: "thread-1".into(),
                token,
            },
            hub.path().to_owned(),
            &[call],
            4,
            false,
        )
        .await;
        let result = &answers[0]["result"];
        assert_eq!(result["isError"], true, "{result}");
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("newer RustCode app")
        );
    }

    #[tokio::test]
    async fn a_wrong_token_or_session_is_refused() {
        let (_dir, hub) = hub().await;
        let token = hub.register("thread-1", Arc::new(EchoApp), super::super::VERSION);
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": "list_sessions", "arguments": {}}});
        for identity in [
            Identity {
                session_id: "thread-1".into(),
                token: "not-the-token".into(),
            },
            Identity {
                session_id: "thread-2".into(),
                token: token.clone(),
            },
        ] {
            let answers = exchange(identity, hub.path().to_owned(), &[call.clone()]).await;
            assert_eq!(answers[0]["result"]["isError"], true);
            assert!(
                answers[0]["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("did not accept")
            );
        }
        // A released thread's token stops working.
        hub.unregister("thread-1");
        let answers = exchange(
            Identity {
                session_id: "thread-1".into(),
                token,
            },
            hub.path().to_owned(),
            &[call],
        )
        .await;
        assert_eq!(answers[0]["result"]["isError"], true);
    }

    /// A call the parent engine never answers: it can only end by being dropped.
    struct Hanging {
        started: Arc<std::sync::atomic::AtomicUsize>,
        dropped: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct CountDrop(Arc<std::sync::atomic::AtomicUsize>);

    impl Drop for CountDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl Forward for Hanging {
        fn call(
            &self,
            _request: CallRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CallAnswer> + Send + '_>> {
            self.started
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let dropped = Arc::clone(&self.dropped);
            Box::pin(async move {
                let _guard = CountDrop(dropped);
                std::future::pending::<CallAnswer>().await
            })
        }
    }

    #[tokio::test]
    async fn a_cancelled_call_stops_at_once_and_is_never_answered() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let started = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let (client, server_side) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (client_read, mut client_write) = tokio::io::split(client);
        let server = tokio::spawn(serve(
            server_read,
            server_write,
            Identity {
                session_id: "thread-1".into(),
                token: "t".into(),
            },
            Arc::new(Hanging {
                started: Arc::clone(&started),
                dropped: Arc::clone(&dropped),
            }),
            super::super::VERSION,
            false,
        ));
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": "list_sessions", "arguments": {}}});
        client_write
            .write_all(format!("{call}\n").as_bytes())
            .await
            .unwrap();
        while started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // The harness stops waiting for the call, in MCP's own words.
        let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                            "params": {"requestId": 1, "reason": "the harness gave up"}});
        client_write
            .write_all(format!("{cancel}\n").as_bytes())
            .await
            .unwrap();
        for _ in 0..200 {
            if dropped.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 1, "the call was dropped");
        // A call nobody waits for is not answered: the harness would read it as a phase late.
        let mut answers = BufReader::new(client_read).lines();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(150), answers.next_line())
                .await
                .is_err(),
            "no answer for a cancelled call"
        );
        client_write.shutdown().await.unwrap();
        server.await.unwrap().unwrap();
    }

    /// A call the app answers after `delay`, or never when it is `None`.
    struct Slow(Option<Duration>);

    impl Forward for Slow {
        fn call(
            &self,
            _request: CallRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CallAnswer> + Send + '_>> {
            let delay = self.0;
            Box::pin(async move {
                match delay {
                    Some(delay) => {
                        tokio::time::sleep(delay).await;
                        CallAnswer {
                            success: true,
                            content: "{\"status\":\"done\"}".to_owned(),
                        }
                    }
                    None => std::future::pending().await,
                }
            })
        }
    }

    /// Everything the server writes for one `tools/call` until its result,
    /// with the virtual time the call took.
    async fn run_slow(params: Value, app: Slow) -> (Vec<Value>, Duration) {
        let (client, server_side) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (client_read, mut client_write) = tokio::io::split(client);
        let server = tokio::spawn(serve_every(
            server_read,
            server_write,
            Identity {
                session_id: "thread-1".into(),
                token: "t".into(),
            },
            Arc::new(app),
            super::super::VERSION,
            false,
            PROGRESS_INTERVAL,
        ));
        let call = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": params});
        let started = tokio::time::Instant::now();
        client_write
            .write_all(format!("{call}\n").as_bytes())
            .await
            .unwrap();
        let mut lines = BufReader::new(client_read).lines();
        let mut seen = Vec::new();
        loop {
            let line = lines.next_line().await.unwrap().expect("a line");
            let message = serde_json::from_str::<Value>(&line).unwrap();
            let done = message.get("id").is_some();
            seen.push(message);
            if done {
                break;
            }
        }
        let took = started.elapsed();
        client_write.shutdown().await.unwrap();
        server.await.unwrap().unwrap();
        (seen, took)
    }

    #[tokio::test(start_paused = true)]
    async fn a_pending_call_reports_progress_until_its_answer() {
        let params = json!({"name": "wait_session",
                            "arguments": {"link_ids": ["a/1", "a/2"], "timeout_ms": 600_000},
                            "_meta": {"progressToken": "tok-1"}});
        let (seen, took) = run_slow(params, Slow(Some(Duration::from_secs(100)))).await;
        assert_eq!(took, Duration::from_secs(100));
        let (result, progress) = seen.split_last().unwrap();
        assert_eq!(result["id"], 7);
        assert_eq!(result["result"]["isError"], false, "{result}");
        // 30, 60 and 90 s: a heartbeat each, none for the instant start.
        assert_eq!(progress.len(), 3, "{progress:?}");
        let mut last = 0;
        for note in progress {
            assert_eq!(note["method"], "notifications/progress");
            assert!(note.get("id").is_none());
            assert_eq!(note["params"]["progressToken"], "tok-1");
            let now = note["params"]["progress"].as_u64().unwrap();
            assert!(now > last, "progress rises: {note}");
            last = now;
            assert_eq!(note["params"]["message"], "still waiting on 2 threads");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn any_pending_call_reports_progress_and_a_quick_one_does_not() {
        let params = json!({"name": "merge_when_green", "arguments": {"session_id": "d1"},
                            "_meta": {"progressToken": 5}});
        let (seen, _) = run_slow(params, Slow(Some(Duration::from_secs(45)))).await;
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0]["params"]["progressToken"], 5);
        assert_eq!(
            seen[0]["params"]["message"],
            "still waiting for the checks of d1"
        );

        let params = json!({"name": "wait_session", "arguments": {"link_id": "a/1"},
                            "_meta": {"progressToken": "t"}});
        let (seen, _) = run_slow(params, Slow(Some(Duration::from_secs(1)))).await;
        assert_eq!(
            seen.len(),
            1,
            "an instant answer sends no progress: {seen:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_without_a_progress_token_stays_silent() {
        let params = json!({"name": "wait_session",
                            "arguments": {"link_id": "a/1", "timeout_ms": 600_000}});
        let (seen, took) = run_slow(params, Slow(Some(Duration::from_secs(100)))).await;
        assert_eq!(took, Duration::from_secs(100));
        assert_eq!(seen.len(), 1, "only the result: {seen:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_answers_its_timeout_even_when_the_app_never_does() {
        // 5 s asked: the answer is out within a few seconds of it.
        let params = json!({"name": "wait_session",
                            "arguments": {"link_id": "a/1", "timeout_ms": 5_000}});
        let (seen, took) = run_slow(params, Slow(None)).await;
        assert!(took >= Duration::from_secs(5), "{took:?}");
        assert!(
            took <= Duration::from_secs(5) + Duration::from_secs(10),
            "{took:?}"
        );
        let result = &seen[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["status"], "timeout");
        assert!(text["note"].as_str().unwrap().contains("again"));
    }

    #[test]
    fn a_clock_that_ran_backwards_counts_as_the_deadline_passed() {
        let at = Duration::from_secs;
        let limit = at(30);
        assert!(!deadline_passed(at(100), at(100), limit));
        assert!(!deadline_passed(at(100), at(129), limit));
        assert!(deadline_passed(at(100), at(130), limit));
        // Set back by an hour: no way to tell how long it was, so it is over.
        assert!(deadline_passed(at(10_000), at(6_400), limit));
        assert!(deadline_passed(at(100), at(99), limit));
    }

    #[test]
    fn the_suspend_aware_clock_never_runs_backwards() {
        let a = suspend_aware_now();
        std::thread::sleep(Duration::from_millis(5));
        let b = suspend_aware_now();
        assert!(b > a, "{a:?} then {b:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_over_the_cap_is_cut_to_it() {
        // The 1 500 000 ms the live call asked for is the cap itself; 3 600 000 is cut to it.
        for asked in [1_500_000u64, 3_600_000] {
            let params = json!({"name": "wait_session",
                                "arguments": {"link_id": "a/1", "timeout_ms": asked}});
            let (_, took) = run_slow(params, Slow(None)).await;
            let cap = Duration::from_millis(super::super::MAX_WAIT_MS);
            assert!(
                took >= cap && took <= cap + Duration::from_secs(10),
                "{asked}: {took:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_merge_that_got_no_answer_says_it_is_pending() {
        let params = json!({"name": "merge_when_green",
                            "arguments": {"session_id": "d1", "timeout_ms": 3_300_000}});
        let (seen, took) = run_slow(params, Slow(None)).await;
        let cap = Duration::from_millis(super::super::MAX_MERGE_WAIT_MS);
        assert!(
            took >= cap && took <= cap + Duration::from_secs(190),
            "{took:?}"
        );
        let text: Value =
            serde_json::from_str(seen[0]["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(text["state"], "pending");
        assert_eq!(text["merged"], false);
    }

    #[tokio::test]
    async fn an_unreachable_engine_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let answers = exchange(
            Identity {
                session_id: "thread-1".into(),
                token: "t".into(),
            },
            dir.path().join("missing.sock"),
            &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                     "params": {"name": "list_sessions", "arguments": {}}})],
        )
        .await;
        assert!(
            answers[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("not reachable")
        );
    }
}
