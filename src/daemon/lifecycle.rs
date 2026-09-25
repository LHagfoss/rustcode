//! Ownership is held by an advisory lock for the entire foreground lifetime.
//! Stop uses authenticated instance identity over the socket, never PID signals.
use super::{
    client::DaemonClient,
    protocol::{DaemonRequest, DaemonResponse, DaemonStatus, PROTOCOL_VERSION},
    server::DaemonServer,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

// A descriptor marked close-on-exec can still be inherited between fork and exec.
const START_LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonRegistration {
    pub pid: u32,
    /// Opaque OS-native birth timestamp (microseconds on macOS, ticks on Linux).
    pub process_start_time: u64,
    pub instance_id: String,
    pub protocol_version: u32,
    pub socket_path: PathBuf,
}

impl DaemonRegistration {
    pub fn current(socket_path: PathBuf) -> Result<Self> {
        let mut random = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        Ok(Self {
            pid: std::process::id(),
            process_start_time: process_start_time(std::process::id())?
                .context("current process missing")?,
            instance_id: random.iter().map(|b| format!("{b:02x}")).collect(),
            protocol_version: PROTOCOL_VERSION,
            socket_path,
        })
    }

    pub fn read(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16 * 1024, "registration too large");
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn publish(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .context("registration requires parent directory")?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        serde_json::to_writer(&mut file, self)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }

    pub fn is_live(&self) -> Result<bool> {
        Ok(process_start_time(self.pid)? == Some(self.process_start_time))
    }
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> Result<Option<u64>> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Ok(None);
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: the kernel writes at most size bytes into a properly sized buffer.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read == size {
        // SAFETY: proc_pidinfo initialized the entire structure on success.
        let info = unsafe { info.assume_init() };
        return Ok(Some(
            info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        ));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(None);
    }
    Err(error.into())
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Result<Option<u64>> {
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // comm may contain spaces and parentheses; field 3 follows the final ')'.
    let fields = stat.rsplit_once(')').context("invalid process stat")?.1;
    Ok(Some(
        fields
            .split_whitespace()
            .nth(19)
            .context("missing process start time")?
            .parse()?,
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_start_time(_pid: u32) -> Result<Option<u64>> {
    bail!("daemon process identity is supported on macOS and Linux")
}

#[derive(Clone)]
pub struct DaemonLifecycle {
    directory: PathBuf,
    pub health_timeout: Duration,
}

impl DaemonLifecycle {
    /// `config_directory` must be the same explicit config root in parent and child.
    pub fn new(config_directory: impl AsRef<Path>) -> Self {
        Self {
            directory: config_directory.as_ref().join("daemon"),
            health_timeout: Duration::from_secs(10),
        }
    }
    pub fn socket_path(&self) -> PathBuf {
        self.directory.join("control.sock")
    }
    pub fn registration_path(&self) -> PathBuf {
        self.directory.join("registration.json")
    }
    pub fn database_path(&self) -> PathBuf {
        self.directory.join("jobs.sqlite")
    }
    pub fn log_path(&self) -> PathBuf {
        self.directory.join("daemon.log")
    }

    pub fn read_log_tail(&self, lines: usize) -> Result<String> {
        const MAX_LOG_BYTES: u64 = 64 * 1024;
        let mut file = File::open(self.log_path())?;
        let length = file.metadata()?.len();
        file.seek(SeekFrom::Start(length.saturating_sub(MAX_LOG_BYTES)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let mut contents = String::from_utf8_lossy(&bytes).into_owned();
        if length > MAX_LOG_BYTES
            && let Some(newline) = contents.find('\n')
        {
            contents.drain(..=newline);
        }
        Ok(super::command::tail_lines(&contents, lines))
    }

    fn lock(&self, name: &str) -> Result<File> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.directory)?;
        let metadata = fs::symlink_metadata(&self.directory)?;
        ensure!(
            metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
            "daemon directory must be private (0700)"
        );
        let lock_path = self.directory.join(name);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)?;
        // SAFETY: file owns a valid descriptor; the lock is released on drop.
        loop {
            // SAFETY: file owns a valid descriptor.
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                break;
            }

            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(error)
                    .with_context(|| format!("daemon lock {} is busy", lock_path.display()));
            }
            return Err(error)
                .with_context(|| format!("failed to acquire daemon lock {}", lock_path.display()));
        }
        Ok(file)
    }

    async fn start_lock(&self) -> Result<File> {
        let deadline = tokio::time::Instant::now() + START_LOCK_WAIT_TIMEOUT;
        loop {
            match self.lock("start.lock") {
                Ok(lock) => return Ok(lock),
                Err(error) if is_lock_busy(&error) => {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        return Err(error);
                    }
                    tokio::time::sleep((deadline - now).min(Duration::from_millis(10))).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn registration(&self) -> Result<Option<DaemonRegistration>> {
        match DaemonRegistration::read(&self.registration_path()) {
            Ok(registration) => Ok(Some(registration)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub fn bind(&self) -> Result<DaemonServer> {
        let lock = self.lock("owner.lock")?;
        if let Some(registration) = self.registration()? {
            ensure!(
                !registration.is_live()?,
                "registered daemon process is still alive"
            );
        }
        remove_if_exists(&self.registration_path())?;
        remove_if_exists(&self.socket_path())?;
        DaemonServer::bind(self.clone(), lock)
    }

    pub async fn run(&self) -> Result<()> {
        let mut server = self.bind()?;
        server.install_executor(
            std::sync::Arc::new(super::executor::ActionExecutor::default()),
            4,
        );
        server.run().await
    }

    pub async fn status(&self) -> Result<Option<DaemonStatus>> {
        let Some(registration) = self.registration()? else {
            return Ok(None);
        };
        if !registration.is_live()? {
            let _lock = self.lock("owner.lock")?;
            if self.registration()?.as_ref() == Some(&registration) {
                remove_if_exists(&self.registration_path())?;
                remove_if_exists(&self.socket_path())?;
            }
            return Ok(None);
        }
        ensure!(
            registration.protocol_version == PROTOCOL_VERSION,
            "unsupported daemon protocol"
        );
        ensure!(
            registration.socket_path == self.socket_path(),
            "unexpected registered socket path"
        );
        match DaemonClient::new(self.socket_path())
            .request(DaemonRequest::Status)
            .await?
        {
            DaemonResponse::Status { status } => {
                ensure!(
                    status.pid == registration.pid
                        && status.process_start_time == registration.process_start_time
                        && status.instance_id == registration.instance_id,
                    "daemon identity mismatch"
                );
                Ok(Some(status))
            }
            response => bail!("unexpected health response: {response:?}"),
        }
    }

    pub async fn stop(&self) -> Result<Option<DaemonStatus>> {
        let Some(status) = self.status().await? else {
            return Ok(None);
        };
        ensure!(
            process_start_time(status.pid)? == Some(status.process_start_time),
            "daemon identity changed before stop"
        );
        let response = DaemonClient::new(self.socket_path())
            .request(DaemonRequest::Shutdown {
                instance_id: status.instance_id.clone(),
            })
            .await?;
        ensure!(
            matches!(response, DaemonResponse::Ack),
            "daemon rejected shutdown: {response:?}"
        );
        tokio::time::timeout(self.health_timeout, async {
            while self
                .registration()?
                .is_some_and(|r| r.instance_id == status.instance_id)
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("daemon did not stop in time")??;
        Ok(Some(status))
    }

    /// Spawn an explicitly constructed foreground command (future CLI: `daemon run`).
    /// No shell interpolation. The caller supplies config-root arguments/environment.
    pub async fn start(&self, mut command: tokio::process::Command) -> Result<DaemonStatus> {
        let _start_lock = self.start_lock().await?;
        if let Some(status) = self.status().await? {
            return Ok(status);
        }
        let log = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.log_path())?;
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        // SAFETY: setsid is async-signal-safe and the callback accesses no shared state.
        unsafe {
            command.as_std_mut().pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = StartingChild(Some(command.spawn()?));
        let pid = child.0.as_ref().unwrap().id().context("child exited")?;
        let status = tokio::time::timeout(self.health_timeout, async {
            loop {
                if let Some(exit) = child.0.as_mut().unwrap().try_wait()? {
                    bail!("daemon exited before health check: {exit}");
                }
                if let Ok(Some(status)) = self.status().await {
                    ensure!(status.pid == pid, "another daemon won startup");
                    return Ok(status);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .context("daemon did not become healthy")??;
        // Tokio reaps dropped children; only a failed/cancelled start kills its child.
        child.0.take();
        Ok(status)
    }
}

fn is_lock_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
    })
}

struct StartingChild(Option<tokio::process::Child>);
impl Drop for StartingChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.start_kill();
        }
    }
}

pub(crate) fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{client::DaemonClient, protocol::DaemonRequest};
    use std::os::{fd::RawFd, unix::fs::PermissionsExt};

    fn fork_waiting_with_lock(lock_fd: RawFd) -> (libc::pid_t, libc::c_int) {
        let mut release_pipe = [0; 2];
        // SAFETY: release_pipe points to two writable integers.
        assert_eq!(unsafe { libc::pipe(release_pipe.as_mut_ptr()) }, 0);

        // SAFETY: the child only uses async-signal-safe close/read/_exit calls.
        let child = unsafe { libc::fork() };
        assert!(
            child >= 0,
            "fork failed: {}",
            std::io::Error::last_os_error()
        );
        if child == 0 {
            // SAFETY: these are valid descriptors inherited from the parent.
            unsafe {
                libc::close(release_pipe[1]);
                let mut release = 0u8;
                let read_result = libc::read(release_pipe[0], (&mut release as *mut u8).cast(), 1);
                libc::close(lock_fd);
                libc::close(release_pipe[0]);
                libc::_exit(if read_result == 1 && release == 1 {
                    0
                } else {
                    1
                });
            }
        }

        // SAFETY: the parent closes its copy of the pipe's read end.
        unsafe { libc::close(release_pipe[0]) };
        (child, release_pipe[1])
    }

    fn release_forked_lock(child: libc::pid_t, release_fd: libc::c_int) {
        // SAFETY: release the child and reap it before assertions can unwind.
        unsafe {
            assert_eq!(libc::write(release_fd, [1u8].as_ptr().cast(), 1), 1);
            libc::close(release_fd);
            let mut status = 0;
            assert_eq!(libc::waitpid(child, &mut status, 0), child);
            assert!(libc::WIFEXITED(status));
            assert_eq!(libc::WEXITSTATUS(status), 0);
        }
    }

    #[tokio::test]
    async fn live_status_stop_and_exclusive_ownership() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = DaemonLifecycle::new(dir.path());
        let server = lifecycle.bind().unwrap();
        assert_eq!(
            std::fs::metadata(lifecycle.socket_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(lifecycle.bind().is_err());
        let task = tokio::spawn(server.run());
        let status = lifecycle.status().await.unwrap().unwrap();
        assert_eq!(status.pid, std::process::id());
        assert!(status.process_start_time > 0);
        lifecycle.stop().await.unwrap();
        task.await.unwrap().unwrap();
        assert!(!lifecycle.socket_path().exists());
        assert!(!lifecycle.registration_path().exists());
        assert!(lifecycle.status().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn stale_pid_identity_cannot_stop_process_and_is_reclaimed() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = DaemonLifecycle::new(dir.path());
        let mut registration = DaemonRegistration::current(lifecycle.socket_path()).unwrap();
        registration.process_start_time += 1;
        registration
            .publish(&lifecycle.registration_path())
            .unwrap();
        assert!(!registration.is_live().unwrap());
        assert!(lifecycle.stop().await.unwrap().is_none());
        assert!(!lifecycle.registration_path().exists());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match lifecycle.bind() {
                Ok(server) => {
                    drop(server);
                    break;
                }
                Err(error) if is_lock_busy(&error) => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "owner lock not released after stale registration cleanup: {error:#}"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("unexpected bind error: {error:#}"),
            }
        }
        assert!(!lifecycle.socket_path().exists());
    }

    #[test]
    fn publication_replaces_complete_json_with_private_permissions() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let path = dir.path().join("registration.json");
        let first = DaemonRegistration::current(dir.path().join("daemon.sock")).unwrap();
        first.publish(&path).unwrap();
        let second = DaemonRegistration::current(first.socket_path.clone()).unwrap();
        second.publish(&path).unwrap();
        assert_ne!(first.instance_id, second.instance_id);
        assert_eq!(DaemonRegistration::read(&path).unwrap(), second);
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn busy_lock_error_names_the_lock_file() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = DaemonLifecycle::new(dir.path());
        let _held_lock = lifecycle.lock("start.lock").unwrap();

        let error = lifecycle.lock("start.lock").unwrap_err();
        assert!(error.to_string().contains("start.lock"), "{error:#}");
        assert!(error.to_string().contains("busy"), "{error:#}");
    }

    #[test]
    fn inherited_lock_descriptor_keeps_start_lock_busy_until_child_closes_it() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = DaemonLifecycle::new(dir.path());
        let held_lock = lifecycle.lock("start.lock").unwrap();
        let (child, release_fd) = fork_waiting_with_lock(held_lock.as_raw_fd());
        drop(held_lock);
        let busy = lifecycle.lock("start.lock").unwrap_err();

        release_forked_lock(child, release_fd);
        assert!(is_lock_busy(&busy));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match lifecycle.lock("start.lock") {
                Ok(lock) => {
                    drop(lock);
                    break;
                }
                Err(error) if is_lock_busy(&error) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "inherited lock descriptors did not close: {error:#}"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("unexpected lock error: {error:#}"),
            }
        }
    }

    #[tokio::test]
    async fn start_retries_transient_inherited_start_lock_contention() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut lifecycle = DaemonLifecycle::new(dir.path());
        lifecycle.health_timeout = Duration::from_millis(60);
        let held_lock = lifecycle.lock("start.lock").unwrap();
        let (child, release_fd) = fork_waiting_with_lock(held_lock.as_raw_fd());
        drop(held_lock);
        let start = tokio::spawn(async move {
            let mut command = tokio::process::Command::new("/usr/bin/tail");
            command.args(["-f", "/dev/null"]);
            lifecycle.start(command).await
        });

        tokio::time::sleep(Duration::from_millis(40)).await;
        release_forked_lock(child, release_fd);

        let error = start.await.unwrap().unwrap_err();
        assert!(
            error.to_string().contains("did not become healthy"),
            "unexpected startup error: {error:#}"
        );
    }

    #[tokio::test]
    async fn absent_socket_client_fails_without_starting_daemon() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let client = DaemonClient::new(dir.path().join("absent.sock"));
        assert!(client.request(DaemonRequest::Status).await.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    // A real detached process running the production foreground path, without
    // needing Task 6's not-yet-implemented application CLI routing.
    #[test]
    fn foreground_child() {
        let Some(root) = std::env::var_os("RUSTCODE_DAEMON_TEST_ROOT") else {
            return;
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(DaemonLifecycle::new(root).run()).unwrap();
    }

    #[tokio::test]
    async fn detached_start_waits_for_health_and_stop_cleans_up() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = DaemonLifecycle::new(dir.path());
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "daemon::lifecycle::tests::foreground_child",
                "--nocapture",
            ])
            .env("RUSTCODE_DAEMON_TEST_ROOT", dir.path());
        let status = lifecycle.start(command).await.unwrap();
        // Always stop before assertions, including when the health payload is wrong.
        let stopped = lifecycle.stop().await.unwrap().unwrap();
        assert_eq!(status.instance_id, stopped.instance_id);
        assert_ne!(status.pid, std::process::id());
        assert!(!lifecycle.socket_path().exists());
        assert!(!lifecycle.registration_path().exists());
    }

    #[tokio::test]
    async fn detached_start_reports_early_exit_and_health_timeout() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut lifecycle = DaemonLifecycle::new(dir.path());
        assert!(
            lifecycle
                .start(tokio::process::Command::new("/usr/bin/false"))
                .await
                .unwrap_err()
                .to_string()
                .contains("exited before health")
        );
        lifecycle.health_timeout = Duration::from_millis(60);
        // The child must remain alive until start's health deadline wins, even
        // if this test is descheduled longer than a short sleep would last.
        let mut command = tokio::process::Command::new("/usr/bin/tail");
        command.args(["-f", "/dev/null"]);
        let error = lifecycle.start(command).await.unwrap_err();
        assert!(
            error.to_string().contains("did not become healthy"),
            "unexpected startup error: {error:#}"
        );
        assert!(lifecycle.status().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn aborted_foreground_task_releases_socket_and_registration() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = DaemonLifecycle::new(dir.path());
        let server = lifecycle.bind().unwrap();
        let task = tokio::spawn(server.run());
        // A scheduler yield does not guarantee the server has started polling
        // its listener. A successful status request proves it is serving.
        assert!(lifecycle.status().await.unwrap().is_some());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!lifecycle.socket_path().exists());
        assert!(!lifecycle.registration_path().exists());
        // The aborted task releases its ownership lock asynchronously; a
        // single yield is not enough on a loaded runner, so poll with a
        // deadline instead of assuming one turn suffices.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match lifecycle.bind() {
                Ok(server) => {
                    drop(server);
                    break;
                }
                Err(error) => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "ownership lock not released after abort: {error:#}"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}
