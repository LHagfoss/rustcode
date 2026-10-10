//! Single-instance ownership and private registration of the gateway.
//!
//! Same safeguards as the scheduler daemon, through its shared helpers: an
//! advisory lock held for the whole foreground lifetime, an owner-only
//! registration checked against the process birth time, and stop by
//! authenticated instance identity over the control socket, never by PID
//! signal. The gateway keeps its own directory and registration, separate
//! from scheduled jobs.

use super::control::{self, ControlRequest, ControlResponse, GatewayStatus, OfferDetails};
use super::devices::{DeviceRecord, DeviceStore};
use crate::daemon::lifecycle::{
    ensure_private_directory, is_lock_busy, lock_private_file, process_start_time,
    publish_private_json, random_instance_id, read_private_json, remove_if_exists,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

const OWNER: &str = "remote gateway";

/// Longest socket path that fits `sockaddr_un` on every supported system
/// (104 bytes on macOS, terminator included).
const MAX_SOCKET_PATH_BYTES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayRegistration {
    #[serde(default)]
    pub version: String,
    pub pid: u32,
    /// Opaque OS-native birth timestamp; see the daemon registration.
    pub process_start_time: u64,
    pub instance_id: String,
    pub gateway_id: String,
    pub control_protocol_version: u32,
    pub socket_path: PathBuf,
    /// Where session owners connect. Absent in a registration written by a
    /// gateway without session routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_socket_path: Option<PathBuf>,
    pub listen_address: String,
    pub advertised_address: String,
}

impl GatewayRegistration {
    pub(super) fn current(
        socket_path: PathBuf,
        owner_socket_path: PathBuf,
        gateway_id: String,
        listen_address: String,
        advertised_address: String,
    ) -> Result<Self> {
        Ok(Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            process_start_time: process_start_time(std::process::id())?
                .context("current process missing")?,
            instance_id: random_instance_id()?,
            gateway_id,
            control_protocol_version: control::CONTROL_PROTOCOL_VERSION,
            socket_path,
            owner_socket_path: Some(owner_socket_path),
            listen_address,
            advertised_address,
        })
    }

    pub fn read(path: &Path) -> Result<Self> {
        read_private_json(path, 16 * 1024)
    }

    pub(super) fn publish(&self, path: &Path) -> Result<()> {
        publish_private_json(path, self)
    }

    pub fn is_live(&self) -> Result<bool> {
        Ok(process_start_time(self.pid)? == Some(self.process_start_time))
    }
}

/// Stable identity of this host's gateway, created on first use.
#[derive(Serialize, Deserialize)]
struct GatewayIdentity {
    gateway_id: String,
}

/// What `revoke` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    pub device_id: String,
    pub device_name: String,
    /// Live connections of that device closed by the running gateway.
    pub closed_connections: usize,
    /// Whether a running gateway performed the revocation.
    pub gateway_running: bool,
}

#[derive(Clone)]
pub struct RemoteLifecycle {
    directory: PathBuf,
    pub health_timeout: Duration,
}

impl RemoteLifecycle {
    /// `config_directory` is the RustCode config root.
    pub fn new(config_directory: impl AsRef<Path>) -> Self {
        Self {
            directory: config_directory.as_ref().join("remote"),
            health_timeout: Duration::from_secs(10),
        }
    }
    /// The configuration root this gateway belongs to.
    pub fn config_directory(&self) -> PathBuf {
        self.directory
            .parent()
            .map_or_else(|| self.directory.clone(), Path::to_path_buf)
    }

    /// Directory of the two sockets: the gateway directory, unless a socket
    /// path under it would not fit in a socket address. A long configuration
    /// path (a deep `RUSTCODE_CONFIG_DIR`, a long home directory) then gets a
    /// short directory under `/tmp` that belongs to this user and this
    /// configuration directory. It is created 0700 and verified to be ours
    /// before anything binds in it; the registration records the paths.
    fn socket_directory(&self) -> PathBuf {
        let longest = self.directory.join("control.sock");
        if longest.as_os_str().as_bytes().len() < MAX_SOCKET_PATH_BYTES {
            return self.directory.clone();
        }
        let digest = Sha256::digest(self.directory.as_os_str().as_bytes());
        // SAFETY: geteuid has no preconditions.
        let user = unsafe { libc::geteuid() };
        PathBuf::from(format!("/tmp/rustcode-{user}")).join(hex::encode(&digest[..8]))
    }

    /// Create the socket directory if it is the fallback one, and refuse it
    /// unless it is a private directory this user owns.
    pub(super) fn prepare_socket_directory(&self) -> Result<()> {
        let directory = self.socket_directory();
        if directory == self.directory {
            return Ok(());
        }
        // SAFETY: geteuid has no preconditions.
        let user = unsafe { libc::geteuid() };
        for path in [
            directory
                .parent()
                .context("socket directory has a parent")?,
            &directory,
        ] {
            ensure_private_directory(path, OWNER)?;
            ensure!(
                std::fs::symlink_metadata(path)?.uid() == user,
                "{} is not owned by this user",
                path.display()
            );
        }
        Ok(())
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket_directory().join("control.sock")
    }
    /// The socket session owners register on; see [`super::owner_ipc`].
    pub fn owner_socket_path(&self) -> PathBuf {
        self.socket_directory().join("owner.sock")
    }
    /// Output of a gateway that `/remote` started in the background.
    pub fn log_path(&self) -> PathBuf {
        self.directory.join("gateway.log")
    }
    pub fn registration_path(&self) -> PathBuf {
        self.directory.join("registration.json")
    }
    pub fn devices_path(&self) -> PathBuf {
        self.directory.join("devices.json")
    }
    pub fn identity_path(&self) -> PathBuf {
        self.directory.join("gateway.json")
    }
    pub fn device_store(&self) -> DeviceStore {
        DeviceStore::new(self.devices_path())
    }

    /// The single-instance lock. Whoever holds it is the only writer of the
    /// device store: the running gateway, or an offline `revoke`.
    pub(super) fn owner_lock(&self) -> Result<File> {
        lock_private_file(&self.directory, "owner.lock", OWNER)
    }

    /// Read the gateway identity, creating it under the owner lock.
    pub(super) fn gateway_id(&self, _owner_lock: &File) -> Result<String> {
        let path = self.identity_path();
        match read_private_json::<GatewayIdentity>(&path, 4 * 1024) {
            Ok(identity) if !identity.gateway_id.is_empty() => return Ok(identity.gateway_id),
            Ok(_) => bail!("gateway identity {} is empty", path.display()),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to read gateway identity {}", path.display())
                });
            }
        }
        let identity = GatewayIdentity {
            gateway_id: random_instance_id()?,
        };
        publish_private_json(&path, &identity)?;
        Ok(identity.gateway_id)
    }

    pub(super) fn registration(&self) -> Result<Option<GatewayRegistration>> {
        match GatewayRegistration::read(&self.registration_path()) {
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

    /// Take ownership for a new gateway, reclaiming a dead predecessor's files.
    pub(super) fn claim(&self) -> Result<File> {
        let lock = self
            .owner_lock()
            .context("another remote gateway is already running")?;
        if let Some(registration) = self.registration()? {
            ensure!(
                !registration.is_live()?,
                "registered remote gateway process is still alive"
            );
        }
        remove_if_exists(&self.registration_path())?;
        self.prepare_socket_directory()?;
        remove_if_exists(&self.socket_path())?;
        remove_if_exists(&self.owner_socket_path())?;
        Ok(lock)
    }

    /// Start a gateway in the background and wait until it answers, unless
    /// one is already running. `command` is the complete foreground command
    /// (`rustcode remote serve …`); nothing goes through a shell. The child
    /// gets its own session, so it outlives the terminal that started it,
    /// and writes to [`RemoteLifecycle::log_path`].
    pub async fn start(&self, mut command: tokio::process::Command) -> Result<GatewayStatus> {
        // Upgrade a stale gateway through its authenticated control socket. Pairing
        // credentials and owner processes survive; they reconnect to the successor.
        if let Some(status) = self.status().await? {
            if !gateway_needs_upgrade(&status.version, env!("CARGO_PKG_VERSION")) {
                return Ok(status);
            }
            self.stop().await?;
        }
        ensure_private_directory(&self.directory, OWNER)?;
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
        let mut child = command.spawn().context("failed to start the gateway")?;
        let started = tokio::time::timeout(self.health_timeout, async {
            loop {
                // Two terminals may start one at the same moment: the owner
                // lock picks the winner, and both find it running.
                if let Ok(Some(status)) = self.status().await {
                    return Ok(status);
                }
                if let Some(exit) = child.try_wait()? {
                    bail!(
                        "the gateway exited at once ({exit}); see {}",
                        self.log_path().display()
                    );
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        match started {
            Ok(result) => result,
            Err(_) => {
                let _ = child.start_kill();
                bail!(
                    "the gateway did not start in time; see {}",
                    self.log_path().display()
                )
            }
        }
    }

    /// Status of the running gateway, or `None` when none is running. A dead
    /// gateway's registration is cleaned up on the way.
    pub async fn status(&self) -> Result<Option<GatewayStatus>> {
        let Some(registration) = self.registration()? else {
            return Ok(None);
        };
        if !registration.is_live()? {
            let _lock = self.owner_lock()?;
            if self.registration()?.as_ref() == Some(&registration) {
                remove_if_exists(&self.registration_path())?;
                remove_if_exists(&self.socket_path())?;
                remove_if_exists(&self.owner_socket_path())?;
            }
            return Ok(None);
        }
        ensure!(
            registration.control_protocol_version == control::CONTROL_PROTOCOL_VERSION,
            "unsupported remote gateway control protocol"
        );
        ensure!(
            registration.socket_path == self.socket_path(),
            "unexpected registered socket path"
        );
        match control::request(&self.socket_path(), &ControlRequest::Status).await? {
            ControlResponse::Status { status } => {
                ensure!(
                    status.pid == registration.pid
                        && status.process_start_time == registration.process_start_time
                        && status.instance_id == registration.instance_id,
                    "remote gateway identity mismatch"
                );
                Ok(Some(status))
            }
            response => bail!("unexpected status response: {response:?}"),
        }
    }

    /// Stop the running gateway and wait until its registration is gone.
    pub async fn stop(&self) -> Result<Option<GatewayStatus>> {
        let Some(status) = self.status().await? else {
            return Ok(None);
        };
        let response = control::request(
            &self.socket_path(),
            &ControlRequest::Shutdown {
                instance_id: status.instance_id.clone(),
            },
        )
        .await?;
        ensure!(
            matches!(response, ControlResponse::Ack),
            "remote gateway rejected shutdown: {response:?}"
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
        .context("remote gateway did not stop in time")??;
        Ok(Some(status))
    }

    /// Ask the running gateway for a fresh pairing challenge.
    pub async fn pair(&self) -> Result<OfferDetails> {
        ensure!(
            self.status().await?.is_some(),
            "the remote gateway is not running; start it with `rustcode remote serve`"
        );
        match control::request(&self.socket_path(), &ControlRequest::Pair).await? {
            ControlResponse::Offer { offer } => Ok(offer),
            ControlResponse::Error { message } => bail!(message),
            response => bail!("unexpected pairing response: {response:?}"),
        }
    }

    /// Paired devices as stored on disk. Never includes a token.
    pub fn devices(&self) -> Result<Vec<DeviceRecord>> {
        Ok(self.device_store().load()?.list().to_vec())
    }

    /// Revoke one device.
    ///
    /// The owner lock decides who writes. If this call gets it, no gateway is
    /// running (and none can start meanwhile), so the store is edited here.
    /// If it is held, the running gateway revokes: it alone can close the
    /// device's live connections in the same step as forgetting its token.
    pub async fn revoke(&self, selector: &str) -> Result<Revocation> {
        match self.owner_lock() {
            Ok(_lock) => {
                let store = self.device_store();
                let mut devices = store.load()?;
                let record = devices.select(selector)?.clone();
                devices.remove(&record.id);
                store.save(&devices)?;
                Ok(Revocation {
                    device_id: record.id,
                    device_name: record.name,
                    closed_connections: 0,
                    gateway_running: false,
                })
            }
            Err(error) if is_lock_busy(&error) => {
                let response = control::request(
                    &self.socket_path(),
                    &ControlRequest::Revoke {
                        device: selector.to_string(),
                    },
                )
                .await
                .context(
                    "the remote gateway holds the device store but did not answer; nothing was revoked",
                )?;
                match response {
                    ControlResponse::Revoked {
                        device_id,
                        device_name,
                        closed_connections,
                    } => Ok(Revocation {
                        device_id,
                        device_name,
                        closed_connections,
                        gateway_running: true,
                    }),
                    ControlResponse::Error { message } => bail!(message),
                    response => bail!("unexpected revoke response: {response:?}"),
                }
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn gateway_identity_is_created_once_and_private() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let lock = lifecycle.owner_lock().unwrap();
        let first = lifecycle.gateway_id(&lock).unwrap();
        assert_eq!(first.len(), 32);
        assert_eq!(lifecycle.gateway_id(&lock).unwrap(), first);
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(lifecycle.identity_path()), 0o600);
        assert_eq!(mode(dir.path().join("remote")), 0o700);
    }

    #[test]
    fn a_second_owner_is_refused_while_the_lock_is_held() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let held = lifecycle.claim().unwrap();
        let error = lifecycle.claim().unwrap_err();
        assert!(is_lock_busy(&error), "{error:#}");
        assert!(
            format!("{error:#}").contains("already running"),
            "{error:#}"
        );
        drop(held);
        // A child forked by a concurrent test can hold an inherited copy of
        // the descriptor until it execs, so poll instead of assuming one try.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Err(error) = lifecycle.claim() {
            assert!(is_lock_busy(&error), "{error:#}");
            assert!(
                std::time::Instant::now() < deadline,
                "owner lock not released: {error:#}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_long_configuration_path_gets_short_private_socket_paths() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let deep = dir.path().join("a".repeat(60)).join("b".repeat(60));
        let lifecycle = RemoteLifecycle::new(&deep);
        let _lock = lifecycle.claim().unwrap();
        for socket in [lifecycle.socket_path(), lifecycle.owner_socket_path()] {
            assert!(
                socket.as_os_str().len() < MAX_SOCKET_PATH_BYTES,
                "{}",
                socket.display()
            );
            assert!(!socket.starts_with(&deep));
            let directory = socket.parent().unwrap();
            let metadata = std::fs::symlink_metadata(directory).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
            // SAFETY: geteuid has no preconditions.
            assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
            std::os::unix::net::UnixListener::bind(&socket).expect("the path fits a socket");
        }
        // Another configuration directory never shares the fallback.
        let other = RemoteLifecycle::new(dir.path().join("c".repeat(120)));
        assert_ne!(other.socket_path(), lifecycle.socket_path());
        let _ = std::fs::remove_dir_all(lifecycle.socket_path().parent().unwrap());

        // A short path keeps its sockets next to the registration.
        let short = RemoteLifecycle::new(dir.path());
        assert_eq!(
            short.socket_path(),
            dir.path().join("remote").join("control.sock")
        );
    }

    #[tokio::test]
    async fn offline_revoke_edits_the_store_and_reports_no_gateway() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let store = lifecycle.device_store();
        let mut devices = store.load().unwrap();
        let (kept, _) = devices.enroll("tablet").unwrap();
        let (gone, token) = devices.enroll("phone").unwrap();
        store.save(&devices).unwrap();

        let revoked = lifecycle.revoke("phone").await.unwrap();
        assert_eq!(revoked.device_id, gone.id);
        assert!(!revoked.gateway_running);
        let listed = lifecycle.devices().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, kept.id);
        assert!(
            store
                .load()
                .unwrap()
                .verify(&gone.id, token.expose())
                .is_none()
        );
        assert!(lifecycle.revoke("phone").await.is_err());
    }

    #[tokio::test]
    async fn revoke_fails_closed_when_the_owner_does_not_answer() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let store = lifecycle.device_store();
        let mut devices = store.load().unwrap();
        devices.enroll("phone").unwrap();
        store.save(&devices).unwrap();
        // The lock is held but nothing listens on the control socket.
        let _held = lifecycle.owner_lock().unwrap();
        let error = lifecycle.revoke("phone").await.unwrap_err();
        assert!(
            format!("{error:#}").contains("nothing was revoked"),
            "{error:#}"
        );
        assert_eq!(lifecycle.devices().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stale_registration_is_reclaimed() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let mut registration = GatewayRegistration::current(
            lifecycle.socket_path(),
            lifecycle.owner_socket_path(),
            "gw".into(),
            "127.0.0.1:1".into(),
            "127.0.0.1:1".into(),
        )
        .unwrap();
        registration.process_start_time += 1;
        registration
            .publish(&lifecycle.registration_path())
            .unwrap();
        assert!(lifecycle.status().await.unwrap().is_none());
        assert!(!lifecycle.registration_path().exists());
        assert!(lifecycle.stop().await.unwrap().is_none());
        assert!(lifecycle.pair().await.is_err());
    }
}

/// Never let an older session owner downgrade a newer installed gateway.
pub(super) fn gateway_needs_upgrade(running: &str, current: &str) -> bool {
    fn version(value: &str) -> Option<Vec<u64>> {
        value
            .split('.')
            .map(str::parse)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()
    }
    if running.is_empty() {
        return true;
    }
    matches!((version(running), version(current)), (Some(old),Some(new)) if old < new)
}
