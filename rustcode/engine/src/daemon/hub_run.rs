//! Hub Loops (#3957, `docs/architecture/hub-plan.md`): a Loop saved on the
//! fleet's hub with a `machine` runs on that member. The hub keeps the
//! schedule, so a run is due even while the member sleeps; each run is sent
//! over the hub link as `start_session` (or `message_session` for a Loop that
//! reuses one thread), and the hub's mailbox holds it until the member
//! connects again. The member answers it through the same gate as any call
//! from its hub, at the access it granted the hub, and a read-only Loop asks
//! for no more than `ask` there.

use super::executor::RunOutcome;
use crate::remote_gateway::hub_link::HubCallAnswer;
use serde_json::{Value, json};

/// One run of a Loop on a member.
pub(crate) struct MemberRun {
    pub machine: String,
    pub name: String,
    pub prompt: String,
    pub workspace: String,
    pub harness: Option<String>,
    pub account: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub read_only: bool,
    pub thread_id: Option<String>,
}

impl MemberRun {
    /// The tool and arguments the hub sends the member.
    pub(crate) fn call(&self) -> (&'static str, Value) {
        let text = crate::orchestration::local_threads::loop_prompt(
            &self.name,
            &self.prompt,
            self.read_only,
        );
        if let Some(thread_id) = &self.thread_id {
            return (
                crate::orchestration::MESSAGE_SESSION,
                json!({ "session_id": thread_id, "prompt": text }),
            );
        }
        let mut arguments = json!({
            "prompt": text,
            "title": self.name,
            "project": self.workspace,
            "standalone": true,
        });
        for (key, value) in [
            ("harness", &self.harness),
            ("account", &self.account),
            ("model", &self.model),
            ("effort", &self.effort),
        ] {
            if let Some(value) = value {
                arguments[key] = json!(value);
            }
        }
        if self.read_only {
            arguments["access"] = json!("ask");
        }
        (crate::orchestration::START_SESSION, arguments)
    }
}

/// Send one run to its member through this hub's remote gateway.
pub(crate) async fn run(run: MemberRun) -> RunOutcome {
    let Some(config_dir) = crate::config::get_config_dir() else {
        return outcome(
            &run.machine,
            Err("RustCode has no configuration directory".into()),
        );
    };
    let (tool, arguments) = run.call();
    let caller = format!("the job “{}”", run.name);
    let answer = crate::remote_gateway::lifecycle::RemoteLifecycle::new(&config_dir)
        .hub_call(&run.machine, tool, arguments, &caller)
        .await
        .map_err(|error| format!("{error:#}"));
    outcome(&run.machine, answer)
}

/// How a member's answer settles the run. The run ends when the member has
/// started the thread (or the hub has queued it); the thread runs on there.
pub(crate) fn outcome(machine: &str, answer: Result<Option<HubCallAnswer>, String>) -> RunOutcome {
    match answer {
        Ok(Some(HubCallAnswer::Answered {
            success: true,
            content,
        })) => {
            let answer: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
            if answer["queued"].as_bool() == Some(true) {
                return RunOutcome::Succeeded {
                    summary: format!(
                        "{machine} is offline; the hub holds the run until it is back"
                    ),
                    output: Some(json!({ "machine": machine, "queued": true }).to_string()),
                };
            }
            RunOutcome::Succeeded {
                summary: format!("The run started on {machine}"),
                output: Some(
                    json!({ "machine": machine, "thread_id": answer["session_id"] }).to_string(),
                ),
            }
        }
        Ok(Some(HubCallAnswer::Answered { content, .. })) => RunOutcome::Permanent {
            error: content,
            output: Some(json!({ "machine": machine }).to_string()),
        },
        Ok(Some(HubCallAnswer::NotMember)) => RunOutcome::Permanent {
            error: format!("{machine} is not a member of this hub, so the run was not sent"),
            output: Some(json!({ "machine": machine }).to_string()),
        },
        // Like a local run while RustCode is closed: skipped, not failed.
        Ok(None) => RunOutcome::Cancelled {
            output: Some(
                json!({
                    "machine": machine,
                    "skipped": "the hub's remote gateway was not running, so the run was skipped",
                })
                .to_string(),
            ),
        },
        Err(error) => RunOutcome::Permanent {
            error: format!("the hub could not send the run to {machine}: {error}"),
            output: Some(json!({ "machine": machine }).to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member_run() -> MemberRun {
        MemberRun {
            machine: "laptop".into(),
            name: "CI watch".into(),
            prompt: "Check CI on main".into(),
            workspace: "/Users/me/code/app".into(),
            harness: Some("codex".into()),
            account: None,
            model: None,
            effort: Some("medium".into()),
            read_only: true,
            thread_id: None,
        }
    }

    #[test]
    fn a_run_starts_a_standalone_thread_in_the_members_folder_at_ask() {
        let (tool, arguments) = member_run().call();
        assert_eq!(tool, crate::orchestration::START_SESSION);
        assert_eq!(arguments["project"], "/Users/me/code/app");
        assert_eq!(arguments["title"], "CI watch");
        assert_eq!(arguments["harness"], "codex");
        assert_eq!(arguments["effort"], "medium");
        assert_eq!(arguments["standalone"], true);
        assert_eq!(arguments["access"], "ask");
        assert!(arguments.get("account").is_none());
        let prompt = arguments["prompt"].as_str().unwrap();
        assert!(prompt.contains("read-only") && prompt.ends_with("Check CI on main"));
        crate::orchestration::validate(tool, &arguments).unwrap();
    }

    #[test]
    fn a_loop_with_a_thread_messages_it_and_a_writing_loop_asks_no_ceiling() {
        let mut run = member_run();
        run.read_only = false;
        assert!(run.call().1.get("access").is_none());
        run.thread_id = Some("thread-7".into());
        let (tool, arguments) = run.call();
        assert_eq!(tool, crate::orchestration::MESSAGE_SESSION);
        assert_eq!(arguments["session_id"], "thread-7");
    }

    #[test]
    fn the_answer_settles_the_run() {
        let answered = |success: bool, content: Value| {
            outcome(
                "laptop",
                Ok(Some(HubCallAnswer::Answered {
                    success,
                    content: content.to_string(),
                })),
            )
        };
        match answered(true, json!({ "session_id": "s-1", "receipt": "started" })) {
            RunOutcome::Succeeded {
                output: Some(output),
                ..
            } => assert!(output.contains("\"thread_id\":\"s-1\"") && output.contains("laptop")),
            other => panic!("{other:?}"),
        }
        match answered(true, json!({ "queued": true })) {
            RunOutcome::Succeeded {
                output: Some(output),
                ..
            } => assert!(output.contains("\"queued\":true")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            answered(false, json!("error: not allowed")),
            RunOutcome::Permanent { .. }
        ));
        assert!(matches!(
            outcome("laptop", Ok(None)),
            RunOutcome::Cancelled { output: Some(ref output) } if output.contains("skipped")
        ));
        assert!(matches!(
            outcome("laptop", Ok(Some(HubCallAnswer::NotMember))),
            RunOutcome::Permanent { .. }
        ));
    }
}
