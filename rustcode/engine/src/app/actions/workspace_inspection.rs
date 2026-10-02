use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const STATUS_OUTPUT_BYTES: usize = 8 * 1024;
const STDERR_OUTPUT_BYTES: usize = 4 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

enum GitError {
    Failed(String),
    OutputLimit(usize),
    Io(io::Error),
    Timeout,
}

pub(super) async fn inspect_workspace(root: &Path) -> Result<String, String> {
    let status = match run_git(
        root,
        &["status", "--short", "--untracked-files=normal"],
        STATUS_OUTPUT_BYTES,
    )
    .await
    {
        Ok(status) => status,
        Err(GitError::Failed(error)) if error.contains("not a git repository") => {
            return Ok("This workspace is not a Git repository.".to_owned());
        }
        Err(error) => return Err(format_git_error(error)),
    };
    let status = String::from_utf8_lossy(&status);
    let untracked = status
        .lines()
        .filter_map(|line| line.strip_prefix("?? "))
        .collect::<Vec<_>>();

    let mut remaining = MAX_OUTPUT_BYTES.saturating_sub(status.len() + 128);
    let staged = match run_git(
        root,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--color=never",
        ],
        remaining,
    )
    .await
    {
        Ok(diff) => diff,
        Err(error) => return Err(format_git_error(error)),
    };
    remaining = remaining.saturating_sub(staged.len());
    let unstaged = match run_git(
        root,
        &["diff", "--no-ext-diff", "--no-textconv", "--color=never"],
        remaining,
    )
    .await
    {
        Ok(diff) => diff,
        Err(error) => return Err(format_git_error(error)),
    };

    if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() {
        return Ok("The Git working tree is clean.".to_owned());
    }

    let mut output = String::new();
    if !staged.is_empty() {
        output.push_str("Staged changes:\n");
        output.push_str(&fenced(&String::from_utf8_lossy(&staged), "diff"));
    }
    if !unstaged.is_empty() {
        output.push_str("Unstaged changes:\n");
        output.push_str(&fenced(&String::from_utf8_lossy(&unstaged), "diff"));
    }
    if !untracked.is_empty() {
        output.push_str("Untracked paths (contents omitted):\n");
        output.push_str(&fenced(&untracked.join("\n"), "text"));
    }
    Ok(output)
}

fn format_git_error(error: GitError) -> String {
    match error {
        GitError::Failed(error) => format!("Git could not read this workspace: {error}"),
        GitError::OutputLimit(limit) => format!(
            "Git output exceeded the {} KiB display limit; narrow the changes before viewing them.",
            limit.div_ceil(1024)
        ),
        GitError::Io(error) => format!("Could not run Git: {error}"),
        GitError::Timeout => "Git diff timed out after 5 seconds.".to_owned(),
    }
}

async fn run_git(root: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>, GitError> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("--no-pager")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg("core.quotePath=true")
        .args(args)
        .env("LC_ALL", "C")
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(GitError::Io)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| GitError::Io(io::Error::other("Git stdout was not captured")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| GitError::Io(io::Error::other("Git stderr was not captured")))?;

    let operation = async {
        let wait = async { child.wait().await.map_err(GitError::Io) };
        let (stdout, stderr, status) = tokio::try_join!(
            read_bounded(stdout, limit),
            read_bounded(stderr, STDERR_OUTPUT_BYTES),
            wait
        )?;
        if !status.success() {
            return Err(GitError::Failed(
                String::from_utf8_lossy(&stderr).trim().to_owned(),
            ));
        }
        Ok(stdout)
    };
    tokio::time::timeout(COMMAND_TIMEOUT, operation)
        .await
        .map_err(|_| GitError::Timeout)?
}

async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> Result<Vec<u8>, GitError> {
    use tokio::io::AsyncReadExt;

    let mut output = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0; 8192];
    loop {
        let remaining = limit.saturating_add(1).saturating_sub(output.len());
        let chunk_size = remaining.min(buffer.len());
        let read = reader
            .read(&mut buffer[..chunk_size])
            .await
            .map_err(GitError::Io)?;
        if read == 0 {
            return Ok(output);
        }
        output.extend_from_slice(&buffer[..read]);
        if output.len() > limit {
            return Err(GitError::OutputLimit(limit));
        }
    }
}

pub(super) fn cwd_text(root: &Path) -> String {
    format!(
        "Effective workspace:\n{}",
        fenced(&root.display().to_string(), "text")
    )
}

pub(super) fn trigger(
    state: &Arc<Mutex<crate::app::AppState>>,
    root: PathBuf,
    expected_workspace_root: Option<PathBuf>,
    session_id: String,
    generation: u64,
) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let result = inspect_workspace(&root).await.unwrap_or_else(|error| error);
        let mut state = state.lock().await;
        if !request_is_current(&state, &expected_workspace_root, &session_id) {
            return;
        }
        state.update_command_panel_if_current("Git diff", generation, result);
    });
}

fn request_is_current(
    state: &crate::app::AppState,
    expected_workspace_root: &Option<PathBuf>,
    session_id: &str,
) -> bool {
    state.active_session_id == session_id
        && state.effective_workspace_root() == *expected_workspace_root
}

fn fenced(content: &str, language: &str) -> String {
    let mut fence = "```".to_owned();
    while content.contains(&fence) {
        fence.push('`');
    }
    format!("{fence}{language}\n{content}\n{fence}\n")
}

#[cfg(test)]
mod tests {
    use super::{inspect_workspace, request_is_current};
    use crate::app::AppState;
    use std::path::Path;

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git should start");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn includes_staged_and_unstaged_diffs_and_only_untracked_names() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-b", "main"]);
        git(
            root.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(root.path(), &["config", "user.name", "Test"]);
        std::fs::write(root.path().join("tracked.txt"), "base\n").unwrap();
        git(root.path(), &["add", "tracked.txt"]);
        git(root.path(), &["commit", "-m", "base"]);

        std::fs::write(root.path().join("tracked.txt"), "staged\n").unwrap();
        git(root.path(), &["add", "tracked.txt"]);
        std::fs::write(root.path().join("tracked.txt"), "unstaged\n").unwrap();
        std::fs::write(
            root.path().join("secret-untracked.txt"),
            "must not appear\n",
        )
        .unwrap();

        let output = inspect_workspace(root.path()).await.unwrap();
        assert!(output.contains("staged changes"));
        assert!(output.contains("Unstaged changes"), "{output}");
        assert!(output.contains("secret-untracked.txt"));
        assert!(!output.contains("must not appear"));
    }

    #[tokio::test]
    async fn reports_clean_repository_and_non_repository() {
        let clean = tempfile::tempdir().unwrap();
        git(clean.path(), &["init", "-b", "main"]);
        let output = inspect_workspace(clean.path()).await.unwrap();
        assert!(output.contains("working tree is clean"));

        let not_git = tempfile::tempdir().unwrap();
        let output = inspect_workspace(not_git.path()).await.unwrap();
        assert!(output.contains("not a Git repository"));
    }

    #[tokio::test]
    async fn reports_diff_output_limit_instead_of_returning_a_partial_diff() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-b", "main"]);
        git(
            root.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(root.path(), &["config", "user.name", "Test"]);
        std::fs::write(root.path().join("large.txt"), "base\n").unwrap();
        git(root.path(), &["add", "large.txt"]);
        git(root.path(), &["commit", "-m", "base"]);
        std::fs::write(root.path().join("large.txt"), "x".repeat(70 * 1024)).unwrap();

        let error = inspect_workspace(root.path()).await.unwrap_err();
        assert!(error.contains("display limit"));
    }

    #[test]
    fn async_result_guard_rejects_a_different_session_or_workspace() {
        let mut state = AppState::new();
        let session = state.active_session_id.clone();
        let workspace = state.effective_workspace_root();
        assert!(request_is_current(&state, &workspace, &session));

        state.active_session_id = "another-session".to_owned();
        assert!(!request_is_current(&state, &workspace, &session));
        state.active_session_id = session.clone();

        state.workspace_root = Some(std::path::PathBuf::from("/another/workspace"));
        assert!(!request_is_current(&state, &workspace, &session));
    }
}
