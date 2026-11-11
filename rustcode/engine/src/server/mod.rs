//! The RustCode server: one engine per user and config directory that
//! several clients share (`docs/server.md`).
//!
//! `rustcode server run` holds the server lock (`lock.rs`), listens on a
//! local endpoint only this user can reach (`listen.rs`, `peer.rs`) and
//! serves ACP on every connection to one [`AcpEngine`]. Sessions, turns and
//! harness processes belong to that engine: a client that disconnects ends
//! none of them, and the harnesses end with the server process.
//!
//! `rustcode server connect` is a client: it relays its stdin and stdout to
//! the server, starting one in the background first if none runs, so
//! anything that spoke ACP to `rustcode --acp` can speak it to the server.
//!
//! An engine started by a client (`rustcode --acp`, the desktop app's) is not
//! a server, but it publishes where a local relay can reach it
//! (`engine_endpoint.rs`), so a paired device opens the threads the app shows
//! without a server running behind it.

pub(crate) mod engine_endpoint;
pub(crate) mod listen;
mod lock;
mod peer;

pub use lock::ServerInfo;

use crate::acp::{AcpEngine, KillHarnesses};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long `connect` waits for a server it started to listen.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// The directory of the server's lock, description and socket:
/// `<config dir>/run`, so `RUSTCODE_CONFIG_DIR` gives a separate server.
pub fn run_dir() -> io::Result<PathBuf> {
    crate::config::get_config_dir()
        .map(|dir| dir.join("run"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no RustCode config directory"))
}

/// Where the server for this user said it listens. A server that crashed
/// leaves this behind, so only connecting tells it runs. (Testing the lock
/// would take it for a moment, and a server starting then would think
/// another one runs.)
pub fn running() -> Option<ServerInfo> {
    lock::read_info(&run_dir().ok()?)
}

/// Every local endpoint a relay can open ACP on in `config_directory`, best
/// first: an engine started by a client (`rustcode --acp`, the app's engine,
/// `engine_endpoint.rs`) and then a shared server (`server.json`). Either may
/// be a dead description — a crashed process leaves it behind — so connecting
/// decides. An empty list means no engine described itself.
pub(crate) fn relay_endpoints(config_directory: &Path) -> Vec<String> {
    let run = config_directory.join("run");
    let mut endpoints: Vec<String> = Vec::with_capacity(2);
    // The app's own engine first: it holds the threads the user is looking at,
    // so a device opens those rather than a shared server's view of the store.
    if let Some(published) = engine_endpoint::published(&run) {
        endpoints.push(published.endpoint);
    }
    if let Some(info) = lock::read_info(&run)
        && !endpoints.contains(&info.endpoint)
    {
        endpoints.push(info.endpoint);
    }
    endpoints
}

/// How `server run` ended.
#[derive(Debug)]
pub enum RunOutcome {
    /// It served until asked to stop.
    Stopped,
    /// Another server already runs for this user; this one did nothing.
    AlreadyRunning(Option<ServerInfo>),
}

/// `rustcode server run`: serve every client of this user until a signal
/// stops the process. `service` is set by a service manager (`--service`):
/// on Windows it has no console window to keep open.
pub async fn run(auto_approve: bool, service: bool) -> anyhow::Result<RunOutcome> {
    let dir = run_dir()?;
    let lock = match lock::acquire(&dir)? {
        lock::Acquired::Held(lock) => lock,
        lock::Acquired::Running(info) => return Ok(RunOutcome::AlreadyRunning(info)),
    };
    if service {
        detach_console();
    }
    // `rustcode server logs` reads this; only a server writes it.
    crate::logger::mirror_to_server_log();
    crate::dbg_log!(
        "RustCode server {} starting (pid {}{})",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        if service { ", as a service" } else { "" }
    );
    let endpoint = listen::endpoint(&dir);
    let mut listener = listen::Listener::bind(&endpoint)?;
    let engine = AcpEngine::start(auto_approve)?;
    // No harness process outlives the server, however it ends.
    let _kill_harnesses = KillHarnesses;
    lock.publish(&ServerInfo::now(endpoint))?;
    crate::dbg_log!("RustCode server listening");
    // With remote access on, this server is what keeps the machine reachable
    // once the app that started it is gone (docs/remote.md): it starts the
    // remote gateway and starts it again when it stops. The app adopts a
    // gateway started here instead of binding a second one.
    crate::remote_gateway::host::spawn();
    serve(&engine, &mut listener, stop_signal()).await;
    crate::dbg_log!("RustCode server stopping");
    engine.shutdown().await;
    drop(listener);
    drop(lock);
    Ok(RunOutcome::Stopped)
}

/// Accept clients on `listener` until `stop` resolves, each served on its
/// own thread by `engine`.
async fn serve(
    engine: &AcpEngine,
    listener: &mut listen::Listener,
    stop: impl std::future::Future<Output = ()>,
) {
    tokio::pin!(stop);
    loop {
        tokio::select! {
            () = &mut stop => break,
            accepted = listener.accept() => match accepted {
                Ok(stream) => serve_client(engine.clone(), stream),
                Err(error) => {
                    // Another user, or a client that left before it was
                    // accepted: neither stops the server.
                    crate::dbg_log!("RustCode server refused a connection: {error}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            },
        }
    }
}

/// Serve one client. Polling a connection's handlers is deep in a debug
/// build, deeper than a runtime worker's stack: like `rustcode --acp`, which
/// serves its client on the main thread, each client gets a thread of its
/// own. What a handler spawns, a turn first, outlives the client.
pub(crate) fn serve_client<S>(engine: AcpEngine, stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
{
    crate::acp::spawn_on_large_stack("rustcode-server-client", async move {
        let (read, write) = tokio::io::split(stream);
        if let Err(error) = engine.serve(read, write).await {
            crate::dbg_log!("RustCode server client ended: {error}");
        }
    });
}

/// A server started by a Windows scheduled task has a console window it does
/// not need: close it.
fn detach_console() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn FreeConsole() -> i32;
        }
        // SAFETY: FreeConsole has no preconditions; it fails harmlessly when
        // the process has no console.
        unsafe {
            FreeConsole();
        }
    }
}

/// Resolves when the process is asked to stop: Ctrl-C, or on Unix
/// `SIGTERM` (launchd, systemd) and `SIGHUP`.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut hangup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        ) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = hangup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// `rustcode server connect`: relay stdin and stdout to the server until
/// either side closes, starting the server first if none runs. `program` is
/// the `rustcode` executable that starts it.
pub async fn connect(program: PathBuf) -> anyhow::Result<()> {
    let stream = match connect_running().await {
        Ok(stream) => stream,
        Err(_) => {
            start_in_background(&program)?;
            wait_for_server().await?
        }
    };
    bridge(stream, tokio::io::stdin(), tokio::io::stdout()).await?;
    Ok(())
}

/// A connection to the server this user runs.
pub(crate) async fn connect_running() -> io::Result<listen::ClientStream> {
    let info = running()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no RustCode server is running"))?;
    listen::connect(&info.endpoint).await
}

async fn wait_for_server() -> anyhow::Result<listen::ClientStream> {
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        match connect_running().await {
            Ok(stream) => return Ok(stream),
            Err(error) if tokio::time::Instant::now() >= deadline => {
                anyhow::bail!(
                    "the RustCode server did not start within {}s ({error}); see {}",
                    START_TIMEOUT.as_secs(),
                    log_path()?.display()
                );
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// Where a server writes its log, and where one started in the background
/// writes what it prints: `<config dir>/logs/server.log`.
pub fn log_path() -> io::Result<PathBuf> {
    crate::logger::server_log_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no RustCode config directory"))
}

/// Start `program server run` detached from this process: its own process
/// group, no terminal, nothing inherited but the environment, so it lives
/// on when the client that started it ends.
pub(crate) fn start_in_background(program: &std::path::Path) -> io::Result<()> {
    let dir = run_dir()?;
    lock::ensure_private_dir(&dir)?;
    let log_path = log_path()?;
    if let Some(logs) = log_path.parent() {
        std::fs::create_dir_all(logs)?;
    }
    let mut log = std::fs::OpenOptions::new();
    log.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        log.mode(0o600);
    }
    let log = log.open(log_path)?;
    let mut command = std::process::Command::new(program);
    command
        .args(["server", "run"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    // Not waited for: it outlives this process. A server that finds another
    // already running exits at once, and the connect below finds that one.
    command.spawn().map(drop)
}

/// Copy `input` to `stream` and `stream` to `output` until either ends.
async fn bridge<S, I, O>(stream: S, mut input: I, mut output: O) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite,
    I: tokio::io::AsyncRead + Unpin,
    O: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let (mut from_server, mut to_server) = tokio::io::split(stream);
    let upstream = async {
        tokio::io::copy(&mut input, &mut to_server).await?;
        // The client is done: the server sees the connection end, answers
        // what it still owes and closes its side.
        to_server.shutdown().await
    };
    let downstream = async {
        tokio::io::copy(&mut from_server, &mut output).await?;
        output.flush().await
    };
    tokio::pin!(downstream);
    tokio::select! {
        result = upstream => {
            result?;
            downstream.await
        }
        // The server went: nothing more can be relayed.
        result = &mut downstream => result,
    }
}

#[cfg(test)]
mod tests;
