//! Small platform/process helpers shared by tools and compiler checks.
//!
//! These functions intentionally have no dependency on the agent or network
//! layers.  Keeping executable-path construction here lets tool execution be
//! extracted into its own crate without introducing a reverse dependency on
//! the network implementation.

use std::path::{Path, PathBuf};

/// Return a stable executable search path for commands launched by RustCode's
/// tools.  The order and duplicate-preserving behavior are kept compatible
/// with the former network-owned helper.
pub(crate) fn augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    augmented_path_from(
        &home,
        std::env::var("PATH").ok().as_deref(),
        false,
        cfg!(target_os = "windows"),
    )
}

/// Return the compiler-check variant of the search path.  Compiler checks have
/// historically included Bun and the current NVM node directory and removed
/// duplicates from PATH; retain that behavior while sharing ownership with
/// the tool-facing path helper.
pub(crate) fn compiler_augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    augmented_path_from(
        &home,
        std::env::var("PATH").ok().as_deref(),
        true,
        cfg!(target_os = "windows"),
    )
}

fn augmented_path_from(
    home: &str,
    existing: Option<&str>,
    compiler_dirs: bool,
    windows: bool,
) -> String {
    // The directories below are POSIX locations joined with `:`. Prepending
    // them to a `;`-separated Windows PATH fuses its first entry into them.
    if windows {
        return existing.unwrap_or_default().to_owned();
    }
    let mut dirs = vec![format!("{home}/.cargo/bin")];
    if compiler_dirs {
        dirs.push(format!("{home}/.bun/bin"));
        dirs.push(format!("{home}/.nvm/versions/node/current/bin"));
    }
    dirs.extend([
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ]);
    if let Some(existing) = existing {
        for dir in existing.split(':') {
            if !compiler_dirs || !dirs.iter().any(|known| known == dir) {
                dirs.push(dir.to_string());
            }
        }
    }
    dirs.join(":")
}

/// Find a locally installed executable before falling back to PATH lookup.
/// This is used by compiler checks and belongs with the PATH construction
/// helper rather than with network transport.
pub(crate) fn resolve_bin(name: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.cargo/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
        format!("/usr/bin/{name}"),
    ];
    for candidate in candidates {
        if Path::new(&candidate).exists() {
            return PathBuf::from(candidate);
        }
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::{augmented_path, augmented_path_from};

    #[test]
    fn augmented_path_keeps_standard_tool_directories() {
        let path = augmented_path();
        let entries: Vec<&str> = path.split(':').collect();
        assert!(entries.iter().any(|entry| entry.ends_with("/.cargo/bin")));
        assert!(entries.contains(&"/usr/bin"));
        assert!(entries.contains(&"/bin"));
    }

    #[test]
    fn path_variants_preserve_tool_and_compiler_contracts() {
        assert_eq!(
            augmented_path_from("/home/test", Some("/custom/bin:/usr/bin"), false, false),
            "/home/test/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/custom/bin:/usr/bin"
        );
        assert_eq!(
            augmented_path_from("/home/test", Some("/custom/bin:/usr/bin"), true, false),
            "/home/test/.cargo/bin:/home/test/.bun/bin:/home/test/.nvm/versions/node/current/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/custom/bin"
        );
    }

    #[test]
    fn windows_path_is_passed_through_untouched() {
        let path = r"C:\Windows\System32;C:\Program Files\Git\cmd";
        for compiler_dirs in [false, true] {
            assert_eq!(
                augmented_path_from(r"C:\Users\test", Some(path), compiler_dirs, true),
                path
            );
        }
        assert_eq!(augmented_path_from("", None, false, true), "");
    }
}

/// Construct a non-interactive std process without a Windows console.
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    rustcode_command::hidden_command(program)
}

/// Construct a non-interactive Tokio process without a Windows console.
pub(crate) fn tokio_command(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    rustcode_command::hide_console_window(command.as_std_mut());
    command
}

/// Let a background process outlive whoever started it: its own session on
/// Unix, so the terminal's hangup does not reach it; on Windows its own
/// process group, so the console's Ctrl+C does not, and no console window.
pub(crate) fn detach(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: setsid is async-signal-safe and the callback accesses no shared state.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(WINDOWS_DETACH_FLAGS);
    }
}

/// Windows creation flags of a background process: its own process group,
/// so the console's Ctrl+C does not reach it, and no console window.
#[cfg_attr(not(windows), allow(dead_code))]
const WINDOWS_DETACH_FLAGS: u32 = 0x0000_0200 | 0x0800_0000;
/// `CREATE_BREAKAWAY_FROM_JOB`.
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// Start a background process with `spawn`, given the Windows creation flags
/// to use, outside the job object the caller runs in when the job allows it.
///
/// Windows OpenSSH runs every session in a job that ends all its processes
/// when the session closes (`KILL_ON_JOB_CLOSE`, with breakaway allowed), and
/// a harness or CI runner can do the same. A process started inside such a
/// job dies with the session that started it, killed without a word in any
/// log: a server restarted over SSH lived until the SSH command returned.
/// A job that forbids breakaway refuses the flag (access denied): start
/// inside it then, which is all that is possible.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn spawn_breaking_away<T>(
    flags: u32,
    mut spawn: impl FnMut(u32) -> std::io::Result<T>,
) -> std::io::Result<T> {
    const ERROR_ACCESS_DENIED: i32 = 5;
    match spawn(flags | CREATE_BREAKAWAY_FROM_JOB) {
        Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED) => spawn(flags),
        result => result,
    }
}

/// Spawn `command` so that it outlives whoever started it ([`detach`]), and
/// on Windows out of the caller's job object ([`spawn_breaking_away`]).
pub(crate) fn spawn_detached(
    command: &mut tokio::process::Command,
) -> std::io::Result<tokio::process::Child> {
    detach(command.as_std_mut());
    #[cfg(windows)]
    {
        spawn_breaking_away(WINDOWS_DETACH_FLAGS, |flags| {
            command.creation_flags(flags);
            command.spawn()
        })
    }
    #[cfg(not(windows))]
    {
        command.spawn()
    }
}

#[cfg(test)]
mod detach_tests {
    use super::*;

    #[test]
    fn a_job_that_refuses_breakaway_gets_the_process_inside_it() {
        let mut tried = Vec::new();
        let spawned = spawn_breaking_away(WINDOWS_DETACH_FLAGS, |flags| {
            tried.push(flags);
            if flags & CREATE_BREAKAWAY_FROM_JOB != 0 {
                Err(std::io::Error::from_raw_os_error(5))
            } else {
                Ok(flags)
            }
        });
        assert_eq!(spawned.unwrap(), WINDOWS_DETACH_FLAGS);
        assert_eq!(
            tried,
            [
                WINDOWS_DETACH_FLAGS | CREATE_BREAKAWAY_FROM_JOB,
                WINDOWS_DETACH_FLAGS
            ]
        );
    }

    #[test]
    fn breakaway_is_tried_first_and_other_failures_are_not_retried() {
        let mut tried = 0;
        let spawned = spawn_breaking_away(WINDOWS_DETACH_FLAGS, |flags| {
            tried += 1;
            Ok(flags)
        });
        assert_eq!(
            spawned.unwrap(),
            WINDOWS_DETACH_FLAGS | CREATE_BREAKAWAY_FROM_JOB
        );
        assert_eq!(tried, 1);
        let mut tried = 0;
        let missing = spawn_breaking_away(WINDOWS_DETACH_FLAGS, |_| -> std::io::Result<()> {
            tried += 1;
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert!(missing.is_err());
        assert_eq!(tried, 1);
    }
}
