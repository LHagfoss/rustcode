//! Owner-only files, advisory locks and process identity, shared by the
//! scheduler daemon and the remote gateway.
//!
//! On Unix every file is created 0600 in a 0700 directory and opened without
//! following a final symlink. Windows has no mode bits, so private directories
//! get a protected DACL that their files inherit. It names the current user's
//! SID, not only `OWNER RIGHTS`, so a file an elevated process created (which
//! Windows then owns by `BUILTIN\Administrators`) stays usable by the user.
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

/// Name the file an I/O error is about. Windows reports a denied open as a bare
/// `Access is denied. (os error 5)`; this adds the path and, on Windows, the
/// usual cause and fix. Errors that are not a denial pass through unchanged, so
/// callers that look for `NotFound` or `WouldBlock` keep working.
pub(crate) fn access_context(error: std::io::Error, action: &str, path: &Path) -> anyhow::Error {
    if error.kind() != std::io::ErrorKind::PermissionDenied {
        return error.into();
    }
    let hint = if cfg!(windows) {
        format!(
            "; a process running as administrator probably created it, so run `icacls \"{}\" /reset /T` from an elevated prompt",
            path.display()
        )
    } else {
        String::new()
    };
    anyhow::Error::new(error).context(format!(
        "access denied to {action} {}{hint}",
        path.display()
    ))
}

/// Open `path` for appending in a private directory, creating it owner-only.
pub(crate) fn open_private_log(path: &Path) -> Result<File> {
    open_private(path, OpenOptions::new().append(true).create(true))
        .map_err(|error| access_context(error, "open", path))
}

/// Open a private file and protect it before the caller writes any content.
/// Existing files are narrowed too; failures are returned rather than ignored.
pub(crate) fn open_private(path: &Path, options: &OpenOptions) -> std::io::Result<File> {
    open_private_impl(path, options, false)
}

pub(crate) fn create_private_new(path: &Path) -> std::io::Result<File> {
    open_private_impl(path, OpenOptions::new().write(true).create_new(true), true)
}

fn open_private_impl(path: &Path, options: &OpenOptions, exclusive: bool) -> std::io::Result<File> {
    let mut options = options.clone();
    #[cfg(windows)]
    {
        // Supply the DACL at creation: another user must never be able to
        // open even the empty file and retain a handle to subsequent writes.
        match private_windows_security(path, true) {
            Ok(()) => {
                options.create_new(false).create(false);
            }
            Err(error) => {
                let error = error
                    .downcast::<std::io::Error>()
                    .unwrap_or_else(std::io::Error::other);
                if exclusive || error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
    }
    let _ = exclusive;
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    protect_private_file(path, &file)?;
    Ok(file)
}

pub(crate) fn protect_private_file(path: &Path, file: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    #[cfg(windows)]
    set_private_windows_dacl(path).map_err(std::io::Error::other)?;
    let _ = (path, file);
    Ok(())
}

/// A temporary file is secured before receiving any sensitive bytes.
pub(crate) fn private_tempfile(directory: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    tempfile::Builder::new().make_in(directory, create_private_new)
}

/// Read a bounded JSON file written by [`publish_private_json`].
pub(crate) fn read_private_json<T: DeserializeOwned>(path: &Path, max_bytes: u64) -> Result<T> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| access_context(error, "open", path))?
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
    #[cfg(windows)]
    ensure_private_directory(parent, "private file")?;
    #[cfg(not(windows))]
    create_private_directory(parent).map_err(|error| access_context(error, "create", parent))?;
    let mut file = private_tempfile(parent)
        .map_err(|error| access_context(error, "create a file in", parent))?;
    serde_json::to_writer(&mut file, value)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|error| access_context(error.error, "replace", path))?;
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
    create_private_directory(directory)
        .map_err(|error| access_context(error, "create", directory))?;
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| access_context(error, "inspect", directory))?;
    ensure!(
        metadata.is_dir() && owner_only(&metadata),
        "{owner} directory must be private (0700)"
    );
    #[cfg(windows)]
    set_private_windows_dacl(directory)?;
    Ok(())
}

/// Whether neither group nor others have any access (Unix mode bits). On
/// Windows `ensure_private_directory` applies the protected DACL separately.
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
        .open(&lock_path)
        .map_err(|error| access_context(error, "open", &lock_path))?;
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

/// Use the process creation FILETIME on Windows. `sysinfo::Process::start_time`
/// has only second precision, which can mistake a quickly reused PID for a
/// stale gateway's process.
#[cfg(target_os = "windows")]
pub(crate) fn process_start_time(pid: u32) -> Result<Option<u64>> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, GetLastError,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return Ok(None);
    }
    // SAFETY: OpenProcess has no pointer arguments; the PID is supplied by the
    // private registration and may simply no longer exist.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        // SAFETY: GetLastError has no preconditions.
        let error = unsafe { GetLastError() };
        if error == ERROR_INVALID_PARAMETER {
            return Ok(None);
        }
        return Err(std::io::Error::from_raw_os_error(error as i32).into());
    }

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all output pointers refer to initialized FILETIME storage and the
    // process handle is valid until it is closed below.
    let succeeded =
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
    if succeeded == 0 {
        // SAFETY: GetLastError has no preconditions.
        let error = unsafe { GetLastError() };
        // SAFETY: process is the valid handle returned by OpenProcess.
        unsafe { CloseHandle(process) };
        return Err(std::io::Error::from_raw_os_error(error as i32).into());
    }
    // SAFETY: process is the valid handle returned by OpenProcess.
    unsafe { CloseHandle(process) };
    Ok(Some(
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
    ))
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

/// The protected DACL of a private directory: full control, inherited by its
/// files, for the owner, for SYSTEM and for the user identified by `user_sid`.
#[cfg(any(windows, test))]
fn private_dacl_sddl(user_sid: &str) -> String {
    format!("D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;{user_sid})")
}

/// The string form (`S-1-5-21-…`) of the SID of the user this process runs as.
/// An elevated token of the same user carries the same SID.
#[cfg(windows)]
fn current_user_sid() -> Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn last_error() -> anyhow::Error {
        // SAFETY: GetLastError has no preconditions.
        std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32).into()
    }

    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo handle of this process is always valid and token is a
    // valid output pointer. The token is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error().context("failed to open the process token"));
    }
    let result = (|| {
        // The first call reports the size the TOKEN_USER and its SID need.
        let mut length = 0u32;
        // SAFETY: a null buffer of length 0 only asks for the required size.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length) };
        if length == 0 {
            return Err(last_error().context("failed to size the token user"));
        }
        // u64 elements keep the buffer aligned for TOKEN_USER.
        let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
        // SAFETY: the buffer holds at least `length` bytes.
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(last_error().context("failed to read the token user"));
        }
        // SAFETY: the buffer now holds a TOKEN_USER whose SID points into it.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: sid is valid and text is a valid output pointer; the string
        // is allocated with LocalAlloc and freed below.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(last_error().context("failed to format the user SID"));
        }
        // SAFETY: text is a NUL-terminated UTF-16 string.
        let sid = unsafe {
            let length = (0..).take_while(|&i| *text.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
        };
        // SAFETY: text was allocated by ConvertSidToStringSidW with LocalAlloc.
        unsafe { LocalFree(text.cast()) };
        Ok(sid)
    })();
    // SAFETY: token is the valid handle opened above.
    unsafe { CloseHandle(token) };
    result
}

#[cfg(windows)]
fn set_private_windows_dacl(directory: &Path) -> Result<()> {
    private_windows_security(directory, false)
}

#[cfg(windows)]
fn private_windows_security(directory: &Path, create: bool) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SetFileSecurityW,
    };

    let path: Vec<u16> = directory
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Protect the DACL from parent inheritance. OI/CI carry the rules to the
    // files inside. The user's SID is explicit because `OWNER RIGHTS` follows
    // the owner, and an elevated process makes that `BUILTIN\Administrators`.
    let sddl: Vec<u16> = private_dacl_sddl(&current_user_sid()?)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: both UTF-16 inputs are NUL-terminated and descriptor is a valid
    // output pointer. The returned descriptor is freed with LocalFree below.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        // SAFETY: GetLastError has no preconditions.
        return Err(std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32).into());
    }

    if create {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
        use windows_sys::Win32::Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: the path and security descriptor remain valid for the call;
        // the returned handle and allocated descriptor are released below.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        let error = if handle == INVALID_HANDLE_VALUE {
            Some(std::io::Error::last_os_error())
        } else {
            // SAFETY: this is the valid handle returned above.
            unsafe { CloseHandle(handle) };
            None
        };
        // SAFETY: the conversion above allocated this descriptor.
        unsafe { LocalFree(descriptor.cast()) };
        return error.map_or(Ok(()), |error| Err(error.into()));
    }

    // SAFETY: path and descriptor are valid for this call. This replaces the
    // existing directory DACL instead of relying on the profile's inheritance.
    let succeeded = unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    let error = if succeeded == 0 {
        // SAFETY: GetLastError has no preconditions.
        Some(std::io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ))
    } else {
        None
    };
    // SAFETY: ConvertStringSecurityDescriptorToSecurityDescriptorW allocated
    // this descriptor with LocalAlloc.
    unsafe { LocalFree(descriptor.cast()) };
    error.map_or(Ok(()), |error| {
        Err(access_context(error, "set permissions on", directory))
    })
}

/// Seconds since the epoch, from the process's creation time.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
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
    fn private_files_are_protected_before_writing_and_reopen_without_truncation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        fs::write(&path, "existing").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let mut file = open_private(&path, OpenOptions::new().append(true).create(true)).unwrap();
        #[cfg(unix)]
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        file.write_all(b" appended").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing appended");
        assert!(create_private_new(&path).is_err());
        let temporary = private_tempfile(directory.path()).unwrap();
        #[cfg(unix)]
        assert_eq!(
            temporary.as_file().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        #[cfg(windows)]
        for path in [path.as_path(), temporary.path()] {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Foundation::LocalFree;
            use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
            use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, GetFileSecurityW};
            let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut needed = 0;
            // SAFETY: a null buffer queries the descriptor's required size.
            unsafe {
                GetFileSecurityW(
                    path.as_ptr(),
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    0,
                    &mut needed,
                )
            };
            assert!(needed > 0);
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
            // SAFETY: buffer is aligned and holds at least needed bytes.
            assert_ne!(
                unsafe {
                    GetFileSecurityW(
                        path.as_ptr(),
                        DACL_SECURITY_INFORMATION,
                        buffer.as_mut_ptr().cast(),
                        needed,
                        &mut needed,
                    )
                },
                0
            );
            let mut text: *mut u16 = std::ptr::null_mut();
            // SAFETY: buffer holds a valid descriptor; text receives an allocated string.
            assert_ne!(
                unsafe {
                    ConvertSecurityDescriptorToStringSecurityDescriptorW(
                        buffer.as_mut_ptr().cast(),
                        1,
                        DACL_SECURITY_INFORMATION,
                        &mut text,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            let mut length = 0;
            // SAFETY: the successful conversion returns a NUL-terminated string.
            let sddl = unsafe {
                while *text.add(length) != 0 {
                    length += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
            };
            // SAFETY: the conversion allocated text with LocalAlloc.
            unsafe { LocalFree(text.cast()) };
            assert!(sddl.contains("D:P"), "{sddl}");
            let user_sid = current_user_sid().unwrap();
            // Windows abbreviates the built-in Administrator (RID 500) as LA.
            assert!(
                sddl.contains(&user_sid) || (user_sid.ends_with("-500") && sddl.contains(";;;LA)")),
                "{sddl} does not grant access to {user_sid}"
            );
            assert!(sddl.contains(";;;SY)"), "{sddl}");
            for broad in [
                ";;;WD)",
                ";;;BU)",
                ";;;AU)",
                ";;;S-1-1-0)",
                ";;;S-1-5-32-545)",
            ] {
                assert!(!sddl.contains(broad), "{sddl}");
            }
        }
    }

    #[test]
    fn a_held_lock_is_busy_until_dropped() {
        let root = tempfile::tempdir().unwrap();
        // Created 0700 by the lock itself.
        let directory = root.path().join("remote");
        let held = lock_private_file(&directory, "owner.lock", "test").unwrap();
        let busy = lock_private_file(&directory, "owner.lock", "test").unwrap_err();
        assert!(is_lock_busy(&busy), "{busy:#}");
        drop(held);
        // A concurrent fork can retain the open file description until exec,
        // even though the descriptor is close-on-exec. Wait for that release.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match lock_private_file(&directory, "owner.lock", "test") {
                Ok(_lock) => break,
                Err(error) if is_lock_busy(&error) && std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(error) => panic!("lock was not released: {error:#}"),
            }
        }
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
    fn the_private_dacl_names_the_user_and_system_beside_the_owner() {
        let sddl = private_dacl_sddl("S-1-5-21-1-2-3-1001");
        // Protected, so a parent directory's rules are not inherited.
        assert!(sddl.starts_with("D:P"), "{sddl}");
        for trustee in ["OW", "SY", "S-1-5-21-1-2-3-1001"] {
            assert!(sddl.contains(&format!("(A;OICI;FA;;;{trustee})")), "{sddl}");
        }
    }

    #[test]
    fn a_denied_open_names_the_file_and_other_errors_pass_through() {
        let path = Path::new("remote/owner.lock");
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let message = format!("{:#}", access_context(denied, "open", path));
        assert!(message.contains("access denied to open"), "{message}");
        assert!(message.contains("owner.lock"), "{message}");
        assert_eq!(message.contains("icacls"), cfg!(windows), "{message}");

        let missing = access_context(std::io::ErrorKind::NotFound.into(), "open", path);
        let missing = missing.downcast::<std::io::Error>().unwrap();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn this_process_has_a_start_time_and_ids_differ() {
        assert!(process_start_time(std::process::id()).unwrap().is_some());
        let id = random_instance_id().unwrap();
        assert_eq!(id.len(), 32);
        assert_ne!(id, random_instance_id().unwrap());
    }
}
