//! Jobs whose action is [`JobAction::Workflow`] on the Jobs page (#4443):
//! saved through `_rustcode/loops/save` with `workflow` set, listed beside
//! Loops with `workflow` and `args` in place of a prompt.

use serde_json::{Value, json};

use super::{LoopsSaveRequest, loop_schedule, run_json, trimmed};
use crate::daemon::{
    command::create_job,
    model::{JobAction, JobRecord, JobRunRecord, ScheduleSpec},
};

/// The workflow job a save request describes. `webhook` is the hash of its
/// webhook secret, if it has one.
pub(super) fn job_from(
    request: LoopsSaveRequest,
    id: &str,
    webhook: Option<String>,
) -> Result<JobRecord, String> {
    let workflow = trimmed(request.workflow.clone()).unwrap_or_default();
    crate::workflow::valid_name(&workflow)?;
    let name = request.name.trim().to_owned();
    if name.is_empty() {
        return Err("a job needs a name".to_owned());
    }
    if let Some(machine) = trimmed(request.machine.clone()) {
        return Err(format!(
            "a workflow job runs on this computer, not on {machine}"
        ));
    }
    let workspace = std::path::Path::new(&request.workspace);
    if !workspace.is_absolute() || !workspace.is_dir() {
        return Err(format!("{} is not a project folder", request.workspace));
    }
    let webhook_events = if webhook.is_some() {
        crate::daemon::webhook::clean_events(&request.webhook_events)?
    } else {
        Vec::new()
    };
    let args = match request.args {
        Value::Null => json!({}),
        args @ Value::Object(_) => args,
        _ => return Err("a workflow's args are a JSON object".to_owned()),
    };
    let schedule = loop_schedule(&request.schedule)?;
    let action = JobAction::Workflow {
        name: workflow,
        workspace: request.workspace.clone(),
        args,
        harness: trimmed(request.harness),
        account: trimmed(request.account),
        webhook,
        webhook_events,
    };
    let schedule_json =
        serde_json::to_string(&schedule).map_err(|error| format!("schedule: {error}"))?;
    let action_json = serde_json::to_string(&action).map_err(|error| format!("action: {error}"))?;
    create_job(
        id,
        &name,
        &request.workspace,
        &schedule_json,
        &action_json,
        None,
        None,
    )
    .map_err(|error| error.message)
}

/// A workflow job as the page shows it. `None` for any other job.
pub(super) fn json(job: &JobRecord, runs: &[JobRunRecord]) -> Option<Value> {
    let JobAction::Workflow {
        name,
        workspace,
        args,
        harness,
        account,
        webhook,
        webhook_events,
    } = &job.action
    else {
        return None;
    };
    let manual = matches!(job.schedule, ScheduleSpec::Manual);
    Some(json!({
        "id": job.id,
        "name": job.name,
        "paused": job.paused,
        "schedule": job.schedule,
        "workspace": workspace,
        "prompt": "",
        "workflow": name,
        "args": args,
        "harness": harness,
        "account": account,
        "readOnly": false,
        "webhook": webhook.is_some(),
        "webhookEvents": webhook_events,
        "nextDueAt": (!manual).then_some(job.next_due_at),
        "createdAt": job.created_at,
        "runs": runs.iter().map(run_json).collect::<Vec<_>>(),
    }))
}

/// The workflows saved in `workspace`'s `.rustcode/workflows/`, by name.
pub(crate) fn saved(workspace: &str) -> Vec<String> {
    let folder = std::path::Path::new(workspace)
        .join(".rustcode")
        .join("workflows");
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_str()?.to_owned();
            (path.extension().and_then(|ext| ext.to_str()) == Some("js")
                && crate::workflow::valid_name(&name).is_ok())
            .then_some(name)
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(workspace: &str) -> LoopsSaveRequest {
        serde_json::from_value(json!({
            "name": " Nightly triage ",
            "workspace": workspace,
            "workflow": " triage ",
            "args": {"label": "bug"},
            "schedule": {"kind": "manual"},
            "harness": "codex",
        }))
        .unwrap()
    }

    #[test]
    fn a_save_with_a_workflow_makes_a_workflow_job() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_str().unwrap();
        let job = super::super::job_from(request(workspace), "job-1", None).unwrap();
        assert_eq!(job.name, "Nightly triage");
        let JobAction::Workflow {
            name,
            args,
            harness,
            ..
        } = &job.action
        else {
            panic!("expected a workflow job, got {:?}", job.action);
        };
        assert_eq!(name, "triage");
        assert_eq!(args, &json!({"label": "bug"}));
        assert_eq!(harness.as_deref(), Some("codex"));
        let shown = json(&job, &[]).unwrap();
        assert_eq!(shown["workflow"], "triage");
        assert_eq!(shown["args"]["label"], "bug");
        assert!(shown["nextDueAt"].is_null());
    }

    #[test]
    fn a_workflow_job_refuses_a_bad_name_args_or_a_machine() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_str().unwrap();
        let mut bad = request(workspace);
        bad.workflow = Some("../escape".into());
        assert!(job_from(bad, "job-1", None).is_err());
        let mut bad = request(workspace);
        bad.args = json!([1]);
        assert!(
            job_from(bad, "job-1", None)
                .unwrap_err()
                .contains("JSON object")
        );
        let mut bad = request(workspace);
        bad.machine = Some("studio".into());
        assert!(job_from(bad, "job-1", None).unwrap_err().contains("studio"));
    }

    #[test]
    fn saved_lists_the_project_workflows_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(".rustcode").join("workflows");
        std::fs::create_dir_all(&folder).unwrap();
        for file in ["triage.js", "audit.js", "notes.md", ".hidden.js"] {
            std::fs::write(folder.join(file), "return 1").unwrap();
        }
        assert_eq!(saved(dir.path().to_str().unwrap()), ["audit", "triage"]);
        assert!(saved("/no/such/folder").is_empty());
    }
}
