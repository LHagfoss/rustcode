//! Per-user background service, installed only after remote sharing was enabled.
//!
//! Two things can keep a gateway up with no app: the RustCode server, which
//! hosts it whenever the server is installed (`host.rs`), and — on macOS,
//! with no server installed — a login agent written here. Never both: an
//! installed server removes the agent, because two supervisors would race for
//! the same port and the same owner lock.
use super::lifecycle::GatewayOwner;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn executable_path() -> Result<PathBuf> {
    crate::service::executable_path()
}

#[cfg(target_os = "macos")]
fn label(config: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "no.rustcode.remote.{}",
        hex::encode(&Sha256::digest(config.as_os_str().as_encoded_bytes())[..8])
    )
}
#[cfg(target_os = "macos")]
use crate::service::xml_escape as xml;

/// The search path an installed agent already runs with, as written in its
/// document. Terminals differ in theirs: taking each caller's would rewrite
/// the agent, and restart the gateway under its devices, whenever a terminal
/// with another environment asked for it.
#[cfg(target_os = "macos")]
fn installed_path(document: &str) -> Option<&str> {
    let (_, rest) = document.split_once("<key>PATH</key><string>")?;
    Some(rest.split_once("</string>")?.0)
}

/// Development/test binaries never register a persistent login service.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn installed_program(program: &Path) -> bool {
    let mut directories = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        directories.extend([home.join(".local/bin"), home.join(".cargo/bin")]);
    }
    if let Some(home) = std::env::var_os("CARGO_HOME") {
        directories.push(PathBuf::from(home).join("bin"));
    }
    directories
        .iter()
        .any(|directory| program.parent() == Some(directory.as_path()))
}

pub async fn start(
    launcher: &super::owner_client::Launcher,
    lifecycle: &super::lifecycle::RemoteLifecycle,
) -> Result<super::control::GatewayStatus> {
    // Serialize adoption and upgrades across terminals and detached owners.
    let _launch_lock = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            match crate::daemon::private_files::lock_private_file(
                launcher.config_directory.join("remote").as_path(),
                "launch.lock",
                "remote launcher",
            ) {
                Ok(lock) => return Ok::<_, anyhow::Error>(lock),
                Err(error) if crate::daemon::private_files::is_lock_busy(&error) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .context("another gateway launcher is still starting")??;
    let effective = effective_launcher(
        launcher,
        lifecycle
            .status()
            .await?
            .as_ref()
            .map(|status| status.listen_address.as_str()),
        server_installed(),
    );
    let launcher = &effective;
    #[cfg(target_os = "macos")]
    if launcher.owner == GatewayOwner::Server || server_installed() {
        // The server supervises the gateway now; which process the agent ran
        // is only interesting to the server's own reconcile step.
        let _agent_pid = remove_login_agent(&launcher.config_directory).await;
    }
    #[cfg(target_os = "macos")]
    if launcher.owner != GatewayOwner::Server
        && !server_installed()
        && installed_program(&launcher.program)
    {
        let home = PathBuf::from(std::env::var_os("HOME").context("home directory unavailable")?);
        let label = label(&launcher.config_directory);
        let agent = home
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"));
        std::fs::create_dir_all(agent.parent().unwrap())?;
        let mut arguments = vec![
            launcher.program.to_string_lossy().into_owned(),
            "remote".into(),
            "serve".into(),
        ];
        if let Some(bind) = &launcher.bind {
            arguments.extend(["--bind".into(), bind.clone()]);
        }
        if let Some(port) = launcher.port {
            arguments.extend(["--port".into(), port.to_string()]);
        }
        if let Some(address) = &launcher.advertise {
            arguments.extend(["--advertise".into(), address.clone()]);
        }
        let arguments = arguments
            .iter()
            .map(|v| format!("<string>{}</string>", xml(v)))
            .collect::<String>();
        let installed = std::fs::read_to_string(&agent).ok();
        let path = match installed.as_deref().and_then(installed_path) {
            Some(path) => path.to_owned(),
            None => xml(&std::env::var("PATH")
                .unwrap_or_else(|_| "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin".into())),
        };
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{arguments}</array><key>EnvironmentVariables</key><dict><key>RUSTCODE_REMOTE_MANAGED</key><string>1</string><key>RUSTCODE_REMOTE_OWNER</key><string>app</string><key>RUSTCODE_CONFIG_DIR</key><string>{config}</string><key>PATH</key><string>{path}</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>5</integer><key>StandardOutPath</key><string>{log}</string><key>StandardErrorPath</key><string>{log}</string></dict></plist>"#,
            config = xml(&launcher.config_directory.to_string_lossy()),
            log = xml(&lifecycle.log_path().to_string_lossy())
        );
        // SAFETY: geteuid has no preconditions.
        let domain = format!("gui/{}", unsafe { libc::geteuid() });
        let service = format!("{domain}/{label}");
        let loaded = crate::platform::tokio_command("/bin/launchctl")
            .args(["print", &service])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await?
            .success();
        let unchanged = installed.as_deref() == Some(document.as_str());
        if !loaded || !unchanged {
            let loaded_now = || async {
                crate::platform::tokio_command("/bin/launchctl")
                    .args(["print", &service])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .is_ok_and(|status| status.success())
            };
            if loaded {
                let _ = crate::platform::tokio_command("/bin/launchctl")
                    .args(["bootout", &service])
                    .status()
                    .await;
                // Removal finishes after the command returns; registering
                // again before it has is refused.
                for _ in 0..50 {
                    if !loaded_now().await {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
            // Adoption closes only the gateway, never its independent session owners.
            lifecycle.stop().await?;
            crate::daemon::private_files::ensure_private_directory(
                lifecycle.log_path().parent().unwrap(),
                "remote gateway",
            )?;
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&agent)?;
            file.write_all(document.as_bytes())?;
            let mut attempts = 0;
            let result = loop {
                let result = crate::platform::tokio_command("/bin/launchctl")
                    .args(["bootstrap", &domain])
                    .arg(&agent)
                    .output()
                    .await?;
                attempts += 1;
                if result.status.success() || attempts == 5 || loaded_now().await {
                    break result;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            };
            anyhow::ensure!(
                result.status.success() || loaded_now().await,
                "could not register background gateway: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        } else if let Some(status) = lifecycle.status().await? {
            if super::lifecycle::gateway_needs_upgrade(&status.version, env!("CARGO_PKG_VERSION")) {
                lifecycle.stop().await?;
            } else {
                return Ok(status);
            }
        }
        return tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Ok(Some(status)) = lifecycle.status().await {
                    return status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .context("background gateway did not start");
    }
    lifecycle.start(launcher.command()).await
}

/// An explicit `remote stop` also disables restart at login.
pub async fn uninstall(config: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let _agent_pid = remove_login_agent(config).await;
    let _ = config;
    Ok(())
}

/// How long a removed agent gets to release the gateway it ran.
#[cfg(target_os = "macos")]
const AGENT_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The pid launchd runs for a job, read from `launchctl print` (`pid = 4242`).
/// `None` when the job is not loaded or runs nothing: a plist on disk alone
/// does not mean launchd has a gateway.
#[cfg(target_os = "macos")]
fn pid_of_print(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("pid = "))
        .and_then(|pid| pid.trim().parse().ok())
}

/// The pid launchd runs for `service`, when it is loaded and running.
#[cfg(target_os = "macos")]
async fn launchd_pid(service: &str) -> Option<u32> {
    let printed = tokio::time::timeout(
        AGENT_STOP_TIMEOUT,
        crate::platform::tokio_command("/bin/launchctl")
            .args(["print", service])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    pid_of_print(&String::from_utf8_lossy(&printed.stdout))
}

/// Remove the gateway's LaunchAgent, if one was installed, and wait for
/// launchd to let go of the gateway it ran. A machine that installs the
/// RustCode server stops using it: the server hosts the gateway instead.
///
/// Answers the pid of the gateway launchd was running, read *before* the job
/// is booted out: only that process is the server's to replace. A plist left
/// behind by an agent whose job is not loaded proves nothing about a gateway
/// that happens to run — the app's or a terminal's, which the server adopts
/// instead of stopping.
#[cfg(target_os = "macos")]
pub(crate) async fn remove_login_agent(config: &Path) -> Option<u32> {
    let label = label(config);
    let document = std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"))
    });
    let installed = document.as_ref().is_some_and(|path| path.exists());
    if !installed {
        return None;
    }
    // SAFETY: geteuid has no preconditions.
    let service = format!("gui/{}/{label}", unsafe { libc::geteuid() });
    let agent_pid = launchd_pid(&service).await;
    let _ = crate::platform::tokio_command("/bin/launchctl")
        .args(["bootout", &service])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    // Removal finishes after the command returns; wait for launchd to let go
    // of the job so a successor can bind without racing it.
    let deadline = tokio::time::Instant::now() + AGENT_STOP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if launchd_pid(&service).await.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if let Some(path) = document.as_ref()
        && path.exists()
    {
        let _ = std::fs::remove_file(path);
    }
    agent_pid
}

/// Whether remote access is switched on for this machine: a gateway has been
/// set up and was not stopped. While it is on, every interactive session is
/// shared with paired devices.
pub fn enabled(config_directory: &Path) -> bool {
    super::lifecycle::RemoteLifecycle::new(config_directory)
        .identity_path()
        .exists()
        && !config_directory.join("remote/disabled").exists()
}

/// How many paired devices could still connect.
pub fn paired_devices(config_directory: &Path) -> usize {
    let now = chrono::Utc::now();
    super::lifecycle::RemoteLifecycle::new(config_directory)
        .devices()
        .map(|devices| {
            devices
                .iter()
                .filter(|device| !device.is_expired(now))
                .count()
        })
        .unwrap_or(0)
}

/// Switch remote access on for this machine with nothing to share yet, as
/// Settings › Remote access does: clear the explicit-off marker, then start
/// the gateway the way `/remote` would. Every open thread shares itself from
/// the next pass of its frontend's auto-share loop.
pub async fn enable_for_host() -> Result<()> {
    let config_directory = crate::config::get_config_dir().context("no configuration directory")?;
    let lifecycle = super::lifecycle::RemoteLifecycle::new(&config_directory);
    let _ = std::fs::remove_file(config_directory.join("remote/disabled"));
    let mut launcher = super::owner_client::Launcher {
        program: executable_path()?,
        config_directory: config_directory.clone(),
        bind: None,
        port: None,
        advertise: None,
        prefer_interface: None,
        // The app's Settings switch, or a terminal: `effective_launcher`
        // makes it the server's when the server service is installed.
        owner: GatewayOwner::App,
    };
    let (_, _, config) = crate::config::load_config();
    launcher.bind = config.remote.bind;
    launcher.port = config.remote.port;
    launcher.advertise = config.remote.advertise;
    launcher.prefer_interface = config.remote.prefer_interface;
    let status = start(&launcher, &lifecycle).await?;
    crate::logger::operational_event(
        "remote.enabled",
        serde_json::json!({
            "advertised_address": status.advertised_address,
            "gateway_id": status.gateway_id,
        }),
    );
    Ok(())
}

/// Switch remote access off for this machine: remember the choice, remove the
/// login service and stop the gateway. Returns whether a gateway was running.
pub async fn disable(config_directory: &Path) -> Result<bool> {
    std::fs::create_dir_all(config_directory.join("remote"))?;
    std::fs::write(
        config_directory.join("remote/disabled"),
        "explicitly stopped",
    )?;
    uninstall(config_directory).await?;
    let stopped = super::lifecycle::RemoteLifecycle::new(config_directory)
        .stop()
        .await?;
    Ok(stopped.is_some())
}

/// Restore a previously enabled gateway on normal CLI startup. Sessions share
/// themselves once it runs.
pub async fn resume_enabled() -> Result<()> {
    let Some(config_directory) = crate::config::get_config_dir() else {
        return Ok(());
    };
    let lifecycle = super::lifecycle::RemoteLifecycle::new(&config_directory);
    if !lifecycle.identity_path().exists() || config_directory.join("remote/disabled").exists() {
        return Ok(());
    }
    let (_, _, config) = crate::config::load_config();
    let launcher = super::owner_client::Launcher {
        program: executable_path()?,
        config_directory,
        bind: config.remote.bind,
        port: config.remote.port,
        advertise: config.remote.advertise,
        prefer_interface: config.remote.prefer_interface,
        owner: GatewayOwner::App,
    };
    start(&launcher, &lifecycle).await?;
    Ok(())
}

/// When the installed executable changes, launchd restarts it. Detached gateways
/// hand off after their listeners and registration have been fully removed.
pub async fn wait_for_upgrade(program: PathBuf) {
    #[cfg(unix)]
    fn identity(path: &Path) -> Option<(u64, u64, u64, i64, i64)> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some((m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
    }
    // An installer replaces the file: its size or write time changes.
    #[cfg(not(unix))]
    fn identity(path: &Path) -> Option<(u64, std::time::SystemTime)> {
        let m = std::fs::metadata(path).ok()?;
        Some((m.len(), m.modified().ok()?))
    }
    let original = identity(&program);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let next = identity(&program);
        if next.is_some() && next != original {
            return;
        }
    }
}

/// The current IP is auto-selection's result, never its durable policy.
fn retain_port(
    launcher: &super::owner_client::Launcher,
    listen: Option<&str>,
) -> super::owner_client::Launcher {
    let mut effective = launcher.clone();
    if let Some(address) = listen.and_then(|value| value.parse::<std::net::SocketAddr>().ok()) {
        effective.port.get_or_insert(address.port());
    }
    effective
}

/// The launcher a start actually uses: the port of a gateway already running,
/// and — when the RustCode server is installed — the server as the gateway's
/// owner, because the server is what starts it again after the app closes,
/// whichever process happened to start it first.
fn effective_launcher(
    launcher: &super::owner_client::Launcher,
    listen: Option<&str>,
    server_installed: bool,
) -> super::owner_client::Launcher {
    let mut effective = retain_port(launcher, listen);
    if server_installed {
        effective.owner = GatewayOwner::Server;
    }
    effective
}

/// Whether the RustCode server is installed as a service for this
/// configuration directory, so it hosts the gateway.
pub fn server_installed() -> bool {
    crate::service::server_service::installed()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adopting_auto_gateway_does_not_pin_its_dhcp_address() {
        let original = super::super::owner_client::Launcher {
            program: "/usr/local/bin/rustcode".into(),
            config_directory: "/tmp/config".into(),
            bind: None,
            advertise: None,
            port: None,
            prefer_interface: None,
            owner: GatewayOwner::App,
        };
        let adopted = retain_port(&original, Some("192.168.1.15:17879"));
        assert_eq!(adopted.bind, None);
        assert_eq!(adopted.advertise, None);
        assert_eq!(adopted.port, Some(17879));
        let explicit = super::super::owner_client::Launcher {
            bind: Some("127.0.0.1".into()),
            advertise: Some("host.local".into()),
            port: Some(23456),
            ..original
        };
        let adopted = retain_port(&explicit, Some("192.168.1.15:17879"));
        assert_eq!(adopted.bind, explicit.bind);
        assert_eq!(adopted.advertise, explicit.advertise);
        assert_eq!(adopted.port, explicit.port);
    }

    #[test]
    fn an_installed_server_owns_the_gateway_and_a_bare_machine_does_not() {
        let launcher = super::super::owner_client::Launcher {
            program: "/usr/local/bin/rustcode".into(),
            config_directory: "/tmp/config".into(),
            bind: None,
            port: None,
            advertise: None,
            prefer_interface: None,
            owner: GatewayOwner::App,
        };
        // With no server installed the app is the owner: the gateway lives
        // as long as what started it.
        let alone = effective_launcher(&launcher, None, false);
        assert_eq!(alone.owner, GatewayOwner::App);
        // With the server installed the server is: it starts the gateway
        // again after the app closes.
        let hosted = effective_launcher(&launcher, None, true);
        assert_eq!(hosted.owner, GatewayOwner::Server);
        // Adopting a running gateway keeps its port either way.
        let hosted = effective_launcher(&launcher, Some("192.168.1.15:17879"), true);
        assert_eq!(hosted.port, Some(17879));
        assert_eq!(hosted.bind, None);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn the_agents_pid_comes_from_launchctl_print_and_nowhere_else() {
        // `launchctl print gui/<uid>/<label>` while the job runs.
        let printed = "\tstate = running\n\tpid = 97702\n\tprogram = /usr/local/bin/rustcode\n";
        assert_eq!(pid_of_print(printed), Some(97702));
        // A loaded job that runs nothing (`launchctl list` shows `-`): no pid,
        // so its plist alone never marks a running gateway as the agent's.
        assert_eq!(pid_of_print("\tstate = spawn scheduled\n"), None);
        assert_eq!(pid_of_print("\tpid = -\n"), None);
        assert_eq!(pid_of_print(""), None);
        // A pid in another key is not the job's.
        assert_eq!(pid_of_print("\tparent_pid = 1\n"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_installed_agent_keeps_its_search_path() {
        let document = "<dict><key>PATH</key><string>/opt/bin:/usr/bin&amp;x</string></dict>";
        assert_eq!(installed_path(document), Some("/opt/bin:/usr/bin&amp;x"));
        assert_eq!(installed_path("<dict></dict>"), None);
    }
    #[test]
    fn cargo_installs_are_managed_but_development_builds_are_not() {
        assert!(!installed_program(Path::new(
            "/tmp/rustcode/target/debug/rustcode"
        )));
        if let Some(home) = std::env::var_os("HOME") {
            assert!(installed_program(
                &PathBuf::from(home).join(".cargo/bin/rustcode")
            ));
        }
    }
}
