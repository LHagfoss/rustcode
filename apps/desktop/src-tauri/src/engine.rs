//! The engine sidecar: runs `rustcode --acp`, relays its newline-delimited
//! JSON-RPC to the webview over a Tauri Channel, and restarts it with backoff
//! when it exits. The webview only ever sees what the engine prints; nothing
//! here reads or forwards credentials. Users know this process as the
//! "RustCode server" (docs/terminology.md), so the text they read says server.
//!
//! What it knows about the running process (pid, start, the last exit, how
//! often it came back after a crash) is kept for Settings › Diagnostics
//! (`diagnostics.rs`).

use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;

/// What the webview receives; matches `TransportEvent` in `src/acp/transport.ts`.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EngineEvent {
    Line {
        line: String,
    },
    /// Lines that were already buffered together (a replay, a burst of chunks):
    /// one channel message, and one task in the webview, instead of one per line.
    Lines {
        lines: Vec<String>,
    },
    Open {
        engine: String,
    },
    Closed {
        reason: String,
    },
    Log {
        line: String,
    },
}

/// At most this many lines, or about this many bytes, go in one `Lines` message.
const BATCH_LINES: usize = 1024;
const BATCH_BYTES: usize = 4 * 1024 * 1024;

/// Blocks for one line, then takes every further line that is already complete
/// in the buffer, without waiting for more. `None` at the end of the stream.
fn read_batch<R: std::io::Read>(reader: &mut BufReader<R>) -> std::io::Result<Option<Vec<String>>> {
    let mut lines = Vec::new();
    let mut bytes = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok((!lines.is_empty()).then_some(lines));
        }
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        bytes += line.len();
        lines.push(line);
        let more = reader.buffer().contains(&b'\n');
        if !more || lines.len() >= BATCH_LINES || bytes >= BATCH_BYTES {
            return Ok(Some(lines));
        }
    }
}

const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A run that stayed up this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// How long a stopping engine gets after its stdin closes before it is killed.
const GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Engine {
    channel: Mutex<Option<Channel<EngineEvent>>>,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    supervising: AtomicBool,
    stopping: AtomicBool,
    /// Set by `restart`: start again at once instead of after the backoff.
    restart_now: AtomicBool,
    generation: AtomicU64,
    /// Started by `prestart`, before the webview attached: the first `start`
    /// adopts that engine instead of restarting it.
    prestarted: AtomicBool,
    /// What the engine said before the webview attached (its `open`, early logs).
    early: Mutex<Vec<EngineEvent>>,
    /// The current run and the last exit, for Diagnostics.
    run: Mutex<RunState>,
}

/// How the last engine process ended.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LastExit {
    pub code: Option<i32>,
    /// The signal that ended it (Unix only).
    pub signal: Option<i32>,
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub reason: String,
    /// The app asked for it (a restart, quitting); otherwise it crashed or failed to start.
    pub expected: bool,
}

#[derive(Default)]
struct RunState {
    pid: Option<u32>,
    path: Option<PathBuf>,
    started_at_ms: Option<u64>,
    /// Times the supervisor started the engine again after an exit nobody asked for.
    crash_restarts: u32,
    last_exit: Option<LastExit>,
}

/// The engine as Diagnostics shows it.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub path: Option<String>,
    pub started_at_ms: Option<u64>,
    pub crash_restarts: u32,
    pub last_exit: Option<LastExit>,
}

/// How one engine process ended: its exit code, or the signal that stopped it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl Exit {
    fn of(status: Option<std::process::ExitStatus>) -> Self {
        let Some(status) = status else {
            return Self::default();
        };
        #[cfg(unix)]
        let signal = std::os::unix::process::ExitStatusExt::signal(&status);
        #[cfg(not(unix))]
        let signal = None;
        Self {
            code: status.code(),
            signal,
        }
    }

    /// What the user reads when the server ends.
    pub fn reason(self) -> String {
        match (self.code, self.signal) {
            (Some(code), _) => format!("The server exited (code {code})."),
            (None, Some(signal)) => format!("The server was stopped by signal {signal}."),
            (None, None) => "The server stopped.".to_string(),
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Events kept for a webview that has not attached yet.
const EARLY_MAX: usize = 200;

/// `$RUSTCODE_ENGINE`, else the workspace's debug build in a dev build, else
/// the bundled sidecar next to the app binary, else `rustcode` on PATH.
pub fn resolve_engine() -> PathBuf {
    if let Some(path) = std::env::var_os("RUSTCODE_ENGINE") {
        return PathBuf::from(path);
    }
    if cfg!(debug_assertions) {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/rustcode");
        if dev.exists() {
            return dev;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join(if cfg!(windows) {
                "rustcode.exe"
            } else {
                "rustcode"
            });
            if bundled.exists() {
                return bundled;
            }
        }
    }
    PathBuf::from("rustcode")
}

impl Engine {
    fn emit(&self, event: EngineEvent) {
        let guard = self.channel.lock().unwrap();
        if let Some(channel) = guard.as_ref() {
            let _ = channel.send(event);
        } else if self.prestarted.load(Ordering::SeqCst) {
            let mut early = self.early.lock().unwrap();
            if early.len() < EARLY_MAX {
                early.push(event);
            }
        }
    }

    /// Start the engine as the app launches, while the webview still loads, so
    /// it has initialized by the time the page asks (saves its startup time).
    pub fn prestart(self: &Arc<Self>) {
        if self.supervising.swap(true, Ordering::SeqCst) {
            return;
        }
        self.prestarted.store(true, Ordering::SeqCst);
        let engine = Arc::clone(self);
        thread::spawn(move || engine.supervise());
    }

    /// Attach the webview's channel and make sure an engine runs. A webview
    /// reload attaches a new channel; the running engine gets a fresh `open`.
    pub fn start(self: &Arc<Self>, channel: Channel<EngineEvent>) {
        {
            // Under the channel lock, so nothing the engine says slips between the early events and the channel.
            let mut guard = self.channel.lock().unwrap();
            for event in self.early.lock().unwrap().drain(..) {
                let _ = channel.send(event);
            }
            *guard = Some(channel);
        }
        self.stopping.store(false, Ordering::SeqCst);
        if self.prestarted.swap(false, Ordering::SeqCst) {
            // The first page adopts the engine started at launch; its `open` was in the early events
            // or is still to come.
            return;
        }
        if self.supervising.swap(true, Ordering::SeqCst) {
            // Already running: hand the new page a clean engine.
            self.restart();
            return;
        }
        let engine = Arc::clone(self);
        thread::spawn(move || engine.supervise());
    }

    pub fn send(&self, line: &str) -> Result<(), String> {
        let mut guard = self.stdin.lock().unwrap();
        let stdin = guard.as_mut().ok_or("The server is not running.")?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("Could not write to the server: {e}"))
    }

    /// The current process and the last exit, for Settings › Diagnostics.
    pub fn status(&self) -> RunStatus {
        let run = self.run.lock().unwrap();
        let running = self.child.lock().unwrap().is_some();
        RunStatus {
            running,
            pid: running.then_some(run.pid).flatten(),
            path: run.path.as_ref().map(|p| p.display().to_string()),
            started_at_ms: running.then_some(run.started_at_ms).flatten(),
            crash_restarts: run.crash_restarts,
            last_exit: run.last_exit.clone(),
        }
    }

    /// Stop the current engine; the supervisor starts a fresh one at once.
    pub fn restart(&self) {
        self.restart_now.store(true, Ordering::SeqCst);
        self.shutdown_child();
    }

    /// Stop the engine and do not start another.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.shutdown_child();
    }

    /// Close stdin (the engine's cue to clean up its sessions), then kill it if it lingers.
    fn shutdown_child(&self) {
        self.stdin.lock().unwrap().take();
        let deadline = Instant::now() + GRACE;
        loop {
            let mut guard = self.child.lock().unwrap();
            let Some(child) = guard.as_mut() else { return };
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    return;
                }
                Ok(None) => {}
            }
            drop(guard);
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn supervise(self: Arc<Self>) {
        let mut backoff = BACKOFF_START;
        while !self.stopping.load(Ordering::SeqCst) {
            let path = resolve_engine();
            let started = Instant::now();
            let (reason, exit) = match self.run_once(&path) {
                Ok(exit) => (exit.reason(), exit),
                Err(error) => (error, Exit::default()),
            };
            let expected =
                self.stopping.load(Ordering::SeqCst) || self.restart_now.load(Ordering::SeqCst);
            self.record_exit(exit, &reason, expected);
            self.emit(EngineEvent::Closed {
                reason: reason.clone(),
            });
            if self.stopping.load(Ordering::SeqCst) {
                break;
            }
            if self.restart_now.swap(false, Ordering::SeqCst) {
                backoff = BACKOFF_START;
                continue;
            }
            self.run.lock().unwrap().crash_restarts += 1;
            if started.elapsed() > STABLE_AFTER {
                backoff = BACKOFF_START;
            }
            self.emit(EngineEvent::Log {
                line: format!(
                    "desktop: {reason} Restarting in {} ms.",
                    backoff.as_millis()
                ),
            });
            // Sleep in slices so a restart or stop cuts the wait short.
            let until = Instant::now() + backoff;
            while Instant::now() < until
                && !self.stopping.load(Ordering::SeqCst)
                && !self.restart_now.load(Ordering::SeqCst)
            {
                thread::sleep(Duration::from_millis(50));
            }
            self.restart_now.store(false, Ordering::SeqCst);
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
        self.supervising.store(false, Ordering::SeqCst);
    }

    fn record_exit(&self, exit: Exit, reason: &str, expected: bool) {
        let mut run = self.run.lock().unwrap();
        run.pid = None;
        run.started_at_ms = None;
        run.last_exit = Some(LastExit {
            code: exit.code,
            signal: exit.signal,
            at_ms: now_ms(),
            reason: reason.to_string(),
            expected,
        });
    }

    /// Run one engine process until it exits; returns how it ended.
    fn run_once(self: &Arc<Self>, path: &PathBuf) -> Result<Exit, String> {
        // Recorded before the spawn, so a binary that will not start is still named.
        self.run.lock().unwrap().path = Some(path.clone());
        let mut child = crate::process::command(path)
            .arg("--acp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start the server ({}): {e}.", path.display()))?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        *self.stdin.lock().unwrap() = child.stdin.take();
        let pid = child.id();
        {
            let mut run = self.run.lock().unwrap();
            run.pid = Some(pid);
            run.started_at_ms = Some(now_ms());
        }
        *self.child.lock().unwrap() = Some(child);
        self.emit(EngineEvent::Log {
            line: format!("desktop: started {} --acp (pid {pid})", path.display()),
        });
        self.emit(EngineEvent::Open {
            engine: path.display().to_string(),
        });

        // The readers look the channel up per message: the engine may start before the page attaches.
        let engine = Arc::clone(self);
        let out = thread::spawn(move || {
            let mut reader = BufReader::with_capacity(256 * 1024, stdout);
            while let Ok(Some(mut lines)) = read_batch(&mut reader) {
                engine.emit(if lines.len() == 1 {
                    EngineEvent::Line {
                        line: lines.pop().unwrap_or_default(),
                    }
                } else {
                    EngineEvent::Lines { lines }
                });
            }
        });
        let engine = Arc::clone(self);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if cfg!(debug_assertions) {
                    eprintln!("[engine] {line}");
                }
                engine.emit(EngineEvent::Log { line });
            }
        });

        let status = loop {
            let mut guard = self.child.lock().unwrap();
            match guard.as_mut().map(Child::try_wait) {
                Some(Ok(Some(status))) => {
                    guard.take();
                    break Some(status);
                }
                Some(Ok(None)) => {}
                Some(Err(_)) | None => {
                    guard.take();
                    break None;
                }
            }
            drop(guard);
            thread::sleep(Duration::from_millis(100));
        };
        // Every stdout line reaches the page before it hears the engine closed.
        let _ = out.join();
        if self.generation.load(Ordering::SeqCst) == generation {
            self.stdin.lock().unwrap().take();
        }
        Ok(Exit::of(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn batches_lines_already_buffered() {
        let mut reader = BufReader::new(Cursor::new(b"a\nb\r\nc\n".to_vec()));
        assert_eq!(
            read_batch(&mut reader).unwrap(),
            Some(vec!["a".into(), "b".into(), "c".into()])
        );
        assert_eq!(read_batch(&mut reader).unwrap(), None);
    }

    #[test]
    fn keeps_a_last_line_without_newline() {
        // A line still being written waits for the next read rather than going out half.
        let mut reader = BufReader::new(Cursor::new(b"a\nb".to_vec()));
        assert_eq!(read_batch(&mut reader).unwrap(), Some(vec!["a".into()]));
        assert_eq!(read_batch(&mut reader).unwrap(), Some(vec!["b".into()]));
        assert_eq!(read_batch(&mut reader).unwrap(), None);
    }

    #[test]
    fn caps_a_batch() {
        let text = "x\n".repeat(BATCH_LINES + 5);
        let mut reader = BufReader::with_capacity(1 << 20, Cursor::new(text.into_bytes()));
        assert_eq!(read_batch(&mut reader).unwrap().unwrap().len(), BATCH_LINES);
        assert_eq!(read_batch(&mut reader).unwrap().unwrap().len(), 5);
    }

    #[test]
    fn an_exit_says_its_code_or_its_signal() {
        let code = Exit {
            code: Some(101),
            signal: None,
        };
        assert_eq!(code.reason(), "The server exited (code 101).");
        let signal = Exit {
            code: None,
            signal: Some(9),
        };
        assert_eq!(signal.reason(), "The server was stopped by signal 9.");
        assert_eq!(Exit::default().reason(), "The server stopped.");
    }

    #[test]
    fn a_fresh_engine_reports_nothing_running() {
        let status = Engine::default().status();
        assert!(!status.running);
        assert_eq!(status.pid, None);
        assert_eq!(status.crash_restarts, 0);
        assert!(status.last_exit.is_none());
    }

    #[test]
    fn a_recorded_exit_is_reported_and_clears_the_run() {
        let engine = Engine::default();
        engine.run.lock().unwrap().pid = Some(42);
        engine.record_exit(
            Exit {
                code: Some(1),
                signal: None,
            },
            "The server exited (code 1).",
            false,
        );
        let status = engine.status();
        let last = status.last_exit.unwrap();
        assert_eq!(last.code, Some(1));
        assert!(!last.expected);
        assert!(last.at_ms > 0);
        assert_eq!(engine.run.lock().unwrap().pid, None);
    }

    #[test]
    fn lines_event_matches_the_webview_shape() {
        let json = serde_json::to_string(&EngineEvent::Lines {
            lines: vec!["{}".into()],
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"lines","lines":["{}"]}"#);
    }
}
