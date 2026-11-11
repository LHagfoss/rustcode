//! The RustCode server hosts the remote gateway (issue #3412).
//!
//! The gateway is its own process: it pairs devices, keeps the shared-session
//! registry and relays device commands to the engine that owns each thread.
//! Whoever starts it is only its launcher, so the machine used to become
//! unreachable as soon as the app that started it quit, even with the server
//! installed and answering.
//!
//! So the server starts the gateway too, and starts it again when it stops:
//! with the server installed, remote access on and the app closed, port 17879
//! keeps answering and paired machines keep reaching the threads that live in
//! the server. The gateway's owner lock keeps one gateway at a time, so the
//! app adopts a gateway the server started instead of binding a second one
//! (`super::lifecycle::RemoteLifecycle::start` already does that), and the
//! server adopts the app's.
//!
//! What the server starts it with comes from `remote/settings.json`, which
//! every gateway writes as it starts (`super::lifecycle::GatewaySettings`), so
//! the same address, port, advertised name and interface preference are used
//! at login with no app around. A machine that enabled remote access before
//! that file existed falls back to `[remote]` in its `config.toml`.

use super::lifecycle::{GatewayOwner, GatewaySettings, RemoteLifecycle, gateway_needs_upgrade};
use super::owner_client::Launcher;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the server looks at a machine with remote access on.
pub const POLL: Duration = Duration::from_secs(5);
/// How long the server waits after a gateway that would not start, so a
/// broken address or a port already taken is retried instead of hammered.
pub const RETRY: Duration = Duration::from_secs(15);
/// With no configuration directory there is nothing to host.
const NO_CONFIG: Duration = Duration::from_secs(60);

/// Watch remote access and keep this machine's gateway running for as long as
/// the server lives. Runs as a task of `rustcode server run`: a server started
/// by the service is exactly the process that is still there when the app
/// closes.
pub fn spawn() {
    // A server with no runtime (a test, a CLI path that never serves) must
    // not panic here.
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async {
        let mut watch = Watch::default();
        loop {
            let wait = match crate::config::get_config_dir() {
                Some(config_directory) => poll(&mut watch, &config_directory).await,
                None => NO_CONFIG,
            };
            tokio::time::sleep(wait).await;
        }
    });
}

/// What this server has already done about the gateway.
#[derive(Debug, Default)]
struct Watch {
    /// The macOS gateway login agent was looked at: at most once per server,
    /// and only while remote access is on.
    #[cfg(target_os = "macos")]
    agent_reconciled: bool,
    /// An agent was removed and any gateway it ran has to be replaced by the
    /// server's own. Always false where there is no agent to remove.
    handover: bool,
}

/// What to do on this look at the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Remote access is off, or a gateway of this version or a newer one is
    /// already running (the app's, a terminal's, or ours): ask nothing, bind
    /// nothing, and let one owner keep the port.
    Wait,
    /// A gateway the removed login agent ran is still there: stop it and
    /// start the server's own, so launchd and the server do not both
    /// supervise the port.
    HandOver,
    /// No gateway runs, or one older than this server does.
    Start,
}

/// Whether the gateway that runs is the one the login agent ran, and so is
/// the one to replace. A plist left behind while its job is not loaded proves
/// nothing: the gateway that happens to be running is the app's or a
/// terminal's, and the server adopts it (`docs/remote.md`).
fn handover_required(agent_pid: Option<u32>, running_pid: Option<u32>) -> bool {
    matches!((agent_pid, running_pid), (Some(agent), Some(running)) if agent == running)
}

fn step(remote_enabled: bool, running_version: Option<&str>, handover: bool) -> Step {
    if !remote_enabled {
        return Step::Wait;
    }
    if handover {
        // Nothing left to replace: starting is the handover.
        return match running_version {
            Some(_) => Step::HandOver,
            None => Step::Start,
        };
    }
    match running_version {
        // A gateway older than this server is replaced after an update,
        // exactly as `remote start` replaces one: `start` stops it through its
        // control socket and starts its successor on the same port.
        Some(version) if gateway_needs_upgrade(version, env!("CARGO_PKG_VERSION")) => Step::Start,
        Some(_) => Step::Wait,
        None => Step::Start,
    }
}

/// One look: start the gateway when it should be running and is not. Answers
/// how long to wait before looking again.
async fn poll(watch: &mut Watch, config_directory: &Path) -> Duration {
    if !super::service::enabled(config_directory) {
        return POLL;
    }
    #[cfg(target_os = "macos")]
    if !watch.agent_reconciled {
        // The first look while remote access is on. An installed server hosts
        // the gateway, so the login agent that used to is removed now — even
        // when its gateway is still running, which is what left launchd and
        // the server supervising the same port.
        watch.agent_reconciled = true;
        if super::service::server_installed() {
            // The pid launchd was running, read before the job is booted out.
            // Only that process is displaced; a gateway from the app or a
            // terminal is adopted, which is what keeps one owner of the port
            // without bouncing working devices.
            let agent_pid = super::service::remove_login_agent(config_directory).await;
            let running_pid = RemoteLifecycle::new(config_directory).running_pid();
            watch.handover = handover_required(agent_pid, running_pid);
        }
    }
    let lifecycle = RemoteLifecycle::new(config_directory);
    // `running_version` reads the registration and checks the process, without
    // the control-socket round trip and address enumeration `status` does:
    // this runs every few seconds for the life of the server.
    let running = lifecycle.running_version();
    let step = step(true, running.as_deref(), watch.handover);
    match step {
        Step::Wait => POLL,
        Step::HandOver | Step::Start => {
            if step == Step::HandOver {
                // A handover happens once, at the server's first look. The
                // agent's gateway is asked to stop through its control socket;
                // its successor is the server's.
                watch.handover = false;
                if let Err(error) = lifecycle.stop().await {
                    crate::dbg_log!(
                        "RustCode server could not stop the login agent's gateway: {error:#}"
                    );
                    return RETRY;
                }
            }
            match start(config_directory).await {
                Ok(status) => {
                    crate::logger::operational_event(
                        "remote.hosted",
                        serde_json::json!({
                            "listen_address": status.listen_address,
                            "advertised_address": status.advertised_address,
                            "pid": status.pid,
                            "started_by": status.started_by.as_env(),
                        }),
                    );
                    crate::dbg_log!(
                        "RustCode server started the remote gateway on {}",
                        status.listen_address
                    );
                    POLL
                }
                Err(error) => {
                    crate::dbg_log!(
                        "RustCode server could not start the remote gateway: {error:#}"
                    );
                    RETRY
                }
            }
        }
    }
}

/// Start this machine's gateway as the server's, and answer its status.
pub async fn start(config_directory: &Path) -> Result<super::control::GatewayStatus> {
    let lifecycle = RemoteLifecycle::new(config_directory);
    let launcher = launcher(config_directory)?;
    super::service::start(&launcher, &lifecycle).await
}

/// The gateway the server would start: the settings the last gateway
/// recorded, or `[remote]` in `config.toml` when there are none.
pub fn launcher(config_directory: &Path) -> Result<Launcher> {
    let recorded = RemoteLifecycle::new(config_directory)
        .settings()
        .ok()
        .flatten();
    let (_, _, config) = crate::config::load_config();
    Ok(launcher_from(
        super::service::executable_path()?,
        config_directory,
        recorded.as_ref(),
        &config.remote,
    ))
}

/// [`launcher`] without touching the disk, so the precedence is testable.
fn launcher_from(
    program: PathBuf,
    config_directory: &Path,
    recorded: Option<&GatewaySettings>,
    remote: &crate::config::RemoteConfig,
) -> Launcher {
    let recorded = recorded.cloned().unwrap_or_default();
    Launcher {
        program,
        config_directory: config_directory.to_path_buf(),
        // What the gateway ran with last, then the user's config: the
        // recorded value is the policy in force, and `auto` is never
        // replaced by the address it happened to resolve to.
        bind: recorded.bind.or_else(|| remote.bind.clone()),
        port: recorded.port.or(remote.port),
        advertise: recorded.advertise.or_else(|| remote.advertise.clone()),
        prefer_interface: recorded
            .prefer_interface
            .or_else(|| remote.prefer_interface.clone()),
        owner: GatewayOwner::Server,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(bind: Option<&str>, port: Option<u16>) -> crate::config::RemoteConfig {
        crate::config::RemoteConfig {
            bind: bind.map(str::to_owned),
            port,
            advertise: Some("studio.netbird.cloud".into()),
            prefer_interface: Some("netbird".into()),
            ..crate::config::RemoteConfig::default()
        }
    }

    #[test]
    fn the_server_starts_the_gateway_only_when_remote_access_is_on_and_it_is_stopped() {
        assert_eq!(step(true, None, false), Step::Start);
        // Off: nothing to host.
        assert_eq!(step(false, None, false), Step::Wait);
        // A gateway of this version already runs (the app's or a terminal's):
        // adopting it is what keeps one owner of the port.
        assert_eq!(
            step(true, Some(env!("CARGO_PKG_VERSION")), false),
            Step::Wait
        );
        assert_eq!(
            step(false, Some(env!("CARGO_PKG_VERSION")), false),
            Step::Wait
        );
        // One from before an update is replaced, as `remote start` does.
        assert_eq!(step(true, Some("0.0.1"), false), Step::Start);
        // A newer one, from an updated app, is left alone.
        assert_eq!(step(true, Some("999.0.0"), false), Step::Wait);
    }

    /// With the login agent's gateway still running (same version), waiting
    /// for it to go left launchd and the server both supervising the port, so
    /// installing the server never handed the gateway over.
    #[test]
    fn a_removed_login_agents_gateway_is_replaced_once_even_while_it_still_runs() {
        let version = env!("CARGO_PKG_VERSION");
        // The gateway the agent ran is the same version: adopting it is what
        // the old code did, and it never handed over.
        assert_eq!(step(true, Some(version), false), Step::Wait);
        assert_eq!(step(true, Some(version), true), Step::HandOver);
        // Nothing left by the time the agent was gone: just start.
        assert_eq!(step(true, None, true), Step::Start);
        // The handover is one look, not every five seconds: `poll` clears the
        // flag, so the next look adopts the server's own gateway.
        assert_eq!(step(true, Some(version), false), Step::Wait);
        // Remote access off is still nothing to host, handover or not.
        assert_eq!(step(false, Some(version), true), Step::Wait);
        assert_eq!(step(false, None, true), Step::Wait);
    }

    /// A plist left behind by an agent whose job is not loaded says nothing
    /// about a gateway that happens to run: the app's or a terminal's is
    /// adopted, never stopped and replaced under its paired devices.
    #[test]
    fn a_stale_plist_never_displaces_a_gateway_the_agent_did_not_run() {
        // launchd had no job (no pid to read), yet a gateway runs: adopt it.
        let version = env!("CARGO_PKG_VERSION");
        assert!(!handover_required(None, Some(4242)));
        assert_eq!(step(true, Some(version), false), Step::Wait);
        // The agent's job ran another process than the one listening: that is
        // not the agent's gateway either (a restart it did not survive).
        assert!(!handover_required(Some(100), Some(4242)));
        // Only the process launchd was running is replaced.
        assert!(handover_required(Some(4242), Some(4242)));
        assert_eq!(step(true, Some(version), true), Step::HandOver);
        // No gateway at all: starting is the handover, whatever the agent did.
        assert!(!handover_required(Some(4242), None));
        assert!(!handover_required(None, None));
        assert_eq!(step(true, None, false), Step::Start);
    }

    #[test]
    fn recorded_settings_win_over_config_and_auto_is_never_pinned() {
        let recorded = GatewaySettings::starting(
            Some("auto".into()),
            Some(17879),
            None,
            Some("netbird".into()),
            GatewayOwner::App,
        );
        let launcher = launcher_from(
            PathBuf::from("/opt/homebrew/bin/rustcode"),
            Path::new("/tmp/config"),
            Some(&recorded),
            &remote(Some("192.168.1.20"), Some(23456)),
        );
        assert_eq!(launcher.bind.as_deref(), Some("auto"));
        assert_eq!(launcher.port, Some(17879));
        // A setting the last gateway did not record falls back to config.
        assert_eq!(launcher.advertise.as_deref(), Some("studio.netbird.cloud"));
        assert_eq!(launcher.prefer_interface.as_deref(), Some("netbird"));
        assert_eq!(launcher.owner, GatewayOwner::Server);
    }

    #[test]
    fn without_recorded_settings_the_server_uses_config_toml() {
        let launcher = launcher_from(
            PathBuf::from("/usr/local/bin/rustcode"),
            Path::new("/tmp/config"),
            None,
            &remote(Some("100.90.28.23"), Some(17879)),
        );
        assert_eq!(launcher.bind.as_deref(), Some("100.90.28.23"));
        assert_eq!(launcher.port, Some(17879));
        assert_eq!(launcher.owner, GatewayOwner::Server);
        // Nothing configured: the gateway's own defaults (auto, 17879).
        let bare = launcher_from(
            PathBuf::from("/usr/local/bin/rustcode"),
            Path::new("/tmp/config"),
            None,
            &crate::config::RemoteConfig::default(),
        );
        assert_eq!(bare.bind, None);
        assert_eq!(bare.port, None);
    }
}
