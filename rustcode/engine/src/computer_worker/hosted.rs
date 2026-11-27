//! `rc computer-worker --listen`: the worker as the RustCode app hosts it.
//!
//! macOS checks Accessibility and Screen Recording against the process
//! responsible for the caller. A worker the background server starts is its
//! own responsible process, the bare `rc` binary, which System Settings does
//! not list and the user never granted. Started by the app, the worker counts
//! as RustCode, so the grants the user gives RustCode apply. The app keeps one
//! listening on a socket in the config folder; the engine connects to it and
//! falls back to starting its own worker when no app is open.
//!
//! macOS reads a process's Screen Recording grant once, so a worker that
//! started before the user granted keeps answering "not allowed". When a
//! check finds a permission missing, the worker asks a fresh copy of itself
//! every few seconds and, once that copy is allowed, starts over in place
//! (same process, same parent), so a grant applies without restarting the app.

use super::{Op, Reply, Request};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The argument after `computer-worker` that makes it listen on the socket.
pub const LISTEN_ARG: &str = "--listen";

const PROBE_LIMIT: Duration = Duration::from_secs(5);
const RECHECK_EVERY: Duration = Duration::from_secs(3);
const RECHECK_FOR: Duration = Duration::from_secs(15 * 60);

static HANDLE: OnceLock<fn(Op) -> Result<Value, String>> = OnceLock::new();
static RECHECKING: AtomicBool = AtomicBool::new(false);

/// The socket the hosted worker listens on.
pub fn socket_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join("run").join("computer-worker.sock"))
}

/// Whether this worker was started to listen on the socket.
pub fn listening() -> bool {
    std::env::args_os()
        .nth(2)
        .is_some_and(|arg| arg == LISTEN_ARG)
}

/// Serve every engine that connects, each on a thread of its own so an idle
/// connection never holds up a permission check, until stdin closes (the app
/// quit) or the user stops the worker. Never returns.
pub(super) fn listen(handle: fn(Op) -> Result<Value, String>) -> ! {
    let _ = HANDLE.set(handle);
    std::thread::spawn(|| {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        std::process::exit(0)
    });
    let listener = match bind() {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("computer-worker: cannot listen: {error}");
            std::process::exit(1)
        }
    };
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if !same_binary(&stream) {
            continue;
        }
        let Ok(reader) = stream.try_clone() else {
            continue;
        };
        std::thread::spawn(move || super::serve_lines(BufReader::new(reader), stream, answer));
    }
    std::process::exit(0)
}

fn answer(op: Op) -> Result<Value, String> {
    let checks_permissions = matches!(op, Op::Status | Op::RequestPermissions);
    let handle = HANDLE.get().ok_or("the worker is not listening")?;
    let result = handle(op);
    let refused = |status: &Value| status["supported"] == Value::Bool(true) && !allowed(status);
    if checks_permissions && result.as_ref().is_ok_and(refused) {
        recheck_until_allowed();
    }
    result
}

fn allowed(status: &Value) -> bool {
    status["accessibility"] == Value::Bool(true) && status["screen_recording"] == Value::Bool(true)
}

/// Watch for the user's grant from a fresh worker, and start over once it is
/// allowed. One watcher at a time, for a while after the last refusal.
fn recheck_until_allowed() {
    if RECHECKING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let started = Instant::now();
        while started.elapsed() < RECHECK_FOR {
            std::thread::sleep(RECHECK_EVERY);
            if fresh_status().is_some_and(|status| allowed(&status)) {
                restart_in_place();
            }
        }
        RECHECKING.store(false, Ordering::SeqCst);
    });
}

/// The status a newly started worker sees, as this one's may be stale.
fn fresh_status() -> Option<Value> {
    let mut child = Command::new(std::env::current_exe().ok()?)
        .arg("computer-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send(line);
    });
    let mut request = serde_json::to_string(&Request {
        id: 1,
        op: Op::Status,
    })
    .ok()?;
    request.push('\n');
    let sent = stdin.write_all(request.as_bytes()).is_ok();
    let line = receiver.recv_timeout(PROBE_LIMIT);
    let _ = child.kill();
    let _ = child.wait();
    let reply: Reply = serde_json::from_str(&line.ok().filter(|_| sent)?).ok()?;
    reply.ok.then_some(reply.result)
}

/// Replace this process with a fresh copy of itself. The pid, the parent and
/// stdin stay, so the app keeps supervising it and macOS keeps counting it as
/// RustCode; the new image reads the grants again.
fn restart_in_place() {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let error = Command::new(exe).args(std::env::args_os().skip(1)).exec();
    eprintln!("computer-worker: could not start over: {error}");
}

fn bind() -> std::io::Result<UnixListener> {
    let path = socket_path().ok_or_else(|| std::io::Error::other("no config folder"))?;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no socket folder"))?;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    match std::fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Only RustCode's own engine may drive it: the same binary as this worker,
/// or one signed the same way (an engine started before an update moved the
/// app). Any other program of the user would otherwise borrow RustCode's
/// grants.
#[cfg(target_os = "macos")]
fn same_binary(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut pid: libc::pid_t = 0;
    let mut size = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `size` outlive the call and `size` is `pid`'s size.
    let read = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &raw mut size,
        )
    };
    if read != 0 || pid <= 0 {
        return false;
    }
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the length passed.
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return false;
    }
    buffer.truncate(length as usize);
    let peer = PathBuf::from(String::from_utf8_lossy(&buffer).into_owned());
    let canonical = |path: PathBuf| std::fs::canonicalize(&path).unwrap_or(path);
    std::env::current_exe().is_ok_and(|own| canonical(own) == canonical(peer))
        || super::macos::signature::same_signer(pid)
}

#[cfg(not(target_os = "macos"))]
fn same_binary(_stream: &UnixStream) -> bool {
    false
}

/// Ask the hosted worker one question, if the app runs one. `None` when no
/// worker listens or it does not answer in time.
pub fn ask(op: Op) -> Option<Value> {
    let stream = UnixStream::connect(socket_path()?).ok()?;
    stream.set_read_timeout(Some(PROBE_LIMIT)).ok()?;
    stream.set_write_timeout(Some(PROBE_LIMIT)).ok()?;
    let mut line = serde_json::to_string(&Request { id: 1, op }).ok()?;
    line.push('\n');
    (&stream).write_all(line.as_bytes()).ok()?;
    let mut answer = String::new();
    BufReader::new(&stream).read_line(&mut answer).ok()?;
    let reply: Reply = serde_json::from_str(&answer).ok()?;
    reply.ok.then_some(reply.result)
}
