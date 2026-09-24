use crate::app::ChatMessage;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use tiktoken_rs::{CoreBPE, cl100k_base};

static BPE: OnceLock<CoreBPE> = OnceLock::new();

/// Number of distinct message contents kept in the live half of the token memo.
/// Two halves are retained (see [`TokenMemo`]), so the real ceiling is twice
/// this. Sized to comfortably hold a long session's history so a compaction
/// pass never evicts an entry it is about to look up again.
pub(super) const TOKEN_MEMO_CAPACITY: usize = 4096;

/// Bounded memo mapping message content to its BPE token count.
///
/// Compaction walks the whole history several times per turn, and the history
/// itself barely changes between turns, so the same strings would otherwise be
/// re-encoded over and over. Keying on the content means a message that *is*
/// rewritten (by pruning) simply misses and gets encoded once under its new
/// value — counts stay exactly what a direct encode would produce.
///
/// Eviction is generational rather than LRU: entries land in `live`, and when
/// `live` fills it becomes `prev` and a fresh `live` is started. A hit in `prev`
/// is promoted back into `live`, so anything still in use survives rotations
/// while genuinely dead entries fall out after two of them. That keeps the
/// memory bounded without the bookkeeping of a true LRU.
#[derive(Default)]
pub(super) struct TokenMemo {
    pub(super) live: HashMap<(usize, u64), usize>,
    pub(super) prev: HashMap<(usize, u64), usize>,
}

pub(super) static TOKEN_MEMO: OnceLock<Mutex<TokenMemo>> = OnceLock::new();

/// Key on (byte length, hash) so a 64-bit hash collision between two strings of
/// different lengths cannot hand back the wrong count.
pub(super) fn memo_key(text: &str) -> (usize, u64) {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    (text.len(), hasher.finish())
}

/// Exact token count for `text` under `cl100k_base`.
///
/// Uses `encode_ordinary`: message content is data (file contents, command
/// output, user prose), never a control channel, so literal `<|endoftext|>`-style
/// markers appearing inside it must be counted as the ordinary text they are
/// rather than collapsed into a single special token. It is also the cheaper of
/// the two encoders, since it skips the special-token scan.
///
/// Results are memoized (see [`TokenMemo`]); the first call for a given string
/// pays a full BPE encode, repeats are a hash and a map lookup.
pub fn estimate_tokens(text: &str) -> usize {
    let key = memo_key(text);
    let memo = TOKEN_MEMO.get_or_init(|| Mutex::new(TokenMemo::default()));

    {
        // A poisoned lock only means some other caller panicked mid-count; the
        // map is still a valid cache, so recover rather than propagate.
        let mut guard = memo.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&count) = guard.live.get(&key) {
            return count;
        }
        if let Some(count) = guard.prev.remove(&key) {
            guard.live.insert(key, count);
            return count;
        }
    }

    // Encode outside the lock: this is the expensive part and there is no need
    // to serialize concurrent counts of different messages behind it.
    let bpe = BPE.get_or_init(|| cl100k_base().unwrap());
    let count = bpe.encode_ordinary(text).len();

    let mut guard = memo.lock().unwrap_or_else(|e| e.into_inner());
    if guard.live.len() >= TOKEN_MEMO_CAPACITY {
        guard.prev = std::mem::take(&mut guard.live);
    }
    guard.live.insert(key, count);
    count
}

/// Estimate the provider-visible cost of the native tool schema payload.
///
/// Text protocols carry their tool definitions in the system prompt, so callers
/// must pass an empty slice for those requests. Keeping this calculation next to
/// the message estimator gives preflight and request telemetry one accounting
/// rule.
pub fn estimate_tool_schema_tokens(tool_schemas: &[serde_json::Value]) -> usize {
    if tool_schemas.is_empty() {
        0
    } else {
        serde_json::to_string(tool_schemas)
            .map(|serialized| estimate_tokens(&serialized))
            .unwrap_or_default()
    }
}

/// Estimate the provider-visible cost of a persisted chat message. Native
/// tool calls are stored outside `content`, so counting only the prose would
/// let large function arguments bypass the history budget.
pub(crate) fn estimate_message_tokens(message: &ChatMessage) -> usize {
    let tool_calls = if message.tool_calls.is_empty() {
        0
    } else {
        serde_json::to_string(&message.tool_calls)
            .map(|calls| estimate_tokens(&calls))
            .unwrap_or_default()
    };
    let tool_call_id = message
        .tool_call_id
        .as_deref()
        .map(estimate_tokens)
        .unwrap_or_default();
    estimate_tokens(&message.content)
        .saturating_add(tool_calls)
        .saturating_add(tool_call_id)
}
