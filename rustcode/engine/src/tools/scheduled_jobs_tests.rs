use super::{AuthorizationDecision, ToolCapability, ToolSafety, authorize_tool_with_args};
use crate::daemon::{
    client::{DaemonClient, DaemonTransport, ResponseFuture},
    model::{JobRunRecord, JobRunState},
    protocol::{DaemonRequest, DaemonResponse},
};
use chrono::{TimeZone, Utc};
use serde_json::json;
use std::sync::{Arc, Mutex};

struct RecordingTransport {
    requests: Arc<Mutex<Vec<DaemonRequest>>>,
    response: DaemonResponse,
}

impl DaemonTransport for RecordingTransport {
    fn request(&self, request: DaemonRequest) -> ResponseFuture<'_> {
        self.requests.lock().unwrap().push(request);
        let response = self.response.clone();
        Box::pin(async move { Ok(response) })
    }
}

fn client(response: DaemonResponse) -> (DaemonClient, Arc<Mutex<Vec<DaemonRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = DaemonClient::with_transport(RecordingTransport {
        requests: requests.clone(),
        response,
    });
    (client, requests)
}

#[test]
fn schema_exposes_typed_create_shapes_and_mutations_require_confirmation() {
    let schema = (super::misc::MANAGE_SCHEDULED_JOBS.schema)();
    assert_eq!(
        schema["properties"]["operation"]["enum"],
        json!([
            "create", "list", "pause", "resume", "run", "history", "delete"
        ])
    );
    assert_eq!(
        schema["properties"]["schedule"]["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        schema["properties"]["action"]["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(super::misc::MANAGE_SCHEDULED_JOBS.requires_confirmation);
    assert_eq!(
        super::misc::MANAGE_SCHEDULED_JOBS.safety,
        ToolSafety::ControlPlane
    );
    assert!(
        super::misc::MANAGE_SCHEDULED_JOBS
            .capabilities
            .contains(&ToolCapability::SessionState)
    );
    assert_eq!(
        authorize_tool_with_args(
            "manage_scheduled_jobs",
            &json!({"operation":"list"}),
            crate::config::AgentMode::Build,
            false,
            false,
        ),
        AuthorizationDecision::RequireConfirmation
    );
}

#[test]
fn create_validates_required_fields_and_sends_typed_job() {
    let (client, requests) = client(DaemonResponse::Ack);
    let error =
        super::misc::manage_scheduled_jobs_with_client(&json!({"operation":"create"}), &client)
            .unwrap_err();
    assert!(error.contains("missing 'id'"), "{error}");

    let output = super::misc::manage_scheduled_jobs_with_client(
        &json!({
            "operation":"create", "id":"morning", "name":"Morning report", "workspace":"/work",
            "schedule":{"kind":"daily", "hour":8, "minute":30, "timezone":"Europe/Oslo"},
            "action":{"type":"prompt", "prompt":"Summarize", "workspace":"/work"}
        }),
        &client,
    )
    .unwrap();
    assert_eq!(output, "Scheduled job created.");
    let requests = requests.lock().unwrap();
    let DaemonRequest::Create { job } = &requests[0] else {
        panic!("expected create")
    };
    assert_eq!(job.id, "morning");
    assert_eq!(
        job.next_due_at,
        job.schedule.next_after(job.created_at).unwrap()
    );
}

#[test]
fn list_and_history_are_compact_and_bounded() {
    let due = Utc.with_ymd_and_hms(2026, 9, 23, 8, 0, 0).unwrap();
    let jobs = (0..150)
        .map(|index| crate::daemon::model::JobRecord {
            id: format!("job-{index}"),
            name: "x".repeat(200),
            paused: false,
            schedule: crate::daemon::model::ScheduleSpec::Once { at: due },
            action: crate::daemon::model::JobAction::Prompt {
                prompt: "secret long prompt".into(),
                workspace: "/work".into(),
                model_profile: None,
                session_id: None,
                settings: None,
            },
            workspace: "/work".into(),
            target_session: None,
            retry_policy: Default::default(),
            next_due_at: due,
            schedule_revision: 1,
            created_at: due,
            updated_at: due,
        })
        .collect();
    let (list_client, _) = client(DaemonResponse::Jobs { jobs });
    let output =
        super::misc::manage_scheduled_jobs_with_client(&json!({"operation":"list"}), &list_client)
            .unwrap();
    assert!(output.len() <= 16_384);
    assert!(serde_json::from_str::<serde_json::Value>(&output).is_ok());
    assert!(output.contains("job-0"));
    assert!(!output.contains("secret long prompt"));
    assert!(output.contains("truncated"));

    let runs = (0..100)
        .map(|index| JobRunRecord {
            id: format!("run-{index}"),
            job_id: "job-0".into(),
            schedule_revision: 1,
            idempotency_key: format!("key-{index}"),
            scheduled_at: due,
            state: JobRunState::Failed,
            attempt: 1,
            lease_owner: None,
            lease_fence: 1,
            lease_expires_at: None,
            started_at: Some(due),
            finished_at: Some(due),
            result_summary: Some("summary".into()),
            error_class: Some("error".into()),
            output: Some("x".repeat(100_000)),
        })
        .collect();
    let (client, requests) = client(DaemonResponse::History { runs });
    let output = super::misc::manage_scheduled_jobs_with_client(
        &json!({"operation":"history", "job_id":"job-0", "limit":999}),
        &client,
    )
    .unwrap();
    assert!(output.len() <= 16_384);
    assert!(serde_json::from_str::<serde_json::Value>(&output).is_ok());
    assert!(!output.contains(&"x".repeat(100)));
    assert!(matches!(
        requests.lock().unwrap()[0],
        DaemonRequest::History { limit: 50, .. }
    ));
}

#[test]
fn all_control_operations_use_protocol_and_errors_are_clear() {
    let cases = [
        (
            "pause",
            DaemonRequest::SetPaused {
                job_id: "missing".into(),
                paused: true,
            },
        ),
        (
            "resume",
            DaemonRequest::SetPaused {
                job_id: "missing".into(),
                paused: false,
            },
        ),
        (
            "run",
            DaemonRequest::RunNow {
                job_id: "missing".into(),
            },
        ),
        (
            "delete",
            DaemonRequest::Delete {
                job_id: "missing".into(),
            },
        ),
    ];
    for (operation, expected) in cases {
        let (client, requests) = client(DaemonResponse::Error {
            code: "not_found".into(),
            message: "unknown job missing".into(),
        });
        let error = super::misc::manage_scheduled_jobs_with_client(
            &json!({"operation":operation,"job_id":"missing"}),
            &client,
        )
        .unwrap_err();
        assert_eq!(error, "error: unknown job missing");
        assert_eq!(requests.lock().unwrap()[0], expected);
    }

    struct Offline;
    impl DaemonTransport for Offline {
        fn request(&self, _request: DaemonRequest) -> ResponseFuture<'_> {
            Box::pin(async { Err(anyhow::anyhow!("connection refused")) })
        }
    }
    let error = super::misc::manage_scheduled_jobs_with_client(
        &json!({"operation":"list"}),
        &DaemonClient::with_transport(Offline),
    )
    .unwrap_err();
    assert!(error.starts_with("error: daemon unavailable:"), "{error}");
}
