use super::*;
use unicode_width::UnicodeWidthStr;

/// Maximum wrapped visual lines for a committed shell command preview,
/// mirroring Codex `command_continuation_max_lines = 2`. Longer commands
/// collapse with an ellipsis instead of flooding the transcript.

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
    if let Some(home) = home_path {
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
        "run_command" | "runcommand" | "execute_command" | "bash" => "Bash".to_string(),
        "search_web" | "searchweb" | "codebase_search" | "codebasesearch" => "Search".to_string(),
        "get_project_map" | "getprojectmap" => "ProjectMap".to_string(),
        "manage_task" | "managetask" => "ManageTask".to_string(),
        "background_task" | "backgroundtask" => "TaskDone".to_string(),
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
type RenderedConversation = (Vec<Line<'static>>, Vec<(u16, String)>, Vec<u16>, u16);

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

#[allow(dead_code)]
pub(super) fn chat_cache_key(state: &RenderSnapshot, width: u16, show_picker: bool) -> ChatKey {
    let history = state.active_history();
    ChatKey {
        hist_len: history.len(),
        total_len: history.iter().map(|m| m.content.len()).sum(),
        last_len: history.last().map_or(0, |m| m.content.len()),
        history_display_start: state.active_history_display_start(),
        width,
        show_picker,
        copied_recently: state
            .last_copy_text()
            .as_ref()
            .map(|(t_text, t)| (t_text.clone(), t.elapsed().as_secs() < 2)),
        theme: state.config().theme.clone(),
    }
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
        let answer = ask_question_answer(state.active_history(), message_index);
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
        let question = ask_question_text(&args);
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
            return (true, "running".to_owned());
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

pub(super) fn indent_tool_result_body(
    lines: Vec<Line<'static>>,
    tool_name: &str,
    verbosity: &rustcode::controller::Verbosity,
    width: u16,
    expanded: bool,
) -> Vec<Line<'static>> {
    if matches!(verbosity, rustcode::controller::Verbosity::High) {
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
    for (index, line) in visible.into_iter().enumerate() {
        if line.spans.is_empty() {
            indented.push(line);
            continue;
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::styled(
            if index == 0 { "  └ " } else { "    " },
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        ));
        spans.extend(line.spans);
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), false),
        );
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

    for (assistant_index, assistant) in history[..message_index].iter().enumerate().rev() {
        if assistant.role != "assistant" {
            continue;
        }
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
    let (action, target) = if kind == ToolTranscriptKind::Explored {
        let args = tool_call_arguments(state, message_index, &tool_name);
        format_exploration_action(&tool_name, &args, state.home_path())
    } else {
        tool_result_action(state, message_index, &tool_name)
    };
    let (success, status) = tool_result_status(message, &tool_name, result);
    let mut body = cached_tool_result(
        &tool_name,
        result,
        width as usize,
        &state.verbosity(),
        show_picker,
    );
    // Write/edit calls whose result carries no embedded diff (e.g.
    // `write_to_file` reports only `wrote 'path' (N lines, M bytes)`) still
    // need their changed lines at low verbosity (#1567). Synthesize an
    // added-lines preview from the call arguments; no-op and failed changes
    // keep their truthful single-line status.
    if kind == ToolTranscriptKind::Edit
        && success
        && !edit_result_is_noop(result)
        && !result_has_embedded_diff(result)
        && body.len() <= 1
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
    })
}

pub(super) fn tool_group_header(title: &str, success: bool, show_picker: bool) -> Line<'static> {
    let bullet_color = if success {
        COLOR_GREEN()
    } else {
        Color::Rgb(229, 123, 123)
    };
    Line::from(vec![
        Span::styled(
            "• ",
            get_themed_style(bullet_color, COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            title.to_owned(),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
    ])
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
    preview.push(Line::from(Span::styled(
        format!("    … +{} lines", window.omitted_rows),
        get_themed_style(
            COLOR_MUTED(),
            COLOR_BG(),
            Modifier::ITALIC | Modifier::DIM,
            show_picker,
        ),
    )));
    preview.extend(tail);
    preview
}

/// Display width the expand hint occupies once appended to a row.
pub(super) const EXPAND_HINT_WIDTH: u16 = EXPAND_HINT.len() as u16;

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

/// Append the expand hint to the first row of an entry's own block.
///
/// The hint must never be appended to the last wrapped line: that line is
/// followed by the next tool row, so the hint reads as annotating *that* row
/// and splits the `Ran` group (#1541).
fn append_expand_hint(lines: &mut [Line<'static>], width: u16, show_picker: bool) {
    if let Some(first) = lines.first_mut() {
        first.spans.push(expand_hint_span(width, show_picker));
    }
}

pub(super) fn tool_child_line(
    entry: &ToolTranscriptEntry,
    first: bool,
    show_hint: bool,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let mut spans = vec![Span::styled(
        if first { "  └ " } else { "    " },
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    )];
    if entry.kind == ToolTranscriptKind::Edit {
        if !entry.target.is_empty() && entry.target != "?" {
            spans.push(Span::styled(
                entry.target.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        } else {
            spans.push(Span::styled(
                entry.action.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
        }
    } else {
        spans.push(Span::styled(
            entry.action.clone(),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
        if !entry.target.is_empty() && entry.target != "?" {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                entry.target.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    }
    let mut lines = Vec::new();
    let continuation = Span::styled(
        "    ",
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    );
    push_wrapped_with_continuation(
        &mut lines,
        spans,
        wrap_width(width, show_hint),
        Some(continuation),
    );
    if show_hint {
        append_expand_hint(&mut lines, width, show_picker);
    }
    lines
}

/// Wrap width for a row that carries the expand hint, leaving room for the
/// hint so the row it annotates is the row the hint lands on.
fn wrap_width(width: u16, show_hint: bool) -> usize {
    let hint_width = if width >= 29 {
        EXPAND_HINT_WIDTH
    } else if width >= 19 {
        COMPACT_EXPAND_HINT.width() as u16
    } else {
        SHORT_EXPAND_HINT.width() as u16
    };
    let reserved = if show_hint { hint_width } else { 0 };
    (width.saturating_sub(reserved) as usize).max(10)
}

/// Maximum wrapped visual lines for a committed shell command preview,
/// mirroring Codex `command_continuation_max_lines = 2`.
pub(super) const COMMAND_DISPLAY_MAX_LINES: usize = 2;

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
    for grapheme in single.split("").filter(|s| !s.is_empty()) {
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

/// Cap already-wrapped visual lines, appending an ellipsis to the last kept
/// line when content was dropped. Keeps long `echo ...; pacman ...` chains to
/// `COMMAND_DISPLAY_MAX_LINES` rows instead of flooding scrollback.
fn truncate_wrapped_lines(mut lines: Vec<Line<'static>>, max_lines: usize) -> Vec<Line<'static>> {
    if lines.len() <= max_lines || max_lines == 0 {
        return lines;
    }
    lines.truncate(max_lines);
    if let Some(last) = lines.last_mut() {
        last.spans.push(Span::styled(
            " …",
            get_themed_style(
                COLOR_MUTED(),
                COLOR_BG(),
                Modifier::DIM | Modifier::ITALIC,
                false,
            ),
        ));
    }
    lines
}

pub(super) fn command_child_lines(
    entry: &ToolTranscriptEntry,
    first: bool,
    show_hint: bool,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    // Collapse multi-line / chained commands to a single-line preview before
    // highlighting, so `echo a; echo b; ...` renders as one dimmable row
    // instead of N source lines each wrapping again.
    let preview = collapse_command_preview(
        &entry.target,
        (width as usize)
            .saturating_sub(
                12 + if show_hint {
                    EXPAND_HINT_WIDTH as usize
                } else {
                    0
                },
            )
            .max(20),
    );
    let mut commands = highlight_shell_command(&preview, COLOR_BG(), show_picker);
    if commands.is_empty() {
        commands.push(Line::default());
    }
    let mut lines = Vec::with_capacity(commands.len());
    let status_suffix =
        (!entry.success || entry.status == "running").then(|| format!(" · {}", entry.status));
    let max_w = wrap_width(width, show_hint)
        .saturating_sub(status_suffix.as_ref().map_or(0, |status| status.width()))
        .max(10);
    for (command_index, command) in commands.into_iter().enumerate() {
        let mut spans = vec![Span::styled(
            if first && command_index == 0 {
                "  └ "
            } else {
                "    "
            },
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        )];
        if command_index == 0 {
            spans.push(Span::styled(
                entry.action.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
            if !entry.target.is_empty() && entry.target != "?" {
                spans.push(Span::styled(
                    " ",
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
            }
        }
        if entry.target != "?" {
            spans.extend(command.spans);
        }
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        );
        push_wrapped_with_continuation(&mut lines, spans, max_w, Some(continuation));
    }
    let mut lines = truncate_wrapped_lines(lines, COMMAND_DISPLAY_MAX_LINES);
    if show_hint {
        append_expand_hint(&mut lines, width, show_picker);
    }
    if let Some(status_suffix) = status_suffix {
        if let Some(line) = lines.last_mut() {
            line.spans.push(Span::styled(
                status_suffix,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    }
    lines
}

pub(super) fn command_summary_lines(
    entry: &ToolTranscriptEntry,
    width: u16,
    show_hint: bool,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let bullet_color = if entry.success {
        COLOR_GREEN()
    } else {
        Color::Rgb(229, 123, 123)
    };
    let has_command = !entry.target.is_empty() && entry.target != "?";
    let prefix = if has_command { "Ran $ " } else { "Ran Bash" };
    let status_suffix = format!(" · {}", entry.status);
    let available = wrap_width(width, show_hint).min(width as usize);
    let preview_width = available.saturating_sub(2 + prefix.width() + status_suffix.width());
    let preview = collapse_command_preview(&entry.target, preview_width);
    let mut spans = vec![
        Span::styled(
            "• ",
            get_themed_style(bullet_color, COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            prefix,
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
    ];
    if has_command && preview_width > 0 {
        for command in highlight_shell_command(&preview, COLOR_BG(), show_picker) {
            spans.extend(command.spans);
        }
    }
    spans.push(Span::styled(
        status_suffix,
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    ));
    let line = Line::from(spans);
    let line = if line.width() > available {
        Line::from(Span::styled(
            collapse_command_preview(&line.to_string(), available),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ))
    } else {
        line
    };
    let mut lines = vec![line];
    if show_hint {
        append_expand_hint(&mut lines, width, show_picker);
    }
    lines
}

pub(super) fn indent_generic_tool_body(
    lines: Vec<Line<'static>>,
    verbosity: &rustcode::controller::Verbosity,
    width: u16,
    show_picker: bool,
    expanded: bool,
) -> Vec<Line<'static>> {
    if matches!(verbosity, rustcode::controller::Verbosity::High) {
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
            indented.push(line);
            continue;
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        spans.extend(line.spans);
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        );
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
            indented.push(line);
            continue;
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        spans.extend(line.spans);
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        );
        push_wrapped_with_continuation(&mut indented, spans, max_w, Some(continuation));
    }
    indented
}

/// Whether an edit entry carries an expandable diff body.
///
/// Successful edits with changed lines (embedded or synthesized diffs, #1567)
/// expand; no-op (`already applied`) and failed changes keep their truthful
/// single-line status with no hint.
pub(super) fn edit_entry_is_expandable(entry: &ToolTranscriptEntry) -> bool {
    entry.kind == ToolTranscriptKind::Edit
        && entry.success
        && !entry.body.is_empty()
        && entry.body.len() > 1
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
        push_wrapped_with_continuation(
            &mut hint_lines,
            vec![Span::raw("    "), hint],
            (width as usize).max(1),
            Some(Span::raw("    ")),
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

fn render_tool_result_group_snapshot(
    state: &RenderSnapshot,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
    include_header: bool,
) -> Vec<Line<'static>> {
    let entries = message_indices
        .iter()
        .filter_map(|&index| tool_transcript_entry(state, index, width, show_picker))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        let kind = entries[index].kind;
        // `message_indices` represents one provider batch (and may include
        // tool-only assistant turns joined by the scrollback layer). Keep it
        // as one visual group even when the provider mixed command, read, or
        // edit tools. The child rows retain their kind-specific formatting.
        let whole_batch = &entries[index..];
        let homogeneous = whole_batch.iter().all(|entry| entry.kind == kind);
        let group_end = if homogeneous
            && kind == ToolTranscriptKind::Command
            && matches!(state.verbosity(), rustcode::controller::Verbosity::Low)
        {
            index + 1
        } else {
            entries.len()
        };
        let group = &entries[index..group_end];
        let success = group.iter().all(|entry| entry.success);

        if include_header && !lines.is_empty() {
            lines.push(Line::from(""));
        }
        if include_header && homogeneous && kind == ToolTranscriptKind::Command {
            if matches!(state.verbosity(), rustcode::controller::Verbosity::High) {
                lines.push(tool_group_header("Ran", success, show_picker));
                for (child_index, entry) in group.iter().enumerate() {
                    // High verbosity renders the body inline, so these rows
                    // never collapse and never carry the expand hint.
                    lines.extend(command_child_lines(
                        entry,
                        child_index == 0,
                        false,
                        width,
                        show_picker,
                    ));
                }
            } else {
                let entry = &group[0];
                let is_expanded = state.expanded_thoughts().contains(&entry.message_index);
                let show_hint = !is_expanded
                    && indent_tool_result_body(
                        entry.body.clone(),
                        &entry.tool_name,
                        &state.verbosity(),
                        width,
                        true,
                    )
                    .len()
                        > COLLAPSED_TOOL_BODY_MAX_LINES;
                let title = command_summary_lines(entry, width, false, show_picker);
                let body = indent_tool_result_body(
                    entry.body.clone(),
                    &entry.tool_name,
                    &state.verbosity(),
                    width,
                    is_expanded,
                );
                append_tool_preview(
                    &mut lines,
                    title,
                    body,
                    entry,
                    show_hint,
                    width,
                    show_picker,
                );
            }
        } else {
            if include_header {
                let title = if !homogeneous {
                    "Ran"
                } else if kind == ToolTranscriptKind::Explored {
                    "Explored"
                } else if kind == ToolTranscriptKind::Edit {
                    if group.iter().all(|entry| entry.action == "Write") {
                        "Wrote"
                    } else {
                        "Edited"
                    }
                } else if kind == ToolTranscriptKind::Tool {
                    "Ran"
                } else {
                    "Called"
                };
                lines.push(tool_group_header(title, success, show_picker));
            }
            let mut seen = std::collections::HashSet::new();
            let mut first_child = true;
            for entry in group {
                let identity = format!("{}\0{}", entry.action, entry.target);
                if entry.kind != ToolTranscriptKind::Explored || seen.insert(identity) {
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
                    let show_hint = expandable
                        && full_rows > COLLAPSED_TOOL_BODY_MAX_LINES
                        && !is_expanded
                        && matches!(state.verbosity(), rustcode::controller::Verbosity::Low);
                    let title = if entry.kind == ToolTranscriptKind::Command {
                        command_child_lines(entry, first_child, false, width, show_picker)
                    } else {
                        tool_child_line(entry, first_child, false, width, show_picker)
                    };
                    let mut body = Vec::new();
                    first_child = false;
                    let low = matches!(state.verbosity(), rustcode::controller::Verbosity::Low);
                    if entry.kind == ToolTranscriptKind::Edit
                        && edit_entry_is_expandable(entry)
                        && low
                    {
                        if is_expanded {
                            body.extend(indent_full_tool_body(
                                entry.body.clone(),
                                width,
                                show_picker,
                            ));
                        } else {
                            body.extend(indent_generic_tool_body(
                                entry.body.clone(),
                                &state.verbosity(),
                                width,
                                show_picker,
                                false,
                            ));
                        }
                    } else if expandable && low {
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
                        show_hint,
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
/// - low verbosity only (high verbosity renders bodies inline, never collapsed);
/// - generic Tool entries with a non-empty body;
/// - Command entries with a non-empty body, whether their provider batch is
///   homogeneous or mixed;
/// - Edit entries with an expandable diff body (successful changes with
///   changed lines; no-op/failed keep truthful status, #1567).
/// Already expanded entries stay in the list: they are what the next press
/// collapses, so dropping them would make a second press skip past the entry
/// it expanded.
pub(crate) fn collapsible_tool_indices(state: &RenderSnapshot, width: u16) -> Vec<usize> {
    if !matches!(state.verbosity(), rustcode::controller::Verbosity::Low) {
        return Vec::new();
    }
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
            let content_empty = msg.content.trim().is_empty();
            let next_is_tool = history.get(idx + 1).is_some_and(|next| next.role == "tool");
            if has_calls && content_empty && next_is_tool {
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
        for &i in &batch {
            let Some(entry) = tool_transcript_entry(state, i, width, false) else {
                continue;
            };
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
    if minutes > 0 {
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
        .filter(|milliseconds| *milliseconds > 60_000)
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
}

const COMPACT_TOOL_WARNING: &str = "[Warning, check debug for more info]";
const DEFERRED_TOOL_NOTICE: &str = "Tool calls were deferred by the scheduler; they did not run.";

fn is_deferred_tool_batch_notice(content: &str) -> bool {
    let content = content.trim();
    content.starts_with("[The model emitted ")
        && content.contains(" tool calls.")
        && content.contains("remaining calls (")
        && content.contains("were not executed or scheduled.")
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
    if is_deferred_tool_batch_notice(content) {
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
    use super::{
        COMMAND_DISPLAY_MAX_LINES, collapse_command_preview, is_hidden_system_notice,
        tool_result_status, truncate_wrapped_lines,
    };
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
            tool_name: "run_command".to_owned(),
            success: false,
            pending: true,
            ..Default::default()
        });
        assert_eq!(
            tool_result_status(&message, "run_command", "Task started in background."),
            (true, "running".to_owned())
        );
    }

    #[test]
    fn cancelled_background_task_renders_cancelled_not_failed() {
        let message = tool_message_with_record(rustcode::controller::ToolResultRecord {
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
    fn completed_command_header_stays_on_one_line() {
        let entry = super::ToolTranscriptEntry {
            message_index: 0,
            tool_name: "run_command".to_owned(),
            action: "Bash".to_owned(),
            target: format!(
                "python3 - <<'PY'\n{}\nPY",
                "日本語 long command ".repeat(30)
            ),
            success: true,
            status: "exit 0".to_owned(),
            body: vec![],
            kind: super::ToolTranscriptKind::Command,
        };
        for width in [18, 24, 48, 80, 180] {
            let lines = super::command_summary_lines(&entry, width, false, false);
            assert_eq!(lines.len(), 1, "width {width}: {lines:?}");
            assert!(lines[0].width() <= width as usize);
            assert!(lines[0].to_string().starts_with("• Ran $ "));
            assert!(lines[0].to_string().ends_with("… · exit 0"));
        }
    }

    #[test]
    fn expansion_hint_follows_long_command_output_without_using_preview_rows() {
        for width in [24, 48, 80] {
            let entry = super::ToolTranscriptEntry {
                message_index: 0,
                tool_name: "run_command".to_owned(),
                action: "Bash".to_owned(),
                target: format!("echo {}\necho done", "日本語".repeat(30)),
                success: true,
                status: "exit 0".to_owned(),
                body: (0..10)
                    .map(|i| ratatui::text::Line::from(format!("output {i}")))
                    .collect(),
                kind: super::ToolTranscriptKind::Command,
            };
            let title = super::command_summary_lines(&entry, width, false, false);
            let body = super::indent_tool_result_body(
                entry.body.clone(),
                &entry.tool_name,
                &rustcode::controller::Verbosity::Low,
                width,
                false,
            );
            assert_eq!(body.len(), 5);
            let title_count = title.len();
            let mut lines = Vec::new();
            super::append_tool_preview(&mut lines, title, body, &entry, true, width, false);
            assert!(
                lines[..title_count]
                    .iter()
                    .all(|line| !line.to_string().contains("ctrl+o"))
            );
            assert!(lines.last().unwrap().to_string().contains("ctrl+o"));
            assert!(lines[lines.len() - 2].to_string().contains("output 9"));
            assert_eq!(lines.len(), title_count + 6);
            assert!(
                lines.iter().all(|line| line.width() <= width as usize),
                "{lines:?}"
            );
        }
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
    fn wrapped_command_lines_cap_at_preview_limit() {
        use ratatui::text::Line;
        let lines = (0..10)
            .map(|i| Line::from(format!("line {i}")))
            .collect::<Vec<_>>();
        let capped = truncate_wrapped_lines(lines, COMMAND_DISPLAY_MAX_LINES);
        assert_eq!(capped.len(), COMMAND_DISPLAY_MAX_LINES);
        assert!(capped.last().unwrap().to_string().contains('…'));
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
            ["row 0", "row 1", "    … +5 lines", "row 7", "row 8"]
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
