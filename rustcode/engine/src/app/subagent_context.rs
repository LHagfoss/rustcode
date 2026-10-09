//! Explicit child context snapshots; never borrow a parent's mutable history.
use super::ChatMessage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextInheritance {
    #[default]
    Minimal,
    Evidence,
    Recent,
    Fork,
}

impl ContextInheritance {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "minimal" => Ok(Self::Minimal),
            "evidence" => Ok(Self::Evidence),
            "recent" => Ok(Self::Recent),
            "fork" => Ok(Self::Fork),
            _ => Err("context_inheritance must be minimal, evidence, recent or fork".into()),
        }
    }
}

/// The role a child is spawned with (`agent_type`). A role is a preset over
/// the existing spawn arguments: it selects `write_access`, and everything
/// `write_access` requires still applies.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Read-only unless `write_access` is passed explicitly.
    #[default]
    Default,
    /// Read-only investigation; never writes.
    Explorer,
    /// Implementation; presets `write_access`.
    Worker,
}

impl AgentRole {
    pub(crate) const NAMES: [&'static str; 3] = ["default", "explorer", "worker"];

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "default" => Ok(Self::Default),
            "explorer" => Ok(Self::Explorer),
            "worker" => Ok(Self::Worker),
            _ => Err(format!(
                "unknown agent_type '{value}'. Available agent types: {}",
                Self::NAMES.join(", ")
            )),
        }
    }

    /// The `write_access` this role spawns with, given the explicit argument.
    /// A contradiction is an error rather than one side silently winning.
    pub(crate) fn write_access(self, explicit: Option<bool>) -> Result<bool, String> {
        match (self, explicit) {
            (Self::Default, explicit) => Ok(explicit.unwrap_or(false)),
            (Self::Explorer, Some(true)) => Err(
                "agent_type explorer is read-only; use agent_type worker for write_access".into(),
            ),
            (Self::Explorer, _) => Ok(false),
            (Self::Worker, Some(false)) => Err(
                "agent_type worker writes; use agent_type explorer for a read-only child".into(),
            ),
            (Self::Worker, _) => Ok(true),
        }
    }

    /// Whether `spawn_agent` arguments ask for a child that may write, by
    /// `write_access` or by a role that presets it.
    pub(crate) fn spawn_requests_write(args: &serde_json::Value) -> bool {
        args.get("write_access")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            || args.get("agent_type").and_then(serde_json::Value::as_str) == Some("worker")
    }

    /// What the child is told about its role.
    pub(crate) fn instructions(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Explorer => {
                " Role: explorer. Investigate and report findings with file paths and evidence; do not change anything."
            }
            Self::Worker => {
                " Role: worker. Implement the task inside allowed_paths, verify it, and report what changed."
            }
        }
    }
}

pub(crate) fn inherit_context(
    parent: &[ChatMessage],
    task: &str,
    strategy: ContextInheritance,
    evidence: Option<&str>,
) -> Vec<ChatMessage> {
    // Start at user-turn boundaries so native assistant/tool groups stay intact.
    let mut history = match strategy {
        ContextInheritance::Minimal | ContextInheritance::Evidence => Vec::new(),
        ContextInheritance::Recent => {
            let start = parent
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, m)| m.role == "user")
                .nth(2)
                .map(|(i, _)| i)
                .unwrap_or(0);
            parent[start..].to_vec()
        }
        ContextInheritance::Fork => parent.to_vec(),
    };
    // Parent control instructions and reminders are not the child's policy.
    history.retain(|m| m.role != "system");
    // A spawn may occur inside a parent native tool batch before its outputs exist.
    // Never fork that incomplete assistant/tool group into a provider request.
    if let Some(index) = history.iter().enumerate().find_map(|(index, message)| {
        (!message.tool_calls.is_empty()
            && message.tool_calls.iter().any(|call| {
                !history[index + 1..].iter().any(|result| {
                    result.role == "tool"
                        && result.tool_call_id.as_deref() == Some(call.id.as_str())
                })
            }))
        .then_some(index)
    }) {
        history.truncate(index);
    }
    if strategy != ContextInheritance::Minimal
        && let Some(evidence) = evidence.filter(|text| !text.trim().is_empty())
    {
        history.push(ChatMessage::new(
            "user",
            format!("Selected evidence:\n{evidence}"),
        ));
    }
    history.push(ChatMessage::new("user", task));
    history
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inheritance_is_explicit_and_keeps_native_tool_pairs_isolated() {
        let mut parent = vec![
            ChatMessage::new("system", "parent policy"),
            ChatMessage::new("user", "old"),
            ChatMessage::new("assistant", "call"),
            ChatMessage::new("tool", "result"),
        ];
        let minimal = inherit_context(
            &parent,
            "child",
            ContextInheritance::Minimal,
            Some("evidence"),
        );
        assert_eq!(minimal.len(), 1);
        assert_eq!(minimal[0].role, "user");
        assert_eq!(minimal[0].content, "child");
        let evidence =
            inherit_context(&parent, "child", ContextInheritance::Evidence, Some("fact"));
        assert_eq!(evidence.len(), 2);
        let fork = inherit_context(&parent, "child", ContextInheritance::Fork, None);
        assert_eq!(
            fork.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(),
            ["user", "assistant", "tool", "user"]
        );
        parent[2].content = "changed parent".into();
        assert_eq!(fork[1].content, "call");
    }
    #[test]
    fn fork_excludes_parent_tool_batch_that_has_not_received_results() {
        let mut assistant = ChatMessage::new("assistant", "");
        assistant.tool_calls = vec![super::super::ToolCallRef {
            id: "spawn-1".into(),
            name: "spawn_agent".into(),
            arguments: "{}".into(),
        }];
        let parent = vec![ChatMessage::new("user", "parent"), assistant.clone()];
        let fork = inherit_context(&parent, "child", ContextInheritance::Fork, None);
        assert_eq!(fork.len(), 2);
        assert_eq!(fork[0].content, "parent");
        let paired = vec![
            ChatMessage::new("user", "parent"),
            assistant,
            ChatMessage::new("tool", "spawned").answering(Some("spawn-1".into())),
        ];
        assert_eq!(
            inherit_context(&paired, "child", ContextInheritance::Fork, None).len(),
            4
        );
    }

    #[test]
    fn recent_context_starts_at_a_complete_user_turn() {
        let parent = (0..5)
            .flat_map(|i| {
                [
                    ChatMessage::new("user", format!("task {i}")),
                    ChatMessage::new("assistant", "done"),
                ]
            })
            .collect::<Vec<_>>();
        let recent = inherit_context(&parent, "child", ContextInheritance::Recent, None);
        assert_eq!(recent.len(), 7);
        assert_eq!(recent[0].content, "task 2");
    }
}
