use super::memory::compact_with_structured_memory;
use super::prune::{DEFAULT_PRUNE_TOKEN_THRESHOLD, KEEP_RECENT_TURNS, prune_floor};
use super::tokens::estimate_message_tokens;
use crate::app::{ChatMessage, CompactionBoundary, CompactionEntry, TokenUsage};
use std::future::Future;
use std::time::Duration;

pub(super) const SUMMARY_INPUT_MAX_BYTES: usize = 64 * 1024;

/// Provider output is requested at 1024 tokens; 16 KiB is a generous defensive
/// byte ceiling for providers that ignore that limit.
pub(super) const SUMMARY_OUTPUT_MAX_BYTES: usize = 16 * 1024;
const PRESERVED_USER_REQUEST_MAX_TOKENS: usize = 20_000;
const PRESERVED_USER_REQUEST_BUDGET_DIVISOR: usize = 5;

/// Recent tokens kept verbatim after a compaction.
///
/// This is an absolute allowance rather than a share of the window, so a large
/// context does not turn into an enormous raw tail that defeats the point of
/// compacting. Twenty thousand tokens is roughly a long working session's worth
/// of recent tool rounds, and matches what Pi keeps by default.
pub(crate) const KEEP_RECENT_TOKENS: usize = 20_000;

/// Share of the window the raw tail may never exceed. On a small local window
/// the absolute allowance above would eat most of the budget, so the window
/// caps it; on a large window the absolute figure binds instead.
const KEEP_RECENT_WINDOW_SHARE: f64 = 0.3;

/// Index of the first message that compaction must leave untouched.
///
/// One rule for both automatic compaction and manual `/compact`. The newest
/// message is always retained even when it alone exceeds the allowance, and the
/// cut is snapped back to a complete turn so a tool result is never replayed
/// without the call that announced it. That floor is what keeps the turn in
/// progress intact.
pub(crate) fn retention_tail_start(history: &[ChatMessage], budget: usize) -> usize {
    if history.is_empty() {
        return 0;
    }
    let limit = ((budget as f64 * KEEP_RECENT_WINDOW_SHARE) as usize)
        .min(KEEP_RECENT_TOKENS)
        .max(1);
    let mut tokens = 0usize;
    let mut start = history.len();
    for index in (0..history.len()).rev() {
        let message_tokens = estimate_message_tokens(&history[index]);
        if start == history.len() || tokens.saturating_add(message_tokens) <= limit {
            tokens = tokens.saturating_add(message_tokens);
            start = index;
        } else {
            break;
        }
    }
    valid_compaction_boundary(history, start)
}

/// The same cut for manual `/compact`.
///
/// An explicit request must reclaim something. When the whole history fits
/// inside the token allowance the automatic rule keeps everything, so the tail
/// is also capped at the last [`KEEP_RECENT_TURNS`] messages, which is what
/// `/compact` kept before the token allowance existed.
pub(crate) fn retention_tail_start_for_manual(history: &[ChatMessage], budget: usize) -> usize {
    let by_tokens = retention_tail_start(history, budget);
    let by_messages = history.len().saturating_sub(KEEP_RECENT_TURNS);
    valid_compaction_boundary(history, by_tokens.max(by_messages))
}

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
#[cfg(test)]
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
#[cfg(test)]
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
    maybe_compact_with_local_policy_and_usage_mode(
        client,
        url,
        model,
        history,
        budget,
        cancel_token,
        local_model,
        provider_usage,
        false,
        true,
    )
    .await
}

pub async fn maybe_compact_with_local_policy_and_usage_mode(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    budget: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
    local_model: bool,
    provider_usage: Option<&TokenUsage>,
    deterministic_only: bool,
    turn_boundary: bool,
) -> bool {
    // 1. Only compact under real pressure. Replacing history the model still
    //    needs while most of the window sits unused buys nothing and costs it
    //    the memory of what it just did — it re-reads, which is slower and is
    //    how repeat-loops start. Below the threshold the history is left
    //    exactly as it happened.
    //
    //    There are no in-place prune passes here any more (#985): they could
    //    not rewrite stored messages, so calling them would only produce zeros
    //    to OR into the result below. Request-time rendering already drops
    //    duplicate reads and historical reasoning without touching storage.
    let raw_tokens = provider_adjusted_tokens(history, provider_usage);
    if raw_tokens < prune_floor(budget) {
        return false;
    }

    // Keep the exact persisted prefix available for archival even when the
    // local request projection below prunes rendered tool output.
    let archival_source = history.clone();
    let total_tokens = raw_tokens;
    // Cancellation must never report a completed compaction or start a
    // summarizer request.
    if cancel_token.is_cancelled() {
        return false;
    }
    if total_tokens < budget {
        emit_compaction_metrics(raw_tokens, budget, "below_budget", false, history.len());
        return false;
    }

    // Keep a bounded recent suffix verbatim, using the same allowance manual
    // `/compact` uses so both paths retain the same amount.
    let summarize_count = retention_tail_start(history, budget);
    let keep_count = history.len().saturating_sub(summarize_count);
    if summarize_count < 4 {
        emit_compaction_metrics(raw_tokens, budget, "too_few_messages", false, history.len());
        return false;
    }
    // Tiered compaction. The deterministic record is the only relief that may
    // run between tool calls: it is local, costs no round-trip, and never waits
    // on the network mid-turn. The LLM summarizer additionally waits for a turn
    // boundary, so a compaction can never rewrite the tool results the model is
    // still working from. Local engines skip summarization entirely — a weak
    // local summary loses fidelity for latency and cost.
    let is_local_engine = local_model || {
        let lower = url.to_ascii_lowercase();
        lower.contains("11434") || lower.contains("ollama")
    };
    if is_local_engine || deterministic_only || !turn_boundary {
        let structured = super::memory::compact_with_structured_memory_from_archive(
            history,
            keep_count,
            budget,
            &archival_source,
        );
        emit_compaction_metrics(
            raw_tokens,
            budget,
            "deterministic_record",
            false,
            history.len(),
        );
        return structured;
    }

    let summary_ok = force_compact_internal(
        client,
        url,
        model,
        history,
        summarize_count,
        Some(budget),
        Some(cancel_token),
        &archival_source,
    )
    .await
    .is_ok();

    if summary_ok {
        emit_compaction_metrics(raw_tokens, budget, "ai_summary", true, history.len());
        true
    } else {
        let structured = compact_with_structured_memory(history, keep_count, budget);
        emit_compaction_metrics(
            raw_tokens,
            budget,
            "deterministic_record",
            false,
            history.len(),
        );
        structured
    }
}

/// Compact history without making a provider request. Used for plan-backed
/// profiles whose authenticated transport is restricted to streaming
/// Responses inference.
pub async fn force_compact_with_budget_deterministic(
    history: &mut Vec<ChatMessage>,
    budget: Option<usize>,
) -> Result<(usize, usize), String> {
    let before_tokens: usize = history.iter().map(estimate_message_tokens).sum();
    let archival_source = history.clone();

    // Same allowance as the summarizing path, so a plan-backed `/compact`
    // keeps as much recent history as any other compaction.
    let summarize_count =
        retention_tail_start_for_manual(history, budget.unwrap_or(DEFAULT_PRUNE_TOKEN_THRESHOLD));
    if summarize_count < 1 {
        return Err("Not enough messages to compact.".to_string());
    }
    let keep_count = history.len().saturating_sub(summarize_count);
    let compacted = super::memory::compact_with_structured_memory_from_archive(
        history,
        keep_count,
        budget.unwrap_or(DEFAULT_PRUNE_TOKEN_THRESHOLD),
        &archival_source,
    );
    if !compacted {
        return Err("Deterministic local compaction could not reduce the history.".to_string());
    }
    let after_tokens: usize = history.iter().map(estimate_message_tokens).sum();
    Ok((before_tokens, after_tokens))
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

/// Record what compaction decided and how far it had to go.
///
/// `originating_path` names the branch that produced this event. The metrics
/// here describe real work only: the retired in-place prune passes used to
/// contribute fields that were always zero.
fn emit_compaction_metrics(
    before_tokens: usize,
    budget: usize,
    originating_path: &str,
    summarized: bool,
    messages_considered: usize,
) {
    crate::logger::operational_event(
        "context.compaction",
        serde_json::json!({
            "before_tokens": before_tokens,
            "budget": budget,
            "messages_considered": messages_considered,
            "summary_generated": summarized,
            "originating_path": originating_path,
        }),
    );
}

#[cfg(test)]
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
    let archival_source = history.clone();

    // Summarize everything before the shared retention tail, so manual
    // `/compact` and automatic compaction retain the same recent history.
    let summarize_count =
        retention_tail_start_for_manual(history, budget.unwrap_or(DEFAULT_PRUNE_TOKEN_THRESHOLD));
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
        &archival_source,
    )
    .await;
    let after_tokens: usize = history.iter().map(estimate_message_tokens).sum();
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
    archival_source: &[ChatMessage],
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
            !m.conversation_recap
                && !(m.role == "system" && m.content.starts_with(SUMMARY_MARKER))
                && !(m.role == "system" && m.content.starts_with(ORIGINAL_TASK_MARKER))
        })
        .collect();

    // Never re-summarize an earlier summary. Feeding it back through the model
    // on every pass is what makes a long session drift: each rewrite drops a
    // detail and the loss compounds. The earlier summary is carried forward
    // verbatim instead, and the model only sees the messages that came after
    // it, so the prefix also stays byte-stable for provider cache reuse.
    // A cut snapped back to the start of the only turn leaves nothing new to
    // summarize. Say so rather than report a compaction that changed nothing.
    if to_summarize.is_empty() {
        return Err("Not enough messages to compact.".to_string());
    }
    let mut summary = match generate_summary(client, url, model, &to_summarize, cancel_token).await
    {
        Some(delta) => match prior_summary.as_deref() {
            Some(prior) if !prior.trim().is_empty() => {
                format!("{prior}\n\n[Work since the previous compaction]\n{delta}")
            }
            _ => delta,
        },
        None => return Err("Failed to generate summary.".to_string()),
    };

    let archival_prefix = archival_source
        .get(..summarize_count)
        .ok_or_else(|| "archival history no longer matches compaction boundary".to_string())?;
    let history_archive = crate::config::archive_history_prefix(archival_prefix)?;
    let mut evidence_memory =
        super::memory::StructuredSessionMemory::extract_from_history(archival_prefix);
    evidence_memory.attach_archive_to_unlinked_reads(&history_archive);
    let read_evidence = evidence_memory.format_read_evidence(2_000);
    let command_evidence = evidence_memory.format_command_evidence(1_500);
    if !read_evidence.is_empty() || !command_evidence.is_empty() {
        summary.push_str("\n\n");
        summary.push_str(&read_evidence);
        summary.push_str(&command_evidence);
    }

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
    history.push(durable_compaction_message(
        &summary,
        &retained_tail,
        &history_archive,
    ));
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
    history_archive: &str,
) -> ChatMessage {
    ChatMessage::new(
        "system",
        format!(
            "{SUMMARY_MARKER}\n{summary}\n[Full compacted history prefix: {history_archive}; exact JSONL archive with one original message per line. Use zoom_context to recover exact details before acting on an incomplete summary.]\n[End Summary — the following messages are the most recent conversation]"
        ),
    )
    .with_compaction_boundary(compaction_boundary(summary, retained_tail, history_archive))
}

/// Build a typed deterministic record without changing its historical
/// non-summary presentation. Local providers intentionally skip the AI
/// summarizer, and callers rely on this record remaining a normal system note.
pub(crate) fn durable_compaction_record_message(
    record: &str,
    retained_tail: &[ChatMessage],
    history_archive: &str,
) -> ChatMessage {
    ChatMessage::new(
        "system",
        format!(
            "{record}\nFull compacted history prefix: {history_archive}; exact JSONL archive with one original message per line. Use zoom_context to recover exact details before acting on an incomplete summary."
        ),
    )
    .with_compaction_boundary(compaction_boundary(record, retained_tail, history_archive))
}

fn compaction_boundary(
    summary: &str,
    retained_tail: &[ChatMessage],
    history_archive: &str,
) -> CompactionBoundary {
    CompactionBoundary {
        version: COMPACTION_BOUNDARY_VERSION,
        summary: summary.to_string(),
        first_retained_entry: retained_tail.first().map(CompactionEntry::from_message),
        history_archive: Some(history_archive.to_string()),
    }
}

/// Prefix that marks a compaction summary message, used to detect and preserve
/// prior summaries during incremental compaction.
pub const SUMMARY_MARKER: &str = "[Session History Summary]";
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

/// Build a bounded prompt that pins the original task, then spends the
/// remaining space on the newest messages in chronological order.
///
/// An earlier summary is never fed back through the model: the caller carries
/// it forward verbatim and this input covers only the work since then.
pub(super) fn build_summary_input(messages: &[&ChatMessage]) -> String {
    let first_user_index = messages.iter().position(|message| message.role == "user");
    let mut input = String::with_capacity(SUMMARY_INPUT_MAX_BYTES);
    input.push_str("Summarize the work in this coding conversation.\n\n");

    if let Some(index) = first_user_index {
        input.push_str("Original user task (preserve this objective):\n");
        let rendered = render_summary_message(messages[index]);
        let remaining = SUMMARY_INPUT_MAX_BYTES.saturating_sub(input.len());
        input.push_str(truncate_utf8(&rendered, remaining));
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
    messages: &[&ChatMessage],
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Option<String> {
    // The earlier summary is carried forward verbatim by the caller, so the
    // model only reports the work that happened after it.
    let user_content = build_summary_input(messages);

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

    #[tokio::test]
    async fn ai_compaction_archives_the_exact_unpruned_prefix_before_rewrite() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let response = r#"{"choices":[{"message":{"content":"summary"}}]}"#;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 16 * 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response.len(),
                        response
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let mut old_output = ChatMessage::new(
            "tool",
            format!("legacy output {}", "raw evidence ".repeat(4_000)),
        );
        old_output.tool_call_id = Some("command-call".into());
        old_output.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "run_command".into(),
            command: Some("cargo test -p rustcode".into()),
            success: true,
            exit_code: Some(0),
            ..Default::default()
        });
        let mut history = vec![ChatMessage::new("user", "original task"), old_output];
        for turn in 0..20 {
            history.push(ChatMessage::new("user", format!("follow-up {turn}")));
            history.push(ChatMessage::new("assistant", format!("progress {turn}")));
        }
        let original = history.clone();

        force_compact_with_budget(
            &reqwest::Client::new(),
            &endpoint,
            "mock-model",
            &mut history,
            Some(10_000),
            None,
        )
        .await
        .expect("mock summary compaction");
        server.await.unwrap();

        let boundary = history[0]
            .compaction_boundary
            .as_ref()
            .expect("AI compaction boundary");
        let archive_path = boundary
            .history_archive
            .as_ref()
            .expect("AI compaction archives retired history");
        let archived = std::fs::read_to_string(archive_path).expect("exact archived JSONL");
        let archived_messages = archived
            .lines()
            .map(|line| serde_json::from_str::<ChatMessage>(line).expect("archived message"))
            .collect::<Vec<_>>();
        assert!(!archived_messages.is_empty());
        assert_eq!(
            archived_messages,
            original[..archived_messages.len()].to_vec()
        );
        assert!(archived[0..].contains("raw evidence ".repeat(4_000).as_str()));
        assert!(history[0].content.contains("Command evidence:"));
        assert!(history[0].content.contains("cargo test -p rustcode"));
        assert!(history[0].content.contains("\"exit_code\":0"));
        assert!(history[0].content.contains("command-call"));
    }

    #[tokio::test]
    async fn deterministic_plan_compaction_never_opens_provider_http() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let mut history = Vec::new();
        for turn in 0..6 {
            history.push(ChatMessage::new("user", format!("task {turn}")));
            history.push(ChatMessage::new("assistant", format!("work {turn}")));
            history.push(ChatMessage::new(
                "tool",
                format!("view_file: {}", "file evidence\n".repeat(1000)),
            ));
        }

        assert!(
            maybe_compact_with_local_policy_and_usage_mode(
                &reqwest::Client::new(),
                &endpoint,
                "chatgpt-plan-model",
                &mut history,
                5_000,
                &tokio_util::sync::CancellationToken::new(),
                false,
                None,
                true,
                true,
            )
            .await
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "plan-backed compaction must use deterministic local reduction"
        );
    }

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
            ..Default::default()
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
    fn retention_tail_keeps_an_absolute_allowance_not_a_window_share() {
        // A window large enough that the 20k allowance, not the 30% cap, binds.
        let budget = 500_000;
        let mut history = vec![ChatMessage::new("user", "original task")];
        // Each turn is a few dozen tokens, so the total well exceeds the
        // allowance and something must actually be compactable.
        for index in 0..2_000 {
            history.push(ChatMessage::new(
                "user",
                format!("turn {index}: {}", "context detail ".repeat(8)),
            ));
        }

        let start = retention_tail_start(&history, budget);
        let kept: usize = history[start..].iter().map(estimate_message_tokens).sum();

        assert!(start > 0, "the head must be compactable");
        assert!(
            kept <= KEEP_RECENT_TOKENS,
            "retained {kept} tokens exceeds the {KEEP_RECENT_TOKENS} allowance"
        );
        // A 30% share of this window would be 150k; the absolute figure wins.
        assert!(kept < budget / 4);
    }

    #[test]
    fn retention_tail_is_capped_on_a_small_window_and_never_empty() {
        let small = 8_000;
        let history: Vec<ChatMessage> = (0..200)
            .map(|index| ChatMessage::new("user", format!("message {index}")))
            .collect();

        let start = retention_tail_start(&history, small);
        let kept: usize = history[start..].iter().map(estimate_message_tokens).sum();

        // 30% of 8k binds well below the 20k allowance.
        assert!(kept <= (small as f64 * KEEP_RECENT_WINDOW_SHARE) as usize + 64);
        // The newest message always survives, even with no room.
        assert!(start < history.len());
        assert_eq!(
            retention_tail_start(&history[..1], 1),
            0,
            "a single message is retained rather than dropped"
        );
    }

    /// Mid-turn, compaction must never reach the provider: the summarizer
    /// waits for a turn boundary, and per-round relief uses the local
    /// deterministic record instead (#2215).
    #[tokio::test]
    async fn mid_turn_compaction_never_opens_provider_http() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let mut history = Vec::new();
        for turn in 0..6 {
            history.push(ChatMessage::new("user", format!("task {turn}")));
            history.push(ChatMessage::new("assistant", format!("work {turn}")));
            history.push(ChatMessage::new(
                "tool",
                format!("view_file: {}", "file evidence\n".repeat(1000)),
            ));
        }

        // Not a plan profile, not local: only the turn boundary gates this.
        assert!(
            maybe_compact_with_local_policy_and_usage_mode(
                &reqwest::Client::new(),
                &endpoint,
                "turn-boundary-model",
                &mut history,
                5_000,
                &tokio_util::sync::CancellationToken::new(),
                false,
                None,
                false,
                false,
            )
            .await,
            "mid-turn compaction must still relieve pressure locally"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "mid-turn compaction must not make a summarizer request"
        );
    }

    #[tokio::test]
    async fn benchmark_half_full_provider_window_keeps_dependency_evidence() {
        let mut history = vec![ChatMessage::new(
            "tool",
            format!("view_file: {}", "dependency source\n".repeat(3000)),
        )];
        for _ in 0..13 {
            history.push(ChatMessage::new("assistant", "inspect"));
        }
        let usage = TokenUsage {
            prompt_tokens: 57_734,
            completion_tokens: 100,
            total_tokens: 57_834,
            cached_tokens: None,
            ..Default::default()
        };
        history.last_mut().unwrap().token_usage = Some(usage.clone());
        let before = serde_json::to_string(&history).unwrap();
        assert!(
            !maybe_compact_with_local_policy_and_usage(
                &reqwest::Client::new(),
                "http://unused",
                "local",
                &mut history,
                106_880,
                &tokio_util::sync::CancellationToken::new(),
                true,
                Some(&usage),
            )
            .await
        );
        assert_eq!(serde_json::to_string(&history).unwrap(), before);
    }

    #[tokio::test]
    async fn measured_prefix_pressure_uses_head_compaction_without_rewriting() {
        // Six full turns so the head holds enough summarizable messages for
        // the FIFO path once in-place excerpting is retired.
        let mut history = Vec::new();
        for turn in 0..6 {
            history.push(ChatMessage::new("user", format!("task {turn}")));
            history.push(ChatMessage::new("assistant", format!("work {turn}")));
            history.push(ChatMessage::new(
                "tool",
                format!("view_file: {}", "dependency source\n".repeat(1500)),
            ));
        }
        let usage = TokenUsage {
            prompt_tokens: 21_000,
            completion_tokens: 100,
            total_tokens: 21_100,
            cached_tokens: None,
            ..Default::default()
        };
        history.last_mut().unwrap().token_usage = Some(usage.clone());
        let original = history.clone();
        // #985: no historical message may be rewritten to an excerpt. The
        // summarizer endpoint is unreachable, so the deterministic
        // fixed-record head compaction absorbs the cut instead.
        assert!(
            maybe_compact_with_local_policy_and_usage(
                &reqwest::Client::new(),
                "http://unused",
                "local",
                &mut history,
                20_000,
                &tokio_util::sync::CancellationToken::new(),
                false,
                Some(&usage),
            )
            .await
        );
        assert!(
            history.len() < original.len(),
            "head compaction must absorb the cut"
        );
        assert!(
            history[0]
                .content
                .starts_with(crate::network::compaction::STRUCTURED_MEMORY_MARKER)
        );
        assert!(
            history[0].compaction_boundary.is_some(),
            "head compaction must persist a typed boundary"
        );
        // The retained tail is the original suffix, byte-identical.
        assert_eq!(
            history[1..],
            original[original.len() - (history.len() - 1)..]
        );
        assert!(
            !history
                .iter()
                .any(|message| message.content.contains("Tool Output Truncated")),
            "no retained message may be rewritten in place"
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
        read.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "view_file".into(),
            success: true,
            completeness: rustcode_core::ToolResultCompleteness::LineTruncated,
            workspace_epoch: Some("epoch-1".into()),
            workspace_generation: Some(9),
            evidence_hash: Some("read-hash".into()),
            full_output_artifact: Some("artifact://read-output".into()),
            inspection: Some(rustcode_core::InspectionResultMetadata {
                requested_path: Some("src/lib.rs".into()),
                returned_path: Some("src/lib.rs".into()),
                returned_range: Some(rustcode_core::InspectionRange {
                    start: Some(3),
                    end: Some(7),
                }),
                delivered_ranges: vec![rustcode_core::InspectionRange {
                    start: Some(3),
                    end: Some(7),
                }],
                fingerprint: "fingerprint".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let mut write = ChatMessage::new("tool", "write_to_file: wrote it");
        write.tool_result = Some(crate::app::ToolResultRecord {
            workspace_generation: None,
            workspace_epoch: None,
            evidence_hash: None,
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
        assert!(record.contains("Inspected file paths: src/lib.rs"));
        assert!(record.contains("\"path\":\"src/lib.rs\""));
        assert!(record.contains("\"start\":3"));
        assert!(record.contains("\"end\":7"));
        assert!(record.contains("read-1"));
        assert!(record.contains("read-hash"));
        assert!(record.contains("artifact://read-output"));
        assert!(!record.contains("tested"));
        assert!(!record.contains("runtime-validated"));
    }
}
