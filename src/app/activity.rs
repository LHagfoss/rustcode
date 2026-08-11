use super::AppStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Ready,
    Queued,
    Working,
    RunningTool,
    ActionRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySnapshot {
    pub kind: ActivityKind,
    pub label: String,
    pub detail: Option<String>,
    pub animated: bool,
}

pub fn classify_activity(status: &AppStatus, running_tools: &[String]) -> ActivitySnapshot {
    let action_required = matches!(
        status,
        AppStatus::AwaitingToolConfirmation
            | AppStatus::AwaitingQuestion
            | AppStatus::VerbosityPicker
            | AppStatus::ThinkingPicker
            | AppStatus::ProtocolPicker
    );

    if action_required {
        return ActivitySnapshot {
            kind: ActivityKind::ActionRequired,
            label: "Action Required".to_string(),
            detail: None,
            animated: true,
        };
    }

    if let Some(tool_name) = running_tools.first() {
        return ActivitySnapshot {
            kind: ActivityKind::RunningTool,
            label: "Running".to_string(),
            detail: Some(tool_name.clone()),
            animated: true,
        };
    }

    match status {
        AppStatus::Queued => ActivitySnapshot {
            kind: ActivityKind::Queued,
            label: "Queued".to_string(),
            detail: Some("waiting for model".to_string()),
            animated: true,
        },
        AppStatus::Streaming => ActivitySnapshot {
            kind: ActivityKind::Working,
            label: "Working".to_string(),
            detail: Some("Responding".to_string()),
            animated: true,
        },
        AppStatus::Idle => ActivitySnapshot {
            kind: ActivityKind::Ready,
            label: "Ready".to_string(),
            detail: None,
            animated: false,
        },
        AppStatus::AwaitingToolConfirmation
        | AppStatus::AwaitingQuestion
        | AppStatus::VerbosityPicker
        | AppStatus::ThinkingPicker
        | AppStatus::ProtocolPicker => unreachable!("handled above"),
    }
}

pub fn sanitize_session_name(raw: &str, max_chars: usize) -> String {
    let normalized = raw
        .chars()
        .map(|character| {
            if character == '|' {
                '/'
            } else if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let trimmed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let limited = trimmed.chars().take(max_chars).collect::<String>();
    if limited.is_empty() {
        "session".to_string()
    } else {
        limited
    }
}

pub fn animation_cells(frame: u64, width: usize) -> Vec<bool> {
    if width == 0 {
        return Vec::new();
    }

    let center = (frame as usize) % (width * 2).saturating_sub(2).max(1);
    let reflected = if center >= width {
        width * 2 - 2 - center
    } else {
        center
    };
    (0..width)
        .map(|index| index.abs_diff(reflected) <= 1)
        .collect()
}

pub fn format_terminal_title(kind: ActivityKind, session_name: &str, frame: u64) -> String {
    let session = sanitize_session_name(session_name, 32);
    let prefix = match kind {
        ActivityKind::Ready => "rustcode · Ready".to_string(),
        ActivityKind::Queued => "[>] Queued".to_string(),
        ActivityKind::Working => {
            let marker = ["·", "••", "••", "•••", "••", "·"][(frame as usize) % 6];
            format!("[{marker}] Working")
        }
        ActivityKind::RunningTool => "[•] Running".to_string(),
        ActivityKind::ActionRequired => "[!] Action Required".to_string(),
    };
    format!("{prefix} · {session}")
}

#[cfg(test)]
mod tests {
    use super::{
        ActivityKind, animation_cells, classify_activity, format_terminal_title,
        sanitize_session_name,
    };
    use crate::app::AppStatus;

    #[test]
    fn activity_precedence_prefers_action_required_then_tool_then_queue() {
        assert_eq!(
            classify_activity(&AppStatus::AwaitingQuestion, &["run_command".into()]).kind,
            ActivityKind::ActionRequired
        );
        assert_eq!(
            classify_activity(&AppStatus::Streaming, &["run_command".into()]).kind,
            ActivityKind::RunningTool
        );
        assert_eq!(
            classify_activity(&AppStatus::Queued, &[]).kind,
            ActivityKind::Queued
        );
    }

    #[test]
    fn session_names_are_sanitized_and_truncated() {
        assert_eq!(
            sanitize_session_name("  fix | parser\u{0007}\nissue  ", 18),
            "fix / parser issue"
        );
        assert!(sanitize_session_name("a very long session name", 12)
            .chars()
            .count() <= 12);
    }

    #[test]
    fn terminal_title_contains_state_and_short_name() {
        let title = format_terminal_title(ActivityKind::Working, "tower defense", 2);
        assert_eq!(title, "[••] Working · tower defense");
    }

    #[test]
    fn animation_cells_reach_both_edges() {
        let first = animation_cells(0, 12);
        let later = animation_cells(8, 12);
        assert!(first.iter().any(|cell| *cell));
        assert!(later.iter().any(|cell| *cell));
        assert_ne!(first, later);
    }

    #[test]
    fn title_states_are_compact_and_distinct() {
        assert_eq!(
            format_terminal_title(ActivityKind::Queued, "bench", 0),
            "[>] Queued · bench"
        );
        assert_eq!(
            format_terminal_title(ActivityKind::ActionRequired, "bench", 0),
            "[!] Action Required · bench"
        );
        assert_eq!(
            format_terminal_title(ActivityKind::Ready, "bench", 0),
            "rustcode · Ready · bench"
        );
    }
}
