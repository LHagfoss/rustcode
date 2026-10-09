use super::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// snake_case / kebab-case → PascalCase, e.g. `use_skill` → `UseSkill`. Used so
/// custom and MCP tools render like the built-ins (no underscores, capitalized)
/// instead of leaking their raw internal names.
pub(super) fn to_pascal_case(name: &str) -> String {
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

pub(super) fn contract_home_path(path: &str, home_path: Option<&str>) -> String {
    if path.is_empty() {
        return String::new();
    }
    // An empty `HOME` would otherwise turn every path into `~/…`.
    if let Some(home) = home_path.filter(|home| !home.is_empty()) {
        if path.starts_with(&home) {
            return format!("~{}", &path[home.len()..]);
        }
    }
    path.to_string()
}

pub(super) fn format_pi_tool_action(
    name: &str,
    args: &serde_json::Value,
    home_path: Option<&str>,
) -> (String, String) {
    let name_lower = name.to_ascii_lowercase();
    let action_label = match name_lower.as_str() {
        "view_file" | "viewfile" | "read_file" | "readfile" => "Read".to_string(),
        "replace_file_content"
        | "replacefilecontent"
        | "multi_replace_file_content"
        | "multireplacefilecontent"
        | "edit_file"
        | "editfile"
        | "patch_file"
        | "patchfile" => "Edit".to_string(),
        "write_to_file" | "writetofile" | "write_file" | "writefile" | "create_file"
        | "createfile" | "write_file_chunk" => "Write".to_string(),
        "delete_file" | "deletefile" => "Delete".to_string(),
        "move_file" | "movefile" => "Move".to_string(),
        "copy_file" | "copyfile" => "Copy".to_string(),
        "list_directory" | "list_dir" | "listdir" | "glob" => "ListDir".to_string(),
        "grep" | "grep_search" | "grepsearch" => "Search".to_string(),
        "find_symbol" | "findsymbol" | "codebase_symbol" | "codebasesymbol" => "Symbol".to_string(),
        "run_command" | "runcommand" | "execute_command" | "bash" => {
            rustcode::controller::shell_label().to_string()
        }
        "search_web" | "searchweb" | "codebase_search" | "codebasesearch" => "Search".to_string(),
        "get_project_map" | "getprojectmap" => "ProjectMap".to_string(),
        "manage_task" | "managetask" => "ManageTask".to_string(),
        "background_task" | "backgroundtask" => "Task".to_string(),
        "ask_question" | "askquestion" => "Asked".to_string(),
        "remember" => "Remember".to_string(),
        "recall_memory" | "recallmemory" => "Recall".to_string(),
        "forget_memory" | "forgetmemory" => "Forget".to_string(),
        _ => rustcode::controller::mcp_tool_display_name(name)
            .unwrap_or_else(|| to_pascal_case(name)),
    };

    if let Some(target) =
        rustcode_core::activity::exploration_tool_parameters(name, args, home_path)
    {
        return (action_label, target);
    }

    let target_arg = match name_lower.as_str() {
        "view_file"
        | "viewfile"
        | "read_file"
        | "readfile"
        | "replace_file_content"
        | "replacefilecontent"
        | "multi_replace_file_content"
        | "multireplacefilecontent"
        | "write_to_file"
        | "write_file_chunk"
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
        | "deletefile" => {
            let path = args
                .get("TargetFile")
                .or_else(|| args.get("target_file"))
                .or_else(|| args.get("AbsolutePath"))
                .or_else(|| args.get("absolute_path"))
                .or_else(|| args.get("path"))
                .or_else(|| args.get("file"))
                .or_else(|| args.get("filePath"))
                .or_else(|| args.get("filepath"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            contract_home_path(path, home_path)
        }
        "move_file" | "movefile" | "copy_file" | "copyfile" => {
            let src = args
                .get("src")
                .or_else(|| args.get("source"))
                .or_else(|| args.get("from"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let dest = args
                .get("dest")
                .or_else(|| args.get("destination"))
                .or_else(|| args.get("to"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("{} -> {}", src, dest)
        }
        "list_directory" | "list_dir" | "glob" => {
            let path = args
                .get("DirectoryPath")
                .or_else(|| args.get("SearchPath"))
                .or_else(|| args.get("path"))
                .or_else(|| args.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            contract_home_path(path, home_path)
        }
        "grep" | "grep_search" => {
            let query = args
                .get("Query")
                .or_else(|| args.get("query"))
                .or_else(|| args.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("Grep {query}")
        }
        "run_command" => args
            .get("CommandLine")
            .or_else(|| args.get("command"))
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        "search_web" | "codebase_search" | "find_symbol" | "codebase_symbol" | "recall_memory" => {
            args.get("query")
                .or_else(|| args.get("Query"))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string()
        }
        "remember" | "forget_memory" => args
            .get("key")
            .and_then(|v| v.as_str())
            .map(|v| rustcode_core::activity::sanitize_tool_parameter(v, 100))
            .unwrap_or_else(|| "?".to_owned()),
        "use_skill" => args
            .get("name")
            .or_else(|| args.get("skill"))
            .or_else(|| args.get("skill_name"))
            .and_then(|v| v.as_str())
            .map(|v| rustcode_core::activity::sanitize_tool_parameter(v, 100))
            .unwrap_or_default(),
        "spawn_agent" => "agent task".to_owned(),
        "send_agent" => "agent message".to_owned(),
        "wait_agent" | "cancel_agent" => args
            .get("id")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            })
            .unwrap_or_default(),
        "set_goal" => "goal".to_owned(),
        "ask_question" | "askquestion" => ask_question_text(args),
        "manage_task" => {
            let action = args
                .get("Action")
                .or_else(|| args.get("action"))
                .and_then(|v| v.as_str())
                .unwrap_or("status");
            let task_id = args
                .get("TaskId")
                .or_else(|| args.get("task_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let clean_id = task_id.rsplit_once('/').map(|(_, r)| r).unwrap_or(task_id);
            if !clean_id.is_empty() {
                format!("{action} {clean_id}")
            } else {
                action.to_string()
            }
        }
        "background_task" => {
            let task_id = args
                .get("TaskId")
                .or_else(|| args.get("task_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let clean_id = task_id.rsplit_once('/').map(|(_, r)| r).unwrap_or(task_id);
            clean_id.to_string()
        }
        _ => format_generic_tool_args(args),
    };

    (action_label, target_arg)
}

pub(super) fn format_generic_tool_args(args: &serde_json::Value) -> String {
    let Some(obj) = args.as_object() else {
        return String::new();
    };
    if obj.is_empty() {
        return String::new();
    }

    let mut parts = Vec::new();
    for (k, v) in obj {
        let key_lower = k.to_ascii_lowercase();
        if k == "CodeContent"
            || k == "ReplacementContent"
            || k == "content"
            || k == "system_prompt"
            || k == "Code"
            || k == "toolSummary"
            || k == "toolAction"
            || key_lower.contains("prompt")
            || key_lower.contains("password")
            || key_lower.contains("secret")
            || key_lower.contains("token")
            || key_lower.contains("credential")
            || key_lower.contains("authorization")
        {
            continue;
        }
        let val_str = match v {
            serde_json::Value::String(s) => {
                let first_line = rustcode_core::activity::sanitize_tool_parameter(
                    s.lines().next().unwrap_or("").trim(),
                    30,
                );
                if first_line.chars().count() > 30 {
                    format!("\"{}...\"", first_line.chars().take(27).collect::<String>())
                } else {
                    format!("\"{}\"", first_line)
                }
            }
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Array(a) => format!("[{} items]", a.len()),
            serde_json::Value::Object(_) => "{...}".to_string(),
            serde_json::Value::Null => "null".to_string(),
        };
        parts.push(format!("{k}={val_str}"));
    }

    if parts.is_empty() {
        if let Some(target) = obj
            .get("TargetFile")
            .or_else(|| obj.get("path"))
            .and_then(|v| v.as_str())
        {
            return target.to_string();
        }
    }

    parts.join(", ")
}

pub(super) fn resolve_tool_result_name(
    preceding_call_name: Option<&str>,
    persisted_name: Option<&str>,
    content: &str,
) -> Option<String> {
    preceding_call_name
        .or(persisted_name)
        .or_else(|| content.split_once(": ").map(|(name, _)| name))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// Memoized conversation render. Building every message's spans and wrapping
/// them several times per frame is O(history) and dominates scroll latency on
/// long sessions. The rendered lines only change when the history, viewport
/// width, expanded-thoughts, modal state, or copy-badge state changes — so we
/// cache them and reuse across the many frames where nothing but the scroll
/// offset moved.
#[allow(dead_code)]
pub(super) struct ChatCache {
    key: ChatKey,
    lines: Vec<Line<'static>>,
    copy_wrapped_rows: Vec<(u16, String)>,
    msg_wrapped_rows: Vec<u16>,
    total_wrapped_lines: u16,
}

#[allow(dead_code)]
#[derive(PartialEq, Clone)]
pub(super) struct ChatKey {
    hist_len: usize,
    total_len: usize,
    last_len: usize,
    history_display_start: usize,
    width: u16,
    show_picker: bool,
    copied_recently: Option<(String, bool)>,
    theme: String,
}

thread_local! {
    static CHAT_CACHE: std::cell::RefCell<Option<ChatCache>> =
        const { std::cell::RefCell::new(None) };
}

/// Deep-copy a borrowed `Line` into an owned `'static` one so it can outlive the
/// `state.history` borrow it was built from and sit in the frame cache.
pub(super) fn own_line(line: &Line) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .iter()
        .map(|s| Span::styled(s.content.clone().into_owned(), s.style))
        .collect();
    let mut owned = Line::from(spans);
    owned.style = line.style;
    owned.alignment = line.alignment;
    owned
}

/// Maximum number of rendered tool results kept in [`TOOL_RESULT_CACHE`].
pub(super) const TOOL_RESULT_CACHE_CAP: usize = 256;

thread_local! {
    /// Rendered tool results keyed by content hash. Bounded with LRU eviction
    /// so overflowing the cap drops one cold entry instead of flushing every
    /// still-visible result and forcing a full re-highlight on the next frame.
    pub(super) static TOOL_RESULT_CACHE: RefCell<lru::LruCache<u64, Vec<Line<'static>>>> =
        RefCell::new(lru::LruCache::new(TOOL_RESULT_CACHE_CAP));
}

pub(super) fn tool_result_cache_key(
    tool_name: &str,
    result: &str,
    width: usize,
    verbosity: &rustcode::controller::Verbosity,
    show_picker: bool,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool_name.hash(&mut hasher);
    result.hash(&mut hasher);
    width.hash(&mut hasher);
    verbosity.hash(&mut hasher);
    show_picker.hash(&mut hasher);
    theme::active_palette().name.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn cached_tool_result(
    tool_name: &str,
    result: &str,
    width: usize,
    verbosity: &rustcode::controller::Verbosity,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let key = tool_result_cache_key(tool_name, result, width, verbosity, show_picker);

    TOOL_RESULT_CACHE.with(|cache| {
        cached_tool_result_in(cache, key, || {
            render_tool_result(tool_name, result, width, verbosity, show_picker)
                .iter()
                .map(own_line)
                .collect()
        })
    })
}

fn cached_file_edit_diff(
    diff: &str,
    path: &str,
    width: usize,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (
        "inline-file-diff",
        diff,
        path,
        width,
        show_picker,
        theme::active_palette().name,
    )
        .hash(&mut hasher);
    let key = hasher.finish();
    TOOL_RESULT_CACHE.with(|cache| {
        cached_tool_result_in(cache, key, || {
            render_file_edit_diff_for_path(diff, path, width, show_picker)
        })
    })
}

pub(super) fn cached_tool_result_in(
    cache: &RefCell<lru::LruCache<u64, Vec<Line<'static>>>>,
    key: u64,
    render: impl FnOnce() -> Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    // A hit refreshes recency, so results currently on screen are never the
    // eviction victim.
    if let Some(lines) = cache.borrow_mut().get(&key) {
        return lines.clone();
    }
    let lines = render();
    cache.borrow_mut().insert(key, lines.clone());
    lines
}

pub(super) fn tool_result_is_hidden(tool_name: &str) -> bool {
    // `ask_question` used to be hidden here, but its tool result carries the
    // user's answer ("User selected: …"). Hiding it left no trace of the
    // question or the choice in the transcript, so it stays visible.
    matches!(tool_name, "set_goal" | "todo_write" | "complete_task")
}

/// Extract the human question from an `ask_question` call's arguments, across
/// both the flat (`question`) and nested (`questions[0].question`) shapes the
/// harness accepts.
pub(super) fn ask_question_text(args: &serde_json::Value) -> String {
    let nested = args
        .get("questions")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first());
    let text = nested
        .and_then(|item| {
            item.get("question")
                .or_else(|| item.get("prompt"))
                .or_else(|| item.get("message"))
        })
        .and_then(|value| value.as_str())
        .or_else(|| {
            args.get("question")
                .or_else(|| args.get("prompt"))
                .or_else(|| args.get("message"))
                .and_then(|value| value.as_str())
        })
        .unwrap_or("");
    let clean = rustcode_core::activity::sanitize_tool_parameter(text, 110);
    if clean.is_empty() {
        "a question".to_owned()
    } else {
        clean
    }
}

/// Extract the user's answer from a committed `ask_question` tool message.
/// The executor records a single answer as `User selected: <answer>` and a
/// chain as `User answers:` plus one `[header] question → answer` line each;
/// a cancellation arrives as a plain notice. Surface a compact summary so the
/// transcript child line reads as a prompt/response pair.
pub(super) fn ask_question_answer(history: &[ChatMessage], message_index: usize) -> String {
    let raw = history
        .get(message_index)
        .map(|message| message.content.as_str())
        .unwrap_or("");
    let result = raw.split_once(": ").map(|(_, rest)| rest).unwrap_or(raw);
    let summary = if let Some(body) = result.strip_prefix("User answers:") {
        body.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("; ")
    } else {
        result
            .strip_prefix("User selected: ")
            .unwrap_or(result)
            .to_owned()
    };
    let summary = summary.trim();
    // The legacy cancel text travelled the success channel before typed
    // cancellations existed; never render it as if the user chose it.
    let summary = if summary == "User cancelled prompt." {
        "cancelled"
    } else {
        summary
    };
    let clean = rustcode_core::activity::sanitize_tool_parameter(summary, 90);
    if clean.is_empty() {
        "no answer".to_owned()
    } else {
        clean
    }
}

pub(super) fn tool_result_action(
    state: &RenderSnapshot,
    message_index: usize,
    tool_name: &str,
) -> (String, String) {
    // Render the Q&A pair on one child line ("Asked <question> → <answer>")
    // so the user's choice is visible without expanding the entry. Chains
    // summarize as a question count plus every header/answer pair.
    if tool_name == "ask_question" {
        let args = tool_call_arguments(state, message_index, tool_name);
        let answer =
            replace_emoji_shortcodes(&ask_question_answer(state.active_history(), message_index));
        let chain_len = args
            .get("questions")
            .and_then(|value| value.as_array())
            .map(|items| items.len())
            .unwrap_or(0);
        if chain_len > 1 {
            return (
                "Asked".to_owned(),
                format!("{chain_len} questions → {answer}"),
            );
        }
        let question = replace_emoji_shortcodes(&ask_question_text(&args));
        return ("Asked".to_owned(), format!("{question} → {answer}"));
    }
    format_pi_tool_action(
        tool_name,
        &tool_call_arguments(state, message_index, tool_name),
        state.home_path(),
    )
}

pub(super) fn tool_result_status(
    message: &ChatMessage,
    tool_name: &str,
    result: &str,
) -> (bool, String) {
    if let Some(record) = &message.tool_result {
        // A background launch receipt is pending, not failed: the real
        // outcome arrives later as a `background_task` completion. Rendering
        // it as failed (issue #1221) misleads the user into thinking the
        // command itself failed.
        if record.pending {
            // The command left the turn and runs as a task; `/tasks` and the
            // footer follow it from here.
            return (true, "background".to_owned());
        }
        if record
            .error_kind
            .as_deref()
            .is_some_and(|kind| kind == "Cancelled")
        {
            return (false, "cancelled".to_owned());
        }
        return match record.exit_code {
            Some(code) => (record.success, format!("exit {code}")),
            None if record.success => (true, "completed".to_owned()),
            None => (false, "failed".to_owned()),
        };
    }

    if tool_name == "run_command" {
        if let Some(code) = result.lines().find_map(|line| {
            line.strip_prefix("exit code: ")
                .and_then(|code| code.trim().parse::<i32>().ok())
        }) {
            return (code == 0, format!("exit {code}"));
        }
    }

    let failed = result
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| {
            let line = line.trim_start().to_ascii_lowercase();
            line.starts_with("error") || line.starts_with('✗')
        });
    if failed {
        (false, "failed".to_owned())
    } else {
        (true, "completed".to_owned())
    }
}

/// Hang a body row under the group's side spine, stripping the payload's own
/// baked gutter (`  │ ` stdout, `  ! ` stderr) so wrapped rows keep one
/// continuous line instead of a dangling stub (#1725). Returns the prefixed
/// spans plus the matching wrap continuation.
fn spine_body_spans(line: Line<'static>, show_picker: bool) -> (Vec<Span<'static>>, Span<'static>) {
    let mut spans_iter = line.spans.into_iter();
    let mut first = spans_iter.next().expect("non-empty line");
    let stderr = if let Some(rest) = first.content.strip_prefix("  ! ") {
        first.content = rest.to_owned().into();
        true
    } else {
        if let Some(rest) = first.content.strip_prefix("  │ ") {
            first.content = rest.to_owned().into();
        } else if first.content == "  │" {
            // Blank-run gutter marker: the spine prefix below replaces it.
            first.content = "".into();
        }
        false
    };
    let mut spans = Vec::with_capacity(spans_iter.len() + 2);
    spans.push(tool_body_spine(show_picker));
    if stderr {
        spans.push(Span::styled(
            "! ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
    }
    spans.push(first);
    spans.extend(spans_iter);
    let continuation = if stderr {
        Span::styled(
            "  │   ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, show_picker),
        )
    } else {
        tool_body_spine(show_picker)
    };
    (spans, continuation)
}

pub(super) fn indent_tool_result_body(
    lines: Vec<Line<'static>>,
    tool_name: &str,
    verbosity: &rustcode::controller::Verbosity,
    width: u16,
    expanded: bool,
) -> Vec<Line<'static>> {
    if matches!(verbosity, rustcode::controller::Verbosity::High) && !expanded {
        return Vec::new();
    }

    let filtered = lines
        .into_iter()
        .filter(|line| {
            tool_name != "run_command"
                || !line
                    .spans
                    .iter()
                    .any(|span| span.content.trim_start().starts_with('✗'))
        })
        .collect::<Vec<_>>();
    // Expanded bodies render in full; collapsed ones are capped after width-
    // aware wrapping so the limit counts terminal rows (#1602).
    let visible = filtered;
    let max_w = (width as usize).max(10);
    let mut indented = Vec::new();
    for line in visible {
        if line.spans.is_empty() {
            // A blank payload line still belongs to the body, so it keeps the
            // spine instead of breaking the vertical line (#1725).
            indented.push(Line::from(tool_body_spine(false)));
            continue;
        }
        // Command payloads arrive with their own baked gutter (`  │ ` stdout,
        // `  ! ` stderr). Strip it and re-hang the row under the group's side
        // spine so wrapped rows keep the line instead of dropping to spaces.
        let (spans, continuation) = spine_body_spans(line, false);
        push_wrapped_with_continuation(&mut indented, spans, max_w, Some(continuation));
    }
    if expanded {
        indented
    } else {
        cap_collapsed_tool_body(indented, false)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolTranscriptKind {
    Explored,
    Command,
    Edit,
    Tool,
}

pub(crate) fn tool_transcript_kind(tool_name: &str) -> ToolTranscriptKind {
    if rustcode_core::activity::is_exploration_tool(tool_name) {
        ToolTranscriptKind::Explored
    } else if tool_name == "run_command" || tool_name.eq_ignore_ascii_case("bash") {
        ToolTranscriptKind::Command
    } else if rustcode_core::activity::is_editing_tool(tool_name) {
        ToolTranscriptKind::Edit
    } else {
        ToolTranscriptKind::Tool
    }
}

pub(crate) fn tool_result_group_kind(
    state: &RenderSnapshot,
    message_indices: &[usize],
    width: u16,
) -> Option<ToolTranscriptKind> {
    let mut kinds = message_indices
        .iter()
        .filter_map(|&index| tool_transcript_entry(state, index, width, false))
        .map(|entry| entry.kind);
    let first = kinds.next()?;
    Some(if kinds.all(|kind| kind == first) {
        first
    } else {
        ToolTranscriptKind::Tool
    })
}

pub(super) fn format_exploration_action(
    name: &str,
    args: &serde_json::Value,
    home_path: Option<&str>,
) -> (String, String) {
    match name {
        "view_file" => {
            let path = args
                .get("TargetFile")
                .or_else(|| args.get("AbsolutePath"))
                .or_else(|| args.get("path"))
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            ("Read".to_string(), contract_home_path(path, home_path))
        }
        "list_directory" | "list_dir" | "glob" => {
            let path = args
                .get("DirectoryPath")
                .or_else(|| args.get("SearchPath"))
                .or_else(|| args.get("path"))
                .or_else(|| args.get("pattern"))
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            ("List".to_string(), contract_home_path(path, home_path))
        }
        "grep" | "grep_search" => {
            let query = args
                .get("Query")
                .or_else(|| args.get("query"))
                .or_else(|| args.get("pattern"))
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let path = args
                .get("SearchPath")
                .or_else(|| args.get("path"))
                .and_then(|value| value.as_str())
                .filter(|path| !path.is_empty() && *path != ".");
            let target = path
                .map(|path| format!("{query} in {}", contract_home_path(path, home_path)))
                .unwrap_or_else(|| query.to_string());
            ("Search".to_string(), target)
        }
        "find_symbol" | "codebase_search" | "codebase_symbol" => {
            let query = args
                .get("query")
                .or_else(|| args.get("Query"))
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            ("Search".to_string(), query.to_string())
        }
        "get_project_map" => ("Read".to_string(), "project map".to_string()),
        _ => format_pi_tool_action(name, args, home_path),
    }
}

pub(super) struct ToolTranscriptEntry {
    pub(super) message_index: usize,
    pub(super) tool_name: String,
    pub(super) action: String,
    pub(super) target: String,
    pub(super) success: bool,
    pub(super) status: String,
    pub(super) body: Vec<Line<'static>>,
    pub(super) kind: ToolTranscriptKind,
    pub(super) diff_counts: Option<(usize, usize)>,
    /// Task completions folded into this row by [`fold_task_completions`],
    /// and how many of those did not succeed.
    pub(super) earlier: usize,
    pub(super) earlier_failed: usize,
    /// The target stands in for a command history no longer holds, so it is
    /// drawn as a note instead of as the command.
    pub(super) target_is_note: bool,
}

/// What a command or task row says when its command cannot be recovered.
const UNKNOWN_COMMAND_NOTE: &str = "(command not recorded)";

fn target_is_missing(target: &str) -> bool {
    target.is_empty() || target == "?"
}

/// The command a background launch receipt quotes
/// (`… Command: <cmd>. Completion notification: …`).
fn launched_command(result: &str) -> Option<&str> {
    let rest = result.split_once(" Command: ")?.1;
    let command = rest
        .rsplit_once(". Completion notification: ")
        .map_or(rest, |(command, _)| command);
    (!command.trim().is_empty()).then_some(command)
}

/// Whether a command result is the receipt for a task that was started, not
/// the output of a command that ran.
fn is_launch_receipt(result: &str) -> bool {
    (result.starts_with("Task started in background.")
        || result.starts_with("Detached task started."))
        && launched_task_id(result).is_some()
}

/// The task id a completion names (`Task <id> completed.`).
fn completed_task_id(result: &str) -> Option<&str> {
    let id = result.strip_prefix("Task ")?.split_once(" completed.")?.0;
    (!id.is_empty() && !id.contains(char::is_whitespace)).then_some(id)
}

/// Tool name of the result a finished background task adds to history.
const TASK_COMPLETION_TOOL: &str = "background_task";

/// The task id a background launch receipt names (`… Task ID: <id>. Status: …`).
fn launched_task_id(result: &str) -> Option<&str> {
    let rest = result.split_once("Task ID: ")?.1;
    let id = rest
        .split(char::is_whitespace)
        .next()?
        .trim_end_matches('.');
    (!id.is_empty()).then_some(id)
}

fn message_tool_name(message: &ChatMessage) -> Option<String> {
    resolve_tool_result_name(
        None,
        message
            .tool_result
            .as_ref()
            .map(|result| result.tool_name.as_str()),
        &message.content,
    )
}

fn message_tool_payload(message: &ChatMessage) -> &str {
    message
        .content
        .split_once(": ")
        .map(|(_, result)| result)
        .unwrap_or(&message.content)
}

/// Outcome named by a `manage_task` `wait` result for `task_id`. A result the
/// model read through `wait` never becomes a `background_task` message.
fn waited_task_outcome(result: &str, task_id: &str) -> Option<(bool, String)> {
    let rest = result.strip_prefix(&format!("Task '{task_id}' "))?;
    if rest.starts_with("exited successfully") {
        Some((true, "completed".to_owned()))
    } else if rest.starts_with("cancelled") {
        Some((false, "cancelled".to_owned()))
    } else if let Some(code) = rest.strip_prefix("failed (exit code ") {
        let code = code.split(')').next().unwrap_or_default();
        Some(match code.parse::<i32>() {
            Ok(code) => (false, format!("exit {code}")),
            Err(_) => (false, "failed".to_owned()),
        })
    } else if rest.starts_with("terminated by")
        || rest.starts_with("spawn failed")
        || rest.starts_with("failed")
    {
        Some((false, "failed".to_owned()))
    } else {
        None
    }
}

/// How the task started by the launch receipt at `message_index` ended, read
/// from the later result for the same task id. `None` while history holds no
/// such result, which is what a still-running task looks like.
///
/// Deriving this per render keeps history append-only: the receipt the model
/// saw is never rewritten.
fn background_launch_outcome(
    history: &[ChatMessage],
    message_index: usize,
    launch_result: &str,
) -> Option<(bool, String)> {
    let task_id = launched_task_id(launch_result)?;
    let completed = format!("Task {task_id} completed.");
    history[message_index + 1..]
        .iter()
        .filter(|message| message.role == "tool")
        .find_map(|message| {
            let payload = message_tool_payload(message);
            match message_tool_name(message)?.as_str() {
                TASK_COMPLETION_TOOL if payload.starts_with(&completed) => {
                    Some(tool_result_status(message, TASK_COMPLETION_TOOL, payload))
                }
                "manage_task" => waited_task_outcome(payload, task_id),
                _ => None,
            }
        })
}

/// Command and output of a task completion (`Task <id> completed. Command:
/// <cmd>. Output:\n<output>`).
fn task_completion_parts(result: &str) -> (Option<&str>, &str) {
    let Some((head, output)) = result.split_once(" Output:\n") else {
        return (None, result);
    };
    let command = head
        .split_once(" Command: ")
        .map(|(_, command)| command.strip_suffix('.').unwrap_or(command));
    (command, output)
}

/// Draw each run of consecutive task completions as its latest one.
///
/// Tasks that finish during a turn join history together at its end, so ten
/// tasks used to add ten rows. The kept entry counts what it stands for; the
/// expand key and a click act on it alone, so they open the latest output.
pub(super) fn fold_task_completions(entries: Vec<ToolTranscriptEntry>) -> Vec<ToolTranscriptEntry> {
    let mut folded: Vec<ToolTranscriptEntry> = Vec::with_capacity(entries.len());
    for mut entry in entries {
        if entry.tool_name == TASK_COMPLETION_TOOL
            && let Some(previous) = folded
                .last()
                .filter(|previous| previous.tool_name == TASK_COMPLETION_TOOL)
        {
            entry.earlier = previous.earlier + 1;
            entry.earlier_failed = previous.earlier_failed + usize::from(!previous.success);
            folded.pop();
        }
        folded.push(entry);
    }
    folded
}

/// ` · +2 earlier` behind a folded task completion row.
fn earlier_completions_suffix(entry: &ToolTranscriptEntry) -> Option<String> {
    (entry.earlier > 0).then(|| {
        if entry.earlier_failed > 0 {
            format!(
                " · +{} earlier ({} failed)",
                entry.earlier, entry.earlier_failed
            )
        } else {
            format!(" · +{} earlier", entry.earlier)
        }
    })
}

pub(super) fn tool_call_arguments(
    state: &RenderSnapshot,
    message_index: usize,
    tool_name: &str,
) -> serde_json::Value {
    let history = state.active_history();
    let message = &history[message_index];
    if let Some(call_id) = message.tool_call_id.as_deref() {
        return history[..message_index]
            .iter()
            .rev()
            .filter(|message| message.role == "assistant")
            .flat_map(|message| message.tool_calls.iter().rev())
            .find(|call| call.id == call_id)
            .and_then(|call| serde_json::from_str(&call.arguments).ok())
            .unwrap_or(serde_json::Value::Null);
    }

    let candidates = state.tool_call_candidate_indices();
    let before_result = candidates.partition_point(|index| *index < message_index);

    // A call the scheduler held runs in a later round, and its result joins
    // that round without a call id. Counting results after the nearest
    // assistant message then points past that message's calls, at nothing or
    // at an unrelated earlier call. The hash the record keeps of its
    // arguments names the call itself. Held calls are released when the user
    // speaks, so the call is always within the current turn.
    if let Some(hash) = message
        .tool_result
        .as_ref()
        .map(|record| record.arguments_hash.as_str())
        .filter(|hash| !hash.is_empty())
    {
        let turn_start = history[..message_index]
            .iter()
            .rposition(|message| message.role == "user")
            .unwrap_or(0);
        for &assistant_index in candidates[..before_result]
            .iter()
            .rev()
            .take_while(|index| **index >= turn_start)
        {
            let calls = rustcode_tool_protocol::resolve_tool_calls(
                &history[assistant_index],
                state.active_tool_protocol(),
            );
            if let Some(call) = calls.into_iter().find(|call| {
                call.name == tool_name
                    && rustcode::controller::tool_arguments_hash(&call.arguments) == hash
            }) {
                return call.arguments;
            }
        }
    }

    for &assistant_index in candidates[..before_result].iter().rev() {
        let assistant = &history[assistant_index];
        let calls =
            rustcode_tool_protocol::resolve_tool_calls(assistant, state.active_tool_protocol());
        if !calls.iter().any(|call| call.name == tool_name) {
            continue;
        }
        let prior_same_name_results = history[assistant_index + 1..message_index]
            .iter()
            .filter(|message| {
                message.role == "tool"
                    && resolve_tool_result_name(
                        None,
                        message
                            .tool_result
                            .as_ref()
                            .map(|result| result.tool_name.as_str()),
                        &message.content,
                    )
                    .as_deref()
                        == Some(tool_name)
            })
            .count();
        if let Some(call) = calls
            .into_iter()
            .filter(|call| call.name == tool_name)
            .nth(prior_same_name_results)
        {
            return call.arguments;
        }
    }

    serde_json::Value::Null
}

pub(super) fn tool_transcript_entry(
    state: &RenderSnapshot,
    message_index: usize,
    width: u16,
    show_picker: bool,
) -> Option<ToolTranscriptEntry> {
    let message = state.active_history().get(message_index)?;
    if message.role != "tool" {
        return None;
    }
    if message
        .tool_result
        .as_ref()
        .and_then(|record| record.error_kind.as_deref())
        .is_some_and(|kind| kind == "Deferred")
        || message
            .content
            .contains("error: intentionally deferred by the scheduler")
    {
        return None;
    }
    let tool_name = resolve_tool_result_name(
        None,
        message
            .tool_result
            .as_ref()
            .map(|result| result.tool_name.as_str()),
        &message.content,
    )
    .unwrap_or_else(|| "Tool".to_owned());
    if tool_result_is_hidden(&tool_name) {
        return None;
    }

    let result = message
        .content
        .split_once(": ")
        .map(|(_, result)| result)
        .unwrap_or(&message.content);
    let kind = tool_transcript_kind(&tool_name);
    let is_task_completion = tool_name == TASK_COMPLETION_TOOL;
    let (task_command, task_output) = if is_task_completion {
        task_completion_parts(result)
    } else {
        (None, result)
    };
    let (action, mut target) = if kind == ToolTranscriptKind::Explored {
        let args = tool_call_arguments(state, message_index, &tool_name);
        format_exploration_action(&tool_name, &args, state.home_path())
    } else {
        tool_result_action(state, message_index, &tool_name)
    };
    let recorded_command = message
        .tool_result
        .as_ref()
        .and_then(|record| record.command.as_deref())
        .filter(|command| !command.trim().is_empty());
    let mut target_is_note = false;
    if is_task_completion && target_is_missing(&target) {
        // No call precedes a completion, so name the task by its command, or
        // by its id when the result kept no command. The row fits it.
        target = recorded_command
            .or(task_command.filter(|command| !command.trim().is_empty()))
            .or_else(|| completed_task_id(result))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                target_is_note = true;
                UNKNOWN_COMMAND_NOTE.to_owned()
            });
    } else if kind == ToolTranscriptKind::Command && target_is_missing(&target) {
        // The call could not be found (history trimmed, or a session written
        // before results were matched by hash): the result still records what
        // ran, and a launch receipt quotes it.
        match recorded_command.or_else(|| launched_command(result)) {
            Some(command) => target = command.to_owned(),
            None => {
                target_is_note = true;
                target = launched_task_id(result)
                    .map(|id| format!("(task {id})"))
                    .unwrap_or_else(|| UNKNOWN_COMMAND_NOTE.to_owned());
            }
        }
    }
    let (mut success, mut status) = tool_result_status(message, &tool_name, result);
    if status == "background"
        && let Some(outcome) =
            background_launch_outcome(state.active_history(), message_index, result)
    {
        (success, status) = outcome;
    }
    let edit_diff = if kind == ToolTranscriptKind::Edit && success && !edit_result_is_noop(result) {
        message
            .diff
            .as_deref()
            .filter(|diff| !diff.is_empty() && !diff.contains('\0'))
            .or_else(|| embedded_edit_diff(result))
    } else {
        None
    };
    let diff_counts = edit_diff.and_then(edit_diff_counts);
    // Only command output and file diffs expose tool payloads. Human answers
    // remain available because they belong to the conversation.
    let mut body = if let Some(diff) = edit_diff {
        cached_file_edit_diff(
            diff,
            &target,
            // Reserve the 2-column side spine, not the old 4-space gutter.
            usize::from(width).saturating_sub(2),
            show_picker,
        )
    } else if is_launch_receipt(result) {
        // The receipt repeats the command and tells the model how to wait for
        // the task. The row already says both; the output comes with the
        // task's own row.
        Vec::new()
    } else if kind == ToolTranscriptKind::Command
        || tool_name == "ask_question"
        || is_task_completion
    {
        // The entry always carries its output; whether it is shown is the
        // group renderer's decision (verbosity default, or opened).
        cached_tool_result(
            if is_task_completion {
                "run_command"
            } else {
                &tool_name
            },
            task_output,
            width as usize,
            &rustcode::controller::Verbosity::Low,
            show_picker,
        )
    } else {
        Vec::new()
    };
    if kind == ToolTranscriptKind::Edit && success && edit_result_is_noop(result) {
        status = "no changes".to_owned();
    }
    // Write/edit calls whose result carries no embedded diff (e.g.
    // `write_to_file` reports only `wrote 'path' (N lines, M bytes)`) still
    // need their changed lines at low verbosity (#1567). Synthesize an
    // added-lines preview from the call arguments; no-op and failed changes
    // keep their truthful single-line status.
    if kind == ToolTranscriptKind::Edit
        && success
        && !edit_result_is_noop(result)
        && !edit_diff_unavailable(result)
        && edit_diff.is_none()
        && !result_has_embedded_diff(result)
    {
        let args = tool_call_arguments(state, message_index, &tool_name);
        let preview = synthesized_edit_preview(
            &tool_name,
            &args,
            result,
            success,
            width as usize,
            show_picker,
        );
        if !preview.is_empty() {
            // Keep the synthesized code at the head of the capped body so the
            // five-row preview shows its beginning before the omitted marker.
            // The title row already carries the edited path.
            let status = std::mem::take(&mut body);
            body = preview;
            body.extend(status);
        }
    }

    Some(ToolTranscriptEntry {
        message_index,
        tool_name,
        action,
        target,
        success,
        status,
        body,
        kind,
        diff_counts,
        earlier: 0,
        earlier_failed: 0,
        target_is_note,
    })
}

/// The heading of a tool block. It never changes: calls in flight, finished
/// and failed all sit under the same heading, and each row carries its own
/// state (#1850).
pub(super) fn tool_group_header(title: &str, show_picker: bool) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "• ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            title.to_owned(),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
    ])
}

/// The heading every tool block carries.
pub(super) const TOOL_BLOCK_HEADING: &str = "Ran";

/// Whether an assistant step between two tool rounds shows nothing: it only
/// carries the calls of the next round. Such a step sits inside the tool
/// block around it. A step that says or thinks something is a boundary: its
/// text is rendered as text and the next round opens a block of its own.
pub(crate) fn tool_step_is_silent(state: &RenderSnapshot, index: usize) -> bool {
    let Some(message) = state.active_history().get(index) else {
        return false;
    };
    message.role == "assistant"
        && !message.conversation_recap
        && (!message.tool_calls.is_empty()
            || !rustcode_tool_protocol::resolve_tool_calls(message, state.active_tool_protocol())
                .is_empty())
        // The renderer is the authority on what shows. Whether a block is
        // empty does not depend on the width, and an empty message skips it.
        && (message.content.trim().is_empty()
            || super::render_committed_history_block_snapshot(state, index, 80).is_empty())
}

/// Expand affordance appended to a collapsed body row. Reserved out of the
/// wrap width so it always lands on the entry's own first row (#1541).
/// Ctrl+O toggles every collapsible entry while Ctrl+Shift+O steps a single
/// entry, so the hint names both (#1601).
pub(super) const EXPAND_HINT: &str = " (ctrl+o all · shift+o one)";
const COMPACT_EXPAND_HINT: &str = " (ctrl+o all)";
const SHORT_EXPAND_HINT: &str = " (o)";

/// Maximum terminal rows in a collapsed tool-result preview, including the
/// omission marker (#1602).
pub(super) const COLLAPSED_TOOL_BODY_MAX_LINES: usize = 5;
const COLLAPSED_FILE_DIFF_PREVIEW_LINES: usize = 5;

/// Row window shared by committed and live tool previews. Callers supply their
/// own marker text so live output can retain its omitted-byte note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ToolPreviewWindow {
    pub(super) head_rows: usize,
    pub(super) tail_rows: usize,
    pub(super) omitted_rows: usize,
}

pub(super) fn tool_preview_window(
    row_count: usize,
    force_marker: bool,
) -> Option<ToolPreviewWindow> {
    if row_count <= COLLAPSED_TOOL_BODY_MAX_LINES && !force_marker {
        return None;
    }
    if row_count.saturating_add(1) <= COLLAPSED_TOOL_BODY_MAX_LINES {
        return Some(ToolPreviewWindow {
            head_rows: row_count,
            tail_rows: 0,
            omitted_rows: 0,
        });
    }

    let head_rows = (COLLAPSED_TOOL_BODY_MAX_LINES - 1) / 2;
    let tail_rows = COLLAPSED_TOOL_BODY_MAX_LINES - head_rows - 1;
    Some(ToolPreviewWindow {
        head_rows,
        tail_rows,
        omitted_rows: row_count.saturating_sub(head_rows + tail_rows),
    })
}

/// Keep the beginning and end of an already wrapped tool body within the
/// collapsed visual-row budget. The marker occupies one of the five rows.
fn cap_collapsed_tool_body(mut lines: Vec<Line<'static>>, show_picker: bool) -> Vec<Line<'static>> {
    let Some(window) = tool_preview_window(lines.len(), false) else {
        return lines;
    };
    let mut preview = Vec::with_capacity(COLLAPSED_TOOL_BODY_MAX_LINES);
    let tail_start = lines.len().saturating_sub(window.tail_rows);
    let tail = lines.split_off(tail_start);
    lines.truncate(window.head_rows);
    preview.extend(lines);
    preview.push(Line::from(vec![
        tool_body_spine(show_picker),
        Span::styled(
            format!("… +{} lines", window.omitted_rows),
            get_themed_style(
                COLOR_MUTED(),
                COLOR_BG(),
                Modifier::ITALIC | Modifier::DIM,
                show_picker,
            ),
        ),
    ]));
    preview.extend(tail);
    preview
}

fn expand_hint_span(width: u16, show_picker: bool) -> Span<'static> {
    let hint = if width >= 29 {
        EXPAND_HINT
    } else if width >= 19 {
        COMPACT_EXPAND_HINT
    } else {
        SHORT_EXPAND_HINT
    };
    Span::styled(
        hint,
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
    )
}

/// Every sibling owns a status marker; wrapped body rows use hanging spaces.
/// The tree connector (`├`/`└`) restores the inward side lines that point at
/// each child, while the marker keeps the execution state (#1725). Both are
/// 4 columns wide combined, matching the previous flat indent.
/// Models write chat-style shortcodes into option labels, and a terminal shows
/// them as `:white_check_mark:`. The few that mark a choice become the glyph
/// they stand for; anything else is left as written.
pub(super) fn replace_emoji_shortcodes(text: &str) -> String {
    const SHORTCODES: &[(&str, &str)] = &[
        (":white_check_mark:", "✓"),
        (":heavy_check_mark:", "✓"),
        (":check:", "✓"),
        (":x:", "✗"),
        (":no_entry:", "✗"),
        (":warning:", "!"),
    ];
    if !text.contains(':') {
        return text.to_owned();
    }
    let mut text = text.to_owned();
    for (code, glyph) in SHORTCODES {
        text = text.replace(code, glyph);
    }
    text
}

/// A call that has not finished, whether it is running or waiting its turn.
pub(super) const LIVE_ROW_GLYPH: char = '○';

fn tool_status_glyph(entry: &ToolTranscriptEntry) -> char {
    match entry.status.as_str() {
        "running" => LIVE_ROW_GLYPH,
        _ if entry.success => '✓',
        // Failed, stopped and cancelled all read the same: it did not finish.
        _ => '✗',
    }
}

pub(super) fn tool_tree_prefix(is_last: bool, show_picker: bool) -> Span<'static> {
    Span::styled(
        {
            // Rows are indented under their heading; a tree would have to be
            // redrawn whenever a block gained a row.
            let _ = is_last;
            "  "
        },
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    )
}

fn tool_title_continuation(is_last: bool, show_picker: bool) -> Span<'static> {
    Span::styled(
        {
            let _ = is_last;
            "    "
        },
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    )
}

/// Continuous side spine for tool body rows. Bodies used to indent with plain
/// spaces (and command payloads carried their own gutter), so a wrapped body
/// showed a dangling `│` stub on its first row and nothing below it (#1725).
/// Every body row — first, wrapped continuation, omission marker — hangs under
/// the same spine instead.
pub(super) fn tool_body_spine(show_picker: bool) -> Span<'static> {
    Span::styled(
        "  │ ",
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, show_picker),
    )
}

fn tool_status_marker(entry: &ToolTranscriptEntry) -> String {
    format!("{} ", tool_status_glyph(entry))
}

/// Which end of a target survives when it is too long for its row.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TargetKeep {
    /// A command or a query is recognised by how it starts.
    Head,
    /// A path is recognised by how it ends.
    Tail,
}

/// One piece of the state behind a row, and whether the row may drop it when
/// the terminal is too narrow to hold everything.
pub(super) struct RowState {
    pub(super) span: Span<'static>,
    pub(super) required: bool,
}

impl RowState {
    pub(super) fn required(span: Span<'static>) -> Self {
        Self {
            span,
            required: true,
        }
    }

    pub(super) fn optional(span: Span<'static>) -> Self {
        Self {
            span,
            required: false,
        }
    }
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

/// Shorten styled spans to `max_width` columns, marking the cut with `…`.
pub(super) fn clip_spans(
    spans: Vec<Span<'static>>,
    max_width: usize,
    keep: TargetKeep,
) -> Vec<Span<'static>> {
    if spans_width(&spans) <= max_width {
        return spans;
    }
    if max_width == 0 {
        return Vec::new();
    }
    let mut remaining = max_width - 1;
    let mut kept: Vec<Span<'static>> = Vec::new();
    let mut cut_style = spans.first().map(|span| span.style).unwrap_or_default();
    match keep {
        TargetKeep::Head => {
            'spans: for span in spans {
                cut_style = span.style;
                let mut text = String::new();
                for grapheme in span.content.graphemes(true) {
                    let grapheme_width = grapheme.width();
                    if grapheme_width > remaining {
                        if !text.is_empty() {
                            kept.push(Span::styled(text, span.style));
                        }
                        break 'spans;
                    }
                    remaining -= grapheme_width;
                    text.push_str(grapheme);
                }
                if !text.is_empty() {
                    kept.push(Span::styled(text, span.style));
                }
            }
            // The cut never follows a space: `cargo …` reads as a word lost.
            while let Some(last) = kept.last_mut() {
                let trimmed = last.content.trim_end().to_owned();
                if trimmed.is_empty() {
                    kept.pop();
                } else {
                    last.content = trimmed.into();
                    break;
                }
            }
            kept.push(Span::styled("…", cut_style));
        }
        TargetKeep::Tail => {
            'spans: for span in spans.into_iter().rev() {
                cut_style = span.style;
                let mut graphemes: Vec<&str> = Vec::new();
                let mut complete = true;
                for grapheme in span.content.graphemes(true).rev() {
                    let grapheme_width = grapheme.width();
                    if grapheme_width > remaining {
                        complete = false;
                        break;
                    }
                    remaining -= grapheme_width;
                    graphemes.push(grapheme);
                }
                if !graphemes.is_empty() {
                    let text = graphemes.into_iter().rev().collect::<String>();
                    kept.push(Span::styled(text, span.style));
                }
                if !complete {
                    break 'spans;
                }
            }
            kept.push(Span::styled("…", cut_style));
            kept.reverse();
        }
    }
    kept
}

/// The fewest columns a shortened target may take; below this it says nothing
/// and the row drops it.
const MIN_TARGET_WIDTH: usize = 4;

/// The fewest columns a shortened label may take.
const MIN_LABEL_WIDTH: usize = 3;

/// Lay a row out on exactly one terminal line: `lead label target state`.
///
/// The state is what the reader scans the block for, so its width is taken
/// out of the row first and the target gets what is left. On a terminal too
/// narrow for that the row gives up, in order, its optional state (from the
/// end), its target, and then the end of its label. The required state is only
/// cut, at the right edge, when no stub of the label would fit beside it.
///
/// The row used to be wrapped after the target had been cut to a guessed
/// width, which sent the state, or half of it, to a line of its own (#1828).
pub(super) fn fit_tool_row(
    lead: Vec<Span<'static>>,
    label: Vec<Span<'static>>,
    target: Vec<Span<'static>>,
    keep: TargetKeep,
    mut state: Vec<RowState>,
    width: usize,
) -> Line<'static> {
    let lead_width = spans_width(&lead);
    let label_width = spans_width(&label);
    let state_width =
        |state: &[RowState]| -> usize { state.iter().map(|part| part.span.content.width()).sum() };
    while lead_width + label_width + state_width(&state) > width {
        let Some(droppable) = state.iter().rposition(|part| !part.required) else {
            break;
        };
        state.remove(droppable);
    }
    let fixed = lead_width + state_width(&state);
    let mut spans = lead;
    if fixed + MIN_LABEL_WIDTH > width {
        // Not even a stub of the label fits beside the state: a row that
        // names nothing is worse than a state cut short at the edge.
        spans.extend(label);
    } else if fixed + label_width > width {
        spans.extend(clip_spans(label, width - fixed, TargetKeep::Head));
    } else {
        spans.extend(label);
        let room = width - fixed - label_width;
        if !target.is_empty() && room > MIN_TARGET_WIDTH {
            let gap_style = target[0].style;
            spans.push(Span::styled(" ", gap_style));
            spans.extend(clip_spans(target, room - 1, keep));
        }
    }
    spans.extend(state.into_iter().map(|part| part.span));
    Line::from(clip_spans(spans, width, TargetKeep::Head))
}

/// Whether a row's target is a path, which keeps its end when shortened.
fn target_keep(entry: &ToolTranscriptEntry) -> TargetKeep {
    if entry.kind == ToolTranscriptKind::Edit
        || matches!(
            entry.action.as_str(),
            "Read" | "Edit" | "Write" | "Delete" | "List" | "ListDir"
        )
    {
        TargetKeep::Tail
    } else {
        TargetKeep::Head
    }
}

/// The state a finished row carries behind it, most important first.
fn entry_row_state(
    entry: &ToolTranscriptEntry,
    show_hint: bool,
    width: u16,
    show_picker: bool,
) -> Vec<RowState> {
    let muted = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker);
    let mut state = Vec::new();
    if let Some((added, removed)) = entry.diff_counts {
        state.push(RowState::optional(Span::styled(
            format!(" (+{added} -{removed})"),
            muted,
        )));
    }
    if !entry.success || entry.status == "background" || entry.status == "no changes" {
        state.push(RowState::required(Span::styled(
            format!(" · {}", entry.status),
            muted,
        )));
    }
    if let Some(earlier) = earlier_completions_suffix(entry) {
        state.push(RowState::optional(Span::styled(
            earlier,
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, show_picker),
        )));
    }
    if show_hint {
        state.push(RowState::optional(expand_hint_span(width, show_picker)));
    }
    state
}

fn entry_row_lead(
    entry: &ToolTranscriptEntry,
    is_last: bool,
    show_picker: bool,
) -> Vec<Span<'static>> {
    vec![
        tool_tree_prefix(is_last, show_picker),
        Span::styled(
            tool_status_marker(entry),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ),
    ]
}

fn note_style(show_picker: bool) -> Style {
    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker)
}

pub(super) fn tool_child_line(
    entry: &ToolTranscriptEntry,
    is_last: bool,
    show_hint: bool,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let lead = entry_row_lead(entry, is_last, show_picker);
    // A dotted `server.tool` name alone reads as a broken row, most of all
    // when the call took no arguments. `MCP` leads the row the way `Bash` and
    // `Read` lead theirs, and the tool name joins the target (#1770).
    let is_mcp = entry.kind == ToolTranscriptKind::Tool && entry.action.contains('.');
    let label = vec![Span::styled(
        if is_mcp {
            "Mcp".to_owned()
        } else {
            entry.action.clone()
        },
        get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
    )];
    let target_style = if entry.target_is_note {
        note_style(show_picker)
    } else {
        get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker)
    };
    let state = entry_row_state(entry, show_hint, width, show_picker);

    // A question and its answer are part of the conversation, so they are
    // shown whole. They wrap in the width the state leaves, and the state
    // joins the first line.
    if entry.tool_name == "ask_question" && !target_is_missing(&entry.target) {
        let state_width: usize = state.iter().map(|part| part.span.content.width()).sum();
        let mut spans = lead;
        spans.extend(label);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(entry.target.clone(), target_style));
        let mut lines = Vec::new();
        push_wrapped_with_continuation(
            &mut lines,
            spans,
            usize::from(width).saturating_sub(state_width).max(10),
            Some(tool_title_continuation(is_last, show_picker)),
        );
        if let Some(first) = lines.first_mut() {
            first.spans.extend(state.into_iter().map(|part| part.span));
        }
        return lines;
    }

    let mut target = if target_is_missing(&entry.target) {
        Vec::new()
    } else {
        // One row is one line: a target that spans lines is joined.
        vec![Span::styled(
            entry
                .target
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            target_style,
        )]
    };
    if is_mcp {
        let name = if target.is_empty() {
            entry.action.clone()
        } else {
            format!("{} ", entry.action)
        };
        target.insert(
            0,
            Span::styled(
                name,
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        );
    }
    vec![fit_tool_row(
        lead,
        label,
        target,
        target_keep(entry),
        state,
        usize::from(width),
    )]
}

/// Collapse a shell command to a single-line preview: newlines become spaces
/// and the result is width-truncated with an ellipsis. Width is display
/// columns, not bytes, so CJK/wide glyphs don't overflow the transcript.
pub(super) fn collapse_command_preview(target: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let single = target.split_whitespace().collect::<Vec<_>>().join(" ");
    if single.width() <= max_width {
        return single;
    }
    let suffix = '…';
    let budget = max_width.saturating_sub(1);
    let mut output = String::new();
    let mut used = 0;
    for grapheme in single.graphemes(true) {
        let w = grapheme.width();
        if used + w > budget {
            break;
        }
        used += w;
        output.push_str(grapheme);
    }
    output.push(suffix);
    output
}

pub(super) fn command_child_lines(
    entry: &ToolTranscriptEntry,
    is_last: bool,
    show_hint: bool,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let lead = entry_row_lead(entry, is_last, show_picker);
    let label = vec![Span::styled(
        entry.action.clone(),
        get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
    )];
    let target = if target_is_missing(&entry.target) {
        Vec::new()
    } else if entry.target_is_note {
        vec![Span::styled(entry.target.clone(), note_style(show_picker))]
    } else {
        // Join a multi-line or chained command into one line before
        // highlighting, and never highlight more of it than a row can hold.
        let preview = collapse_command_preview(&entry.target, usize::from(width));
        highlight_shell_command(&preview, COLOR_BG(), show_picker)
            .into_iter()
            .flat_map(|line| line.spans)
            .collect()
    };
    let mut state = Vec::new();
    if !entry.success || entry.status == "background" {
        state.push(RowState::required(Span::styled(
            format!(" · {}", entry.status),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        )));
    }
    if show_hint {
        state.push(RowState::optional(expand_hint_span(width, show_picker)));
    }
    vec![fit_tool_row(
        lead,
        label,
        target,
        TargetKeep::Head,
        state,
        usize::from(width),
    )]
}

pub(super) fn indent_generic_tool_body(
    lines: Vec<Line<'static>>,
    verbosity: &rustcode::controller::Verbosity,
    width: u16,
    show_picker: bool,
    expanded: bool,
) -> Vec<Line<'static>> {
    if matches!(verbosity, rustcode::controller::Verbosity::High) && !expanded {
        return Vec::new();
    }
    // Expanded Tool bodies render in full; collapsed ones are capped after
    // width-aware wrapping so they stay within five terminal rows (#1602).
    if expanded {
        return indent_full_tool_body(lines, width, show_picker);
    }

    let max_w = (width as usize).max(10);
    let mut indented = Vec::new();
    for line in lines {
        if line.spans.is_empty() {
            // A blank payload line still belongs to the body, so it keeps the
            // spine instead of breaking the vertical line (#1725).
            indented.push(Line::from(tool_body_spine(show_picker)));
            continue;
        }
        let (spans, continuation) = spine_body_spans(line, show_picker);
        push_wrapped_with_continuation(&mut indented, spans, max_w, Some(continuation));
    }
    cap_collapsed_tool_body(indented, show_picker)
}

/// Indent a body without truncating it: the expanded form of a tool preview.
///
/// The collapsed preview reuses [`indent_generic_tool_body`] (shared
/// [`COLLAPSED_TOOL_BODY_MAX_LINES`] head/tail window with an omitted count).
/// Once expanded, the full body renders inline so Ctrl+O visibly changes the
/// chosen body (#1567, #1580).
pub(super) fn indent_full_tool_body(
    lines: Vec<Line<'static>>,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let max_w = (width as usize).max(10);
    let mut indented = Vec::new();
    for line in lines {
        if line.spans.is_empty() {
            // A blank payload line still belongs to the body, so it keeps the
            // spine instead of breaking the vertical line (#1725).
            indented.push(Line::from(tool_body_spine(show_picker)));
            continue;
        }
        let (spans, continuation) = spine_body_spans(line, show_picker);
        push_wrapped_with_continuation(&mut indented, spans, max_w, Some(continuation));
    }
    indented
}

/// Keep the first five wrapped diff rows visible while leaving the complete
/// diff available through Ctrl+O.
fn indent_file_edit_preview_body(
    lines: Vec<Line<'static>>,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let mut indented = indent_full_tool_body(lines, width, show_picker)
        .into_iter()
        .filter(|line| {
            !line
                .to_string()
                .trim_start()
                .trim_start_matches('│')
                .trim()
                .is_empty()
        })
        .collect::<Vec<_>>();
    if indented.len() <= COLLAPSED_FILE_DIFF_PREVIEW_LINES {
        return indented;
    }
    let omitted = indented.len() - COLLAPSED_FILE_DIFF_PREVIEW_LINES;
    indented.truncate(COLLAPSED_FILE_DIFF_PREVIEW_LINES);
    indented.push(Line::from(vec![
        tool_body_spine(show_picker),
        Span::styled(
            format!("… +{omitted} lines"),
            get_themed_style(
                COLOR_MUTED(),
                COLOR_BG(),
                Modifier::ITALIC | Modifier::DIM,
                show_picker,
            ),
        ),
    ]));
    indented
}

/// Whether an edit entry carries an expandable diff body.
///
/// Successful edits with changed lines (embedded or synthesized diffs, #1567)
/// expand; no-op (`already applied`) and failed changes keep their truthful
/// single-line status with no hint.
pub(super) fn edit_entry_is_expandable(entry: &ToolTranscriptEntry) -> bool {
    entry.kind == ToolTranscriptKind::Edit && entry.success && !entry.body.is_empty()
}

/// Keep a fitting title's hint inline; otherwise put it after the visible
/// output without shrinking the command or consuming its five-row preview.
fn append_tool_preview(
    lines: &mut Vec<Line<'static>>,
    mut title: Vec<Line<'static>>,
    body: Vec<Line<'static>>,
    entry: &ToolTranscriptEntry,
    show_hint: bool,
    width: u16,
    show_picker: bool,
) {
    let hint = expand_hint_span(width, show_picker);
    let raw_command_fits = entry.kind != ToolTranscriptKind::Command
        || (!entry.target.contains(['\n', '\r'])
            && entry.target.width() + entry.status.width() + 14 + hint.content.width()
                <= width as usize);
    let inline = show_hint
        && raw_command_fits
        && title.len() == 1
        && title[0].width() + hint.content.width() <= width as usize;
    if inline {
        title[0].spans.push(hint.clone());
    }
    lines.extend(title);
    lines.extend(body);
    if show_hint && !inline {
        let mut hint_lines = Vec::new();
        let spine = tool_body_spine(show_picker);
        // The spine already sets the hint off; its own leading space would
        // indent it past the output above it.
        let hint = Span::styled(hint.content.trim_start().to_owned(), hint.style);
        push_wrapped_with_continuation(
            &mut hint_lines,
            vec![spine.clone(), hint],
            (width as usize).max(1),
            Some(spine),
        );
        lines.extend(hint_lines);
    }
}

pub(crate) fn render_committed_tool_result_group_snapshot(
    state: &RenderSnapshot,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    render_tool_result_group_snapshot(state, message_indices, width, show_picker, true)
}

/// Render a later tool-only round as children of an already committed group.
/// Terminal scrollback cannot revise the first round's heading, so the
/// transcript cursor uses this form when one-tool rounds arrive incrementally.
pub(crate) fn render_committed_tool_result_continuation_snapshot(
    state: &RenderSnapshot,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    render_tool_result_group_snapshot(state, message_indices, width, show_picker, false)
}

/// Whether the tool result at `message_index` opens a provider round that
/// should be set apart from the round before it in the same chain.
///
/// A model that emits no prose between rounds produces one unbroken column of
/// rows. A spine-only row between rounds restores the batches as visual
/// groups; runs of single-call rounds stay compact.
fn tool_round_needs_spacer(history: &[ChatMessage], message_index: usize) -> bool {
    let is_tool = |index: usize| {
        history
            .get(index)
            .is_some_and(|message| message.role == "tool")
    };
    if message_index == 0 || is_tool(message_index - 1) {
        return false;
    }
    let round_len = (message_index..)
        .take_while(|&index| is_tool(index))
        .count();
    let mut previous_end = message_index - 1;
    while !is_tool(previous_end) {
        if previous_end == 0 || history[previous_end].role == "user" {
            return false;
        }
        previous_end -= 1;
    }
    let previous_len = (0..=previous_end)
        .rev()
        .take_while(|&index| is_tool(index))
        .count();
    round_len > 1 || previous_len > 1
}

fn tool_round_spacer(show_picker: bool) -> Line<'static> {
    Line::from(Span::styled(
        "│",
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, show_picker),
    ))
}

fn render_tool_result_group_snapshot(
    state: &RenderSnapshot,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
    include_header: bool,
) -> Vec<Line<'static>> {
    render_tool_result_group_detailed(state, message_indices, width, show_picker, include_header)
}

fn render_tool_result_group_detailed(
    state: &RenderSnapshot,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
    include_header: bool,
) -> Vec<Line<'static>> {
    let entries = fold_task_completions(
        message_indices
            .iter()
            .filter_map(|&index| tool_transcript_entry(state, index, width, show_picker))
            .collect(),
    );
    let mut lines = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        // `message_indices` represents one provider batch (and may include
        // tool-only assistant turns joined by the scrollback layer). Keep it
        // as one visual group even when the provider mixed command, read, or
        // edit tools. The child rows retain their kind-specific formatting.
        let group_end = entries.len();
        let group = &entries[index..group_end];
        if include_header && !lines.is_empty() {
            lines.push(Line::from(""));
        }
        {
            if include_header {
                // What each call was is on its own row, and so is how it
                // ended.
                lines.push(tool_group_header(TOOL_BLOCK_HEADING, show_picker));
            }
            let mut seen = std::collections::HashSet::new();
            // Pre-filter duplicate exploration rows so the last *rendered*
            // child gets the closing `└` connector (#1725).
            let visible: Vec<&ToolTranscriptEntry> = group
                .iter()
                .filter(|entry| {
                    entry.kind != ToolTranscriptKind::Explored
                        || seen.insert(format!("{}\0{}", entry.action, entry.target))
                })
                .collect();
            for (child_index, entry) in visible.iter().enumerate() {
                let continues_block = child_index > 0 || !include_header;
                if continues_block
                    && tool_round_needs_spacer(state.active_history(), entry.message_index)
                {
                    lines.push(tool_round_spacer(show_picker));
                }
                {
                    let is_expanded = state.expanded_thoughts().contains(&entry.message_index);
                    // Command, generic Tool, and Edit-with-diff entries collapse
                    // their bodies with the same expand affordance; Explored
                    // rows keep their kind-specific formatting. Edit previews
                    // differ: the collapsed form already shows a compact diff
                    // window (#1567), while Tool/Command show bounded previews.
                    let expandable = matches!(
                        entry.kind,
                        ToolTranscriptKind::Tool
                            | ToolTranscriptKind::Command
                            | ToolTranscriptKind::Explored
                    ) || edit_entry_is_expandable(entry);
                    let full_rows = if entry.kind == ToolTranscriptKind::Edit {
                        indent_full_tool_body(entry.body.clone(), width, show_picker).len()
                    } else {
                        indent_tool_result_body(
                            entry.body.clone(),
                            &entry.tool_name,
                            &state.verbosity(),
                            width,
                            true,
                        )
                        .len()
                    };
                    let low = matches!(state.verbosity(), rustcode::controller::Verbosity::Low);
                    // A task completion stays one row until opened, so any
                    // output at all is worth the hint.
                    let closed_until_opened = entry.tool_name == TASK_COMPLETION_TOOL;
                    let show_hint = expandable
                        && (full_rows > COLLAPSED_TOOL_BODY_MAX_LINES
                            || (closed_until_opened && full_rows > 0))
                        && !is_expanded
                        && low;
                    // A continuation batch renders under a heading an earlier
                    // frame already committed, so scrollback can never be
                    // revised: only `include_header` batches know their last
                    // child is final. Continuations keep the downward connector
                    // instead of every sibling claiming `└` (#1725).
                    let is_last = include_header && child_index + 1 == visible.len();
                    // A row that stays closed has nothing under it for the hint
                    // to follow, so the hint is part of the row and the target
                    // makes room for it. It is the first thing a narrow row
                    // gives up, and then it goes below after all.
                    let hint_in_row = show_hint && closed_until_opened;
                    let title = if entry.kind == ToolTranscriptKind::Command {
                        command_child_lines(entry, is_last, hint_in_row, width, show_picker)
                    } else {
                        tool_child_line(entry, is_last, hint_in_row, width, show_picker)
                    };
                    let hint = expand_hint_span(width, show_picker);
                    let hint_drawn = hint_in_row
                        && title.first().is_some_and(|line| {
                            line.spans
                                .last()
                                .is_some_and(|span| span.content == hint.content)
                        });
                    // Verbosity sets the default: low shows a five-row preview,
                    // high shows the row alone. An opened entry shows it all.
                    let mut body = Vec::new();
                    if entry.kind == ToolTranscriptKind::Edit && edit_entry_is_expandable(entry) {
                        if is_expanded {
                            body.extend(indent_full_tool_body(
                                entry.body.clone(),
                                width,
                                show_picker,
                            ));
                        } else if low {
                            body.extend(indent_file_edit_preview_body(
                                entry.body.clone(),
                                width,
                                show_picker,
                            ));
                        }
                    } else if expandable && (is_expanded || (low && !closed_until_opened)) {
                        if entry.kind == ToolTranscriptKind::Command {
                            body.extend(indent_tool_result_body(
                                entry.body.clone(),
                                &entry.tool_name,
                                &state.verbosity(),
                                width,
                                is_expanded,
                            ));
                        } else {
                            body.extend(indent_generic_tool_body(
                                entry.body.clone(),
                                &state.verbosity(),
                                width,
                                show_picker,
                                is_expanded,
                            ));
                        }
                    }
                    append_tool_preview(
                        &mut lines,
                        title,
                        body,
                        entry,
                        show_hint && !hint_drawn,
                        width,
                        show_picker,
                    );
                }
            }
        }

        index = group_end;
    }
    lines
}

/// Message indices the expand key can act on, oldest first.
///
/// Derived from the same rules the renderer applies, so the hint and the key
/// can never disagree about what is expandable (#1541, #1563):
/// - generic Tool entries with a non-empty body;
/// - Command entries with a non-empty body, whether their provider batch is
///   homogeneous or mixed;
/// - Edit entries with an expandable diff body (successful changes with
///   changed lines; no-op/failed keep truthful status, #1567).
/// Already expanded entries stay in the list: they are what the next press
/// collapses, so dropping them would make a second press skip past the entry
/// it expanded.
pub(crate) fn collapsible_tool_indices(state: &RenderSnapshot, width: u16) -> Vec<usize> {
    let history = state.active_history();
    // Group consecutive tool messages the way the transcript does, joining
    // across tool-only assistant turns (one-tool-per-round orchestration).
    // A batch's homogeneity decides its grouping, while each command result
    // keeps an independent expansion target.
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut idx = 0;
    while idx < history.len() {
        let Some(msg) = history.get(idx) else {
            break;
        };
        if msg.role == "tool" {
            current.push(idx);
            idx += 1;
            continue;
        }
        if msg.role == "assistant" && !current.is_empty() {
            let has_calls = !msg.tool_calls.is_empty()
                || !rustcode_tool_protocol::resolve_tool_calls(msg, state.active_tool_protocol())
                    .is_empty();
            let next_is_tool = history.get(idx + 1).is_some_and(|next| next.role == "tool");
            if has_calls && tool_step_is_silent(state, idx) && next_is_tool {
                idx += 1;
                continue;
            }
        }
        if !current.is_empty() {
            batches.push(std::mem::take(&mut current));
        }
        idx += 1;
    }
    if !current.is_empty() {
        batches.push(current);
    }

    let mut out = Vec::new();
    for batch in batches {
        let mut seen_explorations: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Fold task completions exactly as the renderer does, so a row the
        // transcript does not draw is never a candidate.
        let entries = fold_task_completions(
            batch
                .iter()
                .filter_map(|&i| tool_transcript_entry(state, i, width, false))
                .collect(),
        );
        for entry in entries {
            let i = entry.message_index;
            // The renderer folds repeated identical `Explored` rows into a
            // single child, so the walk must not offer a candidate for a row
            // the transcript does not draw: a press would report expanding
            // something that stayed collapsed (#1594).
            if entry.kind == ToolTranscriptKind::Explored
                && !seen_explorations.insert(format!("{}\0{}", entry.action, entry.target))
            {
                continue;
            }
            match entry.kind {
                ToolTranscriptKind::Tool => {
                    if !entry.body.is_empty() {
                        out.push(i);
                    }
                }
                ToolTranscriptKind::Command => {
                    if !entry.body.is_empty() {
                        out.push(i);
                    }
                }
                ToolTranscriptKind::Edit => {
                    if edit_entry_is_expandable(&entry) {
                        out.push(i);
                    }
                }
                ToolTranscriptKind::Explored => {
                    if !entry.body.is_empty() {
                        out.push(i);
                    }
                }
            }
        }
    }
    out
}

/// How much of the transcript is expanded, as `(expanded, collapsible)`.
///
/// The transcript-level readout, so the whole-session state is visible while
/// scrolling instead of only inferable from per-row hints (#1594).
pub(crate) fn expand_progress(state: &RenderSnapshot, width: u16) -> (usize, usize) {
    let candidates = collapsible_tool_indices(state, width);
    let expanded = candidates
        .iter()
        .filter(|index| state.expanded_thoughts().contains(index))
        .count();
    (expanded, candidates.len())
}

pub(super) fn render_committed_tool_result(
    state: &RenderSnapshot,
    message_index: usize,
    _tool_name: &str,
    _result: &str,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    render_committed_tool_result_group_snapshot(state, &[message_index], width, show_picker)
}

pub(super) fn format_elapsed_compact(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if milliseconds < 1_000 {
        "<1s".to_owned()
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

pub(crate) fn render_work_separator_before_assistant_snapshot(
    state: &RenderSnapshot,
    assistant_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let history = state.active_history();
    let Some(message) = history.get(assistant_index) else {
        return Vec::new();
    };
    if message.role != "assistant" || message.content.trim().is_empty() {
        return Vec::new();
    }
    let follows_work = history[..assistant_index]
        .iter()
        .rev()
        .find(|candidate| {
            !((candidate.role == "system" || candidate.role == "assistant")
                && is_hidden_system_notice(&candidate.content))
        })
        .is_some_and(|candidate| candidate.role == "tool");
    if !follows_work {
        return Vec::new();
    }

    let label = message
        .response_time_ms
        .map(|milliseconds| format!("─ Worked for {} ─", format_elapsed_compact(milliseconds)));
    let text = if let Some(label) = label {
        let label_width = label.width();
        format!(
            "{label}{}",
            "─".repeat((width as usize).saturating_sub(label_width))
        )
    } else {
        "─".repeat(width.max(1) as usize)
    };
    vec![
        Line::from(Span::styled(
            text,
            get_themed_style(COLOR_TURN_SEPARATOR(), COLOR_BG(), Modifier::empty(), false),
        )),
        Line::from(""),
    ]
}

pub(super) fn push_centered_separator<'a>(
    lines: &mut Vec<Line<'a>>,
    label_text: &str,
    width: u16,
    show_picker: bool,
) {
    if lines.last().map_or(true, |l| !l.spans.is_empty()) {
        lines.push(Line::from(""));
    }
    let label = format!(" {} ", label_text.trim());
    let remaining = (width as usize).saturating_sub(label.width());
    let left = remaining / 2;
    let right = remaining - left;
    let line_style = get_themed_style(
        COLOR_TURN_SEPARATOR(),
        COLOR_BG(),
        Modifier::empty(),
        show_picker,
    );
    let label_style = get_themed_style(
        COLOR_TURN_SEPARATOR(),
        COLOR_BG(),
        Modifier::BOLD,
        show_picker,
    );
    lines.push(Line::from(vec![
        Span::styled("─".repeat(left), line_style),
        Span::styled(label, label_style),
        Span::styled("─".repeat(right), line_style),
    ]));
}

pub(super) fn push_left_aligned_separator<'a>(
    lines: &mut Vec<Line<'a>>,
    label_text: &str,
    width: u16,
    show_picker: bool,
) {
    if lines.last().map_or(true, |l| !l.spans.is_empty()) {
        lines.push(Line::from(""));
    }
    let label = format!("─ {} ─", label_text.trim());
    let label_width = label.width();
    let line_style = get_themed_style(
        COLOR_TURN_SEPARATOR(),
        COLOR_BG(),
        Modifier::empty(),
        show_picker,
    );
    let label_style = get_themed_style(
        COLOR_TURN_SEPARATOR(),
        COLOR_BG(),
        Modifier::BOLD,
        show_picker,
    );
    lines.push(Line::from(vec![
        Span::styled(label, label_style),
        Span::styled(
            "─".repeat((width as usize).saturating_sub(label_width)),
            line_style,
        ),
    ]));
}

#[cfg(test)]
pub(super) fn push_new_chat_separator<'a>(
    lines: &mut Vec<Line<'a>>,
    width: u16,
    show_picker: bool,
) {
    push_centered_separator(lines, "✨ NEW CHAT", width, show_picker);
    lines.push(Line::from(""));
}

pub(crate) fn is_hidden_system_notice(content: &str) -> bool {
    content.contains("Loop warning:")
        || matches!(
            content.trim(),
            "YOLO mode enabled"
                | "YOLO mode disabled"
                | "Request cancelled by user"
                | "[harness: turn stopped — cancelled]"
        )
        || content.contains("tool calls in that response were dropped")
        || content.contains("Oversized response:")
        || is_deferred_tool_batch_notice(content)
        || rustcode::controller::is_compaction_summary(content)
        || content.starts_with("[harness: stopped after ")
        || (content.starts_with("[harness: turn stopped — ") && !is_turn_cancelled_notice(content))
        || content.contains("Your reasoning became repetitive")
        || content.contains("reasoning loop")
        || content.starts_with("The provider exhausted the output-token budget")
}

const COMPACT_TOOL_WARNING: &str = "[Warning, check debug for more info]";
const DEFERRED_TOOL_NOTICE: &str = "Tool calls were deferred by the scheduler; they did not run.";
const MIXED_TOOL_NOTICE: &str = "Some tool calls were queued; review the other results above.";
const UNSCHEDULED_TOOL_NOTICE: &str = "Some tool calls were not run; review the results above.";

fn is_deferred_tool_batch_notice(content: &str) -> bool {
    let content = content.trim();
    content.starts_with("[The model emitted ")
        && content.contains(" tool calls.")
        && content.contains("remaining calls (")
        && content.contains("were not executed or scheduled.")
}

fn is_current_tool_batch_notice(content: &str) -> bool {
    let content = content.trim();
    content.starts_with("[The model emitted ")
        && content.contains(" tool calls.")
        && content.contains("were executed this round.")
}

/// The notice for a round that did not run every call, or `None` when the
/// harness only held calls it runs by itself: those need nothing from the
/// reader, and their rows appear when they run.
fn current_tool_batch_notice_for_display(content: &str) -> Option<&'static str> {
    let queued = content.contains("scheduler held ")
        && content.contains("queued them for automatic execution");
    let others =
        content.contains("The harness did not schedule ") || content.contains("The remaining ");
    match (queued, others) {
        (true, true) => Some(MIXED_TOOL_NOTICE),
        (true, false) => None,
        (false, _) => Some(UNSCHEDULED_TOOL_NOTICE),
    }
}

fn is_validation_rejection_notice(content: &str) -> bool {
    let content = content.trim();
    content.starts_with("[Tool call rejected before execution:")
        && content.contains("] Emit one corrected tool call.")
}

/// Return the presentation-safe form of a system notice.
///
/// Detailed harness diagnostics stay in canonical history and debug logs, but
/// implementation details such as validation schemas, deferred tool names, and
/// call ids should not expand into a wide, noisy transcript row.
pub(crate) fn system_notice_for_display(content: &str) -> Option<&str> {
    if is_current_tool_batch_notice(content) {
        current_tool_batch_notice_for_display(content)
    } else if is_deferred_tool_batch_notice(content) {
        Some(DEFERRED_TOOL_NOTICE)
    } else if is_validation_rejection_notice(content) {
        Some(COMPACT_TOOL_WARNING)
    } else if is_hidden_system_notice(content) {
        None
    } else {
        Some(content)
    }
}

#[cfg(test)]
pub(super) fn tool_result_follows(history: &[ChatMessage], assistant_index: usize) -> bool {
    next_visible_message(history, assistant_index).is_some_and(|message| message.role == "tool")
}

#[cfg(test)]
pub(super) fn next_visible_message(history: &[ChatMessage], index: usize) -> Option<&ChatMessage> {
    history.iter().skip(index + 1).find(|message| {
        !((message.role == "system" || message.role == "assistant")
            && is_hidden_system_notice(&message.content))
    })
}

#[cfg(test)]
pub(crate) fn tool_result_needs_assistant_gap(history: &[ChatMessage], tool_index: usize) -> bool {
    next_visible_message(history, tool_index).is_some_and(|message| message.role == "assistant")
}

pub(super) fn fit_to_width(s: &str, target_width: usize) -> String {
    let char_count = s.chars().count();
    if char_count > target_width {
        if target_width > 1 {
            let truncated: String = s.chars().take(target_width - 1).collect();
            format!("{truncated}…")
        } else {
            s.chars().take(target_width).collect()
        }
    } else {
        format!("{:<width$}", s, width = target_width)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn running_tool_result_uses_the_shared_small_status_marker() {
        let entry = super::ToolTranscriptEntry {
            message_index: 0,
            tool_name: "run_command".to_owned(),
            action: "Bash".to_owned(),
            target: "cargo test".to_owned(),
            success: false,
            status: "running".to_owned(),
            body: Vec::new(),
            kind: super::ToolTranscriptKind::Command,
            diff_counts: None,
            earlier: 0,
            earlier_failed: 0,
            target_is_note: false,
        };
        assert_eq!(super::tool_status_glyph(&entry), '○');

        let mut completed = entry;
        completed.status = "completed".to_owned();
        completed.success = true;
        assert_eq!(super::tool_status_glyph(&completed), '✓');
    }

    #[test]
    fn legacy_arguments_skip_prose_without_losing_supported_call_encodings() {
        use rustcode::controller::{ChatMessage, ToolProtocol};
        for protocol in [
            ToolProtocol::Native,
            ToolProtocol::Json,
            ToolProtocol::ApiNative,
        ] {
            for content in [
                r#"{"name":"run_command","arguments":{"command":"cargo test"}}"#,
                "```tool\n{\"name\":\"run_command\",\"arguments\":{\"command\":\"cargo test\"}}\n```",
                "[TOOL_CALLS]run_command {\"command\":\"cargo test\"}",
            ] {
                let mut state = RenderState::new();
                state.active_tool_protocol = protocol;
                let call = ChatMessage::new("assistant", content);
                let expected = rustcode_tool_protocol::resolve_tool_calls(&call, protocol);
                assert_eq!(expected.len(), 1, "fixture {content}");
                state.history.push(call);
                for _ in 0..100 {
                    state
                        .history
                        .push(ChatMessage::new("assistant", "Ordinary prose 日本語"));
                }
                state
                    .history
                    .push(ChatMessage::new("tool", "run_command: done"));
                let snapshot = crate::ui::render_snapshot::render_snapshot(&state);
                assert_eq!(
                    super::tool_call_arguments(&snapshot, 101, "run_command"),
                    expected[0].arguments
                );
            }
        }
    }

    #[test]
    fn inline_diff_reuses_cached_rows_and_keys_file_language_and_width() {
        let _theme_guard = crate::ui::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        super::TOOL_RESULT_CACHE.with(|cache| cache.borrow_mut().entries.clear());
        let diff = "@@ -1 +1 @@\n+fn main() { let answer = 42; }\n";
        let first = super::cached_file_edit_diff(diff, "main.rs", 40, false);
        let second = super::cached_file_edit_diff(diff, "main.rs", 40, false);
        assert_eq!(first, second);
        super::TOOL_RESULT_CACHE.with(|cache| assert_eq!(cache.borrow().entries.len(), 1));
        super::cached_file_edit_diff(diff, "main.py", 40, false);
        super::cached_file_edit_diff(diff, "main.rs", 20, false);
        super::cached_file_edit_diff(diff, "main.rs", 40, true);
        super::cached_file_edit_diff("@@ -1 +1 @@\n+changed\n", "main.rs", 40, false);
        super::TOOL_RESULT_CACHE.with(|cache| assert_eq!(cache.borrow().entries.len(), 5));
    }

    use super::{collapse_command_preview, is_hidden_system_notice, tool_result_status};
    use rustcode::controller::RenderState;

    #[test]
    fn raw_deferred_batch_notice_is_hidden_from_model_cells() {
        assert!(is_hidden_system_notice(
            "[The model emitted 4 tool calls. Only one was executed this round; the remaining calls (grep, write_to_file) were not executed or scheduled.]"
        ));
        assert!(!is_hidden_system_notice("Notice: background task finished"));
    }

    #[test]
    fn deferred_tool_batch_notice_does_not_look_like_a_failure() {
        assert_eq!(
            super::system_notice_for_display(
                "[The model emitted 5 tool calls. 4 were executed this round; the remaining calls (get_status (call_123)) were not executed or scheduled. Reissue deferred calls only after reviewing the real results.]"
            ),
            Some("Tool calls were deferred by the scheduler; they did not run.")
        );
    }

    #[test]
    fn scheduler_queued_notice_is_not_shown() {
        assert_eq!(
            super::system_notice_for_display(
                "[The model emitted 2 tool calls. 1 were executed this round. The scheduler held 1 over the per-response workspace-change limit and queued them for automatic execution in a later round (run_command (call_123)); do not reissue them, their real results arrive without another request from you.]"
            ),
            None,
            "calls the harness runs by itself need no notice"
        );
    }

    #[test]
    fn scheduler_mixed_and_unscheduled_notices_preserve_their_action() {
        assert_eq!(
            super::system_notice_for_display(
                "[The model emitted 3 tool calls. 1 were executed this round. The scheduler held 1 over the per-response workspace-change limit and queued them for automatic execution in a later round (write_to_file (call_2)); do not reissue them. The harness did not schedule 1 call(s) (grep (call_3)); reissue them only after reviewing the real results. The remaining 2 call(s) were not executed: review the real results above.]"
            ),
            Some("Some tool calls were queued; review the other results above.")
        );
        assert_eq!(
            super::system_notice_for_display(
                "[The model emitted 2 tool calls. 1 were executed this round. The harness did not schedule 1 call(s) (grep (call_2)); reissue them only after reviewing the real results. The remaining 1 call(s) were not executed: review the real results above.]"
            ),
            Some("Some tool calls were not run; review the results above.")
        );
    }

    #[test]
    fn deferred_tool_result_is_not_rendered_as_a_failed_call() {
        let mut state = RenderState::new();
        state.history.push(rustcode::controller::ChatMessage::new(
            "tool",
            "read_email: error: intentionally deferred by the scheduler; reissue it only if still needed after reviewing the executed results",
        ));
        let snapshot = crate::ui::render_snapshot::render_snapshot(&state);
        assert!(super::tool_transcript_entry(&snapshot, 0, 80, false).is_none());
    }

    #[test]
    fn queued_tool_result_hides_synthetic_error_but_keeps_canonical_detail() {
        use rustcode::controller::ToolResultRecord;

        let mut state = RenderState::new();
        let mut message = rustcode::controller::ChatMessage::new(
            "tool",
            "run_command: error: held by the harness and queued for automatic execution in a later round; do not reissue it",
        );
        message.tool_result = Some(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            success: false,
            error_kind: Some("Deferred".to_owned()),
            ..ToolResultRecord::default()
        });
        state.history.push(message);

        let snapshot = crate::ui::render_snapshot::render_snapshot(&state);
        assert!(
            super::tool_transcript_entry(&snapshot, 0, 80, false).is_none(),
            "synthetic queue acknowledgements should not look like command failures"
        );
        assert!(
            snapshot.active_history()[0]
                .content
                .contains("do not reissue it")
        );
    }

    #[test]
    fn validation_rejection_notice_has_a_compact_ui_projection() {
        let detailed = "[Tool call rejected before execution: invalid arguments for 'reply_to_chat_message'. Schema path: $.message_id is required. Expected arguments for 'reply_to_chat_message' use these keys: [\"chat_name\", \"confirm\", \"message\", \"message_id\"]. Example: {...}] Emit one corrected tool call. Batch independent read-only calls freely.";
        assert_eq!(
            super::system_notice_for_display(detailed),
            Some("[Warning, check debug for more info]")
        );
        assert!(!super::is_hidden_system_notice(detailed));
    }

    fn tool_message_with_record(
        record: rustcode::controller::ToolResultRecord,
    ) -> rustcode::controller::ChatMessage {
        let mut message = rustcode::controller::ChatMessage::new(
            "tool",
            "run_command: Task started in background.",
        );
        message.tool_result = Some(record);
        message
    }

    #[test]
    fn pending_background_launch_renders_running_not_failed() {
        let message = tool_message_with_record(rustcode::controller::ToolResultRecord {
            workspace_generation: None,
            workspace_epoch: None,
            evidence_hash: None,
            tool_name: "run_command".to_owned(),
            success: false,
            pending: true,
            ..Default::default()
        });
        assert_eq!(
            tool_result_status(&message, "run_command", "Task started in background."),
            (true, "background".to_owned())
        );
    }

    #[test]
    fn cancelled_background_task_renders_cancelled_not_failed() {
        let message = tool_message_with_record(rustcode::controller::ToolResultRecord {
            workspace_generation: None,
            workspace_epoch: None,
            evidence_hash: None,
            tool_name: "background_task".to_owned(),
            success: false,
            error_kind: Some("Cancelled".to_owned()),
            ..Default::default()
        });
        assert_eq!(
            tool_result_status(&message, "background_task", "background task cancelled"),
            (false, "cancelled".to_owned())
        );
    }

    #[test]
    fn completed_exit_zero_still_renders_exit_status() {
        let message = tool_message_with_record(rustcode::controller::ToolResultRecord {
            workspace_generation: None,
            workspace_epoch: None,
            evidence_hash: None,
            tool_name: "run_command".to_owned(),
            success: true,
            exit_code: Some(0),
            ..Default::default()
        });
        assert_eq!(
            tool_result_status(&message, "run_command", "exit code: 0"),
            (true, "exit 0".to_owned())
        );
    }

    #[test]
    fn command_preview_collapses_whitespace_and_truncates() {
        use unicode_width::UnicodeWidthStr;
        assert_eq!(collapse_command_preview("ls -la", 20), "ls -la");
        let collapsed = collapse_command_preview("echo a;\n  echo b", 20);
        assert!(!collapsed.contains('\n'), "{collapsed:?}");
        let long = collapse_command_preview("curl -sS https://example.com/very/long/path", 20);
        assert!(long.ends_with('…'), "{long:?}");
        assert!(long.width() <= 20, "{long:?}");
    }

    #[test]
    fn fitting_generic_tool_title_keeps_hint_inline() {
        let entry = super::ToolTranscriptEntry {
            message_index: 0,
            tool_name: "get_time".to_owned(),
            action: "GetTime".to_owned(),
            target: "".to_owned(),
            success: true,
            status: "completed".to_owned(),
            body: (0..10)
                .map(|i| ratatui::text::Line::from(format!("output {i}")))
                .collect(),
            kind: super::ToolTranscriptKind::Tool,
            diff_counts: None,
            earlier: 0,
            earlier_failed: 0,
            target_is_note: false,
        };
        let title = super::tool_child_line(&entry, true, false, 80, false);
        let body = super::indent_generic_tool_body(
            entry.body.clone(),
            &rustcode::controller::Verbosity::Low,
            80,
            false,
            false,
        );
        let mut lines = Vec::new();
        super::append_tool_preview(&mut lines, title, body, &entry, true, 80, false);
        assert!(lines[0].to_string().contains("GetTime (ctrl+o all"));
        assert_eq!(lines.len(), 6);
        assert!(lines.last().unwrap().to_string().contains("output 9"));
    }

    #[test]
    fn narrow_command_preview_keeps_compact_expand_hint_inside_its_row() {
        use ratatui::{
            buffer::Buffer,
            layout::Rect,
            widgets::{Paragraph, Widget},
        };

        let entry = super::ToolTranscriptEntry {
            message_index: 0,
            tool_name: "run_command".to_owned(),
            action: "Run".to_owned(),
            target: "echo this command has a longer target".to_owned(),
            success: true,
            status: "exit 0".to_owned(),
            body: Vec::new(),
            kind: super::ToolTranscriptKind::Command,
            diff_counts: None,
            earlier: 0,
            earlier_failed: 0,
            target_is_note: false,
        };
        let width = 24;
        let lines = super::command_child_lines(&entry, true, true, width, false);
        assert!(
            lines.iter().all(|line| line.width() <= usize::from(width)),
            "wrapped command rows plus expand hint must stay within the terminal width: {lines:?}"
        );
        assert!(lines[0].to_string().contains("(ctrl+o all)"), "{lines:?}");
        assert!(!lines[0].to_string().contains("to expand"), "{lines:?}");

        let area = Rect::new(0, 0, width, lines.len() as u16);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines).render(area, &mut buffer);
        let first_row = (0..width)
            .map(|column| buffer[(column, 0)].symbol())
            .collect::<String>();
        assert!(first_row.contains("(ctrl+o all)"), "{first_row:?}");
    }

    #[test]
    fn tool_preview_window_preserves_head_tail_and_forced_markers() {
        use ratatui::text::Line;

        let overflow = super::tool_preview_window(9, false).expect("overflow marker");
        assert_eq!(
            (
                overflow.head_rows,
                overflow.tail_rows,
                overflow.omitted_rows
            ),
            (2, 2, 5)
        );
        let rows = (0..9)
            .map(|row| Line::from(format!("row {row}")))
            .collect::<Vec<_>>();
        let capped = super::cap_collapsed_tool_body(rows, false)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            capped,
            ["row 0", "row 1", "  │ … +5 lines", "row 7", "row 8"]
        );

        let forced_marker = super::tool_preview_window(4, true).expect("byte marker");
        assert_eq!(
            (
                forced_marker.head_rows,
                forced_marker.tail_rows,
                forced_marker.omitted_rows
            ),
            (4, 0, 0)
        );

        assert!(super::tool_preview_window(5, false).is_none());
        let full_with_forced_marker =
            super::tool_preview_window(5, true).expect("marker occupies one preview row");
        assert_eq!(
            (
                full_with_forced_marker.head_rows,
                full_with_forced_marker.tail_rows,
                full_with_forced_marker.omitted_rows
            ),
            (2, 2, 1)
        );
    }
}
