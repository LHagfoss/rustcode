use super::*;

/// Exact `cl100k_base` token count for `text`. Single source of truth for token
/// sizing, shared with compaction.
///
/// Synchronous and in-process — no `fm token-count` subprocess — but not free:
/// the first count of a given string runs a real BPE encode, which is roughly
/// linear in the text's length. Repeat counts of unchanged content are served
/// from the memo inside [`compaction::estimate_tokens`], so calling this once
/// per message per turn is cheap; calling it in a hot loop over freshly built
/// strings is not.
pub(crate) fn count_tokens(text: &str) -> u32 {
    compaction::estimate_tokens(text) as u32
}

/// Parse an SSE line, extracting the data payload.
/// Returns `None` for `[DONE]`, empty data, or non-`data: ` lines.
pub(crate) fn parse_sse_line(line: &str) -> Option<&str> {
    if let Some(s) = line.strip_prefix("data: ") {
        if s == "[DONE]" || s.is_empty() {
            return None;
        }
        Some(s)
    } else {
        None
    }
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
        "run_command" | "grep" | "glob" | "list_directory" | "get_time" | "find_symbol"
        | "get_project_map" | "search_web" => "throwaway",
        "view_file" => "file",
        _ => "other",
    })
}
