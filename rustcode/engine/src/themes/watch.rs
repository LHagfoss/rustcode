//! Live theme changes for ACP clients: `_rustcode/themes/changed`.
//!
//! The engine polls the theme directories (user, and the project of the
//! client's last `_rustcode/themes` request) and the user config, whose
//! `theme` names the active one. A change is told once it has held still
//! for [`DEBOUNCE`], so an editor's save (often a write then a rename) or an
//! agent writing a file in pieces sends one notification.

use super::ThemesSnapshot;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

pub(crate) const DEBOUNCE: Duration = Duration::from_millis(200);
pub(crate) const POLL: Duration = Duration::from_millis(100);

/// Trailing-edge debouncer over a signature of what is watched.
#[derive(Debug)]
pub(crate) struct Debouncer<S> {
    told: S,
    pending: Option<(S, Instant)>,
    delay: Duration,
}

impl<S: PartialEq> Debouncer<S> {
    pub(crate) fn new(initial: S, delay: Duration) -> Self {
        Self {
            told: initial,
            pending: None,
            delay,
        }
    }

    /// Feed the signature seen at `now`; true when a change has held still
    /// for the delay and should be told.
    pub(crate) fn observe(&mut self, current: S, now: Instant) -> bool {
        if let Some((pending, since)) = &self.pending
            && *pending == current
        {
            if now.duration_since(*since) >= self.delay {
                self.told = current;
                self.pending = None;
                return true;
            }
            return false;
        }
        self.pending = (current != self.told).then_some((current, now));
        false
    }
}

pub(crate) type Signature = Vec<(PathBuf, Option<SystemTime>, u64)>;

/// What a change of looks like: each watched theme file and the config file,
/// by path, modification time and size.
pub(crate) fn signature(dirs: &[PathBuf], files: &[PathBuf]) -> Signature {
    let mut out: Signature = dirs
        .iter()
        .flat_map(|dir| super::theme_files(dir))
        .chain(files.iter().cloned())
        .map(|path| {
            let meta = std::fs::metadata(&path).ok();
            let modified = meta.as_ref().and_then(|m| m.modified().ok());
            let len = meta.map(|m| m.len()).unwrap_or(0);
            (path, modified, len)
        })
        .collect();
    out.sort();
    out
}

fn workspace_slot() -> &'static Mutex<Option<PathBuf>> {
    static WORKSPACE: Mutex<Option<PathBuf>> = Mutex::new(None);
    &WORKSPACE
}

/// Remember the client's project, whose themes are watched too.
pub(crate) fn set_workspace(workspace: Option<&Path>) {
    if let Some(workspace) = workspace.filter(|path| path.is_absolute()) {
        *workspace_slot()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(workspace.to_path_buf());
    }
}

pub(crate) fn workspace() -> Option<PathBuf> {
    workspace_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn watched() -> Signature {
    let workspace = workspace();
    let mut dirs: Vec<PathBuf> = super::user_themes_dir().into_iter().collect();
    let mut files = Vec::new();
    if let Some(workspace) = &workspace {
        dirs.extend(super::project_themes_dirs(workspace));
    }
    if let Some(config) = crate::config::get_config_dir() {
        files.push(config.join("config.toml"));
    }
    signature(&dirs, &files)
}

/// Poll forever, calling `tell` with a fresh snapshot after each settled
/// change.
pub(crate) async fn run(tell: impl Fn(ThemesSnapshot)) {
    super::ensure_user_dir();
    let mut debouncer = Debouncer::new(watched(), DEBOUNCE);
    loop {
        tokio::time::sleep(POLL).await;
        let current = tokio::task::spawn_blocking(watched)
            .await
            .unwrap_or_default();
        if debouncer.observe(current, Instant::now()) {
            let workspace = workspace();
            let snapshot =
                tokio::task::spawn_blocking(move || super::snapshot(workspace.as_deref())).await;
            if let Ok(snapshot) = snapshot {
                tell(snapshot);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_told_once_it_holds_still() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut debouncer = Debouncer::new(0, DEBOUNCE);
        assert!(!debouncer.observe(0, at(0)), "nothing changed");
        assert!(!debouncer.observe(1, at(100)), "a change waits");
        assert!(!debouncer.observe(2, at(200)), "a further change restarts");
        assert!(!debouncer.observe(2, at(350)), "not yet 200 ms still");
        assert!(debouncer.observe(2, at(400)), "held still 200 ms");
        assert!(!debouncer.observe(2, at(500)), "told once");
        // A change undone before it settles is never told.
        assert!(!debouncer.observe(3, at(600)));
        assert!(!debouncer.observe(2, at(700)));
        assert!(!debouncer.observe(2, at(1000)));
    }

    #[test]
    fn the_signature_sees_an_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mine.toml");
        std::fs::write(&path, "a").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let dirs = vec![dir.path().to_path_buf()];
        let before = signature(&dirs, &[]);
        assert_eq!(before.len(), 1, "only theme files");
        std::fs::write(&path, "abc").unwrap();
        assert_ne!(signature(&dirs, &[]), before);
    }
}
