//! Settings › Machines › *This computer*: whether the RustCode server runs as a
//! background service, and the one command that installs it (`docs/server.md`).
//!
//! The work is the engine's own: `rustcode server status --json` and
//! `rustcode server install --yes`, the same commands a terminal runs, in the
//! same binary the app starts its engine from. Nothing here re-implements the
//! service managers (launchd, systemd --user, Task Scheduler), and nothing here
//! writes a unit file: the CLI is the source of truth for all three systems.
//!
//! A server in the background is what keeps threads running when the app
//! closes, so Settings › Remote access makes sure it is installed when network
//! access is turned on.

use crate::engine::resolve_engine;
use crate::process;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// `server status --json` answers in well under a second; a locked credential
/// store or a stuck service manager is what this is for.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(20);

/// `server install` writes a unit, starts it and waits for the socket, which
/// the engine bounds at 30 seconds (`server_service`); this is longer than that.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(90);

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
    /// What is wrong, in the words `server status` uses: `linger-disabled`,
    /// `version-mismatch`, and the rest.
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

/// What `server status --json` prints; only the fields the page reads.
#[derive(Debug, Deserialize)]
struct Report {
    installed: bool,
    running: bool,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    cli_version: Option<String>,
    #[serde(default)]
    problems: Vec<ServiceProblem>,
}

/// Whether the RustCode server is installed as a service, and running.
#[tauri::command]
pub async fn server_service_status() -> Result<ServiceStatus, String> {
    read_status().await
}

/// Install the RustCode server as a background service, so threads keep
/// running when the app closes, and answer the state after it. The install is
/// the CLI's own: a development build is refused there, and a server another
/// process runs is never stopped while a turn is running — the install says so
/// instead, and `--yes` stays a decision for a terminal.
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

/// The current state from `rustcode server status --json`. A status that
/// cannot be read is a status the page says it does not know, never an error
/// the page cannot show.
async fn read_status() -> Result<ServiceStatus, String> {
    match run(&["server", "status", "--json"], STATUS_TIMEOUT).await {
        Ok(output) if output.status.success() => {
            let report: Report = serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("Could not read the server's status: {error}"))?;
            Ok(ServiceStatus {
                installed: report.installed,
                running: report.running,
                version: report.version,
                cli_version: report.cli_version,
                problems: report.problems,
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

/// Run `rustcode <args>` and answer what it printed, killing it once `timeout`
/// passes: a command that never answers must not hold a settings page.
pub async fn run(args: &[&str], timeout: Duration) -> Result<std::process::Output, String> {
    run_program(&resolve_engine(), args, timeout)
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
