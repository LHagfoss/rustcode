use super::{DaemonError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicy {
    SkipMissed,
    RunOnce,
}

impl Default for MisfirePolicy {
    fn default() -> Self {
        Self::SkipMissed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff_seconds: u64,
    pub max_backoff_seconds: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_seconds: 30,
            max_backoff_seconds: 900,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobAction {
    McpCall {
        server: String,
        tool: String,
        #[serde(default)]
        arguments: Value,
        workspace: String,
        /// Durable execution context. Legacy jobs must be hydrated before execution.
        #[serde(default)]
        server_config: Option<crate::config::McpServerConfig>,
    },
    Prompt {
        prompt: String,
        workspace: String,
        model_profile: Option<String>,
        session_id: Option<String>,
        #[serde(default)]
        settings: Option<crate::config::SessionSettingsSnapshot>,
    },
    ShellCommand {
        command: String,
        working_directory: String,
        #[serde(default)]
        environment_allowlist: Vec<String>,
        timeout_seconds: u64,
        #[serde(default)]
        authorized: bool,
    },
    Poll {
        action: Box<JobAction>,
        interval_seconds: u64,
        max_runs: u32,
        deadline_seconds: u64,
        #[serde(default)]
        stop_on_change: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleSpec {
    Cron {
        expression: String,
        timezone: String,
        #[serde(default)]
        misfire_policy: MisfirePolicy,
    },
    Daily {
        hour: u8,
        minute: u8,
        timezone: String,
        #[serde(default)]
        misfire_policy: MisfirePolicy,
    },
    Monthly {
        day: u8,
        hour: u8,
        minute: u8,
        timezone: String,
        #[serde(default)]
        misfire_policy: MisfirePolicy,
    },
    Once {
        at: DateTime<Utc>,
    },
}

impl ScheduleSpec {
    pub fn cron(
        expression: impl Into<String>,
        timezone: impl Into<String>,
        misfire_policy: MisfirePolicy,
    ) -> Result<Self> {
        let schedule = Self::Cron {
            expression: expression.into(),
            timezone: timezone.into(),
            misfire_policy,
        };
        schedule.validate()?;
        Ok(schedule)
    }

    pub fn daily(
        hour: u8,
        minute: u8,
        timezone: impl Into<String>,
        misfire_policy: MisfirePolicy,
    ) -> Result<Self> {
        let schedule = Self::Daily {
            hour,
            minute,
            timezone: timezone.into(),
            misfire_policy,
        };
        schedule.validate()?;
        Ok(schedule)
    }

    pub fn monthly(
        day: u8,
        hour: u8,
        minute: u8,
        timezone: impl Into<String>,
        misfire_policy: MisfirePolicy,
    ) -> Result<Self> {
        let schedule = Self::Monthly {
            day,
            hour,
            minute,
            timezone: timezone.into(),
            misfire_policy,
        };
        schedule.validate()?;
        Ok(schedule)
    }

    pub fn validate(&self) -> Result<()> {
        super::schedule::validate(self)
    }

    pub fn next_after(&self, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
        super::schedule::next_after(self, after)
    }

    pub fn misfire_policy(&self) -> MisfirePolicy {
        match self {
            Self::Cron { misfire_policy, .. }
            | Self::Daily { misfire_policy, .. }
            | Self::Monthly { misfire_policy, .. } => *misfire_policy,
            Self::Once { .. } => MisfirePolicy::RunOnce,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub name: String,
    pub paused: bool,
    pub schedule: ScheduleSpec,
    pub action: JobAction,
    pub workspace: String,
    pub target_session: Option<String>,
    pub retry_policy: RetryPolicy,
    pub next_due_at: DateTime<Utc>,
    pub schedule_revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl JobRecord {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Err(DaemonError::InvalidInput(
                "job id and name must not be empty".into(),
            ));
        }
        self.schedule.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobRunState {
    Claimed,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRunRecord {
    pub id: String,
    pub job_id: String,
    pub schedule_revision: i64,
    /// Stable across lease recovery; pass to executors supporting idempotency.
    pub idempotency_key: String,
    pub scheduled_at: DateTime<Utc>,
    pub state: JobRunState,
    pub attempt: u32,
    pub lease_owner: Option<String>,
    pub lease_fence: i64,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result_summary: Option<String>,
    pub error_class: Option<String>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSettlement {
    /// `Running` marks the action boundary; all other accepted states are terminal.
    pub state: JobRunState,
    /// Transition timestamp, also used as `started_at` for the Running transition.
    pub finished_at: DateTime<Utc>,
    pub result_summary: Option<String>,
    pub error_class: Option<String>,
    pub output: Option<String>,
}
