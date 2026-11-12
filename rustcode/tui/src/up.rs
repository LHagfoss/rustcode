//! `rustcode up`: make this computer one another computer can pair with, in
//! one command. It installs (or repairs) the server as a background service,
//! switches remote access on, waits until the gateway answers and creates a
//! connect code. `rustcode machines add-ssh` runs it over SSH as `rustcode up
//! --yes --json` and reads the JSON; a person runs it on a headless machine
//! and types the code it prints on the other one (`docs/remote.md` › One
//! command for a headless machine).

use anyhow::{Context, Result, bail};
use rustcode::remote_gateway::lifecycle::RemoteLifecycle;
use rustcode::service::server_service as service;
use serde::Serialize;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// How long the gateway gets to answer once remote access is on: the
/// server looks every five seconds (`remote_gateway::host::POLL`) and a
/// gateway takes a moment to bind.
const GATEWAY_TIMEOUT: Duration = Duration::from_secs(30);

/// What `rustcode up --json` prints. `rustcode_client::ssh::UpReport` reads
/// it on the other computer; the test below keeps the two in step.
#[derive(Debug, Serialize)]
pub struct UpReport {
    pub version: String,
    pub host_name: Option<String>,
    pub os: &'static str,
    pub server: ServerPart,
    pub gateway: GatewayPart,
    pub routes: Vec<rustcode::remote_gateway::address::HostRoute>,
    pub connect_code: CodePart,
    /// What still needs a person, in order: lingering on Linux, a server
    /// that runs only until the next restart.
    pub problems: Vec<ProblemPart>,
}

#[derive(Debug, Serialize)]
pub struct ServerPart {
    /// `service` (starts at login) or `background` (until the computer
    /// restarts).
    pub mode: &'static str,
    /// `launchd`, `systemd`, `scheduled-task` or `run-key`, for a service.
    pub kind: Option<String>,
    pub running: bool,
    pub version: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct GatewayPart {
    pub listen_address: String,
    pub gateway_id: String,
}

#[derive(Debug, Serialize)]
pub struct CodePart {
    pub code: String,
    pub link: String,
    pub expires_in_secs: u64,
    /// No other computer can reach the gateway (`remote.bind` is loopback).
    pub loopback_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProblemPart {
    pub code: String,
    pub message: String,
}

impl ProblemPart {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

pub async fn run(yes: bool, json: bool, no_service: bool) -> ! {
    let code = match up(yes, no_service).await {
        Ok(report) => {
            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(text) => println!("{text}"),
                    Err(error) => eprintln!("rustcode up: {error}"),
                }
            } else {
                print!("{}", human(&report));
            }
            0
        }
        Err(error) => {
            eprintln!("rustcode up: {error:#}");
            1
        }
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}

/// Say what is happening on standard error, so `--json` keeps standard
/// output to the report alone.
fn step(message: &str) {
    eprintln!("==> {message}");
}

async fn up(yes: bool, no_service: bool) -> Result<UpReport> {
    let config_dir =
        rustcode::config::get_config_dir().context("no RustCode configuration directory")?;
    let mut problems = Vec::new();
    step("Starting the RustCode server");
    let server = ensure_server(yes, no_service, &mut problems).await?;
    step("Switching remote access on");
    let gateway = ensure_gateway(&config_dir, server.running).await?;
    step("Creating a connect code");
    let lifecycle = RemoteLifecycle::new(&config_dir);
    let offer = lifecycle.machine_offer().await?;
    if offer.loopback_only {
        problems.push(ProblemPart::new(
            "loopback-only",
            "Remote access listens on this computer only (`remote.bind`), so no other computer can use the code. Set `remote.bind` to `auto` and run `rustcode up` again.",
        ));
    }
    Ok(UpReport {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        host_name: offer.host_name.clone(),
        os: std::env::consts::OS,
        server,
        gateway: GatewayPart {
            listen_address: gateway.listen_address,
            gateway_id: gateway.gateway_id,
        },
        routes: offer.routes.clone(),
        connect_code: CodePart {
            code: offer.code.expose().to_owned(),
            link: offer.link.expose().to_owned(),
            expires_in_secs: offer.expires_in_secs,
            loopback_only: offer.loopback_only,
        },
        problems,
    })
}

/// Install or repair the service and make sure a server of this version
/// answers. With `no_service`, or where no service can be installed (a
/// container with no systemd), the server runs in the background instead and
/// stops when the computer does.
async fn ensure_server(
    yes: bool,
    no_service: bool,
    problems: &mut Vec<ProblemPart>,
) -> Result<ServerPart> {
    if !no_service {
        match service::install(false, yes).await {
            Ok(report) => {
                let mut live = report.live.clone();
                for problem in &report.problems {
                    if problem.code == "linger-disabled" && enable_linger() {
                        continue;
                    }
                    // `up` handles a server of another version itself, below.
                    if problem.code == "restart-pending" {
                        continue;
                    }
                    problems.push(ProblemPart::new(problem.code, problem.message.clone()));
                }
                if let Some(current) = &live
                    && current.version != env!("CARGO_PKG_VERSION")
                {
                    live = replace_old_server(current, yes, problems).await?;
                }
                if live.is_none() && cfg!(windows) {
                    problems.push(ProblemPart::new(
                        "not-signed-in",
                        "The server runs in your desktop session on Windows, so it starts when you sign in on this PC.",
                    ));
                }
                return Ok(ServerPart {
                    mode: "service",
                    kind: Some(report.kind.to_string()),
                    running: live.is_some(),
                    version: live.as_ref().map(|live| live.version.clone()),
                    pid: live.as_ref().map(|live| live.pid),
                });
            }
            Err(error) => problems.push(ProblemPart::new(
                "no-service",
                format!(
                    "The server could not be installed as a service ({error:#}), so it runs in the background until this computer restarts. Run `rustcode up` again after a restart."
                ),
            )),
        }
    }
    if service::live_status().await.is_none() {
        let program = std::env::current_exe().context("cannot find this rustcode")?;
        rustcode::server::start_in_background(&program, false)
            .context("could not start the server in the background")?;
    }
    let live = wait_for(|| service::live_status(), Duration::from_secs(20)).await;
    Ok(ServerPart {
        mode: "background",
        kind: None,
        running: live.is_some(),
        version: live.as_ref().map(|live| live.version.clone()),
        pid: live.as_ref().map(|live| live.pid),
    })
}

/// A server of another version answers (the one running before an update).
/// Restart it from the service when no turn would be cut short, or with
/// `yes`; otherwise leave it and say so.
async fn replace_old_server(
    current: &service::LiveStatus,
    yes: bool,
    problems: &mut Vec<ProblemPart>,
) -> Result<Option<service::LiveStatus>> {
    if service::runs_inside(current.pid) || (current.running_turns > 0 && !yes) {
        problems.push(ProblemPart::new(
            "restart-pending",
            format!(
                "The server running is version {} and has turns running. `rustcode server restart --when-idle` moves it to {}.",
                current.version,
                env!("CARGO_PKG_VERSION")
            ),
        ));
        return Ok(Some(current.clone()));
    }
    step(&format!(
        "Restarting the server (version {} is running)",
        current.version
    ));
    service::restart()
        .await
        .context("could not restart the server on this version")
}

/// Try `loginctl enable-linger` without sudo: polkit lets a user enable it for
/// themself on many distributions. Answers whether lingering is on now.
fn enable_linger() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")) else {
        return false;
    };
    std::process::Command::new("loginctl")
        .args(["enable-linger", &user])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Switch remote access on and wait for the gateway.
///
/// With a server answering, the server starts the gateway, in its own session
/// (`remote_gateway::host::request_start`): on Windows this command may run
/// over SSH, in a logon session the desktop cannot reach. Elsewhere, or with no
/// server, the gateway is started from here, detached, and a server adopts it.
async fn ensure_gateway(
    config_dir: &Path,
    server_running: bool,
) -> Result<rustcode::remote_gateway::control::GatewayStatus> {
    let lifecycle = RemoteLifecycle::new(config_dir);
    if server_running && cfg!(windows) {
        rustcode::remote_gateway::host::request_start(config_dir)?;
        if let Some(status) = wait_for(
            || async { lifecycle.status().await.ok().flatten() },
            GATEWAY_TIMEOUT,
        )
        .await
        {
            rustcode::remote_gateway::host::clear_start_request(config_dir);
            return Ok(status);
        }
        bail!(
            "the server did not start remote access within {}s; see `rustcode server logs`",
            GATEWAY_TIMEOUT.as_secs()
        );
    }
    if cfg!(windows) {
        bail!(
            "the RustCode server is not running. On Windows it runs in your desktop session: sign in on this PC, then run `rustcode up` again"
        );
    }
    rustcode::remote_gateway::host::request_start(config_dir)?;
    let status = rustcode::remote_gateway::host::start(config_dir)
        .await
        .context("could not start remote access")?;
    rustcode::remote_gateway::host::clear_start_request(config_dir);
    Ok(status)
}

async fn wait_for<T, F, Fut>(mut probe: F, timeout: Duration) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(value) = probe().await {
            return Some(value);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// What `rustcode up` prints for a person.
pub fn human(report: &UpReport) -> String {
    let mut text = String::new();
    let name = report.host_name.as_deref().unwrap_or("This computer");
    match (report.server.mode, report.server.running) {
        ("service", true) => text.push_str(&format!(
            "{name} is up: the RustCode server {} runs in the background from login, with remote access on ({}).\n",
            report.version, report.gateway.listen_address
        )),
        (_, true) => text.push_str(&format!(
            "{name} is up until it restarts: the RustCode server {} runs in the background, with remote access on ({}).\n",
            report.version, report.gateway.listen_address
        )),
        (_, false) => text.push_str(&format!(
            "Remote access is on ({}), but the RustCode server is not running yet.\n",
            report.gateway.listen_address
        )),
    }
    let minutes = report.connect_code.expires_in_secs.div_ceil(60);
    text.push_str(&format!(
        "\nConnect code: {}\nWorks once, for {minutes} minutes. Anyone who has it can see and run all threads here.\n\nOn the other computer:\n  rustcode machines connect {}\n\nOr, from a computer that reaches this one over SSH:\n  rustcode machines add-ssh <user@host>\n",
        report.connect_code.code, report.connect_code.link
    ));
    for problem in &report.problems {
        text.push_str(&format!("\n{}: {}\n", problem.code, problem.message));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> UpReport {
        UpReport {
            version: "1.31.4".into(),
            host_name: Some("box".into()),
            os: "linux",
            server: ServerPart {
                mode: "service",
                kind: Some("systemd".into()),
                running: true,
                version: Some("1.31.4".into()),
                pid: Some(42),
            },
            gateway: GatewayPart {
                listen_address: "100.92.13.44:17879".into(),
                gateway_id: "gw".into(),
            },
            routes: Vec::new(),
            connect_code: CodePart {
                code: "K7MQ-29XH-TP4W-NRD6".into(),
                link: "rustcode://connect/abc".into(),
                expires_in_secs: 600,
                loopback_only: false,
            },
            problems: vec![ProblemPart::new(
                "linger-disabled",
                "sudo loginctl enable-linger ada",
            )],
        }
    }

    #[test]
    fn the_json_is_what_the_ssh_bootstrap_reads() {
        let text = serde_json::to_string(&report()).unwrap();
        let read = rustcode_client::ssh::UpReport::from_output(&text).unwrap();
        assert_eq!(read.version, "1.31.4");
        assert_eq!(read.host_name.as_deref(), Some("box"));
        assert_eq!(read.connect_code.link, "rustcode://connect/abc");
        assert!(read.server.running);
        assert_eq!(read.server.mode, "service");
        assert_eq!(read.problems[0].code, "linger-disabled");
    }

    #[test]
    fn a_person_reads_the_code_the_command_and_what_is_left_to_do() {
        let text = human(&report());
        assert!(text.contains("box is up"));
        assert!(text.contains("K7MQ-29XH-TP4W-NRD6"));
        assert!(text.contains("rustcode machines connect rustcode://connect/abc"));
        assert!(text.contains("10 minutes"));
        assert!(text.contains("linger-disabled: sudo loginctl enable-linger ada"));
    }
}
