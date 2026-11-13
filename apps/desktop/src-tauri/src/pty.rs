//! The workspace terminal: the user's login shell in a pseudo-terminal, in
//! the thread's project folder. Output streams to the webview over a Tauri
//! channel; input and size come back through `pty_write` and `pty_resize`.
//! Every terminal is killed when its panel closes it or the app quits.

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use tauri::ipc::Channel;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PtyEvent {
    Data { data: String },
    Exit { code: Option<u32> },
}

/// One terminal. Input goes to its own writer thread, so a shell that is
/// not reading never holds up the app's main thread or another terminal;
/// the reader thread owns the child, waits for it and reports its exit code.
struct Session {
    master: Box<dyn MasterPty + Send>,
    input: mpsc::Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
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
                let _ = s.killer.kill();
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

/// Starts `program` in a new pseudo-terminal in `cwd`, with a writer thread
/// for its input and a reader thread that hands its output to `emit` (false
/// when nobody listens any more) and ends with its exit code.
fn spawn_session(
    cwd: &Path,
    cols: u16,
    rows: u16,
    program: &str,
    args: &[&str],
    emit: impl Fn(PtyEvent) -> bool + Send + 'static,
) -> Result<Session, String> {
    let pair = native_pty_system()
        .openpty(size(cols, rows))
        .map_err(|e| e.to_string())?;
    // The shell owns a PTY (ConPTY on Windows); do not use CREATE_NO_WINDOW.
    let mut command = CommandBuilder::new(program);
    command.args(args);
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "RustCode");
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let killer = child.clone_killer();
    let (input, queued) = mpsc::channel::<Vec<u8>>();
    // Ends when the session is closed (its sender dropped) or the shell stops reading for good.
    std::thread::spawn(move || {
        for chunk in queued {
            if writer
                .write_all(&chunk)
                .and_then(|_| writer.flush())
                .is_err()
            {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        let mut pending = Vec::new();
        let mut listening = true;
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buffer[..n]);
                    let data = take_utf8(&mut pending);
                    if !data.is_empty() && !emit(PtyEvent::Data { data }) {
                        listening = false;
                        break;
                    }
                }
            }
        }
        // Nobody reads this terminal any more: stop its shell rather than wait for it.
        if !listening {
            let _ = child.kill();
        }
        // Reaped here, so a closed or killed shell leaves no zombie, and its code is reported.
        let code = child.wait().ok().map(|status| status.exit_code());
        let _ = emit(PtyEvent::Exit { code });
    });
    Ok(Session {
        master: pair.master,
        input,
        killer,
    })
}

#[tauri::command(async)]
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
    let session = spawn_session(&cwd, cols, rows, &shell, args, move |event| {
        on_event.send(event).is_ok()
    })?;
    let id = ptys.next.fetch_add(1, Ordering::Relaxed) + 1;
    ptys.sessions
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id, session);
    Ok(id)
}

/// Queues input for the terminal's writer thread. Synchronous on purpose, so
/// keystrokes reach the shell in the order they were typed, and cheap: the
/// sessions lock is held for a lookup and a channel send, never for a write.
#[tauri::command]
pub fn pty_write(ptys: tauri::State<'_, Ptys>, id: u32, data: String) -> Result<(), String> {
    let sessions = ptys.sessions.lock().map_err(|e| e.to_string())?;
    let session = sessions.get(&id).ok_or("No such terminal.")?;
    session
        .input
        .send(data.into_bytes())
        .map_err(|_| "The terminal has stopped.".to_string())
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

/// Stops the terminal's shell; its reader thread reaps it and reports the code.
#[tauri::command(async)]
pub fn pty_close(ptys: tauri::State<'_, Ptys>, id: u32) -> Result<(), String> {
    let session = ptys.sessions.lock().map_err(|e| e.to_string())?.remove(&id);
    if let Some(mut session) = session {
        let _ = session.killer.kill();
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

    #[cfg(unix)]
    fn run_shell(script: &str) -> (Session, mpsc::Receiver<PtyEvent>) {
        let (tx, rx) = mpsc::channel();
        let session = spawn_session(
            &std::env::temp_dir(),
            80,
            24,
            "/bin/sh",
            &["-c", script],
            move |event| tx.send(event).is_ok(),
        )
        .unwrap();
        (session, rx)
    }

    /// Everything the terminal said until it exited, and its exit code.
    #[cfg(unix)]
    fn until_exit(rx: &mpsc::Receiver<PtyEvent>) -> (String, Option<u32>) {
        let mut out = String::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(PtyEvent::Data { data }) => out.push_str(&data),
                Ok(PtyEvent::Exit { code }) => return (out, code),
                Err(error) => panic!("no exit: {error}; said {out:?}"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn input_reaches_the_shell_in_order_and_its_exit_code_is_reported() {
        let (session, rx) = run_shell("read a; read b; echo \"got:$a$b\"; exit 3");
        session.input.send(b"one\n".to_vec()).unwrap();
        session.input.send(b"two\n".to_vec()).unwrap();
        let (out, code) = until_exit(&rx);
        assert!(out.contains("got:onetwo"), "{out:?}");
        assert_eq!(code, Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn a_closed_terminal_is_killed_and_reaped() {
        let (mut session, rx) = run_shell("sleep 30");
        session.killer.kill().unwrap();
        // Reaped by the reader thread: an exit is reported, not left as a zombie.
        let (_, code) = until_exit(&rx);
        assert!(code.is_some_and(|code| code != 0), "{code:?}");
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
