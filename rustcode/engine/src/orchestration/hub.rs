//! The parent engine's end of `rustcode mcp-orchestrator`
//! (`docs/orchestrator.md`, External harnesses as orchestrators).
//!
//! An external harness thread gets the orchestration tools through an MCP
//! server the engine injects into it. That server runs as a child of the
//! harness, so it cannot reach the ACP client itself: it sends each call
//! here over a unix socket, with the thread's id and a per-thread token.
//! The hub checks both and forwards the call through the same
//! `_rustcode/orchestration` client request a built-in tool uses, so the
//! client's rules and approvals apply unchanged.
//!
//! The socket is `<config>/run/orchestrator-<pid>.sock`, mode 0600 in a
//! 0700 directory, and is bound the first time a thread registers.
//!
//! Wire format: one connection per call. The caller writes one JSON line
//! `{"session_id", "token", "tool", "args"}` and reads one JSON line
//! `{"success": bool, "content": "..."}`. Closing the connection early
//! cancels the call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::preview::PreviewClient;

/// The longest request line the hub reads: arguments are bounded well
/// below this by validation.
const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRequest {
    pub session_id: String,
    pub token: String,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    /// The call comes from an outside agent (`rustcode mcp-connect`), not from
    /// a thread's harness: `token` is its own grant's, `session_id` is unused,
    /// and the engine applies the grant's scope (`outside.rs`).
    #[serde(default)]
    pub outside: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallAnswer {
    pub success: bool,
    pub content: String,
}

impl CallAnswer {
    pub(crate) fn failure(content: impl Into<String>) -> Self {
        Self {
            success: false,
            content: content.into(),
        }
    }
}

/// How the engine starts `rustcode mcp-orchestrator` for one thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub command: PathBuf,
    pub args: Vec<String>,
    /// Passed in the environment (`ORCHESTRATOR_TOKEN_ENV`), never in
    /// `args`, so other local users cannot read it from the process list.
    pub token: String,
    /// The user has Smart UI on, so the server also offers `smart_ui_check`.
    pub smart_ui: bool,
    /// The harness has no question tool of its own, so the server also
    /// offers `ask_user_question` (#3393).
    pub ask_user: bool,
    /// The orchestration version the client advertises: which tools it can answer.
    pub version: u8,
}

/// The environment variable `rustcode mcp-orchestrator` reads its token from.
pub const ORCHESTRATOR_TOKEN_ENV: &str = "RUSTCODE_ORCHESTRATOR_TOKEN";

/// Set to `1` when the thread's client runs Smart UI blocks: the server then
/// also lists `smart_ui_check` (`smart_ui::CHECK_TOOL_NAME`).
pub const SMART_UI_ENV: &str = "RUSTCODE_SMART_UI";

/// Set to `1` for a harness with no question tool of its own: the server then
/// also lists `ask_user_question` (`ask_user::TOOL_NAME`, #3393).
pub const ASK_USER_ENV: &str = "RUSTCODE_ASK_USER";

/// The orchestration version the thread's client advertises. The server lists
/// that version's tools and instructions, so a client never sees a tool it
/// would answer with "unknown tool".
pub const ORCHESTRATION_VERSION_ENV: &str = "RUSTCODE_ORCHESTRATION_VERSION";

/// How long a harness should let one call run: the longest wait plus the
/// engine's own deadline for the app's answer and a minute of room.
pub const TOOL_TIMEOUT_SECS: u64 = (if super::MAX_WAIT_MS > super::MAX_MERGE_WAIT_MS {
    super::MAX_WAIT_MS
} else {
    super::MAX_MERGE_WAIT_MS
}) / 1000
    + super::MERGE_ANSWER_GRACE.as_secs()
    + 60;

/// The name the server has in a harness's MCP list.
pub const SERVER_NAME: &str = "rustcode";

/// In `<config>/run/`: which socket the running engine's hub listens on.
pub const PUBLISHED_FILE: &str = "orchestrator.json";

/// The socket the running engine published, when there is one.
pub fn published_socket() -> Option<PathBuf> {
    let path = crate::config::get_config_dir()?
        .join("run")
        .join(PUBLISHED_FILE);
    let body: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    body.get("socket")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

impl Launch {
    pub fn new(command: PathBuf, session_id: &str, socket: &Path, token: &str) -> Self {
        Self {
            command,
            args: vec![
                "mcp-orchestrator".to_owned(),
                "--session".to_owned(),
                session_id.to_owned(),
                "--socket".to_owned(),
                socket.to_string_lossy().into_owned(),
            ],
            token: token.to_owned(),
            smart_ui: false,
            ask_user: false,
            version: super::VERSION,
        }
    }

    /// Also offer `smart_ui_check`, for a thread whose client runs Smart UI blocks.
    pub fn with_smart_ui(mut self, on: bool) -> Self {
        self.smart_ui = on;
        self
    }

    /// Also offer `ask_user_question`, for a harness without a question tool.
    pub fn with_ask_user(mut self, on: bool) -> Self {
        self.ask_user = on;
        self
    }

    /// The orchestration version the client advertises.
    pub fn with_version(mut self, version: u8) -> Self {
        self.version = version;
        self
    }

    /// The settings the server reads, as environment pairs.
    fn env(&self) -> Vec<(&'static str, String)> {
        let mut env = vec![
            (ORCHESTRATOR_TOKEN_ENV, self.token.clone()),
            (ORCHESTRATION_VERSION_ENV, self.version.to_string()),
        ];
        if self.smart_ui {
            env.push((SMART_UI_ENV, "1".to_owned()));
        }
        if self.ask_user {
            env.push((ASK_USER_ENV, "1".to_owned()));
        }
        env
    }

    /// One ACP `McpServer` (stdio) entry for `session/new`'s `mcpServers`.
    pub fn acp_server(&self) -> Value {
        let env = self
            .env()
            .into_iter()
            .map(|(name, value)| serde_json::json!({ "name": name, "value": value }))
            .collect::<Vec<_>>();
        serde_json::json!({
            "name": SERVER_NAME,
            "command": self.command.to_string_lossy(),
            "args": self.args,
            "env": env,
        })
    }

    /// Codex `thread/start` / `thread/resume` `config` overrides that add
    /// the server for that thread only.
    pub fn codex_config(&self) -> Value {
        serde_json::json!({
            format!("mcp_servers.{SERVER_NAME}"): {
                "command": self.command.to_string_lossy(),
                "args": self.args,
                "env": self
                    .env()
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), serde_json::json!(value)))
                    .collect::<serde_json::Map<_, _>>(),
                // Codex stops an MCP call after 60 s by default; a call
                // can wait for the user, and `wait_session` up to an hour.
                "tool_timeout_sec": TOOL_TIMEOUT_SECS,
            }
        })
    }
}

struct Registered {
    token: String,
    /// The app client that answers the calls, when one hosts them. `None`
    /// means this engine answers itself (#3422).
    client: Option<Arc<dyn PreviewClient>>,
    /// The orchestration version this thread's client advertised. The socket
    /// carries calls for one thread, so its version travels with its token and
    /// governs what the hub may forward, whatever the server in front assumes.
    version: u8,
    /// How `ask_user_question` reaches the user: the thread's own permission
    /// channel, set for a harness without a question tool (#3393).
    questions: Option<super::ask_user::Asker>,
}

pub struct Hub {
    path: PathBuf,
    sessions: StdMutex<HashMap<String, Registered>>,
}

fn random_token() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}

fn same_token(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

impl Hub {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Let `session_id` call through `client`; returns its token. A thread
    /// registered again gets a new token, and its version replaces the old one.
    pub fn register(
        &self,
        session_id: &str,
        client: Arc<dyn PreviewClient>,
        version: u8,
    ) -> String {
        self.register_with(session_id, Some(client), version)
    }

    /// Let `session_id` call with the engine answering itself, for a thread
    /// whose app is not attached (#3422).
    pub fn register_local(&self, session_id: &str, version: u8) -> String {
        self.register_with(session_id, None, version)
    }

    /// Answer this thread's calls through `client` from now on, leaving its
    /// token alone: the `mcp-orchestrator` already running was started with
    /// that token, so an app that attaches later can take the calls over
    /// without restarting it (#3422). Whether the thread was registered.
    pub fn set_client(
        &self,
        session_id: &str,
        client: Arc<dyn PreviewClient>,
        version: u8,
    ) -> bool {
        let mut sessions = self.lock();
        let Some(registered) = sessions.get_mut(session_id) else {
            return false;
        };
        registered.client = Some(client);
        registered.version = version.min(super::VERSION);
        true
    }

    fn register_with(
        &self,
        session_id: &str,
        client: Option<Arc<dyn PreviewClient>>,
        version: u8,
    ) -> String {
        let token = random_token();
        let version = version.min(super::VERSION);
        self.lock().insert(
            session_id.to_owned(),
            Registered {
                token: token.clone(),
                client: client.clone(),
                version,
                questions: None,
            },
        );
        // A workflow run this thread left running when the engine stopped
        // continues from its first unfinished call (#3394).
        if version >= 2 {
            crate::workflow::resume(
                session_id,
                crate::workflow::ThreadRunner::new(session_id, client, version),
            );
        }
        token
    }

    /// Let `session_id` ask the user with `ask_user_question` through
    /// `asker`. Whether the thread was registered.
    pub fn set_questions(&self, session_id: &str, asker: super::ask_user::Asker) -> bool {
        let mut sessions = self.lock();
        let Some(registered) = sessions.get_mut(session_id) else {
            return false;
        };
        registered.questions = Some(asker);
        true
    }

    /// The thread's question channel for a call, when its session and token
    /// match and it has one.
    fn questions_for(&self, request: &CallRequest) -> Option<super::ask_user::Asker> {
        let sessions = self.lock();
        let registered = sessions.get(&request.session_id)?;
        same_token(&registered.token, &request.token)
            .then(|| registered.questions.clone())
            .flatten()
    }

    pub fn unregister(&self, session_id: &str) {
        self.lock().remove(session_id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Registered>> {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// The client (when one hosts the tools) and its version for a call, when
    /// its session and token match.
    fn authorize(&self, request: &CallRequest) -> Option<(Option<Arc<dyn PreviewClient>>, u8)> {
        let sessions = self.lock();
        let registered = sessions.get(&request.session_id)?;
        same_token(&registered.token, &request.token).then(|| {
            (
                registered.client.as_ref().map(Arc::clone),
                registered.version,
            )
        })
    }

    /// Answer one decoded call.
    pub async fn answer(
        &self,
        request: CallRequest,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> CallAnswer {
        // An outside agent holds a grant of its own, not a thread's token: the
        // gate checks it, its scope and the user's approval before anything runs.
        if request.outside {
            return super::outside::answer(request, cancel).await;
        }
        let Some((client, version)) = self.authorize(&request) else {
            return CallAnswer::failure(
                "error: RustCode did not accept this orchestrator's session or token. Nothing was sent.",
            );
        };
        if request.tool == super::ask_user::TOOL_NAME {
            return self.ask_user(&request, cancel).await;
        }
        if crate::workflow::is_tool(&request.tool) {
            return workflow_call(&request, client, version, cancel).await;
        }
        if !super::is_tool(&request.tool) {
            return CallAnswer::failure(format!(
                "error: {} is not an orchestration tool",
                request.tool
            ));
        }
        let expanded = match super::with_preset(&request.tool, &request.args) {
            Ok(expanded) => expanded,
            Err(refusal) => {
                return CallAnswer {
                    success: refusal.success,
                    content: refusal.content,
                };
            }
        };
        let args = expanded.as_ref().unwrap_or(&request.args);
        let output = match client {
            Some(client) => {
                super::execute_versioned(Some(client), version, cancel, &request.tool, args).await
            }
            None => super::local::answer(&request.session_id, cancel, &request.tool, args).await,
        };
        let output = super::with_preset_listing(&request.tool, output);
        CallAnswer {
            success: output.success,
            content: output.content,
        }
    }
}

/// `run_workflow` and `workflow_status` (#3394): the engine answers them, and
/// a run's workers are delegates of the calling thread, started through the
/// same client (or the engine's own answer) as its other orchestration calls.
async fn workflow_call(
    request: &CallRequest,
    client: Option<Arc<dyn PreviewClient>>,
    version: u8,
    cancel: &tokio_util::sync::CancellationToken,
) -> CallAnswer {
    if version < 2 {
        return CallAnswer::failure(format!("error: unknown tool '{}'", request.tool));
    }
    let outcome = if request.tool == crate::workflow::RUN_TOOL_NAME {
        let runner = crate::workflow::ThreadRunner::new(&request.session_id, client, version);
        crate::workflow::start(&request.session_id, runner, &request.args)
    } else {
        crate::workflow::status(&request.session_id, &request.args, cancel).await
    };
    match outcome {
        Ok(content) => CallAnswer {
            success: true,
            content,
        },
        Err(reason) => CallAnswer::failure(format!("error: {} failed: {reason}", request.tool)),
    }
}

impl Hub {
    /// `ask_user_question`: ask the thread's client and wait for the answer.
    async fn ask_user(
        &self,
        request: &CallRequest,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> CallAnswer {
        let Some(asker) = self.questions_for(request) else {
            return CallAnswer::failure(format!(
                "error: unknown tool '{}': this thread asks with its own question tool",
                request.tool
            ));
        };
        let prompts = match super::ask_user::validate(&request.args) {
            Ok(prompts) => prompts,
            Err(reason) => {
                return CallAnswer::failure(format!(
                    "error: invalid {} call: {reason}. Nothing was asked.",
                    request.tool
                ));
            }
        };
        match super::ask_user::ask(&asker, &prompts, cancel).await {
            Ok(content) => CallAnswer {
                success: true,
                content,
            },
            Err(content) => CallAnswer::failure(content),
        }
    }
}

#[cfg(unix)]
mod listen {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};

    impl Hub {
        /// Bind the socket at `path` (its directory made 0700, the socket
        /// 0600) and serve it until the process exits.
        pub async fn bind(path: PathBuf) -> std::io::Result<Arc<Self>> {
            use std::os::unix::fs::PermissionsExt;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                prune_stale(dir);
            }
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            publish(&path);
            let hub = Arc::new(Self {
                path,
                sessions: StdMutex::new(HashMap::new()),
            });
            let serving = Arc::clone(&hub);
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let hub = Arc::clone(&serving);
                            tokio::spawn(async move { hub.serve(stream).await });
                        }
                        Err(error) => {
                            crate::dbg_log!("orchestrator socket accept failed: {error}");
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        }
                    }
                }
            });
            Ok(hub)
        }

        async fn serve(self: Arc<Self>, stream: UnixStream) {
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);
            let mut line = Vec::new();
            let read_line = (&mut reader)
                .take(MAX_REQUEST_BYTES as u64 + 1)
                .read_until(b'\n', &mut line)
                .await;
            let answer = match read_line {
                Ok(_) if line.len() > MAX_REQUEST_BYTES => {
                    CallAnswer::failure("error: the orchestration call is too large")
                }
                Ok(_) => match serde_json::from_slice::<CallRequest>(&line) {
                    Ok(request) => {
                        let cancel = tokio_util::sync::CancellationToken::new();
                        // The caller closing its end cancels the call.
                        let watch = {
                            let cancel = cancel.clone();
                            tokio::spawn(async move {
                                let mut rest = [0u8; 64];
                                while matches!(reader.read(&mut rest).await, Ok(n) if n > 0) {}
                                cancel.cancel();
                            })
                        };
                        let answer = self.answer(request, &cancel).await;
                        watch.abort();
                        answer
                    }
                    Err(error) => {
                        CallAnswer::failure(format!("error: bad orchestration call: {error}"))
                    }
                },
                Err(error) => CallAnswer::failure(format!("error: {error}")),
            };
            let mut text = serde_json::to_vec(&answer).unwrap_or_default();
            text.push(b'\n');
            let _ = write.write_all(&text).await;
            let _ = write.shutdown().await;
        }
    }

    /// Say where this engine's hub listens, for `rustcode mcp-connect`, which
    /// starts outside any thread and so has no socket path handed to it. The
    /// last engine to bind wins; a stale file only makes the bridge's connect
    /// fail with a message that RustCode is not running.
    fn publish(socket: &Path) {
        let Some(dir) = socket.parent() else {
            return;
        };
        let body = serde_json::json!({
            "pid": std::process::id(),
            "socket": socket.to_string_lossy(),
        });
        let _ = crate::service::write_private(
            &dir.join(super::PUBLISHED_FILE),
            body.to_string().as_bytes(),
        );
    }

    /// Remove sockets of engines that are no longer running.
    fn prune_stale(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .and_then(|name| name.strip_prefix("orchestrator-"))
                .and_then(|name| name.strip_suffix(".sock"))
                .and_then(|pid| pid.parse::<i32>().ok())
            else {
                continue;
            };
            if pid <= 0 || pid as u32 == std::process::id() {
                continue;
            }
            // SAFETY: signal 0 only checks whether the process exists.
            let alive = unsafe { libc::kill(pid, 0) } == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
            if !alive {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Send one call to the hub at `socket` and read its answer.
    pub async fn call(socket: &Path, request: &CallRequest) -> CallAnswer {
        let mut stream = match UnixStream::connect(socket).await {
            Ok(stream) => stream,
            Err(error) => {
                return CallAnswer::failure(format!(
                    "error: RustCode is not reachable ({error}); the thread that started this harness may have closed. Nothing was sent."
                ));
            }
        };
        let mut line = serde_json::to_vec(request).unwrap_or_default();
        line.push(b'\n');
        if let Err(error) = stream.write_all(&line).await {
            return CallAnswer::failure(format!("error: could not reach RustCode: {error}"));
        }
        let mut answer = Vec::new();
        if let Err(error) = BufReader::new(stream).read_until(b'\n', &mut answer).await {
            return CallAnswer::failure(format!("error: RustCode did not answer: {error}"));
        }
        serde_json::from_slice(&answer).unwrap_or_else(|_| {
            CallAnswer::failure(format!(
                "error: RustCode closed the call without an answer; if it was a message, it may have been delivered{}",
                super::super::retry_hint(&request.tool, &request.args)
            ))
        })
    }
}

#[cfg(unix)]
pub use listen::call;

#[cfg(unix)]
static HUB: tokio::sync::OnceCell<Arc<Hub>> = tokio::sync::OnceCell::const_new();

/// The process's hub, bound on first use.
#[cfg(unix)]
pub async fn shared() -> Result<Arc<Hub>, String> {
    HUB.get_or_try_init(|| async {
        let dir = crate::harness::config::config_dir()?.join("run");
        let path = dir.join(format!("orchestrator-{}.sock", std::process::id()));
        Hub::bind(path)
            .await
            .map_err(|error| format!("orchestrator socket: {error}"))
    })
    .await
    .cloned()
}

/// Register an external harness thread with the process's hub, binding it
/// on first use, and say how to start its MCP server. `None` when the
/// platform or the socket cannot carry it.
pub async fn launch_for(
    session_id: &str,
    client: Arc<dyn PreviewClient>,
    version: u8,
    smart_ui: crate::app::SmartUi,
) -> Option<Launch> {
    launch_with(session_id, Some(client), version, smart_ui).await
}

/// Let a registered thread ask with `ask_user_question` through `asker`
/// (#3393). `false` when no server was started for it.
pub async fn set_questions(session_id: &str, asker: super::ask_user::Asker) -> bool {
    #[cfg(unix)]
    {
        let Ok(hub) = shared().await else {
            return false;
        };
        hub.set_questions(session_id, asker)
    }
    #[cfg(not(unix))]
    {
        let _ = (session_id, asker);
        false
    }
}

/// [`launch_for`] for a thread whose app is not attached: the engine answers
/// the calls itself (#3422).
pub async fn launch_local_for(
    session_id: &str,
    version: u8,
    smart_ui: crate::app::SmartUi,
) -> Option<Launch> {
    launch_with(session_id, None, version, smart_ui).await
}

async fn launch_with(
    session_id: &str,
    client: Option<Arc<dyn PreviewClient>>,
    version: u8,
    smart_ui: crate::app::SmartUi,
) -> Option<Launch> {
    #[cfg(unix)]
    {
        let hub = match shared().await {
            Ok(hub) => hub,
            Err(error) => {
                crate::dbg_log!("orchestrator unavailable: {error}");
                return None;
            }
        };
        let command = std::env::current_exe().ok()?;
        let token = match client {
            Some(client) => hub.register(session_id, client, version),
            None => hub.register_local(session_id, version),
        };
        Some(
            Launch::new(command, session_id, hub.path(), &token)
                .with_smart_ui(smart_ui == crate::app::SmartUi::On)
                .with_version(version),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (session_id, client, version, smart_ui);
        None
    }
}

/// Let an app that just attached to `session_id` answer its calls, keeping
/// the token its `mcp-orchestrator` already has. `false` when the thread is
/// not registered (no server was started for it).
pub async fn swap_client(session_id: &str, client: Arc<dyn PreviewClient>, version: u8) -> bool {
    #[cfg(unix)]
    {
        let Ok(hub) = shared().await else {
            return false;
        };
        hub.set_client(session_id, client, version)
    }
    #[cfg(not(unix))]
    {
        let _ = (session_id, client, version);
        false
    }
}

/// Forget a closed thread's registration.
pub fn release(session_id: &str) {
    #[cfg(unix)]
    {
        // Only a hub that was bound has registrations; never bind here.
        if let Some(hub) = HUB.get() {
            hub.unregister(session_id);
        }
    }
    #[cfg(not(unix))]
    let _ = session_id;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::preview::PreviewFuture;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The app: counts what reached it, then answers like a started thread.
    struct Counted(Arc<AtomicUsize>);

    impl PreviewClient for Counted {
        fn request(&self, _params: Value) -> PreviewFuture {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(json!({ "session_id": "child" })) })
        }
    }

    /// A call for a version 4 thread that names a login: the hub, not the
    /// server in front, must refuse it with the version the thread registered.
    #[tokio::test]
    async fn the_hub_forwards_with_the_version_the_thread_registered() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::bind(dir.path().join("run").join("orchestrator-test.sock"))
            .await
            .unwrap();
        let asked = Arc::new(AtomicUsize::new(0));
        let request = |token: String| CallRequest {
            session_id: "thread-1".to_owned(),
            token,
            tool: super::super::START_SESSION.to_owned(),
            args: json!({ "prompt": "go", "account": "work" }),
            outside: false,
        };
        let token = hub.register("thread-1", Arc::new(Counted(Arc::clone(&asked))), 4);
        let refused = super::call(hub.path(), &request(token)).await;
        assert!(!refused.success);
        assert!(refused.content.contains("newer RustCode app"));
        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "a v4 call never reaches the app"
        );

        // The same thread after its client advertises 5: the same call goes through.
        let token = hub.register("thread-1", Arc::new(Counted(Arc::clone(&asked))), 5);
        let sent = super::call(hub.path(), &request(token)).await;
        assert!(sent.success, "{}", sent.content);
        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }

    /// A thread whose app is not attached: the engine answers the reads and
    /// names the app for a call that needs it, instead of "unknown tool".
    #[tokio::test]
    async fn the_hub_answers_locally_when_no_client_hosts_the_tools() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::bind(dir.path().join("run").join("orchestrator-local.sock"))
            .await
            .unwrap();
        let token = hub.register_local("thread-1", super::super::VERSION);
        let call = |tool: &str, args: Value| CallRequest {
            session_id: "thread-1".to_owned(),
            token: token.clone(),
            tool: tool.to_owned(),
            args,
            outside: false,
        };
        let listed = super::call(hub.path(), &call(super::super::LIST_MACHINES, json!({}))).await;
        assert!(listed.success, "{}", listed.content);
        let value: Value = serde_json::from_str(&listed.content).unwrap();
        assert_eq!(value["current"], json!("local"));

        let sent = super::call(
            hub.path(),
            &call(
                super::super::MESSAGE_SESSION,
                json!({"session_id": "thread-2", "message": "go"}),
            ),
        )
        .await;
        assert!(!sent.success);
        assert!(
            sent.content.contains("RustCode app open"),
            "{}",
            sent.content
        );
    }

    /// An app that attaches to a locally registered thread takes its calls
    /// over without a new token: the `mcp-orchestrator` already running keeps
    /// working (#3422).
    #[tokio::test]
    async fn an_app_attaching_takes_over_without_changing_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::bind(dir.path().join("run").join("orchestrator-swap.sock"))
            .await
            .unwrap();
        let token = hub.register_local("thread-1", super::super::VERSION);
        let asked = Arc::new(AtomicUsize::new(0));
        assert!(hub.set_client(
            "thread-1",
            Arc::new(Counted(Arc::clone(&asked))),
            super::super::VERSION,
        ));
        let sent = super::call(
            hub.path(),
            &CallRequest {
                session_id: "thread-1".to_owned(),
                token,
                tool: super::super::LIST_SESSIONS.to_owned(),
                args: json!({}),
                outside: false,
            },
        )
        .await;
        assert!(sent.success, "{}", sent.content);
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "the app answered with the old token"
        );
        // A thread with no registration is not made orchestrated by an attach.
        assert!(!hub.set_client(
            "unknown",
            Arc::new(Counted(Arc::new(AtomicUsize::new(0)))),
            super::super::VERSION,
        ));
    }
}
