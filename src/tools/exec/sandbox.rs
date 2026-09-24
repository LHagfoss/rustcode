//! Operating-system command isolation.
//!
//! Linux uses bubblewrap to give shell commands a read-only view of the host
//! and writable access only to the active workspace/session scratch directory.
//! Network access is isolated with a private network namespace when bubblewrap
//! can configure loopback; otherwise seccomp restricts network syscalls and
//! socket families. macOS uses Seatbelt through `/usr/bin/sandbox-exec` with
//! the same workspace and network restrictions. Other platforms retain their
//! existing execution path.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct SandboxedCommand {
    pub command: String,
    pub inherited_fds: Vec<Arc<std::fs::File>>,
}

/// Permission inputs shared by native sandbox backends.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct SandboxPolicy<'a> {
    pub command_cwd: Option<&'a Path>,
    pub workspace_root: Option<&'a Path>,
    pub writable_roots: &'a [PathBuf],
    pub session_scratch_roots: &'a [PathBuf],
    /// Network remains denied by default. A later permission mode may opt in.
    pub network_access: bool,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn command(
    command: &str,
    policy: SandboxPolicy<'_>,
) -> Result<SandboxedCommand, String> {
    #[cfg(target_os = "linux")]
    return linux_command(command, policy);

    #[cfg(target_os = "macos")]
    return macos_command(command, policy);
}

#[cfg(target_os = "linux")]
fn linux_command(command: &str, policy: SandboxPolicy<'_>) -> Result<SandboxedCommand, String> {
    let workspace = canonical_directory(policy.workspace_root, "active workspace")?;
    let cwd = match policy.command_cwd {
        Some(path) => canonical_directory(Some(path), "command working directory")?,
        None => workspace.clone(),
    };
    let mut writable_roots = Vec::new();
    for root in policy.writable_roots {
        let canonical = if policy.session_scratch_roots.contains(root) {
            match std::fs::symlink_metadata(root) {
                Ok(_) => canonical_session_scratch(root)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "Linux shell sandbox could not inspect session scratch directory '{}': {error}; command was not run",
                        root.display()
                    ));
                }
            }
        } else {
            canonical_directory(Some(root), "writable root")?
        };
        if canonical == Path::new("/") {
            return Err(
                "Linux shell sandbox refused `/` as a writable root; command was not run"
                    .to_string(),
            );
        }
        writable_roots.push(canonical);
    }
    if !writable_roots.contains(&workspace) {
        return Err("Linux shell sandbox requires the active workspace in its writable roots; command was not run".to_string());
    }
    let bubblewrap = find_bubblewrap().ok_or_else(|| {
        "Linux shell sandbox unavailable: install bubblewrap (`bwrap`) in a root-owned system PATH directory such as /usr/bin; command was not run".to_string()
    })?;
    if !writable_roots.iter().any(|root| cwd.starts_with(root)) {
        return Err("Linux shell sandbox refused a working directory outside its writable roots; command was not run".to_string());
    }

    let seccomp = if !policy.network_access {
        Some(Arc::new(create_network_filter()?))
    } else {
        None
    };
    let use_network_namespace = if !policy.network_access {
        Some(probe_network_namespace(
            &bubblewrap,
            seccomp.as_ref().unwrap(),
        )?)
    } else {
        None
    };

    use std::os::fd::AsRawFd;
    let mut args = base_bubblewrap_arguments(
        use_network_namespace == Some(true),
        seccomp.as_ref().map(|filter| filter.as_raw_fd()),
    );
    for root in writable_roots {
        args.extend([
            "--bind".to_string(),
            root.display().to_string(),
            root.display().to_string(),
        ]);
    }
    args.extend([
        "--chdir".to_string(),
        cwd.display().to_string(),
        "--".to_string(),
        "/bin/bash".to_string(),
        "-o".to_string(),
        "pipefail".to_string(),
        "-c".to_string(),
        command.to_string(),
    ]);

    // rustcode-command invokes its command string through bash -c. Quote every
    // argument here so shell metacharacters remain data across that boundary.
    let mut wrapped = shell_quote(&bubblewrap.to_string_lossy());
    for argument in args {
        wrapped.push(' ');
        wrapped.push_str(&shell_quote(&argument));
    }
    Ok(SandboxedCommand {
        command: wrapped,
        inherited_fds: seccomp.into_iter().collect(),
    })
}

#[cfg(target_os = "linux")]
fn base_bubblewrap_arguments(
    isolate_network: bool,
    seccomp_fd: Option<std::os::fd::RawFd>,
) -> Vec<String> {
    let mut args = vec![
        "--die-with-parent".to_string(),
        "--new-session".to_string(),
        "--unshare-user".to_string(),
        "--unshare-pid".to_string(),
        "--unshare-ipc".to_string(),
        "--disable-userns".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
        "--tmpfs".to_string(),
        "/run".to_string(),
    ];
    if isolate_network {
        args.push("--unshare-net".to_string());
    }
    if let Some(fd) = seccomp_fd {
        args.extend(["--seccomp".to_string(), fd.to_string()]);
    }
    args
}

/// Whether this test host can execute commands inside the production sandbox.
/// A runner without user namespace support cannot exercise command behavior,
/// but production must continue to fail closed in that case.
#[cfg(test)]
pub(crate) fn runtime_tests_available() -> bool {
    #[cfg(not(target_os = "linux"))]
    return true;

    #[cfg(target_os = "linux")]
    {
        use std::sync::OnceLock;
        static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
        match RESULT.get_or_init(|| {
            let bubblewrap = match find_bubblewrap() {
                Some(path) => path,
                None if !bubblewrap_candidate_exists() => {
                    return Err("bubblewrap is not installed".to_string());
                }
                None => return Err("installed bubblewrap is not trusted".to_string()),
            };
            let workspace = tempfile::tempdir()
                .map_err(|error| format!("could not create sandbox probe workspace: {error}"))?;
            let writable_roots = vec![workspace.path().to_path_buf()];
            let prepared = command(
                "true",
                SandboxPolicy {
                    command_cwd: Some(workspace.path()),
                    workspace_root: Some(workspace.path()),
                    writable_roots: &writable_roots,
                    session_scratch_roots: &[],
                    network_access: false,
                },
            )?;
            let request = rustcode_command::CommandRequest {
                command: prepared.command,
                cwd: Some(workspace.path().to_path_buf()),
                env: Vec::new(),
                timeout: std::time::Duration::from_secs(5),
                process_group: true,
                inherited_fds: prepared.inherited_fds,
                status_command: None,
            };
            let output = rustcode_command::run_with_timeout(&request, None)
                .map_err(|error| format!("sandbox execution probe failed: {error}"))?;
            if output.success {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(output.stderr.bytes()).into_owned())
            }
        }) {
            Ok(()) => true,
            Err(reason)
                if reason == "bubblewrap is not installed"
                    || reason.contains("setting up uid map: Permission denied") =>
            {
                eprintln!("skipping shell execution assertion: {reason}");
                false
            }
            Err(reason) => panic!("sandbox test preflight failed unexpectedly: {reason}"),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
fn bubblewrap_candidate_exists() -> bool {
    let mut directories = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    directories.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    directories
        .into_iter()
        .any(|directory| directory.join("bwrap").exists())
}

#[cfg(target_os = "macos")]
const MACOS_PATH_TO_SEATBELT_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

#[cfg(target_os = "macos")]
fn macos_command(command: &str, policy: SandboxPolicy<'_>) -> Result<SandboxedCommand, String> {
    command_with_seatbelt_path(
        command,
        policy,
        Path::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE),
    )
}

#[cfg(target_os = "macos")]
fn command_with_seatbelt_path(
    command: &str,
    policy: SandboxPolicy<'_>,
    seatbelt: &Path,
) -> Result<SandboxedCommand, String> {
    let workspace = canonical_directory(policy.workspace_root, "active workspace")?;
    let cwd = match policy.command_cwd {
        Some(path) => canonical_directory(Some(path), "command working directory")?,
        None => workspace.clone(),
    };

    let mut writable_roots = Vec::new();
    for root in policy.writable_roots {
        let canonical = if policy.session_scratch_roots.contains(root) {
            match std::fs::symlink_metadata(root) {
                Ok(_) => canonical_session_scratch(root)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "macOS shell sandbox could not inspect session scratch directory '{}': {error}; command was not run",
                        root.display()
                    ));
                }
            }
        } else {
            canonical_directory(Some(root), "writable root")?
        };
        if canonical == Path::new("/") {
            return Err(
                "macOS shell sandbox refused `/` as a writable root; command was not run"
                    .to_string(),
            );
        }
        if !writable_roots.contains(&canonical) {
            writable_roots.push(canonical);
        }
    }
    if workspace == Path::new("/") {
        return Err(
            "macOS shell sandbox refused `/` as the active workspace; command was not run"
                .to_string(),
        );
    }
    if !writable_roots.contains(&workspace) {
        return Err("macOS shell sandbox requires the active workspace in its writable roots; command was not run".to_string());
    }
    if !writable_roots.iter().any(|root| cwd.starts_with(root)) {
        return Err("macOS shell sandbox refused a working directory outside its writable roots; command was not run".to_string());
    }

    if seatbelt != Path::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE) && !seatbelt.is_file() {
        return Err(format!(
            "macOS shell sandbox unavailable: '{}' is not a regular sandbox-exec file; command was not run",
            seatbelt.display()
        ));
    }
    if !seatbelt.is_file() {
        return Err(format!(
            "macOS shell sandbox unavailable: required '{}' was not found; command was not run",
            MACOS_PATH_TO_SEATBELT_EXECUTABLE
        ));
    }

    let profile = seatbelt_profile(writable_roots.len(), policy.network_access);
    let mut arguments = vec!["-p".to_string(), profile];
    for (index, root) in writable_roots.iter().enumerate() {
        arguments.push(format!("-DWRITABLE_ROOT_{index}={}", root.display()));
    }
    arguments.extend([
        "--".to_string(),
        "/bin/bash".to_string(),
        "-o".to_string(),
        "pipefail".to_string(),
        "-c".to_string(),
        command.to_string(),
    ]);

    // rustcode-command starts requests through `bash -c`, so preserve every
    // argv boundary and keep shell syntax in the original command as data.
    // Keep compiler and build-tool temporary files inside a writable root.
    // The inherited host TMPDIR usually points outside Seatbelt's policy.
    let temp_dir = shell_quote(&workspace.to_string_lossy());
    let mut wrapped = format!("TMPDIR={temp_dir} TMP={temp_dir} TEMP={temp_dir} ");
    wrapped.push_str(&shell_quote(&seatbelt.to_string_lossy()));
    for argument in arguments {
        wrapped.push(' ');
        wrapped.push_str(&shell_quote(&argument));
    }
    Ok(SandboxedCommand {
        command: wrapped,
        inherited_fds: Vec::new(),
    })
}

#[cfg(target_os = "macos")]
fn seatbelt_profile(writable_root_count: usize, network_access: bool) -> String {
    let mut profile = String::from(
        r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow file-read*)
(allow file-write-data (require-all (literal "/dev/null") (vnode-type CHARACTER-DEVICE)))
(allow sysctl-read)
"#,
    );
    for index in 0..writable_root_count {
        profile.push_str(&format!(
            "(allow file-write* (subpath (param \"WRITABLE_ROOT_{index}\")))\n"
        ));
    }
    if network_access {
        profile
            .push_str("(allow network-outbound)\n(allow network-inbound)\n(allow network-bind)\n");
    }
    profile
}

#[cfg(target_os = "macos")]
fn canonical_directory(path: Option<&Path>, description: &str) -> Result<PathBuf, String> {
    let path = path.ok_or_else(|| {
        format!("macOS shell sandbox needs an {description}; command was not run")
    })?;
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "macOS shell sandbox could not resolve {description} '{}': {error}; command was not run",
            path.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "macOS shell sandbox {description} '{}' is not a directory; command was not run",
            path.display()
        ));
    }
    Ok(canonical)
}

#[cfg(target_os = "macos")]
fn canonical_session_scratch(path: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "macOS shell sandbox could not inspect session scratch directory '{}': {error}; command was not run",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(
            "macOS shell sandbox refused an invalid session scratch directory; command was not run"
                .to_string(),
        );
    }
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "macOS shell sandbox could not resolve session scratch directory '{}': {error}; command was not run",
            path.display()
        )
    })?;
    let parent = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .ok_or_else(|| {
            "macOS shell sandbox could not resolve the session directory; command was not run"
                .to_string()
        })?;
    if canonical != parent.join("sandbox") {
        return Err("macOS shell sandbox refused a redirected session scratch directory; command was not run".to_string());
    }
    Ok(canonical)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn command(
    command: &str,
    _policy: SandboxPolicy<'_>,
) -> Result<SandboxedCommand, String> {
    Ok(SandboxedCommand {
        command: command.to_string(),
        inherited_fds: Vec::new(),
    })
}

#[cfg(target_os = "linux")]
fn create_network_filter() -> Result<std::fs::File, String> {
    use std::io::{Seek, SeekFrom, Write};
    use std::os::fd::FromRawFd;

    let instructions = network_filter_instructions()?;
    let name = std::ffi::CString::new("rustcode-network-filter").unwrap();
    // SAFETY: the name is a valid NUL-terminated C string.
    let fd =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(format!(
            "Linux shell sandbox could not create seccomp filter: {}; command was not run",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: memfd_create returned a new owned descriptor.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    for instruction in instructions {
        file.write_all(&instruction.code.to_ne_bytes())
            .and_then(|_| file.write_all(&[instruction.jt, instruction.jf]))
            .and_then(|_| file.write_all(&instruction.k.to_ne_bytes()))
            .map_err(|error| format!("Linux shell sandbox could not write seccomp filter: {error}; command was not run"))?;
    }
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        format!("Linux shell sandbox could not rewind seccomp filter: {error}; command was not run")
    })?;
    Ok(file)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct BpfInstruction {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[cfg(target_os = "linux")]
fn network_filter_instructions() -> Result<Vec<BpfInstruction>, String> {
    const LD_W_ABS: u16 = 0x20;
    const JMP_JEQ_K: u16 = 0x15;
    const JMP_JSET_K: u16 = 0x45;
    const RET_K: u16 = 0x06;
    const ALLOW: u32 = 0x7fff_0000;
    const ERRNO_EPERM: u32 = 0x0005_0000 | libc::EPERM as u32;
    const KILL_PROCESS: u32 = 0x8000_0000;
    const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
    const AUDIT_ARCH_AARCH64: u32 = 0xc000_00b7;
    const OFF_NR: u32 = 0;
    const OFF_ARCH: u32 = 4;
    const OFF_ARG0: u32 = 16;

    #[cfg(target_arch = "x86_64")]
    let audit_arch = AUDIT_ARCH_X86_64;
    #[cfg(target_arch = "aarch64")]
    let audit_arch = AUDIT_ARCH_AARCH64;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return Err("Linux shell sandbox has no network seccomp filter for this architecture; command was not run".to_string());

    let mut bpf = vec![
        BpfInstruction {
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: OFF_ARCH,
        },
        BpfInstruction {
            code: JMP_JEQ_K,
            jt: 1,
            jf: 0,
            k: audit_arch,
        },
        BpfInstruction {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: KILL_PROCESS,
        },
        BpfInstruction {
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: OFF_NR,
        },
    ];
    #[cfg(target_arch = "x86_64")]
    {
        // Reject x32 ABI syscalls, whose numbers share the x86_64 audit arch.
        bpf.extend([
            BpfInstruction {
                code: JMP_JSET_K,
                jt: 0,
                jf: 1,
                k: 0x4000_0000,
            },
            BpfInstruction {
                code: RET_K,
                jt: 0,
                jf: 0,
                k: ERRNO_EPERM,
            },
            BpfInstruction {
                code: LD_W_ABS,
                jt: 0,
                jf: 0,
                k: OFF_NR,
            },
        ]);
    }
    let denied = [
        libc::SYS_connect,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_getpeername,
        libc::SYS_getsockname,
        libc::SYS_shutdown,
        libc::SYS_sendto,
        libc::SYS_sendmmsg,
        libc::SYS_recvmmsg,
        libc::SYS_getsockopt,
        libc::SYS_setsockopt,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ];
    for syscall in denied {
        bpf.extend([
            BpfInstruction {
                code: JMP_JEQ_K,
                jt: 0,
                jf: 1,
                k: syscall as u32,
            },
            BpfInstruction {
                code: RET_K,
                jt: 0,
                jf: 0,
                k: ERRNO_EPERM,
            },
        ]);
    }
    for syscall in [libc::SYS_socket, libc::SYS_socketpair] {
        bpf.extend([
            BpfInstruction {
                code: JMP_JEQ_K,
                jt: 0,
                jf: 3,
                k: syscall as u32,
            },
            BpfInstruction {
                code: LD_W_ABS,
                jt: 0,
                jf: 0,
                k: OFF_ARG0,
            },
            BpfInstruction {
                code: JMP_JEQ_K,
                jt: 1,
                jf: 0,
                k: libc::AF_UNIX as u32,
            },
            BpfInstruction {
                code: RET_K,
                jt: 0,
                jf: 0,
                k: ERRNO_EPERM,
            },
            BpfInstruction {
                code: LD_W_ABS,
                jt: 0,
                jf: 0,
                k: OFF_NR,
            },
        ]);
    }
    bpf.push(BpfInstruction {
        code: RET_K,
        jt: 0,
        jf: 0,
        k: ALLOW,
    });
    Ok(bpf)
}

#[cfg(target_os = "linux")]
fn probe_network_namespace(bubblewrap: &Path, filter: &Arc<std::fs::File>) -> Result<bool, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let fd = filter.as_raw_fd();
    let mut probe = std::process::Command::new(bubblewrap);
    let mut args = base_bubblewrap_arguments(true, Some(fd));
    args.extend(["--".to_string(), "/bin/true".to_string()]);
    probe.args(args);
    // SAFETY: the hook only makes the filter memfd inheritable in this child.
    unsafe {
        probe.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags == -1 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let result = probe.output().map_err(|error| format!("Linux shell sandbox could not probe bubblewrap network isolation: {error}; command was not run"))?;
    // Bubblewrap consumes the filter FD when setting up a working namespace.
    // The probe and the eventual shell share this memfd's open-file description,
    // so restore its offset before handing it to the real command.
    if let Err(error) = rewind_filter_fd(fd) {
        return Err(format!(
            "Linux shell sandbox could not rewind seccomp filter after network probe: {}; command was not run",
            error
        ));
    }
    if result.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&result.stderr);
    if stderr.contains("loopback: Failed RTM_NEWADDR") && stderr.contains("Operation not permitted")
    {
        return Ok(false);
    }
    Err(format!(
        "Linux shell sandbox bubblewrap network probe failed: {}; command was not run",
        stderr.trim()
    ))
}

#[cfg(target_os = "linux")]
fn rewind_filter_fd(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    if unsafe { libc::lseek(fd, 0, libc::SEEK_SET) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn find_bubblewrap() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let mut directories = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    directories.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    directories
        .into_iter()
        .map(|directory| directory.join("bwrap"))
        .filter_map(|path| path.canonicalize().ok())
        .find(|path| {
            let Ok(metadata) = path.metadata() else {
                return false;
            };
            if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                return false;
            }
            let executable = metadata.mode() & 0o111 != 0;
            executable
                && path.ancestors().all(|ancestor| {
                    ancestor
                        .metadata()
                        .is_ok_and(|metadata| metadata.uid() == 0 && metadata.mode() & 0o022 == 0)
                })
        })
}

#[cfg(target_os = "linux")]
fn canonical_directory(path: Option<&Path>, description: &str) -> Result<PathBuf, String> {
    let path = path.ok_or_else(|| {
        format!("Linux shell sandbox needs an {description}; command was not run")
    })?;
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "Linux shell sandbox could not resolve {description} '{}': {error}; command was not run",
            path.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "Linux shell sandbox {description} '{}' is not a directory; command was not run",
            path.display()
        ));
    }
    #[cfg(target_os = "linux")]
    if (description == "active workspace" || description == "writable root")
        && canonical == Path::new("/")
    {
        return Err(
            "Linux shell sandbox refused `/` as the active workspace; command was not run"
                .to_string(),
        );
    }
    Ok(canonical)
}

#[cfg(target_os = "linux")]
fn canonical_session_scratch(path: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Linux shell sandbox could not inspect session scratch directory '{}': {error}; command was not run",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(
            "Linux shell sandbox refused an invalid session scratch directory; command was not run"
                .to_string(),
        );
    }
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "Linux shell sandbox could not resolve session scratch directory '{}': {error}; command was not run",
            path.display()
        )
    })?;
    let parent = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .ok_or_else(|| {
            "Linux shell sandbox could not resolve the session directory; command was not run"
                .to_string()
        })?;
    if canonical != parent.join("sandbox") {
        return Err("Linux shell sandbox refused a redirected session scratch directory; command was not run".to_string());
    }
    Ok(canonical)
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    fn macos_policy<'a>(
        workspace: &'a Path,
        writable_roots: &'a [PathBuf],
        session_scratch_roots: &'a [PathBuf],
        command_cwd: Option<&'a Path>,
    ) -> SandboxPolicy<'a> {
        SandboxPolicy {
            command_cwd,
            workspace_root: Some(workspace),
            writable_roots,
            session_scratch_roots,
            network_access: false,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_command_keeps_paths_out_of_profile_and_quotes_shell_arguments() {
        let workspace = tempfile::Builder::new()
            .prefix("rustcode workspace's ")
            .tempdir()
            .unwrap();
        let roots = vec![workspace.path().to_path_buf()];
        let prepared = command(
            "printf '%s' \"a b\"",
            macos_policy(workspace.path(), &roots, &[], Some(workspace.path())),
        )
        .unwrap();

        assert!(prepared.command.contains("/usr/bin/sandbox-exec"));
        assert!(prepared.command.contains("deny default"));
        assert!(prepared.command.contains("WRITABLE_ROOT_0"));
        assert!(prepared.command.contains("printf"));
        assert!(prepared.command.contains("a b"));
        assert!(prepared.command.contains("'\\''"));
        assert!(!prepared.command.contains("rustcode workspace's "));
        assert!(prepared.inherited_fds.is_empty());

        let profile = seatbelt_profile(1, false);
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("WRITABLE_ROOT_0"));
        assert!(!profile.contains(&workspace.path().to_string_lossy().to_string()));
        assert!(!profile.contains("network-outbound"));
        assert!(seatbelt_profile(1, true).contains("(allow network-outbound)"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_command_fails_closed_without_sandbox_exec() {
        let workspace = tempfile::tempdir().unwrap();
        let roots = vec![workspace.path().to_path_buf()];
        let error = command_with_seatbelt_path(
            "touch must-not-run",
            macos_policy(workspace.path(), &roots, &[], Some(workspace.path())),
            Path::new("/definitely/missing/sandbox-exec"),
        )
        .unwrap_err();

        assert!(error.contains("sandbox-exec"));
        assert!(error.contains("command was not run"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_requires_workspace_and_rejects_cwd_outside_writable_roots() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let roots = vec![workspace.path().to_path_buf()];

        let missing_workspace = command(
            "id",
            SandboxPolicy {
                command_cwd: None,
                workspace_root: None,
                writable_roots: &[],
                session_scratch_roots: &[],
                network_access: false,
            },
        )
        .unwrap_err();
        assert!(missing_workspace.contains("active workspace"));
        assert!(missing_workspace.contains("command was not run"));

        let outside_cwd = command(
            "id",
            macos_policy(workspace.path(), &roots, &[], Some(outside.path())),
        )
        .unwrap_err();
        assert!(outside_cwd.contains("outside its writable roots"));
        assert!(outside_cwd.contains("command was not run"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rejects_a_session_scratch_symlink() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let session = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let scratch = session.path().join("sandbox");
        symlink(outside.path(), &scratch).unwrap();
        let roots = vec![workspace.path().to_path_buf(), scratch.clone()];
        let scratch_roots = vec![scratch];

        let error = command(
            "id",
            macos_policy(
                workspace.path(),
                &roots,
                &scratch_roots,
                Some(workspace.path()),
            ),
        )
        .unwrap_err();

        assert!(error.contains("invalid session scratch directory"));
        assert!(error.contains("command was not run"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_allows_workspace_and_session_writes_but_denies_outside_and_network() {
        let workspace = tempfile::tempdir().unwrap();
        let session = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let scratch = session.path().join("sandbox");
        std::fs::create_dir(&scratch).unwrap();
        let workspace_file = workspace.path().join("workspace-write");
        let scratch_file = scratch.join("session-write");
        let outside_file = outside.path().join("outside-write");
        let roots = vec![workspace.path().to_path_buf(), scratch.clone()];
        let scratch_roots = vec![scratch.clone()];
        let write_command = format!(
            "printf w > {}; printf s > {}; printf o > {}",
            shell_quote(&workspace_file.to_string_lossy()),
            shell_quote(&scratch_file.to_string_lossy()),
            shell_quote(&outside_file.to_string_lossy()),
        );
        let prepared = command(
            &write_command,
            macos_policy(
                workspace.path(),
                &roots,
                &scratch_roots,
                Some(workspace.path()),
            ),
        )
        .unwrap();
        let output = run_sandboxed(prepared, workspace.path());
        if seatbelt_is_unavailable(&output) {
            eprintln!("skipping Seatbelt enforcement assertions: nested Seatbelt is unavailable");
            return;
        }
        assert!(
            !output.success,
            "outside write should be denied: {}",
            String::from_utf8_lossy(output.stderr.bytes())
        );
        assert_eq!(std::fs::read_to_string(workspace_file).unwrap(), "w");
        assert_eq!(std::fs::read_to_string(scratch_file).unwrap(), "s");
        assert!(!outside_file.exists());

        let ipv4_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let ipv4_port = ipv4_listener.local_addr().unwrap().port();
        let ipv6_listener = std::net::TcpListener::bind(("::1", 0)).unwrap();
        let ipv6_port = ipv6_listener.local_addr().unwrap().port();
        for (family, host, port) in [
            ("AF_INET", "127.0.0.1", ipv4_port),
            ("AF_INET6", "::1", ipv6_port),
        ] {
            let script = format!(
                "import errno,socket;\ns=socket.socket(socket.{family}, socket.SOCK_STREAM)\ntry: s.connect(({host:?}, {port}))\nexcept OSError as e: assert e.errno in (errno.EPERM, errno.EACCES), e\nelse: raise AssertionError('{family} connection was allowed')"
            );
            let network_command = format!("python3 -c {}", shell_quote(&script));
            let prepared = command(
                &network_command,
                macos_policy(
                    workspace.path(),
                    &roots,
                    &scratch_roots,
                    Some(workspace.path()),
                ),
            )
            .unwrap();
            let output = run_sandboxed(prepared, workspace.path());
            assert!(
                output.success,
                "{family} socket was not denied: {}",
                String::from_utf8_lossy(output.stderr.bytes())
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_denies_writes_through_workspace_symlinks() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("symlink-escape");
        let link = workspace.path().join("outside-link");
        symlink(outside.path(), &link).unwrap();
        let roots = vec![workspace.path().to_path_buf()];
        let command_text = format!(
            "printf x > {}",
            shell_quote(&link.join("symlink-escape").to_string_lossy())
        );
        let prepared = command(
            &command_text,
            macos_policy(workspace.path(), &roots, &[], Some(workspace.path())),
        )
        .unwrap();
        let output = run_sandboxed(prepared, workspace.path());
        if seatbelt_is_unavailable(&output) {
            eprintln!("skipping Seatbelt enforcement assertions: nested Seatbelt is unavailable");
            return;
        }

        assert!(!output.success, "symlink escape should fail");
        assert!(!outside_file.exists(), "symlink escaped the writable root");
    }

    #[test]
    fn shell_quote_keeps_metacharacters_as_data() {
        assert_eq!(
            shell_quote("a'b; $(touch nope)"),
            "'a'\\''b; $(touch nope)'"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn network_filter_restricts_socket_families_and_io_uring() {
        let instructions = network_filter_instructions().unwrap();
        let compared_values = instructions
            .iter()
            .filter(|instruction| instruction.code == 0x15)
            .map(|instruction| instruction.k)
            .collect::<Vec<_>>();
        for syscall in [
            libc::SYS_socket,
            libc::SYS_socketpair,
            libc::SYS_connect,
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ] {
            assert!(compared_values.contains(&(syscall as u32)));
        }
        assert!(compared_values.contains(&(libc::AF_UNIX as u32)));
        assert!(instructions.iter().any(|instruction| {
            instruction.code == 0x06 && instruction.k == (0x0005_0000 | libc::EPERM as u32)
        }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn seccomp_filter_fd_rewinds_after_consumption() {
        use std::io::{Seek, SeekFrom};
        use std::os::fd::AsRawFd;

        let mut filter = create_network_filter().unwrap();
        filter.seek(SeekFrom::End(0)).unwrap();
        assert!(filter.stream_position().unwrap() > 0);
        rewind_filter_fd(filter.as_raw_fd()).unwrap();
        assert_eq!(filter.stream_position().unwrap(), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_workspace_fails_closed() {
        let error = command(
            "id",
            SandboxPolicy {
                command_cwd: None,
                workspace_root: None,
                writable_roots: &[],
                session_scratch_roots: &[],
                network_access: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("needs an active workspace"));
        assert!(error.contains("command was not run"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn working_directory_outside_writable_roots_fails_closed() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let roots = vec![workspace.path().to_path_buf()];
        let error = command(
            "id",
            SandboxPolicy {
                command_cwd: Some(outside.path()),
                workspace_root: Some(workspace.path()),
                writable_roots: &roots,
                session_scratch_roots: &[],
                network_access: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("outside its writable roots"));
        assert!(error.contains("command was not run"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn working_directory_symlink_cannot_escape_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let cwd_link = workspace.path().join("outside-link");
        symlink(outside.path(), &cwd_link).unwrap();
        let roots = vec![workspace.path().to_path_buf()];
        let error = command(
            "id",
            SandboxPolicy {
                command_cwd: Some(&cwd_link),
                workspace_root: Some(workspace.path()),
                writable_roots: &roots,
                session_scratch_roots: &[],
                network_access: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("outside its writable roots"));
        assert!(error.contains("command was not run"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn session_scratch_symlink_redirection_fails_closed() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let session = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let scratch_link = session.path().join("sandbox");
        symlink(outside.path(), &scratch_link).unwrap();
        let roots = vec![workspace.path().to_path_buf(), scratch_link.clone()];
        let scratch_roots = vec![scratch_link];
        let error = command(
            "id",
            SandboxPolicy {
                command_cwd: Some(workspace.path()),
                workspace_root: Some(workspace.path()),
                writable_roots: &roots,
                session_scratch_roots: &scratch_roots,
                network_access: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("invalid session scratch directory"));
        assert!(error.contains("command was not run"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_unused_session_scratch_root_is_omitted() {
        let workspace = tempfile::tempdir().unwrap();
        let session = tempfile::tempdir().unwrap();
        let scratch = session.path().join("sandbox");
        let roots = vec![workspace.path().to_path_buf(), scratch.clone()];
        let scratch_roots = vec![scratch.clone()];
        let prepared = command(
            "true",
            SandboxPolicy {
                command_cwd: Some(workspace.path()),
                workspace_root: Some(workspace.path()),
                writable_roots: &roots,
                session_scratch_roots: &scratch_roots,
                network_access: true,
            },
        )
        .unwrap();
        assert!(!prepared.command.contains(&scratch.display().to_string()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sandbox_command_contains_filesystem_mounts_without_a_runtime_probe() {
        let workspace = tempfile::tempdir().unwrap();
        let roots = vec![workspace.path().to_path_buf()];
        let output = command(
            "printf '%s' \"a b\"",
            SandboxPolicy {
                command_cwd: Some(workspace.path()),
                workspace_root: Some(workspace.path()),
                writable_roots: &roots,
                session_scratch_roots: &[],
                network_access: true,
            },
        );
        if find_bubblewrap().is_some() {
            let wrapped = output.unwrap();
            assert!(wrapped.command.contains("--ro-bind"));
            assert!(wrapped.command.contains("--bind"));
            assert!(wrapped.command.contains("printf"));
            assert!(wrapped.command.contains("a b"));
            assert!(wrapped.inherited_fds.is_empty());
        } else {
            eprintln!("skipping bwrap argv assertions: bubblewrap is unavailable");
            assert!(output.unwrap_err().contains("install bubblewrap"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bubblewrap_blocks_host_writes_and_network_when_available() {
        if !runtime_tests_available() {
            let workspace = tempfile::tempdir().unwrap();
            let marker = workspace.path().join("command-must-not-run");
            let roots = vec![workspace.path().to_path_buf()];
            let result = command(
                &format!("touch {}", shell_quote(&marker.to_string_lossy())),
                SandboxPolicy {
                    command_cwd: Some(workspace.path()),
                    workspace_root: Some(workspace.path()),
                    writable_roots: &roots,
                    session_scratch_roots: &[],
                    network_access: false,
                },
            );
            match result {
                Ok(prepared) => {
                    let output = run_sandboxed(prepared, workspace.path());
                    assert!(
                        !output.success,
                        "sandbox setup unexpectedly succeeded on this runner"
                    );
                    let error = String::from_utf8_lossy(output.stderr.bytes());
                    assert!(
                        error.contains("setting up uid map: Permission denied"),
                        "unexpected sandbox setup failure: {error}"
                    );
                }
                Err(error) => {
                    assert!(error.contains("command was not run"), "{error}");
                }
            }
            assert!(!marker.exists(), "command ran after sandbox setup failed");
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("should-not-exist");
        let workspace_file = workspace.path().join("allowed-write");
        let command_text = format!(
            "printf x > {}; printf y > {}",
            shell_quote(&outside_file.to_string_lossy()),
            shell_quote(&workspace_file.to_string_lossy())
        );
        let roots = vec![workspace.path().to_path_buf()];
        let make_policy = || SandboxPolicy {
            command_cwd: Some(workspace.path()),
            workspace_root: Some(workspace.path()),
            writable_roots: &roots,
            session_scratch_roots: &[],
            network_access: false,
        };
        let wrapped = command(&command_text, make_policy()).unwrap();
        use std::os::fd::AsRawFd;
        assert_eq!(
            unsafe { libc::lseek(wrapped.inherited_fds[0].as_raw_fd(), 0, libc::SEEK_CUR,) },
            0,
            "seccomp filter must be rewound after the bwrap probe"
        );
        let output = run_sandboxed(wrapped, workspace.path());
        assert!(
            output.success,
            "{}",
            String::from_utf8_lossy(output.stderr.bytes())
        );
        assert!(
            !outside_file.exists(),
            "sandbox wrote outside the workspace"
        );
        assert_eq!(std::fs::read_to_string(workspace_file).unwrap(), "y");

        let git = command("git init -q", make_policy()).unwrap();
        let output = run_sandboxed(git, workspace.path());
        assert!(
            output.success,
            "git init failed: {}",
            String::from_utf8_lossy(output.stderr.bytes())
        );
        assert!(workspace.path().join(".git").is_dir());

        for family in ["AF_INET", "AF_INET6"] {
            let script = format!(
                "import errno,socket;\ntry: socket.socket(socket.{family}, socket.SOCK_STREAM)\nexcept OSError as e: assert e.errno == errno.EPERM, e\nelse: raise AssertionError('{family} socket was allowed')"
            );
            let network_command = format!("python3 -c {}", shell_quote(&script));
            let output = run_sandboxed(
                command(&network_command, make_policy()).unwrap(),
                workspace.path(),
            );
            assert!(
                output.success,
                "{family} was not denied: {}",
                String::from_utf8_lossy(output.stderr.bytes())
            );
        }
        let unix_script = "import socket; a,b=socket.socketpair(socket.AF_UNIX); a.send(b'x'); assert b.recv(1)==b'x'";
        let unix_command = format!("python3 -c {}", shell_quote(unix_script));
        let output = run_sandboxed(
            command(&unix_command, make_policy()).unwrap(),
            workspace.path(),
        );
        assert!(
            output.success,
            "AF_UNIX socketpair failed: {}",
            String::from_utf8_lossy(output.stderr.bytes())
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn run_sandboxed(command: SandboxedCommand, cwd: &Path) -> rustcode_command::CommandOutput {
        rustcode_command::run_with_timeout(
            &rustcode_command::CommandRequest {
                command: command.command,
                status_command: None,
                cwd: Some(cwd.to_path_buf()),
                env: Vec::new(),
                timeout: std::time::Duration::from_secs(10),
                process_group: true,
                inherited_fds: command.inherited_fds,
            },
            None,
        )
        .expect("sandboxed command should start")
    }

    #[cfg(target_os = "macos")]
    fn seatbelt_is_unavailable(output: &rustcode_command::CommandOutput) -> bool {
        String::from_utf8_lossy(output.stderr.bytes())
            .contains("sandbox_apply: Operation not permitted")
    }
}
