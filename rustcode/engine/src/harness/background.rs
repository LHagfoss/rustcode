//! Background work a harness keeps running outside its prompt turns:
//!
//! - Claude Code's explicit async tasks (`async_task_spawned`,
//!   `async_task_progress`, `async_task_state_update`, the `asyncTasks`
//!   capability of the JetBrains AIR extension that `claude-agent-acp` speaks);
//! - Implicit background tools and autonomous wakeups for ACP harnesses without
//!   `async_task_*` (such as Antigravity, which backgrounds long shell commands
//!   while returning `end_turn` and later streams a continuation turn outside
//!   `session/prompt`, finishing with a terminal `usage_update`).
//!
//! Both become what a RustCode thread already reports for its own background
//! commands: `_rustcode/session/activity` (`waiting` with a count, `running`
//! while a turn it started on its own works, `idle`), and for explicit tasks a
//! `completed` or `failed` update for the tool call that started a task once
//! the task ends, so its row stops running then rather than at the call's
//! result.

use agent_client_protocol::schema::v1::{
    Meta, SessionUpdate, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

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
    /// True once the harness speaks JetBrains AIR `async_task_*`, which then
    /// owns activity reporting exclusively.
    explicit: bool,
    /// Whether a `session/prompt` request is currently in flight.
    in_prompt: bool,
    /// Tool calls currently `pending` or `in_progress` in the active turn.
    open_tools: HashSet<String>,
    /// Tool calls left `pending` or `in_progress` when an Antigravity turn
    /// ended normally (`end_turn` or terminal `usage_update`).
    background_tools: HashSet<String>,
    /// Whether the harness is streaming an autonomous continuation turn outside
    /// `session/prompt`.
    implicit_turn: bool,
    /// The activity last reported, so only a change goes out.
    reported: Option<Activity>,
}

/// What one task update turns into.
#[derive(Debug, Default)]
pub(crate) struct Effects {
    pub activity: Option<Activity>,
    pub update: Option<SessionUpdate>,
}

fn is_compaction_notice(meta: Option<&Meta>) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("kind"))
        .and_then(Value::as_str)
        == Some("compaction")
}

fn is_implicit_turn_work(update: &SessionUpdate) -> bool {
    match update {
        SessionUpdate::AgentThoughtChunk(_)
        | SessionUpdate::ToolCall(_)
        | SessionUpdate::ToolCallUpdate(_)
        | SessionUpdate::Plan(_) => true,
        SessionUpdate::AgentMessageChunk(chunk) => !is_compaction_notice(chunk.meta.as_ref()),
        _ => false,
    }
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
        self.explicit = true;
        self.implicit_turn = false;
        self.open_tools.clear();
        self.background_tools.clear();
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
        effects.activity = self.emit();
        effects
    }

    /// A `session/prompt` request is starting.
    pub(crate) fn begin_prompt(&mut self) -> Option<Activity> {
        self.in_prompt = true;
        if self.explicit {
            return None;
        }
        self.implicit_turn = false;
        self.open_tools.clear();
        self.emit()
    }

    /// The `session/prompt` request finished. On Antigravity, any tool calls
    /// still pending or in progress when `end_turn` returns are background
    /// commands that will wake the agent when they finish.
    pub(crate) fn end_prompt(
        &mut self,
        completed_normally: bool,
        antigravity: bool,
    ) -> Option<Activity> {
        self.in_prompt = false;
        if self.explicit {
            return None;
        }
        if completed_normally && antigravity {
            self.background_tools.extend(self.open_tools.drain());
        } else {
            self.open_tools.clear();
        }
        self.emit()
    }

    /// Track a regular `session/update` for implicit background tools and
    /// autonomous turns outside `session/prompt`.
    pub(crate) fn observe_update(
        &mut self,
        update: &SessionUpdate,
        antigravity: bool,
    ) -> Option<Activity> {
        if self.explicit {
            return None;
        }
        match update {
            SessionUpdate::ToolCall(call) => {
                self.set_tool(call.tool_call_id.0.as_ref(), call.status);
            }
            SessionUpdate::ToolCallUpdate(call) => {
                if let Some(status) = call.fields.status {
                    self.set_tool(call.tool_call_id.0.as_ref(), status);
                }
            }
            _ => {}
        }
        if self.in_prompt {
            return self.emit();
        }
        if matches!(update, SessionUpdate::UsageUpdate(_)) {
            if self.implicit_turn && antigravity {
                return self.finish_implicit_turn(true);
            }
            return self.emit();
        }
        if is_implicit_turn_work(update) {
            self.implicit_turn = true;
        }
        self.emit()
    }

    /// A `session/request_permission` arrived outside `session/prompt`: the
    /// agent is actively running an autonomous turn.
    pub(crate) fn observe_permission(&mut self) -> Option<Activity> {
        if self.explicit || self.in_prompt {
            return None;
        }
        self.implicit_turn = true;
        self.emit()
    }

    /// Settle an implicit autonomous turn after a quiet fallback window.
    pub(crate) fn settle_implicit_turn(&mut self, antigravity: bool) -> Option<Activity> {
        if self.explicit || self.in_prompt || !self.implicit_turn {
            return None;
        }
        self.finish_implicit_turn(antigravity)
    }

    /// The harness process disconnected: no background work or autonomous turn
    /// remains live.
    pub(crate) fn disconnect(&mut self) -> Option<Activity> {
        self.tasks.clear();
        self.in_prompt = false;
        self.open_tools.clear();
        self.background_tools.clear();
        self.implicit_turn = false;
        self.emit()
    }

    /// Whether an implicit autonomous turn is currently running outside a prompt.
    pub(crate) fn implicit_turn_running(&self) -> bool {
        !self.explicit && !self.in_prompt && self.implicit_turn
    }

    /// Whether the active turn has a non-background tool call still running.
    pub(crate) fn has_open_tools(&self) -> bool {
        !self.open_tools.is_empty()
    }

    fn set_tool(&mut self, id: &str, status: ToolCallStatus) {
        match status {
            ToolCallStatus::Pending | ToolCallStatus::InProgress => {
                if !self.background_tools.contains(id) {
                    self.open_tools.insert(id.to_owned());
                }
            }
            _ => {
                self.open_tools.remove(id);
                self.background_tools.remove(id);
            }
        }
    }

    fn finish_implicit_turn(&mut self, antigravity: bool) -> Option<Activity> {
        self.implicit_turn = false;
        if antigravity {
            self.background_tools.extend(self.open_tools.drain());
        } else {
            self.open_tools.clear();
        }
        self.emit()
    }

    fn emit(&mut self) -> Option<Activity> {
        let now = self.activity();
        if self.reported.as_ref() == Some(&now) {
            return None;
        }
        if self.reported.is_none() && now.state == "idle" {
            return None;
        }
        self.reported = Some(now.clone());
        Some(now)
    }

    fn activity(&self) -> Activity {
        if self.explicit {
            let background = self.tasks.values().filter(|task| !task.turn).count();
            let state = if self.tasks.values().any(|task| task.turn) {
                "running"
            } else if background > 0 {
                "waiting"
            } else {
                "idle"
            };
            return Activity { state, background };
        }
        let background = self.background_tools.len();
        let state = if self.implicit_turn {
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

    fn update(value: Value) -> SessionUpdate {
        serde_json::from_value(value).unwrap()
    }

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

    #[test]
    fn antigravity_background_command_waits_wakes_and_ends_on_usage_update() {
        let mut background = Background::default();
        assert_eq!(background.begin_prompt(), None);
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "tool_call", "toolCallId": "c1",
                    "title": "Run checks", "kind": "execute", "status": "in_progress"
                })),
                true,
            ),
            None
        );
        // Prompt returns end_turn while `c1` is still running in the background.
        assert_eq!(
            background.end_prompt(true, true),
            Some(Activity {
                state: "waiting",
                background: 1
            })
        );
        // `c1` completes outside `session/prompt`: agent wakes up autonomously.
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": "c1",
                    "status": "completed"
                })),
                true,
            ),
            Some(Activity {
                state: "running",
                background: 0
            })
        );
        assert!(background.implicit_turn_running());
        // Launches a second background command `c2` during the woken turn.
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "tool_call", "toolCallId": "c2",
                    "title": "Merge PR", "kind": "execute", "status": "in_progress"
                })),
                true,
            ),
            None
        );
        assert!(background.has_open_tools());
        // Terminal `usage_update` ends the woken turn and parks on `c2`.
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "usage_update", "used": 1000, "size": 200000
                })),
                true,
            ),
            Some(Activity {
                state: "waiting",
                background: 1
            })
        );
        assert!(!background.implicit_turn_running());
        // `c2` finishes and the final woken turn ends with no open tools.
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": "c2",
                    "status": "completed"
                })),
                true,
            ),
            Some(Activity {
                state: "running",
                background: 0
            })
        );
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "usage_update", "used": 1200, "size": 200000
                })),
                true,
            ),
            Some(Activity {
                state: "idle",
                background: 0
            })
        );
    }

    #[test]
    fn explicit_async_tasks_take_exclusive_control() {
        let mut background = Background::default();
        background.apply(&json!({
            "sessionUpdate": "async_task_spawned", "asyncTaskId": "b1", "taskType": "local_bash",
        }));
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "unsolicited"}
                })),
                false,
            ),
            None
        );
        assert!(!background.implicit_turn_running());
    }

    #[test]
    fn unsolicited_turn_on_non_antigravity_settles_and_disconnect_resets() {
        let mut background = Background::default();
        assert_eq!(background.begin_prompt(), None);
        assert_eq!(background.end_prompt(true, false), None);
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "thinking"}
                })),
                false,
            ),
            Some(Activity {
                state: "running",
                background: 0
            })
        );
        // Non-Antigravity does not treat `usage_update` as end-of-turn.
        assert_eq!(
            background.observe_update(
                &update(json!({
                    "sessionUpdate": "usage_update", "used": 500, "size": 100000
                })),
                false,
            ),
            None
        );
        assert_eq!(
            background.settle_implicit_turn(false),
            Some(Activity {
                state: "idle",
                background: 0
            })
        );
        assert_eq!(
            background.observe_permission(),
            Some(Activity {
                state: "running",
                background: 0
            })
        );
        assert_eq!(
            background.disconnect(),
            Some(Activity {
                state: "idle",
                background: 0
            })
        );
        assert_eq!(background.disconnect(), None);
    }
}
