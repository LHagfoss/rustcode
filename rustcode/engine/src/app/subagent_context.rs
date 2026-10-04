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
        assert_eq!(minimal, vec![ChatMessage::new("user", "child")]);
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
