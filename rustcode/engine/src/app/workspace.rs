use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// The footer should notice branch changes promptly without making Git part of
/// the render path or polling it on every event-loop iteration.
pub(crate) const LOCATION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceLocation {
    pub(crate) path: String,
    pub(crate) branch: String,
}

impl WorkspaceLocation {
    pub(crate) fn detect(cwd: &Path) -> Self {
        let absolute_path = cwd.to_string_lossy().to_string();
        let path = match std::env::var("HOME") {
            Ok(home) if !home.is_empty() && absolute_path.starts_with(&home) => {
                absolute_path.replacen(&home, "~", 1)
            }
            _ => absolute_path,
        };

        let branch = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(cwd)
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    std::str::from_utf8(&out.stdout)
                        .ok()
                        .map(|value| value.trim().to_string())
                } else {
                    None
                }
            })
            .filter(|branch| !branch.is_empty())
            // Keep the existing footer fallback for directories without Git.
            .unwrap_or_else(|| "main".to_string());

        Self { path, branch }
    }

    pub(crate) fn display(&self) -> String {
        format!("{}:{}", self.path, self.branch)
    }
}

#[derive(Debug)]
pub(crate) struct WorkspaceLocationCache {
    location: WorkspaceLocation,
    next_refresh_at: Instant,
}

impl WorkspaceLocationCache {
    pub(crate) fn new(cwd: &Path, now: Instant) -> Self {
        Self {
            location: WorkspaceLocation::detect(cwd),
            next_refresh_at: now + LOCATION_REFRESH_INTERVAL,
        }
    }

    pub(crate) fn display(&self) -> String {
        self.location.display()
    }

    /// Refresh at most once per interval, returning whether the footer value
    /// changed. The next check is scheduled even when Git reports no change,
    /// which bounds subprocess frequency while idle.
    pub(crate) fn refresh_if_due(&mut self, cwd: &Path, now: Instant) -> bool {
        if now < self.next_refresh_at {
            return false;
        }

        self.next_refresh_at = now + LOCATION_REFRESH_INTERVAL;
        let location = WorkspaceLocation::detect(cwd);
        if location == self.location {
            return false;
        }

        self.location = location;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{LOCATION_REFRESH_INTERVAL, WorkspaceLocation, WorkspaceLocationCache};
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("git should be available");
        assert!(status.success(), "git command failed: {args:?}");
    }

    fn repository() -> TempDir {
        let dir = tempfile::tempdir().expect("temporary repository");
        git(dir.path(), &["init", "-q"]);
        git(
            dir.path(),
            &["config", "user.email", "rustcode@example.test"],
        );
        git(dir.path(), &["config", "user.name", "RustCode Tests"]);
        fs::write(dir.path().join("README.md"), "test\n").expect("seed repository");
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-qm", "initial"]);
        dir
    }

    #[test]
    fn refresh_is_debounced_and_picks_up_an_external_branch_change() {
        let repo = repository();
        let now = Instant::now();
        let mut cache = WorkspaceLocationCache::new(repo.path(), now);
        let initial = cache.display();

        git(repo.path(), &["checkout", "-qb", "footer-refresh"]);
        assert_eq!(cache.display(), initial);
        assert!(!cache.refresh_if_due(
            repo.path(),
            now + LOCATION_REFRESH_INTERVAL - Duration::from_millis(1)
        ));

        assert!(cache.refresh_if_due(repo.path(), now + LOCATION_REFRESH_INTERVAL));
        assert!(cache.display().ends_with(":footer-refresh"));
    }

    #[test]
    fn detached_head_is_kept_as_head() {
        let repo = repository();
        git(repo.path(), &["checkout", "--detach", "HEAD"]);

        let location = WorkspaceLocation::detect(repo.path());
        assert_eq!(location.branch, "HEAD");
    }

    #[test]
    fn non_git_directories_keep_the_existing_main_fallback() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let location = WorkspaceLocation::detect(dir.path());

        assert_eq!(location.branch, "main");
    }
}
