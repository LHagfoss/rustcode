//! The remote gateway keeps the RustCode server it relays to running.
//!
//! A paired machine reaches this machine's threads through the gateway, which
//! relays ACP to the server (`remote_gateway/acp_relay.rs`). The gateway can
//! outlive the server: one killed with the session that started it (a restart
//! over SSH on Windows) left a gateway that passed every handshake and refused
//! every `acp_open`, until a person started the server again. So the gateway
//! `rustcode remote serve` runs starts the server when none answers: when a
//! device opens ACP, and, for a server installed as a background service,
//! from a watch of its own. A server that left `server.json` behind is dead
//! when its process is gone or its pid names a later process. Starts back off
//! and are bounded, and each is logged to the gateway's log; when the server
//! still does not answer, the device is refused with `internal`, which a
//! client reads as `server_not_running`.
//!
//! What is decided is plain functions ([`liveness`], [`Backoff`]), tested
//! without processes.

use super::{ServerInfo, connect_running, listen::ClientStream};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long a relay waits for a server it started. A device waits 10 s for
/// `acp_opened`, and a server listens within about a second.
pub(crate) const RELAY_WAIT: Duration = Duration::from_secs(7);
/// How often the watch looks at an installed server.
const WATCH_POLL: Duration = Duration::from_secs(30);
/// How long the watch waits for a server it started.
const WATCH_WAIT: Duration = Duration::from_secs(30);
/// The wait after the first start that did not answer; it doubles after each.
const FIRST_RETRY: Duration = Duration::from_secs(5);
/// Starts in a row that may fail before the gateway pauses.
const MAX_ATTEMPTS: u32 = 5;
/// The pause after [`MAX_ATTEMPTS`] failed starts, before a fresh round.
const PAUSE: Duration = Duration::from_secs(30 * 60);
/// A server writes `started_at` after its process starts. A process that
/// started later than this past it is another one under a reused pid.
const START_SLACK_SECS: u64 = 2;

/// Whether the server that described itself can still be the one running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Liveness {
    /// No server described itself, or the one described is gone: its process
    /// ended, or its pid now names a process that started after it. Start one.
    Dead,
    /// Its process still runs but does not accept a connection: starting, or
    /// stuck. A second server would only find its lock, and a restart would
    /// end what it is doing, so it is left alone.
    Unresponsive { pid: u32 },
}

/// Decide from the server's description and when the process with its pid
/// started (`None` when no such process runs), in seconds since the epoch.
pub(crate) fn liveness(info: Option<&ServerInfo>, process_started_at: Option<u64>) -> Liveness {
    match (info, process_started_at) {
        (Some(info), Some(started)) if started <= info.started_at + START_SLACK_SECS => {
            Liveness::Unresponsive { pid: info.pid }
        }
        _ => Liveness::Dead,
    }
}

/// When the process `pid` started, in seconds since the epoch, if it runs.
fn process_started_at(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(sysinfo::Process::start_time)
}

/// Whether a start may happen now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    Start,
    /// A start failed lately; the next one may happen after this long.
    Wait(Duration),
}

/// Bounded, growing waits between starts that did not bring a server up:
/// 5, 10, 20 and 40 s, then a pause of 30 minutes after the fifth, and a
/// fresh round after it. A server that answers resets it.
#[derive(Debug, Default)]
pub(crate) struct Backoff {
    failures: u32,
    next: Option<Instant>,
}

impl Backoff {
    pub(crate) fn decide(&self, now: Instant) -> Decision {
        match self.next {
            Some(next) if next > now => Decision::Wait(next - now),
            _ => Decision::Start,
        }
    }

    /// A start did not bring a server up; returns the wait before the next.
    pub(crate) fn failed(&mut self, now: Instant) -> Duration {
        self.failures += 1;
        let wait = if self.failures >= MAX_ATTEMPTS {
            self.failures = 0;
            PAUSE
        } else {
            FIRST_RETRY * 2u32.pow(self.failures - 1)
        };
        self.next = Some(now + wait);
        wait
    }

    pub(crate) fn answered(&mut self) {
        *self = Self::default();
    }
}

struct Supervisor {
    /// The `rustcode` that starts a server no service manager runs.
    program: PathBuf,
    config_directory: PathBuf,
    /// Held across a start, so relays that find no server at the same time
    /// start one between them.
    backoff: tokio::sync::Mutex<Backoff>,
}

static SUPERVISOR: OnceLock<Supervisor> = OnceLock::new();

/// Let this gateway start the server of `config_directory` when none answers,
/// and watch an installed one. Only `rustcode remote serve` calls it: a
/// gateway in a test or inside another process starts nothing.
pub fn supervise(program: PathBuf, config_directory: PathBuf) {
    let supervisor = Supervisor {
        program,
        config_directory,
        backoff: tokio::sync::Mutex::new(Backoff::default()),
    };
    if SUPERVISOR.set(supervisor).is_err() {
        return;
    }
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(watch());
    }
}

/// A relay reached a server: the next one that does not answer starts with
/// a fresh round of attempts.
pub(crate) fn answered() {
    if let Some(supervisor) = SUPERVISOR.get()
        && let Ok(mut backoff) = supervisor.backoff.try_lock()
    {
        backoff.answered();
    }
}

/// Bring the server back when none answers, waiting up to `wait` for it,
/// and connect to it. The error says why there is no server to relay to.
pub(crate) async fn revive(wait: Duration) -> Result<ClientStream, String> {
    let Some(supervisor) = SUPERVISOR.get() else {
        return Err("this gateway does not start the server".to_owned());
    };
    let mut backoff = supervisor.backoff.lock().await;
    // Another relay may have started it while this one waited for the lock.
    if let Ok(stream) = connect_running().await {
        backoff.answered();
        return Ok(stream);
    }
    let info = super::running();
    let started_at = info.as_ref().and_then(|info| process_started_at(info.pid));
    if let Liveness::Unresponsive { pid } = liveness(info.as_ref(), started_at) {
        return Err(format!(
            "the RustCode server (pid {pid}) runs but does not accept connections"
        ));
    }
    if let Decision::Wait(left) = backoff.decide(Instant::now()) {
        return Err(format!(
            "the last start of the RustCode server did not answer; the next one is in {}s",
            left.as_secs().max(1)
        ));
    }
    let gone = match &info {
        Some(info) => format!(
            "the RustCode server (pid {}, version {}) is gone and left {} behind",
            info.pid, info.version, info.endpoint
        ),
        None => "no RustCode server is running".to_owned(),
    };
    match start(supervisor).await {
        Ok(how) => note(&format!("gateway: {gone}; starting it {how}")),
        Err(error) => {
            let next = backoff.failed(Instant::now());
            let why = format!("{gone}, and it could not be started: {error:#}");
            note(&format!("gateway: {why} (next try in {}s)", next.as_secs()));
            return Err(why);
        }
    }
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if let Ok(stream) = connect_running().await {
            backoff.answered();
            note("gateway: the RustCode server it started answers");
            return Ok(stream);
        }
        if tokio::time::Instant::now() >= deadline {
            let next = backoff.failed(Instant::now());
            let why = format!(
                "the RustCode server was started but did not answer within {}s; see `rustcode server logs`",
                wait.as_secs()
            );
            note(&format!("gateway: {why} (next try in {}s)", next.as_secs()));
            return Err(why);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Start the server: through its service manager when it is installed, so
/// the manager runs the one it supervises, and otherwise as `server connect`
/// does. Says how, for the log.
async fn start(supervisor: &Supervisor) -> anyhow::Result<&'static str> {
    let program = supervisor.program.clone();
    let config_directory = supervisor.config_directory.clone();
    tokio::task::spawn_blocking(move || {
        use crate::service::server_service::{installed_for, start_installed_for};
        if installed_for(&config_directory) {
            start_installed_for(&config_directory)?;
            Ok("through its background service")
        } else {
            super::start_in_background(&program, false)?;
            Ok("in the background")
        }
    })
    .await?
}

/// Watch an installed server, and start it again when it is gone on two
/// looks in a row: a restart in progress has the time between them.
async fn watch() {
    let mut gone_looks = 0;
    loop {
        tokio::time::sleep(WATCH_POLL).await;
        let Some(supervisor) = SUPERVISOR.get() else {
            return;
        };
        if connect_running().await.is_ok() {
            answered();
            gone_looks = 0;
            continue;
        }
        let info = super::running();
        let started_at = info.as_ref().and_then(|info| process_started_at(info.pid));
        if liveness(info.as_ref(), started_at) != Liveness::Dead {
            gone_looks = 0;
            continue;
        }
        let config_directory = supervisor.config_directory.clone();
        let installed = tokio::task::spawn_blocking(move || {
            crate::service::server_service::installed_for(&config_directory)
        })
        .await
        .unwrap_or(false);
        if !installed {
            gone_looks = 0;
            continue;
        }
        gone_looks += 1;
        if gone_looks < 2 {
            continue;
        }
        gone_looks = 0;
        // What happened is in the log; nobody waits for the answer here.
        let _ = revive(WATCH_WAIT).await;
    }
}

/// Log to the gateway's own log (its standard error) and the debug log.
fn note(line: &str) {
    eprintln!("{line}");
    crate::dbg_log!("{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn described(pid: u32, started_at: u64) -> ServerInfo {
        ServerInfo {
            pid,
            version: "1.31.4".to_owned(),
            endpoint: r"\\.\pipe\rustcode-server-test".to_owned(),
            started_at,
        }
    }

    #[test]
    fn a_description_whose_process_is_gone_or_reused_is_a_dead_server() {
        let info = described(5236, 1_000);
        // No description: nothing to wait for.
        assert_eq!(liveness(None, None), Liveness::Dead);
        // The process ended (killed with an SSH session's job): server.json
        // still names it, and its pipe is gone.
        assert_eq!(liveness(Some(&info), None), Liveness::Dead);
        // The pid names a process that started after the server did.
        assert_eq!(liveness(Some(&info), Some(1_100)), Liveness::Dead);
        // Its own process still runs: starting or stuck, left alone.
        assert_eq!(
            liveness(Some(&info), Some(999)),
            Liveness::Unresponsive { pid: 5236 }
        );
        assert_eq!(
            liveness(Some(&info), Some(1_000 + START_SLACK_SECS)),
            Liveness::Unresponsive { pid: 5236 }
        );
    }

    #[test]
    fn failed_starts_back_off_then_pause_and_an_answer_resets() {
        let now = Instant::now();
        let mut backoff = Backoff::default();
        assert_eq!(backoff.decide(now), Decision::Start);
        let waits: Vec<u64> = (0..MAX_ATTEMPTS)
            .map(|_| backoff.failed(now).as_secs())
            .collect();
        assert_eq!(waits, [5, 10, 20, 40, PAUSE.as_secs()]);
        assert_eq!(backoff.decide(now), Decision::Wait(PAUSE));
        assert_eq!(backoff.decide(now + PAUSE), Decision::Start);
        // A fresh round after the pause.
        assert_eq!(backoff.failed(now + PAUSE), FIRST_RETRY);
        backoff.answered();
        assert_eq!(backoff.decide(now), Decision::Start);
        assert_eq!(backoff.failed(now), FIRST_RETRY);
    }

    #[tokio::test]
    async fn a_gateway_that_does_not_supervise_starts_nothing() {
        // Tests never call `supervise`: a gateway in a test must not start
        // this test binary as a server.
        let error = revive(Duration::from_millis(10)).await.unwrap_err();
        assert!(error.contains("does not start"), "{error}");
    }
}
