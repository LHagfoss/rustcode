use super::*;

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
        | "createfile" => "Write".to_string(),
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
        "remember" => "Remember".to_string(),
        "recall_memory" | "recallmemory" => "Recall".to_string(),
        "forget_memory" | "forgetmemory" => "Forget".to_string(),
        _ => to_pascal_case(name),
    };

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
            .unwrap_or("?")
            .to_string(),
        "use_skill" => args
            .get("name")
            .or_else(|| args.get("skill"))
            .or_else(|| args.get("skill_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "spawn_agent" => args
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "send_agent" => args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "wait_agent" | "cancel_agent" => args
            .get("id")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            })
            .unwrap_or_default(),
        "set_goal" => args
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "ask_question" => {
            if let Some(q) = args.get("question").and_then(|v| v.as_str()) {
                q.to_string()
            } else if let Some(q_arr) = args.get("questions").and_then(|v| v.as_array()) {
                q_arr
                    .first()
                    .and_then(|q| q.get("question"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
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
        if k == "CodeContent"
            || k == "ReplacementContent"
            || k == "content"
            || k == "system_prompt"
            || k == "Code"
            || k == "toolSummary"
            || k == "toolAction"
        {
            continue;
        }
        let val_str = match v {
            serde_json::Value::String(s) => {
                let first_line = s.lines().next().unwrap_or("").trim();
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
    verbosity: &crate::app::Verbosity,
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
    verbosity: &crate::app::Verbosity,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let key = tool_result_cache_key(tool_name, result, width, verbosity, show_picker);

    TOOL_RESULT_CACHE.with(|cache| {
        // A hit refreshes recency, so results currently on screen are never the
        // eviction victim.
        if let Some(lines) = cache.borrow_mut().get(&key) {
            return lines.clone();
        }
        let lines = render_tool_result(tool_name, result, width, verbosity, show_picker)
            .iter()
            .map(own_line)
            .collect::<Vec<_>>();
        cache.borrow_mut().insert(key, lines.clone());
        lines
    })
}

pub(super) fn tool_result_is_hidden(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "set_goal" | "todo_write" | "complete_task" | "ask_question"
    )
}

pub(super) fn tool_result_action(
    state: &RenderSnapshot,
    message_index: usize,
    tool_name: &str,
) -> (String, String) {
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
    verbosity: &crate::app::Verbosity,
    width: u16,
) -> Vec<Line<'static>> {
    if matches!(verbosity, crate::app::Verbosity::High) {
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
    let max_visible = 6;
    let omitted = filtered.len().saturating_sub(max_visible);
    let head_count = max_visible / 2;
    let tail_count = max_visible - head_count;
    let visible = if omitted == 0 {
        filtered
    } else {
        filtered[..head_count]
            .iter()
            .chain(&filtered[filtered.len() - tail_count..])
            .cloned()
            .collect()
    };
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
    if omitted > 0 {
        indented.insert(
            head_count,
            Line::from(Span::styled(
                format!("    … +{omitted} lines"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, false),
            )),
        );
    }
    indented
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolTranscriptKind {
    Explored,
    Command,
    Edit,
    Tool,
}

pub(super) fn tool_transcript_kind(tool_name: &str) -> ToolTranscriptKind {
    if crate::app::activity::is_exploration_tool(tool_name) {
        ToolTranscriptKind::Explored
    } else if tool_name == "run_command" || tool_name.eq_ignore_ascii_case("bash") {
        ToolTranscriptKind::Command
    } else if crate::app::activity::is_editing_tool(tool_name) {
        ToolTranscriptKind::Edit
    } else {
        ToolTranscriptKind::Tool
    }
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
    message_index: usize,
    tool_name: String,
    action: String,
    target: String,
    success: bool,
    status: String,
    body: Vec<Line<'static>>,
    kind: ToolTranscriptKind,
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
        let calls = crate::tools::resolve_tool_calls(assistant, state.active_tool_protocol());
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
    let body = cached_tool_result(
        &tool_name,
        result,
        width as usize,
        &state.verbosity(),
        show_picker,
    );

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
    if show_hint {
        spans.push(Span::styled(
            " (ctrl+o to expand)",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
        ));
    }
    let mut lines = Vec::new();
    let continuation = Span::styled(
        "    ",
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
    );
    push_wrapped_with_continuation(
        &mut lines,
        spans,
        (width as usize).max(10),
        Some(continuation),
    );
    lines
}

pub(super) fn command_child_lines(
    entry: &ToolTranscriptEntry,
    first: bool,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let mut commands = highlight_shell_command(&entry.target, COLOR_BG(), show_picker);
    if commands.is_empty() {
        commands.push(Line::default());
    }
    let mut lines = Vec::with_capacity(commands.len());
    let max_w = (width as usize).max(10);
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
    if !entry.success {
        if let Some(line) = lines.last_mut() {
            line.spans.push(Span::styled(
                format!(" · {}", entry.status),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
    }
    lines
}

pub(super) fn command_summary_lines(
    entry: &ToolTranscriptEntry,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    let bullet_color = if entry.success {
        COLOR_GREEN()
    } else {
        Color::Rgb(229, 123, 123)
    };
    let has_command = !entry.target.is_empty() && entry.target != "?";
    let mut commands = highlight_shell_command(&entry.target, COLOR_BG(), show_picker);
    if commands.is_empty() {
        commands.push(Line::default());
    }
    let last = commands.len().saturating_sub(1);
    let max_w = (width as usize).max(10);
    let mut lines = Vec::new();
    for (index, command) in commands.into_iter().enumerate() {
        let mut spans = if index == 0 {
            vec![
                Span::styled(
                    "• ",
                    get_themed_style(bullet_color, COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    if has_command { "Ran $ " } else { "Ran Bash" },
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
            ]
        } else {
            vec![Span::styled(
                "    ",
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            )]
        };
        if has_command {
            spans.extend(command.spans);
        }
        if index == last {
            spans.push(Span::styled(
                format!(" · {}", entry.status),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
        let continuation = Span::styled(
            "    ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        );
        push_wrapped_with_continuation(&mut lines, spans, max_w, Some(continuation));
    }
    lines
}

pub(super) fn indent_generic_tool_body(
    lines: Vec<Line<'static>>,
    verbosity: &crate::app::Verbosity,
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    if matches!(verbosity, crate::app::Verbosity::High) {
        return Vec::new();
    }

    let max_visible = 6;
    let omitted = lines.len().saturating_sub(max_visible);
    let head_count = max_visible / 2;
    let tail_count = max_visible - head_count;
    let visible = if omitted == 0 {
        lines
    } else {
        lines[..head_count]
            .iter()
            .chain(&lines[lines.len() - tail_count..])
            .cloned()
            .collect()
    };
    let max_w = (width as usize).max(10);
    let mut indented = Vec::new();
    for line in visible {
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
    if omitted > 0 {
        indented.insert(
            head_count,
            Line::from(Span::styled(
                format!("    … +{omitted} lines"),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
            )),
        );
    }
    indented
}

pub(crate) fn render_committed_tool_result_group_snapshot(
    state: &RenderSnapshot,
    message_indices: &[usize],
    width: u16,
    show_picker: bool,
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
            && matches!(state.verbosity(), crate::app::Verbosity::Low)
        {
            index + 1
        } else {
            entries.len()
        };
        let group = &entries[index..group_end];
        let success = group.iter().all(|entry| entry.success);

        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        if homogeneous && kind == ToolTranscriptKind::Command {
            if matches!(state.verbosity(), crate::app::Verbosity::High) {
                lines.push(tool_group_header("Ran", success, show_picker));
                for (child_index, entry) in group.iter().enumerate() {
                    lines.extend(command_child_lines(
                        entry,
                        child_index == 0,
                        width,
                        show_picker,
                    ));
                }
            } else {
                let entry = &group[0];
                lines.extend(command_summary_lines(entry, width, show_picker));
                lines.extend(indent_tool_result_body(
                    entry.body.clone(),
                    &entry.tool_name,
                    &state.verbosity(),
                    width,
                ));
            }
        } else {
            let title = if !homogeneous {
                "Ran"
            } else if kind == ToolTranscriptKind::Explored {
                "Explored"
            } else if kind == ToolTranscriptKind::Edit {
                "Edited"
            } else if kind == ToolTranscriptKind::Tool {
                "Ran"
            } else {
                "Called"
            };
            lines.push(tool_group_header(title, success, show_picker));
            let mut seen = std::collections::HashSet::new();
            let mut first_child = true;
            for entry in group {
                let identity = format!("{}\0{}", entry.action, entry.target);
                if entry.kind != ToolTranscriptKind::Explored || seen.insert(identity) {
                    let is_expanded = state.expanded_thoughts().contains(&entry.message_index);
                    let show_hint = entry.kind == ToolTranscriptKind::Tool
                        && !entry.body.is_empty()
                        && !is_expanded
                        && matches!(state.verbosity(), crate::app::Verbosity::Low);
                    if entry.kind == ToolTranscriptKind::Command {
                        lines.extend(command_child_lines(entry, first_child, width, show_picker));
                    } else {
                        lines.extend(tool_child_line(
                            entry,
                            first_child,
                            show_hint,
                            width,
                            show_picker,
                        ));
                    }
                    first_child = false;
                    if entry.kind == ToolTranscriptKind::Tool
                        && is_expanded
                        && matches!(state.verbosity(), crate::app::Verbosity::Low)
                    {
                        lines.extend(indent_generic_tool_body(
                            entry.body.clone(),
                            &state.verbosity(),
                            width,
                            show_picker,
                        ));
                    }
                }
            }
        }

        index = entries.len();
    }
    lines
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
    let label_style = get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
    lines.push(Line::from(vec![
        Span::styled("─".repeat(left), line_style),
        Span::styled(label, label_style),
        Span::styled("─".repeat(right), line_style),
    ]));
}

pub(super) fn push_new_chat_separator<'a>(
    lines: &mut Vec<Line<'a>>,
    width: u16,
    show_picker: bool,
) {
    push_centered_separator(lines, "✨ NEW CHAT", width, show_picker);
    lines.push(Line::from(""));
}

pub(super) fn is_hidden_system_notice(content: &str) -> bool {
    content.contains("Loop warning:")
        || content.contains("tool calls in that response were dropped")
        || content.contains("Oversized response:")
        || content.starts_with(crate::network::compaction::SUMMARY_MARKER)
        || content.starts_with("[harness: stopped after ")
        || (content.starts_with("[harness: turn stopped — ") && !is_turn_cancelled_notice(content))
        || content.contains("Your reasoning became repetitive")
        || content.contains("reasoning loop")
}

pub(super) fn tool_result_follows(history: &[ChatMessage], assistant_index: usize) -> bool {
    next_visible_message(history, assistant_index).is_some_and(|message| message.role == "tool")
}

pub(super) fn next_visible_message(history: &[ChatMessage], index: usize) -> Option<&ChatMessage> {
    history.iter().skip(index + 1).find(|message| {
        !((message.role == "system" || message.role == "assistant")
            && is_hidden_system_notice(&message.content))
    })
}

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
