use super::{AppStatus, LiveToolCall};
use rustcode_core::activity::{
    exploration_tool_parameters, is_exploration_tool, safe_parameter, sanitize_tool_parameter,
};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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
#[cfg(test)]
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
    #[cfg(test)]
    Error,
}

const TERMINAL_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl TerminalProgress {
    pub fn osc_sequence(&self) -> &'static str {
        match self {
            Self::Hidden => "\x1b]9;4;0;0\x07",
            Self::Indeterminate => "\x1b]9;4;3;0\x07",
            Self::Paused => "\x1b]9;4;4;100\x07",
            #[cfg(test)]
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

fn compact_target(raw: &str) -> String {
    sanitize_tool_parameter(raw, 120)
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
            rustcode_command::shell_label(),
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
        | "createfile" | "write_file_chunk" => (
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
    // Not-yet-started projections are queued, not running. Show Running or
    // Exploring only after actual execution starts (#1495).
    let all_speculative = calls.iter().all(|call| !call.execution_started);
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
    if all_speculative {
        return Some(ActivitySnapshot {
            kind: ActivityKind::Queued,
            label: "Queued".to_owned(),
            detail,
            animated: true,
        });
    }
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

#[cfg(test)]
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
    match kind {
        ActivityKind::Working | ActivityKind::RunningTool => format!(
            "{} · {session}",
            TERMINAL_SPINNER[frame as usize % TERMINAL_SPINNER.len()]
        ),
        ActivityKind::Ready | ActivityKind::Queued | ActivityKind::ActionRequired => session,
    }
}

/// Cadence of the animated spinner, shared by the tab title and the chat's
/// live activity row so both advance in step.
pub const SPINNER_FRAME_MS: u64 = 120;

/// Braille spinner frame for a point on the animation timeline.
///
/// Quantized to [`SPINNER_FRAME_MS`] so timer-only frames inside the same
/// bucket render identical rows instead of invalidating caches every tick.
pub fn spinner_frame(elapsed: Duration) -> char {
    let millis = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    TERMINAL_SPINNER[(millis / SPINNER_FRAME_MS) as usize % TERMINAL_SPINNER.len()]
}

/// Spinner frame index for a point on the animation timeline.
pub fn spinner_frame_index(elapsed: Duration) -> u64 {
    let millis = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    (millis / SPINNER_FRAME_MS) as u64
}

/// Baseline for the shared spinner timeline.
///
/// The tab title and the chat's running row are written by different callers on
/// different frames, so the timeline needs one process-wide origin for the two
/// to advance in step.
static SPINNER_EPOCH: OnceLock<Instant> = OnceLock::new();

/// Elapsed time on the shared spinner timeline.
///
/// Callers must use this instead of measuring from `Instant::now()`:
/// `Instant::now().elapsed()` is the gap between two adjacent clock reads,
/// which is always ~0 and therefore pins the animation to frame zero.
pub fn spinner_elapsed() -> Duration {
    SPINNER_EPOCH.get_or_init(Instant::now).elapsed()
}

/// Spinner frame index for the current point on the shared timeline.
pub fn current_spinner_frame_index() -> u64 {
    spinner_frame_index(spinner_elapsed())
}

#[cfg(test)]
mod tests {
    use super::{
        ActivityKind, AnimationCell, LiveToolCall, TERMINAL_SPINNER, animation_trail,
        classify_activity, classify_live_tools, current_spinner_frame_index, format_terminal_title,
        sanitize_session_name, spinner_elapsed, spinner_frame, spinner_frame_index,
        summarize_tool_call,
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
    fn summaries_redact_sensitive_parameters() {
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
    fn terminal_title_contains_spinner_and_short_name_while_working() {
        let title = format_terminal_title(ActivityKind::Working, "tower defense", 2);
        assert_eq!(title, "⠹ · tower defense");
    }

    #[test]
    fn active_terminal_titles_cycle_through_the_requested_spinner() {
        let expected = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        for (frame, spinner) in expected.into_iter().enumerate() {
            let title = format_terminal_title(ActivityKind::Working, "bench", frame as u64);
            assert_eq!(title, format!("{spinner} · bench"));
            assert_eq!(
                format_terminal_title(ActivityKind::RunningTool, "bench", frame as u64),
                format!("{spinner} · bench")
            );
        }
        assert_eq!(
            format_terminal_title(ActivityKind::Working, "bench", 10),
            format_terminal_title(ActivityKind::Working, "bench", 0)
        );
    }

    /// The title must actually animate. The previous caller measured with
    /// `Instant::now().elapsed()` — the gap between two adjacent clock reads,
    /// always ~0 — so the frame index was permanently 0 and the tab title
    /// stayed pinned on `⠋` for the whole turn.
    #[test]
    fn the_shared_timeline_advances_so_the_title_is_not_pinned() {
        let first = current_spinner_frame_index();
        // Elapsed time is monotonic, so two reads can never go backwards.
        let second = current_spinner_frame_index();
        assert!(second >= first);

        // Cross a frame boundary on the real clock and the title must change.
        let start = current_spinner_frame_index();
        let mut changed = false;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if current_spinner_frame_index() != start {
                changed = true;
                break;
            }
        }
        assert!(
            changed,
            "the shared timeline never advanced past frame {start}"
        );
    }

    #[test]
    fn the_shared_timeline_keeps_the_title_and_chat_in_step() {
        // Both surfaces read the same epoch, so one instant yields one frame.
        let elapsed = spinner_elapsed();
        assert_eq!(spinner_frame_index(elapsed), current_spinner_frame_index());
        assert_eq!(
            spinner_frame(elapsed),
            TERMINAL_SPINNER[current_spinner_frame_index() as usize % TERMINAL_SPINNER.len()]
        );
        assert!(
            spinner_elapsed() >= elapsed,
            "the shared timeline must be monotonic"
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
                "Build chess MCP"
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
    fn inactive_title_states_show_only_the_session_name() {
        assert_eq!(
            format_terminal_title(ActivityKind::Queued, "bench", 0),
            "bench"
        );
        assert_eq!(
            format_terminal_title(ActivityKind::ActionRequired, "bench", 0),
            "bench"
        );
        assert_eq!(
            format_terminal_title(ActivityKind::Ready, "bench", 0),
            "bench"
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
    fn speculative_live_tools_are_queued_until_execution_starts() {
        let mut queued = LiveToolCall::new("call-1", None, "run_command", "Bash", "cargo test");
        queued.execution_started = false;
        let activity = classify_live_tools(&[queued]).expect("live activity");

        assert_eq!(activity.kind, ActivityKind::Queued);
        assert_eq!(activity.label, "Queued");
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
