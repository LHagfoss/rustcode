//! Operating-system command isolation.
//!
//! Linux uses bubblewrap to give shell commands a read-only view of the host,
//! writable access only to the active workspace/session scratch directory,
//! and a private network namespace. Other platforms retain their existing
//! execution path until their native backend is implemented.

use std::path::Path;
use std::path::PathBuf;

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

#[cfg(target_os = "linux")]
pub(crate) fn command(command: &str, policy: SandboxPolicy<'_>) -> Result<String, String> {
    let workspace = canonical_directory(policy.workspace_root, "active workspace")?;
    let cwd = match policy.command_cwd {
        Some(path) => canonical_directory(Some(path), "command working directory")?,
        None => workspace.clone(),
    };
    let mut writable_roots = Vec::new();
    for root in policy.writable_roots {
        let canonical = if policy.session_scratch_roots.contains(root) {
            canonical_session_scratch(root)?
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
    if !policy.network_access {
        args.push("--unshare-net".to_string());
    }
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
    Ok(wrapped)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn command(command: &str, _policy: SandboxPolicy<'_>) -> Result<String, String> {
    Ok(command.to_string())
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

#[cfg(any(target_os = "linux", test))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_keeps_metacharacters_as_data() {
        assert_eq!(
            shell_quote("a'b; $(touch nope)"),
            "'a'\\''b; $(touch nope)'"
        );
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
    fn sandbox_command_contains_private_network_and_read_only_host_mounts() {
        let workspace = tempfile::tempdir().unwrap();
        let roots = vec![workspace.path().to_path_buf()];
        let output = command(
            "printf '%s' \"a b\"",
            SandboxPolicy {
                command_cwd: Some(workspace.path()),
                workspace_root: Some(workspace.path()),
                writable_roots: &roots,
                session_scratch_roots: &[],
                network_access: false,
            },
        );
        if find_bubblewrap().is_some() {
            let wrapped = output.unwrap();
            assert!(wrapped.contains("--unshare-net"));
            assert!(wrapped.contains("--ro-bind"));
            assert!(wrapped.contains("--bind"));
            assert!(wrapped.contains("printf '%s' \"a b\""));
        } else {
            eprintln!("skipping bwrap argv assertions: bubblewrap is unavailable");
            assert!(output.unwrap_err().contains("install bubblewrap"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bubblewrap_blocks_host_writes_and_network_when_available() {
        use std::net::TcpListener;
        use std::process::Command;

        let Some(_bwrap) = find_bubblewrap() else {
            eprintln!("skipping OS sandbox integration: bubblewrap is not installed");
            return;
        };
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
        let output = Command::new("/bin/bash")
            .args(["-c", &wrapped])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !outside_file.exists(),
            "sandbox wrote outside the workspace"
        );
        assert_eq!(std::fs::read_to_string(workspace_file).unwrap(), "y");

        let git = command("git init -q", make_policy()).unwrap();
        let output = Command::new("/bin/bash")
            .args(["-c", &git])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(workspace.path().join(".git").is_dir());

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        assert!(
            Command::new("python3")
                .arg("--version")
                .output()
                .unwrap()
                .status
                .success()
        );
        let port = listener.local_addr().unwrap().port();
        let network_command = format!(
            "python3 -c {}",
            shell_quote(&format!(
                "import socket; s=socket.create_connection(('127.0.0.1',{port}), timeout=1)"
            ))
        );
        let wrapped = command(&network_command, make_policy()).unwrap();
        let output = Command::new("/bin/bash")
            .args(["-c", &wrapped])
            .output()
            .unwrap();
        assert!(!output.status.success(), "sandbox reached host loopback");
        drop(listener);
    }
}
