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
        loop {
            let wait = match crate::config::get_config_dir() {
                Some(config_directory) => poll(&config_directory).await,
                None => NO_CONFIG,
            };
            tokio::time::sleep(wait).await;
        }
    });
}

/// What to do on this look at the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Remote access is off, or a gateway of this version or a newer one is
    /// already running (the app's, a terminal's, or ours): ask nothing, bind
    /// nothing, and let one owner keep the port.
    Wait,
    Start,
}

fn step(remote_enabled: bool, running_version: Option<&str>) -> Step {
    if !remote_enabled {
        return Step::Wait;
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
async fn poll(config_directory: &Path) -> Duration {
    let enabled = super::service::enabled(config_directory);
    // `running_version` reads the registration and checks the process, without
    // the control-socket round trip and address enumeration `status` does:
    // this runs every few seconds for the life of the server.
    let running = RemoteLifecycle::new(config_directory).running_version();
    match step(enabled, running.as_deref()) {
        Step::Wait => POLL,
        Step::Start => match start(config_directory).await {
            Ok(status) => {
                crate::logger::operational_event(
                    "remote.hosted",
                    serde_json::json!({
                        "listen_address": status.listen_address,
                        "advertised_address": status.advertised_address,
                        "pid": status.pid,
                    }),
                );
                crate::dbg_log!(
                    "RustCode server started the remote gateway on {}",
                    status.listen_address
                );
                POLL
            }
            Err(error) => {
                crate::dbg_log!("RustCode server could not start the remote gateway: {error:#}");
                RETRY
            }
        },
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
        assert_eq!(step(true, None), Step::Start);
        // Off: nothing to host.
        assert_eq!(step(false, None), Step::Wait);
        // A gateway of this version already runs (the app's or a terminal's):
        // adopting it is what keeps one owner of the port.
        assert_eq!(step(true, Some(env!("CARGO_PKG_VERSION"))), Step::Wait);
        assert_eq!(step(false, Some(env!("CARGO_PKG_VERSION"))), Step::Wait);
        // One from before an update is replaced, as `remote start` does.
        assert_eq!(step(true, Some("0.0.1")), Step::Start);
        // A newer one, from an updated app, is left alone.
        assert_eq!(step(true, Some("999.0.0")), Step::Wait);
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
