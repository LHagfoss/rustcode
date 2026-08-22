use super::{AppStatus, LiveToolCall};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalProgress {
    Hidden,
    Indeterminate,
    Paused,
    Error,
}

impl TerminalProgress {
    pub fn osc_sequence(&self) -> &'static str {
        match self {
            Self::Hidden => "\x1b]9;4;0;0\x07",
            Self::Indeterminate => "\x1b]9;4;3;0\x07",
            Self::Paused => "\x1b]9;4;4;100\x07",
            Self::Error => "\x1b]9;4;2;100\x07",
        }
    }
}

pub fn terminal_progress_for_activity(kind: ActivityKind) -> TerminalProgress {
    match kind {
        ActivityKind::Ready => TerminalProgress::Hidden,
        ActivityKind::Queued | ActivityKind::Working | ActivityKind::RunningTool => {
            TerminalProgress::Indeterminate
        }
        ActivityKind::ActionRequired => TerminalProgress::Paused,
    }
}

pub fn is_exploration_tool(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "view_file"
            | "viewfile"
            | "read_file"
            | "readfile"
            | "list_directory"
            | "list_dir"
            | "listdir"
            | "glob"
            | "grep"
            | "grep_search"
            | "grepsearch"
            | "find_symbol"
            | "findsymbol"
            | "codebase_search"
            | "codebasesearch"
            | "codebase_symbol"
            | "codebasesymbol"
            | "get_project_map"
            | "getprojectmap"
    )
}

pub fn is_editing_tool(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "replace_file_content"
            | "replacefilecontent"
            | "multi_replace_file_content"
            | "multireplacefilecontent"
            | "write_to_file"
            | "writetofile"
            | "write_file"
            | "writefile"
            | "edit_file"
            | "editfile"
            | "create_file"
            | "createfile"
            | "patch_file"
            | "patchfile"
            | "delete_file"
            | "deletefile"
            | "move_file"
            | "movefile"
            | "copy_file"
            | "copyfile"
            | "generate_sound_effect"
            | "generate_music"
    )
}

fn compact_target(raw: &str) -> String {
    let compacted = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut target = compacted.chars().take(120).collect::<String>();
    if compacted.chars().count() > 120 {
        target.push('…');
    }
    target
}

/// Return the small semantic label shown for a live tool. This is shared by
/// the executor and TUI so the network layer records no terminal formatting.
pub fn summarize_tool_call(name: &str, args: &serde_json::Value) -> (String, String) {
    let value = |keys: &[&str], fallback: &str| -> String {
        keys.iter()
            .find_map(|key| args.get(*key).and_then(|value| value.as_str()))
            .unwrap_or(fallback)
            .to_string()
    };
    let name_lower = name.to_ascii_lowercase();
    let (action, target) = match name_lower.as_str() {
        "view_file" | "viewfile" | "read_file" | "readfile" => {
            ("Read", value(&["TargetFile", "target_file", "AbsolutePath", "absolute_path", "path", "file", "filePath", "filepath"], "?"))
        }
        "list_directory" | "list_dir" | "listdir" | "glob" => (
            "List",
            value(&["DirectoryPath", "directory_path", "SearchPath", "search_path", "path", "pattern"], "."),
        ),
        "grep" | "grep_search" | "grepsearch" => {
            let query = value(&["Query", "query", "pattern"], "?");
            let path = args
                .get("SearchPath")
                .or_else(|| args.get("search_path"))
                .or_else(|| args.get("path"))
                .and_then(|value| value.as_str())
                .filter(|path| !path.is_empty() && *path != ".");
            return (
                "Search".to_string(),
                compact_target(
                    &path
                        .map(|path| format!("{query} in {path}"))
                        .unwrap_or(query),
                ),
            );
        }
        "find_symbol" | "findsymbol" | "codebase_search" | "codebasesearch" | "codebase_symbol" | "codebasesymbol" | "search_web" | "searchweb" => {
            ("Search", value(&["query", "Query"], "?"))
        }
        "run_command" | "runcommand" | "execute_command" | "bash" => {
            ("Bash", value(&["CommandLine", "command_line", "command"], "?"))
        }
        "replace_file_content" | "replacefilecontent" | "multi_replace_file_content" | "multireplacefilecontent" | "edit_file" | "editfile" | "patch_file" | "patchfile" => {
            ("Edit", value(&["TargetFile", "target_file", "AbsolutePath", "absolute_path", "path", "file", "filePath", "filepath"], "?"))
        }
        "write_to_file" | "writetofile" | "write_file" | "writefile" | "create_file" | "createfile" => {
            ("Write", value(&["TargetFile", "target_file", "AbsolutePath", "absolute_path", "path", "file", "filePath", "filepath"], "?"))
        }
        "delete_file" | "deletefile" => (
            "Delete",
            value(&["TargetFile", "target_file", "AbsolutePath", "absolute_path", "path", "file", "filePath", "filepath"], "?"),
        ),
        "move_file" | "movefile" | "copy_file" | "copyfile" => {
            let src = value(&["src", "source", "from"], "?");
            let dest = value(&["dest", "destination", "to"], "?");
            return (
                to_pascal_action(name),
                compact_target(&format!("{src} → {dest}")),
            );
        }
        "get_project_map" | "getprojectmap" => ("Read", "project map".to_string()),
        _ => {
            let target = value(&["TargetFile", "target_file", "AbsolutePath", "absolute_path", "path", "file", "filePath", "filepath", "target", "query", "name", "command", "output_path"], "");
            return (to_pascal_action(name), compact_target(&target));
        }
    };
    (action.to_string(), compact_target(&target))
}

fn to_pascal_action(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Collapse multiple active calls into the one status snapshot shown below
/// the live assistant tail. Individual targets remain available for grouped
/// exploration and parallel command activity.
pub fn classify_live_tools(calls: &[LiveToolCall]) -> Option<ActivitySnapshot> {
    if calls.is_empty() {
        return None;
    }
    let all_exploration = calls
        .iter()
        .all(|call| is_exploration_tool(&call.tool_name));
    let detail = if all_exploration {
        let details = calls
            .iter()
            .filter(|call| !call.target.is_empty() && call.target != "?")
            .map(|call| format!("{} {}", call.action, call.target))
            .take(3)
            .collect::<Vec<_>>();
        (!details.is_empty()).then(|| details.join(", "))
    } else {
        let details = calls
            .iter()
            .take(2)
            .map(|call| {
                if call.target.is_empty() || call.target == "?" {
                    call.action.clone()
                } else {
                    format!("{} {}", call.action, call.target)
                }
            })
            .collect::<Vec<_>>();
        (!details.is_empty()).then(|| details.join(", "))
    };
    let label = if all_exploration {
        "Exploring".to_owned()
    } else if calls.iter().any(|call| call.tool_name == "run_command") {
        "Running".to_owned()
    } else if calls.len() == 1 {
        calls[0].action.clone()
    } else {
        "Calling".to_owned()
    };
    Some(ActivitySnapshot {
        kind: ActivityKind::RunningTool,
        label,
        detail,
        animated: true,
    })
}

pub fn classify_activity(status: &AppStatus, running_tools: &[String]) -> ActivitySnapshot {
    let action_required = matches!(
        status,
        AppStatus::AwaitingToolConfirmation
            | AppStatus::AwaitingQuestion
            | AppStatus::VerbosityPicker
            | AppStatus::ThinkingPicker
            | AppStatus::EffortPicker
            | AppStatus::ProtocolPicker
            | AppStatus::YoloPicker
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
        | AppStatus::EffortPicker
        | AppStatus::ProtocolPicker
        | AppStatus::YoloPicker => unreachable!("handled above"),
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
        ActivityKind, AnimationCell, LiveToolCall, animation_trail, classify_activity,
        classify_live_tools, format_terminal_title, sanitize_session_name, summarize_tool_call,
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

    #[test]
    fn live_tool_activity_preserves_action_and_target() {
        let (action, target) = summarize_tool_call(
            "run_command",
            &serde_json::json!({"command": "cargo test --lib"}),
        );
        let activity = classify_live_tools(&[LiveToolCall::new(
            "call-1", None, "run_command", action, target,
        )])
        .expect("live activity");

        assert_eq!(activity.kind, ActivityKind::RunningTool);
        assert_eq!(activity.label, "Running");
        assert_eq!(activity.detail.as_deref(), Some("Bash cargo test --lib"));
    }

    #[test]
    fn live_exploration_activity_groups_targets() {
        let calls = [
            LiveToolCall::new("read", None, "view_file", "Read", "src/main.rs"),
            LiveToolCall::new("search", None, "grep", "Search", "renderer in src"),
        ];
        let activity = classify_live_tools(&calls).expect("live activity");

        assert_eq!(activity.label, "Exploring");
        assert_eq!(
            activity.detail.as_deref(),
            Some("Read src/main.rs, Search renderer in src")
        );
    }

    #[test]
    fn terminal_progress_matches_activity_kind() {
        use super::{TerminalProgress, terminal_progress_for_activity};

        assert_eq!(
            terminal_progress_for_activity(ActivityKind::Ready),
            TerminalProgress::Hidden
        );
        assert_eq!(TerminalProgress::Hidden.osc_sequence(), "\x1b]9;4;0;0\x07");

        assert_eq!(
            terminal_progress_for_activity(ActivityKind::Queued),
            TerminalProgress::Indeterminate
        );
        assert_eq!(
            terminal_progress_for_activity(ActivityKind::Working),
            TerminalProgress::Indeterminate
        );
        assert_eq!(
            terminal_progress_for_activity(ActivityKind::RunningTool),
            TerminalProgress::Indeterminate
        );
        assert_eq!(
            TerminalProgress::Indeterminate.osc_sequence(),
            "\x1b]9;4;3;0\x07"
        );

        assert_eq!(
            terminal_progress_for_activity(ActivityKind::ActionRequired),
            TerminalProgress::Paused
        );
        assert_eq!(TerminalProgress::Paused.osc_sequence(), "\x1b]9;4;4;100\x07");
        assert_eq!(TerminalProgress::Error.osc_sequence(), "\x1b]9;4;2;100\x07");
    }
}
