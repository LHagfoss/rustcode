//! Owner-only files, advisory locks and process identity, shared by the
//! scheduler daemon and the remote gateway.
//!
//! On Unix every file is created 0600 in a 0700 directory and opened without
//! following a final symlink. Windows has no mode bits: the files live in the
//! user's profile, whose inherited ACL already admits only that user (and the
//! system and administrators), the same protection `credentials.json` has.
use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

/// Random identity for one process lifetime, shared with the remote gateway.
pub(crate) fn random_instance_id() -> Result<String> {
    let random: [u8; 16] = rand::random();
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}

/// Read a bounded JSON file written by [`publish_private_json`].
pub(crate) fn read_private_json<T: DeserializeOwned>(path: &Path, max_bytes: u64) -> Result<T> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= max_bytes, "private file too large");
    Ok(serde_json::from_slice(&bytes)?)
}

/// Replace `path` with complete owner-only JSON. The temporary file is created
/// 0600 in the same directory, so the destination never exists with wider
/// permissions or partial contents.
pub(crate) fn publish_private_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .context("registration requires parent directory")?;
    create_private_directory(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut file, value)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    // A directory cannot be opened as a file on Windows; the rename is
    // durable there once the file itself was synced.
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn create_private_directory(directory: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(directory)
}

/// Create `directory` 0700 if missing and refuse one that is not a private
/// directory (a symlink, or readable by group/other on Unix).
pub(crate) fn ensure_private_directory(directory: &Path, owner: &str) -> Result<()> {
    create_private_directory(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && owner_only(&metadata),
        "{owner} directory must be private (0700)"
    );
    Ok(())
}

/// Whether neither group nor others have any access (Unix mode bits). On
/// Windows the profile's inherited ACL is what keeps others out, so there is
/// nothing to check here.
pub(crate) fn owner_only(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    return metadata.permissions().mode() & 0o077 == 0;
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

/// Options for a file only this user may read: created 0600 and never through
/// a symlink on Unix. The caller adds read, write, append, create, truncate.
pub(crate) fn private_open_options() -> OpenOptions {
    #[allow(unused_mut)]
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    options
}

/// Take a non-blocking exclusive advisory lock on `directory/name`, held until
/// the returned file is dropped. A held lock reports `WouldBlock`.
pub(crate) fn lock_private_file(directory: &Path, name: &str, owner: &str) -> Result<File> {
    ensure_private_directory(directory, owner)?;
    let lock_path = directory.join(name);
    let file = private_open_options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    match try_lock(&file) {
        Ok(()) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            Err(error).with_context(|| format!("{owner} lock {} is busy", lock_path.display()))
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to acquire {owner} lock {}", lock_path.display())),
    }
}

#[cfg(unix)]
fn try_lock(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        // SAFETY: file owns a valid descriptor; the lock is released on drop.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// `LockFileEx` through std: exclusive, failing at once when held, released
/// when the handle closes.
#[cfg(not(unix))]
fn try_lock(file: &File) -> std::io::Result<()> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(fs::TryLockError::WouldBlock) => Err(std::io::ErrorKind::WouldBlock.into()),
        Err(fs::TryLockError::Error(error)) => Err(error),
    }
}

/// Opaque OS-native birth timestamp of `pid`, `None` when it is not running.
/// A registration stores it with the pid, so a recycled pid is not mistaken
/// for the process that wrote it.
#[cfg(target_os = "macos")]
pub(crate) fn process_start_time(pid: u32) -> Result<Option<u64>> {
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
pub(crate) fn process_start_time(pid: u32) -> Result<Option<u64>> {
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

/// Seconds since the epoch, from the process's creation time.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn process_start_time(pid: u32) -> Result<Option<u64>> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    Ok(system.process(pid).map(|process| process.start_time()))
}

pub(crate) fn is_lock_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
    })
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

    #[test]
    fn a_held_lock_is_busy_until_dropped() {
        let root = tempfile::tempdir().unwrap();
        // Created 0700 by the lock itself.
        let directory = root.path().join("remote");
        let held = lock_private_file(&directory, "owner.lock", "test").unwrap();
        let busy = lock_private_file(&directory, "owner.lock", "test").unwrap_err();
        assert!(is_lock_busy(&busy), "{busy:#}");
        drop(held);
        lock_private_file(&directory, "owner.lock", "test").unwrap();
    }

    #[test]
    fn published_json_reads_back_and_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/value.json");
        publish_private_json(&path, &serde_json::json!({"port": 4242})).unwrap();
        let value: serde_json::Value = read_private_json(&path, 1024).unwrap();
        assert_eq!(value["port"], 4242);
        assert!(read_private_json::<serde_json::Value>(&path, 4).is_err());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn this_process_has_a_start_time_and_ids_differ() {
        assert!(process_start_time(std::process::id()).unwrap().is_some());
        let id = random_instance_id().unwrap();
        assert_eq!(id.len(), 32);
        assert_ne!(id, random_instance_id().unwrap());
    }
}
