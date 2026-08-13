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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationCell {
    Empty,
    Tail,
    Middle,
    Lead,
}

pub fn is_exploration_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "view_file"
            | "list_directory"
            | "list_dir"
            | "glob"
            | "grep"
            | "grep_search"
            | "find_symbol"
            | "codebase_search"
            | "codebase_symbol"
            | "get_project_map"
    )
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
        let all_exploration = running_tools
            .iter()
            .all(|tool_name| is_exploration_tool(tool_name));
        let representative = running_tools
            .iter()
            .find(|tool_name| !is_exploration_tool(tool_name))
            .unwrap_or(tool_name);
        let (label, detail) = if all_exploration {
            ("Exploring".to_string(), None)
        } else if representative == "run_command" {
            ("Running".to_string(), Some(representative.clone()))
        } else {
            ("Tool".to_string(), Some(representative.clone()))
        };
        return ActivitySnapshot {
            kind: ActivityKind::RunningTool,
            label,
            detail,
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
            detail: None,
            animated: true,
        },
        AppStatus::Idle => ActivitySnapshot {
            kind: ActivityKind::Ready,
            label: "Idle".to_string(),
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

pub fn animation_trail(frame: u64, width: usize) -> Vec<AnimationCell> {
    if width == 0 {
        return Vec::new();
    }

    let period = width.saturating_mul(2).saturating_sub(2).max(1);
    let phase = (frame as usize) % period;
    let reflected = if phase >= width {
        width * 2 - 2 - phase
    } else {
        phase
    };
    let trail_direction = if reflected == 0 {
        1isize
    } else if reflected + 1 >= width {
        -1
    } else if phase < width.saturating_sub(1) {
        -1
    } else {
        1
    };

    (0..width)
        .map(|index| {
            let offset = index as isize - reflected as isize;
            match offset * trail_direction {
                0 => AnimationCell::Lead,
                1 => AnimationCell::Middle,
                2 => AnimationCell::Tail,
                _ => AnimationCell::Empty,
            }
        })
        .collect()
}

pub fn format_terminal_title(kind: ActivityKind, session_name: &str, frame: u64) -> String {
    let session = sanitize_session_name(session_name, 32);
    let prefix = match kind {
        ActivityKind::Ready => "rustcode · Idle".to_string(),
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
        ActivityKind, AnimationCell, animation_trail, classify_activity, format_terminal_title,
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
        assert_eq!(
            classify_activity(&AppStatus::Streaming, &["list_directory".into()]).label,
            "Exploring"
        );
        assert_eq!(
            classify_activity(&AppStatus::Streaming, &["use_skill".into()]).label,
            "Tool"
        );
        assert_eq!(
            classify_activity(
                &AppStatus::Streaming,
                &["list_directory".into(), "run_command".into()]
            )
            .label,
            "Running"
        );
    }

    #[test]
    fn session_names_are_sanitized_and_truncated() {
        assert_eq!(
            sanitize_session_name("  fix | parser\u{0007}\nissue  ", 18),
            "fix / parser issue"
        );
        assert!(
            sanitize_session_name("a very long session name", 12)
                .chars()
                .count()
                <= 12
        );
    }

    #[test]
    fn terminal_title_contains_state_and_short_name() {
        let title = format_terminal_title(ActivityKind::Working, "tower defense", 2);
        assert_eq!(title, "[••] Working · tower defense");
    }

    #[test]
    fn animation_trail_marks_lead_middle_and_tail() {
        assert_eq!(
            animation_trail(2, 6),
            vec![
                AnimationCell::Tail,
                AnimationCell::Middle,
                AnimationCell::Lead,
                AnimationCell::Empty,
                AnimationCell::Empty,
                AnimationCell::Empty,
            ]
        );
    }

    #[test]
    fn animation_trail_keeps_three_roles_visible_at_bounce_edges() {
        let left = animation_trail(0, 6);
        let right = animation_trail(5, 6);

        assert_eq!(
            left,
            vec![
                AnimationCell::Lead,
                AnimationCell::Middle,
                AnimationCell::Tail,
                AnimationCell::Empty,
                AnimationCell::Empty,
                AnimationCell::Empty,
            ]
        );
        assert_eq!(
            right,
            vec![
                AnimationCell::Empty,
                AnimationCell::Empty,
                AnimationCell::Empty,
                AnimationCell::Tail,
                AnimationCell::Middle,
                AnimationCell::Lead,
            ]
        );
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
            "rustcode · Idle · bench"
        );
    }

    #[test]
    fn idle_activity_is_labeled_idle() {
        assert_eq!(classify_activity(&AppStatus::Idle, &[]).label, "Idle");
    }
}
