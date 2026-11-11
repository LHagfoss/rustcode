//! Settings › Machines › *This computer*: whether the RustCode server runs as a
//! background service, and the one command that installs it (`docs/server.md`).
//!
//! The work is the engine's own: `rustcode server status --json` and
//! `rustcode server install --yes`, the same commands a terminal runs, in the
//! same binary the app starts its engine from. Nothing here re-implements the
//! service managers (launchd, systemd --user, Task Scheduler), and nothing here
//! writes a unit file: the CLI is the source of truth for all three systems.
//!
//! What that service is today: the shared engine that threads can run in, kept
//! up by the service manager and started when the user logs in, plus the remote
//! gateway it hosts, so a paired machine still reaches this computer with the
//! app closed (`docs/remote.md`). The threads this app itself runs stay in the
//! app's own `--acp` engine, so closing the app still ends them; switching the
//! app to the server is a later step (`docs/server.md`). The words the page
//! shows say that and no more.
//!
//! An AppImage runs its bundled `rustcode` from a mount that goes away with the
//! app, so the service is installed from a copy under the user's data directory
//! ([`stable_program`]); a unit naming the mount would be `203/EXEC` at the next
//! login.

use crate::engine::resolve_engine;
use crate::process;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// `server status --json` answers in well under a second; a locked credential
/// store or a stuck service manager is what this is for.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(20);

/// `server install` writes a unit, starts it and waits for the socket, which
/// the engine bounds at 30 seconds (`server_service`); this is longer than that.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(90);

/// How long the catch-up after an update waits for running turns before it
/// leaves the old server for the next start: a turn can run for hours, and
/// this is a courtesy, not a promise.
const RESTART_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The server as a background service, as the page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    /// A service definition is in place, so the server starts when the user
    /// logs in.
    pub installed: bool,
    /// A server answers on its socket now.
    pub running: bool,
    /// The running server's version, or none while it does not answer.
    pub version: Option<String>,
    /// The version of the `rustcode` that answered.
    pub cli_version: Option<String>,
    /// This machine's remote gateway: whether another machine reaches it with
    /// the app closed, where, and who started it (`docs/remote.md`).
    pub gateway: Gateway,
    /// The `rustcode` a shell runs, when it is a different engine (issue
    /// #3411). The warning itself is one of `problems`.
    pub path_rustcode: Option<PathRustcode>,
    /// What is wrong, in the words `server status` uses: `linger-disabled`,
    /// `version-mismatch`, `path-version-mismatch`, and the rest.
    pub problems: Vec<ServiceProblem>,
    /// The command that failed, when reading the status at all did.
    pub error: Option<String>,
}

/// One problem `server status` reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceProblem {
    pub code: String,
    pub message: String,
}

/// This machine's remote gateway, as `server status --json` reports it.
///
/// The CLI writes snake_case fields; the page reads camelCase, as it does for
/// the rest of this status.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Gateway {
    /// Remote access is on for this machine.
    pub remote_enabled: bool,
    /// A gateway answers now, so paired devices can reach this machine.
    pub running: bool,
    /// What devices dial, while it runs.
    pub address: Option<String>,
    pub advertised: Option<String>,
    /// Who started it: `server`, `app` or `terminal`.
    pub owner: Option<String>,
    pub version: Option<String>,
    pub pid: Option<u32>,
    pub paired_devices: usize,
    /// The port it starts on from its recorded settings, while it runs not.
    pub port: Option<u16>,
}

/// The `rustcode` a shell finds first on `PATH`, and its version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRustcode {
    pub path: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// What `server status --json` prints; only the fields the page reads.
#[derive(Debug, Deserialize)]
struct Report {
    installed: bool,
    running: bool,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    cli_version: Option<String>,
    /// The program the service definition names: what a restart runs.
    #[serde(default)]
    binary: Option<String>,
    #[serde(default)]
    gateway: Gateway,
    #[serde(default)]
    path_rustcode: Option<PathRustcode>,
    #[serde(default)]
    problems: Vec<ServiceProblem>,
}

/// Whether the RustCode server is installed as a service, and running.
#[tauri::command]
pub async fn server_service_status() -> Result<ServiceStatus, String> {
    read_status().await
}

/// Install the RustCode server as a background service — the shared engine that
/// stays up when the app closes and starts when the user logs in — and answer
/// the state after it. The install is the CLI's own: a
/// development build is refused there, and a server another process runs is
/// never stopped while a turn is running — the install says so instead, and
/// `--yes` stays a decision for a terminal.
#[tauri::command]
pub async fn server_service_install() -> Result<ServiceStatus, String> {
    let output = run(&["server", "install"], INSTALL_TIMEOUT).await?;
    let said = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let message = if why.is_empty() { said } else { why };
        return Err(if message.is_empty() {
            "The RustCode server could not be installed.".to_owned()
        } else {
            message
        });
    }
    // The app installed it, so the definition is this app's to rewrite when a
    // later update needs the service to run the new engine — and only while it
    // still names what this line wrote.
    remember_installed_by_app(&service_program());
    let mut status = read_status().await?;
    if !status.installed {
        status.error = Some(if said.is_empty() {
            "The RustCode server was installed but does not read as installed yet.".to_owned()
        } else {
            said
        });
    }
    Ok(status)
}

/// Point the `rustcode` a shell runs at this app's engine (issue #3411): the
/// CLI's own `server link-path`, run by the bundled engine so the shell then
/// finds the app's version. Homebrew's copy and a development build are refused
/// there with the reason, which is shown as it is. Answers the state after it.
#[tauri::command]
pub async fn server_link_path() -> Result<ServiceStatus, String> {
    let output = run(&["server", "link-path"], STATUS_TIMEOUT).await?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let why = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let message = if why.is_empty() { said } else { why };
        return Err(if message.is_empty() {
            "`rustcode` on PATH could not be pointed at this app.".to_owned()
        } else {
            message
        });
    }
    read_status().await
}

/// The current state from `rustcode server status --json`. A status that
/// cannot be read is a status the page says it does not know, never an error
/// the page cannot show.
async fn read_status() -> Result<ServiceStatus, String> {
    match run(&["server", "status", "--json"], STATUS_TIMEOUT).await {
        Ok(output) if output.status.success() => {
            let report: Report = serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("Could not read the server's status: {error}"))?;
            // A restart that would bring the same old copy back is named for
            // what it is, instead of the engine's "restart brings it up to
            // date" advice, which cannot work there (issue #3487).
            let mut problems = report.problems.clone();
            if let Some(message) = foreign_server(&report, &service_program()) {
                problems.retain(|problem| problem.code != "version-mismatch");
                problems.push(ServiceProblem {
                    code: "server-other-rustcode".to_owned(),
                    message,
                });
            }
            Ok(ServiceStatus {
                installed: report.installed,
                running: report.running,
                version: report.version,
                cli_version: report.cli_version,
                gateway: report.gateway,
                path_rustcode: report.path_rustcode,
                problems,
                error: None,
            })
        }
        Ok(output) => Err(format!(
            "Could not read the server's status: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Err(error),
    }
}

/// `rustcode server status --json` on a plain thread, for the catch-up at app
/// start: no settings page is waiting for it.
fn read_report() -> Option<Report> {
    let output = run_sync(&["server", "status", "--json"], STATUS_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Run `rustcode <args>` and answer what it printed, killing it once `timeout`
/// passes: a command that never answers must not hold a settings page.
pub async fn run(args: &[&str], timeout: Duration) -> Result<std::process::Output, String> {
    run_sync(args, timeout)
}

/// [`run`] on a plain thread, for the catches that run at app start before any
/// page asks (`catch_up_after_update`).
fn run_sync(args: &[&str], timeout: Duration) -> Result<std::process::Output, String> {
    run_program(&prepare_program()?, args, timeout)
}

/// Keep the copy a service installed from an AppImage runs up to date, at app
/// start: the AppImage may have been updated since it was made. A copy that
/// cannot be made is not reported here (the app still runs); the page says it
/// when it asks for the status or installs.
pub fn refresh_program() {
    let _ = prepare_program();
}

/// The `rustcode` a service runs from this app, and that `server install`
/// writes into the unit: its engine, or — inside an AppImage — the copy under
/// the user's data directory that [`refresh_program`] keeps current. A
/// definition naming this is one this app manages. Nothing is copied here:
/// this is the path a definition names, not the copy itself.
fn service_program() -> PathBuf {
    let engine = resolve_engine();
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    stable_program(
        &engine,
        appimage.is_some(),
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
    .unwrap_or(engine)
}

/// What `rustcode server update-cli --json` said it found and did.
#[derive(Debug, Deserialize)]
struct UpdatedCli {
    /// `missing`, `current`, `updated`, `homebrew`, `cargo`, `development` or
    /// `foreign`.
    outcome: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// What this app does about a background server that runs another engine.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerCatchUp {
    /// Nothing: no service, nothing running, or the server is this engine's
    /// version or a newer one.
    Leave,
    /// The service runs a binary this app manages: restart it from the
    /// installed definition, which brings it up to date.
    Restart,
    /// The service runs another `rustcode` and the app installed the service:
    /// rewrite the definition to this app's engine (`server install`), then
    /// restart from it.
    Repoint,
    /// The service runs a `rustcode` this app does not install (Homebrew's,
    /// `cargo install`'s): say which, once, rather than restarting into the
    /// same old copy at every launch.
    Foreign { path: String },
}

/// The facts the catch-up decides on.
#[derive(Debug, Clone, Copy)]
struct CatchUp<'a> {
    installed: bool,
    running: bool,
    /// The running server's version.
    server: Option<&'a str>,
    /// This app's engine version (the `rustcode` that answered the status).
    engine: Option<&'a str>,
    /// The program the service definition names.
    binary: Option<&'a str>,
    /// This app's engine, as a path.
    engine_binary: &'a Path,
    /// The copy `update-cli` replaced, if it replaced one.
    updated: Option<&'a str>,
    /// The program the app installed the service with, when it did: the one
    /// definition this app may rewrite.
    app_program: Option<&'a Path>,
}

/// Whether two paths name the same file, a link and its target included.
fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

fn server_catch_up(catch_up: &CatchUp<'_>) -> ServerCatchUp {
    let CatchUp {
        installed,
        running,
        server,
        engine,
        binary,
        engine_binary,
        updated,
        app_program,
    } = *catch_up;
    let (true, true, Some(server), Some(engine), Some(binary)) =
        (installed, running, server, engine, binary)
    else {
        return ServerCatchUp::Leave;
    };
    // A server on this engine or a newer one is not behind: never restart it
    // into an older copy (a Homebrew or beta server, an app that was lowered).
    if !crate::updater::newer(engine, server) {
        return ServerCatchUp::Leave;
    }
    let binary = Path::new(binary);
    // The unit runs this app's engine, or the copy `update-cli` just replaced
    // with this engine's bytes: a restart converges.
    if same_path(binary, engine_binary)
        || updated.is_some_and(|updated| same_path(binary, Path::new(updated)))
    {
        return ServerCatchUp::Restart;
    }
    // The app installed this definition, and it still names what the app put
    // there (the app moved, or an AppImage's copy did): rewriting it is the
    // app's own repair.
    if app_program.is_some_and(|program| same_path(binary, program)) {
        return ServerCatchUp::Repoint;
    }
    ServerCatchUp::Foreign {
        path: binary.display().to_string(),
    }
}

/// Why a restart would not bring the server up to this app's engine, in the
/// words the page shows: the service runs another `rustcode`, older than this
/// one, that this app does not install. `None` when there is nothing to say.
fn foreign_server(report: &Report, engine_binary: &Path) -> Option<String> {
    let server = report.version.as_deref()?;
    let engine = report.cli_version.as_deref()?;
    let binary = Path::new(report.binary.as_deref()?);
    if !report.installed || !report.running || !crate::updater::newer(engine, server) {
        return None;
    }
    if same_path(binary, engine_binary) {
        return None;
    }
    Some(format!(
        "Server runs an older rustcode at {} (it runs {server}, this app ships {engine}). Update that copy, or install the server from this app.",
        binary.display()
    ))
}

/// A file under the config folder's `service` directory, which the app keeps
/// for the service it installed.
fn service_state(name: &str) -> Option<PathBuf> {
    let dir = rustcode_client::config::config_dir().ok()?;
    Some(dir.join("service").join(name))
}

fn write_service_state(name: &str, text: &str) {
    let Some(path) = service_state(name) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

/// Record that this app installed the server, and with which program, so the
/// catch-up may re-point the definition at its engine — and only while the
/// definition still names what the app installed. A service a terminal
/// installed, or reinstalled, is never rewritten.
pub fn remember_installed_by_app(program: &Path) {
    write_service_state("installed-by-app", &format!("{}\n", program.display()));
}

/// The program the app installed the service with, if it installed it. A
/// definition naming another binary is a terminal's, whatever this says.
fn installed_by_app_program() -> Option<PathBuf> {
    let text = std::fs::read_to_string(service_state("installed-by-app")?).ok()?;
    let program = text.trim();
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// One catch-up attempt, so a pair that cannot converge is not tried again at
/// every launch: the server's version, this engine's, and the binary the
/// service names. Any of them changing makes it a fresh decision.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct Attempt {
    server: String,
    engine: String,
    binary: Option<String>,
}

fn remembered_attempt() -> Option<Attempt> {
    let path = service_state("catch-up.json")?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn remember_attempt(attempt: Option<&Attempt>) {
    match attempt {
        Some(attempt) => {
            if let Ok(text) = serde_json::to_string(attempt) {
                write_service_state("catch-up.json", &text);
            }
        }
        None => {
            if let Some(path) = service_state("catch-up.json") {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// The copies outside the app catch up with the engine this build ships, at
/// every start, because an update is installed between two of them: the
/// AppImage's copy a service runs ([`refresh_program`]), the `rustcode` a shell
/// runs (issue #3487), and a background server still running the engine from
/// before the update. Each step is best effort: the app runs even when one
/// cannot finish, and the page that asks for the status names what is left.
pub fn catch_up_after_update() {
    refresh_program();
    // A development build's engine is not the released one a service or a
    // terminal should run: neither catch applies to it.
    if cfg!(debug_assertions) {
        return;
    }
    let updated = refresh_installed_cli();
    restart_outdated_server(updated.as_deref());
}

/// Bring the `rustcode` a shell runs up to this app's version, and answer the
/// copy it replaced, if any. The engine decides: it knows the version it was
/// built as, refuses a development build, and leaves Homebrew's and `cargo
/// install`'s copies to their tools with a line saying so.
fn refresh_installed_cli() -> Option<PathBuf> {
    let output = match run_sync(&["server", "update-cli", "--json"], INSTALL_TIMEOUT) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("[service] could not update the rustcode a terminal runs: {error}");
            return None;
        }
    };
    let Ok(report) = serde_json::from_slice::<UpdatedCli>(&output.stdout) else {
        eprintln!(
            "[service] update-cli did not answer as JSON: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    };
    if output.status.success() {
        if report.outcome == "updated" {
            eprintln!(
                "[service] {} now runs this app's version.",
                report
                    .path
                    .as_deref()
                    .unwrap_or("the rustcode a terminal runs")
            );
        }
    } else {
        eprintln!(
            "[service] {}",
            report
                .message
                .unwrap_or_else(|| "a terminal's rustcode was left as it was.".to_owned())
        );
        return None;
    }
    if report.outcome == "updated" {
        return report.path.map(PathBuf::from);
    }
    None
}

/// Restart a background server that still runs an older engine, once per
/// version pair: `server install` first when the service definition names
/// another copy the app installed it with.
fn restart_outdated_server(updated: Option<&Path>) {
    let Some(report) = read_report() else {
        return;
    };
    let app_program = installed_by_app_program();
    let catch_up = server_catch_up(&CatchUp {
        installed: report.installed,
        running: report.running,
        server: report.version.as_deref(),
        engine: report.cli_version.as_deref(),
        binary: report.binary.as_deref(),
        engine_binary: &service_program(),
        updated: updated.map(|path| path.to_string_lossy()).as_deref(),
        app_program: app_program.as_deref(),
    });
    if catch_up == ServerCatchUp::Leave {
        // The engine and the server agree: a new update gets a fresh attempt.
        remember_attempt(None);
        return;
    }
    let attempt = Attempt {
        server: report.version.clone().unwrap_or_default(),
        engine: report.cli_version.clone().unwrap_or_default(),
        binary: report.binary.clone(),
    };
    if remembered_attempt().as_ref() == Some(&attempt) {
        return;
    }
    match &catch_up {
        ServerCatchUp::Leave => return,
        ServerCatchUp::Foreign { path } => {
            eprintln!(
                "[service] the RustCode server runs an older rustcode at {path}: this app does not install that copy, so it is left alone."
            );
            remember_attempt(Some(&attempt));
            return;
        }
        ServerCatchUp::Repoint => {
            // `server install` rewrites the definition and leaves a running
            // server alone (it says the restart applies it), so the restart
            // below is still the one that ends turns, and it waits for idle.
            match run_sync(&["server", "install"], INSTALL_TIMEOUT) {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    eprintln!(
                        "[service] the server could not be pointed at this app: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                    remember_attempt(Some(&attempt));
                    return;
                }
                Err(error) => {
                    eprintln!("[service] the server could not be pointed at this app: {error}");
                    remember_attempt(Some(&attempt));
                    return;
                }
            }
        }
        ServerCatchUp::Restart => {}
    }
    // `--when-idle` waits for running turns, so a turn in the middle of its
    // work is not cut off. This thread's own deadline can end that wait, which
    // is not a failure: nothing is remembered, and the next start asks again.
    let busy = restart_when_idle();
    let after = read_report();
    let outcome = after_restart(
        busy,
        after.as_ref().and_then(|report| report.version.as_deref()),
        after
            .as_ref()
            .and_then(|report| report.cli_version.as_deref()),
    );
    match outcome {
        // Server and engine are one version now: a new update gets a fresh try.
        AfterRestart::Converged => remember_attempt(None),
        // A turn outlasted this start's patience: remembered nothing.
        AfterRestart::StillBusy => {}
        // A refusal, or a server that came back on the old engine: remembered,
        // so the same pair is not attempted at every launch.
        AfterRestart::Failed => remember_attempt(Some(&attempt)),
    }
}

/// What a restart left behind, which decides whether it is remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterRestart {
    /// The server runs this engine's version (or a newer one) now.
    Converged,
    /// This thread's deadline ended the `--when-idle` wait: a turn was still
    /// running, so the next start tries again instead of remembering a failure
    /// that never happened.
    StillBusy,
    /// The CLI refused, or the server came back on the old engine.
    Failed,
}

fn after_restart(busy: bool, server: Option<&str>, engine: Option<&str>) -> AfterRestart {
    if busy {
        return AfterRestart::StillBusy;
    }
    match (server, engine) {
        (Some(server), Some(engine)) if !crate::updater::newer(engine, server) => {
            AfterRestart::Converged
        }
        _ => AfterRestart::Failed,
    }
}

/// Run `server restart --when-idle`, and answer whether this thread's deadline
/// ended the wait (running turns) rather than the CLI answering.
fn restart_when_idle() -> bool {
    match run_sync(&["server", "restart", "--when-idle"], RESTART_IDLE_TIMEOUT) {
        Ok(output) if output.status.success() => false,
        Ok(output) => {
            eprintln!(
                "[service] the server did not restart: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            false
        }
        Err(error) => {
            if cut_short(&error) {
                eprintln!(
                    "[service] the server restart is still waiting for running turns; the next start tries again."
                );
                true
            } else {
                eprintln!("[service] the server did not restart: {error}");
                false
            }
        }
    }
}

/// Whether a command error is this thread's own deadline rather than a failure:
/// `run_program`'s timeout wording.
fn cut_short(error: &str) -> bool {
    error.contains("did not answer within")
}

/// The `rustcode` the service commands run and that `server install` writes
/// into the unit: the binary the app runs, or — inside an AppImage, where that
/// binary sits in a mount that disappears — a copy of it under the user's data
/// directory.
fn prepare_program() -> Result<PathBuf, String> {
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    program_for(
        resolve_engine(),
        appimage.as_deref(),
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
}

/// [`prepare_program`] with the environment named, so a test can watch the
/// copy without setting process-wide variables. `appimage` is `$APPIMAGE`, the
/// file the user launched.
fn program_for(
    engine: PathBuf,
    appimage: Option<&Path>,
    data_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, String> {
    if !needs_copy(&engine, appimage.is_some()) {
        return Ok(engine);
    }
    let target = stable_program(&engine, appimage.is_some(), data_home, home).ok_or_else(|| {
        "Cannot install the RustCode server from an AppImage: there is no data directory to \
         copy it to (HOME and XDG_DATA_HOME are both unset), and the AppImage mount is gone \
         when the app closes."
            .to_owned()
    })?;
    copy_program(&engine, &target, appimage)?;
    Ok(target)
}

/// Whether a service definition naming `exe` would point into an AppImage
/// mount: run from one (`$APPIMAGE`), or living under `/tmp/.mount_*`.
fn needs_copy(exe: &Path, appimage: bool) -> bool {
    appimage
        || exe.components().any(|component| {
            component
                .as_os_str()
                .to_string_lossy()
                .starts_with(".mount_")
        })
}

/// Where a service should run from instead: `$XDG_DATA_HOME/rustcode/bin`, or
/// `~/.local/share/rustcode/bin`. `None` when the binary does not need a copy
/// or there is no data directory to put one in.
pub fn stable_program(
    exe: &Path,
    appimage: bool,
    data_home: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if !needs_copy(exe, appimage) {
        return None;
    }
    let data = data_home
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| {
            home.filter(|dir| !dir.as_os_str().is_empty())
                .map(|dir| dir.join(".local").join("share"))
        })?;
    Some(data.join("rustcode").join("bin").join(if cfg!(windows) {
        "rustcode.exe"
    } else {
        "rustcode"
    }))
}

/// Copy `source` to `target`, unless `target` is already that binary. Written
/// beside the target and renamed over it, so a server running the old copy
/// keeps its inode and a half-copied file never becomes the service's program.
///
/// `appimage` is the file the user launches: it is rewritten by every update,
/// so its timestamp (`the AppImage was installed after the copy was made`) is
/// what makes an updated AppImage refresh the copy even when its bundled
/// binary kept the timestamp the image was built with.
fn copy_program(source: &Path, target: &Path, appimage: Option<&Path>) -> Result<(), String> {
    if let Ok(to) = std::fs::metadata(target) {
        let to_time = to.modified().ok();
        let newer = |path: &Path| {
            let time = std::fs::metadata(path)
                .and_then(|from| from.modified())
                .ok();
            matches!((time, to_time), (Some(time), Some(to_time)) if time > to_time)
        };
        let length = std::fs::metadata(source).map(|from| from.len()).ok();
        let stale = length != Some(to.len()) || newer(source) || appimage.is_some_and(newer);
        if !stale {
            return Ok(());
        }
    }
    let failure = |error: std::io::Error| {
        format!(
            "Cannot install the RustCode server from an AppImage: {} could not be copied to {} \
             ({error}). Install the .deb, or run `rustcode server install` from a RustCode that \
             lives outside an AppImage.",
            source.display(),
            target.display()
        )
    };
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(failure)?;
    }
    let temp = target.with_extension("new");
    std::fs::copy(source, &temp).map_err(failure)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).map_err(failure)?;
    }
    // `std::fs::copy` keeps the source's timestamp on macOS, where the copy is
    // new: the next update is measured against when the copy was made.
    if let Ok(file) = std::fs::File::options().write(true).open(&temp) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
    std::fs::rename(&temp, target).map_err(failure)
}

/// [`run`] with the program named, so a test can watch the deadline without a
/// `rustcode` binary.
fn run_program(
    program: &std::path::Path,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let mut child = {
        let mut command = process::command(program);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
            .spawn()
            .map_err(|error| format!("Could not run {}: {error}", program.display()))?
    };
    // The pipes are read on a thread of their own: a command that prints more
    // than a pipe buffer holds would otherwise wait on a reader forever.
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut err = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut out);
        }
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut err);
        }
        (out, err)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                kill(&mut child);
                let _ = child.wait();
                return Err(format!(
                    "{} did not answer within {} seconds.",
                    program.display(),
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                kill(&mut child);
                return Err(format!("Could not wait for {}: {error}", program.display()));
            }
        }
    };
    let (stdout, stderr) = reader.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Stop a command that outlived its deadline, and whatever it started.
fn kill(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_report_is_what_the_page_reads() {
        let report: Report = serde_json::from_str(
            r#"{
                "installed": true,
                "running": true,
                "version": "1.28.0",
                "cli_version": "1.28.0",
                "pid": 48213,
                "uptime_secs": 61,
                "socket": "/Users/lars/.config/rustcode/run/server.sock",
                "binary": "/Applications/RustCode.app/Contents/MacOS/rustcode",
                "service": {"kind": "launchd", "path": null},
                "clients": 1,
                "running_turns": 0,
                "log": "/Users/lars/.config/rustcode/logs/server.log",
                "problems": [{"code": "restart-pending", "message": "A restart waits for the turns to finish."}]
            }"#,
        )
        .expect("the CLI's own JSON");
        let status = ServiceStatus {
            installed: report.installed,
            running: report.running,
            version: report.version,
            cli_version: report.cli_version,
            gateway: report.gateway,
            path_rustcode: report.path_rustcode,
            problems: report.problems,
            error: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["installed"], true);
        assert_eq!(json["running"], true);
        assert_eq!(json["version"], "1.28.0");
        assert_eq!(json["cliVersion"], "1.28.0");
        assert_eq!(json["problems"][0]["code"], "restart-pending");
        assert_eq!(json["error"], serde_json::Value::Null);
        // A report from a `rustcode` that predates the gateway keeps reading:
        // the page says the gateway is not running rather than failing.
        assert_eq!(json["gateway"]["running"], false);
        assert!(json["pathRustcode"].is_null());
    }

    /// The gateway and the shell's `rustcode` reach the page as the CLI
    /// reports them (issues #3411, #3412).
    #[test]
    fn the_gateway_and_the_shells_rustcode_come_through() {
        let report: Report = serde_json::from_str(
            r#"{
                "installed": true,
                "running": true,
                "version": "1.28.0",
                "cli_version": "1.28.0",
                "gateway": {
                    "remote_enabled": true,
                    "running": true,
                    "address": "100.90.28.23:17879",
                    "advertised": "100.90.28.23:17879",
                    "owner": "server",
                    "version": "1.28.0",
                    "pid": 97702,
                    "paired_devices": 2,
                    "port": 17879
                },
                "path_rustcode": {"path": "/opt/homebrew/bin/rustcode", "version": "0.96.0"},
                "problems": [{"code": "path-version-mismatch", "message": "older"}]
            }"#,
        )
        .expect("the CLI's JSON with a gateway");
        let status = serde_json::to_value(ServiceStatus {
            installed: report.installed,
            running: report.running,
            version: report.version,
            cli_version: report.cli_version,
            gateway: report.gateway,
            path_rustcode: report.path_rustcode,
            problems: report.problems,
            error: None,
        })
        .unwrap();
        assert_eq!(status["gateway"]["owner"], "server");
        assert_eq!(status["gateway"]["address"], "100.90.28.23:17879");
        assert_eq!(status["gateway"]["pairedDevices"], 2);
        assert_eq!(status["pathRustcode"]["path"], "/opt/homebrew/bin/rustcode");
        assert_eq!(status["pathRustcode"]["version"], "0.96.0");
    }

    /// A status with nothing installed is a state, not an error.
    #[test]
    fn a_server_that_is_not_installed_still_answers() {
        let report: Report = serde_json::from_str(
            r#"{"installed": false, "running": false, "cli_version": "1.28.0"}"#,
        )
        .expect("the shortest report");
        assert!(!report.installed && !report.running);
        assert!(report.version.is_none());
        assert!(report.problems.is_empty());
    }

    /// A command that hangs is killed at the deadline instead of holding the
    /// page: the same rule the engine applies to the credential store.
    #[cfg(unix)]
    #[test]
    fn a_command_that_never_answers_is_killed() {
        let started = Instant::now();
        let output = run_program(
            std::path::Path::new("/bin/sh"),
            &["-c", "sleep 30"],
            Duration::from_millis(200),
        );
        let error = output.expect_err("a hanging command times out");
        assert!(error.contains("did not answer"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// An AppImage's sidecar lives in a mount that disappears with the app, so
    /// the unit must name a copy that outlives it: `$XDG_DATA_HOME/rustcode/bin`.
    #[test]
    fn an_appimage_is_installed_from_a_copy_under_the_data_directory() {
        let mount = Path::new("/tmp/.mount_RustCoJpCJlA/usr/bin/rustcode");
        assert_eq!(
            stable_program(
                mount,
                true,
                Some(Path::new("/home/lars/.local/share")),
                Some(Path::new("/home/lars"))
            ),
            Some(PathBuf::from(
                "/home/lars/.local/share/rustcode/bin/rustcode"
            ))
        );
        // `$XDG_DATA_HOME` wins; without it the default data directory is used.
        assert_eq!(
            stable_program(mount, true, None, Some(Path::new("/home/lars"))),
            Some(PathBuf::from(
                "/home/lars/.local/share/rustcode/bin/rustcode"
            ))
        );
        // The path itself says it, even when `$APPIMAGE` is not in the env.
        assert_eq!(
            stable_program(mount, false, None, Some(Path::new("/home/lars"))),
            Some(PathBuf::from(
                "/home/lars/.local/share/rustcode/bin/rustcode"
            ))
        );
        // An empty `XDG_DATA_HOME` is not a directory.
        assert_eq!(
            stable_program(
                mount,
                true,
                Some(Path::new("")),
                Some(Path::new("/home/lars"))
            ),
            Some(PathBuf::from(
                "/home/lars/.local/share/rustcode/bin/rustcode"
            ))
        );
    }

    /// A binary that already outlives the app is installed where it is: a
    /// .deb, a Homebrew keg, a Windows install. Nothing is copied.
    #[test]
    fn a_binary_outside_an_appimage_is_installed_where_it_is() {
        assert_eq!(
            stable_program(
                Path::new("/usr/bin/rustcode"),
                false,
                None,
                Some(Path::new("/home/lars"))
            ),
            None
        );
        assert!(!needs_copy(Path::new("/usr/bin/rustcode"), false));
        assert!(needs_copy(Path::new("/usr/bin/rustcode"), true));
    }

    /// With no home and no `XDG_DATA_HOME` there is nowhere to put the copy:
    /// the install is refused instead of writing a unit for the mount.
    #[test]
    fn no_data_directory_refuses_the_copy() {
        let mount = Path::new("/tmp/.mount_RustCoJpCJlA/usr/bin/rustcode");
        assert_eq!(stable_program(mount, true, None, None), None);
    }

    /// The copy is executable, and a second run keeps it instead of copying
    /// again; a new AppImage is newer than the copy, so it is refreshed.
    #[cfg(unix)]
    #[test]
    fn the_copy_is_written_once_and_refreshed_by_a_new_appimage() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("rc-service-copy-{}", std::process::id()));
        let mount = root.join(".mount_RustCoJpCJlA").join("usr/bin");
        std::fs::create_dir_all(&mount).unwrap();
        let source = mount.join("rustcode");
        std::fs::write(&source, b"#!/bin/sh\necho 1.28.0\n").unwrap();
        let target = root.join(".local/share/rustcode/bin/rustcode");

        copy_program(&source, &target, None).expect("the copy is made");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            std::fs::read(&source).unwrap()
        );
        assert_ne!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o111,
            0
        );

        // Nothing changed: the copy is left alone, timestamps and all.
        let before = std::fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        copy_program(&source, &target, None).expect("a fresh copy is not remade");
        assert_eq!(
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            before
        );

        // A new AppImage is a newer file than the copy, even when its bundled
        // binary kept the build's timestamp: the copy is refreshed.
        let appimage = root.join("RustCode.AppImage");
        std::fs::write(&appimage, b"image").unwrap();
        copy_program(&source, &target, Some(&appimage)).expect("a new install refreshes it");
        let after = std::fs::metadata(&target).unwrap().modified().unwrap();
        assert!(after > before);

        let _ = std::fs::remove_dir_all(root);
    }

    /// The command runs the copy, so the unit `server install` writes names it:
    /// the CLI takes `current_exe()` of the process it runs in.
    #[cfg(unix)]
    #[test]
    fn the_service_commands_run_the_copy_not_the_mount() {
        let root = std::env::temp_dir().join(format!("rc-service-run-{}", std::process::id()));
        let mount = root.join(".mount_RustCoJpCJlA").join("usr/bin");
        std::fs::create_dir_all(&mount).unwrap();
        let engine = mount.join("rustcode");
        std::fs::write(
            &engine,
            b"#!/bin/sh\necho \"$0\"\necho \"installed=$(dirname \"$0\")\"\n",
        )
        .unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let appimage = root.join("RustCode.AppImage");
        std::fs::write(&appimage, b"image").unwrap();

        let program = program_for(engine, Some(&appimage), None, Some(&home)).expect("the copy");
        assert_eq!(program, home.join(".local/share/rustcode/bin/rustcode"));
        assert!(!program.starts_with(mount), "{}", program.display());
        let output = run_program(&program, &["server", "status"], Duration::from_secs(5))
            .expect("the copy runs");
        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(
            printed.starts_with(&program.display().to_string()),
            "{printed}"
        );
        assert!(!printed.contains(".mount_"), "{printed}");

        let _ = std::fs::remove_dir_all(root);
    }

    /// A copy that cannot be made is refused with the reason, never a unit
    /// naming a path inside the AppImage mount.
    #[cfg(unix)]
    #[test]
    fn a_copy_that_cannot_be_made_is_refused() {
        let root = std::env::temp_dir().join(format!("rc-service-nocopy-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("rustcode");
        std::fs::write(&source, b"binary").unwrap();
        // A file where the directory should be: creating the copy's directory fails.
        let blocker = root.join("blocked");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let error = copy_program(&source, &blocker.join("bin/rustcode"), None)
            .expect_err("a copy into a file is refused");
        assert!(
            error.contains("Cannot install the RustCode server from an AppImage"),
            "{error}"
        );
        assert!(error.contains("could not be copied"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    const ENGINE: &str = "/Applications/RustCode.app/Contents/MacOS/rustcode";

    /// The catch-up's decision for one reading of `server status`.
    fn catch_up(
        server: Option<&str>,
        engine: &str,
        binary: Option<&str>,
        updated: Option<&str>,
        app_program: Option<&str>,
    ) -> ServerCatchUp {
        server_catch_up(&CatchUp {
            installed: true,
            running: true,
            server,
            engine: Some(engine),
            binary,
            engine_binary: Path::new(ENGINE),
            updated,
            app_program: app_program.map(Path::new),
        })
    }

    /// Only a server the service starts is restarted, only when it is older,
    /// and only into a binary this app manages (issue #3487).
    #[test]
    fn only_an_older_installed_server_is_restarted() {
        // The service runs this app's engine: a restart brings the new one.
        assert_eq!(
            catch_up(Some("1.25.0"), "1.30.0", Some(ENGINE), None, None),
            ServerCatchUp::Restart
        );
        // Or a copy `update-cli` just replaced with this engine's bytes.
        let copy = "/Users/ada/.local/bin/rustcode";
        assert_eq!(
            catch_up(Some("1.25.0"), "1.30.0", Some(copy), Some(copy), None),
            ServerCatchUp::Restart
        );
        // A beta of the release this app is: still behind it.
        assert_eq!(
            catch_up(Some("1.30.0-beta.2"), "1.30.0", Some(ENGINE), None, None),
            ServerCatchUp::Restart
        );
        // The same engine, and one ahead of the app: nothing to do, and never
        // a downgrade.
        assert_eq!(
            catch_up(Some("1.30.0"), "1.30.0", Some(ENGINE), None, None),
            ServerCatchUp::Leave
        );
        assert_eq!(
            catch_up(Some("1.31.0"), "1.30.0", Some(ENGINE), None, None),
            ServerCatchUp::Leave
        );
        // A version that cannot be read is not a reason to restart.
        assert_eq!(
            catch_up(Some("dev"), "1.30.0", Some(ENGINE), None, None),
            ServerCatchUp::Leave
        );
        // No service, nothing running, or a definition with no binary.
        let off = |installed: bool, running: bool| {
            server_catch_up(&CatchUp {
                installed,
                running,
                server: Some("1.25.0"),
                engine: Some("1.30.0"),
                binary: Some(ENGINE),
                engine_binary: Path::new(ENGINE),
                updated: None,
                app_program: None,
            })
        };
        assert_eq!(off(false, true), ServerCatchUp::Leave);
        assert_eq!(off(true, false), ServerCatchUp::Leave);
        assert_eq!(
            catch_up(Some("1.25.0"), "1.30.0", None, None, None),
            ServerCatchUp::Leave
        );
    }

    /// The unit runs another `rustcode`: the app rewrites it only while the
    /// definition still names the program it installed, and otherwise names the
    /// copy once instead of restarting into it at every launch.
    #[test]
    fn another_rustcode_in_the_unit_is_repointed_or_named() {
        let copy = "/Users/ada/.local/bin/rustcode";
        // Installed by the app, and still naming what it installed.
        assert_eq!(
            catch_up(Some("1.29.0"), "1.30.0", Some(copy), None, Some(copy)),
            ServerCatchUp::Repoint
        );
        // Installed by a terminal, or reinstalled since: never rewritten.
        assert_eq!(
            catch_up(Some("1.29.0"), "1.30.0", Some(copy), None, None),
            ServerCatchUp::Foreign {
                path: copy.to_owned()
            }
        );
        assert_eq!(
            catch_up(
                Some("1.29.0"),
                "1.30.0",
                Some(copy),
                None,
                Some("/Applications/RustCode.app/Contents/MacOS/rustcode")
            ),
            ServerCatchUp::Foreign {
                path: copy.to_owned()
            }
        );
        // A copy update-cli updated is the app's, wherever the unit found it.
        assert_eq!(
            catch_up(Some("1.29.0"), "1.30.0", Some(copy), Some(copy), None),
            ServerCatchUp::Restart
        );
    }

    /// A `--when-idle` restart this start cut short waits for a turn, which is
    /// not a failure: it is not remembered, and the next start asks again
    /// (issue #3487).
    #[test]
    fn a_long_turn_is_not_remembered_as_a_failure() {
        assert_eq!(
            after_restart(true, Some("1.29.0"), Some("1.30.0")),
            AfterRestart::StillBusy
        );
        // A restart that answered but did not take is remembered.
        assert_eq!(
            after_restart(false, Some("1.29.0"), Some("1.30.0")),
            AfterRestart::Failed
        );
        // Nothing answered after the restart: remembered too.
        assert_eq!(after_restart(false, None, None), AfterRestart::Failed);
        // Converged, including a server that came back newer than the app.
        assert_eq!(
            after_restart(false, Some("1.30.0"), Some("1.30.0")),
            AfterRestart::Converged
        );
        assert_eq!(
            after_restart(false, Some("1.31.0"), Some("1.30.0")),
            AfterRestart::Converged
        );
    }

    /// The page says which copy the server runs when a restart would bring the
    /// same one back, instead of "restart brings it up to date".
    #[test]
    fn a_server_on_another_copy_is_named() {
        let report = |server: &str, engine: &str, binary: &str| Report {
            installed: true,
            running: true,
            version: Some(server.to_owned()),
            cli_version: Some(engine.to_owned()),
            binary: Some(binary.to_owned()),
            gateway: Gateway::default(),
            path_rustcode: None,
            problems: Vec::new(),
        };
        let message = foreign_server(
            &report("1.29.0", "1.30.0", "/Users/ada/.local/bin/rustcode"),
            Path::new(ENGINE),
        )
        .expect("an older copy the app does not manage");
        assert!(
            message.contains("Server runs an older rustcode at /Users/ada/.local/bin/rustcode"),
            "{message}"
        );
        assert!(message.contains("it runs 1.29.0"), "{message}");
        // The app's own engine, a newer server, and an equal version say
        // nothing.
        assert_eq!(
            foreign_server(&report("1.29.0", "1.30.0", ENGINE), Path::new(ENGINE)),
            None
        );
        assert_eq!(
            foreign_server(
                &report("1.31.0", "1.30.0", "/Users/ada/.local/bin/rustcode"),
                Path::new(ENGINE)
            ),
            None
        );
        assert_eq!(
            foreign_server(
                &report("1.30.0", "1.30.0", "/Users/ada/.local/bin/rustcode"),
                Path::new(ENGINE)
            ),
            None
        );
    }

    /// What the command printed comes back with its exit status.
    #[cfg(unix)]
    #[test]
    fn a_commands_output_is_kept() {
        let output = run_program(
            std::path::Path::new("/bin/echo"),
            &["{\"installed\": false}"],
            Duration::from_secs(5),
        )
        .expect("echo answers");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            r#"{"installed": false}"#
        );
    }
}
