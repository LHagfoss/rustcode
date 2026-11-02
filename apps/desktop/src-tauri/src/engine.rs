//! The engine sidecar: runs `rustcode --acp`, relays its newline-delimited
//! JSON-RPC to the webview over a Tauri Channel, and restarts it with backoff
//! when it exits. The webview only ever sees what the engine prints; nothing
//! here reads or forwards credentials.

use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
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
}

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
        if let Some(channel) = self.channel.lock().unwrap().as_ref() {
            let _ = channel.send(event);
        }
    }

    /// Attach the webview's channel and make sure an engine runs. A webview
    /// reload attaches a new channel; the running engine gets a fresh `open`.
    pub fn start(self: &Arc<Self>, channel: Channel<EngineEvent>) {
        *self.channel.lock().unwrap() = Some(channel);
        self.stopping.store(false, Ordering::SeqCst);
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
        let stdin = guard.as_mut().ok_or("The engine is not running.")?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("Could not write to the engine: {e}"))
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
            let reason = match self.run_once(&path) {
                Ok(reason) => reason,
                Err(error) => error,
            };
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

    /// Run one engine process until it exits; returns why it ended.
    fn run_once(&self, path: &PathBuf) -> Result<String, String> {
        let mut child = Command::new(path)
            .arg("--acp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start {}: {e}.", path.display()))?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        *self.stdin.lock().unwrap() = child.stdin.take();
        let pid = child.id();
        *self.child.lock().unwrap() = Some(child);
        self.emit(EngineEvent::Log {
            line: format!("desktop: started {} --acp (pid {pid})", path.display()),
        });
        self.emit(EngineEvent::Open {
            engine: path.display().to_string(),
        });

        let stdout_channel = self.channel.lock().unwrap().clone();
        let out = thread::spawn(move || {
            let mut reader = BufReader::with_capacity(256 * 1024, stdout);
            while let Ok(Some(mut lines)) = read_batch(&mut reader) {
                let Some(channel) = &stdout_channel else {
                    continue;
                };
                let event = if lines.len() == 1 {
                    EngineEvent::Line {
                        line: lines.pop().unwrap_or_default(),
                    }
                } else {
                    EngineEvent::Lines { lines }
                };
                let _ = channel.send(event);
            }
        });
        let stderr_channel = self.channel.lock().unwrap().clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if cfg!(debug_assertions) {
                    eprintln!("[engine] {line}");
                }
                if let Some(channel) = &stderr_channel {
                    let _ = channel.send(EngineEvent::Log { line });
                }
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
        Ok(match status.and_then(|s| s.code()) {
            Some(code) => format!("The engine exited (code {code})."),
            None => "The engine stopped.".to_string(),
        })
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
    fn lines_event_matches_the_webview_shape() {
        let json = serde_json::to_string(&EngineEvent::Lines {
            lines: vec!["{}".into()],
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"lines","lines":["{}"]}"#);
    }
}
