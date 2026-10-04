//! Request-only evidence lifecycle. Stored transcripts remain immutable.
use crate::app::ChatMessage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceClass {
    Pinned,
    Relevant,
    Recoverable,
    Disposable,
}

pub fn classify(
    message: &ChatMessage,
    current_generation: Option<u64>,
    recent: bool,
) -> EvidenceClass {
    if message.role == "user" || message.role == "system" {
        return EvidenceClass::Pinned;
    }
    let Some(record) = message.tool_result.as_ref() else {
        return EvidenceClass::Relevant;
    };
    if record.replayed && message.content.contains("[Unchanged read replay:") {
        return EvidenceClass::Disposable;
    }
    if !record.success || record.pending || record.truncated {
        return EvidenceClass::Relevant;
    }
    if (record.workspace_epoch.as_deref() != Some(crate::workspace_intelligence::epoch())
        || record
            .workspace_generation
            .zip(current_generation)
            .is_some_and(|(old, current)| old != current))
        && matches!(
            record.tool_name.as_str(),
            "view_file"
                | "grep"
                | "glob"
                | "find_symbol"
                | "project_map"
                | "codebase_map"
                | "get_project_map"
                | "list_directory"
        )
    {
        return EvidenceClass::Recoverable;
    }
    if recent {
        EvidenceClass::Relevant
    } else {
        EvidenceClass::Recoverable
    }
}

pub(crate) fn project(
    message: &ChatMessage,
    generation: Option<u64>,
    recent: bool,
) -> Option<String> {
    match classify(message, generation, recent) {
        EvidenceClass::Pinned | EvidenceClass::Relevant => None,
        EvidenceClass::Disposable => {
            Some("[Duplicate evidence replay; use the retained source evidence.]".into())
        }
        EvidenceClass::Recoverable => {
            let record = message.tool_result.as_ref()?;
            // Mutations and external side effects are irreplaceable evidence.
            if !matches!(
                record.tool_name.as_str(),
                "view_file"
                    | "grep"
                    | "glob"
                    | "find_symbol"
                    | "project_map"
                    | "codebase_map"
                    | "get_project_map"
                    | "list_directory"
            ) {
                return None;
            }
            let source = record
                .inspection
                .as_ref()
                .map(|i| {
                    i.returned_path
                        .as_deref()
                        .or(i.requested_path.as_deref())
                        .unwrap_or("unknown source")
                })
                .unwrap_or("source recorded in transcript");
            Some(format!(
                "[Recoverable evidence: {source}; generation={:?}; fingerprint={}. Reacquire with the original inspection tool before relying on current contents.]",
                record.workspace_generation,
                record.evidence_hash.as_deref().unwrap_or("unknown")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_requirements_and_current_failure_survive_pressure() {
        assert_eq!(
            classify(
                &ChatMessage::new("user", "Preserve cancellation"),
                Some(2),
                false
            ),
            EvidenceClass::Pinned
        );
        let failure = ChatMessage::new("tool", "compiler errors").with_tool_result(
            crate::app::ToolResultRecord {
                tool_name: "run_command".into(),
                success: false,
                ..Default::default()
            },
        );
        assert_eq!(classify(&failure, Some(2), false), EvidenceClass::Relevant);
        assert!(project(&failure, Some(2), false).is_none());
    }
    #[test]
    fn resumed_old_epoch_is_recoverable_even_when_generation_matches() {
        let read = ChatMessage::new("tool", "view_file: old contents").with_tool_result(
            crate::app::ToolResultRecord {
                tool_name: "view_file".into(),
                success: true,
                workspace_generation: Some(1),
                workspace_epoch: Some(uuid::Uuid::new_v4().to_string()),
                evidence_hash: Some("old hash".into()),
                ..Default::default()
            },
        );
        let restored: ChatMessage =
            serde_json::from_slice(&serde_json::to_vec(&read).unwrap()).unwrap();
        assert_eq!(
            classify(&restored, Some(1), true),
            EvidenceClass::Recoverable
        );
        assert!(
            project(&restored, Some(1), true)
                .unwrap()
                .contains("Reacquire")
        );
        let mut legacy = restored;
        legacy.tool_result.as_mut().unwrap().workspace_epoch = None;
        assert_eq!(classify(&legacy, Some(1), true), EvidenceClass::Recoverable);
    }

    #[test]
    fn stale_read_is_recoverable_and_metadata_survives_resume() {
        let read = ChatMessage::new("tool", "view_file: old contents").with_tool_result(
            crate::app::ToolResultRecord {
                tool_name: "view_file".into(),
                success: true,
                workspace_generation: Some(1),
                workspace_epoch: Some(crate::workspace_intelligence::epoch().to_owned()),
                evidence_hash: Some("hash".into()),
                ..Default::default()
            },
        );
        let restored: ChatMessage =
            serde_json::from_slice(&serde_json::to_vec(&read).unwrap()).unwrap();
        assert_eq!(
            classify(&restored, Some(2), true),
            EvidenceClass::Recoverable
        );
        assert!(
            project(&restored, Some(2), true)
                .unwrap()
                .contains("Reacquire")
        );
        assert!(project(&restored, Some(1), true).is_none());
    }
}
