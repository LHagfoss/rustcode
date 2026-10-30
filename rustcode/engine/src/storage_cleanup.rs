//! Retention for the generated file stores (#2599): `tool_output/` (the
//! full output of commands too long for the conversation) and
//! `attachments/` (images attached to messages). Thread histories refer to
//! these files by name, so a file is a candidate only when nothing under
//! `threads/` or `history_archive/` names it and it has not changed for
//! `min_age`. Nothing else in the configuration directory is touched.
//!
//! Removal is recoverable: candidates move to `storage/removed/<time>/`,
//! with their store as a subfolder, and a later clean deletes such a folder
//! once it is `keep_removed` old. `plan` changes nothing.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The stores cleaned, as directories of the configuration directory.
pub const STORES: [&str; 2] = ["tool_output", "attachments"];
/// Where the files that name store files live.
const REFERRERS: [&str; 2] = ["threads", "history_archive"];
/// Where removed files wait before they are deleted.
const REMOVED: &str = "storage/removed";

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// A file changed more recently than this is kept, referenced or not.
    pub min_age: Duration,
    /// A folder of removed files is deleted once this old.
    pub keep_removed: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            min_age: Duration::from_secs(30 * 24 * 3600),
            keep_removed: Duration::from_secs(14 * 24 * 3600),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Candidate {
    pub store: &'static str,
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StoreSummary {
    pub store: &'static str,
    pub files: usize,
    pub referenced: usize,
    pub recent: usize,
    pub candidates: usize,
    pub candidate_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CleanPlan {
    pub config_dir: PathBuf,
    pub min_age_days: u64,
    pub stores: Vec<StoreSummary>,
    pub candidates: Vec<Candidate>,
    /// Folders under `storage/removed` old enough to delete.
    pub expired_removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CleanResult {
    pub moved: usize,
    pub moved_bytes: u64,
    /// Where the moved files are, to restore them.
    pub removed_dir: Option<PathBuf>,
    pub deleted_removed: Vec<String>,
    pub errors: Vec<String>,
}

/// File names a text names: a run of name characters ending in an
/// extension. Generous on purpose: a false match keeps a file.
fn names_in(text: &str, into: &mut HashSet<String>) {
    static NAME: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let name = NAME.get_or_init(|| {
        regex::Regex::new(r"[A-Za-z0-9_.\-]+\.[A-Za-z0-9]{1,5}").expect("valid file-name pattern")
    });
    for found in name.find_iter(text) {
        into.insert(found.as_str().to_owned());
    }
}

/// Every name any file under the referrer folders mentions. Files are read
/// as bytes, so a binary file or bad UTF-8 is scanned as far as it reads.
fn referenced(config_dir: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut stack: Vec<PathBuf> = REFERRERS.iter().map(|dir| config_dir.join(dir)).collect();
    while let Some(path) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                stack.extend(entries.flatten().map(|entry| entry.path()));
            }
        } else if let Ok(bytes) = std::fs::read(&path) {
            names_in(&String::from_utf8_lossy(&bytes), &mut names);
        }
    }
    names
}

fn age(meta: &std::fs::Metadata, now: SystemTime) -> Duration {
    meta.modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .unwrap_or_default()
}

/// What a clean would remove now, changing nothing.
pub fn plan(config_dir: &Path, policy: Policy) -> CleanPlan {
    plan_at(config_dir, policy, SystemTime::now())
}

fn plan_at(config_dir: &Path, policy: Policy, now: SystemTime) -> CleanPlan {
    let referenced = referenced(config_dir);
    let mut stores = Vec::new();
    let mut candidates = Vec::new();
    for store in STORES {
        let mut summary = StoreSummary {
            store,
            files: 0,
            referenced: 0,
            recent: 0,
            candidates: 0,
            candidate_bytes: 0,
        };
        let mut entries: Vec<_> = std::fs::read_dir(config_dir.join(store))
            .map(|entries| entries.flatten().collect())
            .unwrap_or_default();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let Ok(meta) = entry.metadata() else { continue };
            // Only plain files the store wrote: never a folder or a link.
            if !meta.is_file() {
                continue;
            }
            summary.files += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            if referenced.contains(&name) {
                summary.referenced += 1;
            } else if age(&meta, now) < policy.min_age {
                summary.recent += 1;
            } else {
                summary.candidates += 1;
                summary.candidate_bytes += meta.len();
                candidates.push(Candidate {
                    store,
                    name,
                    bytes: meta.len(),
                });
            }
        }
        stores.push(summary);
    }
    let mut expired_removed: Vec<String> = std::fs::read_dir(config_dir.join(REMOVED))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .metadata()
                        .is_ok_and(|meta| meta.is_dir() && age(&meta, now) >= policy.keep_removed)
                })
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    expired_removed.sort();
    CleanPlan {
        config_dir: config_dir.to_path_buf(),
        min_age_days: policy.min_age.as_secs() / 86_400,
        stores,
        candidates,
        expired_removed,
    }
}

/// Carry out `plan`: move its candidates to a new folder under
/// `storage/removed`, then delete the expired removed folders. A file that
/// changed or became referenced since the plan is not re-checked here, so
/// apply a fresh plan.
pub fn apply(plan: &CleanPlan) -> CleanResult {
    let config_dir = &plan.config_dir;
    let mut result = CleanResult {
        moved: 0,
        moved_bytes: 0,
        removed_dir: None,
        deleted_removed: Vec::new(),
        errors: Vec::new(),
    };
    for name in &plan.expired_removed {
        match std::fs::remove_dir_all(config_dir.join(REMOVED).join(name)) {
            Ok(()) => result.deleted_removed.push(name.clone()),
            Err(error) => result.errors.push(format!("{REMOVED}/{name}: {error}")),
        }
    }
    if plan.candidates.is_empty() {
        return result;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let removed_dir = config_dir.join(REMOVED).join(stamp);
    for candidate in &plan.candidates {
        let from = config_dir.join(candidate.store).join(&candidate.name);
        let to_dir = removed_dir.join(candidate.store);
        let moved = std::fs::create_dir_all(&to_dir)
            .and_then(|()| std::fs::rename(&from, to_dir.join(&candidate.name)));
        match moved {
            Ok(()) => {
                result.moved += 1;
                result.moved_bytes += candidate.bytes;
            }
            Err(error) => result
                .errors
                .push(format!("{}/{}: {error}", candidate.store, candidate.name)),
        }
    }
    if result.moved > 0 {
        result.removed_dir = Some(removed_dir);
    }
    result
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b if b >= 1_000 => format!("{:.1} KB", b as f64 / 1e3),
        b => format!("{b} B"),
    }
}

impl CleanPlan {
    pub fn render(&self) -> String {
        let mut out = format!(
            "Generated files in {} that no thread refers to and that are over {} days old:\n\n",
            self.config_dir.display(),
            self.min_age_days
        );
        for store in &self.stores {
            out.push_str(&format!(
                "  {:<12} {} files: {} referenced, {} recent, {} to remove ({})\n",
                store.store,
                store.files,
                store.referenced,
                store.recent,
                store.candidates,
                size(store.candidate_bytes)
            ));
        }
        if !self.expired_removed.is_empty() {
            out.push_str(&format!(
                "\n{} folder(s) in {REMOVED} are old enough to delete.\n",
                self.expired_removed.len()
            ));
        }
        out.push_str(
            "\nNothing was changed. Run with --apply to move these files to storage/removed, \
             where they can be restored until a clean deletes them 14 days later.\n",
        );
        out
    }
}

impl CleanResult {
    pub fn render(&self) -> String {
        let mut out = format!(
            "Moved {} file(s), {}.\n",
            self.moved,
            size(self.moved_bytes)
        );
        if let Some(dir) = &self.removed_dir {
            out.push_str(&format!(
                "They are in {}; move a file back to its store to restore it.\n",
                dir.display()
            ));
        }
        if !self.deleted_removed.is_empty() {
            out.push_str(&format!(
                "Deleted {} expired folder(s) of removed files.\n",
                self.deleted_removed.len()
            ));
        }
        for error in &self.errors {
            out.push_str(&format!("Not moved: {error}\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn old(path: &Path) {
        let then = SystemTime::now() - Duration::from_secs(60 * 24 * 3600);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(then)
            .unwrap();
    }

    #[test]
    fn only_old_unreferenced_files_are_candidates_and_apply_moves_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let kept = root.join("tool_output/1784627230768_view_file.txt");
        let archived = root.join("tool_output/1784627230999_run_command.txt");
        let orphan = root.join("tool_output/1784849083217_run_command.txt");
        let recent = root.join("tool_output/1799999999999_run_command.txt");
        let image = root.join("attachments/native_1793004934105295000_4.jpg");
        for path in [&kept, &archived, &orphan, &recent, &image] {
            write(path, b"0123456789");
        }
        for path in [&kept, &archived, &orphan, &image] {
            old(path);
        }
        write(
            &root.join("threads/2026/10/01/a/history.json"),
            br#"[{"content":"full output: ~/.config/rustcode/tool_output/1784627230768_view_file.txt"}]"#,
        );
        write(
            &root.join("history_archive/a.json"),
            b"see 1784627230999_run_command.txt",
        );
        // A folder or link in a store is never a candidate.
        std::fs::create_dir_all(root.join("attachments/folder")).unwrap();

        let plan = plan(root, Policy::default());
        let names: Vec<_> = plan.candidates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "1784849083217_run_command.txt",
                "native_1793004934105295000_4.jpg"
            ]
        );
        let tool = &plan.stores[0];
        assert_eq!(
            (tool.files, tool.referenced, tool.recent, tool.candidates),
            (4, 2, 1, 1)
        );
        assert!(plan.render().contains("Nothing was changed"));
        assert!(orphan.exists(), "a plan changes nothing");

        let result = apply(&plan);
        assert_eq!(result.moved, 2);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(!orphan.exists() && kept.exists() && archived.exists() && recent.exists());
        let removed = result.removed_dir.unwrap();
        assert!(
            removed
                .join("tool_output/1784849083217_run_command.txt")
                .exists()
        );
        assert!(
            removed
                .join("attachments/native_1793004934105295000_4.jpg")
                .exists()
        );
    }

    #[test]
    fn removed_folders_expire_after_the_keep_period() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("storage/removed/20260901-000000/tool_output")).unwrap();
        std::fs::create_dir_all(root.join("storage/removed/20261030-000000")).unwrap();
        let later = SystemTime::now() + Duration::from_secs(20 * 24 * 3600);
        let policy = Policy {
            min_age: Duration::from_secs(30 * 24 * 3600),
            keep_removed: Duration::from_secs(14 * 24 * 3600),
        };
        // Both are 20 days old seen from `later`; neither is from `now`.
        assert!(plan(root, policy).expired_removed.is_empty());
        let plan = plan_at(root, policy, later);
        assert_eq!(plan.expired_removed.len(), 2);
        let result = apply(&plan);
        assert_eq!(result.deleted_removed.len(), 2);
        assert!(!root.join("storage/removed/20260901-000000").exists());
    }

    #[test]
    fn an_empty_or_missing_config_dir_plans_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan(dir.path(), Policy::default());
        assert!(plan.candidates.is_empty() && plan.expired_removed.is_empty());
        assert_eq!(apply(&plan).moved, 0);
    }
}
