//! One server per user and config directory: an advisory lock on
//! `run/server.lock`, held for the server's life, and `run/server.json`
//! saying where the server that holds it listens.
//!
//! The lock is the operating system's (`flock` on Unix, `LockFileEx` on
//! Windows through `File::try_lock`), so it is released however the process
//! ends; a crashed server never blocks the next one. The description is a
//! separate file: Windows does not let another process read a locked one.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const LOCK_FILE: &str = "server.lock";
const INFO_FILE: &str = "server.json";
/// Larger than any description; a bigger file is not one.
const MAX_INFO_BYTES: u64 = 16 * 1024;

/// Where a running server listens, and which one it is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServerInfo {
    pub pid: u32,
    /// The `rustcode` version it runs.
    pub version: String,
    /// The Unix socket's path, or the Windows named pipe's name.
    pub endpoint: String,
    /// When it started, in seconds since the Unix epoch.
    pub started_at: u64,
}

impl ServerInfo {
    pub(crate) fn now(endpoint: String) -> Self {
        Self {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            endpoint,
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs()),
        }
    }
}

/// The outcome of [`acquire`].
#[derive(Debug)]
pub(crate) enum Acquired {
    /// This process is the server.
    Held(ServerLock),
    /// Another one is, described when it has said where it listens yet.
    Running(Option<ServerInfo>),
}

/// The server's lock, held until dropped.
#[derive(Debug)]
pub(crate) struct ServerLock {
    _file: File,
    info: PathBuf,
}

/// Take the server lock in `dir`, creating the directory private to this
/// user if it is missing.
pub(crate) fn acquire(dir: &Path) -> io::Result<Acquired> {
    ensure_private_dir(dir)?;
    let file = open_private(&dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Acquired::Held(ServerLock {
            _file: file,
            info: dir.join(INFO_FILE),
        })),
        Err(std::fs::TryLockError::WouldBlock) => Ok(Acquired::Running(read_info(dir))),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

impl ServerLock {
    /// Say where this server listens. Replaced whole, so a reader never sees
    /// half of it.
    pub(crate) fn publish(&self, info: &ServerInfo) -> io::Result<()> {
        write_info(&self.info, info)
    }
}

/// Write a description of a running endpoint to `path` whole, readable by
/// this user alone on Unix: a reader never sees half of it, so a partially
/// written file is never taken for an endpoint.
pub(crate) fn write_info(path: &Path, info: &ServerInfo) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the endpoint description has no directory"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(&mut temporary, info)?;
    temporary.flush()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

impl Drop for ServerLock {
    fn drop(&mut self) {
        // Removed while the lock is still held: the next server never has
        // its fresh description deleted by this one.
        let _ = std::fs::remove_file(&self.info);
    }
}

/// The running server's description, if one has published it.
pub fn read_info(dir: &Path) -> Option<ServerInfo> {
    read_info_file(&dir.join(INFO_FILE))
}

/// The description in `path`, if it is a whole one.
pub(crate) fn read_info_file(path: &Path) -> Option<ServerInfo> {
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_INFO_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_INFO_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// Why the published description cannot be read, when the file is there but the caller may not
/// open it (a sandboxed shell). `None` when it is absent, readable or fails for another reason.
pub(crate) fn info_denied(dir: &Path) -> Option<io::Error> {
    match File::open(dir.join(INFO_FILE)) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Some(error),
        _ => None,
    }
}

fn open_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// Create `dir` readable by this user alone, and refuse one that is not: on
/// Unix a directory that is a symlink, someone else's, or open to group or
/// other. The socket in it is reachable only through it.
pub(crate) fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let metadata = std::fs::symlink_metadata(dir)?;
        // SAFETY: geteuid has no preconditions and cannot fail.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.uid() != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not this user's directory", dir.display()),
            ));
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // Under the user's profile (`%APPDATA%`), which only they can read.
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_server_finds_the_first_and_its_description() {
        let dir = tempfile::tempdir().expect("dir");
        let run = dir.path().join("run");
        let Acquired::Held(lock) = acquire(&run).expect("first") else {
            panic!("the first server takes the lock");
        };
        // Locked but not yet listening.
        assert!(matches!(
            acquire(&run).expect("second"),
            Acquired::Running(None)
        ));

        let info = ServerInfo::now("/tmp/server.sock".to_owned());
        lock.publish(&info).expect("publish");
        match acquire(&run).expect("third") {
            Acquired::Running(Some(running)) => assert_eq!(running, info),
            other => panic!("expected the running server, got {other:?}"),
        }

        drop(lock);
        assert_eq!(
            read_info(&run),
            None,
            "a stopped server leaves no description"
        );
        // Another test forking meanwhile holds a copy of the descriptor, and
        // so the lock, until its child execs.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !matches!(acquire(&run).expect("after"), Acquired::Held(_)) {
            assert!(
                std::time::Instant::now() < deadline,
                "the lock is free once the server stops"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_run_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("dir");
        let run = dir.path().join("run");
        std::fs::create_dir(&run).expect("run");
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        ensure_private_dir(&run).expect("private");
        let mode = std::fs::metadata(&run).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&run, &link).expect("symlink");
        assert!(ensure_private_dir(&link).is_err(), "a symlink is refused");
    }
}
