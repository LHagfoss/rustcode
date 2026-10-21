pub const DEFAULT_PRUNE_TOKEN_THRESHOLD: usize = 90_000;

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
/// The in-place pass functions that used to live here (`prune_*`) were
/// removed once the contract landed: they had degenerated into no-ops that
/// still returned counts, so compaction OR-ed their zeros into its own
/// "did anything happen" result and reported prune metrics that were always
/// zero. Deduplication now happens as pure selection at request-render time
/// (see `history::redundant_tool_result_indices`), and ` thinking` blocks are
/// likewise stripped only in the rendered request
/// (`history::to_messages`). The guard test below is what keeps that true.

/// Share of the budget that must be in use before compaction considers the
/// history under pressure.
///
/// Below this the window has room to spare, and keeping what the model actually
/// read is worth more than the tokens reclaimed.
const PRUNE_PRESSURE_RATIO: f64 = 0.8;

/// Token count at which compaction starts for a given budget.
pub(super) fn prune_floor(budget: usize) -> usize {
    (budget as f64 * PRUNE_PRESSURE_RATIO) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatMessage;

    fn serialized(history: &[ChatMessage]) -> String {
        serde_json::to_string(history).unwrap()
    }

    /// #985 guard: repeated turns must observe identical bytes for every
    /// retained message, both in storage and in the rendered request prefix.
    /// This replaces the per-pass "never rewritten" tests that guarded
    /// functions which no longer exist.
    #[test]
    fn repeated_turns_leave_all_retained_bytes_identical() {
        let same = "view_file: [File: src/lib.rs]\n1: old";
        let large = format!("run_command: {}", "x ".repeat(3000));
        let mut history = vec![
            ChatMessage::new("user", "inspect this"),
            ChatMessage::new("assistant", " thinkingplan</think>reading"),
            ChatMessage::new("tool", same),
            ChatMessage::new("tool", large.clone()),
        ];
        // A second turn repeats the identical read, then adds new work.
        history.push(ChatMessage::new("assistant", "re-checking"));
        history.push(ChatMessage::new("tool", same));
        history.push(ChatMessage::new("user", "next step"));
        let before = serialized(&history);

        let first_render = crate::network::history::to_messages(&history, "system");
        let second_render = crate::network::history::to_messages(&history, "system");

        assert_eq!(serialized(&history), before);
        assert_eq!(first_render, second_render);
        // The large aged-out output and the reasoning scratchpad both survive
        // verbatim in storage.
        assert_eq!(history[3].content, large);
        assert!(history[1].content.contains("</think>"));
    }

    #[test]
    fn prune_floor_scales_with_the_budget() {
        assert_eq!(prune_floor(100_000), 80_000);
        assert_eq!(prune_floor(8_000), 6_400);
    }
}
