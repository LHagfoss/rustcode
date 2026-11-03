//! Background work a harness keeps running outside its prompt turns: Claude
//! Code's async tasks (`async_task_spawned`, `async_task_progress`,
//! `async_task_state_update`, the `asyncTasks` capability of the JetBrains AIR
//! extension that `claude-agent-acp` speaks).
//!
//! A command sent to the background, a `Monitor`, and the turn Claude Code
//! starts on its own when one of them finishes (a task of type `turn`) each
//! arrive as a task. They become what a RustCode thread already reports for
//! its own background commands: `_rustcode/session/activity` (`waiting` with
//! a count, `running` while a turn it started on its own works, `idle`), and
//! a `completed` or `failed` update for the tool call that started a task once
//! the task ends, so its row stops running then rather than at the call's
//! result.

use agent_client_protocol::schema::v1::{
    SessionUpdate, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use serde_json::Value;
use std::collections::HashMap;

/// The client capabilities `_meta` that asks for async tasks.
pub(crate) fn capabilities_meta() -> Value {
    serde_json::json!({ "jetbrains": { "air": { "version": 1, "capabilities": ["asyncTasks"] } } })
}

/// What a thread does outside a prompt, as `_rustcode/session/activity` says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activity {
    pub state: &'static str,
    pub background: usize,
}

#[derive(Debug, Default)]
struct Task {
    turn: bool,
    tool_call: Option<String>,
}

/// The tasks of one harness session that still run.
#[derive(Debug, Default)]
pub(crate) struct Background {
    tasks: HashMap<String, Task>,
    /// The activity last reported, so only a change goes out.
    reported: Option<Activity>,
}

/// What one task update turns into.
#[derive(Debug, Default)]
pub(crate) struct Effects {
    pub activity: Option<Activity>,
    pub update: Option<SessionUpdate>,
}

impl Background {
    /// Whether `update` is a task update. Each one is answered by [`Self::apply`].
    pub(crate) fn is_task_update(update: &Value) -> bool {
        update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.starts_with("async_task_"))
    }

    pub(crate) fn apply(&mut self, update: &Value) -> Effects {
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(id) = update.get("asyncTaskId").and_then(Value::as_str) else {
            return Effects::default();
        };
        let tool_call = update
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut effects = Effects::default();
        match kind {
            "async_task_spawned" => {
                let task = self.tasks.entry(id.to_owned()).or_default();
                task.turn = update.get("taskType").and_then(Value::as_str) == Some("turn");
                task.tool_call = tool_call.or(task.tool_call.take());
            }
            "async_task_progress" => {
                if let (Some(task), Some(tool_call)) = (self.tasks.get_mut(id), tool_call) {
                    task.tool_call = Some(tool_call);
                }
            }
            "async_task_state_update" => {
                let state = update
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if matches!(state, "completed" | "failed" | "stopped") {
                    if let Some(task) = self.tasks.remove(id)
                        && let Some(tool_call) = tool_call.or(task.tool_call)
                    {
                        let status = if state == "completed" {
                            ToolCallStatus::Completed
                        } else {
                            ToolCallStatus::Failed
                        };
                        effects.update = Some(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                            tool_call,
                            ToolCallUpdateFields::new().status(status),
                        )));
                    }
                } else if let (Some(task), Some(tool_call)) = (self.tasks.get_mut(id), tool_call) {
                    task.tool_call = Some(tool_call);
                }
            }
            _ => {}
        }
        let now = self.activity();
        if self.reported.as_ref() != Some(&now) {
            self.reported = Some(now.clone());
            effects.activity = Some(now);
        }
        effects
    }

    fn activity(&self) -> Activity {
        let background = self.tasks.values().filter(|task| !task.turn).count();
        let state = if self.tasks.values().any(|task| task.turn) {
            "running"
        } else if background > 0 {
            "waiting"
        } else {
            "idle"
        };
        Activity { state, background }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_background_command_waits_then_its_call_completes() {
        let mut background = Background::default();
        let spawned = background.apply(&json!({
            "sessionUpdate": "async_task_spawned", "asyncTaskId": "b1",
            "taskType": "local_bash", "toolCallId": "toolu_1",
        }));
        assert_eq!(
            spawned.activity,
            Some(Activity {
                state: "waiting",
                background: 1
            })
        );
        assert!(spawned.update.is_none());

        let done = background.apply(&json!({
            "sessionUpdate": "async_task_state_update", "asyncTaskId": "b1", "state": "completed",
        }));
        assert_eq!(
            done.activity,
            Some(Activity {
                state: "idle",
                background: 0
            })
        );
        let Some(SessionUpdate::ToolCallUpdate(update)) = done.update else {
            panic!("expected a tool call update");
        };
        assert_eq!(update.tool_call_id.to_string(), "toolu_1");
        assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
    }

    #[test]
    fn a_turn_claude_starts_runs_until_it_ends() {
        let mut background = Background::default();
        background.apply(&json!({
            "sessionUpdate": "async_task_spawned", "asyncTaskId": "b1", "taskType": "local_bash",
        }));
        let woke = background.apply(&json!({
            "sessionUpdate": "async_task_spawned", "asyncTaskId": "t1", "taskType": "turn",
        }));
        assert_eq!(
            woke.activity,
            Some(Activity {
                state: "running",
                background: 1
            })
        );
        // The command it woke for ends inside the turn: still running.
        let inside = background.apply(&json!({
            "sessionUpdate": "async_task_state_update", "asyncTaskId": "b1", "state": "completed",
        }));
        assert_eq!(
            inside.activity,
            Some(Activity {
                state: "running",
                background: 0
            })
        );
        let ended = background.apply(&json!({
            "sessionUpdate": "async_task_state_update", "asyncTaskId": "t1", "state": "completed",
        }));
        assert_eq!(
            ended.activity,
            Some(Activity {
                state: "idle",
                background: 0
            })
        );
        assert!(ended.update.is_none());
    }

    #[test]
    fn progress_without_change_reports_nothing() {
        let mut background = Background::default();
        background.apply(&json!({
            "sessionUpdate": "async_task_spawned", "asyncTaskId": "b1", "taskType": "local_bash",
        }));
        let progress = background.apply(&json!({
            "sessionUpdate": "async_task_progress", "asyncTaskId": "b1", "toolCallId": "toolu_9",
        }));
        assert!(progress.activity.is_none());
        let stopped = background.apply(&json!({
            "sessionUpdate": "async_task_state_update", "asyncTaskId": "b1", "state": "stopped",
        }));
        let Some(SessionUpdate::ToolCallUpdate(update)) = stopped.update else {
            panic!("expected a tool call update");
        };
        assert_eq!(update.tool_call_id.to_string(), "toolu_9");
        assert_eq!(update.fields.status, Some(ToolCallStatus::Failed));
    }
}
