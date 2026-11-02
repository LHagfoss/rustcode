//! What a thread on an external harness keeps beside its transcript:
//! `harness.json` in the thread's directory names the harness, the
//! harness's own session id, the working directory, title, model and
//! effort, and when it started and last changed.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const RECORD_FILE: &str = "harness.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HarnessRecord {
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session_id: Option<String>,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl HarnessRecord {
    pub(crate) fn new(harness: &str, cwd: PathBuf) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            harness: harness.to_owned(),
            cwd,
            created_at: now.clone(),
            updated_at: now,
            ..Self::default()
        }
    }
}

fn record_path(session_id: &str) -> Option<PathBuf> {
    if !rustcode_session::valid_session_id(session_id) {
        return None;
    }
    crate::config::get_active_session_dir(session_id).map(|dir| dir.join(RECORD_FILE))
}

/// The record of `session_id`, when it runs on an external harness.
pub(crate) fn load(session_id: &str) -> Option<HarnessRecord> {
    let text = std::fs::read_to_string(record_path(session_id)?).ok()?;
    serde_json::from_str::<HarnessRecord>(&text)
        .ok()
        .filter(|record| !record.harness.is_empty() && record.harness != super::BUILTIN_ID)
}

pub(crate) fn save(session_id: &str, record: &HarnessRecord) {
    let Some(path) = record_path(session_id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(text) = serde_json::to_string_pretty(record) else {
        return;
    };
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, text).is_ok() && std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_beside_the_thread() {
        let session_id = crate::config::create_detached_session();
        assert!(load(&session_id).is_none());
        let mut record = HarnessRecord::new("codex", PathBuf::from("/tmp/project"));
        record.native_session_id = Some("thread-1".into());
        record.model = Some("gpt-6-luna".into());
        save(&session_id, &record);
        assert_eq!(load(&session_id), Some(record));

        let builtin = HarnessRecord::new("rustcode", PathBuf::from("/tmp"));
        save(&session_id, &builtin);
        assert!(
            load(&session_id).is_none(),
            "built-in threads have no record"
        );
    }
}
