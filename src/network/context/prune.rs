use crate::app::ChatMessage;

pub const DEFAULT_PRUNE_TOKEN_THRESHOLD: usize = 90_000;

/// Tokens reclaimed by last compaction, for metrics logging.
pub static LAST_COMPACTION_RECLAIMED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Number of most-recent messages whose tool outputs are always kept verbatim.
/// Older tool outputs are eligible for message-count-based pruning and, on
/// structured compaction, everything before this suffix is folded into a summary.
pub const KEEP_RECENT_TURNS: usize = 12;

/// Immutable-history contract (#985): stored conversation messages are
/// append-only and never rewritten in place.
///
/// Rewriting retained messages (collapsing tool outputs to excerpts,
/// stripping reasoning blocks, replacing duplicates with placeholders)
/// changes the bytes of the prompt prefix on every turn, defeating KV-cache
/// reuse and risking silent evidence loss. Under context pressure, relief
/// comes from FIFO head compaction (a fixed summary/record replaces the head
/// while the retained tail stays byte-identical) and from request-time
/// trimming of the rendered payload — never from editing history.
///
/// The functions below therefore observe but do not mutate: they return 0
/// because no stored message was changed. Deduplication now happens as pure
/// selection at request-render time (see `history::to_messages`), and
/// `<think>` blocks are likewise stripped only in the rendered request.
pub fn prune_historical_tool_outputs(
    history: &[ChatMessage],
    keep_recent_count: usize,
) -> usize {
    let _ = (history, keep_recent_count);
    0
}

pub fn prune_old_tool_outputs(history: &[ChatMessage], threshold: usize) -> usize {
    let _ = (history, threshold);
    0
}

/// Historical reasoning scratchpads are left verbatim in storage. The
/// provider request already excludes them at render time
/// (`history::to_messages` strips `<think>` blocks into a non-mutating view),
/// so rewriting storage would only churn the persisted prefix for no
/// request-side gain.
pub fn prune_historical_reasoning(history: &[ChatMessage], keep_recent_turns: usize) -> usize {
    let _ = (history, keep_recent_turns);
    0
}

/// Duplicate file reads are no longer collapsed by rewriting the older copy
/// in storage. The rendered request excludes the redundant older read while
/// retaining the newer identical read verbatim
/// (`history::redundant_tool_result_indices`), so stored history stays
/// byte-identical across turns.
pub fn prune_duplicate_tool_results(
    history: &[ChatMessage],
    keep_recent_count: usize,
) -> usize {
    let _ = (history, keep_recent_count);
    0
}

/// Share of the budget that must be in use before old tool output is collapsed.
///
/// Below this the window has room to spare, and keeping what the model actually
/// read is worth more than the tokens reclaimed.
const PRUNE_PRESSURE_RATIO: f64 = 0.8;

/// Token count at which pruning starts for a given budget.
pub(super) fn prune_floor(budget: usize) -> usize {
    (budget as f64 * PRUNE_PRESSURE_RATIO) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn large_tool_output() -> ChatMessage {
        ChatMessage::new("tool", format!("run_command: {}", "x ".repeat(3000)))
    }

    fn serialized(history: &[ChatMessage]) -> String {
        serde_json::to_string(history).unwrap()
    }

    // #985: pruning passes must never rewrite stored messages. Whatever they
    // report, the serialized history before and after must be identical.
    #[test]
    fn historical_tool_outputs_are_never_rewritten() {
        let mut history = vec![large_tool_output()];
        for i in 0..(KEEP_RECENT_TURNS + 2) {
            history.push(ChatMessage::new("user", format!("m{i}")));
        }
        history.push(large_tool_output());
        let before = serialized(&history);

        assert_eq!(prune_historical_tool_outputs(&history, KEEP_RECENT_TURNS), 0);
        assert_eq!(serialized(&history), before);
        assert!(history[0].content.starts_with("run_command: x x"));
    }

    #[test]
    fn old_tool_outputs_are_never_rewritten() {
        let mut history = vec![large_tool_output()];
        for i in 0..(KEEP_RECENT_TURNS + 2) {
            history.push(ChatMessage::new("user", format!("m{i}")));
        }
        let before = serialized(&history);

        assert_eq!(prune_old_tool_outputs(&history, 1), 0);
        assert_eq!(serialized(&history), before);
    }

    #[test]
    fn historical_reasoning_is_never_rewritten() {
        let mut history = vec![ChatMessage::new(
            "assistant",
            "<think>private scratchpad</think>final answer",
        )];
        for i in 0..(KEEP_RECENT_TURNS + 2) {
            history.push(ChatMessage::new("user", format!("m{i}")));
        }
        let before = serialized(&history);

        assert_eq!(prune_historical_reasoning(&history, KEEP_RECENT_TURNS), 0);
        assert_eq!(serialized(&history), before);
        assert!(history[0].content.contains("<think>"));
    }

    #[test]
    fn duplicate_reads_are_never_rewritten() {
        let same = "view_file: [File: src/lib.rs]\n1: old";
        let history = vec![
            ChatMessage::new("tool", same),
            ChatMessage::new("assistant", "edit"),
            ChatMessage::new("tool", same),
            ChatMessage::new("user", "verify"),
        ];
        let before = serialized(&history);

        assert_eq!(prune_duplicate_tool_results(&history, 1), 0);
        assert_eq!(serialized(&history), before);
    }

    // End-to-end through every pass: repeated turns must observe identical
    // bytes for every retained message.
    #[test]
    fn repeated_turns_leave_all_retained_bytes_identical() {
        let same = "view_file: [File: src/lib.rs]\n1: old";
        let mut history = vec![
            ChatMessage::new("user", "inspect this"),
            ChatMessage::new("assistant", "<think>plan</think>reading"),
            ChatMessage::new("tool", same),
            ChatMessage::new("tool", large_tool_output().content),
        ];
        // A second turn repeats the identical read, then adds new work.
        history.push(ChatMessage::new("assistant", "re-checking"));
        history.push(ChatMessage::new("tool", same));
        history.push(ChatMessage::new("user", "next step"));
        let before = serialized(&history);

        prune_duplicate_tool_results(&history, KEEP_RECENT_TURNS);
        prune_historical_tool_outputs(&history, KEEP_RECENT_TURNS);
        prune_historical_reasoning(&history, KEEP_RECENT_TURNS);
        prune_old_tool_outputs(&history, 1);
        let messages = crate::network::history::to_messages(&history, "system");

        assert_eq!(serialized(&history), before);
        assert!(!messages.is_empty());
    }
}
