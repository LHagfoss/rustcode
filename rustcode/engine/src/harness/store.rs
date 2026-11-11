//! What a thread on an external harness keeps beside its transcript:
//! `harness.json` in the thread's directory names the harness, the
//! harness's own session id, the working directory, title, model and
//! effort, and when it started and last changed.
//!
//! This file is what a thread resumes from, so it survives everything that
//! stops a process: a harness that died, a turn torn down for silence, or the
//! idle reaper stopping a process nobody is using (`session.rs`). Reaping
//! changes nothing here — the next start resumes exactly what the thread had.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const RECORD_FILE: &str = "harness.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HarnessRecord {
    pub harness: String,
    /// The named account of that harness the thread runs on, when it runs on
    /// one; its conversation lives in that account's config directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session_id: Option<String>,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The thread's latest `usage_update`, sent again on `session/load`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
    /// The harness the thread was switched from; its next prompt carries the
    /// conversation so far (`harness/handoff.rs`), then this clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_from: Option<String>,
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

/// Restoration must never turn an unreadable external record into a built-in
/// thread. Keep the saved harness, account and native id, or report the error.
pub(crate) fn load_for_restore(session_id: &str) -> Result<Option<HarnessRecord>, String> {
    let path = record_path(session_id).ok_or("session storage is unavailable")?;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read saved harness: {error}")),
    };
    let record: HarnessRecord = serde_json::from_str(&text)
        .map_err(|error| format!("cannot read saved harness: {error}"))?;
    if record.harness.is_empty() {
        return Err("saved harness is empty".into());
    }
    Ok((record.harness != super::BUILTIN_ID).then_some(record))
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
    fn restart_keeps_the_saved_login_and_native_session_or_reports_damage() {
        let id = crate::config::create_detached_session();
        let mut record = HarnessRecord::new("claude-code", std::env::temp_dir());
        record.account = Some("personal-1".into());
        record.native_session_id = Some("native-session".into());
        save(&id, &record);
        assert_eq!(load_for_restore(&id).unwrap(), Some(record));
        std::fs::write(record_path(&id).unwrap(), "broken json").unwrap();
        assert!(load_for_restore(&id).is_err());
    }

    #[test]
    fn records_round_trip_beside_the_thread() {
        let session_id = crate::config::create_detached_session();
        assert!(load(&session_id).is_none());
        let mut record = HarnessRecord::new("codex", PathBuf::from("/tmp/project"));
        record.native_session_id = Some("thread-1".into());
        record.model = Some("gpt-6-luna".into());
        save(&session_id, &record);
        assert_eq!(load(&session_id), Some(record));

        let mut with_account = HarnessRecord::new("claude-code", PathBuf::from("/tmp/project"));
        with_account.account = Some("paral-2".into());
        save(&session_id, &with_account);
        assert_eq!(load(&session_id), Some(with_account));

        let builtin = HarnessRecord::new("rustcode", PathBuf::from("/tmp"));
        save(&session_id, &builtin);
        assert!(
            load(&session_id).is_none(),
            "built-in threads have no record"
        );
    }

    #[test]
    fn a_record_keeps_everything_a_resume_needs() {
        let session_id = crate::config::create_detached_session();
        let mut record = HarnessRecord::new("claude-code", PathBuf::from("/tmp/project"));
        record.account = Some("paral-2".into());
        record.native_session_id = Some("native-7".into());
        record.model = Some("claude-sonnet-5".into());
        record.effort = Some("high".into());
        save(&session_id, &record);
        // A process stopped by the idle reaper writes nothing: reading the
        // record back is what the next start resumes from.
        let loaded = load(&session_id).expect("an external thread has a record");
        assert_eq!(loaded.account, Some("paral-2".into()));
        assert_eq!(loaded.native_session_id, Some("native-7".into()));
        assert_eq!(loaded.model, Some("claude-sonnet-5".into()));
        assert_eq!(loaded.effort, Some("high".into()));
    }

    #[test]
    fn old_records_without_an_account_still_load() {
        let record: HarnessRecord = serde_json::from_str(
            r#"{"harness":"claude-code","cwd":"/tmp","created_at":"2026-01-01T00:00:00Z",
                "updated_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(record.account, None);
        let text = serde_json::to_string(&record).unwrap();
        assert!(!text.contains("account"), "{text}");
        let with = HarnessRecord {
            account: Some("paral-2".into()),
            ..record
        };
        let text = serde_json::to_string(&with).unwrap();
        assert!(text.contains("\"account\":\"paral-2\""), "{text}");
    }
}
