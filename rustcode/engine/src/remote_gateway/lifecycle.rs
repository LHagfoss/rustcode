//! Single-instance ownership and private registration of the gateway.
//!
//! Same safeguards as the scheduler daemon, through its shared helpers: an
//! advisory lock held for the whole foreground lifetime, an owner-only
//! registration checked against the process birth time, and stop by
//! authenticated instance identity over the control socket, never by PID
//! signal. The gateway keeps its own directory and registration, separate
//! from scheduled jobs.

use super::control::{
    self, ConnectAnswer, ControlRequest, ControlResponse, GatewayStatus, OfferDetails,
};
use super::devices::{DeviceRecord, DeviceStore, RevocationNotes};
use super::local_socket::{CONTROL_ENDPOINT, OWNER_ENDPOINT};
use crate::daemon::private_files::{
    ensure_private_directory, is_lock_busy, lock_private_file, private_open_options,
    process_start_time, publish_private_json, random_instance_id, read_private_json,
    remove_if_exists,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

const OWNER: &str = "remote gateway";

/// Longest socket path that fits `sockaddr_un` on every supported system
/// (104 bytes on macOS, terminator included).
#[cfg(unix)]
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// Who started the gateway process.
///
/// One gateway runs at a time (the owner lock), so its owner is also the
/// answer to "is this machine still reachable when the app is closed": a
/// server-hosted gateway outlives the app. A gateway started by a terminal is
/// reported as neither, because no app or server will bring it back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayOwner {
    /// The RustCode server started it and starts it again if it stops.
    Server,
    /// The desktop app, or a terminal asking for the app's gateway.
    App,
    /// A `rustcode remote serve` someone ran by hand.
    #[default]
    Terminal,
}

impl GatewayOwner {
    /// Set on the gateway process's environment by its launcher
    /// ([`super::owner_client::Launcher`]).
    pub const ENV: &str = "RUSTCODE_REMOTE_OWNER";

    /// The owner the launcher of this process named, `Terminal` when it did
    /// not name one (`rustcode remote serve` from a shell).
    pub fn from_env() -> Self {
        match std::env::var(Self::ENV).ok().as_deref() {
            Some("server") => Self::Server,
            Some("app") => Self::App,
            _ => Self::Terminal,
        }
    }

    /// The words status output uses.
    pub fn label(self) -> &'static str {
        match self {
            Self::Server => "the RustCode server",
            Self::App => "the app",
            Self::Terminal => "a terminal",
        }
    }

    /// The value a launcher puts in [`GatewayOwner::ENV`].
    pub fn as_env(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::App => "app",
            Self::Terminal => "terminal",
        }
    }
}

/// How this machine's gateway started last time: the durable settings the
/// server needs to start it at login with no app running.
///
/// The listening address itself is never stored: an `auto` bind is a policy,
/// and the address it resolves to belongs to the machine of the moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewaySettings {
    /// `auto`, or the address `--bind` named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advertise: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefer_interface: Option<String>,
    /// Who started the gateway that wrote this.
    #[serde(default)]
    pub started_by: GatewayOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl GatewaySettings {
    /// The settings of a gateway this process is about to start.
    pub fn starting(
        bind: Option<String>,
        port: Option<u16>,
        advertise: Option<String>,
        prefer_interface: Option<String>,
        started_by: GatewayOwner,
    ) -> Self {
        Self {
            bind,
            port,
            advertise,
            prefer_interface,
            started_by,
            updated_at: Some(chrono::Utc::now()),
        }
    }
}

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
    /// Who started this gateway. Absent in a registration from before it was
    /// recorded; those report `Terminal`.
    #[serde(default)]
    pub started_by: GatewayOwner,
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
            started_by: GatewayOwner::from_env(),
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
    /// Windows endpoints are files of any path length, so they always stay
    /// in the gateway directory.
    fn socket_directory(&self) -> PathBuf {
        #[cfg(not(unix))]
        return self.directory.clone();
        #[cfg(unix)]
        self.unix_socket_directory()
    }

    #[cfg(unix)]
    fn unix_socket_directory(&self) -> PathBuf {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        let longest = self.directory.join(CONTROL_ENDPOINT);
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
        #[cfg(unix)]
        self.prepare_unix_socket_directory(&directory)?;
        Ok(())
    }

    #[cfg(unix)]
    fn prepare_unix_socket_directory(&self, directory: &Path) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        let user = unsafe { libc::geteuid() };
        for path in [
            directory
                .parent()
                .context("socket directory has a parent")?,
            directory,
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

    /// The control endpoint: a Unix socket, or on Windows the private file
    /// naming its loopback port ([`super::local_socket`]).
    pub fn socket_path(&self) -> PathBuf {
        self.socket_directory().join(CONTROL_ENDPOINT)
    }
    /// The socket session owners register on; see [`super::owner_ipc`].
    pub fn owner_socket_path(&self) -> PathBuf {
        self.socket_directory().join(OWNER_ENDPOINT)
    }
    /// Output of a gateway that `/remote` started in the background.
    pub fn log_path(&self) -> PathBuf {
        self.directory.join("gateway.log")
    }
    /// Lease and log of every session owner started from a device.
    pub fn owners_directory(&self) -> PathBuf {
        self.directory.join("owners")
    }
    /// Receipts of the sessions devices asked for.
    pub fn creations_directory(&self) -> PathBuf {
        self.directory.join("created")
    }
    pub fn registration_path(&self) -> PathBuf {
        self.directory.join("registration.json")
    }
    pub fn devices_path(&self) -> PathBuf {
        self.directory.join("devices.json")
    }
    /// Families a failed device-store write marked revoked.
    pub fn revocations_path(&self) -> PathBuf {
        self.directory.join("revocations.json")
    }
    pub fn revocation_notes(&self) -> RevocationNotes {
        RevocationNotes::new(self.revocations_path())
    }
    pub fn identity_path(&self) -> PathBuf {
        self.directory.join("gateway.json")
    }
    /// The gateway's durable settings, written whenever one starts.
    pub fn settings_path(&self) -> PathBuf {
        self.directory.join("settings.json")
    }

    /// The settings the last gateway started with, if it recorded any.
    pub fn settings(&self) -> Result<Option<GatewaySettings>> {
        match read_private_json::<GatewaySettings>(&self.settings_path(), 16 * 1024) {
            Ok(settings) => Ok(Some(settings)),
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

    /// Record how this machine's gateway is set up, so the RustCode server can
    /// start it at login with no app or terminal running.
    pub fn save_settings(&self, settings: &GatewaySettings) -> Result<()> {
        ensure_private_directory(&self.directory, OWNER)?;
        publish_private_json(&self.settings_path(), settings)
    }

    /// The version of the gateway this directory has, when its process is
    /// alive; `None` when no gateway runs.
    ///
    /// A supervisor polls this; [`RemoteLifecycle::status`] asks the gateway
    /// and enumerates this machine's addresses, which a poll every couple of
    /// seconds should not do.
    pub fn running_version(&self) -> Option<String> {
        self.registration()
            .ok()
            .flatten()
            .filter(|registration| registration.is_live().unwrap_or(false))
            .map(|registration| registration.version)
    }

    /// Whether a gateway process is alive, without asking it anything.
    pub fn running(&self) -> bool {
        self.running_version().is_some()
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
        let log = private_open_options()
            .append(true)
            .create(true)
            .open(self.log_path())?;
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        crate::platform::detach(command.as_std_mut());
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

    /// Grant or deny full ACP access under the same writer lock as revocation.
    pub async fn set_acp(&self, selector: &str, allow: bool) -> Result<()> {
        match self.owner_lock() {
            Ok(_lock) => {
                let store = self.device_store();
                let mut devices = store.load()?;
                devices.set_acp(selector, allow)?;
                store.save(&devices)
            }
            Err(error) if is_lock_busy(&error) => {
                ensure!(
                    self.status().await?.is_some(),
                    "gateway holds the device store but did not answer; grant unchanged"
                );
                match control::request(
                    &self.socket_path(),
                    &ControlRequest::SetAcp {
                        device: selector.to_owned(),
                        allow,
                    },
                )
                .await?
                {
                    ControlResponse::Ack => Ok(()),
                    ControlResponse::Error { message } => bail!(message),
                    response => bail!("unexpected ACP grant response: {response:?}"),
                }
            }
            Err(error) => Err(error),
        }
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

    /// Accept or deny a device's connection request on the running gateway.
    /// Accepting answers with the code to show; it replaces any outstanding
    /// pairing challenge.
    pub async fn answer_connect_request(&self, id: &str, accept: bool) -> Result<ConnectAnswer> {
        let request = ControlRequest::AnswerConnectRequest {
            id: id.to_owned(),
            accept,
        };
        match control::request(&self.socket_path(), &request)
            .await
            .context("the remote gateway did not answer")?
        {
            ControlResponse::ConnectAnswered { answer } => Ok(answer),
            ControlResponse::Error { message } => bail!(message),
            response => bail!("unexpected answer to a connection request: {response:?}"),
        }
    }
}

// Unix sockets, modes and owners.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

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
                .verify(&gone.id, token.expose(), chrono::Utc::now())
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

    #[test]
    fn the_gateway_settings_are_kept_for_the_next_start() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        assert_eq!(lifecycle.settings().unwrap(), None);
        // No gateway has run here yet, and no process claims to.
        assert!(!lifecycle.running());

        let settings = GatewaySettings::starting(
            Some("auto".into()),
            Some(17879),
            Some("studio.netbird.cloud".into()),
            Some("netbird".into()),
            GatewayOwner::Server,
        );
        lifecycle.save_settings(&settings).unwrap();
        let read = lifecycle
            .settings()
            .unwrap()
            .expect("settings were written");
        assert_eq!(read.bind.as_deref(), Some("auto"));
        assert_eq!(read.port, Some(17879));
        assert_eq!(read.advertise.as_deref(), Some("studio.netbird.cloud"));
        assert_eq!(read.prefer_interface.as_deref(), Some("netbird"));
        // Who started it is part of what is kept: `server status` reports it.
        assert_eq!(read.started_by, GatewayOwner::Server);

        // A registration whose process is gone never counts as a gateway.
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
        assert!(!lifecycle.running());
    }

    #[test]
    fn an_owner_survives_a_round_trip_and_older_registrations_still_read() {
        let owner = |text: &str| serde_json::from_str::<GatewayOwner>(text).unwrap();
        assert_eq!(owner("\"server\""), GatewayOwner::Server);
        assert_eq!(owner("\"app\""), GatewayOwner::App);
        assert_eq!(owner("\"terminal\""), GatewayOwner::Terminal);
        // A registration written before the owner was recorded reports the
        // gateway as a terminal's, which no app or server brings back.
        let old: GatewayRegistration = serde_json::from_value(serde_json::json!({
            "version": "1.0.0", "pid": 7, "process_start_time": 1,
            "instance_id": "i", "gateway_id": "g", "control_protocol_version": 1,
            "socket_path": "/tmp/c.sock", "listen_address": "127.0.0.1:1",
            "advertised_address": "127.0.0.1:1",
        }))
        .unwrap();
        assert_eq!(old.started_by, GatewayOwner::Terminal);
        assert_eq!(GatewayOwner::from_env(), GatewayOwner::Terminal);
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

#[cfg(test)]
mod upgrade_tests {
    use super::gateway_needs_upgrade;
    #[test]
    fn only_an_older_gateway_is_replaced() {
        assert!(gateway_needs_upgrade("", "0.64.4"));
        assert!(gateway_needs_upgrade("0.64.3", "0.64.4"));
        assert!(!gateway_needs_upgrade("0.64.4", "0.64.4"));
        assert!(!gateway_needs_upgrade("0.65.0", "0.64.4"));
        assert!(!gateway_needs_upgrade("dev", "0.64.4"));
    }
}
