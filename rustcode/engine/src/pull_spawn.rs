//! Why `gh` could not be started in a folder.

use std::path::Path;

use crate::pull_requests::PullError;

/// The error for a `gh` spawn that failed with "not found". On macOS and
/// Linux a missing working folder, such as a removed worktree, fails the same
/// way as a missing program, so the folder is checked before blaming `gh`.
pub(crate) fn not_found(cwd: &Path) -> PullError {
    if !cwd.is_dir() {
        return PullError {
            code: "folder_missing",
            message: format!("The folder {} no longer exists.", cwd.display()),
        };
    }
    PullError {
        code: "gh_missing",
        message: format!(
            "The GitHub CLI (gh) is not installed. Install it ({}) and connect GitHub in Settings ▸ Connections.",
            crate::install_hints::host_hint(crate::install_hints::Tool::GitHub)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::not_found;

    #[test]
    fn a_folder_that_no_longer_exists_is_not_reported_as_gh_missing() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("removed-worktree");
        let error = not_found(&gone);
        assert_eq!(error.code, "folder_missing", "{error:?}");
        assert!(error.message.contains("removed-worktree"), "{error:?}");
        assert_eq!(not_found(dir.path()).code, "gh_missing");
    }

    #[tokio::test]
    async fn gh_in_a_removed_folder_says_the_folder_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("removed-worktree");
        let error = crate::pull_requests::gh(&gone, &["--version"], None)
            .await
            .unwrap_err();
        assert_eq!(error.code, "folder_missing", "{error:?}");
    }
}
