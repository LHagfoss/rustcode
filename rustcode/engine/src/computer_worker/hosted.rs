//! `rc computer-worker --listen`: the worker as the RustCode app hosts it.
//!
//! macOS checks Accessibility and Screen Recording against the process
//! responsible for the caller. A worker the background server starts is its
//! own responsible process, the bare `rc` binary, which System Settings does
//! not list and the user never granted. Started by the app, the worker counts
//! as RustCode, so the grants the user gives RustCode apply. The app keeps one
//! listening on a socket in the config folder; the engine connects to it and
//! falls back to starting its own worker when no app is open.

use super::{Op, Reply, Request};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// The argument after `computer-worker` that makes it listen on the socket.
pub const LISTEN_ARG: &str = "--listen";

const PROBE_LIMIT: Duration = Duration::from_secs(5);

/// Engine connections being served; when the last one closes, nothing is
/// controlled any more and the cursor's label is reset.
static OPEN: AtomicUsize = AtomicUsize::new(0);

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
        OPEN.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            super::serve_lines(BufReader::new(reader), stream, handle);
            if OPEN.fetch_sub(1, Ordering::SeqCst) == 1 {
                let _ = handle(Op::Activity { label: None });
            }
        });
    }
    std::process::exit(0)
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

/// Only RustCode's own engine, the same binary as this worker, may drive it:
/// any other program of the user would otherwise borrow RustCode's grants.
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
