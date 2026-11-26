//! Project memory for an external-harness thread (#4373): `remember`,
//! `recall_memory` and `forget_memory` on the `rustcode` MCP server.
//!
//! The server answers them itself, against the calling thread's folder, with
//! the built-in tools' own handlers: one memory file per repository, shared
//! by every worktree and by built-in threads, with the same limits and
//! refusals. No orchestration version gates them, as no client is asked.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::tool_socket::CallAnswer;

/// The memory tools, by name, as the built-in registry has them.
pub(super) const TOOL_NAMES: [&str; 3] = ["remember", "recall_memory", "forget_memory"];

/// The most the memory line adds to an external thread's instructions.
pub(crate) const MAX_NOTE_BYTES: usize = 240;

pub(super) fn is_tool(tool: &str) -> bool {
    TOOL_NAMES.contains(&tool)
}

/// Answer a memory call for `session_id`, whose folder names the project.
pub(super) fn answer(tool: &str, args: &Value, session_id: &str) -> CallAnswer {
    let Some(root) = thread_root(session_id) else {
        return CallAnswer::failure(
            "error: this thread has no folder, so it has no project memory. Nothing was changed.",
        );
    };
    answer_in(tool, args, &root)
}

fn thread_root(session_id: &str) -> Option<PathBuf> {
    crate::checkpoints::thread_cwd(session_id).filter(|cwd| cwd.is_dir())
}

pub(super) fn answer_in(tool: &str, args: &Value, root: &Path) -> CallAnswer {
    use crate::tools::{forget_memory_in, recall_memory_in, remember_in};
    let root = Some(root);
    let result = match tool {
        "remember" => remember_in(root, args),
        "recall_memory" => recall_memory_in(root, args),
        "forget_memory" => forget_memory_in(root, args),
        _ => Err(format!("unknown tool '{tool}'")),
    };
    match result {
        Ok(content) => CallAnswer {
            success: true,
            content,
        },
        Err(message) => CallAnswer::failure(format!("error: {message}")),
    }
}

/// `note` with one line naming the memory tools, when the project in `cwd`
/// has saved facts. Fact bodies are never put there: the thread reads them
/// with `recall_memory`.
pub(crate) fn with_note(note: Option<String>, cwd: &Path) -> Option<String> {
    let Some(line) = note_line(saved_facts(cwd)) else {
        return note;
    };
    Some(match note {
        Some(note) => format!("{note}\n\n{line}"),
        None => line,
    })
}

fn saved_facts(cwd: &Path) -> usize {
    let project = crate::memory::load(Some(cwd)).map_or(0, |memory| memory.facts.len());
    let global = crate::memory::load_global().map_or(0, |memory| memory.facts.len());
    project + global
}

fn note_line(facts: usize) -> Option<String> {
    let noun = if facts == 1 { "fact" } else { "facts" };
    (facts > 0).then(|| {
        format!(
            "Project memory: {facts} saved {noun}. Call `recall_memory` on the `rustcode` MCP server before relying on a convention; `remember` and `forget_memory` change them."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_note_names_the_tools_only_when_facts_exist_and_stays_short() {
        assert_eq!(note_line(0), None);
        let line = note_line(64).unwrap();
        assert!(line.contains("64 saved facts") && line.contains("`recall_memory`"));
        assert!(line.len() <= MAX_NOTE_BYTES, "{}", line.len());
        assert!(note_line(1).unwrap().contains("1 saved fact."));
    }

    #[test]
    fn a_call_saves_recalls_and_forgets_with_the_built_in_refusals() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();

        let saved = answer_in(
            "remember",
            &json!({"key": "runner", "value": "Use bun for tests", "category": "build"}),
            root,
        );
        assert!(saved.success, "{}", saved.content);
        assert!(saved.content.contains("Remembered for this project"));

        let found = answer_in("recall_memory", &json!({"query": "bun"}), root);
        assert!(found.success && found.content.contains("runner: Use bun for tests"));
        assert!(with_note(None, root).unwrap().contains("1 saved fact"));

        let too_long = answer_in(
            "remember",
            &json!({"key": "big", "value": "x".repeat(800)}),
            root,
        );
        let built_in =
            crate::tools::remember_in(Some(root), &json!({"key": "big", "value": "x".repeat(800)}));
        assert!(!too_long.success);
        assert_eq!(
            too_long.content,
            format!("error: {}", built_in.unwrap_err())
        );

        let missing = answer_in("remember", &json!({"value": "v"}), root);
        assert_eq!(missing.content, "error: missing required argument 'key'");

        let forgot = answer_in("forget_memory", &json!({"key": "runner"}), root);
        assert!(forgot.content.contains("Removed 1 fact"));
        let gone = answer_in("recall_memory", &json!({"query": "bun"}), root);
        assert!(gone.content.starts_with("No remembered facts"));
    }

    #[test]
    fn a_thread_without_a_folder_is_refused() {
        let answer = answer(
            "remember",
            &json!({"key": "k", "value": "v"}),
            "no-such-thread",
        );
        assert!(!answer.success);
        assert!(answer.content.contains("no project memory"));
    }
}
