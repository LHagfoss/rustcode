//! `_rustcode/machine/update`: update RustCode on this machine when another
//! machine in the fleet asks (Settings › Machines › **Update**, issue #4153).
//!
//! After a release the main computer showed a paired machine as outdated and
//! could only say "update it there". With this, the paired app asks the
//! machine to update itself:
//!
//! - `start` installs the newest stable release exactly as the server's own
//!   auto-update does (`server/auto_update.rs`: the archive checked against
//!   `SHA256SUMS`, Homebrew through `brew`), then restarts onto it with
//!   `rc server restart --when-idle`, so no running turn is cut off.
//! - A binary inside an app (a macOS `.app` bundle, a `.deb`, an AppImage
//!   mount) is never replaced in place: that breaks the app's signature or its
//!   package. The request is left for the app on this machine instead, which
//!   claims it (`claim`) and installs its own update once its threads are idle.
//! - `status` says where the update stands, so the asking row can show it.
//!
//! A development or prerelease build refuses, as the auto-update does. The
//! work runs once at a time; asking again while it runs only reads it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustcode_core::update::{Version, format_version};
use serde_json::{Value, json};

use crate::server::auto_update::{Build, build_kind};
use crate::service::server_service as service;
use crate::update::{current_version, latest_available_version, run_update};

/// `_rustcode/machine/update`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/machine/update", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MachineUpdateRequest {
    #[serde(default)]
    pub action: Action,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Action {
    /// Where an update stands.
    #[default]
    Status,
    /// Start one, unless one already runs.
    Start,
    /// For the app on this machine: take a request left for it.
    Claim,
}

/// Where the update stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Checking,
    Installing(Version),
    /// Installed; the server restarts onto it once no turn runs.
    Restarting(Version),
    UpToDate(Version),
    /// The app on this machine updates it, and was asked to.
    AskedApp,
    Failed(String),
}

impl Phase {
    fn busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Installing(_) | Self::Restarting(_)
        )
    }
}

/// What `start` does on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    Install,
    AskApp,
    Refuse(&'static str),
}

/// A request older than this is left over from another time, not an ask now.
const REQUEST_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const REQUEST_FILE: &str = "update-request.json";

static PHASE: Mutex<Phase> = Mutex::new(Phase::Idle);

fn phase() -> Phase {
    PHASE.lock().map(|p| p.clone()).unwrap_or(Phase::Idle)
}

fn set(next: Phase) {
    if let Ok(mut phase) = PHASE.lock() {
        *phase = next;
    }
}

/// Whether `program` belongs to an app or a package, which replaces it as a
/// whole: a macOS bundle, a `.deb`'s `/usr`, an AppImage's mount.
pub(crate) fn managed_by_app(program: &Path) -> bool {
    let text = program.to_string_lossy();
    text.contains(".app/Contents/")
        || text.starts_with("/usr/bin/")
        || text.starts_with("/usr/lib/")
        || text.starts_with("/tmp/.mount_")
}

/// The decision, from the build and where the binary lives. Pure.
pub(crate) fn plan(build: Build, app: bool) -> Plan {
    match build {
        Build::Development => Plan::Refuse("a development build does not update itself"),
        Build::Prerelease => {
            Plan::Refuse("a beta build updates through the beta channel, not from another machine")
        }
        Build::Release if app => Plan::AskApp,
        Build::Release => Plan::Install,
    }
}

/// The answer for `phase`, with the running turns a waiting restart waits on.
pub(crate) fn answer(phase: &Phase, running_turns: Option<usize>) -> Value {
    let current = env!("CARGO_PKG_VERSION");
    let (name, target, error) = match phase {
        Phase::Idle => ("idle", None, None),
        Phase::Checking => ("checking", None, None),
        Phase::Installing(v) => ("installing", Some(*v), None),
        Phase::Restarting(v) => ("restarting", Some(*v), None),
        Phase::UpToDate(v) => ("upToDate", Some(*v), None),
        Phase::AskedApp => ("app", None, None),
        Phase::Failed(error) => ("failed", None, Some(error.clone())),
    };
    let mut value = json!({ "phase": name, "current": current });
    if let Some(target) = target {
        value["target"] = json!(format_version(target));
    }
    if let Some(error) = error {
        value["error"] = json!(error);
    }
    if let (Phase::Restarting(_), Some(running)) = (phase, running_turns) {
        value["runningTurns"] = json!(running);
    }
    value
}

/// Answer `request`.
pub(crate) async fn handle(request: MachineUpdateRequest) -> Result<Value, String> {
    match request.action {
        Action::Status => {}
        Action::Start => start()?,
        Action::Claim => return Ok(claim()),
    }
    let now = phase();
    let running = if matches!(now, Phase::Restarting(_)) {
        service::running_turns().await
    } else {
        None
    };
    Ok(answer(&now, running))
}

fn start() -> Result<(), String> {
    if phase().busy() {
        return Ok(());
    }
    let program = crate::service::executable_path()
        .map_err(|error| format!("could not find this RustCode's binary: {error}"))?;
    match plan(build_kind(&program), managed_by_app(&program)) {
        Plan::Refuse(why) => Err(why.to_owned()),
        Plan::AskApp => {
            ask_app()?;
            crate::dbg_log!("RustCode update: asked by another machine; left for the app");
            set(Phase::AskedApp);
            Ok(())
        }
        Plan::Install => {
            set(Phase::Checking);
            tokio::spawn(install(program));
            Ok(())
        }
    }
}

async fn install(program: PathBuf) {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(10 * 60))
        .build()
    {
        Ok(client) => client,
        Err(error) => return set(Phase::Failed(format!("could not start: {error}"))),
    };
    let Some(latest) = latest_available_version(&client).await else {
        return set(Phase::Failed(
            "could not read the latest release from GitHub".to_owned(),
        ));
    };
    if latest <= current_version() {
        return set(Phase::UpToDate(latest));
    }
    let version = format_version(latest);
    crate::dbg_log!("RustCode update: asked by another machine; installing {version}");
    set(Phase::Installing(latest));
    if let Err(error) = run_update(&client, latest).await {
        crate::dbg_log!("RustCode update: installing {version} failed: {error}");
        return set(Phase::Failed(format!(
            "installing {version} failed: {error}"
        )));
    }
    crate::dbg_log!("RustCode update: installed {version}; restarting once no turn is running");
    match service::spawn_detached_restart_from(&program, true) {
        Ok(()) => set(Phase::Restarting(latest)),
        Err(error) => set(Phase::Failed(format!(
            "installed {version}, but the server could not restart onto it: {error:#}"
        ))),
    }
}

fn request_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(REQUEST_FILE))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn ask_app() -> Result<(), String> {
    let path = request_path().ok_or("no configuration folder on this machine")?;
    let body = json!({ "requestedAt": now_ms() }).to_string();
    std::fs::write(&path, body)
        .map_err(|error| format!("could not leave the request for the app: {error}"))
}

/// Whether a request written at `at` still asks, at `now` (both ms).
pub(crate) fn request_fresh(at: u64, now: u64) -> bool {
    at <= now.saturating_add(60_000)
        && now.saturating_sub(at) < u64::try_from(REQUEST_TTL.as_millis()).unwrap_or(u64::MAX)
}

/// Take the request left for the app, if any: it is answered once.
fn claim() -> Value {
    let Some(path) = request_path() else {
        return json!({ "requested": false });
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return json!({ "requested": false });
    };
    let _ = std::fs::remove_file(&path);
    let at = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| value.get("requestedAt").and_then(Value::as_u64));
    let requested = at.is_some_and(|at| request_fresh(at, now_ms()));
    json!({ "requested": requested })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_installs_an_app_is_asked_and_other_builds_refuse() {
        assert_eq!(plan(Build::Release, false), Plan::Install);
        assert_eq!(plan(Build::Release, true), Plan::AskApp);
        assert!(matches!(plan(Build::Development, false), Plan::Refuse(_)));
        assert!(matches!(plan(Build::Prerelease, true), Plan::Refuse(_)));
    }

    #[test]
    fn a_binary_inside_an_app_or_a_package_is_the_apps() {
        assert!(managed_by_app(Path::new(
            "/Applications/RustCode.app/Contents/MacOS/rc"
        )));
        assert!(managed_by_app(Path::new("/usr/bin/rc")));
        assert!(managed_by_app(Path::new(
            "/tmp/.mount_RustCoXYZ/usr/bin/rc"
        )));
        assert!(!managed_by_app(Path::new("/home/me/.rustcode/bin/rc")));
        assert!(!managed_by_app(Path::new("/opt/homebrew/bin/rc")));
        assert!(!managed_by_app(Path::new("/usr/local/bin/rc")));
    }

    #[test]
    fn the_answer_names_the_phase_target_error_and_waiting_turns() {
        let restarting = answer(&Phase::Restarting((1, 47, 0)), Some(2));
        assert_eq!(restarting["phase"], "restarting");
        assert_eq!(restarting["target"], "1.47.0");
        assert_eq!(restarting["runningTurns"], 2);
        assert_eq!(restarting["current"], env!("CARGO_PKG_VERSION"));
        let failed = answer(&Phase::Failed("no network".into()), Some(3));
        assert_eq!(failed["phase"], "failed");
        assert_eq!(failed["error"], "no network");
        assert!(failed.get("runningTurns").is_none());
        assert_eq!(answer(&Phase::AskedApp, None)["phase"], "app");
        assert_eq!(
            answer(&Phase::UpToDate((1, 46, 1)), None)["target"],
            "1.46.1"
        );
    }

    #[test]
    fn a_request_older_than_a_day_or_from_the_future_does_not_ask() {
        let now = 10 * 24 * 60 * 60 * 1000;
        assert!(request_fresh(now - 1000, now));
        assert!(!request_fresh(now - 25 * 60 * 60 * 1000, now));
        assert!(!request_fresh(now + 2 * 60 * 60 * 1000, now));
    }

    #[test]
    fn the_action_defaults_to_status() {
        let request: MachineUpdateRequest = serde_json::from_value(json!({})).unwrap();
        assert_eq!(request.action, Action::Status);
        let start: MachineUpdateRequest =
            serde_json::from_value(json!({ "action": "start" })).unwrap();
        assert_eq!(start.action, Action::Start);
    }
}
