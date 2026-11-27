//! The engine's side of `rc computer-worker`: start it on the first desktop
//! call, send one request at a time, and stop it after two idle minutes. When
//! it exits on its own (the user chose Stop in the menu bar), every thread's
//! desktop grants end.

use crate::computer_worker::{Op, Reply, Request, WORKER_ARG};
use serde_json::Value;
use std::process::Stdio;
use std::sync::Once;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_util::sync::CancellationToken;

const IDLE_LIMIT: Duration = Duration::from_secs(2 * 60);
const WATCH_EVERY: Duration = Duration::from_secs(5);
const STOPPED: &str = "Desktop control stopped: the user chose Stop in the menu bar, or its worker exited. Nothing more was run; ask the user before trying again.";

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next: u64,
    last_used: Instant,
}

static WORKER: tokio::sync::Mutex<Option<Worker>> = tokio::sync::Mutex::const_new(None);

fn spawn() -> Result<Worker, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let log = crate::config::get_config_dir()
        .map(|dir| dir.join("computer-use"))
        .and_then(|dir| std::fs::create_dir_all(&dir).ok().map(|()| dir))
        .and_then(|dir| std::fs::File::create(dir.join("desktop-worker.log")).ok());
    let mut child = Command::new(executable)
        .arg(WORKER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log.map_or_else(Stdio::null, Stdio::from))
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Could not start the desktop worker: {e}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or("The desktop worker has no stdin")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("The desktop worker has no stdout")?;
    Ok(Worker {
        child,
        stdin,
        lines: BufReader::new(stdout).lines(),
        next: 1,
        last_used: Instant::now(),
    })
}

fn watch() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(WATCH_EVERY).await;
                let Ok(mut slot) = WORKER.try_lock() else {
                    continue;
                };
                let Some(worker) = slot.as_mut() else {
                    continue;
                };
                if worker.child.try_wait().ok().flatten().is_some() {
                    *slot = None;
                    super::desktop::stopped();
                } else if worker.last_used.elapsed() > IDLE_LIMIT {
                    *slot = None;
                }
            }
        });
    });
}

/// Run one request on the worker, starting it if needed.
pub(super) async fn call(
    op: Op,
    limit: Duration,
    cancel: &CancellationToken,
) -> Result<Value, String> {
    let mut slot = WORKER.lock().await;
    if slot
        .as_mut()
        .is_some_and(|worker| worker.child.try_wait().ok().flatten().is_some())
    {
        *slot = None;
        super::desktop::stopped();
        return Err(STOPPED.into());
    }
    if slot.is_none() {
        *slot = Some(spawn()?);
        watch();
    }
    let worker = slot.as_mut().ok_or("The desktop worker did not start")?;
    let id = worker.next;
    worker.next += 1;
    worker.last_used = Instant::now();
    let mut line = serde_json::to_string(&Request { id, op }).map_err(|e| e.to_string())?;
    line.push('\n');
    let sent = async {
        worker.stdin.write_all(line.as_bytes()).await?;
        worker.stdin.flush().await
    }
    .await;
    if sent.is_err() {
        *slot = None;
        super::desktop::stopped();
        return Err(STOPPED.into());
    }
    let answer = async {
        loop {
            match worker.lines.next_line().await {
                Ok(Some(text)) => match serde_json::from_str::<Reply>(&text) {
                    Ok(reply) if reply.id == id => return Some(reply),
                    _ => continue,
                },
                _ => return None,
            }
        }
    };
    let reply = tokio::select! {
        _ = cancel.cancelled() => return Err("Cancelled".into()),
        reply = tokio::time::timeout(limit, answer) => reply,
    };
    let reply = match reply {
        Ok(Some(reply)) => reply,
        Ok(None) => {
            *slot = None;
            super::desktop::stopped();
            return Err(STOPPED.into());
        }
        Err(_) => {
            *slot = None;
            return Err(format!(
                "The desktop action took longer than {} s; the worker was restarted",
                limit.as_secs()
            ));
        }
    };
    if let Some(worker) = slot.as_mut() {
        worker.last_used = Instant::now();
    }
    if reply.ok {
        Ok(reply.result)
    } else {
        Err(reply
            .error
            .unwrap_or_else(|| "The desktop action failed".into()))
    }
}
