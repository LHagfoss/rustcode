//! An engine started by a client — `rustcode --acp`, which is what the
//! desktop app runs — publishes where another local process can open ACP on
//! it: `run/engine.json` describing a socket of its own,
//! `run/engine-<pid>.sock` (#3341).
//!
//! A shared server is the other local engine, described by `run/server.json`
//! (`lock.rs`). The remote gateway relays a paired desktop to whichever of
//! the two is running, so a device reaches the engine whose threads the app
//! shows without a `rustcode server run` behind it.
//!
//! The endpoint is reachable only by this user: the socket is `0600` in the
//! `0700` `run` directory, and every connection's peer uid is checked
//! (`listen.rs`, `peer.rs`).

use super::listen;
use super::lock::{self, ServerInfo};
use crate::acp::AcpEngine;
use std::io;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

const INFO_FILE: &str = "engine.json";

fn info_path(run_dir: &Path) -> PathBuf {
    run_dir.join(INFO_FILE)
}

/// Where an engine started by a client published, if one did. Only
/// connecting tells it still runs: an engine that crashed leaves this behind.
pub(crate) fn published(run_dir: &Path) -> Option<ServerInfo> {
    lock::read_info_file(&info_path(run_dir))
}

/// Serve `engine` on a local endpoint of its own and say where in
/// `run/engine.json`, so a local relay can reach this engine. The description
/// and the endpoint are removed when the returned value is dropped.
pub(crate) fn publish(run_dir: &Path, engine: &AcpEngine) -> io::Result<Published> {
    lock::ensure_private_dir(run_dir)?;
    let endpoint = listen::engine_endpoint(run_dir);
    let mut listener = listen::Listener::bind(&endpoint)?;
    let info = ServerInfo::now(endpoint);
    lock::write_info(&info_path(run_dir), &info)?;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let serving = engine.clone();
    let task = tokio::spawn(async move {
        super::serve(&serving, &mut listener, stop.cancelled()).await;
    });
    Ok(Published {
        run_dir: run_dir.to_owned(),
        info,
        cancel,
        task,
    })
}

/// A published engine endpoint, serving until dropped.
pub(crate) struct Published {
    run_dir: PathBuf,
    info: ServerInfo,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Published {
    /// Where local processes reach this engine.
    pub(crate) fn endpoint(&self) -> &str {
        &self.info.endpoint
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        self.cancel.cancel();
        // The task owns the listener, and with it the socket file: aborting
        // it stops accepting, and the socket is removed with the listener.
        self.task.abort();
        remove_published(&self.run_dir, self.info.pid);
    }
}

/// Remove the description of the engine with `pid`, if it is still the one
/// published: a later engine published its own, and that one stays.
fn remove_published(run_dir: &Path, pid: u32) {
    if published(run_dir).is_none_or(|info| info.pid == pid) {
        let _ = std::fs::remove_file(info_path(run_dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_says_where_an_engine_listens() {
        let dir = tempfile::tempdir().expect("dir");
        let run = dir.path().join("run");
        lock::ensure_private_dir(&run).expect("run");
        assert!(published(&run).is_none(), "nothing published yet");
        let endpoint = listen::engine_endpoint(&run);
        let info = ServerInfo::now(endpoint.clone());
        lock::write_info(&info_path(&run), &info).expect("publish");
        let read = published(&run).expect("a description");
        assert_eq!(read.endpoint, endpoint);
        assert_eq!(read.pid, std::process::id());
    }

    #[test]
    fn an_engine_removes_only_its_own_description() {
        let dir = tempfile::tempdir().expect("dir");
        let run = dir.path().join("run");
        lock::ensure_private_dir(&run).expect("run");
        let engine = ServerInfo::now(listen::engine_endpoint(&run));
        lock::write_info(&info_path(&run), &engine).expect("publish");
        remove_published(&run, engine.pid);
        assert!(published(&run).is_none(), "its own description is removed");

        // A later engine's description is not the first one's to remove.
        let mut later = ServerInfo::now("/tmp/later.sock".to_owned());
        later.pid = engine.pid.wrapping_add(1);
        lock::write_info(&info_path(&run), &later).expect("publish");
        remove_published(&run, engine.pid);
        assert_eq!(
            published(&run)
                .expect("the later engine's description stays")
                .pid,
            later.pid
        );
    }
}
