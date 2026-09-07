use super::history;
use super::view_file_unchanged_since_last_read;

/// Request-local progress that should stay visible while the durable history
/// grows or is compacted. The strings are intentionally short and owned only
/// for the duration of request assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextCheckpoint {
    pub(crate) objective: Option<String>,
    pub(crate) edit_status: &'static str,
    pub(crate) next_action: &'static str,
}

impl Default for ContextCheckpoint {
    fn default() -> Self {
        Self {
            objective: None,
            edit_status: "no edits made yet",
            next_action: "inspect the relevant files, then make the smallest scoped change",
        }
    }
}

#[allow(unused_assignments)]
pub(crate) fn build_volatile_context_block(
    token_usage: Option<&crate::app::TokenUsage>,
    quota_remaining: Option<f32>,
    context_window: u32,
) -> String {
    let now = chrono::Local::now();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "(unknown)".to_string());

    // Working directory and platform are stable workspace identity and are
    // rendered by the environment fragment. Keeping them out of this block
    // avoids paying for the same values again on the first request while the
    // date/time and accounting fields below remain turn-varying.
    let mut b = String::from("# Runtime Context (volatile — do not rely on this being cached)\n");
    b.push_str(&format!(
        "- Current date/time: {}\n",
        now.format("%A %Y-%m-%d %H:%M:%S %Z")
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

pub(crate) fn prepend_skill_routing_hint(context: &mut String, hint: Option<&str>) {
    let Some(hint) = hint.filter(|hint| !hint.is_empty()) else {
        return;
    };
    if context.is_empty() {
        context.push_str(hint);
        return;
    }

    let mut prefixed = String::with_capacity(hint.len() + context.len() + 2);
    prefixed.push_str(hint);
    prefixed.push_str("\n\n");
    prefixed.push_str(context);
    *context = prefixed;
}

pub(crate) fn build_dynamic_context_tail_with_memory(
    context_section: String,
    read_files: &[String],
    todos: &[crate::app::TodoItem],
    project_memory: Option<String>,
) -> String {
    build_dynamic_context_tail_internal(
        context_section,
        read_files,
        todos,
        project_memory,
        None,
        None,
    )
}

pub(crate) fn build_dynamic_context_tail_with_checkpoint(
    context_section: String,
    read_files: &[String],
    todos: &[crate::app::TodoItem],
    project_memory: Option<String>,
    checkpoint: &ContextCheckpoint,
    objective_hint: Option<&str>,
) -> String {
    build_dynamic_context_tail_internal(
        context_section,
        read_files,
        todos,
        project_memory,
        Some(checkpoint),
        objective_hint,
    )
}

fn build_dynamic_context_tail_internal(
    context_section: String,
    read_files: &[String],
    todos: &[crate::app::TodoItem],
    project_memory: Option<String>,
    checkpoint: Option<&ContextCheckpoint>,
    objective_hint: Option<&str>,
) -> String {
    let mut fragments = vec![history::ContextFragment::new(
        "environment",
        context_section,
    )];
    if let Some(project_memory) = project_memory {
        fragments.push(history::ContextFragment::new(
            "project memory",
            project_memory,
        ));
    }

    if let Some(checkpoint) = checkpoint {
        let objective = checkpoint
            .objective
            .as_deref()
            .map(str::to_string)
            .or_else(|| objective_hint.map(compact_objective))
            .unwrap_or_else(|| "Continue the user's request".to_string());
        let files_status = if read_files.is_empty() {
            "none recorded".to_string()
        } else {
            format!("{} tracked below", read_files.len())
        };
        fragments.push(history::ContextFragment::new(
            "checkpoint",
            format!(
                "# Turn checkpoint (refresh each round)\n- Files already read: {files_status}\n- Current objective: {objective}\n- Edit status: {}\n- Next action: {}",
                checkpoint.edit_status, checkpoint.next_action
            ),
        ));
    }

    if !read_files.is_empty() || !todos.is_empty() {
        if let Some(map) = build_repo_map_fragment() {
            fragments.push(history::ContextFragment::new("repo_map", map));
        }
    }

    if !read_files.is_empty() {
        const MAX_FILES_IN_CONTEXT: usize = 30;
        let (files_to_render, truncated_count) = if read_files.len() > MAX_FILES_IN_CONTEXT {
            let mut selected = Vec::new();
            // Prioritize stale files first
            for f in read_files.iter().filter(|f| f.contains("STALE")) {
                if selected.len() < MAX_FILES_IN_CONTEXT {
                    selected.push(f.clone());
                }
            }
            // Fill remainder with most recent files
            for f in read_files.iter().rev() {
                if selected.len() >= MAX_FILES_IN_CONTEXT {
                    break;
                }
                if !selected.contains(f) {
                    selected.push(f.clone());
                }
            }
            selected.sort();
            let trunc = read_files.len() - selected.len();
            (selected, trunc)
        } else {
            (read_files.to_vec(), 0)
        };

        let mut files_body = files_to_render
            .iter()
            .map(|f| format!("- {f}"))
            .collect::<Vec<_>>()
            .join("\n");
        if truncated_count > 0 {
            files_body.push_str(&format!(
                "\n- ... ({truncated_count} more unchanged files omitted from context tail)"
            ));
        }

        fragments.push(history::ContextFragment::new(
            "files",
            format!(
                "# Files already in context (re-read files marked stale or named by compiler diagnostics before editing)\n{files_body}"
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

fn compact_objective(text: &str) -> String {
    const MAX_OBJECTIVE_CHARS: usize = 180;
    let compacted = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compacted.chars();
    let prefix = chars.by_ref().take(MAX_OBJECTIVE_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn usage(
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    ) -> crate::app::TokenUsage {
        crate::app::TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens: None,
        }
    }

    #[test]
    fn fresh_context_renders_stable_identity_once_and_date_once() {
        let stable = crate::context::environment_context_at(Path::new(env!("CARGO_MANIFEST_DIR")));
        let volatile = build_volatile_context_block(Some(&usage(10, 3, 13)), Some(91.5), 32_000);
        let fresh = format!("{stable}\n{volatile}");

        assert_eq!(fresh.matches("- Working directory:").count(), 1);
        assert_eq!(fresh.matches("- Platform:").count(), 1);
        assert_eq!(fresh.matches("- Current date/time:").count(), 1);
        assert_eq!(fresh.matches("Today's date:").count(), 0);
        assert!(!volatile.contains("- Working directory:"));
        assert!(!volatile.contains("- Platform:"));
    }

    #[test]
    fn later_context_keeps_volatile_accounting_fields_current() {
        let first_usage = usage(10, 3, 13);
        let later_usage = usage(120, 30, 150);
        let first = build_volatile_context_block(Some(&first_usage), Some(91.5), 32_000);
        let later = build_volatile_context_block(Some(&later_usage), Some(84.0), 28_000);

        assert!(first.contains("Context window: 32000 tokens"));
        assert!(first.contains("Last-turn token usage: prompt 10 / completion 3 / total 13"));
        assert!(first.contains("Model quota remaining: 91.5%"));
        assert!(later.contains("Context window: 28000 tokens"));
        assert!(later.contains("Last-turn token usage: prompt 120 / completion 30 / total 150"));
        assert!(later.contains("Model quota remaining: 84.0%"));
        assert_ne!(first, later);
        assert_eq!(later.matches("- Current date/time:").count(), 1);
    }

    #[test]
    fn skill_routing_hint_is_first_dynamic_context_fragment() {
        let mut context =
            "# Environment\nworkspace\n\n# Files already in context\n- src/lib.rs".to_string();

        prepend_skill_routing_hint(
            &mut context,
            Some("# Priority skill route\nCall use_skill with `solidtime` first."),
        );

        let hint_end = context.find("# Environment").expect("environment fragment");
        let files_start = context
            .find("# Files already in context")
            .expect("files fragment");
        assert_eq!(
            &context[..hint_end],
            "# Priority skill route\nCall use_skill with `solidtime` first.\n\n"
        );
        assert!(hint_end < files_start);
    }

    #[test]
    fn checkpoint_is_visible_and_compact() {
        let checkpoint = ContextCheckpoint {
            objective: Some("Verify the request assembly".to_string()),
            edit_status: "edits made; verification pending",
            next_action: "run the focused tests",
        };
        let rendered = build_dynamic_context_tail_with_checkpoint(
            "# Environment".to_string(),
            &["src/network.rs (snapshot current)".to_string()],
            &[],
            None,
            &checkpoint,
            Some("this fallback must not win"),
        );

        assert!(rendered.contains("# Turn checkpoint (refresh each round)"));
        assert!(rendered.contains("Files already read: 1 tracked below"));
        assert!(rendered.contains("Current objective: Verify the request assembly"));
        assert!(rendered.contains("Edit status: edits made; verification pending"));
        assert!(rendered.contains("Next action: run the focused tests"));
        assert!(!rendered.contains("this fallback must not win"));
    }

    #[test]
    fn objective_hint_is_whitespace_collapsed_and_bounded() {
        let rendered = build_dynamic_context_tail_with_checkpoint(
            String::new(),
            &[],
            &[],
            None,
            &ContextCheckpoint::default(),
            Some(&format!("  first\n{}", "x ".repeat(200))),
        );
        let objective = rendered
            .lines()
            .find(|line| line.starts_with("- Current objective: "))
            .expect("checkpoint objective");
        assert!(objective.starts_with("- Current objective: first x x"));
        assert!(objective.chars().count() <= 220);
        assert!(objective.ends_with('…'));
    }
}
