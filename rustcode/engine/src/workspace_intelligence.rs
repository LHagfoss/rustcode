//! Conservative workspace revisions shared by symbols, evidence and verification.
//! The initial scan records directories; unchanged queries validate metadata only.
//! Directory changes trigger discovery, including external creates/deletes/renames.
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    mutation: u64,
    target: Option<Box<Stamp>>,
    #[cfg(unix)]
    change: (i64, i64),
}
fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let mut value = metadata_stamp(&metadata);
    if metadata.file_type().is_symlink() {
        value.target = std::fs::metadata(path)
            .ok()
            .map(|target| Box::new(metadata_stamp(&target)));
    }
    Some(value)
}
fn metadata_stamp(metadata: &std::fs::Metadata) -> Stamp {
    Stamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
        mutation: 0,
        target: None,
        #[cfg(unix)]
        change: {
            use std::os::unix::fs::MetadataExt;
            (metadata.ctime(), metadata.ctime_nsec())
        },
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub root: PathBuf,
    pub generation: u64,
    pub cache_safe: bool,
    pub discovery: u64,
    pub files: BTreeMap<PathBuf, Stamp>,
}
#[derive(Default)]
struct State {
    generation: u64,
    mutation: u64,
    path_mutations: HashMap<PathBuf, u64>,
    files: BTreeMap<PathBuf, Stamp>,
    directories: BTreeMap<PathBuf, Stamp>,
    /// Directories left undesceded because the discovery budget ran out
    /// (oversized workspaces such as a home directory). Their own stamps are
    /// still checked, so topology changes under them trigger a bounded
    /// re-discovery instead of silently passing.
    pruned: BTreeMap<PathBuf, Stamp>,
    discovery: u64,
    initialized: bool,
    cache_safe: bool,
    /// Last time a discovery walk ran. A truncated workspace re-discovers at
    /// most once per hold window on background churn; explicit
    /// `invalidate()`/`invalidate_path()` (our own writes) always bypass it.
    last_discovery: Option<std::time::Instant>,
    /// Last time any snapshot (discovery or fast path) completed. Supports
    /// `fresh_generation()` so informational call sites share one refresh per
    /// turn instead of each scanning the workspace.
    last_snapshot: Option<std::time::Instant>,
}

/// Upper bound on entries enumerated by one discovery walk. A home directory
/// easily holds millions of entries (~/Library, ~/.cache, ...); without a
/// budget every background download re-triggers a minute-long walk. The
/// budget applies below depth 1: the root's direct children are always
/// enumerated (up to a separate generous cap) so project files and manifests
/// stay tracked no matter which subtree a depth-first walk visits first.
const MAX_DISCOVER_ENTRIES: usize = 50_000;
/// Cap on root-level entries of a single discovery walk. Only pathological
/// flat directories ever approach it.
const MAX_DEPTH1_ENTRIES: usize = 100_000;
/// Minimum spacing between churn-triggered re-discoveries of a truncated
/// workspace. Our own writes bypass this via `invalidate()`.
const TRUNCATED_REDISCOVERY_HOLD: std::time::Duration = std::time::Duration::from_secs(60);
/// Cap on pruned-subtree frontiers tracked per workspace. Past this, the root
/// stamp stands in for the untracked remainder.
const MAX_PRUNED_DIRS: usize = 10_000;
/// Walks slower than this (or any truncated walk) emit a telemetry event so
/// oversized workspace roots (e.g. a home directory) are visible in session
/// logs instead of silent per-round stalls.
const SLOW_SNAPSHOT_US: u64 = 500_000;

thread_local! {
    /// Handoff for directories pruned by the discovery filter: `filter_entry`
    /// must be `Fn + 'static`, so over-budget directory stamps collected
    /// during the walk are drained into the pruned map afterwards.
    static PRUNED_DIR_TX: std::cell::RefCell<Vec<(PathBuf, Stamp)>> =
        std::cell::RefCell::new(Vec::new());
}

fn home_directory_is(root: &Path) -> bool {
    std::env::var_os("HOME")
        .map(|home| root == Path::new(&home))
        .unwrap_or(false)
}
static WORKSPACES: LazyLock<Mutex<HashMap<PathBuf, State>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Persisted evidence must never confuse a revision from another process with
/// the current workspace revision. Epochs are opaque and are not persisted as
/// live state; every process starts a fresh evidence namespace.
pub(crate) fn epoch() -> &'static str {
    static EPOCH: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());
    EPOCH.as_str()
}

pub(crate) fn invalidate(root: &Path) {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut workspaces = WORKSPACES.lock().unwrap_or_else(|e| e.into_inner());
    let state = workspaces.entry(root).or_default();
    state.generation = state.generation.saturating_add(1);
    state.mutation = state.generation;
    // Mutation paths are validated on the next snapshot. Explicit invalidation
    // also covers commands that restore timestamps or modify ignored inputs.
    state.initialized = false;
}

/// Known file writes preserve discovery state and only invalidate that file.
pub(crate) fn invalidate_path(root: &Path, path: &Path) {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path = path.canonicalize().unwrap_or(path);
    let mut workspaces = WORKSPACES.lock().unwrap_or_else(|e| e.into_inner());
    let state = workspaces.entry(root).or_default();
    state.generation = state.generation.saturating_add(1);
    state.path_mutations.insert(path, state.generation);
}

/// The generation last recorded for `root`, without inspecting the
/// filesystem: it moves with RustCode's own writes and with whatever the last
/// [`snapshot`] found, and is zero for a workspace never inspected.
pub(crate) fn known_generation(root: &Path) -> u64 {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    WORKSPACES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&root)
        .map_or(0, |state| state.generation)
}

pub(crate) fn snapshot(root: &Path) -> Result<Snapshot, String> {
    snapshot_with_budget(root, MAX_DISCOVER_ENTRIES)
}

/// Generation observed recently without touching the filesystem when possible.
/// Informational call sites (evidence identity, read-cache invalidation checks)
/// use this to share one refresh per turn instead of each triggering their own
/// directory-stamp scan on oversized workspaces. Returns the canonicalized
/// root alongside the generation so callers key state exactly like
/// [`snapshot`] does.
pub(crate) fn fresh_generation(
    root: &Path,
    max_age: std::time::Duration,
) -> Option<(PathBuf, u64)> {
    let root = root.canonicalize().ok()?;
    let workspaces = WORKSPACES.lock().unwrap_or_else(|e| e.into_inner());
    let state = workspaces.get(&root)?;
    if !state.initialized {
        return None;
    }
    let recent = state.last_snapshot.is_some_and(|at| at.elapsed() < max_age);
    recent.then(|| (root, state.generation))
}

fn snapshot_with_budget(root: &Path, budget: usize) -> Result<Snapshot, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut workspaces = WORKSPACES.lock().unwrap_or_else(|e| e.into_inner());
    let state = workspaces.entry(root.clone()).or_default();
    let churned = state
        .directories
        .iter()
        .chain(state.pruned.iter())
        .any(|(p, previous)| stamp(p).as_ref() != Some(previous));
    // A truncated workspace re-discovers on background churn at most once per
    // hold window; our own writes go through invalidate()/invalidate_path(),
    // which reset `initialized` and always rediscover immediately.
    let hold_expired = state
        .last_discovery
        .is_none_or(|at| at.elapsed() >= TRUNCATED_REDISCOVERY_HOLD);
    let was_truncated = !state.pruned.is_empty();
    let discover = !state.initialized || (churned && (!was_truncated || hold_expired));
    let mut files = BTreeMap::new();
    let mut topology_changed = false;
    if discover {
        let walk_started = std::time::Instant::now();
        let mut directories = BTreeMap::new();
        let mut pruned: BTreeMap<PathBuf, Stamp> = BTreeMap::new();
        let mut cache_safe = true;
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let pruned_overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Include hidden and ignored configuration inputs; exclude generated
        // dependency/build trees, which would invalidate their own checks.
        // Past the entry budget, subtrees are stamped-but-not-descended so one
        // walk stays bounded no matter how large the workspace is.
        // `filter_entry(false)` hides an entry *and* its descendants from the
        // iterator, so over-budget directories are stamped inside the filter
        // and handed back through a thread-local (the serial walker runs on
        // this thread; `filter_entry` must be `Fn + 'static`).
        let walker = ignore::WalkBuilder::new(&root)
            .standard_filters(false)
            .follow_links(false)
            .filter_entry({
                let seen = std::sync::Arc::clone(&seen);
                let pruned_overflow = std::sync::Arc::clone(&pruned_overflow);
                move |entry: &ignore::DirEntry| {
                    if entry.depth() == 0 {
                        return true;
                    }
                    if matches!(
                        entry.file_name().to_str(),
                        Some(".git" | "target" | "node_modules" | ".rustcode")
                    ) {
                        return false;
                    }
                    // The workspace root's direct children are always
                    // enumerated: project files, manifests and ignore files
                    // live here, and there are few of them in practice. (A
                    // pathological flat directory is still capped.)
                    if entry.depth() == 1 {
                        let n = seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if n < MAX_DEPTH1_ENTRIES {
                            return true;
                        }
                        return false;
                    }
                    let n = seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if n < budget {
                        return true;
                    }
                    if entry.file_type().is_some_and(|t| t.is_dir()) {
                        PRUNED_DIR_TX.with(|tx| {
                            if tx.borrow().len() < MAX_PRUNED_DIRS {
                                if let Some(value) = stamp(entry.path()) {
                                    tx.borrow_mut().push((entry.path().to_path_buf(), value));
                                }
                            } else {
                                pruned_overflow.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        });
                    }
                    false
                }
            })
            .build();
        for entry in walker {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().to_path_buf();
            if entry.file_type().is_some_and(|t| t.is_symlink()) {
                cache_safe = false;
            }
            let Some(value) = stamp(&path) else {
                return Err(format!(
                    "workspace changed while inspecting {}",
                    path.display()
                ));
            };
            if entry.file_type().is_some_and(|t| t.is_dir()) {
                directories.insert(path, value);
            } else {
                files.insert(path, value);
            }
        }
        PRUNED_DIR_TX.with(|tx| {
            for (path, value) in tx.borrow_mut().drain(..) {
                // A directory fully enumerated before the budget ran out stays
                // tracked normally; only genuinely undesceded subtrees are
                // kept as pruned frontiers.
                if !directories.contains_key(&path) {
                    pruned.insert(path, value);
                }
            }
        });
        if pruned_overflow.load(std::sync::atomic::Ordering::Relaxed) {
            // Too many pruned subtrees to track individually: fall back to the
            // root stamp so any top-level change still triggers re-discovery.
            if let Some(value) = stamp(&root) {
                pruned.insert(root.clone(), value);
            }
        }
        let previous_keys: Vec<&PathBuf> = state
            .directories
            .keys()
            .chain(state.pruned.keys())
            .collect();
        let next_keys: Vec<&PathBuf> = directories.keys().chain(pruned.keys()).collect();
        topology_changed = previous_keys != next_keys;
        state.discovery = state.discovery.saturating_add(1);
        state.directories = directories;
        state.pruned = pruned;
        state.cache_safe = cache_safe;
        state.last_discovery = Some(std::time::Instant::now());
        let walk_us = walk_started.elapsed().as_micros() as u64;
        let truncated = !state.pruned.is_empty();
        if truncated || walk_us > SLOW_SNAPSHOT_US {
            crate::logger::operational_event(
                "workspace.snapshot_bounded",
                serde_json::json!({
                    "root": root.display().to_string(),
                    "entries": state.directories.len() + state.files.len(),
                    "pruned_dirs": state.pruned.len(),
                    "truncated": truncated,
                    "walk_us": walk_us,
                    "home_root": home_directory_is(&root),
                }),
            );
        }
    } else {
        for path in state.files.keys() {
            if let Some(value) = stamp(path) {
                files.insert(path.clone(), value);
            }
        }
    }
    for (path, stamp) in &mut files {
        stamp.mutation = state
            .mutation
            .max(state.path_mutations.get(path).copied().unwrap_or(0));
    }
    if state.initialized
        && files.iter().any(|(path, stamp)| {
            matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some(".gitignore" | ".ignore")
            ) && state.files.get(path) != Some(stamp)
        })
    {
        state.discovery = state.discovery.saturating_add(1);
    }
    if !state.initialized || topology_changed || files != state.files {
        state.generation = state.generation.saturating_add(1);
    }
    state.initialized = true;
    state.files = files;
    state.last_snapshot = Some(std::time::Instant::now());
    Ok(Snapshot {
        root,
        generation: state.generation,
        cache_safe: state.cache_safe,
        discovery: state.discovery,
        files: state.files.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisions_detect_external_edit_create_delete_and_explicit_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        std::fs::write(&file, "first").unwrap();
        let first = snapshot(dir.path()).unwrap();
        assert_eq!(first.generation, snapshot(dir.path()).unwrap().generation);
        std::fs::write(&file, "other").unwrap();
        let edited = snapshot(dir.path()).unwrap();
        assert!(edited.generation > first.generation);
        let child = dir.path().join("nested");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("new.rs"), "new").unwrap();
        let created = snapshot(dir.path()).unwrap();
        assert!(created.generation > edited.generation);
        std::fs::remove_file(&file).unwrap();
        let deleted = snapshot(dir.path()).unwrap();
        assert!(deleted.generation > created.generation);
        invalidate(dir.path());
        assert!(snapshot(dir.path()).unwrap().generation > deleted.generation);
    }
    #[test]
    fn oversized_workspace_truncates_walk_but_tracks_tracked_edits() {
        let dir = tempfile::tempdir().unwrap();
        // Root holds exactly two children so both are always enumerated
        // regardless of readdir order; the bulk subtree overflows the budget.
        std::fs::write(dir.path().join("tracked.rs"), "first").unwrap();
        let bulk = dir.path().join("bulk");
        std::fs::create_dir(&bulk).unwrap();
        for i in 0..16 {
            let sub = bulk.join(format!("sub{i:02}"));
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(sub.join("data.bin"), "x").unwrap();
        }
        let first = snapshot_with_budget(dir.path(), 4).unwrap();
        assert_eq!(first.generation, snapshot(dir.path()).unwrap().generation);
        // Edits inside the tracked prefix still advance the generation.
        std::fs::write(dir.path().join("tracked.rs"), "other").unwrap();
        let edited = snapshot_with_budget(dir.path(), 4).unwrap();
        assert!(edited.generation > first.generation);
        // A fresh generation is served without a rescan inside the window.
        let (_, fresh) = fresh_generation(dir.path(), std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(fresh, edited.generation);
    }

    #[test]
    fn truncated_workspace_holds_rediscovery_against_background_churn() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tracked.rs"), "first").unwrap();
        let bulk = dir.path().join("bulk");
        std::fs::create_dir(&bulk).unwrap();
        for i in 0..16 {
            let sub = bulk.join(format!("sub{i:02}"));
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(sub.join("data.bin"), "x").unwrap();
        }
        let first = snapshot_with_budget(dir.path(), 4).unwrap();
        let discovery = first.discovery;
        // Background churn visible at the tracked root must not trigger an
        // immediate full re-discovery while the hold window is active.
        std::fs::write(dir.path().join("churn.txt"), "churn").unwrap();
        let second = snapshot_with_budget(dir.path(), 4).unwrap();
        assert_eq!(second.discovery, discovery);
        assert_eq!(second.generation, first.generation);
        // Our own writes always bypass the hold.
        invalidate(dir.path());
        let third = snapshot_with_budget(dir.path(), 4).unwrap();
        assert!(third.discovery > discovery);
    }

    #[test]
    fn known_mutations_only_invalidate_the_affected_file_without_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.rs");
        let other = dir.path().join("other.rs");
        std::fs::write(&first, "first").unwrap();
        std::fs::write(&other, "other").unwrap();
        let first = first.canonicalize().unwrap();
        let other = other.canonicalize().unwrap();
        let initial = snapshot(dir.path()).unwrap();
        invalidate_path(dir.path(), &first);
        let changed = snapshot(dir.path()).unwrap();
        assert!(changed.generation > initial.generation);
        assert_eq!(changed.discovery, initial.discovery);
        assert_ne!(changed.files[&first], initial.files[&first]);
        assert_eq!(changed.files[&other], initial.files[&other]);
        assert_eq!(changed.generation, snapshot(dir.path()).unwrap().generation);
    }

    #[cfg(unix)]
    #[test]
    fn external_symlink_target_changes_advance_generation() {
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let target = external.path().join("actual.rs");
        std::fs::write(&target, "pub fn first() {}\n").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("linked.rs")).unwrap();
        let initial = snapshot(dir.path()).unwrap();
        assert!(!initial.cache_safe);
        std::fs::write(&target, "pub fn other() {}\n").unwrap();
        assert!(snapshot(dir.path()).unwrap().generation > initial.generation);
    }

    #[test]
    fn hidden_configuration_invalidates_but_build_output_does_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::create_dir(dir.path().join(".cargo")).unwrap();
        let config = dir.path().join(".cargo/config.toml");
        std::fs::write(&config, "first").unwrap();
        let initial = snapshot(dir.path()).unwrap();
        std::fs::write(dir.path().join("target/result"), "build").unwrap();
        assert_eq!(initial.generation, snapshot(dir.path()).unwrap().generation);
        std::fs::write(config, "other").unwrap();
        assert!(snapshot(dir.path()).unwrap().generation > initial.generation);
    }
}

/// Git administrative state is outside the source generation. Read the small
/// refs tree plus HEAD/index/config so external checkout/commit/staging updates
/// invalidate environment snapshots without spawning Git on every query.
pub(crate) fn git_revision(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let marker = root
        .ancestors()
        .map(|p| p.join(".git"))
        .find(|p| p.exists());
    let Some(marker) = marker else {
        return hash.finish();
    };
    let git_dir = if marker.is_dir() {
        marker
    } else {
        let content = std::fs::read_to_string(&marker).unwrap_or_default();
        content.hash(&mut hash);
        let Some(path) = content.trim().strip_prefix("gitdir: ") else {
            return hash.finish();
        };
        marker.parent().unwrap_or(root).join(path)
    };
    let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|p| git_dir.join(p.trim()))
        .unwrap_or_else(|| git_dir.clone());
    for dir in [git_dir, common_dir] {
        for name in [
            "HEAD",
            "index",
            "packed-refs",
            "config",
            "shallow",
            "info/exclude",
        ] {
            let path = dir.join(name);
            path.hash(&mut hash);
            if let Some(value) = stamp(&path) {
                format!("{value:?}").hash(&mut hash);
            }
        }
        for entry in ignore::WalkBuilder::new(dir.join("refs"))
            .standard_filters(false)
            .build()
            .flatten()
        {
            entry.path().hash(&mut hash);
            if let Some(value) = stamp(entry.path()) {
                format!("{value:?}").hash(&mut hash);
            }
        }
    }
    hash.finish()
}

pub(crate) fn directory_revision(root: &Path) -> String {
    format!("{:?}", stamp(root))
}
