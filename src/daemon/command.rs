use super::{
    model::{JobAction, JobRecord, ScheduleSpec},
    protocol::DaemonResponse,
};
use chrono::Utc;
use std::fmt;

const MAX_HUMAN_ROWS: usize = 100;
const MAX_HUMAN_OUTPUT_BYTES: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandError {
    pub code: String,
    pub message: String,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError {
        code: "invalid_input".into(),
        message: message.into(),
    }
}

pub(crate) fn create_job(
    id: &str,
    name: &str,
    workspace: &str,
    schedule_json: &str,
    action_json: &str,
    target_session: Option<&str>,
    retry_policy_json: Option<&str>,
) -> Result<JobRecord, CommandError> {
    let now = Utc::now();
    let schedule: ScheduleSpec = serde_json::from_str(schedule_json)
        .map_err(|error| invalid(format!("invalid schedule: {error}")))?;
    schedule
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    let mut action: JobAction = serde_json::from_str(action_json)
        .map_err(|error| invalid(format!("invalid action: {error}")))?;
    hydrate_mcp_actions(&mut action)?;
    let next_due_at = match schedule {
        ScheduleSpec::Once { at } => at,
        _ => schedule
            .next_after(now)
            .map_err(|error| invalid(error.to_string()))?,
    };
    let retry_policy = retry_policy_json
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| invalid(format!("invalid retry_policy: {error}")))?
        .unwrap_or_default();
    let job = JobRecord {
        id: id.to_owned(),
        name: name.to_owned(),
        paused: false,
        schedule,
        action,
        workspace: workspace.to_owned(),
        target_session: target_session.map(str::to_owned),
        retry_policy,
        next_due_at,
        schedule_revision: 1,
        created_at: now,
        updated_at: now,
    };
    job.validate().map_err(|error| invalid(error.to_string()))?;
    Ok(job)
}

fn hydrate_mcp_actions(action: &mut JobAction) -> Result<(), CommandError> {
    match action {
        JobAction::McpCall {
            server,
            workspace,
            server_config,
            ..
        } if server_config.is_none() => {
            let server_name = server.clone();
            let action_workspace = workspace.clone();
            let (_, _, config) =
                crate::config::load_config_for_workspace(std::path::Path::new(&action_workspace));
            *server_config = Some(
                config
                    .mcp_servers
                    .iter()
                    .find(|candidate| candidate.name == server_name)
                    .cloned()
                    .ok_or_else(|| {
                        invalid(format!(
                            "MCP server '{server_name}' is not configured for workspace {action_workspace}"
                        ))
                    })?,
            );
        }
        JobAction::Poll { action, .. } => hydrate_mcp_actions(action)?,
        _ => {}
    }
    Ok(())
}

pub(crate) fn format_response(
    operation: &str,
    response: &DaemonResponse,
    json: bool,
) -> Result<String, CommandError> {
    if let DaemonResponse::Error { code, message } = response {
        return Err(CommandError {
            code: code.clone(),
            message: message.clone(),
        });
    }
    if json {
        return serde_json::to_string(response).map_err(|error| CommandError {
            code: "serialization_failed".into(),
            message: error.to_string(),
        });
    }
    let output = match response {
        DaemonResponse::Status { status } => format!(
            "Daemon: running (pid {})\nUptime: {}s\nActive runs: {}\nNext wake: {}\nSocket: {}\nDatabase: {}",
            status.pid,
            status.uptime_seconds,
            status.active_runs,
            status.next_wake_at.as_deref().unwrap_or("none"),
            status.socket_path,
            status.database_path
        ),
        DaemonResponse::Ack => match operation {
            "pause" => "Scheduled job paused.".into(),
            "resume" => "Scheduled job resumed.".into(),
            "delete" => "Scheduled job deleted.".into(),
            _ => "Command completed.".into(),
        },
        DaemonResponse::Job { job } => format!(
            "Scheduled job '{}' ({}) created; next due {}.",
            job.id, job.name, job.next_due_at
        ),
        DaemonResponse::Jobs { jobs } => {
            if jobs.is_empty() {
                "No scheduled jobs.".into()
            } else {
                let mut rows = jobs
                    .iter()
                    .take(MAX_HUMAN_ROWS)
                    .map(|job| {
                        format!(
                            "{}\t{}\t{}\t{}",
                            job.id,
                            if job.paused { "paused" } else { "active" },
                            job.next_due_at,
                            job.name
                        )
                    })
                    .collect::<Vec<_>>();
                if jobs.len() > rows.len() {
                    rows.push(format!("... {} more", jobs.len() - rows.len()));
                }
                bounded_text(rows.join("\n"))
            }
        }
        DaemonResponse::History { runs } => {
            if runs.is_empty() {
                "No run history.".into()
            } else {
                bounded_text(
                    runs.iter()
                        .take(50)
                        .map(|run| {
                            format!(
                                "{}\t{:?}\tattempt {}\t{}",
                                run.scheduled_at,
                                run.state,
                                run.attempt,
                                run.result_summary.as_deref().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
        }
        DaemonResponse::RunAccepted { job_id, state } => {
            format!("Run accepted for '{job_id}' ({state:?}).")
        }
        DaemonResponse::Error { .. } => unreachable!(),
    };
    Ok(output)
}

fn bounded_text(mut text: String) -> String {
    if text.len() <= MAX_HUMAN_OUTPUT_BYTES {
        return text;
    }
    let mut boundary = MAX_HUMAN_OUTPUT_BYTES.saturating_sub(14);
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    text.push_str("\n... truncated");
    text
}

pub(crate) fn tail_lines(contents: &str, lines: usize) -> String {
    let rows: Vec<&str> = contents.lines().collect();
    let start = rows.len().saturating_sub(lines);
    let mut output = rows[start..].join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{
        model::{MisfirePolicy, ScheduleSpec},
        protocol::{DaemonResponse, DaemonStatus},
    };

    #[test]
    fn formats_status_for_humans_and_json() {
        let response = DaemonResponse::Status {
            status: DaemonStatus {
                pid: 42,
                process_start_time: 7,
                instance_id: "instance".into(),
                uptime_seconds: 12,
                socket_path: "/tmp/control.sock".into(),
                database_path: "/tmp/jobs.sqlite".into(),
                active_runs: 2,
                next_wake_at: Some("2026-09-22T10:00:00Z".into()),
            },
        };
        let human = format_response("status", &response, false).unwrap();
        assert!(human.contains("running (pid 42)"));
        assert!(human.contains("Active runs: 2"));
        let json = format_response("status", &response, true).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()["type"],
            "status"
        );
    }

    #[test]
    fn formats_protocol_errors_as_command_errors() {
        let error = format_response(
            "history",
            &DaemonResponse::Error {
                code: "not_found".into(),
                message: "job missing".into(),
            },
            false,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "not_found: job missing");
    }

    #[test]
    fn log_tail_is_line_bounded() {
        assert_eq!(tail_lines("one\ntwo\nthree\n", 2), "two\nthree\n");
        assert_eq!(tail_lines("one\ntwo", 1), "two\n");
    }

    #[test]
    fn create_job_derives_next_due_and_validates_timezone() {
        let job = create_job(
            "job-1",
            "Job",
            "/work",
            r#"{"kind":"daily","hour":9,"minute":0,"timezone":"Europe/Oslo"}"#,
            r#"{"type":"prompt","prompt":"hello","workspace":"/work","model_profile":null,"session_id":null}"#,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(
            job.schedule,
            ScheduleSpec::Daily {
                misfire_policy: MisfirePolicy::SkipMissed,
                ..
            }
        ));
        assert!(job.next_due_at > job.created_at);

        let error = create_job(
            "job-1",
            "Job",
            "/work",
            r#"{"kind":"daily","hour":9,"minute":0,"timezone":"Mars/Olympus"}"#,
            r#"{"type":"prompt","prompt":"hello","workspace":"/work","model_profile":null,"session_id":null}"#,
            None,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("invalid schedule"));
    }
}
