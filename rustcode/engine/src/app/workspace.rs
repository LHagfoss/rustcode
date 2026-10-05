use std::path::Path;
use std::path::PathBuf;
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
    refresh_generation: u64,
    refresh_in_flight: Option<WorkspaceLocationRefresh>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceLocationRefresh {
    pub(crate) generation: u64,
    pub(crate) cwd: PathBuf,
    pub(crate) session_id: String,
}

impl WorkspaceLocationCache {
    pub fn new(cwd: &Path, now: Instant) -> Self {
        Self {
            location: WorkspaceLocation::detect(cwd),
            next_refresh_at: now + LOCATION_REFRESH_INTERVAL,
            refresh_generation: 0,
            refresh_in_flight: None,
        }
    }

    pub(crate) fn display(&self) -> String {
        self.location.display()
    }

    /// Claim one refresh at a time. The caller runs Git off the event loop and
    /// returns the result with the request token so stale session lookups can
    /// be discarded safely.
    pub(crate) fn claim_refresh(
        &mut self,
        cwd: &Path,
        session_id: &str,
        now: Instant,
    ) -> Option<WorkspaceLocationRefresh> {
        if self.refresh_in_flight.is_some() || now < self.next_refresh_at {
            return None;
        }

        self.next_refresh_at = now + LOCATION_REFRESH_INTERVAL;
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        let request = WorkspaceLocationRefresh {
            generation: self.refresh_generation,
            cwd: cwd.to_path_buf(),
            session_id: session_id.to_owned(),
        };
        self.refresh_in_flight = Some(request.clone());
        Some(request)
    }

    /// Install a result only while its request still owns the refresh slot and
    /// the active session and working directory still match its inputs.
    pub(crate) fn complete_refresh(
        &mut self,
        request: WorkspaceLocationRefresh,
        cwd: &Path,
        session_id: &str,
        location: Option<WorkspaceLocation>,
    ) -> bool {
        if self.refresh_in_flight.as_ref() != Some(&request) {
            return false;
        }
        self.refresh_in_flight = None;
        if request.cwd != cwd || request.session_id != session_id {
            return false;
        }
        let Some(location) = location else {
            return false;
        };
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
    use std::time::Instant;
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
    fn refresh_is_single_flight_and_discards_a_result_for_an_old_session() {
        let repo = repository();
        let now = Instant::now();
        let mut cache = WorkspaceLocationCache::new(repo.path(), now);
        let initial = cache.display();

        let due = now + LOCATION_REFRESH_INTERVAL;
        let request = cache
            .claim_refresh(repo.path(), "session-a", due)
            .expect("refresh is due");
        assert!(
            cache
                .claim_refresh(repo.path(), "session-a", due + LOCATION_REFRESH_INTERVAL)
                .is_none()
        );

        let stale = WorkspaceLocation {
            path: initial.split(':').next().unwrap().to_owned(),
            branch: "stale-result".to_owned(),
        };
        assert!(!cache.complete_refresh(request, repo.path(), "session-b", Some(stale),));
        assert_eq!(cache.display(), initial);

        assert!(
            cache
                .claim_refresh(repo.path(), "session-b", due + LOCATION_REFRESH_INTERVAL)
                .is_some()
        );
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
