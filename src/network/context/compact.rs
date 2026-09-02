use super::memory::compact_with_structured_memory;
use super::prune::{
    DEFAULT_PRUNE_TOKEN_THRESHOLD, KEEP_RECENT_TURNS, LAST_COMPACTION_RECLAIMED,
    prune_duplicate_tool_results, prune_floor, prune_historical_reasoning,
    prune_historical_tool_outputs, prune_old_tool_outputs,
};
use super::tokens::estimate_message_tokens;
use crate::app::{ChatMessage, CompactionBoundary, CompactionEntry, TokenUsage};
use std::future::Future;
use std::time::Duration;

pub(super) const SUMMARY_INPUT_MAX_BYTES: usize = 64 * 1024;

/// A prior summary is high-value context, but must leave room for the original
/// task and recent facts inside [`SUMMARY_INPUT_MAX_BYTES`].
const SUMMARY_PRIOR_MAX_BYTES: usize = 24 * 1024;

/// Provider output is requested at 1024 tokens; 16 KiB is a generous defensive
/// byte ceiling for providers that ignore that limit.
pub(super) const SUMMARY_OUTPUT_MAX_BYTES: usize = 16 * 1024;
const PRESERVED_USER_REQUEST_MAX_TOKENS: usize = 20_000;
const PRESERVED_USER_REQUEST_BUDGET_DIVISOR: usize = 5;

/// Total wall-clock budget for a non-streaming summary request, including
/// connection, response headers, body transfer, and JSON decoding. Twenty-five
/// seconds bounds the request while leaving ample time for valid summaries.
pub(super) const SUMMARY_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

const COMPACTION_BOUNDARY_VERSION: u8 = 1;

/// Preserve the existing per-message limit while additionally enforcing the
/// whole-input byte ceiling above.
const SUMMARY_MESSAGE_MAX_CHARS: usize = 2_000;

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
    maybe_compact_with_local_policy_and_usage(
        client,
        url,
        model,
        history,
        budget,
        cancel_token,
        false,
        None,
    )
    .await
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
    maybe_compact_with_local_policy_and_usage(
        client,
        url,
        model,
        history,
        budget,
        cancel_token,
        local_model,
        None,
    )
    .await
}

/// Automatic compaction using the last provider-reported prompt size when it
/// is available. Provider usage describes the complete prompt at the previous
/// response; only messages appended after that response are estimated locally.
/// This avoids replacing an authoritative provider measurement with a second,
/// divergent tokenization of the entire conversation.
pub async fn maybe_compact_with_local_policy_and_usage(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    budget: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
    local_model: bool,
    provider_usage: Option<&TokenUsage>,
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
    let raw_tokens = provider_adjusted_tokens(history, provider_usage);
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
    let total_tokens = provider_adjusted_tokens(history, provider_usage);
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

    // Determine how many messages to summarize. Keep a bounded recent suffix;
    // preserving an unbounded number of "recent" messages defeats compaction
    // when one tool result is very large.
    let mut accumulated_tokens = 0;
    let keep_token_limit = (budget as f64 * 0.3) as usize;

    let mut keep_count = 0;
    for &tokens in per_message.iter().rev() {
        if keep_count == 0 || accumulated_tokens + tokens <= keep_token_limit {
            accumulated_tokens += tokens;
            keep_count += 1;
        } else {
            break;
        }
    }

    let summarize_count = bounded_recent_suffix_start(
        history,
        history.len().saturating_sub(keep_count),
        keep_token_limit,
    );
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
        Some(budget),
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

/// Return the best available prompt estimate. A provider usage record belongs
/// to the most recent assistant response carrying that record. Messages after
/// that assistant response (tool results, lifecycle notes, and the next user
/// request) were not part of the measured prompt and are therefore estimated
/// deterministically and added to it.
fn provider_adjusted_tokens(history: &[ChatMessage], usage: Option<&TokenUsage>) -> usize {
    let Some(usage) = usage else {
        return history.iter().map(estimate_message_tokens).sum();
    };

    let measured_response = history.iter().rposition(|message| {
        message.role == "assistant"
            && message
                .token_usage
                .as_ref()
                .is_some_and(|record| record == usage)
    });
    let Some(index) = measured_response else {
        // Without a durable response marker there is no safe way to identify
        // the unmeasured suffix. Re-estimating the complete history is the
        // conservative fallback.
        return history.iter().map(estimate_message_tokens).sum();
    };

    (usage.prompt_tokens as usize).saturating_add(
        history[index.saturating_add(1)..]
            .iter()
            .map(estimate_message_tokens)
            .sum::<usize>(),
    )
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

    let result = force_compact_internal(
        client,
        url,
        model,
        history,
        summarize_count,
        budget,
        cancel_token,
    )
    .await;
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
    budget: Option<usize>,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(), String> {
    // A caller may have selected a message-count cut point. Normalize it to a
    // complete turn so a result is never sent without its announcing call.
    let summarize_count = valid_compaction_boundary(history, summarize_count);
    // Incremental compaction: if a prior summary already sits at the front of the
    // range, preserve its facts and only summarize the messages that came after.
    // Avoids re-compressing an already-compressed summary (which drifts and loses
    // detail every pass).
    let prior_summary = history
        .iter()
        .take(summarize_count)
        .find(|m| m.role == "system" && m.content.starts_with(SUMMARY_MARKER))
        .map(|m| {
            m.compaction_boundary
                .as_ref()
                .map(|boundary| boundary.summary.clone())
                .unwrap_or_else(|| {
                    m.content
                        .trim_start_matches(SUMMARY_MARKER)
                        .trim_start_matches('\n')
                        .to_string()
                })
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
    let preserved_user_requests = collect_preserved_user_requests(
        &history[..summarize_count],
        &tail,
        pinned_task.as_deref(),
        budget
            .map(|tokens| tokens / PRESERVED_USER_REQUEST_BUDGET_DIVISOR)
            .unwrap_or(PRESERVED_USER_REQUEST_MAX_TOKENS)
            .min(PRESERVED_USER_REQUEST_MAX_TOKENS),
    );

    // Replace the summarized range with a single summary message. Build the
    // complete retained suffix first so the durable boundary points at the
    // actual first entry the next request will replay, including pinned task
    // and preserved-request markers.
    let mut retained_tail = Vec::with_capacity(tail.len() + preserved_user_requests.len() + 1);
    history.clear();
    // Re-inject the original task verbatim if it fell inside the summarized range.
    if let Some(task) = pinned_task
        && !task_in_tail
        && !marker_in_tail
    {
        retained_tail.push(ChatMessage::new(
            "system",
            format!("{ORIGINAL_TASK_MARKER}\n{task}"),
        ));
    }
    for request in preserved_user_requests {
        retained_tail.push(ChatMessage::new(
            "system",
            format!("{PRESERVED_USER_REQUEST_MARKER}\n{request}"),
        ));
    }
    retained_tail.extend(tail);
    history.push(durable_compaction_message(&summary, &retained_tail));
    history.extend(retained_tail);

    Ok(())
}

/// Build the durable summary entry used by both AI and deterministic
/// compaction. Keeping the boundary on the summary message preserves the
/// existing history wire format while giving newer readers a typed retained
/// suffix anchor.
pub(crate) fn durable_compaction_message(
    summary: &str,
    retained_tail: &[ChatMessage],
) -> ChatMessage {
    ChatMessage::new(
        "system",
        format!(
            "{SUMMARY_MARKER}\n{summary}\n[End Summary — the following messages are the most recent conversation]"
        ),
    )
    .with_compaction_boundary(compaction_boundary(summary, retained_tail))
}

/// Build a typed deterministic record without changing its historical
/// non-summary presentation. Local providers intentionally skip the AI
/// summarizer, and callers rely on this record remaining a normal system note.
pub(crate) fn durable_compaction_record_message(
    record: &str,
    retained_tail: &[ChatMessage],
) -> ChatMessage {
    ChatMessage::new("system", record)
        .with_compaction_boundary(compaction_boundary(record, retained_tail))
}

fn compaction_boundary(summary: &str, retained_tail: &[ChatMessage]) -> CompactionBoundary {
    CompactionBoundary {
        version: COMPACTION_BOUNDARY_VERSION,
        summary: summary.to_string(),
        first_retained_entry: retained_tail.first().map(CompactionEntry::from_message),
    }
}

/// Prefix that marks a compaction summary message, used to detect and preserve
/// prior summaries during incremental compaction.
pub(crate) const SUMMARY_MARKER: &str = "[Session History Summary]";
pub(super) const ORIGINAL_TASK_MARKER: &str = "[Original task — do not lose sight of this]";
pub(super) const PRESERVED_USER_REQUEST_MARKER: &str = "[Preserved user request]";

/// Find a suffix start that keeps the recent token allowance while respecting
/// assistant tool-call/result transactions. The newest message is always
/// retained, even when it alone exceeds the allowance; dropping it would lose
/// the active turn entirely and is handled by deterministic output pruning.
pub(crate) fn bounded_recent_suffix_start(
    history: &[ChatMessage],
    desired_start: usize,
    token_limit: usize,
) -> usize {
    if history.is_empty() {
        return 0;
    }
    let mut start = desired_start.min(history.len().saturating_sub(1));
    let mut tokens = history[start..]
        .iter()
        .map(estimate_message_tokens)
        .sum::<usize>();
    // Keep at least the newest message even when it alone exceeds the suffix budget.
    while start + 1 < history.len() && tokens > token_limit {
        start += 1;
        tokens = history[start..].iter().map(estimate_message_tokens).sum();
    }
    valid_compaction_boundary(history, start)
}

/// Move a cut before a complete user turn. Tool results are valid only after
/// the assistant message that announced their call; keeping that assistant and
/// its preceding user request avoids orphaned protocol entries on replay.
pub(crate) fn valid_compaction_boundary(history: &[ChatMessage], boundary: usize) -> usize {
    let mut start = boundary.min(history.len());
    if start == history.len() || start == 0 {
        return start;
    }

    if history[start].role == "user" && !history[start].content.starts_with("<tool_result>") {
        return start;
    }

    if history[start].role == "tool" {
        if let Some(call_id) = history[start].tool_call_id.as_deref()
            && let Some(assistant) = history[..start].iter().rposition(|message| {
                message.role == "assistant"
                    && message.tool_calls.iter().any(|call| call.id == call_id)
            })
        {
            start = assistant;
        }
    } else if history[start].role == "assistant" && !history[start].tool_calls.is_empty() {
        // A structured call is part of the user turn even if no result has
        // arrived yet.
        start = start.saturating_sub(1);
    }

    history[..start]
        .iter()
        .rposition(|message| {
            message.role == "user" && !message.content.starts_with("<tool_result>")
        })
        .unwrap_or(start)
}

fn collect_preserved_user_requests(
    summarized: &[ChatMessage],
    tail: &[ChatMessage],
    pinned_task: Option<&str>,
    token_budget: usize,
) -> Vec<String> {
    if token_budget == 0 {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    for message in summarized {
        let content = if message.role == "user" && !message.content.starts_with("<tool_result>") {
            Some(message.content.as_str())
        } else if message.role == "system"
            && message.content.starts_with(PRESERVED_USER_REQUEST_MARKER)
        {
            message
                .content
                .strip_prefix(PRESERVED_USER_REQUEST_MARKER)
                .map(str::trim_start)
        } else {
            None
        };
        let Some(content) = content else { continue };
        if content.is_empty()
            || pinned_task == Some(content)
            || tail
                .iter()
                .any(|message| message.role == "user" && message.content == content)
            || candidates
                .iter()
                .any(|existing: &String| existing == content)
        {
            continue;
        }
        candidates.push(content.to_string());
    }

    let mut selected = Vec::new();
    let mut used = 0usize;
    for request in candidates.into_iter().rev() {
        let tokens = estimate_message_tokens(&ChatMessage::new("user", request.clone()));
        if used.saturating_add(tokens) > token_budget {
            continue;
        }
        used = used.saturating_add(tokens);
        selected.push(request);
    }
    selected.reverse();
    selected
}

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
            "\n[execution metadata: success={}, error_kind={}, retryable={}, exit_code={:?}, truncated={}, completeness={}, replayed={}, changed_paths={paths}]",
            record.success,
            error,
            record.retryable,
            record.exit_code,
            record.truncated,
            record.resolved_completeness().as_str(),
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
pub(super) fn build_summary_input(
    prior_summary: Option<&str>,
    messages: &[&ChatMessage],
) -> String {
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

pub(super) fn parse_summary_response(body: &serde_json::Value) -> Option<String> {
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
pub(super) enum SummaryRequestError {
    Cancelled,
    TimedOut,
}

/// Apply one deadline to the complete summary exchange. The supplied future
/// includes both `send()` and response decoding, unlike a client-level connect
/// timeout which only covers establishing the connection.
pub(super) async fn await_summary_request<F>(
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
mod preserved_user_request_tests {
    use super::*;

    #[test]
    fn keeps_recent_authoritative_user_wording_within_budget() {
        let summarized = vec![
            ChatMessage::new("user", "original goal"),
            ChatMessage::new("assistant", "working"),
            ChatMessage::new("user", "do not change the public API"),
            ChatMessage::new("user", "also keep the wire format stable"),
        ];
        let tail = vec![ChatMessage::new("user", "current follow-up")];

        let preserved =
            collect_preserved_user_requests(&summarized, &tail, Some("original goal"), 100);

        assert_eq!(
            preserved,
            vec![
                "do not change the public API".to_string(),
                "also keep the wire format stable".to_string(),
            ]
        );
    }

    #[test]
    fn carries_preserved_requests_across_later_compactions_without_duplicates() {
        let summarized = vec![
            ChatMessage::new(
                "system",
                format!("{PRESERVED_USER_REQUEST_MARKER}\nkeep this exact constraint"),
            ),
            ChatMessage::new("user", "keep this exact constraint"),
        ];

        let preserved = collect_preserved_user_requests(&summarized, &[], None, 100);

        assert_eq!(preserved, vec!["keep this exact constraint".to_string()]);
    }

    #[test]
    fn provider_usage_is_extended_only_by_messages_after_measured_response() {
        let usage = TokenUsage {
            prompt_tokens: 1_000,
            completion_tokens: 20,
            total_tokens: 1_020,
            cached_tokens: None,
        };
        let mut measured = ChatMessage::new("assistant", "measured response");
        measured.token_usage = Some(usage.clone());
        let history = vec![
            ChatMessage::new("user", "old prompt"),
            measured,
            ChatMessage::new("tool", "run_command: output"),
            ChatMessage::new("user", "follow-up"),
        ];

        let estimate = provider_adjusted_tokens(&history, Some(&usage));
        assert_eq!(
            estimate,
            1_000 + estimate_message_tokens(&history[2]) + estimate_message_tokens(&history[3])
        );
    }

    #[test]
    fn compaction_boundary_moves_before_tool_call_transaction() {
        let history = vec![
            ChatMessage::new("user", "task"),
            ChatMessage::new("assistant", "call").with_tool_calls(vec![crate::app::ToolCallRef {
                id: "call-1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            }]),
            ChatMessage::new("tool", "read_file: result").answering(Some("call-1".into())),
            ChatMessage::new("user", "next"),
        ];

        assert_eq!(valid_compaction_boundary(&history, 2), 0);
        assert_eq!(valid_compaction_boundary(&history, 3), 3);
    }

    #[test]
    fn bounded_suffix_always_keeps_the_newest_oversized_message() {
        let history = vec![
            ChatMessage::new("user", "old task"),
            ChatMessage::new("assistant", "x".repeat(20_000)),
        ];

        assert_eq!(bounded_recent_suffix_start(&history, 1, 1), 0);
    }

    #[test]
    fn deterministic_record_carries_bounded_file_inventories() {
        let mut read = ChatMessage::new("tool", "view_file: [File: src/lib.rs]\ncontents");
        read.tool_call_id = Some("read-1".into());
        let mut write = ChatMessage::new("tool", "write_to_file: wrote it");
        write.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "write_to_file".into(),
            success: true,
            changed_paths: vec!["src/lib.rs".into()],
            ..Default::default()
        });
        let record = crate::network::compaction::StructuredSessionMemory::extract_from_history(&[
            ChatMessage::new("user", "task"),
            read,
            write,
        ])
        .format_record(2_000);
        assert!(record.contains("Modified files: src/lib.rs"));
        assert!(record.contains("Inspected files: src/lib.rs"));
    }
}
