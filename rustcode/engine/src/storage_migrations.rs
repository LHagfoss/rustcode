//! Versioned user-data migrations (#2599).
//!
//! Each migration has a stable, ordered id. The ids that have run to
//! completion are recorded in `storage/migrations.json` under the config
//! directory, so a step introduced in a newer release runs once, on the first
//! startup after the upgrade, whichever way the binary was installed. A step
//! is recorded only after it succeeded; a failed step stops the run, is
//! reported, and is retried on the next startup. `storage/migrations.lock`
//! serializes RustCode processes that start at the same time.
//!
//! Migrations must be idempotent: an interrupted step (the process died
//! before its id was recorded) runs again from the start.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Bumped only for an incompatible change to the checkpoint file. A file with
/// a newer format comes from a newer RustCode; this one then neither runs
/// migrations nor rewrites the file.
pub const CHECKPOINT_FORMAT: u32 = 1;

const STORAGE_DIR: &str = "storage";
const CHECKPOINT_FILE: &str = "migrations.json";
const LOCK_FILE: &str = "migrations.lock";

/// How long startup waits for another process that holds the migration lock
/// before it carries on and leaves the work to that process.
const STARTUP_LOCK_WAIT: Duration = Duration::from_secs(10);

/// One user-data migration. `run` gets the config directory and must be
/// idempotent and leave user data recoverable when it fails.
pub struct Migration {
    /// Stable id, never reused or renamed once released. Order in
    /// [`MIGRATIONS`] is the order steps run in.
    pub id: &'static str,
    pub description: &'static str,
    pub run: fn(&Path) -> Result<(), String>,
}

/// The registry, oldest first. Append new steps at the end.
///
/// Empty for now: the legacy config (`config.json`/`models.json`) and session
/// layout migrations keep their existing entry points until each can be moved
/// here without changing its compatibility behavior.
pub const MIGRATIONS: &[Migration] = &[];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Checkpoint {
    format: u32,
    #[serde(default)]
    completed: Vec<CompletedMigration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletedMigration {
    pub id: String,
    /// RFC 3339 time the step finished.
    pub completed_at: String,
    /// RustCode version that ran it.
    pub version: String,
}

/// What the checkpoint file says, without changing it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CheckpointState {
    /// No file yet: a fresh install, or nothing has run.
    Missing,
    Current,
    /// Written by a newer RustCode; migrations are left alone.
    Newer {
        format: u32,
    },
    /// Unparseable; migrations are left alone until it is moved aside.
    Unreadable {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MigrationState {
    Pending,
    Completed {
        completed_at: String,
        version: String,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MigrationEntry {
    pub id: String,
    pub description: String,
    #[serde(flatten)]
    pub state: MigrationState,
}

/// Read-only report for `rustcode storage status`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MigrationStatus {
    pub config_dir: PathBuf,
    pub checkpoint_path: PathBuf,
    pub checkpoint_format: u32,
    pub checkpoint: CheckpointState,
    pub migrations: Vec<MigrationEntry>,
    /// Completed ids this build does not know (recorded by a newer release).
    /// They are kept when the checkpoint is rewritten.
    pub unknown_completed: Vec<String>,
}

impl MigrationStatus {
    pub fn pending(&self) -> impl Iterator<Item = &MigrationEntry> {
        self.migrations
            .iter()
            .filter(|entry| entry.state == MigrationState::Pending)
    }

    /// Plain-text form for a person.
    pub fn render(&self) -> String {
        let mut out = format!(
            "Config directory: {}\nCheckpoint: {} ({})\n",
            self.config_dir.display(),
            self.checkpoint_path.display(),
            match &self.checkpoint {
                CheckpointState::Missing => "not written yet".to_owned(),
                CheckpointState::Current => format!("format {}", self.checkpoint_format),
                CheckpointState::Newer { format } => format!(
                    "format {format} from a newer RustCode; this version supports {} and will not run migrations",
                    self.checkpoint_format
                ),
                CheckpointState::Unreadable { error } =>
                    format!("unreadable: {error}; move it aside to let migrations run again"),
            }
        );
        if self.migrations.is_empty() {
            out.push_str("No migrations are registered in this version.\n");
        }
        for entry in &self.migrations {
            match &entry.state {
                MigrationState::Pending => {
                    out.push_str(&format!("pending    {}  {}\n", entry.id, entry.description));
                }
                MigrationState::Completed {
                    completed_at,
                    version,
                } => out.push_str(&format!(
                    "completed  {}  {} (at {completed_at}, v{version})\n",
                    entry.id, entry.description
                )),
            }
        }
        for id in &self.unknown_completed {
            out.push_str(&format!(
                "completed  {id}  (recorded by a newer RustCode)\n"
            ));
        }
        out
    }
}

/// Outcome of one run of pending migrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// Nothing to do; no file was touched.
    UpToDate,
    Applied(Vec<String>),
    /// `id` failed; `applied` ran (and were recorded) before it. The failed
    /// step and those after it run again on the next startup.
    Failed {
        applied: Vec<String>,
        id: String,
        error: String,
    },
    /// Another process held the lock for the whole wait; it runs the steps.
    Busy,
    /// The checkpoint is newer or unreadable; nothing ran.
    Blocked(String),
}

pub fn checkpoint_path(config_dir: &Path) -> PathBuf {
    config_dir.join(STORAGE_DIR).join(CHECKPOINT_FILE)
}

fn lock_path(config_dir: &Path) -> PathBuf {
    config_dir.join(STORAGE_DIR).join(LOCK_FILE)
}

fn read_checkpoint(path: &Path) -> (CheckpointState, Checkpoint) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (CheckpointState::Missing, Checkpoint::default());
        }
        Err(error) => {
            return (
                CheckpointState::Unreadable {
                    error: error.to_string(),
                },
                Checkpoint::default(),
            );
        }
    };
    // Look at the format first so a newer file whose shape changed reports
    // as newer rather than unreadable.
    let format = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("format").and_then(serde_json::Value::as_u64));
    if let Some(format) = format
        && format > u64::from(CHECKPOINT_FORMAT)
    {
        return (
            CheckpointState::Newer {
                format: u32::try_from(format).unwrap_or(u32::MAX),
            },
            Checkpoint::default(),
        );
    }
    match serde_json::from_str::<Checkpoint>(&text) {
        Ok(checkpoint) => (CheckpointState::Current, checkpoint),
        Err(error) => (
            CheckpointState::Unreadable {
                error: error.to_string(),
            },
            Checkpoint::default(),
        ),
    }
}

fn status_with(config_dir: &Path, registry: &[Migration]) -> (MigrationStatus, Checkpoint) {
    let path = checkpoint_path(config_dir);
    let (state, checkpoint) = read_checkpoint(&path);
    let migrations = registry
        .iter()
        .map(|migration| MigrationEntry {
            id: migration.id.to_owned(),
            description: migration.description.to_owned(),
            state: checkpoint
                .completed
                .iter()
                .find(|done| done.id == migration.id)
                .map_or(MigrationState::Pending, |done| MigrationState::Completed {
                    completed_at: done.completed_at.clone(),
                    version: done.version.clone(),
                }),
        })
        .collect();
    let unknown_completed = checkpoint
        .completed
        .iter()
        .filter(|done| !registry.iter().any(|migration| migration.id == done.id))
        .map(|done| done.id.clone())
        .collect();
    (
        MigrationStatus {
            config_dir: config_dir.to_path_buf(),
            checkpoint_path: path,
            checkpoint_format: CHECKPOINT_FORMAT,
            checkpoint: state,
            migrations,
            unknown_completed,
        },
        checkpoint,
    )
}

/// Pending and completed migrations for `config_dir`. Never writes.
pub fn status(config_dir: &Path) -> MigrationStatus {
    status_with(config_dir, MIGRATIONS).0
}

fn blocked_reason(status: &MigrationStatus) -> Option<String> {
    match &status.checkpoint {
        CheckpointState::Newer { format } => Some(format!(
            "{} has format {format}, newer than this RustCode supports ({CHECKPOINT_FORMAT}); upgrade RustCode",
            status.checkpoint_path.display()
        )),
        CheckpointState::Unreadable { error } => Some(format!(
            "{} is unreadable ({error}); move it aside to run migrations again",
            status.checkpoint_path.display()
        )),
        CheckpointState::Missing | CheckpointState::Current => None,
    }
}

fn write_checkpoint(path: &Path, checkpoint: &Checkpoint) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(checkpoint).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, json)
        .and_then(|()| File::open(&temporary)?.sync_all())
        .and_then(|()| crate::atomic_file::replace_file(&temporary, path))
        .map_err(|error| {
            let _ = fs::remove_file(&temporary);
            format!("could not write {}: {error}", path.display())
        })
}

/// Hold the migration lock, waiting up to `wait`. `None` when another
/// process held it the whole time.
fn acquire_lock(config_dir: &Path, wait: Duration) -> Result<Option<File>, String> {
    let path = lock_path(config_dir);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!("could not lock {}: {error}", path.display()));
            }
        }
    }
}

fn run_pending_with(config_dir: &Path, registry: &[Migration], lock_wait: Duration) -> RunOutcome {
    // Decide from a read alone, so an up-to-date install writes nothing.
    let (status, _) = status_with(config_dir, registry);
    if let Some(reason) = blocked_reason(&status) {
        return RunOutcome::Blocked(reason);
    }
    if status.pending().next().is_none() {
        return RunOutcome::UpToDate;
    }
    if let Err(error) = fs::create_dir_all(config_dir.join(STORAGE_DIR)) {
        return RunOutcome::Blocked(format!("could not create the storage directory: {error}"));
    }
    let _lock = match acquire_lock(config_dir, lock_wait) {
        Ok(Some(lock)) => lock,
        Ok(None) => return RunOutcome::Busy,
        Err(error) => return RunOutcome::Blocked(error),
    };
    // Another process may have run the steps while this one waited.
    let (status, mut checkpoint) = status_with(config_dir, registry);
    if let Some(reason) = blocked_reason(&status) {
        return RunOutcome::Blocked(reason);
    }
    if status.pending().next().is_none() {
        return RunOutcome::UpToDate;
    }
    checkpoint.format = CHECKPOINT_FORMAT;
    let path = status.checkpoint_path.clone();
    let mut applied = Vec::new();
    for migration in registry {
        if checkpoint
            .completed
            .iter()
            .any(|done| done.id == migration.id)
        {
            continue;
        }
        if let Err(error) = (migration.run)(config_dir) {
            return RunOutcome::Failed {
                applied,
                id: migration.id.to_owned(),
                error,
            };
        }
        checkpoint.completed.push(CompletedMigration {
            id: migration.id.to_owned(),
            completed_at: chrono::Utc::now().to_rfc3339(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        if let Err(error) = write_checkpoint(&path, &checkpoint) {
            return RunOutcome::Failed {
                applied,
                id: migration.id.to_owned(),
                error,
            };
        }
        applied.push(migration.id.to_owned());
    }
    RunOutcome::Applied(applied)
}

/// Run the registered migrations that have not completed yet.
pub fn run_pending(config_dir: &Path) -> RunOutcome {
    run_pending_with(config_dir, MIGRATIONS, STARTUP_LOCK_WAIT)
}

/// Startup entry point: run pending migrations, log the outcome, and report a
/// failure on stderr without stopping startup.
pub fn run_at_startup() {
    let Some(config_dir) = crate::config::get_config_dir() else {
        return;
    };
    let outcome = run_pending(&config_dir);
    let fields = match &outcome {
        RunOutcome::UpToDate => return,
        RunOutcome::Applied(applied) => serde_json::json!({"applied": applied}),
        RunOutcome::Failed { applied, id, error } => {
            eprintln!(
                "RustCode: user-data migration `{id}` failed: {error}. RustCode will retry it on the next start; run `rustcode storage status` to inspect."
            );
            serde_json::json!({"applied": applied, "failed": id, "error": error})
        }
        RunOutcome::Busy => serde_json::json!({"busy": true}),
        RunOutcome::Blocked(reason) => {
            eprintln!("RustCode: user-data migrations skipped: {reason}.");
            serde_json::json!({"blocked": reason})
        }
    };
    crate::logger::operational_event("storage.migrations", fields);
}

#[cfg(test)]
#[path = "storage_migrations_tests.rs"]
mod tests;
