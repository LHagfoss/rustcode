use super::history;
use super::view_file_unchanged_since_last_read;

#[allow(unused_assignments)]
pub(crate) fn build_volatile_context_block(
    token_usage: Option<&crate::app::TokenUsage>,
    quota_remaining: Option<f32>,
    context_window: u32,
) -> String {
    let now = chrono::Local::now();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "(unknown)".to_string());

    let mut b = String::from("# Runtime Context (volatile — do not rely on this being cached)\n");
    b.push_str(&format!(
        "- Current date/time: {}\n",
        now.format("%A %Y-%m-%d %H:%M:%S %Z")
    ));
    b.push_str(&format!("- Working directory: {cwd}\n"));
    b.push_str(&format!(
        "- Platform: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    b.push_str(&format!("- Shell: {shell}\n"));
    b.push_str(&format!("- Context window: {context_window} tokens\n"));
    if let Some(u) = token_usage {
        b.push_str(&format!(
            "- Last-turn token usage: prompt {} / completion {} / total {}",
            u.prompt_tokens, u.completion_tokens, u.total_tokens
        ));
        if let Some(cached) = u.cached_tokens {
            b.push_str(&format!(" (cached {cached})"));
        }
        b.push('\n');
    }
    if let Some(q) = quota_remaining {
        b.push_str(&format!("- Model quota remaining: {q:.1}%\n"));
    }
    b
}

pub(crate) fn build_repo_map_fragment() -> Option<String> {
    let root = std::env::current_dir().ok()?;
    let mut entries: Vec<String> = std::fs::read_dir(&root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with('.') || n == "target" {
                return None;
            }
            let mut s = n;
            if e.file_type().ok()?.is_dir() {
                s.push('/');
            }
            Some(s)
        })
        .collect();
    entries.sort();
    let mut out = String::from("# Repo map (top-level)\n");
    for e in entries.iter().take(30) {
        out.push_str(&format!("- {e}\n"));
    }
    if let Ok(src) = std::fs::read_dir(root.join("src")) {
        let mut mods: Vec<String> = src
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        mods.sort();
        if !mods.is_empty() {
            out.push_str("\nsrc/\n");
            for m in mods.iter().take(20) {
                out.push_str(&format!("- {m}\n"));
            }
        }
    }
    Some(out)
}

pub(crate) fn format_read_file_context_entry(
    path: &str,
    snapshot_mtime: Option<std::time::SystemTime>,
    current_mtime: Option<std::time::SystemTime>,
) -> String {
    let status = if view_file_unchanged_since_last_read(snapshot_mtime, current_mtime) {
        "snapshot current"
    } else {
        "STALE — changed on disk; re-read before editing"
    };
    format!("{path} ({status})")
}

pub(crate) fn build_dynamic_context_tail(
    context_section: String,
    read_files: &[String],
    todos: &[crate::app::TodoItem],
) -> String {
    build_dynamic_context_tail_with_memory(context_section, read_files, todos, None)
}

pub(crate) fn build_dynamic_context_tail_with_memory(
    context_section: String,
    read_files: &[String],
    todos: &[crate::app::TodoItem],
    project_memory: Option<String>,
) -> String {
    let mut fragments = vec![history::ContextFragment::new(
        "environment",
        context_section,
    )];
    if let Some(project_memory) = project_memory {
        fragments.push(history::ContextFragment::new("project memory", project_memory));
    }
    if !read_files.is_empty() || !todos.is_empty() {
        if let Some(map) = build_repo_map_fragment() {
            fragments.push(history::ContextFragment::new("repo_map", map));
        }
    }

    if !read_files.is_empty() {
        fragments.push(history::ContextFragment::new(
            "files",
            format!(
                "# Files already in context (re-read files marked stale or named by compiler diagnostics before editing)\n{}",
                read_files
                    .iter()
                    .map(|f| format!("- {f}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ));
    }

    if !todos.is_empty() {
        let mut plan =
            String::from("# Your current task plan (execute in order; update via todo_write)\n");
        for (i, t) in todos.iter().enumerate() {
            let mark = match t.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[~]",
                _ => "[ ]",
            };
            plan.push_str(&format!(
                "{}. {} {} ({})\n",
                i + 1,
                mark,
                t.content,
                t.priority
            ));
        }
        fragments.push(history::ContextFragment::new("task plan", plan));
    }

    history::render_context_fragments(&fragments)
}
