use crate::app::ChatMessage;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tiktoken_rs::{CoreBPE, cl100k_base};

static BPE: OnceLock<CoreBPE> = OnceLock::new();

/// Number of distinct message contents kept in the live half of the token memo.
/// Two halves are retained (see [`TokenMemo`]), so the real ceiling is twice
/// this. Sized to comfortably hold a long session's history so a compaction
/// pass never evicts an entry it is about to look up again.
const TOKEN_MEMO_CAPACITY: usize = 4096;

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
struct TokenMemo {
    live: HashMap<(usize, u64), usize>,
    prev: HashMap<(usize, u64), usize>,
}

static TOKEN_MEMO: OnceLock<Mutex<TokenMemo>> = OnceLock::new();

/// Key on (byte length, hash) so a 64-bit hash collision between two strings of
/// different lengths cannot hand back the wrong count.
fn memo_key(text: &str) -> (usize, u64) {
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreflightBudget {
    pub system_tokens: usize,
    pub tool_schema_tokens: usize,
    pub history_tokens: usize,
    pub dynamic_tail_tokens: usize,
    pub continuation_overhead_tokens: usize,
    pub provider_margin: usize,
    pub total_estimated_prompt: usize,
    pub completion_reserve: usize,
    pub soft_context_target: usize,
    pub hard_effective_limit: usize,
    pub context_window: usize,
}

impl PreflightBudget {
    pub fn fits_hard_limit(&self) -> bool {
        self.total_estimated_prompt
            .saturating_add(self.completion_reserve)
            <= self.hard_effective_limit
    }

    pub fn fits_soft_target(&self) -> bool {
        self.total_estimated_prompt <= self.soft_context_target
    }
}

/// Calculate the comprehensive preflight budget before sending a request to the provider.
pub fn calculate_preflight_budget(
    system_prompt: &str,
    tool_schemas: &[serde_json::Value],
    history: &[ChatMessage],
    dynamic_context_tail: &str,
    continuation_overhead: usize,
    budget: &crate::config::ContextBudget,
) -> PreflightBudget {
    let system_tokens = estimate_tokens(system_prompt);
    let tool_schema_tokens = estimate_tool_schema_tokens(tool_schemas);
    let history_tokens: usize = history.iter().map(estimate_message_tokens).sum();
    let dynamic_tail_tokens = estimate_tokens(dynamic_context_tail);
    let provider_margin = budget.provider_overhead_margin as usize;
    let total_estimated_prompt = system_tokens
        .saturating_add(tool_schema_tokens)
        .saturating_add(history_tokens)
        .saturating_add(dynamic_tail_tokens)
        .saturating_add(continuation_overhead)
        .saturating_add(provider_margin);

    PreflightBudget {
        system_tokens,
        tool_schema_tokens,
        history_tokens,
        dynamic_tail_tokens,
        continuation_overhead_tokens: continuation_overhead,
        provider_margin,
        total_estimated_prompt,
        completion_reserve: budget.completion_reserve as usize,
        soft_context_target: budget.soft_context_target as usize,
        hard_effective_limit: budget.hard_effective_limit as usize,
        context_window: budget.context_window as usize,
    }
}

pub const STRUCTURED_MEMORY_MARKER: &str = "[Deterministic context record]";

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StructuredSessionMemory {
    pub initial_goal: String,
    pub current_task: Option<String>,
    pub user_constraints: Vec<String>,
    pub key_architecture: Vec<String>,
    pub inspected_files: Vec<String>,
    pub modified_files: Vec<String>,
    pub decisions: Vec<String>,
    pub failures_and_errors: Vec<String>,
    pub verification_state: Vec<String>,
}

pub(crate) fn compact_context_line(content: &str, max_chars: usize) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    line.chars().take(max_chars).collect()
}

pub(crate) fn compact_context_block(content: &str, max_chars: usize) -> String {
    let block = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    block.chars().take(max_chars).collect()
}

impl StructuredSessionMemory {
    pub fn extract_from_history(history: &[ChatMessage]) -> Self {
        let mut memory = Self::default();

        for message in history {
            if message.role == "system" {
                if message.content.starts_with(STRUCTURED_MEMORY_MARKER)
                    || message.content.starts_with(SUMMARY_MARKER)
                    || message.content.starts_with("[Structured Session Memory]")
                    || message
                        .content
                        .starts_with("[Deterministic context record]")
                {
                    memory.merge_from_text(&message.content);
                } else if !message.content.starts_with('[') {
                    let block = compact_context_block(&message.content, 900);
                    if !block.is_empty() && !memory.user_constraints.contains(&block) {
                        memory.user_constraints.push(block);
                    }
                }
            }

            if message.role == "user" && !message.content.starts_with("<tool_result>") {
                if memory.initial_goal.is_empty() {
                    memory.initial_goal = compact_context_line(&message.content, 700);
                } else {
                    let task = compact_context_line(&message.content, 700);
                    if !task.is_empty() && task != memory.initial_goal {
                        memory.current_task = Some(task);
                    }
                }

                let lower = message.content.to_ascii_lowercase();
                if lower.contains("never")
                    || lower.contains("do not")
                    || lower.contains("don't")
                    || lower.contains("must")
                    || lower.contains("always")
                    || lower.contains("constraint")
                    || lower.contains("rule")
                    || lower.contains("preference")
                {
                    for line in message.content.lines() {
                        let trimmed = line.trim();
                        let l = trimmed.to_ascii_lowercase();
                        if (l.contains("never")
                            || l.contains("do not")
                            || l.contains("don't")
                            || l.contains("must")
                            || l.contains("always")
                            || l.contains("rule")
                            || l.contains("constraint")
                            || l.contains("preference"))
                            && !memory.user_constraints.iter().any(|c| c == trimmed)
                        {
                            memory.user_constraints.push(trimmed.to_string());
                        }
                    }
                }
            }

            if let Some(ref result) = message.tool_result {
                for path in &result.changed_paths {
                    if !memory.modified_files.contains(path) {
                        memory.modified_files.push(path.clone());
                    }
                }
                if !result.success || result.error_kind.is_some() {
                    let err = format!(
                        "{} ({}, exit={:?})",
                        result.tool_name,
                        result.error_kind.as_deref().unwrap_or("failed"),
                        result.exit_code
                    );
                    if !memory.failures_and_errors.contains(&err) {
                        memory.failures_and_errors.push(err);
                    }
                }
                if result.tool_name == "run_command" {
                    let v = format!(
                        "command ({}, exit={:?})",
                        if result.success { "success" } else { "failed" },
                        result.exit_code
                    );
                    if !memory.verification_state.contains(&v) {
                        memory.verification_state.push(v);
                    }
                }
            }

            if message.role == "tool" {
                if let Some((name, body)) = message.content.split_once(": ") {
                    if matches!(name, "view_file" | "read_file") {
                        if let Some(first_line) = body.lines().next() {
                            if let Some(path) = first_line
                                .strip_prefix("[File: ")
                                .and_then(|s| s.split(']').next())
                            {
                                if !memory.inspected_files.iter().any(|f| f == path) {
                                    memory.inspected_files.push(path.to_string());
                                }
                            }
                        }
                    }
                    if body.contains("error:")
                        || body.contains("exit code: 1")
                        || body.contains("FAILED")
                    {
                        let snippet = compact_context_line(body, 200);
                        if !snippet.is_empty() && !memory.failures_and_errors.contains(&snippet) {
                            memory.failures_and_errors.push(snippet);
                        }
                    }
                }
            }

            if message.role == "assistant" {
                for call in &message.tool_calls {
                    if call.name == "run_command"
                        && let Ok(arguments) =
                            serde_json::from_str::<serde_json::Value>(&call.arguments)
                        && let Some(command) =
                            arguments.get("command").and_then(|value| value.as_str())
                    {
                        let v = compact_context_line(command, 240);
                        if !v.is_empty() && !memory.verification_state.contains(&v) {
                            memory.verification_state.push(v);
                        }
                    }
                }
                let prose = super::text::strip_think_blocks(&message.content);
                let line = compact_context_line(&prose, 300);
                if !line.is_empty()
                    && !line.starts_with("```tool")
                    && !line.starts_with('{')
                    && !line.starts_with('!')
                    && !line.starts_with("• ")
                    && !memory.decisions.contains(&line)
                {
                    memory.decisions.push(line);
                }
            }
        }

        memory
    }

    pub fn merge_from_text(&mut self, text: &str) {
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(goal) = trimmed.strip_prefix("Goal: ") {
                if self.initial_goal.is_empty() {
                    self.initial_goal = goal.to_string();
                }
            } else if let Some(task) = trimmed.strip_prefix("Current follow-up: ") {
                if self.current_task.is_none() {
                    self.current_task = Some(task.to_string());
                }
            } else if let Some(constraints) =
                trimmed.strip_prefix("Project instructions/constraints: ")
            {
                for c in constraints.split("; ") {
                    if !self.user_constraints.iter().any(|existing| existing == c) {
                        self.user_constraints.push(c.to_string());
                    }
                }
            } else if let Some(files) = trimmed.strip_prefix("Modified files: ") {
                for f in files.split(", ") {
                    if !self.modified_files.iter().any(|existing| existing == f) {
                        self.modified_files.push(f.to_string());
                    }
                }
            } else if let Some(failures) = trimmed.strip_prefix("Failures/unresolved work: ") {
                for fail in failures.split("; ") {
                    if !self
                        .failures_and_errors
                        .iter()
                        .any(|existing| existing == fail)
                    {
                        self.failures_and_errors.push(fail.to_string());
                    }
                }
            } else if let Some(verifications) = trimmed.strip_prefix("Verification state: ") {
                for v in verifications.split("; ") {
                    if !self.verification_state.iter().any(|existing| existing == v) {
                        self.verification_state.push(v.to_string());
                    }
                }
            } else if let Some(arch) = trimmed.strip_prefix("Key architecture: ") {
                for a in arch.split("; ") {
                    if !self.key_architecture.iter().any(|existing| existing == a) {
                        self.key_architecture.push(a.to_string());
                    }
                }
            } else if let Some(decisions) =
                trimmed.strip_prefix("Architecture/decisions/next steps: ")
            {
                for d in decisions.split("; ") {
                    if !self.decisions.iter().any(|existing| existing == d) {
                        self.decisions.push(d.to_string());
                    }
                }
            } else if let Some(constraint) = trimmed.strip_prefix("- Constraint: ") {
                if !self.user_constraints.iter().any(|c| c == constraint) {
                    self.user_constraints.push(constraint.to_string());
                }
            } else if let Some(arch) = trimmed.strip_prefix("- Architecture: ") {
                if !self.key_architecture.iter().any(|a| a == arch) {
                    self.key_architecture.push(arch.to_string());
                }
            } else if let Some(decision) = trimmed.strip_prefix("- Decision: ") {
                if !self.decisions.iter().any(|d| d == decision) {
                    self.decisions.push(decision.to_string());
                }
            } else if let Some(failure) = trimmed.strip_prefix("- Failure: ") {
                if !self.failures_and_errors.iter().any(|f| f == failure) {
                    self.failures_and_errors.push(failure.to_string());
                }
            }
        }
    }

    pub fn format_record(&self, max_chars: usize) -> String {
        let mut out = format!("{STRUCTURED_MEMORY_MARKER}\n");
        if !self.initial_goal.is_empty() {
            out.push_str(&format!("Goal: {}\n", self.initial_goal));
        }
        if let Some(ref task) = self.current_task {
            out.push_str(&format!("Current follow-up: {}\n", task));
        }
        if !self.user_constraints.is_empty() {
            out.push_str("Project instructions/constraints: ");
            out.push_str(
                &self
                    .user_constraints
                    .iter()
                    .take(12)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            out.push('\n');
        }
        if !self.modified_files.is_empty() {
            out.push_str(&format!(
                "Modified files: {}\n",
                self.modified_files
                    .iter()
                    .take(20)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.failures_and_errors.is_empty() {
            out.push_str("Failures/unresolved work: ");
            out.push_str(
                &self
                    .failures_and_errors
                    .iter()
                    .rev()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            out.push('\n');
        }
        if !self.verification_state.is_empty() {
            out.push_str(&format!(
                "Verification state: {}\n",
                self.verification_state
                    .iter()
                    .rev()
                    .take(6)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if !self.key_architecture.is_empty() {
            out.push_str("Key architecture: ");
            out.push_str(
                &self
                    .key_architecture
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            out.push('\n');
        }
        if !self.decisions.is_empty() {
            out.push_str("Architecture/decisions/next steps: ");
            out.push_str(
                &self
                    .decisions
                    .iter()
                    .rev()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            out.push('\n');
        }
        out.chars().take(max_chars).collect()
    }
}

pub fn compact_with_structured_memory(
    history: &mut Vec<ChatMessage>,
    keep_recent_count: usize,
    budget: usize,
) -> bool {
    if history.len() <= keep_recent_count || history.len() < 4 {
        return false;
    }
    let cutoff = history.len().saturating_sub(keep_recent_count);
    if cutoff == 0 {
        return false;
    }
    let memory = StructuredSessionMemory::extract_from_history(&history[..cutoff]);
    let max_chars = budget.saturating_mul(3).clamp(1000, 8000);
    let record = memory.format_record(max_chars);

    let tail = history[cutoff..].to_vec();
    history.clear();
    history.push(ChatMessage::new("system", record));
    history.extend(tail);
    true
}

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
const SUMMARY_INPUT_MAX_BYTES: usize = 64 * 1024;

/// A prior summary is high-value context, but must leave room for the original
/// task and recent facts inside [`SUMMARY_INPUT_MAX_BYTES`].
const SUMMARY_PRIOR_MAX_BYTES: usize = 24 * 1024;

/// Provider output is requested at 1024 tokens; 16 KiB is a generous defensive
/// byte ceiling for providers that ignore that limit.
const SUMMARY_OUTPUT_MAX_BYTES: usize = 16 * 1024;

/// Total wall-clock budget for a non-streaming summary request, including
/// connection, response headers, body transfer, and JSON decoding. Twenty-five
/// seconds bounds the request while leaving ample time for valid summaries.
const SUMMARY_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

/// Preserve the existing per-message limit while additionally enforcing the
/// whole-input byte ceiling above.
const SUMMARY_MESSAGE_MAX_CHARS: usize = 2_000;

/// Tool outputs above this token size are collapsed once they age out of the
/// recent window.
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
            let stripped = super::text::strip_think_blocks(&message.content);
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
fn prune_floor(budget: usize) -> usize {
    (budget as f64 * PRUNE_PRESSURE_RATIO) as usize
}

/// Check if history needs compaction and compact if so.
/// Returns true if compaction was performed.
pub async fn maybe_compact(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    budget: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> bool {
    maybe_compact_with_local_policy(client, url, model, history, budget, cancel_token, false).await
}

/// Automatic compaction with an explicit local-model hint from the active
/// profile. Keeping the wrapper above preserves direct callers and tests while
/// allowing localhost OpenAI-compatible runtimes to avoid an unnecessary
/// summarizer prefill even when their URL is not Ollama's default port.
pub async fn maybe_compact_with_local_policy(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    budget: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
    local_model: bool,
) -> bool {
    // 1. Local, zero-cost tool output pruning: collapse large tool outputs that
    //    have aged past the recent window, then apply the hard rolling token cap
    //    as a safety net.
    //
    //    Only under real pressure. Collapsing a file the model read three
    //    round-trips ago while most of the window sits unused buys nothing and
    //    costs the model its memory of what it just looked at — it re-reads,
    //    which is both slower and how repeat-loops start. Below the threshold
    //    the history is left exactly as it happened.
    //
    //    These two passes each rewrite the messages they collapse, so their
    //    counts are deliberately not shared: a collapsed message is a different
    //    string and must be counted as such. The memo in `estimate_tokens` is
    //    what keeps the passes cheap — every message the passes leave alone is
    //    encoded once and looked up thereafter.
    let raw_tokens: usize = history.iter().map(estimate_message_tokens).sum();
    if raw_tokens < prune_floor(budget) {
        return false;
    }

    let duplicate_reads = prune_duplicate_tool_results(history, KEEP_RECENT_TURNS);
    let historical_outputs = prune_historical_tool_outputs(history, KEEP_RECENT_TURNS);
    let pruned_reasoning = prune_historical_reasoning(history, KEEP_RECENT_TURNS);
    let old_outputs = prune_old_tool_outputs(history, (budget as f64 * 0.6) as usize);

    // 2. Count the post-prune history once. `history` is not touched again
    //    until compaction actually runs, so the same per-message counts serve
    //    both the budget check and the keep-suffix walk below.
    let per_message: Vec<usize> = history.iter().map(estimate_message_tokens).collect();
    let total_tokens: usize = per_message.iter().sum();
    // Cancellation still permits the deterministic local pruning above, but
    // must never report a completed compaction or start a summarizer request.
    if cancel_token.is_cancelled() {
        return false;
    }
    if total_tokens < budget {
        emit_compaction_metrics(
            raw_tokens,
            total_tokens,
            budget,
            duplicate_reads,
            historical_outputs + old_outputs + pruned_reasoning,
            false,
            false,
            history.len(),
        );
        return duplicate_reads + historical_outputs + old_outputs + pruned_reasoning > 0;
    }

    // Determine how many messages to summarize.
    // We want to keep at least the KEEP_RECENT_TURNS most recent messages
    // verbatim, but also retain a recent suffix of up to 30% of the token
    // budget verbatim.
    let mut accumulated_tokens = 0;
    let keep_token_limit = (budget as f64 * 0.3) as usize; // Keep 30% of budget verbatim

    let mut keep_count = 0;
    for &tokens in per_message.iter().rev() {
        if accumulated_tokens + tokens <= keep_token_limit || keep_count < KEEP_RECENT_TURNS {
            accumulated_tokens += tokens;
            keep_count += 1;
        } else {
            break;
        }
    }

    let summarize_count = history.len().saturating_sub(keep_count);
    if summarize_count < 4 {
        emit_compaction_metrics(
            raw_tokens,
            total_tokens,
            budget,
            duplicate_reads,
            historical_outputs + old_outputs,
            false,
            false,
            history.len(),
        );
        return duplicate_reads + historical_outputs + old_outputs > 0;
    }
    // Tiered compaction: local ollama engines skip LLM summarization —
    // prune+trim already reclaimed tokens, and weak local summaries lose
    // fidelity while adding latency/cost. Gate strictly on ollama endpoint
    // so mock/test servers (random 127.0.0.1 ports) still exercise the
    // summary path.
    let is_local_engine = local_model || {
        let lower = url.to_ascii_lowercase();
        lower.contains("11434") || lower.contains("ollama")
    };
    if is_local_engine {
        let structured = compact_with_structured_memory(history, keep_count, budget);
        emit_compaction_metrics(
            raw_tokens,
            total_tokens,
            budget,
            duplicate_reads,
            historical_outputs + old_outputs,
            false,
            true,
            history.len(),
        );
        return structured || (duplicate_reads + historical_outputs + old_outputs > 0);
    }

    let summary_ok = force_compact_internal(
        client,
        url,
        model,
        history,
        summarize_count,
        Some(cancel_token),
    )
    .await
    .is_ok();

    if summary_ok {
        emit_compaction_metrics(
            raw_tokens,
            total_tokens,
            budget,
            duplicate_reads,
            historical_outputs + old_outputs,
            true,
            false,
            history.len(),
        );
        true
    } else {
        let structured = compact_with_structured_memory(history, keep_count, budget);
        emit_compaction_metrics(
            raw_tokens,
            total_tokens,
            budget,
            duplicate_reads,
            historical_outputs + old_outputs,
            false,
            true,
            history.len(),
        );
        structured || (duplicate_reads + historical_outputs + old_outputs > 0)
    }
}

fn emit_compaction_metrics(
    before_tokens: usize,
    after_prune_tokens: usize,
    budget: usize,
    duplicate_reads: usize,
    pruned_outputs: usize,
    summarized: bool,
    local_prune_only: bool,
    messages_considered: usize,
) {
    crate::logger::operational_event(
        "context.compaction",
        serde_json::json!({
            "before_tokens": before_tokens,
            "after_prune_tokens": after_prune_tokens,
            "reclaimed_tokens": before_tokens.saturating_sub(after_prune_tokens),
            "budget": budget,
            "messages_considered": messages_considered,
            "messages_retained_raw": messages_considered.min(KEEP_RECENT_TURNS),
            "duplicate_reads_collapsed": duplicate_reads,
            "tool_outputs_pruned": pruned_outputs,
            "summary_generated": summarized,
            "local_prune_only": local_prune_only,
            "hard_trim": false,
        }),
    );
}

pub async fn force_compact(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(usize, usize), String> {
    force_compact_with_budget(client, url, model, history, None, cancel_token).await
}

pub async fn force_compact_with_budget(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    budget: Option<usize>,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(usize, usize), String> {
    let before_tokens: usize = history.iter().map(estimate_message_tokens).sum();
    prune_duplicate_tool_results(history, KEEP_RECENT_TURNS);
    prune_historical_tool_outputs(history, KEEP_RECENT_TURNS);
    prune_historical_reasoning(history, KEEP_RECENT_TURNS);
    let prune_threshold = budget
        .map(|b| (b as f64 * 0.6) as usize)
        .unwrap_or(DEFAULT_PRUNE_TOKEN_THRESHOLD);
    prune_old_tool_outputs(history, prune_threshold);

    // Summarize all but the most recent KEEP_RECENT_TURNS messages.
    let summarize_count = history.len().saturating_sub(KEEP_RECENT_TURNS);
    if summarize_count < 1 {
        return Err("Not enough messages to compact.".to_string());
    }

    let result =
        force_compact_internal(client, url, model, history, summarize_count, cancel_token).await;
    let after_tokens: usize = history.iter().map(estimate_message_tokens).sum();
    if after_tokens < before_tokens {
        LAST_COMPACTION_RECLAIMED.store(
            before_tokens - after_tokens,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    result.map(|_| (before_tokens, after_tokens))
}

async fn force_compact_internal(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    summarize_count: usize,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(), String> {
    // Incremental compaction: if a prior summary already sits at the front of the
    // range, preserve its facts and only summarize the messages that came after.
    // Avoids re-compressing an already-compressed summary (which drifts and loses
    // detail every pass).
    let prior_summary = history
        .iter()
        .take(summarize_count)
        .find(|m| m.role == "system" && m.content.starts_with(SUMMARY_MARKER))
        .map(|m| {
            m.content
                .trim_start_matches(SUMMARY_MARKER)
                .trim_start_matches('\n')
                .to_string()
        });

    // Pin the original task (first user message, or the marker left by an
    // earlier compaction) so the goal is never blurred away.
    let first_user_task = history
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone());
    let pinned_task = history
        .iter()
        .find(|m| m.role == "system" && m.content.starts_with(ORIGINAL_TASK_MARKER))
        .map(|m| {
            m.content
                .strip_prefix(ORIGINAL_TASK_MARKER)
                .unwrap_or_default()
                .trim_start_matches('\n')
                .to_string()
        })
        .filter(|task| !task.is_empty())
        .or(first_user_task.clone());

    // Only summarize messages that aren't the prior summary itself.
    let to_summarize: Vec<&ChatMessage> = history[..summarize_count]
        .iter()
        .filter(|m| {
            !(m.role == "system" && m.content.starts_with(SUMMARY_MARKER))
                && !(m.role == "system" && m.content.starts_with(ORIGINAL_TASK_MARKER))
        })
        .collect();

    let summary = match generate_summary(
        client,
        url,
        model,
        prior_summary.as_deref(),
        &to_summarize,
        cancel_token,
    )
    .await
    {
        Some(s) => s,
        None => return Err("Failed to generate summary.".to_string()),
    };

    let tail: Vec<ChatMessage> = history[summarize_count..].to_vec();
    let task_in_tail = pinned_task
        .as_ref()
        .is_some_and(|task| tail.iter().any(|m| m.role == "user" && &m.content == task));
    let marker_in_tail = pinned_task.as_ref().is_some_and(|task| {
        tail.iter()
            .any(|m| m.role == "system" && m.content == format!("{ORIGINAL_TASK_MARKER}\n{task}"))
    });

    // Replace the summarized range with a single summary message.
    history.clear();
    history.push(ChatMessage::new(
        "system",
        format!("{SUMMARY_MARKER}\n{summary}\n[End Summary — the following messages are the most recent conversation]"),
    ));
    // Re-inject the original task verbatim if it fell inside the summarized range.
    if let Some(task) = pinned_task
        && !task_in_tail
        && !marker_in_tail
    {
        history.push(ChatMessage::new(
            "system",
            format!("{ORIGINAL_TASK_MARKER}\n{task}"),
        ));
    }
    history.extend(tail);

    Ok(())
}

/// Prefix that marks a compaction summary message, used to detect and preserve
/// prior summaries during incremental compaction.
pub(crate) const SUMMARY_MARKER: &str = "[Session History Summary]";
const ORIGINAL_TASK_MARKER: &str = "[Original task — do not lose sight of this]";

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }

    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn render_summary_message(message: &ChatMessage) -> String {
    let role_label = match message.role.as_str() {
        "user" => "User",
        "assistant" => "Assistant",
        "tool" => "Tool Result",
        "system" => "System",
        _ => "Unknown",
    };
    let mut chars = message.content.chars();
    let mut content: String = chars.by_ref().take(SUMMARY_MESSAGE_MAX_CHARS).collect();
    if chars.next().is_some() {
        content.push_str("... [truncated]");
    }
    if let Some(record) = &message.tool_result {
        let error =
            record
                .error_kind
                .as_deref()
                .unwrap_or(if record.success { "none" } else { "unknown" });
        let paths = if record.changed_paths.is_empty() {
            "none".to_string()
        } else {
            record
                .changed_paths
                .iter()
                .take(16)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        content.push_str(&format!(
            "\n[execution metadata: success={}, error_kind={}, retryable={}, exit_code={:?}, truncated={}, replayed={}, changed_paths={paths}]",
            record.success,
            error,
            record.retryable,
            record.exit_code,
            record.truncated,
            record.replayed,
        ));
    }
    if !message.tool_calls.is_empty() {
        let calls = message
            .tool_calls
            .iter()
            .take(16)
            .map(|call| format!("{}({})", call.name, call.id))
            .collect::<Vec<_>>()
            .join(", ");
        content.push_str(&format!("\n[structured tool calls: {calls}]"));
    }
    format!("{role_label}:\n{content}\n\n")
}

/// Build a bounded prompt that pins the original task and prior summary, then
/// spends the remaining space on the newest messages in chronological order.
fn build_summary_input(prior_summary: Option<&str>, messages: &[&ChatMessage]) -> String {
    let first_user_index = messages.iter().position(|message| message.role == "user");
    let mut input = String::with_capacity(SUMMARY_INPUT_MAX_BYTES);
    input.push_str("Summarize this coding conversation.\n\n");

    if let Some(index) = first_user_index {
        input.push_str("Original user task (preserve this objective):\n");
        let rendered = render_summary_message(messages[index]);
        let remaining = SUMMARY_INPUT_MAX_BYTES.saturating_sub(input.len());
        input.push_str(truncate_utf8(&rendered, remaining));
    }

    if let Some(previous) = prior_summary {
        input.push_str("Existing summary of earlier context (preserve every fact):\n");
        let previous = truncate_utf8(previous, SUMMARY_PRIOR_MAX_BYTES);
        let remaining = SUMMARY_INPUT_MAX_BYTES.saturating_sub(input.len());
        input.push_str(truncate_utf8(previous, remaining));
        input.push_str("\n\n");
    }

    input.push_str("Newest messages to fold into the summary:\n\n");
    let mut recent = Vec::new();
    let mut remaining = SUMMARY_INPUT_MAX_BYTES.saturating_sub(input.len());
    for (index, message) in messages.iter().enumerate().rev() {
        if Some(index) == first_user_index {
            continue;
        }
        let rendered = render_summary_message(message);
        if rendered.len() <= remaining {
            remaining -= rendered.len();
            recent.push(rendered);
            continue;
        }
        if remaining > 0 {
            recent.push(truncate_utf8(&rendered, remaining).to_string());
        }
        break;
    }
    for rendered in recent.into_iter().rev() {
        input.push_str(&rendered);
    }

    debug_assert!(input.len() <= SUMMARY_INPUT_MAX_BYTES);
    input
}

fn parse_summary_response(body: &serde_json::Value) -> Option<String> {
    let content = body
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()?
        .trim();
    let content = truncate_utf8(content, SUMMARY_OUTPUT_MAX_BYTES).trim_end();
    if content.is_empty()
        || !content
            .chars()
            .any(|character| !character.is_control() && !character.is_whitespace())
    {
        return None;
    }

    Some(content.to_string())
}

#[derive(Debug, PartialEq, Eq)]
enum SummaryRequestError {
    Cancelled,
    TimedOut,
}

/// Apply one deadline to the complete summary exchange. The supplied future
/// includes both `send()` and response decoding, unlike a client-level connect
/// timeout which only covers establishing the connection.
async fn await_summary_request<F>(
    future: F,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<F::Output, SummaryRequestError>
where
    F: Future,
{
    let timed = tokio::time::timeout(SUMMARY_REQUEST_TIMEOUT, future);
    tokio::pin!(timed);

    if let Some(cancel_token) = cancel_token {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => Err(SummaryRequestError::Cancelled),
            result = &mut timed => result.map_err(|_| SummaryRequestError::TimedOut),
        }
    } else {
        timed.await.map_err(|_| SummaryRequestError::TimedOut)
    }
}

async fn generate_summary(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    prior_summary: Option<&str>,
    messages: &[&ChatMessage],
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Option<String> {
    let user_content = build_summary_input(prior_summary, messages);

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are a conversation summarizer for a coding session. Produce a concise, incremental, inspectable bullet-point state record; merge new facts into the existing summary instead of retelling the transcript. Preserve these headings when applicable: Goal; Constraints; Architecture/discoveries; Decisions; Modified files; Failures and attempted fixes; Verification/test state; Unresolved work; Next steps. Keep exact file paths, commands, diagnostics, and outcomes that remain relevant. Retain recent raw evidence elsewhere in the conversation. Never invent facts, never drop a still-relevant fact from the existing summary, and do not include tool call syntax, JSON, secrets, source-code dumps, or full command output."
            },
            {
                "role": "user",
                "content": user_content
            }
        ],
        "stream": false,
        "temperature": 0.3,
        "max_tokens": 1024,
    });

    let request = async {
        let resp = client.post(url).json(&payload).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: serde_json::Value = resp.json().await.ok()?;
        parse_summary_response(&body)
    };

    await_summary_request(request, cancel_token)
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_msg(content: &str) -> ChatMessage {
        ChatMessage::new("tool", content)
    }

    async fn one_shot_json_server(body: serde_json::Value) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let read = socket.read(&mut buffer).await.expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }

            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        format!("http://{address}")
    }

    async fn pending_response_server() -> (String, tokio::sync::oneshot::Receiver<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept request");
            accepted_tx.send(()).ok();
            std::future::pending::<()>().await;
            drop(socket);
        });
        (format!("http://{address}"), accepted_rx)
    }

    /// The memo must be a pure cache: whatever it returns, on a cold miss or a
    /// warm hit, has to equal what a direct encode of the same string produces.
    #[test]
    fn memoized_counts_match_a_direct_encode() {
        let bpe = cl100k_base().unwrap();
        let long_repeat = "x ".repeat(5000);
        let code_blob = format!(
            "view_file: {}",
            "fn main() { println!(\"hi\"); }\n".repeat(200)
        );
        let samples: [&str; 6] = [
            "",
            "hello world",
            "run_command: ls -la /usr/local/bin\nexit code 0",
            "emoji ✅ and accents éàü and CJK 日本語テキスト",
            &long_repeat,
            &code_blob,
        ];
        for (i, s) in samples.iter().enumerate() {
            let expected = bpe.encode_ordinary(s).len();
            // Cold (or already-warm) first call, then a guaranteed cache hit.
            assert_eq!(estimate_tokens(s), expected, "first count of sample {i}");
            assert_eq!(estimate_tokens(s), expected, "cached count of sample {i}");
        }
    }

    /// Distinct strings must not share a memo entry, and rewriting a message
    /// (as pruning does) must be counted afresh rather than hitting the old
    /// content's entry.
    #[test]
    fn memo_distinguishes_different_content() {
        let bpe = cl100k_base().unwrap();
        let original = format!("run_command: {}", "data ".repeat(400));
        let rewritten = "run_command: [Tool Output Truncated: 400 tokens reduced to summary.]";

        assert_eq!(
            estimate_tokens(&original),
            bpe.encode_ordinary(&original).len()
        );
        assert_eq!(
            estimate_tokens(rewritten),
            bpe.encode_ordinary(rewritten).len()
        );
        assert_ne!(estimate_tokens(&original), estimate_tokens(rewritten));
    }

    /// Filling the memo past its capacity must not grow it without bound, and
    /// must not corrupt the counts that survive.
    #[test]
    fn memo_stays_bounded_and_correct_under_churn() {
        let bpe = cl100k_base().unwrap();
        let hot = "the hot message that keeps being counted every pass";
        let hot_expected = bpe.encode_ordinary(hot).len();

        for i in 0..(TOKEN_MEMO_CAPACITY * 2 + 100) {
            estimate_tokens(&format!("unique filler message number {i}"));
            if i % 64 == 0 {
                assert_eq!(estimate_tokens(hot), hot_expected);
            }
        }

        let memo = TOKEN_MEMO
            .get()
            .expect("memo initialized by the calls above");
        let guard = memo.lock().unwrap_or_else(|e| e.into_inner());
        assert!(guard.live.len() <= TOKEN_MEMO_CAPACITY);
        assert!(guard.prev.len() <= TOKEN_MEMO_CAPACITY);
        drop(guard);

        assert_eq!(estimate_tokens(hot), hot_expected);
    }

    // Regression: session 1785593632937. A 6677-token read of src/main.rs was
    // collapsed to a one-line summary three round-trips later, while the window
    // was barely used, so the model could no longer answer from what it had just
    // read.
    #[tokio::test]
    async fn a_roomy_window_keeps_every_tool_output_intact() {
        let big_read = format!("view_file: {}", "line of source\n".repeat(4000));
        let mut history = vec![
            ChatMessage::new("user", "what is the binary name"),
            ChatMessage::new("assistant", "reading"),
            ChatMessage::new("tool", big_read.clone()),
        ];
        for _ in 0..8 {
            history.push(ChatMessage::new("assistant", "thinking"));
            history.push(ChatMessage::new("tool", "grep: one match"));
        }

        let client = reqwest::Client::new();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        // Budget far larger than the history: nothing should be touched.
        let compacted = maybe_compact(
            &client,
            "http://unused",
            "model",
            &mut history,
            1_000_000,
            &cancel_token,
        )
        .await;

        assert!(!compacted);
        assert_eq!(history[2].content, big_read, "the read must survive");
        assert!(!history[2].content.contains("Truncated"));
    }

    #[test]
    fn prune_floor_scales_with_the_budget() {
        assert_eq!(prune_floor(100_000), 50_000);
        assert_eq!(prune_floor(8_000), 4_000);
    }

    #[test]
    fn prune_historical_keeps_recent_and_collapses_old() {
        let big = format!("run_command: {}", "x ".repeat(3000)); // > 1000 tokens
        // A large tool output at the front, and a large one near the tail.
        let mut history = vec![tool_msg(&big)]; // index 0: will age out
        for i in 0..(KEEP_RECENT_TURNS + 1) {
            history.push(ChatMessage::new("user", format!("pad {i}")));
        }
        let recent_idx = history.len();
        history.push(tool_msg(&big)); // within the last KEEP_RECENT_TURNS -> kept

        prune_historical_tool_outputs(&mut history, KEEP_RECENT_TURNS);

        // Old, large tool output collapsed with prefix + token count preserved.
        assert!(
            history[0]
                .content
                .starts_with("run_command: [Tool Output Truncated:")
        );
        assert!(history[0].content.contains("tokens reduced to summary"));
        // Recent large tool output left fully intact.
        assert!(history[recent_idx].content.starts_with("run_command: x x"));
    }

    #[test]
    fn prune_historical_reports_exit_code() {
        let big = format!("run_command: {} exit code 2", "y ".repeat(3000));
        let mut history = vec![tool_msg(&big)];
        for i in 0..(KEEP_RECENT_TURNS + 2) {
            history.push(ChatMessage::new("user", format!("m{i}")));
        }
        prune_historical_tool_outputs(&mut history, KEEP_RECENT_TURNS);
        assert!(history[0].content.contains("Command exited with code 2."));
    }

    #[test]
    fn prune_historical_leaves_small_outputs_alone() {
        let mut history = vec![tool_msg("grep: match at line 4")];
        for i in 0..(KEEP_RECENT_TURNS + 2) {
            history.push(ChatMessage::new("user", format!("m{i}")));
        }
        prune_historical_tool_outputs(&mut history, KEEP_RECENT_TURNS);
        assert_eq!(history[0].content, "grep: match at line 4");
    }

    #[test]
    fn prune_old_tool_outputs_does_not_count_already_stubbed_results() {
        let stub =
            "run_command: [Tool output truncated: 2000 tokens pruned to maintain context window]";
        let recent = "grep: recent match";
        let mut history = vec![tool_msg(stub), tool_msg(recent)];
        let threshold = estimate_message_tokens(&history[1]) + 1;

        let pruned = prune_old_tool_outputs(&mut history, threshold);

        assert_eq!(pruned, 0);
        assert_eq!(history[0].content, stub);
    }

    #[test]
    fn duplicate_old_file_reads_collapse_but_changed_reads_survive() {
        let same = "view_file: [File: src/lib.rs]\n1: old";
        let changed = "view_file: [File: src/lib.rs]\n1: new";
        let mut history = vec![
            tool_msg(same),
            ChatMessage::new("assistant", "edit").with_diff(Some("real diff".to_string())),
            tool_msg(changed),
        ];
        let collapsed = prune_duplicate_tool_results(&mut history, 1);
        assert_eq!(collapsed, 0, "the duplicate is in the protected suffix");

        history.extend([
            ChatMessage::new("assistant", "more work"),
            ChatMessage::new("user", "verify"),
        ]);
        let collapsed = prune_duplicate_tool_results(&mut history, 2);
        assert_eq!(collapsed, 0, "different file content is not a duplicate");

        history.insert(0, tool_msg(same));
        let collapsed = prune_duplicate_tool_results(&mut history, 2);
        assert_eq!(collapsed, 1);
        assert!(history[0].content.contains("Duplicate unchanged file read"));
    }

    #[test]
    fn duplicate_read_pruning_requires_file_identity_and_complete_content() {
        let mut history = vec![
            tool_msg("view_file: [File: src/a.rs, Lines 1 to 1 of 1]\n1: same"),
            tool_msg("view_file: [File: src/a.rs, Lines 1 to 1 of 1]\n1: same"),
            tool_msg("view_file: [File: src/b.rs, Lines 1 to 1 of 1]\n1: same"),
            tool_msg("view_file: error: cannot read 'src/c.rs'"),
            tool_msg(
                "view_file: [File: src/d.rs, Lines 1 to 1 of 2]\n1: same\n[Truncated: lines 2-2 of 2]",
            ),
        ];
        history.push(ChatMessage::new("user", "keep recent"));
        assert_eq!(prune_duplicate_tool_results(&mut history, 1), 1);
        assert!(history[0].content.contains("Duplicate unchanged file read"));
        assert!(history[1].content.contains("src/a.rs"));
        assert!(history[2].content.contains("src/b.rs"));
        assert!(history[3].content.contains("cannot read"));
        assert!(history[4].content.contains("Truncated"));
    }

    #[test]
    fn old_failures_keep_compact_evidence() {
        let mut history = vec![tool_msg(&format!(
            "run_command: error: {}",
            "diagnostic ".repeat(3_000)
        ))];
        prune_old_tool_outputs(&mut history, 1);
        assert!(history[0].content.contains("failure/diagnostic evidence"));
    }

    #[test]
    fn summary_input_is_globally_bounded_and_keeps_task_and_recent_facts() {
        let mut history = vec![ChatMessage::new(
            "user",
            "ORIGINAL-TASK: preserve this exact objective",
        )];
        for i in 0..200 {
            history.push(ChatMessage::new(
                "tool",
                format!("OLD-FACT-{i}: {}", "x".repeat(2_000)),
            ));
        }
        history.push(ChatMessage::new(
            "assistant",
            "NEWEST-FACT: src/network/compaction.rs is the active file",
        ));
        let refs: Vec<&ChatMessage> = history.iter().collect();
        let prior = format!("PRIOR-SUMMARY: {}", "é".repeat(SUMMARY_INPUT_MAX_BYTES));

        let input = build_summary_input(Some(&prior), &refs);

        assert!(
            input.len() <= SUMMARY_INPUT_MAX_BYTES,
            "{} bytes",
            input.len()
        );
        assert!(input.contains("ORIGINAL-TASK: preserve this exact objective"));
        assert!(input.contains("NEWEST-FACT: src/network/compaction.rs is the active file"));
        assert!(input.contains("PRIOR-SUMMARY:"));
        assert!(
            !input.contains("OLD-FACT-0:"),
            "oldest bulk should be dropped first"
        );
    }

    #[test]
    fn summary_response_rejects_empty_or_invalid_provider_content() {
        let empty = serde_json::json!({
            "choices": [{"message": {"content": "  \n\t "}}]
        });
        let control_only = serde_json::json!({
            "choices": [{"message": {"content": "\u{0000}\u{0007}"}}]
        });
        let missing_content = serde_json::json!({
            "choices": [{"message": {}}]
        });

        assert!(parse_summary_response(&empty).is_none());
        assert!(parse_summary_response(&control_only).is_none());
        assert!(parse_summary_response(&missing_content).is_none());
    }

    #[test]
    fn summary_response_caps_multibyte_utf8_safely() {
        let body = serde_json::json!({
            "choices": [{
                "message": {"content": format!("useful facts {}", "é".repeat(SUMMARY_OUTPUT_MAX_BYTES))}
            }]
        });

        let summary = parse_summary_response(&body).expect("valid summary");

        assert!(
            summary.len() <= SUMMARY_OUTPUT_MAX_BYTES,
            "{} bytes",
            summary.len()
        );
        assert!(summary.starts_with("useful facts"));
        assert!(std::str::from_utf8(summary.as_bytes()).is_ok());
    }

    #[test]
    fn summary_response_rejects_output_invalidated_by_truncation() {
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "content": format!("{}visible", "\u{0007}".repeat(SUMMARY_OUTPUT_MAX_BYTES))
                }
            }]
        });

        assert!(parse_summary_response(&body).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn summary_request_total_timeout_bounds_body_decoding() {
        let task =
            tokio::spawn(async { await_summary_request(std::future::pending::<()>(), None).await });
        tokio::task::yield_now().await;

        tokio::time::advance(SUMMARY_REQUEST_TIMEOUT + std::time::Duration::from_millis(1)).await;

        assert_eq!(
            task.await.expect("timeout task must not panic"),
            Err(SummaryRequestError::TimedOut)
        );
    }

    #[tokio::test]
    async fn summary_request_cancellation_interrupts_pending_io() {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let request_cancel = cancel_token.clone();
        let task = tokio::spawn(async move {
            await_summary_request(std::future::pending::<()>(), Some(&request_cancel)).await
        });
        tokio::task::yield_now().await;

        cancel_token.cancel();

        assert_eq!(
            task.await.expect("cancellation task must not panic"),
            Err(SummaryRequestError::Cancelled)
        );
    }

    #[tokio::test]
    async fn manual_compaction_cancellation_interrupts_pending_summary_request() {
        let (url, request_accepted) = pending_response_server().await;
        let mut history = vec![ChatMessage::new("user", "original task")];
        for index in 0..(KEEP_RECENT_TURNS + 4) {
            history.push(ChatMessage::new("assistant", format!("fact {index}")));
        }
        let expected: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let task_token = cancel_token.clone();
        let mut task = tokio::spawn(async move {
            let result = force_compact(
                &reqwest::Client::new(),
                &url,
                "model",
                &mut history,
                Some(&task_token),
            )
            .await;
            (result, history)
        });
        tokio::select! {
            accepted = request_accepted => {
                accepted.expect("manual compaction server must signal acceptance");
            }
            result = &mut task => {
                let (result, _) = result.expect("manual compaction task must not panic");
                panic!("manual compaction ended before making its request: {result:?}");
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
                panic!("manual compaction request must start");
            }
        }

        cancel_token.cancel();

        let (result, history) = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("cancellation must interrupt manual compaction")
            .expect("manual compaction task must not panic");
        let actual: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        assert_eq!(result, Err("Failed to generate summary.".to_string()));
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn cancelled_automatic_compaction_keeps_history_with_local_pruning_only() {
        let mut history = vec![ChatMessage::new("user", "keep the original task")];
        history.push(tool_msg(&format!("view_file: {}", "source ".repeat(3_000))));
        for i in 0..12 {
            history.push(ChatMessage::new(
                "assistant",
                format!("FACT-{i}: {}", "progress ".repeat(30)),
            ));
        }
        let expected_non_tool: Vec<(String, String)> = history
            .iter()
            .filter(|message| message.role != "tool")
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();

        let compacted = maybe_compact(
            &reqwest::Client::new(),
            "http://unused",
            "model",
            &mut history,
            200,
            &cancel_token,
        )
        .await;

        let actual_non_tool: Vec<(String, String)> = history
            .iter()
            .filter(|message| message.role != "tool")
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        assert!(!compacted);
        assert_eq!(actual_non_tool, expected_non_tool);
        assert!(history[1].content.contains("Tool Output Truncated"));
        assert!(
            !history
                .iter()
                .any(|message| message.content.starts_with(SUMMARY_MARKER))
        );
    }

    #[tokio::test]
    async fn explicit_local_model_hint_skips_summary_request() {
        let mut history = vec![ChatMessage::new("user", "keep this task")];
        history.extend((0..20).map(|index| {
            ChatMessage::new(
                "assistant",
                format!("fact {index}: {}", "detail ".repeat(120)),
            )
        }));

        let compacted = maybe_compact_with_local_policy(
            &reqwest::Client::new(),
            "http://127.0.0.1:9/v1",
            "local-qwen",
            &mut history,
            200,
            &tokio_util::sync::CancellationToken::new(),
            true,
        )
        .await;

        assert!(compacted);
        assert!(
            !history
                .iter()
                .any(|message| message.content.starts_with(SUMMARY_MARKER))
        );
    }

    #[tokio::test]
    async fn manual_compaction_failure_keeps_history_and_returns_error() {
        let url = one_shot_json_server(serde_json::json!({
            "choices": [{"message": {"content": "  "}}]
        }))
        .await;
        let mut history = vec![ChatMessage::new("user", "original task")];
        for i in 0..(KEEP_RECENT_TURNS + 1) {
            history.push(ChatMessage::new("assistant", format!("fact {i}")));
        }
        let expected: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();

        let result =
            force_compact(&reqwest::Client::new(), &url, "model", &mut history, None).await;

        let actual: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        assert_eq!(result, Err("Failed to generate summary.".to_string()));
        assert_eq!(actual, expected);
        assert!(
            !history
                .iter()
                .any(|message| message.content.starts_with(SUMMARY_MARKER))
        );
    }

    #[tokio::test]
    async fn incremental_compaction_keeps_the_original_task_marker() {
        let url = one_shot_json_server(serde_json::json!({
            "choices": [{"message": {"content": "Goal: retain the original objective"}}]
        }))
        .await;
        let mut history = vec![
            ChatMessage::new("system", format!("{SUMMARY_MARKER}\nPrior facts")),
            ChatMessage::new(
                "system",
                format!("{ORIGINAL_TASK_MARKER}\noriginal objective"),
            ),
            ChatMessage::new("user", "current follow-up"),
        ];
        history.extend(
            (0..13).map(|index| ChatMessage::new("assistant", format!("new fact {index}"))),
        );

        force_compact(&reqwest::Client::new(), &url, "model", &mut history, None)
            .await
            .expect("incremental compaction should succeed");

        assert!(history.iter().any(|message| {
            message.content == format!("{ORIGINAL_TASK_MARKER}\noriginal objective")
        }));
    }
}
