//! Per-user background service, installed only after remote sharing was enabled.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn executable_path() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    // Preserve a Homebrew symlink so the service follows future upgrades.
    for candidate in [
        PathBuf::from("/opt/homebrew/bin/rustcode"),
        PathBuf::from("/usr/local/bin/rustcode"),
    ] {
        if candidate.canonicalize().ok().as_ref() == Some(&current) {
            return Ok(candidate);
        }
    }
    Ok(current)
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
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Development/test binaries never register a persistent login service.
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
            match crate::daemon::lifecycle::lock_private_file(
                launcher.config_directory.join("remote").as_path(),
                "launch.lock",
                "remote launcher",
            ) {
                Ok(lock) => return Ok::<_, anyhow::Error>(lock),
                Err(error) if crate::daemon::lifecycle::is_lock_busy(&error) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .context("another gateway launcher is still starting")??;
    let effective = retain_port(
        launcher,
        lifecycle
            .status()
            .await?
            .as_ref()
            .map(|status| status.listen_address.as_str()),
    );
    let launcher = &effective;
    #[cfg(target_os = "macos")]
    if installed_program(&launcher.program) {
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
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{arguments}</array><key>EnvironmentVariables</key><dict><key>RUSTCODE_REMOTE_MANAGED</key><string>1</string><key>RUSTCODE_CONFIG_DIR</key><string>{config}</string><key>PATH</key><string>{path}</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>5</integer><key>StandardOutPath</key><string>{log}</string><key>StandardErrorPath</key><string>{log}</string></dict></plist>"#,
            config = xml(&launcher.config_directory.to_string_lossy()),
            path = xml(&std::env::var("PATH")
                .unwrap_or_else(|_| "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin".into())),
            log = xml(&lifecycle.log_path().to_string_lossy())
        );
        // SAFETY: geteuid has no preconditions.
        let domain = format!("gui/{}", unsafe { libc::geteuid() });
        let service = format!("{domain}/{label}");
        let loaded = tokio::process::Command::new("/bin/launchctl")
            .args(["print", &service])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await?
            .success();
        let unchanged = std::fs::read_to_string(&agent).ok().as_deref() == Some(document.as_str());
        if !loaded || !unchanged {
            if loaded {
                let _ = tokio::process::Command::new("/bin/launchctl")
                    .args(["bootout", &service])
                    .status()
                    .await;
            }
            // Adoption closes only the gateway, never its independent session owners.
            lifecycle.stop().await?;
            crate::daemon::lifecycle::ensure_private_directory(
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
            let result = tokio::process::Command::new("/bin/launchctl")
                .args(["bootstrap", &domain])
                .arg(&agent)
                .output()
                .await?;
            anyhow::ensure!(
                result.status.success(),
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
    {
        let label = label(config);
        // SAFETY: geteuid has no preconditions.
        let service = format!("gui/{}/{label}", unsafe { libc::geteuid() });
        let _ = tokio::process::Command::new("/bin/launchctl")
            .args(["bootout", &service])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
        if let Some(home) = std::env::var_os("HOME") {
            let path = PathBuf::from(home)
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist"));
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
    }
    let _ = config;
    Ok(())
}

/// Restore a previously enabled gateway on normal CLI startup. Never enables sharing of a session.
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
    };
    start(&launcher, &lifecycle).await?;
    Ok(())
}

/// When the installed executable changes, launchd restarts it. Detached gateways
/// hand off after their listeners and registration have been fully removed.
pub async fn wait_for_upgrade(program: PathBuf) {
    use std::os::unix::fs::MetadataExt;
    fn identity(path: &Path) -> Option<(u64, u64, u64, i64, i64)> {
        let m = std::fs::metadata(path).ok()?;
        Some((m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
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
