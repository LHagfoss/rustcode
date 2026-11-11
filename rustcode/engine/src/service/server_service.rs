//! The RustCode server as a background service: `rustcode server install`,
//! `status`, `restart`, `uninstall` and `logs` (`docs/server.md`).
//!
//! What can be decided without the machine is plain functions
//! ([`service_name`], [`definition`], [`build_report`], [`plan_restart`]),
//! and the waiting for an idle server takes its observations as a closure
//! ([`wait_until_idle`]), so all of it is tested against fakes. Nothing here
//! runs from a test.

use super::path_binary::PathRustcode;
use super::{Definition, Installed, Manager, Problem, Process, State};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long `install` and `restart` wait for the server to listen.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a request to the running server may take.
const ASK_TIMEOUT: Duration = Duration::from_secs(5);
/// The ACP extension method that answers [`LiveStatus`].
pub const STATUS_METHOD: &str = "_rustcode/server/status";

/// The service's name: `server`, or `server-<hash of the directory>` for a
/// server of another `RUSTCODE_CONFIG_DIR`, so that one never replaces the
/// definition of the server of the default directory.
pub fn service_name(config_override: Option<&Path>) -> String {
    use sha2::{Digest, Sha256};
    match config_override {
        Some(dir) if !dir.as_os_str().is_empty() => format!(
            "server-{}",
            hex::encode(&Sha256::digest(dir.as_os_str().as_encoded_bytes())[..4])
        ),
        _ => "server".to_owned(),
    }
}

fn config_override() -> Option<PathBuf> {
    std::env::var_os("RUSTCODE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// What the operating system runs: `program server run --service`, with the
/// configuration directory it was installed for when that is not the default.
pub fn definition(program: &Path, config_override: Option<&Path>, log: PathBuf) -> Definition {
    Definition {
        name: service_name(config_override),
        description: "RustCode server".to_owned(),
        program: program.to_owned(),
        args: ["server", "run", "--service"].map(str::to_owned).to_vec(),
        env: config_override
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(|dir| {
                vec![(
                    "RUSTCODE_CONFIG_DIR".to_owned(),
                    dir.to_string_lossy().into_owned(),
                )]
            })
            .unwrap_or_default(),
        log,
    }
}

fn current_manager() -> Result<(Box<dyn Manager>, Definition)> {
    let program = super::executable_path()?;
    let log = crate::server::log_path()?;
    let overridden = config_override();
    let definition = definition(&program, overridden.as_deref(), log);
    Ok((super::manager(&definition.name)?, definition))
}

/// What a running server says of itself (`_rustcode/server/status`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LiveStatus {
    pub version: String,
    pub pid: u32,
    /// Connected clients that said `initialize`.
    pub clients: usize,
    /// Threads with a turn running.
    pub running_turns: usize,
}

/// Ask a server on `stream` for its status. The reply is read past any
/// notification the server sends first.
pub async fn ask_status<S>(stream: S) -> std::io::Result<LiveStatus>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (read, mut write) = tokio::io::split(stream);
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": STATUS_METHOD, "params": {}
    });
    write.write_all(format!("{request}\n").as_bytes()).await?;
    write.flush().await?;
    let mut lines = tokio::io::BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if message.get("id") != Some(&serde_json::json!(1)) || message.get("method").is_some() {
            continue;
        }
        if let Some(error) = message.get("error") {
            return Err(std::io::Error::other(format!(
                "the server refused the status request: {error}"
            )));
        }
        return serde_json::from_value(message["result"].clone())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error));
    }
    Err(std::io::ErrorKind::UnexpectedEof.into())
}

/// The running server's status, or `None` when none answers.
pub async fn live_status() -> Option<LiveStatus> {
    let ask = async {
        let stream = crate::server::connect_running().await.ok()?;
        ask_status(stream).await.ok()
    };
    tokio::time::timeout(ASK_TIMEOUT, ask).await.ok().flatten()
}

/// Whether a service definition is in place for this configuration
/// directory. Reads the manager's state and nothing else, so a caller that
/// needs a yes/no (does the RustCode server host the remote gateway?) can ask
/// it on every start.
pub fn installed() -> bool {
    current_manager()
        .and_then(|(manager, _)| manager.state())
        .map(|state| state.installed)
        .unwrap_or(false)
}

/// One problem `server status` can report.
fn problem(code: &'static str, message: impl Into<String>) -> Problem {
    Problem {
        code,
        message: message.into(),
    }
}

/// What `server status` knows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StatusReport {
    /// A definition is in place.
    pub installed: bool,
    /// A server answers on its socket.
    pub running: bool,
    /// The running server's version.
    pub version: Option<String>,
    /// This `rustcode`'s version.
    pub cli_version: String,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    /// The Unix socket's path, or the Windows pipe's name.
    pub socket: Option<String>,
    /// The program the installed service runs.
    pub binary: Option<String>,
    pub service: ServiceReport,
    pub clients: Option<usize>,
    pub running_turns: Option<usize>,
    /// This machine's remote gateway: whether another machine can reach it
    /// with the app closed, and who started it.
    pub gateway: GatewayReport,
    /// The `rustcode` a shell runs, when `PATH` has one (issue #3411).
    pub path_rustcode: Option<PathRustcode>,
    pub log: String,
    pub problems: Vec<Problem>,
}

/// This machine's remote gateway (`docs/remote.md`), as `server status` shows
/// it: whether another machine can reach this one, where, and who started it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct GatewayReport {
    /// Remote access is on for this machine.
    pub remote_enabled: bool,
    /// A gateway process answers.
    pub running: bool,
    /// What devices dial, while it runs.
    pub address: Option<String>,
    pub advertised: Option<String>,
    /// Who started it: `server`, `app` or `terminal`.
    pub owner: Option<String>,
    pub version: Option<String>,
    pub pid: Option<u32>,
    pub paired_devices: usize,
    /// The port its recorded settings name, while it does not run.
    pub port: Option<u16>,
}

impl GatewayReport {
    /// One `Gateway` line in `rustcode server status`. `server_installed` makes
    /// the difference between "the server starts it again" and "nothing will
    /// until you do": with no server installed nothing restarts it.
    pub fn human(&self, server_installed: bool) -> String {
        if self.running {
            let mut out = format!(
                "running on {}",
                self.address.as_deref().unwrap_or("an unknown address")
            );
            if let Some(owner) = self.owner.as_deref() {
                out.push_str(&format!(" (started by {}", owner_words(owner)));
                if let Some(version) = self.version.as_deref() {
                    out.push_str(&format!(", version {version}"));
                }
                out.push(')');
            }
            out.push_str(&format!(
                ", {} paired {}",
                self.paired_devices,
                if self.paired_devices == 1 {
                    "device"
                } else {
                    "devices"
                }
            ));
            return out;
        }
        if !self.remote_enabled {
            return "stopped (remote access is off: `rustcode remote start`, or Settings › Remote access)".to_owned();
        }
        // Who starts it again: only an installed server does, so only then may
        // the line say so.
        let what = if server_installed {
            "the RustCode server starts it"
        } else {
            "nothing starts it until `rustcode remote start` (no server is installed)"
        };
        match self.port {
            Some(port) => format!("stopped (remote access is on; {what} on port {port})"),
            None => format!("stopped (remote access is on; {what})"),
        }
    }
}

/// `server`, `app` or `terminal` in the words the user reads.
fn owner_words(owner: &str) -> &'static str {
    match owner {
        "server" => "the RustCode server",
        "app" => "the app",
        _ => "a terminal",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServiceReport {
    /// `launchd`, `systemd`, `scheduled-task` or `run-key`.
    pub kind: String,
    /// The definition's file, when it is one.
    pub path: Option<String>,
}

/// Everything [`build_report`] needs, gathered by [`status`].
pub struct Observed {
    pub kind: String,
    pub state: State,
    /// From a socket that answered.
    pub live: Option<LiveStatus>,
    /// From the description the server published.
    pub info: Option<crate::server::ServerInfo>,
    /// A `restart --when-idle` is waiting.
    pub restart_waiting: bool,
    /// The manager's own problems (lingering).
    pub manager_problems: Vec<Problem>,
    pub cli_version: String,
    /// The engine this `rustcode` is (`executable_path`).
    pub cli_program: PathBuf,
    pub gateway: GatewayReport,
    pub path_rustcode: Option<PathRustcode>,
    pub log: PathBuf,
}

/// A server answers that the manager did not start: the manager runs no
/// process of its own, or another one. `Unknown` says nothing.
fn is_unmanaged(process: Process, live_pid: u32) -> bool {
    match process {
        Process::Unknown => false,
        Process::Stopped => true,
        Process::Running(managed) => managed != live_pid,
    }
}

/// Turn what was observed into the report, and name what is wrong.
pub fn build_report(observed: Observed, now_secs: u64) -> StatusReport {
    let Observed {
        kind,
        state,
        live,
        info,
        restart_waiting,
        manager_problems,
        cli_version,
        cli_program,
        gateway,
        path_rustcode,
        log,
    } = observed;
    let mut problems = Vec::new();
    if !state.installed {
        problems.push(problem(
            "not-installed",
            "The server is not installed as a background service: threads stop with the app. Run `rustcode server install`.",
        ));
    } else {
        problems.extend(manager_problems);
        if live.is_none() {
            problems.push(problem(
                "stopped",
                "The service is installed but no server answers. See `rustcode server logs`, or run `rustcode server restart`.",
            ));
        }
    }
    if let Some(live) = &live {
        if live.version != cli_version {
            problems.push(problem(
                "version-mismatch",
                format!(
                    "The server runs {} and this rustcode is {cli_version}. `rustcode server restart --when-idle` brings the server up to date.",
                    live.version
                ),
            ));
        }
        if state.installed && is_unmanaged(state.process, live.pid) {
            problems.push(problem(
                "unmanaged",
                format!(
                    "The server answering (pid {}) was not started by the service. `rustcode server restart` hands it over.",
                    live.pid
                ),
            ));
        }
    }
    if restart_waiting {
        problems.push(problem(
            "restart-pending",
            "A restart is waiting for running turns to finish.",
        ));
    }
    // Which `rustcode` a shell runs, against the engine that answered
    // (issue #3411): an older copy on PATH makes `server install` and version
    // checks between machines use the wrong binary. Each engine is compared
    // with its own path and its own version — the app's engine, then the
    // running server when it is another build — so a message never names one
    // binary's path with the other's version.
    if let Some(found) = &path_rustcode {
        if let Some(message) = super::path_binary::mismatch(found, &cli_program, &cli_version) {
            problems.push(problem("path-version-mismatch", message));
        }
        if let Some(live) = &live
            && live.version != cli_version
        {
            let server_binary = state.program.clone().unwrap_or_else(|| cli_program.clone());
            if let Some(message) =
                super::path_binary::mismatch(found, &server_binary, &live.version)
            {
                problems.push(problem("path-version-mismatch-server", message));
            }
        }
    }
    // The description a crashed server left behind says nothing.
    let info = info.filter(|info| live.as_ref().is_some_and(|live| live.pid == info.pid));
    StatusReport {
        installed: state.installed,
        running: live.is_some(),
        version: live.as_ref().map(|live| live.version.clone()),
        cli_version,
        pid: live.as_ref().map(|live| live.pid),
        uptime_secs: info
            .as_ref()
            .map(|info| now_secs.saturating_sub(info.started_at)),
        socket: info.map(|info| info.endpoint),
        binary: state
            .program
            .map(|program| program.to_string_lossy().into_owned()),
        service: ServiceReport {
            kind,
            path: state.path.map(|path| path.to_string_lossy().into_owned()),
        },
        clients: live.as_ref().map(|live| live.clients),
        running_turns: live.as_ref().map(|live| live.running_turns),
        gateway,
        path_rustcode,
        log: log.to_string_lossy().into_owned(),
        problems,
    }
}

impl StatusReport {
    /// The report as `rustcode server status` prints it.
    pub fn human(&self) -> String {
        let mut out = String::new();
        let line = |out: &mut String, label: &str, value: String| {
            out.push_str(&format!("{label:<12}{value}\n"));
        };
        line(
            &mut out,
            "Installed",
            if self.installed {
                format!("yes ({})", self.service.kind)
            } else {
                "no".to_owned()
            },
        );
        line(
            &mut out,
            "Running",
            match (&self.version, self.pid) {
                (Some(version), Some(pid)) => format!("yes (version {version}, pid {pid})"),
                _ => "no".to_owned(),
            },
        );
        if let Some(uptime) = self.uptime_secs {
            line(&mut out, "Uptime", human_duration(uptime));
        }
        if let Some(socket) = &self.socket {
            line(&mut out, "Socket", socket.clone());
        }
        if let Some(binary) = &self.binary {
            line(&mut out, "Binary", binary.clone());
        }
        line(&mut out, "Gateway", self.gateway.human(self.installed));
        if let Some(clients) = self.clients {
            line(&mut out, "Clients", clients.to_string());
        }
        if let Some(turns) = self.running_turns {
            line(&mut out, "Turns", format!("{turns} running"));
        }
        line(&mut out, "Log", self.log.clone());
        for problem in &self.problems {
            out.push_str(&format!(
                "Problem     {}: {}\n",
                problem.code, problem.message
            ));
        }
        out
    }
}

fn human_duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        3600..86400 => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d {}h", secs / 86400, secs % 86400 / 3600),
    }
}

/// Look at the service and the server.
pub async fn status() -> Result<StatusReport> {
    let (manager, _) = current_manager()?;
    let state = manager.state()?;
    let live = live_status().await;
    let gateway = observe_gateway().await;
    let path_rustcode = super::path_binary::observe().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    Ok(build_report(
        Observed {
            kind: manager.kind().to_owned(),
            manager_problems: if state.installed {
                manager.problems()
            } else {
                Vec::new()
            },
            state,
            live,
            info: crate::server::running(),
            restart_waiting: restart_waiting(),
            cli_version: env!("CARGO_PKG_VERSION").to_owned(),
            cli_program: super::executable_path()?,
            gateway,
            path_rustcode,
            log: crate::server::log_path()?,
        },
        now,
    ))
}

/// This machine's remote gateway, if remote access was ever set up here. A
/// gateway that does not answer within [`ASK_TIMEOUT`] counts as stopped:
/// `server status` must not hang on it.
async fn observe_gateway() -> GatewayReport {
    let Some(config_directory) = crate::config::get_config_dir() else {
        return GatewayReport::default();
    };
    let lifecycle = crate::remote_gateway::lifecycle::RemoteLifecycle::new(&config_directory);
    let settings = lifecycle.settings().ok().flatten();
    let port = settings.as_ref().and_then(|settings| settings.port);
    let status = tokio::time::timeout(ASK_TIMEOUT, lifecycle.status())
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    GatewayReport {
        remote_enabled: crate::remote_gateway::service::enabled(&config_directory),
        running: status.is_some(),
        address: status.as_ref().map(|status| status.listen_address.clone()),
        advertised: status
            .as_ref()
            .map(|status| status.advertised_address.clone()),
        owner: status
            .as_ref()
            .map(|status| status.started_by.as_env().to_owned()),
        version: status.as_ref().map(|status| status.version.clone()),
        pid: status.as_ref().map(|status| status.pid),
        paired_devices: status
            .as_ref()
            .map(|status| status.paired_devices)
            .unwrap_or(0),
        port,
    }
}

/// What `install` did.
#[derive(Debug)]
pub struct InstallReport {
    pub kind: &'static str,
    pub program: PathBuf,
    pub outcome: Installed,
    /// The server that answered once installed.
    pub live: Option<LiveStatus>,
    /// What the user should still do, in order.
    pub problems: Vec<Problem>,
}

/// `rustcode server install`: write the definition, start the service and
/// wait for the server to listen. A definition that is already as wanted
/// changes nothing.
///
/// A server that answers but is not the service's (an app's, or one run in
/// a terminal) holds the server lock, so the service's own would find it,
/// exit cleanly and never be started again. `install` stops it first, as
/// `restart` does, and asks for `yes` when it has turns running.
pub async fn install(force_dev: bool, yes: bool) -> Result<InstallReport> {
    let (manager, definition) = current_manager()?;
    preflight(manager.as_ref(), &definition, force_dev)?;
    let state = manager.state()?;
    let before = live_status().await;
    match plan_takeover(&state, before.as_ref(), yes) {
        Takeover::Leave => {}
        Takeover::Confirm { running } => anyhow::bail!(
            "a server that is not the service's is running {running} {}, and installing stops it; \
             wait for them to finish or pass --yes",
            if running == 1 { "turn" } else { "turns" }
        ),
        Takeover::Stop { pid } => {
            terminate(pid)?;
            wait_until_gone(pid).await;
        }
    }
    let outcome = manager.install(&definition)?;
    let live = wait_for_server(None).await;
    let mut problems = manager.problems();
    if outcome == Installed::Started && live.is_none() {
        problems.push(problem(
            "stopped",
            format!(
                "The service was installed but the server did not answer within {}s. See `rustcode server logs`.",
                START_TIMEOUT.as_secs()
            ),
        ));
    }
    if outcome == Installed::ChangedWhileActive {
        problems.push(problem(
            "restart-pending",
            "The service definition changed. The running server keeps the old one until `rustcode server restart --when-idle`.",
        ));
    }
    if let Some(live) = &live
        && is_unmanaged(manager.state()?.process, live.pid)
    {
        problems.push(problem(
            "unmanaged",
            format!(
                "The server answering (pid {}) was not started by the service. `rustcode server restart` hands it over.",
                live.pid
            ),
        ));
    }
    Ok(InstallReport {
        kind: manager.kind(),
        program: definition.program,
        outcome,
        live,
        problems,
    })
}

/// Everything that can refuse an install, before a running server is stopped
/// for it: a development build, or a definition the manager cannot take.
fn preflight(manager: &dyn Manager, definition: &Definition, force_dev: bool) -> Result<()> {
    super::refuse_dev_binary(&definition.program, force_dev)?;
    manager.check(definition)
}

/// What `install` does about a server that already answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Takeover {
    /// Nothing answers, or the service's own does.
    Leave,
    /// Stop the server (not the service's) so the service can start.
    Stop { pid: u32 },
    /// Turns are running on it: ask before ending them.
    Confirm { running: usize },
}

/// Decide what `install` does about `live`, the server answering before it.
pub fn plan_takeover(state: &State, live: Option<&LiveStatus>, yes: bool) -> Takeover {
    let Some(live) = live else {
        return Takeover::Leave;
    };
    // A manager that cannot say what it runs (Windows) is believed once the
    // service is installed; before that nothing of its own can be running.
    let stray = match state.process {
        Process::Unknown => !state.installed,
        Process::Stopped => true,
        Process::Running(managed) => managed != live.pid,
    };
    match stray {
        false => Takeover::Leave,
        true if live.running_turns > 0 && !yes => Takeover::Confirm {
            running: live.running_turns,
        },
        true => Takeover::Stop { pid: live.pid },
    }
}

/// Wait for a server to answer, a different one than `not_pid` when given.
async fn wait_for_server(not_pid: Option<u32>) -> Option<LiveStatus> {
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        if let Some(live) = live_status().await
            && Some(live.pid) != not_pid
        {
            return Some(live);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// What `restart` does, given what the user asked and what runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPlan {
    /// Restart now.
    Now,
    /// Turns are running: ask before ending them.
    Confirm { running: usize },
    /// Wait for no turn to run, then restart.
    WaitForIdle,
}

/// `running` is the turns running, or `None` when no server answers.
pub fn plan_restart(when_idle: bool, yes: bool, running: Option<usize>) -> RestartPlan {
    match running {
        _ if when_idle => RestartPlan::WaitForIdle,
        Some(running) if running > 0 && !yes => RestartPlan::Confirm { running },
        _ => RestartPlan::Now,
    }
}

/// How waiting for an idle server ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idle {
    /// No turn is running.
    Idle,
    /// The server stopped answering; there is nothing to wait for.
    ServerGone,
    /// Turns were still running at the deadline.
    TimedOut { running: usize },
}

/// Poll `running_turns` until it has said `0` `settled` times in a row (a
/// queued prompt can start right after a turn ends). `None` from it means no
/// server answers. `progress` hears every change of the count.
pub async fn wait_until_idle<F, Fut>(
    mut running_turns: F,
    poll: Duration,
    settled: u32,
    timeout: Option<Duration>,
    mut progress: impl FnMut(usize),
) -> Idle
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<usize>>,
{
    let deadline = timeout.map(|timeout| tokio::time::Instant::now() + timeout);
    let mut idle_polls = 0;
    let mut last = None;
    loop {
        let Some(running) = running_turns().await else {
            return Idle::ServerGone;
        };
        if last != Some(running) {
            progress(running);
            last = Some(running);
        }
        if running == 0 {
            idle_polls += 1;
            if idle_polls >= settled {
                return Idle::Idle;
            }
        } else {
            idle_polls = 0;
            if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                return Idle::TimedOut { running };
            }
        }
        tokio::time::sleep(poll).await;
    }
}

/// Holds the marker that `status` reads as `restart-pending` while a
/// `restart --when-idle` waits. The operating system drops it with the
/// process, however that ends.
pub struct PendingRestart(#[allow(dead_code)] std::fs::File);

fn pending_path() -> Result<PathBuf> {
    Ok(crate::server::run_dir()?.join("restart-pending"))
}

/// Mark a restart as waiting; `None` when another one already does.
pub fn mark_restart_pending() -> Result<Option<PendingRestart>> {
    let dir = crate::server::run_dir()?;
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(pending_path()?)?;
    Ok(file.try_lock().ok().map(|()| PendingRestart(file)))
}

fn restart_waiting() -> bool {
    let Ok(path) = pending_path() else {
        return false;
    };
    let Ok(file) = std::fs::OpenOptions::new().read(true).open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// The server's pid and running turns now, for deciding on a restart.
pub async fn running_turns() -> Option<usize> {
    live_status().await.map(|live| live.running_turns)
}

/// `rustcode server restart`: stop the server and start it again from the
/// installed definition, waiting for it to listen. Returns the new server.
pub async fn restart() -> Result<Option<LiveStatus>> {
    let (manager, _) = current_manager()?;
    let state = manager.state()?;
    anyhow::ensure!(
        state.installed,
        "the server is not installed as a background service; run `rustcode server install`"
    );
    let before = live_status().await;
    // A server the service did not start (an app's) would make the service's
    // new one exit at once, finding it: end it first.
    if let Some(before) = &before
        && state.pid() != Some(before.pid)
    {
        terminate(before.pid)?;
        wait_until_gone(before.pid).await;
    }
    manager.restart()?;
    Ok(wait_for_server(before.as_ref().map(|live| live.pid)).await)
}

async fn wait_until_gone(pid: u32) {
    for _ in 0..150 {
        if live_status().await.is_none_or(|live| live.pid != pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Ask the process to end, as a service manager does.
fn terminate(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use anyhow::Context;
        let pid = i32::try_from(pid).context("the pid is out of range")?;
        // SAFETY: kill has no memory preconditions.
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).context("could not stop the running server");
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let output = super::run_tool(
            &super::windows_system_tool("taskkill.exe"),
            &["/PID", &pid.to_string(), "/F"],
        )?;
        anyhow::ensure!(
            output.status.success(),
            "could not stop the running server: {}",
            super::failure(&output)
        );
        Ok(())
    }
}

/// `rustcode server uninstall`: stop the service and remove its definition.
/// Threads, settings, pairings and logs stay.
pub fn uninstall() -> Result<bool> {
    let (manager, _) = current_manager()?;
    let installed = manager.state()?.installed;
    manager.uninstall()?;
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_server_is_named_server_and_another_directory_gets_its_own() {
        assert_eq!(service_name(None), "server");
        assert_eq!(service_name(Some(Path::new(""))), "server");
        let one = service_name(Some(Path::new("/tmp/one")));
        assert!(
            one.starts_with("server-") && one.len() == "server-".len() + 8,
            "{one}"
        );
        assert_eq!(one, service_name(Some(Path::new("/tmp/one"))));
        assert_ne!(one, service_name(Some(Path::new("/tmp/two"))));
    }

    #[test]
    fn the_service_runs_server_run_with_the_configuration_it_was_installed_for() {
        let plain = definition(
            Path::new("/usr/bin/rustcode"),
            None,
            "/c/logs/server.log".into(),
        );
        assert_eq!(plain.args, ["server", "run", "--service"]);
        assert!(plain.env.is_empty());
        let other = definition(
            Path::new("/usr/bin/rustcode"),
            Some(Path::new("/tmp/rc")),
            "/c/logs/server.log".into(),
        );
        assert_eq!(
            other.env,
            [("RUSTCODE_CONFIG_DIR".to_owned(), "/tmp/rc".to_owned())]
        );
    }

    fn installed_state() -> State {
        State {
            installed: true,
            active: true,
            process: Process::Running(10),
            path: Some("/home/ada/.config/systemd/user/rustcode-server.service".into()),
            program: Some("/usr/bin/rustcode".into()),
        }
    }

    fn live(version: &str, pid: u32) -> LiveStatus {
        LiveStatus {
            version: version.to_owned(),
            pid,
            clients: 2,
            running_turns: 1,
        }
    }

    fn observed(state: State, live: Option<LiveStatus>) -> Observed {
        Observed {
            kind: "systemd".to_owned(),
            state,
            info: live.as_ref().map(|live| crate::server::ServerInfo {
                pid: live.pid,
                version: live.version.clone(),
                endpoint: "/run/server.sock".to_owned(),
                started_at: 1_000,
            }),
            live,
            restart_waiting: false,
            manager_problems: Vec::new(),
            cli_version: "1.23.0".to_owned(),
            cli_program: "/usr/bin/rustcode".into(),
            gateway: GatewayReport::default(),
            path_rustcode: None,
            log: "/c/logs/server.log".into(),
        }
    }

    fn codes(report: &StatusReport) -> Vec<&'static str> {
        report.problems.iter().map(|problem| problem.code).collect()
    }

    #[test]
    fn status_json_has_the_documented_shape() {
        let report = build_report(observed(installed_state(), Some(live("1.23.0", 10))), 1_090);
        let json = serde_json::to_value(&report).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "installed": true,
                "running": true,
                "version": "1.23.0",
                "cli_version": "1.23.0",
                "pid": 10,
                "uptime_secs": 90,
                "socket": "/run/server.sock",
                "binary": "/usr/bin/rustcode",
                "service": {
                    "kind": "systemd",
                    "path": "/home/ada/.config/systemd/user/rustcode-server.service"
                },
                "clients": 2,
                "running_turns": 1,
                "gateway": {
                    "remote_enabled": false,
                    "running": false,
                    "address": null,
                    "advertised": null,
                    "owner": null,
                    "version": null,
                    "pid": null,
                    "paired_devices": 0,
                    "port": null
                },
                "path_rustcode": null,
                "log": "/c/logs/server.log",
                "problems": []
            })
        );
    }

    /// The gateway `build_report` reads, as `observe_gateway` gathers it.
    fn gateway(owner: &str) -> GatewayReport {
        GatewayReport {
            remote_enabled: true,
            running: true,
            address: Some("100.90.28.23:17879".into()),
            advertised: Some("100.90.28.23:17879".into()),
            owner: Some(owner.into()),
            version: Some("1.23.0".into()),
            pid: Some(77),
            paired_devices: 1,
            port: Some(17879),
        }
    }

    #[test]
    fn the_gateway_line_says_whether_the_machine_stays_reachable_and_who_owns_it() {
        let report = build_report(observed_gateway(gateway("server")), 0);
        let line = report
            .human()
            .lines()
            .find(|line| line.starts_with("Gateway"))
            .expect("a Gateway line")
            .to_owned();
        assert_eq!(
            line,
            "Gateway     running on 100.90.28.23:17879 (started by the RustCode server, version 1.23.0), 1 paired device"
        );
        // The app started it: the line says so, because the machine is
        // reachable only while the app runs.
        let app = build_report(observed_gateway(gateway("app")), 0);
        assert!(
            app.human().contains("(started by the app, version 1.23.0)"),
            "{}",
            app.human()
        );

        // Stopped with remote access on and a server installed: the server
        // starts it again.
        let mut stopped = gateway("server");
        stopped.running = false;
        stopped.pid = None;
        stopped.address = None;
        assert!(
            build_report(observed_gateway(stopped.clone()), 0)
                .human()
                .contains("the RustCode server starts it on port 17879")
        );
        stopped.port = None;
        assert!(
            stopped
                .human(true)
                .contains("the RustCode server starts it")
        );
        // With no server installed nothing restarts it, so the line must not
        // promise one (`StatusReport::human` passes `installed` down).
        let alone = build_report(
            Observed {
                gateway: stopped.clone(),
                ..observed(State::default(), None)
            },
            0,
        );
        assert!(
            alone
                .human()
                .contains("nothing starts it until `rustcode remote start`"),
            "{}",
            alone.human()
        );
        // Off: nothing starts it, and the line says what does.
        let off = GatewayReport::default();
        assert!(
            off.human(true).contains("remote access is off"),
            "{}",
            off.human(true)
        );
    }

    fn observed_gateway(gateway: GatewayReport) -> Observed {
        Observed {
            gateway,
            ..observed(installed_state(), Some(live("1.23.0", 10)))
        }
    }

    #[test]
    fn an_older_rustcode_on_the_path_is_named_with_its_fix() {
        let found = PathRustcode {
            path: "/opt/homebrew/bin/rustcode".into(),
            version: Some("0.96.0".into()),
        };
        let mut with_path = observed(installed_state(), Some(live("1.23.0", 10)));
        with_path.path_rustcode = Some(found.clone());
        let report = build_report(with_path, 0);
        assert!(
            codes(&report).contains(&"path-version-mismatch"),
            "{report:?}"
        );
        let message = report
            .problems
            .iter()
            .find(|problem| problem.code == "path-version-mismatch")
            .unwrap()
            .message
            .clone();
        assert!(
            message.contains("0.96.0 (/opt/homebrew/bin/rustcode)"),
            "{message}"
        );
        assert!(
            message.contains("this engine is 1.23.0 (/usr/bin/rustcode)"),
            "{message}"
        );
        assert!(
            message.contains("brew upgrade lhagfoss/tap/rustcode"),
            "{message}"
        );
        assert_eq!(report.path_rustcode, Some(found));

        // The same version, or a copy that does not answer, is no problem.
        let mut same = observed(installed_state(), Some(live("1.23.0", 10)));
        same.path_rustcode = Some(PathRustcode {
            path: "/opt/homebrew/bin/rustcode".into(),
            version: Some("1.23.0".into()),
        });
        assert_eq!(codes(&build_report(same, 0)), Vec::<&'static str>::new());

        // A shell copy older than the *running server* is the mismatch, even
        // when this rustcode is up to date.
        let mut behind = observed(installed_state(), Some(live("1.24.0", 10)));
        behind.cli_version = "1.24.0".to_owned();
        behind.path_rustcode = Some(PathRustcode {
            path: "/usr/local/bin/rustcode".into(),
            version: Some("1.23.0".into()),
        });
        let report = build_report(behind, 0);
        assert!(
            codes(&report).contains(&"path-version-mismatch"),
            "{report:?}"
        );
    }

    /// An older server runs against a newer app. Each engine's warning must
    /// name that engine's own path and version, never one's path with the
    /// other's version.
    #[test]
    fn each_engine_is_compared_with_its_own_path_and_version() {
        // Three different copies: the app's engine, the service's, and the
        // one a shell runs.
        let mut newer_app = observed(installed_state(), Some(live("1.22.0", 10)));
        newer_app.cli_version = "1.28.0".to_owned();
        newer_app.cli_program = "/Applications/RustCode.app/Contents/MacOS/rustcode".into();
        newer_app.path_rustcode = Some(PathRustcode {
            path: "/usr/local/bin/rustcode".into(),
            version: Some("1.25.0".into()),
        });
        let report = build_report(newer_app, 0);
        // The app's engine: its own path, its own version.
        let app = report
            .problems
            .iter()
            .find(|problem| problem.code == "path-version-mismatch")
            .expect("this engine's mismatch");
        assert!(
            app.message.contains(
                "this engine is 1.28.0 (/Applications/RustCode.app/Contents/MacOS/rustcode)"
            ),
            "{}",
            app.message
        );
        // The running server: the *server's* path with the *server's* version
        // (`installed_state` runs /usr/bin/rustcode).
        let server = report
            .problems
            .iter()
            .find(|problem| problem.code == "path-version-mismatch-server")
            .expect("the running server's mismatch");
        assert!(
            server
                .message
                .contains("this engine is 1.22.0 (/usr/bin/rustcode)"),
            "{}",
            server.message
        );
        assert!(
            server
                .message
                .contains("`rustcode` on PATH is 1.25.0 (/usr/local/bin/rustcode)"),
            "{}",
            server.message
        );
        // Never one binary's path with the other's version.
        assert!(
            !server
                .message
                .contains("/Applications/RustCode.app/Contents/MacOS/rustcode"),
            "{}",
            server.message
        );
        assert!(!server.message.contains("1.28.0"), "{}", server.message);

        // A copy that matches the app is still behind the old server: that
        // mismatch tells the user to update RustCode here, not the copy.
        let mut matches_app = observed(installed_state(), Some(live("1.22.0", 10)));
        matches_app.cli_version = "1.28.0".to_owned();
        matches_app.path_rustcode = Some(PathRustcode {
            path: "/usr/bin/rustcode".into(),
            version: Some("1.28.0".into()),
        });
        let report = build_report(matches_app, 0);
        assert_eq!(
            codes(&report),
            ["version-mismatch", "path-version-mismatch-server"],
            "{report:?}"
        );
        let server = report
            .problems
            .iter()
            .find(|problem| problem.code == "path-version-mismatch-server")
            .unwrap();
        assert!(
            server.message.contains("Update RustCode here"),
            "{}",
            server.message
        );
    }

    #[test]
    fn a_missing_service_and_server_is_reported_with_nulls() {
        let report = build_report(observed(State::default(), None), 5);
        let json = serde_json::to_value(&report).expect("json");
        assert_eq!(json["installed"], false);
        assert_eq!(json["running"], false);
        for key in [
            "version",
            "pid",
            "uptime_secs",
            "socket",
            "binary",
            "clients",
            "running_turns",
        ] {
            assert!(json[key].is_null(), "{key}");
        }
        assert_eq!(codes(&report), ["not-installed"]);
    }

    #[test]
    fn problems_have_stable_codes() {
        let stopped = build_report(observed(installed_state(), None), 0);
        assert_eq!(codes(&stopped), ["stopped"]);

        let old = build_report(observed(installed_state(), Some(live("1.22.0", 10))), 0);
        assert_eq!(codes(&old), ["version-mismatch"]);

        let stray = build_report(observed(installed_state(), Some(live("1.23.0", 99))), 0);
        assert_eq!(codes(&stray), ["unmanaged"]);

        let mut waiting = observed(installed_state(), Some(live("1.23.0", 10)));
        waiting.restart_waiting = true;
        waiting.manager_problems = vec![problem("linger-disabled", "x")];
        assert_eq!(
            codes(&build_report(waiting, 0)),
            ["linger-disabled", "restart-pending"]
        );
    }

    #[test]
    fn a_server_answering_while_the_manager_runs_nothing_is_unmanaged() {
        // The stray holds the lock, so the service's own process exited.
        let stopped = State {
            process: Process::Stopped,
            ..installed_state()
        };
        let report = build_report(observed(stopped, Some(live("1.23.0", 99))), 0);
        assert_eq!(codes(&report), ["unmanaged"]);

        // A manager that cannot say (Windows) is not accused.
        let unknown = State {
            process: Process::Unknown,
            ..installed_state()
        };
        let report = build_report(observed(unknown, Some(live("1.23.0", 99))), 0);
        assert!(codes(&report).is_empty(), "{:?}", codes(&report));
    }

    struct Refusing;

    impl Manager for Refusing {
        fn kind(&self) -> &'static str {
            "fake"
        }
        fn state(&self) -> Result<State> {
            unreachable!("a refused install reads nothing")
        }
        fn check(&self, _definition: &Definition) -> Result<()> {
            anyhow::bail!("the manager cannot take this definition")
        }
        fn install(&self, _definition: &Definition) -> Result<Installed> {
            unreachable!("a refused install writes nothing")
        }
        fn restart(&self) -> Result<()> {
            unreachable!()
        }
        fn uninstall(&self) -> Result<()> {
            unreachable!()
        }
    }

    #[test]
    fn install_is_refused_before_a_server_is_stopped() {
        let definition = definition(
            Path::new("/usr/bin/rustcode"),
            Some(Path::new("/tmp/rc")),
            "/c/logs/server.log".into(),
        );
        let error = preflight(&Refusing, &definition, false).expect_err("refused");
        assert!(error.to_string().contains("cannot take"), "{error}");
        // The development-build refusal is part of it.
        let dev = super::super::Definition {
            program: "/w/target/debug/rustcode".into(),
            ..definition
        };
        let error = preflight(&Refusing, &dev, false).expect_err("refused");
        assert!(error.to_string().contains("development build"), "{error}");
    }

    #[test]
    fn install_takes_over_from_a_server_that_is_not_the_services() {
        let idle = LiveStatus {
            running_turns: 0,
            ..live("1.23.0", 99)
        };
        let busy = live("1.23.0", 99);
        let with = |process, installed| State {
            installed,
            process,
            ..installed_state()
        };
        // Nothing answers: nothing to take over.
        assert_eq!(
            plan_takeover(&with(Process::Stopped, false), None, false),
            Takeover::Leave
        );
        // The service's own server stays.
        assert_eq!(
            plan_takeover(&with(Process::Running(99), true), Some(&idle), false),
            Takeover::Leave
        );
        // The manager runs nothing, or something else: the server answering
        // is a stray.
        for process in [Process::Stopped, Process::Running(5)] {
            assert_eq!(
                plan_takeover(&with(process, true), Some(&idle), false),
                Takeover::Stop { pid: 99 }
            );
        }
        // Turns are not ended without --yes.
        assert_eq!(
            plan_takeover(&with(Process::Stopped, false), Some(&busy), false),
            Takeover::Confirm { running: 1 }
        );
        assert_eq!(
            plan_takeover(&with(Process::Stopped, false), Some(&busy), true),
            Takeover::Stop { pid: 99 }
        );
        // A manager that cannot say: a stray only before it is installed.
        assert_eq!(
            plan_takeover(&with(Process::Unknown, false), Some(&idle), false),
            Takeover::Stop { pid: 99 }
        );
        assert_eq!(
            plan_takeover(&with(Process::Unknown, true), Some(&idle), false),
            Takeover::Leave
        );
    }

    #[test]
    fn a_stale_description_is_not_trusted() {
        let mut observed = observed(installed_state(), None);
        observed.info = Some(crate::server::ServerInfo {
            pid: 7,
            version: "1".to_owned(),
            endpoint: "/gone.sock".to_owned(),
            started_at: 1,
        });
        let report = build_report(observed, 100);
        assert_eq!(report.socket, None);
        assert_eq!(report.uptime_secs, None);
    }

    #[test]
    fn the_human_report_names_every_fact() {
        let report = build_report(observed(installed_state(), Some(live("1.22.0", 10))), 1_090);
        let text = report.human();
        for expected in [
            "Installed   yes (systemd)",
            "Running     yes (version 1.22.0, pid 10)",
            "Uptime      1m 30s",
            "Socket      /run/server.sock",
            "Binary      /usr/bin/rustcode",
            "Clients     2",
            "Turns       1 running",
            "Problem     version-mismatch:",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
    }

    #[test]
    fn restart_asks_only_when_turns_would_be_lost() {
        assert_eq!(plan_restart(false, false, Some(0)), RestartPlan::Now);
        assert_eq!(plan_restart(false, false, None), RestartPlan::Now);
        assert_eq!(
            plan_restart(false, false, Some(2)),
            RestartPlan::Confirm { running: 2 }
        );
        assert_eq!(plan_restart(false, true, Some(2)), RestartPlan::Now);
        assert_eq!(plan_restart(true, false, Some(2)), RestartPlan::WaitForIdle);
        assert_eq!(plan_restart(true, true, Some(2)), RestartPlan::WaitForIdle);
    }

    /// A server whose running-turn count follows a script, then stays at its
    /// last value; `None` entries are a server that does not answer.
    fn scripted(
        script: Vec<Option<usize>>,
    ) -> (
        impl FnMut() -> std::future::Ready<Option<usize>>,
        std::rc::Rc<std::cell::Cell<usize>>,
    ) {
        let polls = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = polls.clone();
        let next = move || {
            let at = seen.get();
            seen.set(at + 1);
            std::future::ready(script[at.min(script.len() - 1)])
        };
        (next, polls)
    }

    #[tokio::test]
    async fn restart_when_idle_waits_for_the_turns_to_finish() {
        let (server, polls) = scripted(vec![Some(2), Some(2), Some(1), Some(0), Some(0)]);
        let mut heard = Vec::new();
        let outcome = wait_until_idle(server, Duration::from_millis(1), 2, None, |running| {
            heard.push(running)
        })
        .await;
        assert_eq!(outcome, Idle::Idle);
        assert_eq!(heard, [2, 1, 0]);
        assert_eq!(polls.get(), 5, "it needs two idle readings in a row");
    }

    #[tokio::test]
    async fn a_turn_starting_between_idle_readings_restarts_the_count() {
        let (server, polls) = scripted(vec![Some(0), Some(1), Some(0), Some(0)]);
        let outcome = wait_until_idle(server, Duration::from_millis(1), 2, None, |_| {}).await;
        assert_eq!(outcome, Idle::Idle);
        assert_eq!(polls.get(), 4);
    }

    #[tokio::test]
    async fn waiting_stops_when_the_server_goes() {
        let (server, _) = scripted(vec![Some(3), None]);
        let outcome = wait_until_idle(server, Duration::from_millis(1), 2, None, |_| {}).await;
        assert_eq!(outcome, Idle::ServerGone);
    }

    #[tokio::test]
    async fn waiting_gives_up_at_the_deadline() {
        let (server, _) = scripted(vec![Some(3)]);
        let outcome = wait_until_idle(
            server,
            Duration::from_millis(5),
            2,
            Some(Duration::from_millis(20)),
            |_| {},
        )
        .await;
        assert_eq!(outcome, Idle::TimedOut { running: 3 });
    }

    #[tokio::test]
    async fn the_status_request_is_answered_past_notifications() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let (client, server) = tokio::io::duplex(4096);
        let fake = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut lines = tokio::io::BufReader::new(read).lines();
            let request: serde_json::Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], STATUS_METHOD);
            let id = request["id"].clone();
            let notification = serde_json::json!({
                "jsonrpc": "2.0", "method": "session/update", "params": {}
            });
            let reply = serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": {"version": "1.23.0", "pid": 42, "clients": 3, "running_turns": 1}
            });
            write
                .write_all(format!("{notification}\n{reply}\n").as_bytes())
                .await
                .unwrap();
        });
        let status = ask_status(client).await.expect("status");
        assert_eq!(
            status,
            LiveStatus {
                version: "1.23.0".to_owned(),
                pid: 42,
                clients: 3,
                running_turns: 1,
            }
        );
        fake.await.unwrap();
    }

    #[tokio::test]
    async fn an_error_reply_is_an_error() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let (client, server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut lines = tokio::io::BufReader::new(read).lines();
            let _ = lines.next_line().await;
            let reply = serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "no"}
            });
            let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
        });
        assert!(ask_status(client).await.is_err());
    }
}
