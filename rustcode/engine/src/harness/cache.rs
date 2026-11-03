//! What the engine learned about each external harness, kept across engine
//! restarts in `harness-models.json` in the config directory
//! (`docs/harnesses.md`): its model list (with each model's efforts and
//! default effort) and the capabilities read from its options. A restored
//! thread then offers the full model and effort choice before the harness
//! starts again. An entry older than [`MAX_AGE`], or recorded for another
//! harness version, is still used but refreshed in the background.

use super::{HarnessModel, Learned};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CACHE_FILE: &str = "harness-models.json";
/// How long a cached model list counts as fresh.
pub(crate) const MAX_AGE_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedHarness {
    /// Unix seconds of the listing.
    #[serde(default)]
    pub fetched_at: i64,
    /// The harness version the listing came from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub models: Vec<HarnessModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    harnesses: BTreeMap<String, CachedHarness>,
}

fn path_in(dir: &Path) -> PathBuf {
    dir.join(CACHE_FILE)
}

fn read_in(dir: &Path) -> CacheFile {
    std::fs::read_to_string(path_in(dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The cached entry of `id` in `dir`.
pub(crate) fn load_in(dir: &Path, id: &str) -> Option<CachedHarness> {
    read_in(dir).harnesses.remove(id)
}

/// Record `entry` for `id` in `dir`, keeping the other harnesses' entries.
pub(crate) fn save_in(dir: &Path, id: &str, entry: CachedHarness) {
    let mut file = read_in(dir);
    if file.harnesses.get(id) == Some(&entry) {
        return;
    }
    file.harnesses.insert(id.to_owned(), entry);
    if let Ok(text) = serde_json::to_string(&file) {
        let _ = std::fs::create_dir_all(dir);
        let _ = crate::worktrees::write_atomic(&path_in(dir), text.as_bytes());
    }
}

/// A cached entry as what the engine learned.
pub(crate) fn to_learned(entry: CachedHarness) -> Learned {
    Learned {
        models: (!entry.models.is_empty()).then_some(entry.models),
        error: None,
        resume: entry.resume,
        images: entry.images,
        steer: entry.steer,
        effort: entry.effort,
        fetched_at: Some(entry.fetched_at),
        version: entry.version,
    }
}

/// What of `learned` is worth keeping; `None` when nothing is.
pub(crate) fn from_learned(learned: &Learned) -> Option<CachedHarness> {
    let models = learned.models.clone().unwrap_or_default();
    if models.is_empty()
        && learned.effort.is_none()
        && learned.resume.is_none()
        && learned.images.is_none()
        && learned.steer.is_none()
    {
        return None;
    }
    Some(CachedHarness {
        fetched_at: learned.fetched_at.unwrap_or_default(),
        version: learned.version.clone(),
        models,
        effort: learned.effort,
        resume: learned.resume,
        images: learned.images,
        steer: learned.steer,
    })
}

/// Whether a model list learned at `fetched_at` for `version` should be
/// listed again: too old, or the harness was updated since.
pub(crate) fn is_stale(
    fetched_at: Option<i64>,
    version: Option<&str>,
    current_version: Option<&str>,
    now: i64,
) -> bool {
    let too_old = fetched_at.is_none_or(|at| now - at > MAX_AGE_SECS);
    let updated = matches!((version, current_version), (Some(was), Some(is)) if was != is);
    too_old || updated
}

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> HarnessModel {
        HarnessModel {
            id: id.into(),
            name: id.into(),
            efforts: vec!["low".into(), "high".into()],
            default_effort: Some("high".into()),
        }
    }

    #[test]
    fn entries_persist_per_harness() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_in(dir.path(), "codex").is_none());
        let codex = CachedHarness {
            fetched_at: 100,
            version: Some("0.166.0".into()),
            models: vec![model("gpt-6-luna")],
            effort: Some(true),
            ..Default::default()
        };
        save_in(dir.path(), "codex", codex.clone());
        save_in(
            dir.path(),
            "opencode",
            CachedHarness {
                fetched_at: 200,
                models: vec![model("deepseek/deepseek-v4-pro")],
                ..Default::default()
            },
        );
        assert_eq!(load_in(dir.path(), "codex"), Some(codex.clone()));
        assert_eq!(load_in(dir.path(), "opencode").unwrap().fetched_at, 200);

        let learned = to_learned(codex.clone());
        assert_eq!(
            learned.models.as_ref().unwrap()[0]
                .default_effort
                .as_deref(),
            Some("high")
        );
        assert_eq!(learned.effort, Some(true));
        assert_eq!(from_learned(&learned), Some(codex));
        assert_eq!(from_learned(&Learned::default()), None);
    }

    #[test]
    fn a_broken_cache_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CACHE_FILE), "{not json").unwrap();
        assert!(load_in(dir.path(), "codex").is_none());
        save_in(dir.path(), "codex", CachedHarness::default());
        assert!(load_in(dir.path(), "codex").is_some());
    }

    #[test]
    fn staleness_follows_age_and_version() {
        let now = 10 * MAX_AGE_SECS;
        assert!(!is_stale(Some(now - 60), Some("1.0"), Some("1.0"), now));
        assert!(!is_stale(Some(now - 60), None, Some("1.0"), now));
        assert!(!is_stale(Some(now - 60), Some("1.0"), None, now));
        assert!(is_stale(Some(now - 60), Some("1.0"), Some("1.1"), now));
        assert!(is_stale(
            Some(now - MAX_AGE_SECS - 1),
            Some("1.0"),
            Some("1.0"),
            now
        ));
        assert!(is_stale(None, None, None, now));
    }
}
