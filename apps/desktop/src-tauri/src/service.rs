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
                gateway: report.gateway,
                path_rustcode: report.path_rustcode,
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
    run_program(&prepare_program()?, args, timeout)
}

/// Keep the copy a service installed from an AppImage runs up to date, at app
/// start: the AppImage may have been updated since it was made. A copy that
/// cannot be made is not reported here (the app still runs); the page says it
/// when it asks for the status or installs.
pub fn refresh_program() {
    let _ = prepare_program();
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
