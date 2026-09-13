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

const TERMINAL_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

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
            | "render_video"
    )
}

fn compact_target(raw: &str) -> String {
    sanitize_tool_parameter(raw, 120)
}

/// Sanitize a short tool parameter before it reaches a live or committed
/// transcript. Tool summaries are intentionally bounded and never render raw
/// prompts, file contents, or structured arguments.
pub fn sanitize_tool_parameter(raw: &str, max_chars: usize) -> String {
    let compacted = raw
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut value = compacted.chars().take(max_chars).collect::<String>();
    if compacted.chars().count() > max_chars {
        value.push('…');
    }
    value
}

fn safe_parameter(key: &str, value: &str, max_chars: usize) -> String {
    let key = key.to_ascii_lowercase();
    let value_lower = value.to_ascii_lowercase();
    if key.contains("password")
        || key.contains("secret")
        || key.contains("credential")
        || key.contains("token")
        || key.contains("api_key")
        || key.contains("authorization")
        || key.contains("prompt")
        || key.contains("content")
        || value_lower.contains("bearer ")
        || value_lower.contains("sk-")
        || value_lower.contains("ghp_")
    {
        "[redacted]".to_owned()
    } else {
        sanitize_tool_parameter(value, max_chars)
    }
}

fn string_arg<'a, 'b>(
    args: &'a serde_json::Value,
    keys: &'b [&'b str],
) -> Option<(&'b str, &'a str)> {
    keys.iter().find_map(|key| {
        args.get(*key)
            .and_then(|value| value.as_str())
            .map(|value| (*key, value))
    })
}

fn path_with_home(path: &str, home_path: Option<&str>) -> String {
    let path = if let Some(home) = home_path {
        path.strip_prefix(home)
            .map(|suffix| format!("~{suffix}"))
            .unwrap_or_else(|| path.to_owned())
    } else {
        path.to_owned()
    };
    safe_parameter("path", &path, 100)
}

/// Render only the allowlisted exploration arguments shared by live and
/// committed tool summaries.
pub fn exploration_tool_parameters(
    name: &str,
    args: &serde_json::Value,
    home_path: Option<&str>,
) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let path = string_arg(
        args,
        &[
            "TargetFile",
            "target_file",
            "AbsolutePath",
            "absolute_path",
            "DirectoryPath",
            "directory_path",
            "SearchPath",
            "search_path",
            "path",
            "file",
            "filePath",
            "filepath",
        ],
    )
    .map(|(_, value)| path_with_home(value, home_path));
    match lower.as_str() {
        "view_file" | "viewfile" | "read_file" | "readfile" => {
            let path = path.unwrap_or_else(|| "?".to_owned());
            let start = ["start_line", "StartLine", "startLine"]
                .iter()
                .find_map(|key| args.get(*key).and_then(|value| value.as_u64()));
            let end = ["end_line", "EndLine", "endLine"]
                .iter()
                .find_map(|key| args.get(*key).and_then(|value| value.as_u64()));
            Some(match (start, end) {
                (Some(start), Some(end)) => format!("{path} (lines {start}-{end})"),
                (Some(start), None) => format!("{path} (line {start})"),
                _ => path,
            })
        }
        "list_directory" | "list_dir" | "listdir" | "glob" => {
            let pattern = string_arg(args, &["pattern", "glob"])
                .map(|(key, value)| safe_parameter(key, value, 80));
            Some(match (path, pattern) {
                (Some(path), Some(pattern)) if pattern != path => format!("{path} ({pattern})"),
                (Some(path), _) => path,
                (None, Some(pattern)) => pattern,
                _ => ".".to_owned(),
            })
        }
        "grep" | "grep_search" | "grepsearch" => {
            let (pattern_key, pattern) =
                string_arg(args, &["Query", "query", "pattern", "Pattern"])
                    .unwrap_or(("pattern", "?"));
            let pattern = safe_parameter(pattern_key, pattern, 80);
            let mut summary = match path {
                Some(path) if path != "." => format!("{pattern} in {path}"),
                _ => pattern,
            };
            if let Some((key, include)) = string_arg(args, &["include", "Include", "glob", "Glob"])
            {
                summary.push_str(&format!(" ({} {})", key, safe_parameter(key, include, 50)));
            }
            if args
                .get("ignore_case")
                .or_else(|| args.get("IgnoreCase"))
                .or_else(|| args.get("case_insensitive"))
                .and_then(|value| value.as_bool())
                == Some(true)
            {
                summary.push_str(" (case-insensitive)");
            }
            Some(sanitize_tool_parameter(&summary, 140))
        }
        "find_symbol" | "findsymbol" | "codebase_search" | "codebasesearch" | "codebase_symbol"
        | "codebasesymbol" => {
            let (key, query) =
                string_arg(args, &["query", "Query", "symbol"]).unwrap_or(("query", "?"));
            Some(safe_parameter(key, query, 100))
        }
        "get_project_map" | "getprojectmap" => Some("project map".to_owned()),
        _ => None,
    }
}

/// Return the small semantic label shown for a live tool. This is shared by
/// the executor and TUI so the network layer records no terminal formatting.
pub fn summarize_tool_call(name: &str, args: &serde_json::Value) -> (String, String) {
    if let Some(target) = exploration_tool_parameters(name, args, None) {
        let action = match name.to_ascii_lowercase().as_str() {
            "view_file" | "viewfile" | "read_file" | "readfile" => "Read",
            "list_directory" | "list_dir" | "listdir" | "glob" => "List",
            "find_symbol" | "findsymbol" | "codebase_search" | "codebasesearch"
            | "codebase_symbol" | "codebasesymbol" => "Search",
            "get_project_map" | "getprojectmap" => "Read",
            _ => "Search",
        };
        return (action.to_owned(), compact_target(&target));
    }
    let value = |keys: &[&str], fallback: &str| -> String {
        keys.iter()
            .find_map(|key| {
                args.get(*key)
                    .and_then(|value| value.as_str())
                    .map(|value| safe_parameter(key, value, 120))
            })
            .unwrap_or_else(|| fallback.to_owned())
    };
    let name_lower = name.to_ascii_lowercase();
    let (action, target) = match name_lower.as_str() {
        "search_web" | "searchweb" => ("Search", "query".to_owned()),
        "run_command" | "runcommand" | "execute_command" | "bash" => (
            "Bash",
            value(&["CommandLine", "command_line", "command"], "?"),
        ),
        "replace_file_content"
        | "replacefilecontent"
        | "multi_replace_file_content"
        | "multireplacefilecontent"
        | "edit_file"
        | "editfile"
        | "patch_file"
        | "patchfile" => (
            "Edit",
            value(
                &[
                    "TargetFile",
                    "target_file",
                    "AbsolutePath",
                    "absolute_path",
                    "path",
                    "file",
                    "filePath",
                    "filepath",
                ],
                "?",
            ),
        ),
        "write_to_file" | "writetofile" | "write_file" | "writefile" | "create_file"
        | "createfile" => (
            "Write",
            value(
                &[
                    "TargetFile",
                    "target_file",
                    "AbsolutePath",
                    "absolute_path",
                    "path",
                    "file",
                    "filePath",
                    "filepath",
                ],
                "?",
            ),
        ),
        "delete_file" | "deletefile" => (
            "Delete",
            value(
                &[
                    "TargetFile",
                    "target_file",
                    "AbsolutePath",
                    "absolute_path",
                    "path",
                    "file",
                    "filePath",
                    "filepath",
                ],
                "?",
            ),
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
            let target = value(
                &[
                    "TargetFile",
                    "target_file",
                    "AbsolutePath",
                    "absolute_path",
                    "path",
                    "file",
                    "filePath",
                    "filepath",
                    "target",
                    "query",
                    "name",
                    "command",
                    "output_path",
                    "project_path",
                ],
                "",
            );
            let action =
                crate::tools::mcp_tool_display_name(name).unwrap_or_else(|| to_pascal_action(name));
            return (action, compact_target(&target));
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
    } else {
        "Running".to_owned()
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
    let normalized = rustcode_session::unwrap_title_paste_markers(raw)
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
        ActivityKind::Working => format!(
            "{} Working",
            TERMINAL_SPINNER[frame as usize % TERMINAL_SPINNER.len()]
        ),
        ActivityKind::RunningTool => format!(
            "{} Running",
            TERMINAL_SPINNER[frame as usize % TERMINAL_SPINNER.len()]
        ),
        ActivityKind::ActionRequired => "[!] Action Required".to_string(),
    };
    format!("{prefix} · {session}")
}

#[cfg(test)]
mod tests {
    use super::{
        ActivityKind, AnimationCell, LiveToolCall, animation_trail, classify_activity,
        classify_live_tools, exploration_tool_parameters, format_terminal_title,
        sanitize_session_name, summarize_tool_call,
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
    fn exploration_summaries_are_bounded_and_include_safe_navigation_parameters() {
        let view = exploration_tool_parameters(
            "view_file",
            &serde_json::json!({"path": "/workspace/src/lib.rs", "start_line": 10, "end_line": 20}),
            Some("/workspace"),
        )
        .unwrap();
        assert_eq!(view, "~/src/lib.rs (lines 10-20)");

        let grep = summarize_tool_call(
            "grep",
            &serde_json::json!({
                "pattern": "renderer",
                "SearchPath": "src",
                "include": "*.rs",
                "ignore_case": true,
                "password": "do-not-display"
            }),
        );
        assert_eq!(grep.0, "Search");
        assert_eq!(grep.1, "renderer in src (include *.rs) (case-insensitive)");
        assert!(!grep.1.contains("do-not-display"));
    }

    #[test]
    fn terminal_title_contains_state_and_short_name() {
        let title = format_terminal_title(ActivityKind::Working, "tower defense", 2);
        assert_eq!(title, "⠹ Working · tower defense");
    }

    #[test]
    fn active_terminal_titles_cycle_through_the_requested_spinner() {
        let expected = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        for (frame, spinner) in expected.into_iter().enumerate() {
            let title = format_terminal_title(ActivityKind::Working, "bench", frame as u64);
            assert!(title.starts_with(&format!("{spinner} Working")));
            assert_eq!(
                format_terminal_title(ActivityKind::RunningTool, "bench", frame as u64),
                format!("{spinner} Running · bench")
            );
        }
        assert_eq!(
            format_terminal_title(ActivityKind::Working, "bench", 10),
            format_terminal_title(ActivityKind::Working, "bench", 0)
        );
    }

    #[test]
    fn terminal_titles_hide_complete_and_truncated_paste_framing() {
        for raw in [
            "<!--PASTE:15:Build chess MCP-->",
            "<!--PASTE:1937:Build chess MCP",
        ] {
            assert_eq!(
                format_terminal_title(ActivityKind::Ready, raw, 0),
                "rustcode · Idle · Build chess MCP"
            );
        }
        assert_eq!(
            sanitize_session_name("<!--PASTE:5:棋棋棋棋棋-->", 3),
            "棋棋棋"
        );
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
            "call-1",
            None,
            "run_command",
            action,
            target,
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
    fn live_custom_tool_activity_uses_running_label() {
        let activity = classify_live_tools(&[LiveToolCall::new(
            "mcp-call",
            None,
            "SearchEmails",
            "SearchEmails",
            "query=\"*\"",
        )])
        .expect("live activity");

        assert_eq!(activity.label, "Running");
        assert_eq!(activity.detail.as_deref(), Some("SearchEmails query=\"*\""));
    }

    #[test]
    fn mcp_tool_activity_includes_server_name() {
        let (action, target) = summarize_tool_call(
            "mcp__mail_mcp__SearchEmails",
            &serde_json::json!({"query": "*"}),
        );
        assert_eq!(action, "mail_mcp.SearchEmails");
        assert_eq!(target, "*");
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
        assert_eq!(
            TerminalProgress::Paused.osc_sequence(),
            "\x1b]9;4;4;100\x07"
        );
        assert_eq!(TerminalProgress::Error.osc_sequence(), "\x1b]9;4;2;100\x07");
    }
}
