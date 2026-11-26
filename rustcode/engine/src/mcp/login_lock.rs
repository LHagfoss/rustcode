use std::collections::HashMap;
use std::fs::OpenOptions;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::Duration;

static IN_PROCESS: LazyLock<StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

const CROSS_PROCESS_WAIT: Duration = Duration::from_secs(330);
const CROSS_PROCESS_POLL: Duration = Duration::from_millis(50);

/// Held while one client refreshes or signs in to a remote MCP server.
///
/// A server that rotates refresh tokens accepts each one once, so two clients
/// refreshing together leave one of them with a spent token and a browser
/// sign-in of its own. Holding this guard makes the second client wait and
/// then read the token the first one saved.
pub(super) struct LoginGuard {
    file: Option<std::fs::File>,
    _local: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for LoginGuard {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = fs2::FileExt::unlock(file);
        }
    }
}

/// Waits for this server's sign-in to be free in this process and in every
/// other RustCode process sharing the config directory.
pub(super) async fn acquire(server: &str, lock_path: Option<std::path::PathBuf>) -> LoginGuard {
    let local = IN_PROCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(server.to_string())
        .or_default()
        .clone();
    let local = local.lock_owned().await;
    let file = match lock_path {
        Some(path) => cross_process(server, &path).await,
        None => None,
    };
    LoginGuard {
        file,
        _local: local,
    }
}

async fn cross_process(server: &str, path: &std::path::Path) -> Option<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    let file = crate::daemon::private_files::open_private(path, &options).ok()?;
    let deadline = tokio::time::Instant::now() + CROSS_PROCESS_WAIT;
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Some(file),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(CROSS_PROCESS_POLL).await;
            }
            Err(error) => {
                crate::dbg_log!("[mcp:{server}] sign-in lock unavailable: {error}");
                return None;
            }
        }
    }
}
