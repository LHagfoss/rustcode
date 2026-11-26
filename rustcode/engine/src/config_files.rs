//! Skills, themes and prompt commands copied between paired machines (#4485).
//!
//! The config directory's `skills/`, `themes/` and `commands/` travel over the
//! paired connection, one entry per request so every frame stays under the
//! gateway's 1 MiB limit: a skill folder with every file in it, a theme's
//! `.toml` or a command's `.md`. A machine that follows another, or fetches
//! from it once, lists both sides, copies the entries whose content differs
//! and prunes the copies it took from that machine before that are gone there.
//!
//! `followed-files.json` in the config directory records which entries came
//! from which machine. Pruning removes only those, so an entry the receiving
//! machine made itself under a name the source does not have is never touched.
//! Hidden files and symlinks never travel, and nothing outside these three
//! folders does: credentials, accounts and `~/.agents/skills` stay put.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The most an entry's files may hold together, so one entry fits one frame
/// after base64.
pub const ENTRY_LIMIT: u64 = 512 * 1024;
/// The most files one skill folder may carry.
pub const FILE_COUNT_LIMIT: usize = 200;
/// The path an entry that is a single file gives its one file.
pub const SINGLE_FILE: &str = ".";
const ORIGINS_FILE: &str = "followed-files.json";

/// One of the config directory's folders that travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigKind {
    Skills,
    Themes,
    Commands,
}

impl ConfigKind {
    pub const ALL: [ConfigKind; 3] = [Self::Skills, Self::Themes, Self::Commands];

    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Themes => "themes",
            Self::Commands => "commands",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.dir_name() == value.trim())
    }

    /// A skill is a folder with a `SKILL.md`; a theme and a command are one
    /// file each.
    fn is_dir(self) -> bool {
        matches!(self, Self::Skills)
    }

    fn file_suffix(self) -> &'static str {
        match self {
            Self::Skills => "",
            Self::Themes => ".toml",
            Self::Commands => ".md",
        }
    }
}

/// An entry as a listing names it: what is compared before anything is copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntrySummary {
    pub kind: ConfigKind,
    pub name: String,
    /// sha256 over every file's path and bytes; equal on both machines means
    /// there is nothing to copy.
    pub hash: String,
    pub size: u64,
    /// Why this entry cannot be copied (too large, too many files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// One file of an entry, its bytes in standard base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryFile {
    /// `/`-separated and relative to the skill folder, or [`SINGLE_FILE`].
    pub path: String,
    pub content: String,
}

/// An entry with its content, as it travels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: ConfigKind,
    pub name: String,
    pub files: Vec<EntryFile>,
}

/// What one put did on the receiving machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutOutcome {
    Written,
    Unchanged,
}

/// An entry's files as `(relative path, bytes)`.
type Files = Vec<(String, Vec<u8>)>;

type Origins = BTreeMap<String, BTreeMap<String, BTreeSet<String>>>;

fn valid_name(kind: ConfigKind, name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0', ':'])
        && name.ends_with(kind.file_suffix())
        && name.len() > kind.file_suffix().len()
}

fn kind_dir(config_dir: &Path, kind: ConfigKind) -> PathBuf {
    config_dir.join(kind.dir_name())
}

/// Every entry of `kinds` in `config_dir`, sorted by kind and name.
pub fn list(config_dir: &Path, kinds: &[ConfigKind]) -> Vec<EntrySummary> {
    let mut out = Vec::new();
    for &kind in kinds {
        let Ok(items) = std::fs::read_dir(kind_dir(config_dir, kind)) else {
            continue;
        };
        let mut names: Vec<String> = items
            .flatten()
            .filter_map(|item| item.file_name().into_string().ok())
            .filter(|name| valid_name(kind, name))
            .collect();
        names.sort();
        for name in names {
            let path = kind_dir(config_dir, kind).join(&name);
            let summary = match collect_files(kind, &path) {
                Ok(Some(files)) => summarize(kind, &name, &files),
                Ok(None) => continue,
                Err(reason) => EntrySummary {
                    kind,
                    name,
                    hash: String::new(),
                    size: 0,
                    skipped: Some(reason),
                },
            };
            out.push(summary);
        }
    }
    out
}

/// One entry with its content, for the machine that copies it.
pub fn read(config_dir: &Path, kind: ConfigKind, name: &str) -> Result<Entry, String> {
    if !valid_name(kind, name) {
        return Err(format!("'{name}' is not a {} entry", kind.dir_name()));
    }
    let path = kind_dir(config_dir, kind).join(name);
    let files = collect_files(kind, &path)?
        .ok_or_else(|| format!("there is no {} entry named '{name}'", kind.dir_name()))?;
    let engine = base64::engine::general_purpose::STANDARD;
    Ok(Entry {
        kind,
        name: name.to_owned(),
        files: files
            .into_iter()
            .map(|(path, bytes)| EntryFile {
                path,
                content: engine.encode(bytes),
            })
            .collect(),
    })
}

/// The entry's files as `(relative path, bytes)`, sorted by path; `None` when
/// it is not an entry of this kind (a file in `skills/`, a skill folder with
/// no `SKILL.md`, a symlink).
fn collect_files(kind: ConfigKind, path: &Path) -> Result<Option<Files>, String> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if kind.is_dir() != meta.is_dir() || meta.file_type().is_symlink() {
        return Ok(None);
    }
    let mut files = Vec::new();
    if kind.is_dir() {
        if !path.join("SKILL.md").is_file() {
            return Ok(None);
        }
        walk(path, "", &mut files)?;
    } else {
        check_size(meta.len())?;
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        files.push((SINGLE_FILE.to_owned(), bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Some(files))
}

fn check_size(size: u64) -> Result<(), String> {
    if size > ENTRY_LIMIT {
        return Err(format!(
            "larger than {} KiB, the most one copy carries",
            ENTRY_LIMIT / 1024
        ));
    }
    Ok(())
}

fn walk(dir: &Path, prefix: &str, files: &mut Files) -> Result<(), String> {
    let items = std::fs::read_dir(dir).map_err(|error| error.to_string())?;
    for item in items.flatten() {
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if file_type.is_dir() {
            walk(&item.path(), &relative, files)?;
        } else if file_type.is_file() {
            if files.len() >= FILE_COUNT_LIMIT {
                return Err(format!(
                    "more than {FILE_COUNT_LIMIT} files, the most one copy carries"
                ));
            }
            let bytes = std::fs::read(item.path()).map_err(|error| error.to_string())?;
            let total: u64 = files.iter().map(|(_, b)| b.len() as u64).sum();
            check_size(total + bytes.len() as u64)?;
            files.push((relative, bytes));
        }
    }
    Ok(())
}

fn summarize(kind: ConfigKind, name: &str, files: &[(String, Vec<u8>)]) -> EntrySummary {
    EntrySummary {
        kind,
        name: name.to_owned(),
        hash: content_hash(files),
        size: files.iter().map(|(_, bytes)| bytes.len() as u64).sum(),
        skipped: None,
    }
}

fn content_hash(files: &[(String, Vec<u8>)]) -> String {
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hex::encode(hasher.finalize())
}

fn valid_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', '\0', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'))
}

fn decode(kind: ConfigKind, entry: &Entry) -> Result<Files, String> {
    let engine = base64::engine::general_purpose::STANDARD;
    if !kind.is_dir() && (entry.files.len() != 1 || entry.files[0].path != SINGLE_FILE) {
        return Err(format!("a {} entry is one file", kind.dir_name()));
    }
    if entry.files.len() > FILE_COUNT_LIMIT {
        return Err(format!("more than {FILE_COUNT_LIMIT} files"));
    }
    let mut files = Vec::with_capacity(entry.files.len());
    let mut total = 0u64;
    for file in &entry.files {
        if kind.is_dir() && !valid_relative(&file.path) {
            return Err(format!("'{}' is not a path inside the skill", file.path));
        }
        let bytes = engine
            .decode(file.content.as_bytes())
            .map_err(|error| format!("'{}' is not base64: {error}", file.path))?;
        total += bytes.len() as u64;
        check_size(total)?;
        files.push((file.path.clone(), bytes));
    }
    if kind.is_dir() && !files.iter().any(|(path, _)| path == "SKILL.md") {
        return Err("a skill needs its SKILL.md".to_owned());
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    Ok(files)
}

/// Write `entry` as the copy `source` sent, replacing what this machine had
/// under that name, and record that it came from `source`.
pub fn put(config_dir: &Path, source: &str, entry: &Entry) -> Result<PutOutcome, String> {
    let kind = entry.kind;
    let name = entry.name.as_str();
    if !valid_name(kind, name) {
        return Err(format!("'{name}' is not a {} entry", kind.dir_name()));
    }
    if source.trim().is_empty() {
        return Err("the copy does not say which machine it came from".to_owned());
    }
    let files = decode(kind, entry)?;
    let target = kind_dir(config_dir, kind).join(name);
    let current = collect_files(kind, &target).ok().flatten();
    let outcome = if current.is_some_and(|current| content_hash(&current) == content_hash(&files)) {
        PutOutcome::Unchanged
    } else {
        write_entry(config_dir, kind, name, &files)?;
        PutOutcome::Written
    };
    let mut origins = load_origins(config_dir);
    for kinds in origins.values_mut() {
        if let Some(names) = kinds.get_mut(kind.dir_name()) {
            names.remove(name);
        }
    }
    origins
        .entry(source.to_owned())
        .or_default()
        .entry(kind.dir_name().to_owned())
        .or_default()
        .insert(name.to_owned());
    save_origins(config_dir, &origins)?;
    if outcome == PutOutcome::Written && kind == ConfigKind::Skills {
        crate::skills::bump_skill_catalog_generation();
    }
    Ok(outcome)
}

fn write_entry(
    config_dir: &Path,
    kind: ConfigKind,
    name: &str,
    files: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let parent = kind_dir(config_dir, kind);
    std::fs::create_dir_all(&parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    let target = parent.join(name);
    if !kind.is_dir() {
        return crate::atomic_file::write_described(&target, &files[0].1);
    }
    let staging = parent.join(format!(".incoming-{name}"));
    let replaced = parent.join(format!(".replaced-{name}"));
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&replaced);
    for (path, bytes) in files {
        let file = staging.join(path);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
        }
        std::fs::write(&file, bytes)
            .map_err(|error| format!("could not write {}: {error}", file.display()))?;
    }
    if std::fs::symlink_metadata(&target).is_ok() {
        std::fs::rename(&target, &replaced)
            .map_err(|error| format!("could not replace {}: {error}", target.display()))?;
    }
    std::fs::rename(&staging, &target)
        .map_err(|error| format!("could not write {}: {error}", target.display()))?;
    let _ = std::fs::remove_dir_all(&replaced);
    Ok(())
}

/// Remove the `kind` entries this machine took from `source` that `keep` no
/// longer names. Answers the names removed.
pub fn prune(
    config_dir: &Path,
    source: &str,
    kind: ConfigKind,
    keep: &[String],
) -> Result<Vec<String>, String> {
    let mut origins = load_origins(config_dir);
    let Some(names) = origins
        .get_mut(source)
        .and_then(|kinds| kinds.get_mut(kind.dir_name()))
    else {
        return Ok(Vec::new());
    };
    let gone: Vec<String> = names
        .iter()
        .filter(|name| !keep.contains(name))
        .cloned()
        .collect();
    for name in &gone {
        names.remove(name);
        if !valid_name(kind, name) {
            continue;
        }
        let path = kind_dir(config_dir, kind).join(name);
        let removed = if kind.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(error) = removed
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(format!("could not remove {}: {error}", path.display()));
        }
    }
    save_origins(config_dir, &origins)?;
    if !gone.is_empty() && kind == ConfigKind::Skills {
        crate::skills::bump_skill_catalog_generation();
    }
    Ok(gone)
}

fn load_origins(config_dir: &Path) -> Origins {
    std::fs::read(config_dir.join(ORIGINS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_origins(config_dir: &Path, origins: &Origins) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(origins).map_err(|error| error.to_string())?;
    crate::atomic_file::write_described(&config_dir.join(ORIGINS_FILE), &bytes)
}

/// What one copy from another machine did here, counted per outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyReport {
    pub written: Vec<String>,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
}

impl CopyReport {
    pub fn summary(&self) -> String {
        if self.written.is_empty() && self.removed.is_empty() {
            return "already up to date".to_owned();
        }
        let mut parts = Vec::new();
        if !self.written.is_empty() {
            parts.push(format!("copied {}", self.written.join(", ")));
        }
        if !self.removed.is_empty() {
            parts.push(format!("removed {}", self.removed.join(", ")));
        }
        parts.join("; ")
    }
}

/// The entries of another machine's listing that differ here, so the caller
/// reads only those over its connection before [`apply`].
pub fn plan(
    config_dir: &Path,
    kinds: &[ConfigKind],
    remote: &[EntrySummary],
) -> Vec<(ConfigKind, String)> {
    let local = local_hashes(config_dir, kinds);
    remote
        .iter()
        .filter(|entry| kinds.contains(&entry.kind) && entry.skipped.is_none())
        .filter(|entry| local.get(&(entry.kind, entry.name.clone())) != Some(&entry.hash))
        .map(|entry| (entry.kind, entry.name.clone()))
        .collect()
}

fn local_hashes(config_dir: &Path, kinds: &[ConfigKind]) -> BTreeMap<(ConfigKind, String), String> {
    list(config_dir, kinds)
        .into_iter()
        .map(|entry| ((entry.kind, entry.name), entry.hash))
        .collect()
}

/// Bring `config_dir` up to date with `source`'s listing: write the entries
/// [`plan`] asked for (`fetched`), record the ones that already match as
/// copies of `source`, and prune the copies of `source` it no longer lists.
/// `rc config sync from` uses it; the app runs the same steps request by
/// request over its own connections.
pub fn apply(
    config_dir: &Path,
    source: &str,
    kinds: &[ConfigKind],
    remote: &[EntrySummary],
    fetched: &[Entry],
) -> Result<CopyReport, String> {
    let local = local_hashes(config_dir, kinds);
    let mut report = CopyReport::default();
    for &kind in kinds {
        let mut keep = Vec::new();
        for entry in remote.iter().filter(|entry| entry.kind == kind) {
            keep.push(entry.name.clone());
            let label = format!("{}/{}", kind.dir_name(), entry.name);
            if let Some(reason) = &entry.skipped {
                report.skipped.push(format!("{label} ({reason})"));
                continue;
            }
            let incoming = fetched
                .iter()
                .find(|copy| copy.kind == kind && copy.name == entry.name);
            let outcome = match incoming {
                Some(copy) => put(config_dir, source, copy)?,
                None if local.get(&(kind, entry.name.clone())) == Some(&entry.hash) => {
                    put_unchanged(config_dir, source, kind, &entry.name)?
                }
                None => {
                    report.skipped.push(format!("{label} (not read)"));
                    continue;
                }
            };
            if outcome == PutOutcome::Written {
                report.written.push(label);
            }
        }
        for name in prune(config_dir, source, kind, &keep)? {
            report.removed.push(format!("{}/{name}", kind.dir_name()));
        }
    }
    Ok(report)
}

/// Record an entry that already matches `source` as its copy, so a later
/// delete there removes it here too.
fn put_unchanged(
    config_dir: &Path,
    source: &str,
    kind: ConfigKind,
    name: &str,
) -> Result<PutOutcome, String> {
    let entry = read(config_dir, kind, name)?;
    put(config_dir, source, &entry)
}

#[cfg(test)]
#[path = "config_files_tests.rs"]
mod tests;
