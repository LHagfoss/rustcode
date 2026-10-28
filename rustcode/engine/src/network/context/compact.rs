use super::memory::compact_with_structured_memory;
use super::prune::{DEFAULT_PRUNE_TOKEN_THRESHOLD, KEEP_RECENT_TURNS, prune_floor};
use super::tokens::estimate_message_tokens;
use crate::app::{ChatMessage, CompactionBoundary, CompactionEntry, TokenUsage};
use std::future::Future;
use std::time::Duration;

pub(super) const SUMMARY_INPUT_MAX_BYTES: usize = 64 * 1024;

/// Provider output is requested at 2048 tokens; 16 KiB is a generous defensive
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
    valid_compaction_boundary(history, recent_tail_start(history, budget))
}

/// Characters an old tool result keeps when a long turn has to make room.
const SHORTENED_TOOL_RESULT_CHARS: usize = 400;

/// How far below the budget shortening goes, so it does not have to run again
/// on the very next round (each run changes the prompt prefix and costs the
/// provider cache).
const SHORTENED_TARGET_SHARE: f64 = 0.75;

const SHORTENED_TOOL_RESULT_NOTE: &str = "more characters of this result were dropped to keep the turn within the context window; run the tool again if you still need them]";

/// Relief for a single turn that outgrew the window on its own (#2475).
///
/// Compaction cuts at a user request, so a turn holding the whole history
/// leaves nothing before the cut. What fills such a turn is its tool output:
/// files read, command output, edit diffs. Results older than the recent tail
/// are shortened in place, oldest first, to their first few hundred
/// characters and a note saying how to get the rest, until the history is
/// comfortably under `budget`. Messages are never removed, so every tool call
/// keeps its result, and the recent tail stays byte-identical. `tokens` is
/// the current estimate of the history. True when anything was shortened.
pub(crate) fn shorten_old_tool_output(
    history: &mut [ChatMessage],
    tokens: usize,
    budget: usize,
) -> bool {
    let target = (budget as f64 * SHORTENED_TARGET_SHARE) as usize;
    let tail = recent_tail_start(history, budget);
    let mut tokens = tokens;
    let mut changed = false;
    for message in history[..tail].iter_mut() {
        if tokens <= target {
            break;
        }
        if message.role != "tool" || message.content.ends_with(SHORTENED_TOOL_RESULT_NOTE) {
            continue;
        }
        let length = message.content.chars().count();
        if length <= SHORTENED_TOOL_RESULT_CHARS * 2 {
            continue;
        }
        let before = estimate_message_tokens(message);
        let kept: String = message
            .content
            .chars()
            .take(SHORTENED_TOOL_RESULT_CHARS)
            .collect();
        message.content = format!(
            "{kept}\n[… {} {SHORTENED_TOOL_RESULT_NOTE}",
            length - SHORTENED_TOOL_RESULT_CHARS
        );
        tokens = tokens.saturating_sub(before.saturating_sub(estimate_message_tokens(message)));
        changed = true;
    }
    changed
}

/// Index of the first message in the recent allowance, without snapping back
/// to a user request: shortening a result in place never separates it from
/// its call, so the cut may fall inside a turn.
fn recent_tail_start(history: &[ChatMessage], budget: usize) -> usize {
    let limit = ((budget as f64 * KEEP_RECENT_WINDOW_SHARE) as usize)
        .min(KEEP_RECENT_TOKENS)
        .max(1);
    let mut tokens = 0usize;
    let mut start = history.len();
    for index in (0..history.len()).rev() {
        let message_tokens = estimate_message_tokens(&history[index]);
        if start != history.len() && tokens.saturating_add(message_tokens) > limit {
            break;
        }
        tokens = tokens.saturating_add(message_tokens);
        start = index;
    }
    start
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

/// Output requested from the summarizer. A handoff covers goal, constraints,
/// decisions, files, verification and next steps; 1024 tokens cut it short.
const SUMMARY_MAX_TOKENS: u32 = 2_048;

/// Size the carried handoff may reach before it is consolidated into one
/// record. Roughly three thousand tokens: small next to the retained tail.
const CARRIED_SUMMARY_MAX_BYTES: usize = 12 * 1024;

/// How much of the earlier handoff the summarizer sees as context, so the new
/// section reports what changed instead of repeating it.
const PRIOR_SUMMARY_CONTEXT_MAX_BYTES: usize = 8 * 1024;

/// Heading of each section added by a later compaction.
const WORK_SINCE_MARKER: &str = "[Work since the previous compaction]";

/// Headings of the evidence lists appended after the narrative.
const EVIDENCE_HEADINGS: [&str; 2] = ["Command evidence (", "Read evidence ("];

const HANDOFF_INSTRUCTIONS: &str = "You write the handoff that lets a coding agent continue a session after its earlier messages are removed from context. The agent will see only your record, a few pinned user requests, and the most recent raw messages. Write a concise bullet-point record under these headings, omitting a heading only when nothing belongs there: Goal (the original objective); Current task (what the latest user request asks for now); User constraints and instructions (keep the user's wording); Decisions (with the reason); Files changed; Files inspected; Verified (only what a command, test or tool result in the conversation showed, naming it); Not verified or assumed; Failures and attempted fixes; Unresolved work and risks; Next steps (concrete actions). Keep exact file paths, commands, error text and outcomes that still matter. Never invent facts, never present a guess as verified, and do not include tool call syntax, JSON, secrets, source-code dumps or full command output.";

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
        // One turn holds (nearly) the whole history, so there is no earlier
        // turn to cut at. Its own old tool output is what fills the window:
        // shorten that rather than let the turn run into the preflight stop
        // (#2475).
        if shorten_old_tool_output(history, raw_tokens, budget) {
            emit_compaction_metrics(
                raw_tokens,
                budget,
                "turn_tool_output_shortened",
                false,
                history.len(),
            );
            return true;
        }
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
        })
        // Evidence lists are rebuilt from each pass's own archive; carrying
        // the old ones forward would repeat them on every compaction.
        .map(|summary| carried_narrative(&summary).to_string());

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
    let prior_summary = prior_summary.filter(|prior| !prior.trim().is_empty());
    let delta = generate_summary(
        client,
        url,
        model,
        &to_summarize,
        prior_summary.as_deref(),
        cancel_token,
    )
    .await
    .ok_or_else(|| "Failed to generate summary.".to_string())?;
    let mut summary = match prior_summary.as_deref() {
        Some(prior) => {
            let carried = format!("{prior}\n\n{WORK_SINCE_MARKER}\n{delta}");
            if carried.len() <= CARRIED_SUMMARY_MAX_BYTES {
                carried
            } else {
                // Appending a section per pass keeps earlier facts verbatim but
                // grows without bound over a long session. Past the cap, merge
                // once into a single record; if that fails, keep the first
                // section (goal and constraints) and the newest work.
                match consolidate_summary(client, url, model, &carried, cancel_token).await {
                    Some(merged) => merged,
                    None => trim_carried_sections(&carried, CARRIED_SUMMARY_MAX_BYTES),
                }
            }
        }
        None => delta,
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
        summary.push_str(&command_evidence);
        summary.push_str(&read_evidence);
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
pub(super) use rustcode_session::ORIGINAL_TASK_MARKER;
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

    if history[start].is_user_request() {
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
        .rposition(ChatMessage::is_user_request)
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
        let content = if message.is_user_request() {
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
pub(super) fn build_summary_input(messages: &[&ChatMessage], prior: Option<&str>) -> String {
    let first_user_index = messages.iter().position(|message| message.role == "user");
    let mut input = String::with_capacity(SUMMARY_INPUT_MAX_BYTES);
    match prior {
        Some(prior) => {
            input.push_str("An earlier handoff already covers the start of this session. It is kept as it is; do not repeat it. Write only what the messages below add or change, including any earlier item they complete or overturn.\n\nEarlier handoff (context only):\n");
            input.push_str(truncate_utf8(prior, PRIOR_SUMMARY_CONTEXT_MAX_BYTES));
            input.push_str("\n\n");
        }
        None => input.push_str("Summarize the work in this coding conversation.\n\n"),
    }

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
    prior: Option<&str>,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Option<String> {
    // The earlier summary is carried forward verbatim by the caller, so the
    // model only reports the work that happened after it.
    let user_content = build_summary_input(messages, prior);
    request_summary(client, url, model, &user_content, cancel_token).await
}

/// Merge a carried handoff that outgrew its cap into one record. This is the
/// only pass that rewrites earlier summary text, and it runs once per cap
/// crossing rather than on every compaction.
async fn consolidate_summary(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    carried: &str,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Option<String> {
    let input = format!(
        "These are handoff sections written at successive compactions of one session, oldest first. Merge them into a single record. Keep the Goal and the user's constraints and instructions word for word. Where a later section completes or overturns an earlier item, keep only the current state. Drop nothing that still matters.\n\n{}",
        truncate_utf8(carried, SUMMARY_INPUT_MAX_BYTES)
    );
    let merged = request_summary(client, url, model, &input, cancel_token).await?;
    // A merge that leaves little room would be merged again on the very next
    // pass, which is the repeated rewriting this cap exists to avoid.
    (merged.len() <= CARRIED_SUMMARY_MAX_BYTES / 2).then_some(merged)
}

/// Fallback when consolidation fails: keep the first section, which holds the
/// goal and constraints, then as many of the newest sections as fit.
pub(super) fn trim_carried_sections(carried: &str, max_bytes: usize) -> String {
    let separator = format!("\n\n{WORK_SINCE_MARKER}\n");
    let mut sections = carried.split(separator.as_str());
    let first = sections.next().unwrap_or_default();
    let later: Vec<&str> = sections.collect();
    let omitted_note = |count: usize| {
        format!(
            "\n\n[{count} earlier work sections omitted here; the exact history is in the compacted history archives]"
        )
    };
    let first = truncate_utf8(first, max_bytes / 2);
    let mut kept: Vec<&str> = Vec::new();
    let mut used = first.len() + omitted_note(later.len()).len();
    for section in later.iter().rev() {
        let cost = separator.len() + section.len();
        if used + cost > max_bytes {
            break;
        }
        used += cost;
        kept.push(section);
    }
    let mut out = first.to_string();
    let omitted = later.len() - kept.len();
    if omitted > 0 {
        out.push_str(&omitted_note(omitted));
    }
    for section in kept.into_iter().rev() {
        out.push_str(&separator);
        out.push_str(section);
    }
    out
}

/// The narrative part of a handoff, without the evidence lists appended after
/// it.
pub(super) fn carried_narrative(summary: &str) -> &str {
    let end = EVIDENCE_HEADINGS
        .iter()
        .filter_map(|heading| summary.find(&format!("\n\n{heading}")))
        .min()
        .unwrap_or(summary.len());
    summary[..end].trim_end()
}

tokio::task_local! {
    /// The session a compaction summarizes for, when it must be asked through
    /// the session's own transport: a ChatGPT or Claude CLI plan has no plain
    /// chat-completions endpoint for the request below (#2548).
    static SUMMARY_SESSION: std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>;
}

/// Run a compaction whose summaries go through `session`'s transport, as its
/// turns do, instead of a plain request to its URL.
pub(crate) async fn with_session_summarizer<F: Future>(
    session: std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    compaction: F,
) -> F::Output {
    SUMMARY_SESSION.scope(session, compaction).await
}

/// A plan model streams and may reason first; a summary gets longer than a
/// plain request's 25 seconds.
const SESSION_SUMMARY_TIMEOUT: Duration = Duration::from_secs(120);

/// The summary asked of the session's model on its own transport. A scratch
/// state with a temporary id keeps the request out of the session's history,
/// stream and usage, as a session title's request does.
async fn request_session_summary(
    client: &reqwest::Client,
    session: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    user_content: &str,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Option<String> {
    let request_id = format!(
        "compaction-summary-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let (scratch, url, model) = {
        let live = session.lock().await;
        let workspace = live
            .effective_workspace_root()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let mut scratch =
            crate::app::AppState::new_with_workspace_session(&workspace, Some(&request_id));
        scratch.config = live.config.clone();
        scratch.api_base_url = live.api_base_url.clone();
        scratch.model_name = live.model_name.clone();
        let (url, model) = (scratch.api_base_url.clone(), scratch.model_name.clone());
        (
            std::sync::Arc::new(tokio::sync::Mutex::new(scratch)),
            url,
            model,
        )
    };
    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(crate::network::StreamBuffer::new()));
    let cancel = cancel_token.map_or_else(tokio_util::sync::CancellationToken::new, |token| {
        token.child_token()
    });
    let messages = vec![
        serde_json::json!({"role": "system", "content": HANDOFF_INSTRUCTIONS}),
        serde_json::json!({"role": "user", "content": user_content}),
    ];
    let request = crate::network::stream_request(
        client,
        scratch,
        cancel.clone(),
        &url,
        &model,
        messages,
        std::sync::Arc::clone(&buffer),
        true,
        false,
        crate::network::stream_request::ThinkingMode::Disabled,
        crate::tools::ToolSchemaPolicy::root(false),
        Some(&request_id),
        None,
    );
    let finished = tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        result = tokio::time::timeout(SESSION_SUMMARY_TIMEOUT, request) => {
            matches!(result, Ok(Ok(_)))
        }
    };
    if !finished {
        cancel.cancel();
        return None;
    }
    let content = buffer.lock().await.content.clone();
    let content = rustcode_tool_protocol::text::strip_think_blocks(&content);
    parse_summary_response(&serde_json::json!({
        "choices": [{"message": {"content": content}}]
    }))
}

async fn request_summary(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    user_content: &str,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Option<String> {
    if let Ok(session) = SUMMARY_SESSION.try_with(std::sync::Arc::clone) {
        return request_session_summary(client, &session, user_content, cancel_token).await;
    }
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": HANDOFF_INSTRUCTIONS
            },
            {
                "role": "user",
                "content": user_content
            }
        ],
        "stream": false,
        "temperature": 0.3,
        "max_tokens": SUMMARY_MAX_TOKENS,
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

    /// One long turn over budget has no earlier turn to compact; its old
    /// tool output is shortened instead, so it can keep going (#2475).
    #[test]
    fn a_long_single_turn_sheds_old_tool_output_and_keeps_its_tail() {
        let mut history = vec![ChatMessage::new("user", "fix it all, then deploy")];
        for index in 0..60 {
            let mut call = ChatMessage::new("assistant", format!("reading file {index}"));
            call.tool_calls = vec![crate::app::ToolCallRef {
                id: format!("call-{index}"),
                name: "view_file".into(),
                arguments: "{}".into(),
            }];
            history.push(call);
            let mut result = ChatMessage::new(
                "tool",
                format!(
                    "view_file: file {index}\n{}",
                    "let value = 1;\n".repeat(400)
                ),
            );
            result.tool_call_id = Some(format!("call-{index}"));
            history.push(result);
        }
        let budget = 40_000;
        let tokens: usize = history.iter().map(estimate_message_tokens).sum();
        assert!(tokens > budget, "the turn must be over budget ({tokens})");
        assert!(
            retention_tail_start(&history, budget) < 4,
            "nothing to compact by turns"
        );
        let tail = recent_tail_start(&history, budget);
        let recent = history[tail..].to_vec();

        assert!(shorten_old_tool_output(&mut history, tokens, budget));

        let after: usize = history.iter().map(estimate_message_tokens).sum();
        assert!(
            after <= (budget as f64 * SHORTENED_TARGET_SHARE) as usize,
            "{after}"
        );
        assert_eq!(history.len(), 121, "no message is removed");
        assert_eq!(
            &history[tail..],
            recent.as_slice(),
            "the recent tail is untouched"
        );
        assert!(history[2].content.starts_with("view_file: file 0"));
        assert!(history[2].content.ends_with(SHORTENED_TOOL_RESULT_NOTE));
        // Already short, so a second pass changes nothing.
        let shortened = history.clone();
        shorten_old_tool_output(&mut history, after, budget);
        assert_eq!(history, shortened);
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
