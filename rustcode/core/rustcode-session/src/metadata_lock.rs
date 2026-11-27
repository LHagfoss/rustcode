//! Exclusive per-thread lock serializing `metadata.json` read-modify-write
//! updates across threads and processes.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Lock file beside `metadata.json` inside a thread's folder.
pub const SESSION_METADATA_LOCK_FILE: &str = "metadata.lock";

/// Held for the duration of a `metadata.json` read-modify-write cycle.
/// Released automatically on drop or process exit.
pub(crate) struct MetadataLock {
    file: File,
}

impl MetadataLock {
    pub(crate) fn acquire(directory: &Path) -> io::Result<Self> {
        if !directory.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} does not exist", directory.display()),
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(SESSION_METADATA_LOCK_FILE))?;
        file.lock()?;
        Ok(Self { file })
    }
}

impl Drop for MetadataLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MachineOrigin, SessionCompaction, SessionModel, SessionParent, SessionRelationship,
        SessionStore,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    #[test]
    fn metadata_lock_rejects_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent");
        let error = MetadataLock::acquire(&missing).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!missing.exists());
    }

    #[test]
    fn concurrent_metadata_updates_preserve_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path()));
        let session_id = "20261127-130000-lock-race";
        store.ensure_session(session_id);

        let barrier = Arc::new(Barrier::new(8));
        let mut handles = Vec::new();

        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for iteration in 0..8 {
                    store
                        .save_session_yolo(session_id, iteration % 2 == 1)
                        .unwrap();
                }
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.mark_session_user_titled(session_id).unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store
                    .save_session_model(
                        session_id,
                        SessionModel {
                            profile: "claude-opus".into(),
                            reasoning_effort: Some("high".into()),
                            fast: true,
                        },
                    )
                    .unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store
                    .save_session_parent(
                        session_id,
                        SessionParent {
                            id: Some("parent-1".into()),
                            title: Some("Parent".into()),
                            relationship: Some(SessionRelationship::Delegate),
                        },
                    )
                    .unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store
                    .save_session_repo(session_id, Some(PathBuf::from("/repo/rustcode")))
                    .unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store
                    .save_session_scheduled_job(session_id, "nightly-check")
                    .unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store
                    .save_session_compaction(
                        session_id,
                        SessionCompaction {
                            count: 2,
                            trigger: "manual".into(),
                            mode: "ai_summary".into(),
                            before_tokens: 100_000,
                            after_tokens: 20_000,
                            messages_considered: 40,
                            at_ms: 1_700_000_000_000,
                        },
                    )
                    .unwrap();
            }));
        }
        {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let mut origins = BTreeMap::new();
                origins.insert(
                    "macbook".into(),
                    MachineOrigin {
                        cwd: Some(PathBuf::from("/Users/lucas/rustcode")),
                        harness: Some("claude-code".into()),
                        account: Some("personal".into()),
                        model: None,
                        effort: None,
                        native_session_id: Some("native-1".into()),
                        turn_count: 3,
                    },
                );
                store
                    .save_session_machine_origins(session_id, origins)
                    .unwrap();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let metadata = store.load_session_metadata(session_id).unwrap();
        assert_eq!(metadata.yolo, Some(true));
        assert!(metadata.user_titled);
        assert_eq!(
            metadata.model.as_ref().map(|m| m.profile.as_str()),
            Some("claude-opus")
        );
        assert_eq!(
            metadata.parent.as_ref().and_then(|p| p.id.as_deref()),
            Some("parent-1")
        );
        assert_eq!(metadata.repo, Some(PathBuf::from("/repo/rustcode")));
        assert_eq!(metadata.scheduled_job.as_deref(), Some("nightly-check"));
        assert_eq!(metadata.compaction.as_ref().map(|c| c.count), Some(2));
        assert!(metadata.machine_origins.contains_key("macbook"));
    }

    #[test]
    fn ensure_session_never_clobbers_concurrent_metadata_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path()));
        let session_id = "20261127-130500-ensure-race";
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for index in 0..4 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                if index == 0 {
                    store.save_session_yolo(session_id, true).unwrap();
                } else {
                    store.ensure_session(session_id);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(store.load_session_yolo(session_id), Some(true));
    }
}
