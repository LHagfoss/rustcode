use super::tokens::{estimate_message_tokens, memo_key};
use crate::app::ChatMessage;
use std::collections::HashSet;

pub const DEFAULT_PRUNE_TOKEN_THRESHOLD: usize = 90_000;

/// Tokens reclaimed by last compaction, for metrics logging.
pub static LAST_COMPACTION_RECLAIMED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Number of most-recent messages whose tool outputs are always kept verbatim.
/// Older tool outputs are eligible for message-count-based pruning and, on
/// structured compaction, everything before this suffix is folded into a summary.
pub const KEEP_RECENT_TURNS: usize = 12;

/// Hard byte ceiling for the complete user prompt sent to the summarizer.
/// 64 KiB is deliberately conservative: it leaves ample room for the pinned
/// task, a prior summary, and several recent messages without allowing history
/// length to grow the request without bound.
const PRUNE_TOKEN_THRESHOLD: usize = 1000;

/// Message-count-based pruning of historical tool outputs.
///
/// Keeps the most recent `keep_recent_count` messages fully intact for accuracy.
/// For older messages, any tool result larger than [`PRUNE_TOKEN_THRESHOLD`] is
/// replaced with a one-line summary that preserves the `tool_name:` prefix — so
/// the tool call / result pairing and schema validity stay intact — along with
/// the original token count and, when detectable, the command's exit status.
pub fn prune_historical_tool_outputs(
    history: &mut [ChatMessage],
    keep_recent_count: usize,
) -> usize {
    let len = history.len();
    if len <= keep_recent_count {
        return 0;
    }
    let cutoff = len - keep_recent_count;
    let mut pruned = 0;
    for m in history[..cutoff].iter_mut() {
        if m.role != "tool" {
            continue;
        }
        // Skip anything already collapsed by a prior pass.
        if m.content.contains("[Tool Output Truncated")
            || m.content.contains("content cleared to save context")
        {
            continue;
        }
        let tokens = estimate_message_tokens(m);
        if tokens <= PRUNE_TOKEN_THRESHOLD {
            continue;
        }
        // Preserve the "tool_name: " prefix so the call/result pairing survives.
        let prefix = match m.content.find(": ") {
            Some(pos) => m.content[..pos + 2].to_string(),
            None => String::new(),
        };
        let status = detect_exit_status(&m.content);
        m.content =
            format!("{prefix}[Tool Output Truncated: {tokens} tokens reduced to summary.{status}]");
        pruned += 1;
    }
    pruned
}

/// Best-effort extraction of a command exit code from raw tool output, so the
/// pruned summary can still report whether the command succeeded.
fn detect_exit_status(content: &str) -> String {
    if let Some(idx) = content.find("exit code") {
        let code: String = content[idx..]
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !code.is_empty() {
            return format!(" Command exited with code {code}.");
        }
    }
    String::new()
}

pub fn prune_old_tool_outputs(history: &mut [ChatMessage], threshold: usize) -> usize {
    let mut total_tool_tokens = 0;
    let mut pruned = 0;
    // Walk backward through history
    for m in history.iter_mut().rev() {
        if m.role == "tool" && !is_stubbed_tool_output(&m.content) {
            let tokens = estimate_message_tokens(m);
            total_tool_tokens += tokens;
            // Protect the last ~90k tokens of tool outputs (approx 360k chars).
            // Prune older ones to save context window space. Sized for the 128k
            // main model's ~108k budget so a whole large source file (e.g. a
            // 32k-token network.rs) stays fully in context instead of being
            // wiped mid-read — the amnesia that made the agent re-read forever.
            // NOTE: still a fixed cap; if you run a small-context model as the
            // main model, lower this to fit its window.
            if total_tool_tokens > threshold {
                let valuable = has_failure_or_diagnostic(&m.content);
                if let Some(pos) = m.content.find(": ") {
                    let tool_name = &m.content[..pos];
                    m.content = if valuable {
                        format!(
                            "{}: [Old tool result retained as compact failure/diagnostic evidence; output cleared to save context]",
                            tool_name
                        )
                    } else {
                        format!(
                            "{}: [Old tool result content cleared to save context]",
                            tool_name
                        )
                    };
                } else {
                    m.content = if valuable {
                        "[Old tool result retained as compact failure/diagnostic evidence; output cleared to save context]".to_string()
                    } else {
                        "[Old tool result content cleared to save context]".to_string()
                    };
                }
                pruned += 1;
            }
        }
    }
    pruned
}

fn is_stubbed_tool_output(content: &str) -> bool {
    content.contains("[Tool output truncated")
        || content.contains("[Tool Output Truncated")
        || content.contains("content cleared to save context")
        || content.contains("output cleared to save context")
        || content.contains("[Duplicate unchanged file read omitted")
        || content.contains("[superseded")
}

/// Strip `<think>...</think>` blocks from older assistant messages.
///
/// Historical reasoning scratchpads in completed turns dominate context growth
/// without providing continuity value once the answer or tool call is finalized.
pub fn prune_historical_reasoning(history: &mut [ChatMessage], keep_recent_turns: usize) -> usize {
    let mut pruned = 0;
    let cutoff = history.len().saturating_sub(keep_recent_turns);
    for message in &mut history[..cutoff] {
        if message.role == "assistant" && message.content.contains("<think>") {
            let stripped = crate::network::text::strip_think_blocks(&message.content);
            let trimmed = stripped.trim();
            let new_content = if trimmed.is_empty() {
                "(completed reasoning)".to_string()
            } else {
                trimmed.to_string()
            };
            if new_content != message.content {
                message.content = new_content;
                pruned += 1;
            }
        }
    }
    pruned
}

fn has_failure_or_diagnostic(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("compiler errors")
        || lower.contains("lsp/compiler")
        || lower.contains("error:")
        || lower.contains("exit code: 1")
        || lower.contains("exit code: 2")
        || lower.contains("exit code: 3")
        || lower.contains("exit code: 4")
        || lower.contains("exit code: 5")
        || lower.contains("exit code: 6")
        || lower.contains("exit code: 7")
        || lower.contains("exit code: 8")
        || lower.contains("exit code: 9")
}

fn file_read_key(content: &str) -> Option<u64> {
    let (name, body) = content.split_once(": ")?;
    if !matches!(name, "view_file" | "read_file") {
        return None;
    }
    // A normal view_file result starts with a path/range header. Require that
    // identity before deduplicating: identical contents from two different
    // files, a failed read, a replay notice, or a truncated read must never be
    // collapsed merely because their rendered bodies happen to match.
    let header = body.lines().next()?;
    if !header.starts_with("[File: ")
        || body.contains("[Truncated:")
        || body.starts_with("[Unchanged since")
    {
        return None;
    }
    Some(memo_key(&format!("{name}\0{header}\0{body}")).1)
}

/// Collapse exact duplicate file reads outside the recent suffix. A newer
/// identical read is authoritative for the current workspace; keeping every
/// copy only encourages small-context models to attend to stale repetitions.
/// Reads with different content remain intact, as do errors and recent raw
/// context.
pub fn prune_duplicate_tool_results(
    history: &mut [ChatMessage],
    keep_recent_count: usize,
) -> usize {
    let cutoff = history.len().saturating_sub(keep_recent_count);
    let mut seen = HashSet::new();
    let mut pruned = 0;
    for index in (0..history.len()).rev() {
        let Some(key) = file_read_key(&history[index].content) else {
            continue;
        };
        if !seen.insert(key) && index < cutoff {
            let prefix = history[index]
                .content
                .split_once(": ")
                .map(|(name, _)| format!("{name}: "))
                .unwrap_or_default();
            history[index].content = format!(
                "{prefix}[Duplicate unchanged file read omitted; the newer identical read is retained.]"
            );
            pruned += 1;
        }
    }
    pruned
}

/// Share of the budget that must be in use before old tool output is collapsed.
///
/// Below this the window has room to spare, and keeping what the model actually
/// read is worth more than the tokens reclaimed.
const PRUNE_PRESSURE_RATIO: f64 = 0.5;

/// Token count at which pruning starts for a given budget.
pub(super) fn prune_floor(budget: usize) -> usize {
    (budget as f64 * PRUNE_PRESSURE_RATIO) as usize
}
