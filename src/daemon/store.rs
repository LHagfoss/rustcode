//! Durable scheduler state. Mutations use IMMEDIATE transactions so independent
//! connections cannot claim the same occurrence. A Claimed run has not executed;
//! the executor must settle it as Running *before* starting any external action.
use super::model::{JobRecord, JobRunRecord, JobRunState, MisfirePolicy, RunSettlement, ScheduleSpec};
use super::{DaemonError, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;

pub const MAX_HISTORY_ROWS: usize = 100;
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_CLAIMS: usize = 100;

pub struct JobStore {
    connection: Connection,
}

fn storage(error: impl std::fmt::Display) -> DaemonError {
    DaemonError::Storage(error.to_string())
}

fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(storage)
}

fn decode<T: DeserializeOwned>(value: String) -> Result<T> {
    serde_json::from_str(&value).map_err(storage)
}

fn get_job(connection: &Connection, id: &str) -> Result<JobRecord> {
    let json: Option<String> = connection.query_row("SELECT payload FROM jobs WHERE id=?1", [id], |r| r.get(0))
        .optional().map_err(storage)?;
    decode(json.ok_or_else(|| DaemonError::NotFound(id.into()))?)
}

fn save_job(connection: &Connection, job: &JobRecord) -> Result<()> {
    connection.execute("UPDATE jobs SET payload=?2, paused=?3, next_due=?4 WHERE id=?1",
        params![job.id, encode(job)?, job.paused, job.next_due_at.timestamp_millis()]).map_err(storage)?;
    Ok(())
}

fn save_run(connection: &Connection, run: &JobRunRecord) -> Result<()> {
    connection.execute("UPDATE job_runs SET payload=?2, active=?3 WHERE id=?1",
        params![run.id, encode(run)?, matches!(run.state, JobRunState::Claimed | JobRunState::Running)]).map_err(storage)?;
    Ok(())
}

fn advance(job: &mut JobRecord, now: DateTime<Utc>) -> Result<()> {
    if matches!(job.schedule, ScheduleSpec::Once { .. }) {
        job.paused = true;
        job.updated_at = now;
        return Ok(());
    }
    match job.schedule.next_after(now.max(job.next_due_at)) {
        Ok(next) => job.next_due_at = next,
        // Exhausted one-shot (or finite cron) jobs retain their history and pause.
        Err(DaemonError::InvalidSchedule(_)) => job.paused = true,
        Err(error) => return Err(error),
    }
    job.updated_at = now;
    Ok(())
}

fn bounded(value: Option<String>) -> Option<String> {
    value.map(|mut text| {
        let mut end = text.len().min(MAX_OUTPUT_BYTES);
        while !text.is_char_boundary(end) { end -= 1; }
        text.truncate(end);
        text
    })
}

impl JobStore {
    /// Called once after acquiring exclusive daemon ownership. Running actions
    /// from an earlier process become ambiguous during the next claim scan;
    /// unstarted claims are safe to recover. Retry deadlines remain intact.
    pub fn recover_previous_owner(&mut self, owner: &str, now: DateTime<Utc>) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let runs = {
            let mut stmt = tx
                .prepare("SELECT payload FROM job_runs WHERE active=1")
                .map_err(storage)?;
            stmt.query_map([], |r| r.get::<_, String>(0))
                .map_err(storage)?
                .map(|r| decode::<JobRunRecord>(r.map_err(storage)?))
                .collect::<Result<Vec<_>>>()?
        };
        for mut run in runs {
            if run.lease_owner.as_deref() != Some(owner)
                && !(run.state == JobRunState::Claimed
                    && run.attempt > 1
                    && run.started_at.is_none())
            {
                run.lease_expires_at = Some(now);
                save_run(&tx, &run)?;
            }
        }
        tx.commit().map_err(storage)
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path).map_err(storage)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory().map_err(storage)?)
    }

    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(std::time::Duration::from_secs(5)).map_err(storage)?;
        connection.execute_batch("PRAGMA foreign_keys=ON;").map_err(storage)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(storage)?;
        if version > 1 { return Err(DaemonError::Storage(format!("unsupported schema version {version}"))); }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY NOT NULL, payload TEXT NOT NULL,
            paused INTEGER NOT NULL, next_due INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS jobs_due ON jobs(paused, next_due);
            CREATE TABLE IF NOT EXISTS job_runs (
            id TEXT PRIMARY KEY NOT NULL, job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL, scheduled_at TEXT NOT NULL,
            scheduled_order INTEGER NOT NULL, active INTEGER NOT NULL, payload TEXT NOT NULL,
            UNIQUE(job_id, revision, scheduled_at));
            CREATE UNIQUE INDEX IF NOT EXISTS job_active_run ON job_runs(job_id) WHERE active=1;
            CREATE INDEX IF NOT EXISTS run_history ON job_runs(job_id, scheduled_order DESC);
            PRAGMA user_version=1;").map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(Self { connection })
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.connection.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(storage)
    }

    /// Retains the caller's due field for compatibility, but requires the initial
    /// occurrence derived from created_at. Overdue one-shots remain due.
    pub fn create(&mut self, job: JobRecord) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        job.validate()?;
        if job.updated_at != job.created_at {
            return Err(DaemonError::InvalidInput("creation timestamps must match".into()));
        }
        let initial_due = match &job.schedule {
            ScheduleSpec::Once { at } => *at,
            schedule => schedule.next_after(job.created_at)?,
        };
        if job.next_due_at != initial_due {
            return Err(DaemonError::InvalidInput("initial due time does not match schedule".into()));
        }
        if tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=?1)", [&job.id], |r| r.get::<_, bool>(0)).map_err(storage)? {
            return Err(DaemonError::Conflict(format!("job {} already exists", job.id)));
        }
        tx.execute("INSERT INTO jobs(id,payload,paused,next_due) VALUES (?1,?2,?3,?4)",
            params![job.id, encode(&job)?, job.paused, job.next_due_at.timestamp_millis()]).map_err(storage)?;
        tx.commit().map_err(storage)
    }

    pub fn get(&self, id: &str) -> Result<JobRecord> { get_job(&self.connection, id) }

    pub fn list(&self) -> Result<Vec<JobRecord>> {
        let mut stmt = self.connection.prepare("SELECT payload FROM jobs ORDER BY id").map_err(storage)?;
        stmt.query_map([], |r| r.get::<_, String>(0)).map_err(storage)?
            .map(|r| decode(r.map_err(storage)?)).collect()
    }

    /// Pausing prevents new claims; an already running action may still settle.
    /// This is not a schedule edit and therefore does not change its revision.
    pub fn set_paused(&mut self, id: &str, paused: bool, now: DateTime<Utc>) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        let mut job = get_job(&tx, id)?;
        job.paused = paused;
        job.updated_at = now;
        save_job(&tx, &job)?;
        tx.commit().map_err(storage)
    }

    /// Explicit deletion also removes this job's history and invalidates its leases.
    pub fn delete(&mut self, id: &str) -> Result<()> {
        let changed = self.connection.execute("DELETE FROM jobs WHERE id=?1", [id]).map_err(storage)?;
        if changed == 0 { return Err(DaemonError::NotFound(id.into())); }
        Ok(())
    }

    /// Recurring skip_missed jobs tolerate less than one minute of wakeup delay
    /// (the schedule's resolution); older occurrences advance without dispatch.
    /// RunOnce coalesces overdue occurrences. Expired Running runs are ambiguous,
    /// never returned for replay. Limits bound returned claims, not recovery work.
    pub fn claim_due(&mut self, now: DateTime<Utc>, owner: &str, lease: Duration, limit: usize) -> Result<Vec<JobRunRecord>> {
        self.claim_due_matching(now, owner, lease, limit, |_| true)
    }

    pub fn claim_due_matching(&mut self, now: DateTime<Utc>, owner: &str, lease: Duration, limit: usize, mut eligible: impl FnMut(&JobRecord) -> bool) -> Result<Vec<JobRunRecord>> {
        if owner.trim().is_empty() || lease <= Duration::zero() {
            return Err(DaemonError::InvalidInput("lease owner and positive duration required".into()));
        }
        let expires = now.checked_add_signed(lease).ok_or_else(|| DaemonError::InvalidInput("lease overflow".into()))?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        let active_runs = {
            let mut stmt = tx.prepare("SELECT payload FROM job_runs WHERE active=1").map_err(storage)?;
            stmt.query_map([], |r| r.get::<_, String>(0)).map_err(storage)?
                .map(|r| decode::<JobRunRecord>(r.map_err(storage)?)).collect::<Result<Vec<_>>>()?
        };
        for mut run in active_runs {
            if run.state != JobRunState::Running
                || run.lease_expires_at.is_some_and(|expiry| expiry > now)
            {
                continue;
            }
            let mut job = get_job(&tx, &run.job_id)?;
            run.state = JobRunState::Ambiguous;
            run.finished_at = Some(now);
            run.lease_expires_at = None;
            run.error_class = Some("expired_lease".into());
            save_run(&tx, &run)?;
            if run.schedule_revision == job.schedule_revision {
                advance(&mut job, now)?;
                save_job(&tx, &job)?;
            }
        }
        let jobs = {
            let mut stmt = tx.prepare("SELECT payload FROM jobs WHERE paused=0 AND next_due<=?1 ORDER BY next_due,id").map_err(storage)?;
            stmt.query_map([now.timestamp_millis()], |r| r.get::<_, String>(0)).map_err(storage)?
                .map(|r| decode::<JobRecord>(r.map_err(storage)?)).collect::<Result<Vec<_>>>()?
        };
        let mut claims = Vec::new();
        for mut job in jobs {
            if claims.len() >= limit.min(MAX_CLAIMS) { break; }
            if job.next_due_at > now { continue; }
            let active: Option<String> = tx.query_row("SELECT payload FROM job_runs WHERE job_id=?1 AND active=1", [&job.id], |r| r.get(0)).optional().map_err(storage)?;
            if let Some(json) = active {
                let mut run: JobRunRecord = decode(json)?;
                if run.lease_expires_at.is_some_and(|expiry| expiry > now) { continue; }
                if !eligible(&job) { continue; }
                if run.schedule_revision != job.schedule_revision {
                    run.state = JobRunState::Cancelled;
                    run.finished_at = Some(now);
                    run.lease_expires_at = None;
                    run.error_class = Some("expired_lease".into());
                    save_run(&tx, &run)?;
                    if run.schedule_revision == job.schedule_revision { advance(&mut job, now)?; save_job(&tx, &job)?; }
                    continue;
                }
                run.lease_fence = run.lease_fence.checked_add(1).ok_or_else(|| storage("lease fence exhausted"))?;
                run.lease_owner = Some(owner.into());
                run.lease_expires_at = Some(expires);
                save_run(&tx, &run)?;
                claims.push(run);
                continue;
            }
            if job.schedule.misfire_policy() == MisfirePolicy::SkipMissed && now.signed_duration_since(job.next_due_at) >= Duration::minutes(1) {
                advance(&mut job, now)?;
                save_job(&tx, &job)?;
                continue;
            }
            let id: String = tx.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0)).map_err(storage)?;
            if !eligible(&job) { continue; }
            let run = JobRunRecord { id: id.clone(), idempotency_key: id, job_id: job.id.clone(),
                schedule_revision: job.schedule_revision, scheduled_at: job.next_due_at,
                state: JobRunState::Claimed, attempt: 1, lease_owner: Some(owner.into()), lease_fence: 1,
                lease_expires_at: Some(expires), started_at: None, finished_at: None,
                result_summary: None, error_class: None, output: None };
            tx.execute("INSERT INTO job_runs(id,job_id,revision,scheduled_at,scheduled_order,active,payload) VALUES(?1,?2,?3,?4,?5,1,?6)",
                params![run.id, run.job_id, run.schedule_revision, run.scheduled_at.to_rfc3339(), run.scheduled_at.timestamp_millis(), encode(&run)?]).map_err(storage)?;
            claims.push(run);
        }
        tx.commit().map_err(storage)?;
        Ok(claims)
    }

    pub fn settle_run(&mut self, id: &str, owner: &str, fence: i64, settlement: RunSettlement) -> Result<()> {
        self.transition_run(id, owner, fence, settlement, false, false)
    }

    /// Retry state reuses the occurrence/idempotency key. The claim's expiry is
    /// its durable retry deadline; no action has started for the next attempt.
    pub fn retry_run(&mut self, id: &str, owner: &str, fence: i64, now: DateTime<Utc>, error: String, output: Option<String>) -> Result<()> {
        self.transition_run(id, owner, fence, RunSettlement { state: JobRunState::Failed, finished_at: now, result_summary: Some(error), error_class: Some("transient".into()), output }, false, true)
    }

    pub fn run_now(&mut self, id: &str, now: DateTime<Utc>) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        let mut job = get_job(&tx, id)?;
        let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM job_runs WHERE job_id=?1 AND active=1)", [id], |r| r.get(0)).map_err(storage)?;
        if active || job.paused { return Err(DaemonError::Conflict("job is paused or already active".into())); }
        job.schedule_revision = job.schedule_revision.checked_add(1).ok_or_else(|| storage("schedule revision exhausted"))?;
        job.next_due_at = now;
        job.updated_at = now;
        save_job(&tx, &job)?;
        tx.commit().map_err(storage)
    }

    /// Ignore capacity/session-blocked jobs until completion wakes the loop.
    /// Foreign leases still supply recovery deadlines, including paused jobs.
    pub fn next_wake(&self, now: DateTime<Utc>, active_jobs: &std::collections::HashSet<String>, sessions: &std::collections::HashSet<String>, capacity: bool) -> Result<Option<DateTime<Utc>>> {
        let mut next = None;
        for job in self.list()? {
            if active_jobs.contains(&job.id) { continue; }
            let json: Option<String> = self.connection.query_row("SELECT payload FROM job_runs WHERE job_id=?1 AND active=1", [&job.id], |r| r.get(0)).optional().map_err(storage)?;
            let active = json.map(decode::<JobRunRecord>).transpose()?;
            let deadline = if let Some(run) = active {
                if run.state == JobRunState::Running { run.lease_expires_at }
                else if !job.paused && capacity && !job.target_session.as_ref().is_some_and(|s| sessions.contains(s)) { Some(job.next_due_at.max(run.lease_expires_at.unwrap_or(now))) }
                else { None }
            } else if !job.paused && capacity && !job.target_session.as_ref().is_some_and(|s| sessions.contains(s)) { Some(job.next_due_at) }
            else { None };
            if let Some(deadline) = deadline { next = Some(next.map_or(deadline, |old: DateTime<Utc>| old.min(deadline))); }
        }
        Ok(next)
    }

    /// Cancel only before the durable action boundary, using the current lease.
    pub fn cancel_claimed(&mut self, id: &str, owner: &str, fence: i64, now: DateTime<Utc>, reason: &str) -> Result<()> {
        self.transition_run(id, owner, fence, RunSettlement {
            state: JobRunState::Cancelled, finished_at: now,
            result_summary: None, error_class: Some(reason.into()), output: None,
        }, true, false)
    }

    fn transition_run(&mut self, id: &str, owner: &str, fence: i64, settlement: RunSettlement, cancel_claimed: bool, retry: bool) -> Result<()> {
        if settlement.state == JobRunState::Claimed { return Err(DaemonError::InvalidInput("cannot settle as claimed".into())); }
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        let json: Option<String> = tx.query_row("SELECT payload FROM job_runs WHERE id=?1 AND active=1", [id], |r| r.get(0)).optional().map_err(storage)?;
        let mut run: JobRunRecord = decode(json.ok_or_else(|| DaemonError::Conflict("run is no longer active".into()))?)?;
        let mut job = get_job(&tx, &run.job_id)?;
        if run.lease_owner.as_deref() != Some(owner) || run.lease_fence != fence
            || run.schedule_revision != job.schedule_revision
            || !run.lease_expires_at.is_some_and(|expiry| expiry > settlement.finished_at) {
            return Err(DaemonError::Conflict("stale run lease or schedule revision".into()));
        }
        if settlement.state == JobRunState::Running {
            if run.state != JobRunState::Claimed { return Err(DaemonError::Conflict("action already started".into())); }
            run.state = JobRunState::Running;
            run.started_at = Some(settlement.finished_at);
        } else {
            let required = if cancel_claimed { JobRunState::Claimed } else { JobRunState::Running };
            if run.state != required {
                return Err(DaemonError::Conflict("invalid action boundary transition".into()));
            }
            run.state = settlement.state;
            run.finished_at = Some(settlement.finished_at);
            run.lease_expires_at = None;
            run.result_summary = bounded(settlement.result_summary);
            run.error_class = bounded(settlement.error_class);
            run.output = bounded(settlement.output);
            if retry && run.attempt < job.retry_policy.max_attempts {
                let policy = &job.retry_policy;
                let base = policy.initial_backoff_seconds.saturating_mul(1u64.checked_shl(run.attempt.saturating_sub(1)).unwrap_or(u64::MAX));
                // Stable small jitter survives restarts and never exceeds the cap.
                let jitter = run.id.bytes().fold(0u64, |sum, b| sum + u64::from(b)) % (base / 10 + 1);
                let seconds = base.saturating_add(jitter).min(policy.max_backoff_seconds).min(31_536_000);
                let next = settlement.finished_at + Duration::seconds(seconds.max(1) as i64);
                run.state = JobRunState::Claimed;
                run.attempt += 1;
                run.started_at = None;
                run.finished_at = None;
                run.lease_expires_at = Some(next);
                job.next_due_at = next;
                job.updated_at = settlement.finished_at;
            } else {
                advance(&mut job, settlement.finished_at)?;
            }
            save_job(&tx, &job)?;
        }
        save_run(&tx, &run)?;
        tx.commit().map_err(storage)
    }

    pub fn history(&self, id: &str, limit: usize) -> Result<Vec<JobRunRecord>> {
        self.get(id)?;
        let mut stmt = self.connection.prepare("SELECT payload FROM job_runs WHERE job_id=?1 ORDER BY scheduled_order DESC,rowid DESC LIMIT ?2").map_err(storage)?;
        stmt.query_map(params![id, limit.min(MAX_HISTORY_ROWS) as i64], |r| r.get::<_, String>(0)).map_err(storage)?
            .map(|r| decode(r.map_err(storage)?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::DaemonError;
    use crate::daemon::model::{
        JobAction, JobRecord, JobRunState, MisfirePolicy, RetryPolicy, RunSettlement,
        ScheduleSpec,
    };
    use chrono::{Duration, TimeZone, Timelike, Utc};

    fn at(minute: i64) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap()
            + Duration::minutes(minute)
    }

    fn job(id: &str, due: chrono::DateTime<Utc>) -> JobRecord {
        JobRecord {
            id: id.into(),
            name: format!("job {id}"),
            paused: false,
            schedule: ScheduleSpec::daily(due.hour() as u8, due.minute() as u8, "UTC", MisfirePolicy::SkipMissed).unwrap(),
            action: JobAction::McpCall {
                server: "teams".into(),
                tool: "send_chat_message".into(),
                arguments: serde_json::json!({"message": "hello"}),
                workspace: "/tmp/project".into(),
            },
            workspace: "/tmp/project".into(),
            target_session: None,
            retry_policy: RetryPolicy::default(),
            next_due_at: due,
            schedule_revision: 1,
            created_at: at(-10),
            updated_at: at(-10),
        }
    }

    #[test]
    fn initializes_schema_transactionally() {
        let store = JobStore::in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), 1);
        for table in ["jobs", "job_runs"] {
            let count: i64 = store
                .connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing {table}");
        }
    }

    #[test]
    fn creates_gets_and_lists_jobs_with_json_action_payloads() {
        let mut store = JobStore::in_memory().unwrap();
        let expected = job("b", at(5));
        store.create(expected.clone()).unwrap();
        store.create(job("a", at(4))).unwrap();
        assert_eq!(store.get("b").unwrap(), expected);
        assert_eq!(store.list().unwrap().iter().map(|job| job.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(matches!(store.create(job("b", at(9))), Err(DaemonError::Conflict(_))));
    }

    #[test]
    fn creation_rejects_due_time_that_does_not_match_schedule() {
        let mut store = JobStore::in_memory().unwrap();
        let mut inconsistent = job("wrong-due", at(5));
        inconsistent.schedule = ScheduleSpec::Once { at: at(6) };

        assert!(matches!(
            store.create(inconsistent),
            Err(DaemonError::InvalidInput(_))
        ));
        assert!(matches!(
            store.get("wrong-due"),
            Err(DaemonError::NotFound(_))
        ));
    }

    #[test]
    fn creation_rejects_inconsistent_creation_timestamps() {
        let mut store = JobStore::in_memory().unwrap();
        let mut inconsistent = job("wrong-updated", at(5));
        inconsistent.updated_at = inconsistent.created_at + Duration::seconds(1);

        assert!(matches!(
            store.create(inconsistent),
            Err(DaemonError::InvalidInput(_))
        ));
        assert!(matches!(
            store.get("wrong-updated"),
            Err(DaemonError::NotFound(_))
        ));
    }

    #[test]
    fn pauses_resumes_and_deletes_jobs() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("job-1", at(5))).unwrap();
        store.set_paused("job-1", true, at(1)).unwrap();
        assert!(store.get("job-1").unwrap().paused);
        store.set_paused("job-1", false, at(2)).unwrap();
        assert!(!store.get("job-1").unwrap().paused);
        store.delete("job-1").unwrap();
        assert!(matches!(store.get("job-1"), Err(DaemonError::NotFound(_))));
    }

    #[test]
    fn claims_only_due_unpaused_jobs_and_rejects_duplicate_active_claims() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        store.create(job("later", at(10))).unwrap();
        store.create(job("paused", at(0))).unwrap();
        store.set_paused("paused", true, at(-1)).unwrap();

        let claims = store.claim_due(at(0), "daemon-a", Duration::minutes(5), 10).unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].job_id, "due");
        assert_eq!(claims[0].lease_fence, 1);
        assert!(store.claim_due(at(1), "daemon-b", Duration::minutes(5), 10).unwrap().is_empty());
    }

    #[test]
    fn expired_lease_is_reclaimed_with_a_new_fence() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        let first = store.claim_due(at(0), "daemon-a", Duration::minutes(1), 1).unwrap().remove(0);
        let second = store.claim_due(at(2), "daemon-b", Duration::minutes(1), 1).unwrap().remove(0);
        assert_eq!(second.id, first.id);
        assert_eq!(second.lease_fence, first.lease_fence + 1);
        assert_eq!(second.lease_owner.as_deref(), Some("daemon-b"));
    }

    #[test]
    fn stale_fenced_settlement_is_rejected() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        let first = store.claim_due(at(0), "daemon-a", Duration::minutes(1), 1).unwrap().remove(0);
        let second = store.claim_due(at(2), "daemon-b", Duration::minutes(1), 1).unwrap().remove(0);
        let settlement = RunSettlement {
            state: JobRunState::Succeeded,
            // The replacement lease expires at minute 3; settling at minute 2
            // keeps this test on the valid side of the exclusive lease bound.
            finished_at: at(2),
            result_summary: Some("sent".into()),
            error_class: None,
            output: Some("ok".into()),
        };
        assert!(matches!(
            store.settle_run(&first.id, "daemon-a", first.lease_fence, settlement.clone()),
            Err(DaemonError::Conflict(_))
        ));
        start(&mut store, &second, at(2));
        store.settle_run(&second.id, "daemon-b", second.lease_fence, settlement).unwrap();
        assert_eq!(store.history("due", 10).unwrap()[0].state, JobRunState::Succeeded);
    }

    #[test]
    fn history_is_newest_first_and_bounded() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        let mut last_id = String::new();
        for day in 0..110 {
            let now = at(day * 1440);
            let run = store.claim_due(now, "daemon", Duration::minutes(5), 1).unwrap().remove(0);
            last_id = run.id.clone();
            start(&mut store, &run, now);
            store.settle_run(&run.id, "daemon", run.lease_fence, success(now)).unwrap();
        }
        let history = store.history("due", 1_000).unwrap();
        assert_eq!(history.len(), MAX_HISTORY_ROWS);
        assert_eq!(history[0].id, last_id);
        assert!(store.history("due", 0).unwrap().is_empty());
    }

    fn success(now: chrono::DateTime<Utc>) -> RunSettlement {
        RunSettlement { state: JobRunState::Succeeded, finished_at: now,
            result_summary: None, error_class: None, output: None }
    }

    fn start(store: &mut JobStore, run: &JobRunRecord, now: DateTime<Utc>) {
        store.settle_run(&run.id, run.lease_owner.as_deref().unwrap(), run.lease_fence,
            RunSettlement { state: JobRunState::Running, ..success(now) }).unwrap();
    }

    #[test]
    fn persists_across_reopen_and_coordinates_independent_connections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite");
        let mut first = JobStore::open(&path).unwrap();
        first.create(job("durable", at(0))).unwrap();
        let mut second = JobStore::open(&path).unwrap();
        let run = first.claim_due(at(0), "a", Duration::minutes(5), 1).unwrap().remove(0);
        assert!(second.claim_due(at(0), "b", Duration::minutes(5), 1).unwrap().is_empty());
        start(&mut second, &run, at(0));
        second.settle_run(&run.id, "a", run.lease_fence, success(at(1))).unwrap();
        drop(first);
        drop(second);
        let reopened = JobStore::open(&path).unwrap();
        assert_eq!(reopened.get("durable").unwrap().next_due_at, at(1440));
        assert_eq!(reopened.history("durable", 10).unwrap()[0].state, JobRunState::Succeeded);
    }

    #[test]
    fn misfires_skip_or_coalesce_and_one_shots_remain_due() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("skip", at(0))).unwrap();
        let mut once = job("once", at(0));
        once.schedule = ScheduleSpec::Once { at: at(0) };
        store.create(once).unwrap();
        let mut catchup = job("catchup", at(0));
        catchup.schedule = ScheduleSpec::daily(0, 0, "UTC", MisfirePolicy::RunOnce).unwrap();
        store.create(catchup).unwrap();
        let claims = store.claim_due(at(120), "a", Duration::minutes(5), 10).unwrap();
        assert_eq!(claims.iter().map(|r| r.job_id.as_str()).collect::<Vec<_>>(), ["catchup", "once"]);
        assert_eq!(store.get("skip").unwrap().next_due_at, at(1440));
        for run in claims {
            start(&mut store, &run, at(120));
            store.settle_run(&run.id, "a", run.lease_fence, success(at(121))).unwrap();
        }
        assert!(store.get("once").unwrap().paused);
        assert!(store.claim_due(at(122), "a", Duration::minutes(5), 10).unwrap().is_empty());
    }

    #[test]
    fn rejects_expired_or_duplicate_settlement_and_bounds_utf8_output() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        let run = store.claim_due(at(0), "a", Duration::minutes(5), 1).unwrap().remove(0);
        assert!(matches!(store.settle_run(&run.id, "a", run.lease_fence, success(at(5))), Err(DaemonError::Conflict(_))));
        let mut settlement = success(at(1));
        settlement.output = Some("é".repeat(20_000));
        start(&mut store, &run, at(0));
        store.settle_run(&run.id, "a", run.lease_fence, settlement).unwrap();
        assert!(store.history("due", 1).unwrap()[0].output.as_ref().unwrap().len() <= MAX_OUTPUT_BYTES);
        assert!(matches!(store.settle_run(&run.id, "a", run.lease_fence, success(at(2))), Err(DaemonError::Conflict(_))));
    }

    #[test]
    fn terminal_settlement_requires_running_action_boundary() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        let run = store
            .claim_due(at(0), "a", Duration::minutes(5), 1)
            .unwrap()
            .remove(0);

        assert!(matches!(
            store.settle_run(&run.id, "a", run.lease_fence, success(at(1))),
            Err(DaemonError::Conflict(_))
        ));
        assert_eq!(
            store.history("due", 1).unwrap()[0].state,
            JobRunState::Claimed
        );
    }

    #[test]
    fn creation_validates_first_recurring_occurrence_and_overdue_one_shots() {
        let mut store = JobStore::in_memory().unwrap();
        let mut recurring = job("recurring", at(5));
        recurring.next_due_at = at(1445);
        assert!(matches!(store.create(recurring), Err(DaemonError::InvalidInput(_))));
        assert!(store.list().unwrap().is_empty());

        let mut timestamps = job("timestamps", at(5));
        timestamps.updated_at = at(-11);
        assert!(matches!(store.create(timestamps), Err(DaemonError::InvalidInput(_))));

        for (id, due) in [("overdue", at(-20)), ("immediate", at(-10))] {
            let mut once = job(id, due);
            once.schedule = ScheduleSpec::Once { at: due };
            store.create(once).unwrap();
        }
        assert_eq!(store.claim_due(at(0), "a", Duration::minutes(5), 10).unwrap().len(), 2);
    }

    #[test]
    fn all_terminal_states_require_running() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        let run = store.claim_due(at(0), "a", Duration::minutes(5), 1).unwrap().remove(0);
        for state in [JobRunState::Succeeded, JobRunState::Failed, JobRunState::Cancelled, JobRunState::Ambiguous] {
            assert!(matches!(store.settle_run(&run.id, "a", run.lease_fence,
                RunSettlement { state, ..success(at(1)) }), Err(DaemonError::Conflict(_))));
        }
        assert_eq!(store.get("due").unwrap().next_due_at, at(0));
        start(&mut store, &run, at(1));
        assert!(matches!(store.cancel_claimed(&run.id, "a", run.lease_fence, at(2), "shutdown"), Err(DaemonError::Conflict(_))));
        assert_eq!(store.history("due", 1).unwrap()[0].state, JobRunState::Running);
    }

    #[test]
    fn cancellation_requires_current_unexpired_lease_and_revision() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        let run = store.claim_due(at(0), "a", Duration::minutes(5), 1).unwrap().remove(0);
        for (owner, fence, now) in [("b", run.lease_fence, at(1)), ("a", run.lease_fence + 1, at(1)), ("a", run.lease_fence, at(5))] {
            assert!(matches!(store.cancel_claimed(&run.id, owner, fence, now, "shutdown"), Err(DaemonError::Conflict(_))));
        }
        let mut changed = store.get("due").unwrap();
        changed.schedule_revision += 1;
        save_job(&store.connection, &changed).unwrap();
        assert!(matches!(store.cancel_claimed(&run.id, "a", run.lease_fence, at(1), "shutdown"), Err(DaemonError::Conflict(_))));
        changed.schedule_revision -= 1;
        save_job(&store.connection, &changed).unwrap();
        store.cancel_claimed(&run.id, "a", run.lease_fence, at(1), "shutdown").unwrap();
        assert!(matches!(store.cancel_claimed(&run.id, "a", run.lease_fence, at(2), "shutdown"), Err(DaemonError::Conflict(_))));
        assert_eq!(store.get("due").unwrap().next_due_at, at(1440));
    }

    #[test]
    fn expired_running_is_durably_ambiguous_and_never_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite");
        let mut store = JobStore::open(&path).unwrap();
        let mut once = job("once", at(0));
        once.schedule = ScheduleSpec::Once { at: at(0) };
        store.create(once).unwrap();
        let run = store.claim_due(at(0), "a", Duration::minutes(1), 1).unwrap().remove(0);
        start(&mut store, &run, at(0));
        drop(store);
        let mut store = JobStore::open(&path).unwrap();
        assert_eq!(store.history("once", 1).unwrap()[0].state, JobRunState::Running);
        assert!(store.claim_due(at(1), "b", Duration::minutes(5), 1).unwrap().is_empty());
        assert!(matches!(store.settle_run(&run.id, "a", run.lease_fence, success(at(1))), Err(DaemonError::Conflict(_))));
        drop(store);
        let mut store = JobStore::open(&path).unwrap();
        assert!(store.claim_due(at(10), "c", Duration::minutes(5), 1).unwrap().is_empty());
        let history = store.history("once", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].state, JobRunState::Ambiguous);
        assert_eq!(history[0].started_at, Some(at(0)));
        assert_eq!(history[0].finished_at, Some(at(1)));
        assert!(history[0].lease_expires_at.is_none());
    }

    #[test]
    fn expired_running_is_recovered_while_job_is_paused() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("paused", at(0))).unwrap();
        let run = store.claim_due(at(0), "a", Duration::minutes(1), 1).unwrap().remove(0);
        start(&mut store, &run, at(0));
        store.set_paused("paused", true, at(0)).unwrap();

        assert!(store.claim_due(at(1), "b", Duration::minutes(5), 1).unwrap().is_empty());

        let recovered = store.history("paused", 1).unwrap().remove(0);
        assert_eq!(recovered.state, JobRunState::Ambiguous);
        assert_eq!(recovered.finished_at, Some(at(1)));
        assert!(recovered.lease_expires_at.is_none());
        assert_eq!(recovered.error_class.as_deref(), Some("expired_lease"));
        assert_eq!(store.get("paused").unwrap().next_due_at, at(1440));
        assert!(store.get("paused").unwrap().paused);
    }

    #[test]
    fn claim_limit_does_not_bound_expired_running_recovery() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("running-a", at(0))).unwrap();
        store.create(job("running-b", at(0))).unwrap();
        let running = store.claim_due(at(0), "a", Duration::minutes(1), 2).unwrap();
        for run in &running {
            start(&mut store, run, at(0));
        }
        let mut claim_first = job("claim-first", at(0));
        claim_first.schedule = ScheduleSpec::Once { at: at(0) };
        store.create(claim_first).unwrap();

        let claims = store.claim_due(at(1), "b", Duration::minutes(5), 1).unwrap();

        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].job_id, "claim-first");
        for id in ["running-a", "running-b"] {
            let recovered = store.history(id, 1).unwrap().remove(0);
            assert_eq!(recovered.state, JobRunState::Ambiguous);
            assert_eq!(recovered.finished_at, Some(at(1)));
            assert!(recovered.lease_expires_at.is_none());
            assert_eq!(recovered.error_class.as_deref(), Some("expired_lease"));
            assert_eq!(store.get(id).unwrap().next_due_at, at(1440));
        }
    }

    #[test]
    fn claimed_run_can_be_cancelled_before_external_action() {
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0))).unwrap();
        let run = store
            .claim_due(at(0), "a", Duration::minutes(5), 1)
            .unwrap()
            .remove(0);

        store
            .cancel_claimed(&run.id, "a", run.lease_fence, at(1), "shutdown")
            .unwrap();

        let cancelled = store.history("due", 1).unwrap().remove(0);
        assert_eq!(cancelled.state, JobRunState::Cancelled);
        assert_eq!(cancelled.error_class.as_deref(), Some("shutdown"));
        assert!(cancelled.started_at.is_none());
    }
}
