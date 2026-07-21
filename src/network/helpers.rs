use super::*;

/// Fast token estimate (~4 chars per token). Single source of truth for token
/// sizing, shared with compaction. Deliberately approximate and synchronous —
/// compaction and usage display only need a cheap size signal, not exact counts,
/// so we skip the `fm token-count` subprocess (which added latency per message).
pub(crate) fn count_tokens(text: &str) -> u32 {
    compaction::estimate_tokens(text) as u32
}

/// Classify a stored tool result for compaction priority.
/// Returns `None` for non-tool messages. Tool results are bucketed into:
/// "throwaway" (run_command, grep, glob, list_directory, get_time,
/// find_symbol, get_project_map, search_web) — pruned first; "file"
/// (view_file contents) — pruned last; and "other".
pub(crate) fn classify_tool_msg(m: &ChatMessage) -> Option<&'static str> {
    if m.role != "tool" {
        return None;
    }
    let name = m.content.split(':').next().unwrap_or("").trim();
    Some(match name {
        "run_command" | "grep" | "glob" | "list_directory" | "get_time"
        | "find_symbol" | "get_project_map" | "search_web" => "throwaway",
        "view_file" => "file",
        _ => "other",
    })
}
