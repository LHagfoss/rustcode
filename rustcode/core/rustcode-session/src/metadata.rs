//! Unified thread state stored in `metadata.json` (`status`, `pinned`,
//! `snoozed_until`, `title`, and `user_titled`), with read fallback to the
//! legacy per-field sidecar files (`status.json`, `pin.json`, `snooze.json`,
//! `title.txt`, and `user-titled`).

use crate::{
    SESSION_METADATA_FILE, SESSION_METADATA_SCHEMA_VERSION, SessionMetadata, SessionStore,
    current_time_ms, metadata_lock, read_store_file, read_store_text, session_timestamp_ms,
    unwrap_title_paste_markers, valid_session_id, write_session_file,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// Legacy sidecar file replaced by `SessionMetadata::status`.
pub const LEGACY_STATUS_FILE: &str = "status.json";
/// Legacy sidecar file replaced by `SessionMetadata::pinned`.
pub const LEGACY_PIN_FILE: &str = "pin.json";
/// Legacy sidecar file replaced by `SessionMetadata::snoozed_until`.
pub const LEGACY_SNOOZE_FILE: &str = "snooze.json";
/// Legacy sidecar file replaced by `SessionMetadata::title`.
pub const LEGACY_TITLE_FILE: &str = "title.txt";
/// Legacy sidecar file replaced by `SessionMetadata::user_titled`.
pub const LEGACY_USER_TITLED_FILE: &str = "user-titled";

/// Stored thread status (`review`, `done`, or `stalled`; `idle` is stored as absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusRecord {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

/// Load `metadata.json` directly from a thread directory without a [`SessionStore`].
pub fn load_session_metadata_in(directory: &Path) -> Option<SessionMetadata> {
    let metadata: SessionMetadata =
        serde_json::from_slice(&std::fs::read(directory.join(SESSION_METADATA_FILE)).ok()?).ok()?;
    (metadata.schema_version == SESSION_METADATA_SCHEMA_VERSION).then_some(metadata)
}

fn read_legacy_status(directory: &Path) -> Option<String> {
    let text = std::fs::read_to_string(directory.join(LEGACY_STATUS_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("status")?.as_str().map(str::to_owned)
}

fn read_legacy_snooze(directory: &Path) -> Option<u64> {
    serde_json::from_slice::<Option<u64>>(&std::fs::read(directory.join(LEGACY_SNOOZE_FILE)).ok()?)
        .ok()
        .flatten()
}

impl SessionStore {
    pub(crate) fn update_session_metadata_with_cleanup(
        &self,
        session_id: &str,
        create_if_missing: bool,
        update: impl FnOnce(&mut SessionMetadata) -> bool,
        cleanup_files: &[&str],
    ) -> io::Result<()> {
        if !valid_session_id(session_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid session id",
            ));
        }
        let directory = if create_if_missing {
            self.ensure_session(session_id)
        } else {
            let directory = self.session_dir(session_id);
            if !directory.is_dir() {
                return Ok(());
            }
            directory
        };
        let _lock = metadata_lock::MetadataLock::acquire(&directory)?;
        let mut metadata = self.load_session_metadata(session_id).unwrap_or_else(|| {
            SessionMetadata::new(
                session_id,
                session_timestamp_ms(session_id).unwrap_or_else(current_time_ms),
                None,
            )
        });
        if update(&mut metadata) {
            let contents = serde_json::to_vec_pretty(&metadata).map_err(io::Error::other)?;
            write_session_file(&directory, SESSION_METADATA_FILE, &contents)?;
        }
        for file in cleanup_files {
            let _ = std::fs::remove_file(directory.join(file));
        }
        Ok(())
    }

    /// Save the session's title inside `metadata.json` and remove any legacy `title.txt`.
    pub fn save_session_title(&self, session_id: &str, title: &str) {
        let _ = self.update_session_metadata_with_cleanup(
            session_id,
            true,
            |metadata| {
                let changed = metadata.title.as_deref() != Some(title);
                metadata.title = Some(title.to_owned());
                changed
            },
            &[LEGACY_TITLE_FILE],
        );
    }

    /// Save a title chosen by the user and mark `user_titled` in one locked
    /// `metadata.json` update, removing legacy `title.txt` and `user-titled`.
    pub fn save_user_session_title(&self, session_id: &str, title: &str) -> io::Result<()> {
        self.update_session_metadata_with_cleanup(
            session_id,
            true,
            |metadata| {
                let title_changed = metadata.title.as_deref() != Some(title);
                metadata.title = Some(title.to_owned());
                let flag_changed = !std::mem::replace(&mut metadata.user_titled, true);
                title_changed || flag_changed
            },
            &[LEGACY_TITLE_FILE, LEGACY_USER_TITLED_FILE],
        )
    }

    /// Load the session's saved title, checking legacy/external `title.txt` first
    /// (so shell rewrites are observed) before `metadata.json`.
    pub fn load_session_title(&self, session_id: &str) -> Option<String> {
        let legacy = self.session_dir(session_id).join(LEGACY_TITLE_FILE);
        if legacy.exists()
            && let Ok(value) = read_store_text(&legacy)
        {
            return Some(unwrap_title_paste_markers(value.trim()).into_owned());
        }
        self.load_session_metadata(session_id)
            .and_then(|metadata| metadata.title)
            .map(|value| unwrap_title_paste_markers(value.trim()).into_owned())
    }

    /// Record that the user named the session themselves, so an agent leaves
    /// the title alone (issue 3599).
    pub fn mark_session_user_titled(&self, session_id: &str) -> io::Result<()> {
        self.update_session_metadata_with_cleanup(
            session_id,
            true,
            |metadata| !std::mem::replace(&mut metadata.user_titled, true),
            &[LEGACY_USER_TITLED_FILE],
        )
    }

    /// Whether the user named the session themselves.
    pub fn session_user_titled(&self, session_id: &str) -> bool {
        self.load_session_metadata(session_id)
            .is_some_and(|metadata| metadata.user_titled)
    }

    /// Store or clear (`None` for `idle`) the thread's persisted status in `metadata.json`.
    pub fn save_session_status(&self, session_id: &str, status: Option<&str>) -> io::Result<()> {
        self.update_session_metadata_with_cleanup(
            session_id,
            status.is_some(),
            |metadata| match status {
                Some(status) => {
                    let changed = metadata
                        .status
                        .as_ref()
                        .map(|record| record.status.as_str())
                        != Some(status);
                    if changed {
                        metadata.status = Some(SessionStatusRecord {
                            status: status.to_owned(),
                            at: Some(Utc::now().to_rfc3339()),
                        });
                    }
                    changed
                }
                None => metadata.status.take().is_some(),
            },
            &[LEGACY_STATUS_FILE],
        )
    }

    /// Load the thread's persisted status from `metadata.json`, falling back to legacy `status.json`.
    pub fn load_session_status(&self, session_id: &str) -> Option<String> {
        if !valid_session_id(session_id) {
            return None;
        }
        let directory = self.session_dir(session_id);
        let metadata = self.load_session_metadata(session_id);
        Self::load_session_status_in(&directory, metadata.as_ref())
    }

    /// Resolve a thread's persisted status from preloaded metadata or legacy `status.json`.
    pub fn load_session_status_in(
        directory: &Path,
        metadata: Option<&SessionMetadata>,
    ) -> Option<String> {
        metadata
            .and_then(|metadata| metadata.status.as_ref())
            .map(|record| record.status.clone())
            .or_else(|| read_legacy_status(directory))
    }

    /// Store whether the thread is pinned in `metadata.json`.
    pub fn save_session_pinned(&self, session_id: &str, pinned: bool) -> io::Result<()> {
        self.update_session_metadata_with_cleanup(
            session_id,
            pinned,
            |metadata| {
                let changed = metadata.pinned != pinned;
                metadata.pinned = pinned;
                changed
            },
            &[LEGACY_PIN_FILE],
        )
    }

    /// Whether the thread is pinned, checking `metadata.json` and legacy `pin.json`.
    pub fn load_session_pinned(&self, session_id: &str) -> bool {
        if !valid_session_id(session_id) {
            return false;
        }
        let directory = self.session_dir(session_id);
        let metadata = self.load_session_metadata(session_id);
        Self::load_session_pinned_in(&directory, metadata.as_ref())
    }

    /// Resolve a thread's pin state from preloaded metadata or legacy `pin.json`.
    pub fn load_session_pinned_in(directory: &Path, metadata: Option<&SessionMetadata>) -> bool {
        metadata.is_some_and(|metadata| metadata.pinned)
            || directory.join(LEGACY_PIN_FILE).is_file()
    }

    /// Store or clear the thread's snooze deadline (Unix milliseconds) in `metadata.json`.
    pub fn save_session_snoozed_until(
        &self,
        session_id: &str,
        until: Option<u64>,
    ) -> io::Result<()> {
        self.update_session_metadata_with_cleanup(
            session_id,
            until.is_some(),
            |metadata| {
                let changed = metadata.snoozed_until != until;
                metadata.snoozed_until = until;
                changed
            },
            &[LEGACY_SNOOZE_FILE],
        )
    }

    /// Load the thread's snooze deadline from `metadata.json`, falling back to legacy `snooze.json`.
    pub fn load_session_snoozed_until(&self, session_id: &str) -> Option<u64> {
        if !valid_session_id(session_id) {
            return None;
        }
        let directory = self.session_dir(session_id);
        let metadata = self.load_session_metadata(session_id);
        Self::load_session_snoozed_until_in(&directory, metadata.as_ref())
    }

    /// Resolve a thread's snooze deadline from preloaded metadata or legacy `snooze.json`.
    pub fn load_session_snoozed_until_in(
        directory: &Path,
        metadata: Option<&SessionMetadata>,
    ) -> Option<u64> {
        metadata
            .and_then(|metadata| metadata.snoozed_until)
            .or_else(|| read_legacy_snooze(directory))
    }

    /// Restore imported status, pin, and snooze in a single locked `metadata.json` write.
    pub fn restore_session_state(
        &self,
        session_id: &str,
        status: Option<&str>,
        pinned: bool,
        snoozed_until: Option<u64>,
    ) -> io::Result<()> {
        if status.is_none() && !pinned && snoozed_until.is_none() {
            return Ok(());
        }
        self.update_session_metadata_with_cleanup(
            session_id,
            true,
            |metadata| {
                let mut changed = false;
                if let Some(status) = status
                    && metadata
                        .status
                        .as_ref()
                        .map(|record| record.status.as_str())
                        != Some(status)
                {
                    metadata.status = Some(SessionStatusRecord {
                        status: status.to_owned(),
                        at: Some(Utc::now().to_rfc3339()),
                    });
                    changed = true;
                }
                if pinned && !metadata.pinned {
                    metadata.pinned = true;
                    changed = true;
                }
                if let Some(until) = snoozed_until
                    && metadata.snoozed_until != Some(until)
                {
                    metadata.snoozed_until = Some(until);
                    changed = true;
                }
                changed
            },
            &[LEGACY_STATUS_FILE, LEGACY_PIN_FILE, LEGACY_SNOOZE_FILE],
        )
    }

    pub(crate) fn load_session_metadata_counted(
        &self,
        session_id: &str,
    ) -> Option<SessionMetadata> {
        if !valid_session_id(session_id) {
            return None;
        }
        let metadata: SessionMetadata = serde_json::from_slice(
            &read_store_file(&self.session_dir(session_id).join(SESSION_METADATA_FILE)).ok()?,
        )
        .ok()?;
        (metadata.id == session_id && metadata.schema_version == SESSION_METADATA_SCHEMA_VERSION)
            .then_some(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn status_pin_snooze_and_title_round_trip_in_metadata() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        let id = "20261127-140000-unified";
        let dir = store.ensure_session(id);

        assert_eq!(store.load_session_status(id), None);
        assert!(!store.load_session_pinned(id));
        assert_eq!(store.load_session_snoozed_until(id), None);
        assert_eq!(store.load_session_title(id), None);

        store.save_session_status(id, Some("review")).unwrap();
        store.save_session_pinned(id, true).unwrap();
        store
            .save_session_snoozed_until(id, Some(1_700_000_000_000))
            .unwrap();
        store.save_user_session_title(id, "Unified state").unwrap();

        assert_eq!(store.load_session_status(id).as_deref(), Some("review"));
        assert!(store.load_session_pinned(id));
        assert_eq!(
            store.load_session_snoozed_until(id),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            store.load_session_title(id).as_deref(),
            Some("Unified state")
        );
        assert!(store.session_user_titled(id));

        assert!(!dir.join(LEGACY_STATUS_FILE).exists());
        assert!(!dir.join(LEGACY_PIN_FILE).exists());
        assert!(!dir.join(LEGACY_SNOOZE_FILE).exists());
        assert!(!dir.join(LEGACY_TITLE_FILE).exists());
        assert!(!dir.join(LEGACY_USER_TITLED_FILE).exists());

        store.save_session_status(id, None).unwrap();
        store.save_session_pinned(id, false).unwrap();
        store.save_session_snoozed_until(id, None).unwrap();

        assert_eq!(store.load_session_status(id), None);
        assert!(!store.load_session_pinned(id));
        assert_eq!(store.load_session_snoozed_until(id), None);
        assert_eq!(
            store.load_session_title(id).as_deref(),
            Some("Unified state")
        );
        assert!(store.session_user_titled(id));
    }

    #[test]
    fn legacy_sidecar_files_remain_readable_and_are_cleaned_up_on_write() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        let id = "20261127-140500-legacy";
        let dir = store.ensure_session(id);

        std::fs::write(dir.join(LEGACY_STATUS_FILE), r#"{"status":"done"}"#).unwrap();
        std::fs::write(dir.join(LEGACY_PIN_FILE), r#"{"pinned":true}"#).unwrap();
        std::fs::write(dir.join(LEGACY_SNOOZE_FILE), "1800000000000").unwrap();
        std::fs::write(dir.join(LEGACY_TITLE_FILE), "Legacy title").unwrap();

        assert_eq!(store.load_session_status(id).as_deref(), Some("done"));
        assert!(store.load_session_pinned(id));
        assert_eq!(
            store.load_session_snoozed_until(id),
            Some(1_800_000_000_000)
        );
        assert_eq!(
            store.load_session_title(id).as_deref(),
            Some("Legacy title")
        );

        store.save_session_status(id, None).unwrap();
        store.save_session_pinned(id, false).unwrap();
        store.save_session_snoozed_until(id, None).unwrap();
        store.save_session_title(id, "Migrated title");

        assert_eq!(store.load_session_status(id), None);
        assert!(!store.load_session_pinned(id));
        assert_eq!(store.load_session_snoozed_until(id), None);
        assert_eq!(
            store.load_session_title(id).as_deref(),
            Some("Migrated title")
        );
        assert!(!dir.join(LEGACY_STATUS_FILE).exists());
        assert!(!dir.join(LEGACY_PIN_FILE).exists());
        assert!(!dir.join(LEGACY_SNOOZE_FILE).exists());
        assert!(!dir.join(LEGACY_TITLE_FILE).exists());
    }

    #[test]
    fn external_title_file_wins_until_next_engine_rename() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        let id = "20261127-141000-external-title";
        let dir = store.ensure_session(id);

        store.save_session_title(id, "Automatic title");
        assert_eq!(
            store.load_session_title(id).as_deref(),
            Some("Automatic title")
        );

        std::fs::write(dir.join(LEGACY_TITLE_FILE), "Desktop shell rename").unwrap();
        assert_eq!(
            store.load_session_title(id).as_deref(),
            Some("Desktop shell rename")
        );

        store.save_user_session_title(id, "Engine rename").unwrap();
        assert_eq!(
            store.load_session_title(id).as_deref(),
            Some("Engine rename")
        );
        assert!(!dir.join(LEGACY_TITLE_FILE).exists());
    }

    #[test]
    fn restore_session_state_sets_fields_atomically() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        let id = "20261127-141500-restore";

        store
            .restore_session_state(id, Some("review"), true, Some(1_900_000_000_000))
            .unwrap();

        let metadata = store.load_session_metadata(id).unwrap();
        assert_eq!(
            metadata
                .status
                .as_ref()
                .map(|record| record.status.as_str()),
            Some("review")
        );
        assert!(metadata.pinned);
        assert_eq!(metadata.snoozed_until, Some(1_900_000_000_000));
    }

    #[test]
    fn concurrent_unified_state_updates_preserve_every_field() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(root.path()));
        let id = "20261127-142000-concurrent";
        store.ensure_session(id);

        let barrier = Arc::new(Barrier::new(5));
        let mut handles = Vec::new();
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.save_session_status(id, Some("review")).unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.save_session_pinned(id, true).unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store
                    .save_session_snoozed_until(id, Some(2_000_000_000_000))
                    .unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store
                    .save_user_session_title(id, "Concurrent title")
                    .unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.save_session_yolo(id, true).unwrap();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let metadata = store.load_session_metadata(id).unwrap();
        assert_eq!(
            metadata
                .status
                .as_ref()
                .map(|record| record.status.as_str()),
            Some("review")
        );
        assert!(metadata.pinned);
        assert_eq!(metadata.snoozed_until, Some(2_000_000_000_000));
        assert_eq!(metadata.title.as_deref(), Some("Concurrent title"));
        assert!(metadata.user_titled);
        assert_eq!(metadata.yolo, Some(true));
    }
}
