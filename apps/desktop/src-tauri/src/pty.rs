//! The workspace terminal: the user's login shell in a pseudo-terminal, in
//! the thread's project folder. Output streams to the webview over a Tauri
//! channel; input and size come back through `pty_write` and `pty_resize`.
//! Every terminal is killed when its panel closes it or the app quits.

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tauri::ipc::Channel;

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PtyEvent {
    Data { data: String },
    Exit { code: Option<u32> },
}

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

#[derive(Default)]
pub struct Ptys {
    next: AtomicU32,
    sessions: Mutex<HashMap<u32, Session>>,
}

impl Ptys {
    pub fn close_all(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, mut s) in sessions.drain() {
                let _ = s.child.kill();
            }
        }
    }
}

const MAX_SESSIONS: usize = 8;

/// Inject lookups so every platform's policy can be tested on any host.
fn shell_command(
    os: &str,
    env: impl Fn(&str) -> Option<String>,
    on_path: impl Fn(&str) -> Option<String>,
) -> (String, &'static [&'static str]) {
    let shell = env("SHELL");
    if os == "windows" {
        let program = shell
            .filter(|value| !value.trim().is_empty())
            .or_else(|| on_path("pwsh.exe"))
            .or_else(|| on_path("powershell.exe"))
            .or_else(|| env("COMSPEC").filter(|value| !value.trim().is_empty()))
            .unwrap_or_else(|| "cmd.exe".into());
        (program, &[])
    } else {
        (shell.unwrap_or_else(|| "/bin/zsh".into()), &["-l"])
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(2, 500),
        cols: cols.clamp(2, 1000),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Text from a byte stream, keeping a split UTF-8 sequence for the next read.
pub fn take_utf8(pending: &mut Vec<u8>) -> String {
    match std::str::from_utf8(pending) {
        Ok(text) => {
            let text = text.to_owned();
            pending.clear();
            text
        }
        Err(error) => {
            let valid = error.valid_up_to();
            if error.error_len().is_none() {
                // An incomplete sequence at the end: keep it.
                let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
                pending.drain(..valid);
                text
            } else {
                let text = String::from_utf8_lossy(pending).into_owned();
                pending.clear();
                text
            }
        }
    }
}

#[tauri::command]
pub fn pty_open(
    ptys: tauri::State<'_, Ptys>,
    cwd: String,
    cols: u16,
    rows: u16,
    on_event: Channel<PtyEvent>,
) -> Result<u32, String> {
    let cwd = PathBuf::from(cwd);
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err("The terminal needs an existing folder.".into());
    }
    if ptys.sessions.lock().map_err(|e| e.to_string())?.len() >= MAX_SESSIONS {
        return Err("Too many terminals are open.".into());
    }
    let pair = native_pty_system()
        .openpty(size(cols, rows))
        .map_err(|e| e.to_string())?;
    let (shell, args) = shell_command(
        std::env::consts::OS,
        |name| std::env::var(name).ok(),
        |name| {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join(name))
                .find(|path| path.is_file())
                .map(|path| path.to_string_lossy().into_owned())
        },
    );
    // The shell owns a PTY (ConPTY on Windows); do not use CREATE_NO_WINDOW.
    let mut command = CommandBuilder::new(&shell);
    command.args(args);
    command.cwd(&cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "RustCode");
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let id = ptys.next.fetch_add(1, Ordering::Relaxed) + 1;
    ptys.sessions.lock().map_err(|e| e.to_string())?.insert(
        id,
        Session {
            master: pair.master,
            writer,
            child,
        },
    );
    std::thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        let mut pending = Vec::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buffer[..n]);
                    let data = take_utf8(&mut pending);
                    if !data.is_empty() && on_event.send(PtyEvent::Data { data }).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = on_event.send(PtyEvent::Exit { code: None });
    });
    Ok(id)
}

#[tauri::command]
pub fn pty_write(ptys: tauri::State<'_, Ptys>, id: u32, data: String) -> Result<(), String> {
    let mut sessions = ptys.sessions.lock().map_err(|e| e.to_string())?;
    let session = sessions.get_mut(&id).ok_or("No such terminal.")?;
    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| e.to_string())?;
    session.writer.flush().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_resize(
    ptys: tauri::State<'_, Ptys>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let sessions = ptys.sessions.lock().map_err(|e| e.to_string())?;
    let session = sessions.get(&id).ok_or("No such terminal.")?;
    session
        .master
        .resize(size(cols, rows))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_close(ptys: tauri::State<'_, Ptys>, id: u32) -> Result<(), String> {
    if let Some(mut session) = ptys.sessions.lock().map_err(|e| e.to_string())?.remove(&id) {
        let _ = session.child.kill();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_selection_by_platform() {
        for os in ["macos", "linux"] {
            for shell in [None, Some("/bin/fish")] {
                let (program, args) = shell_command(
                    os,
                    |key| {
                        assert_eq!(key, "SHELL");
                        shell.map(str::to_owned)
                    },
                    |_| panic!("Unix must not probe Windows shells"),
                );
                assert_eq!(program, shell.unwrap_or("/bin/zsh"));
                assert_eq!(args, &["-l"]);
            }
        }
    }

    #[test]
    fn windows_shell_precedence_and_arguments() {
        for (shell, available, comspec, expected) in [
            (
                Some("C:/Git/bin/bash.exe"),
                vec!["pwsh.exe"],
                None,
                "C:/Git/bin/bash.exe",
            ),
            (
                None,
                vec!["pwsh.exe", "powershell.exe"],
                Some("custom-cmd.exe"),
                "pwsh.exe",
            ),
            (
                None,
                vec!["powershell.exe"],
                Some("custom-cmd.exe"),
                "powershell.exe",
            ),
            (None, vec![], Some("custom-cmd.exe"), "custom-cmd.exe"),
            (None, vec![], None, "cmd.exe"),
            (Some(" "), vec!["pwsh.exe"], None, "pwsh.exe"),
            (None, vec![], Some(""), "cmd.exe"),
        ] {
            let (program, args) = shell_command(
                "windows",
                |key| match key {
                    "SHELL" => shell.map(str::to_owned),
                    "COMSPEC" => comspec.map(str::to_owned),
                    _ => panic!("unexpected environment lookup"),
                },
                |name| available.contains(&name).then(|| name.to_owned()),
            );
            assert_eq!(program, expected);
            assert!(args.is_empty(), "Windows shells must not receive -l");
        }
    }

    #[test]
    fn keeps_split_utf8_for_the_next_read() {
        let bytes = "é".as_bytes();
        let mut pending = vec![b'a', bytes[0]];
        assert_eq!(take_utf8(&mut pending), "a");
        pending.push(bytes[1]);
        assert_eq!(take_utf8(&mut pending), "é");
        assert!(pending.is_empty());
    }
}
