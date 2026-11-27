//! Hosts the desktop-control worker (`rc computer-worker --listen`) as a child
//! of the app, so macOS checks RustCode's own Accessibility and Screen
//! Recording grants for it instead of the bare `rc` binary's
//! (`rustcode/engine/src/computer_worker/hosted.rs`). The engine connects to
//! it when it runs. The worker exits when the app quits, as its stdin closes,
//! and is started again when it exits early (the user chose Stop).
//!
//! The setup step `desktop-control` asks through a one-off worker started
//! the same way, so the prompts and System Settings name RustCode.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

const WORKER_ARGS: [&str; 2] = ["computer-worker", "--listen"];
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LONGEST_RETRY: Duration = Duration::from_secs(60);
const STEADY_RUN: Duration = Duration::from_secs(60);
const ASK_LIMIT: Duration = Duration::from_secs(10);

/// Start hosting the worker, on macOS only.
pub fn start() {
    if cfg!(target_os = "macos") {
        std::thread::spawn(supervise);
    }
}

fn supervise() {
    let mut retry = FIRST_RETRY;
    loop {
        let started = Instant::now();
        if let Err(error) = run_once() {
            eprintln!("desktop-control worker: {error}");
        }
        retry = if started.elapsed() >= STEADY_RUN {
            FIRST_RETRY
        } else {
            (retry * 2).min(LONGEST_RETRY)
        };
        std::thread::sleep(retry);
    }
}

fn run_once() -> Result<(), String> {
    let log = crate::diagnostics::config_dir()
        .map(|dir| dir.join("computer-use"))
        .and_then(|dir| std::fs::create_dir_all(&dir).ok().map(|()| dir))
        .and_then(|dir| std::fs::File::create(dir.join("hosted-worker.log")).ok());
    let mut child = crate::process::command(crate::engine::resolve_engine())
        .args(WORKER_ARGS)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(log.map_or_else(Stdio::null, Stdio::from))
        .spawn()
        .map_err(|error| format!("could not start: {error}"))?;
    let stdin = child.stdin.take();
    let status = child.wait().map_err(|error| error.to_string())?;
    drop(stdin);
    Err(format!("exited ({status})"))
}

/// Ask a one-off worker started by the app: `status`, or
/// `request_permissions` to show macOS's prompts for RustCode.
pub fn ask(op: &str) -> Result<Value, String> {
    let mut child = crate::process::command(crate::engine::resolve_engine())
        .arg(WORKER_ARGS[0])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start the desktop-control worker: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("The worker has no stdin")?;
    let stdout = child.stdout.take().ok_or("The worker has no stdout")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send(line);
    });
    let request = json!({"id": 1, "op": op}).to_string();
    let sent = writeln!(stdin, "{request}").and_then(|()| stdin.flush());
    let line = receiver.recv_timeout(ASK_LIMIT);
    let _ = child.kill();
    let _ = child.wait();
    sent.map_err(|error| error.to_string())?;
    let line = line.map_err(|_| "The desktop-control worker did not answer".to_owned())?;
    let reply: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
    if reply["ok"] == json!(true) {
        Ok(reply["result"].clone())
    } else {
        Err(reply["error"]
            .as_str()
            .unwrap_or("The desktop-control worker failed")
            .to_owned())
    }
}

/// The Privacy & Security pane in System Settings for one permission desktop
/// control needs.
fn privacy_pane(permission: &str) -> Option<&'static str> {
    match permission {
        "accessibility" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        }
        "screen_recording" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        }
        _ => None,
    }
}

/// Opens the System Settings pane where the user turns `permission` on for
/// RustCode.
#[tauri::command(async)]
pub fn open_desktop_control_settings(permission: String) -> Result<(), String> {
    let pane = privacy_pane(&permission).ok_or("Unknown permission")?;
    if !cfg!(target_os = "macos") {
        return Err("Only the macOS app has these settings to open.".into());
    }
    crate::process::command("open")
        .arg(pane)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The permissions the worker still lacks, by their System Settings names.
pub fn missing(status: &Value) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if status["accessibility"] != json!(true) {
        missing.push("Accessibility");
    }
    if status["screen_recording"] != json!(true) {
        missing.push("Screen & System Audio Recording");
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_the_two_privacy_panes() {
        assert!(privacy_pane("accessibility")
            .unwrap()
            .ends_with("Privacy_Accessibility"));
        assert!(privacy_pane("screen_recording")
            .unwrap()
            .ends_with("Privacy_ScreenCapture"));
        assert_eq!(privacy_pane("camera"), None);
    }

    #[test]
    fn names_each_permission_still_missing() {
        assert!(missing(&json!({"accessibility": true, "screen_recording": true})).is_empty());
        assert_eq!(
            missing(&json!({"accessibility": false, "screen_recording": true})),
            ["Accessibility"]
        );
        assert_eq!(
            missing(&json!({})),
            ["Accessibility", "Screen & System Audio Recording"]
        );
    }
}
