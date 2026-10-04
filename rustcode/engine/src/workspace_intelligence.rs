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
    discovery: u64,
    initialized: bool,
    cache_safe: bool,
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

pub(crate) fn snapshot(root: &Path) -> Result<Snapshot, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut workspaces = WORKSPACES.lock().unwrap_or_else(|e| e.into_inner());
    let state = workspaces.entry(root.clone()).or_default();
    let discover = !state.initialized
        || state
            .directories
            .iter()
            .any(|(p, previous)| stamp(p).as_ref() != Some(previous));
    let mut files = BTreeMap::new();
    let mut topology_changed = false;
    if discover {
        let mut directories = BTreeMap::new();
        let mut cache_safe = true;
        // Include hidden and ignored configuration inputs; exclude generated
        // dependency/build trees, which would invalidate their own checks.
        let walker = ignore::WalkBuilder::new(&root)
            .standard_filters(false)
            .follow_links(false)
            .filter_entry(|entry| {
                entry.depth() == 0
                    || !matches!(
                        entry.file_name().to_str(),
                        Some(".git" | "target" | "node_modules" | ".rustcode")
                    )
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
        topology_changed = !state.directories.keys().eq(directories.keys());
        state.discovery = state.discovery.saturating_add(1);
        state.directories = directories;
        state.cache_safe = cache_safe;
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
