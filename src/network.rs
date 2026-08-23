use crate::app::{AppState, AppStatus, ChatMessage};
use std::sync::Arc;
use tokio::sync::Mutex;

#[path = "network/compaction.rs"]
pub(crate) mod compaction;

#[path = "network/retry.rs"]
pub(crate) mod retry;

#[path = "network/loop_detect.rs"]
pub(crate) mod loop_detect;

#[path = "network/helpers.rs"]
pub(crate) mod helpers;
pub(crate) use helpers::{classify_tool_msg, count_tokens, parse_sse_line};

#[path = "network/messages.rs"]
pub(crate) mod messages;
pub(crate) use messages::{
    attach_request_context_tail, inject_system_reminder, trim_msgs_to_budget,
};

#[path = "network/text.rs"]
pub(crate) mod text;
use text::{is_cut_off, strip_think_blocks};

#[path = "network/stream.rs"]
pub(crate) mod stream;
pub(crate) use stream::StreamBuffer;

#[path = "network/stream_request.rs"]
pub(crate) mod stream_request;
pub use stream_request::stream_request;
pub(crate) use stream_request::{
    parse_native_tool_arguments, request_debug_log_line, request_log_summary,
};

#[path = "network/output.rs"]
pub(crate) mod output;

#[path = "network/events.rs"]
pub(crate) mod events;
pub(crate) use events::{ToolResult, ToolResultMetadata};

#[path = "network/ui_adapter.rs"]
pub(crate) mod ui_adapter;
pub(crate) use ui_adapter::{AgentUiEvent, AgentUiEventReceiver, AgentUiEventSender};

#[path = "network/tool_exec.rs"]
pub(crate) mod tool_exec;
pub(crate) use tool_exec::{
    augmented_path, bounded_tool_result_history_message, confirm_and_execute, execute_tool_batch,
    extract_diff_block, final_tool_diff, finalize_tool_result, get_diff_preview,
    get_tool_project_root, resolve_bin, subagent_tool_history_message, tool_result_from_execution,
    tool_result_history_message, tool_result_precludes_preview_fallback,
};

#[path = "network/turn_engine.rs"]
pub(crate) mod turn_engine;
pub(crate) use turn_engine::ToolFenceCounter;
pub use turn_engine::{TurnContext, process_queue_orchestrator, run_agent_turn};
pub(crate) use turn_engine::process_queue_orchestrator_with_ui_events;

#[path = "network/lifecycle.rs"]
pub(crate) mod lifecycle;

#[path = "network/history.rs"]
pub(crate) mod history;

#[path = "network/runner.rs"]
pub(crate) mod runner;

#[path = "network/policy.rs"]
pub(crate) mod policy;

#[path = "network/verification.rs"]
pub(crate) mod verification;

#[path = "network/image_fallback.rs"]
pub(crate) mod image_fallback;

#[path = "network/payload.rs"]
pub(crate) mod payload;
pub use payload::{fetch_model_quota, parse_multimodal_content};

#[path = "network/compiler.rs"]
pub(crate) mod compiler;
pub(crate) use compiler::{
    append_compiler_diagnostics, cached_compiler_check, compiler_diagnostic_fingerprint,
    compiler_diagnostics_with_snippets, run_compiler_check, update_compiler_diagnostic_streak,
};

#[path = "network/subagents.rs"]
pub(crate) mod subagents;
#[allow(unused_imports)]
pub(crate) use subagents::{handle_agent_tool, run_subagent, set_subagent_status};

#[path = "network/title.rs"]
pub(crate) mod title;
pub use title::generate_title;
#[allow(unused_imports)]
pub(crate) use title::{record_prompt_to_history, spawn_title_generation};

#[path = "network/context_tail.rs"]
pub(crate) mod context_tail;
pub(crate) use context_tail::{
    build_dynamic_context_tail, build_dynamic_context_tail_with_memory,
    build_volatile_context_block, format_read_file_context_entry, prepend_skill_routing_hint,
};

/// Injected as a system directive for the final wrap-up turn after a loop is
/// detected. Disables tools and forces a prose answer so the user gets a
/// summary instead of a silently aborted session. Ported from opencode's
/// `MAX_STEPS_PROMPT`.
pub(crate) const FORCE_ANSWER_PROMPT: &str = "CRITICAL — you are stuck in a loop. Tools are now DISABLED for this turn. \
Do NOT emit any tool calls (no reads, writes, edits, searches). Respond with TEXT ONLY, and include: \
a short statement that you stopped to avoid looping, a summary of what you found or accomplished so far, \
any remaining tasks, and a recommendation for what to do next. This overrides all other instructions.";

pub(crate) const LOOP_RECOVERY_PROMPT: &str = "The previous tool action repeated without making progress. Tools remain enabled for one recovery attempt. \
Do not repeat the same tool call or the same exact edit. Re-read a broader file region or use grep to verify exact target content, \
then use a grounded approach. If emitting a tool call in this recovery attempt, output the ```tool block cleanly. \
If the requested change is already present or cannot be applied safely, explain that instead of retrying. This is the final recovery attempt.";

pub(crate) const REASONING_LOOP_RECOVERY_PROMPT: &str = "[Your reasoning became repetitive without making progress. Thinking is disabled for this recovery attempt. Do not read files again or restate the requirements. If the user requested workspace changes, emit exactly one mutating tool call now using what you already learned. Otherwise, give the direct final answer.]";

pub(crate) const MAX_LOOP_RECOVERY_ROUNDS: u8 = 1;
pub(crate) const MAX_REASONING_RECOVERY_ROUNDS: u8 = 1;

/// Safety budgets for a single agent turn. These are deliberately generous —
/// the goal is to catch a runaway session (the benchmark that motivated this
/// hit 106 rounds with no hard stop), not to cut off healthy long-running
/// work. Any one signal firing is enough: a session that is genuinely
/// healthy on every other axis but has spent 500k tokens or 40 rounds has
/// stopped being worth running unattended.
const MAX_TURN_TOKEN_BUDGET: u64 = 5_000_000;
/// A tool that reports success without changing anything (already-applied
/// edits, no-op runs) does not count as progress, so this escalates much
/// faster than the round budget when the agent is just spinning.
const MAX_CONSECUTIVE_NO_PROGRESS: usize = 8;
const MAX_CONSECUTIVE_FAILED_MUTATIONS: usize = 5;
const MAX_CONSECUTIVE_COMPILER_ERROR_GATES: usize = 5;
const MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS: usize = 4;
/// A malformed tool-call block is a protocol error, not a failed mutation —
/// the model tried to call a tool and produced text the harness couldn't
/// parse at all. Retrying blindly forever wastes rounds and tokens on a
/// model that isn't going to self-correct, so this budget trips much faster
/// than the general round cap.
const MAX_CONSECUTIVE_MALFORMED_CALLS: usize = 4;

/// Which safety budget stopped the turn, with enough detail for the final
/// summary to name the exact limit that was hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnBudgetLimit {
    ToolRounds(usize),
    Tokens(u64),
    NoProgress(usize),
    FailedMutations(usize),
    CompilerErrorGates(usize),
    CompilerDiagnostics(usize),
    MalformedCalls(usize),
}

impl std::fmt::Display for TurnBudgetLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TurnBudgetLimit::ToolRounds(n) => write!(f, "maximum tool rounds reached ({n})"),
            TurnBudgetLimit::Tokens(n) => write!(f, "maximum token budget reached (~{n} tokens)"),
            TurnBudgetLimit::NoProgress(n) => write!(
                f,
                "{n} consecutive tool results with no meaningful progress (no-op or unchanged edits)"
            ),
            TurnBudgetLimit::FailedMutations(n) => {
                write!(f, "{n} consecutive failed edits")
            }
            TurnBudgetLimit::CompilerErrorGates(n) => {
                write!(
                    f,
                    "{n} consecutive completion attempts with the build still broken"
                )
            }
            TurnBudgetLimit::CompilerDiagnostics(n) => {
                write!(
                    f,
                    "{n} consecutive edits left the same compiler diagnostics unchanged"
                )
            }
            TurnBudgetLimit::MalformedCalls(n) => {
                write!(
                    f,
                    "{n} consecutive malformed tool-call blocks the harness could not parse"
                )
            }
        }
    }
}

/// Adds this round's token usage onto the turn's running total. The
/// provider's `usage` field is per-response (this round's full prompt +
/// completion), not a cumulative conversation total, so it is correct to sum
/// it round over round rather than overwrite — overwriting would let a
/// 30-round turn look like it spent only what the last round used, silently
/// defeating the token safety budget. Falls back to a character-based
/// estimate for providers that don't report usage.
pub(crate) fn accumulate_tokens_used(
    current: u64,
    reported_this_round: Option<u64>,
    content: &str,
) -> u64 {
    current.saturating_add(reported_this_round.unwrap_or_else(|| count_tokens(content) as u64))
}

/// Checks every budget signal and returns the first one that has been
/// exceeded, if any. Order matters only for which reason is reported when
/// several trip on the same round — all are equally terminal.
pub(crate) fn turn_budget_exceeded(ctx: &TurnContext) -> Option<TurnBudgetLimit> {
    if ctx.tokens_used >= MAX_TURN_TOKEN_BUDGET {
        return Some(TurnBudgetLimit::Tokens(ctx.tokens_used));
    }
    if ctx.consecutive_no_progress >= MAX_CONSECUTIVE_NO_PROGRESS {
        return Some(TurnBudgetLimit::NoProgress(ctx.consecutive_no_progress));
    }
    if ctx.consecutive_failed_mutations >= MAX_CONSECUTIVE_FAILED_MUTATIONS {
        return Some(TurnBudgetLimit::FailedMutations(
            ctx.consecutive_failed_mutations,
        ));
    }
    if ctx.consecutive_compiler_error_gates >= MAX_CONSECUTIVE_COMPILER_ERROR_GATES {
        return Some(TurnBudgetLimit::CompilerErrorGates(
            ctx.consecutive_compiler_error_gates,
        ));
    }
    if ctx.consecutive_compiler_diagnostics >= MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS {
        return Some(TurnBudgetLimit::CompilerDiagnostics(
            ctx.consecutive_compiler_diagnostics,
        ));
    }
    if ctx.consecutive_malformed_calls >= MAX_CONSECUTIVE_MALFORMED_CALLS {
        return Some(TurnBudgetLimit::MalformedCalls(
            ctx.consecutive_malformed_calls,
        ));
    }
    // The round count is intentionally last: evidence-aware recovery and
    // focused failure guards get a chance to act first. This remains the hard
    // final backstop for a model that keeps producing novel but unproductive
    // actions which evade the more specific deterministic signals.
    if ctx.tool_rounds >= ctx.max_tool_rounds {
        return Some(TurnBudgetLimit::ToolRounds(ctx.tool_rounds));
    }
    None
}

/// Stop the turn safely when a budget has been exceeded: never claim
/// completion, leave the transcript intact, and explain exactly which limit
/// was hit so the user can decide whether to resume.
pub(crate) async fn stop_turn_for_budget(
    _state: &Arc<Mutex<AppState>>,
    ctx: &mut TurnContext,
    limit: TurnBudgetLimit,
) -> bool {
    dbg_log!("Turn budget exceeded: {}", limit);
    crate::logger::operational_event(
        "turn.budget_exceeded",
        serde_json::json!({
            "limit": limit.to_string(),
            "tool_rounds": ctx.tool_rounds,
            "elapsed_secs": ctx.turn_started_at.elapsed().as_secs(),
            "tokens_used": ctx.tokens_used,
            "failed_mutations": ctx.failed_mutations,
        }),
    );
    let summary = format!(
        "[harness: stopped after {} tool round(s) — {limit}. The task is NOT complete. \
         Review the transcript above; if the remaining work is still valid, resume it in a new turn.]",
        ctx.tool_rounds
    );
    ctx.final_content = summary;
    ctx.task_completed = false;
    ctx.budget_stopped = Some(limit.to_string());
    ctx.stop_reason = Some(lifecycle::StopReason::BudgetExceeded(limit.to_string()));
    let mut s = _state.lock().await;
    s.continuous_mode = false;
    s.status = AppStatus::Idle;
    drop(s);
    false
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LoopRecoveryAction {
    Recover,
    ForceFinal,
}

pub(crate) fn loop_recovery_action(attempts: u8) -> LoopRecoveryAction {
    if attempts < MAX_LOOP_RECOVERY_ROUNDS {
        LoopRecoveryAction::Recover
    } else {
        LoopRecoveryAction::ForceFinal
    }
}

pub(crate) fn reasoning_loop_recovery_action(attempts: u8) -> LoopRecoveryAction {
    if attempts < MAX_REASONING_RECOVERY_ROUNDS {
        LoopRecoveryAction::Recover
    } else {
        LoopRecoveryAction::ForceFinal
    }
}

/// Push a loop warning, replacing the previous one if it's still the last
/// history entry — a model stuck in a loop would otherwise collect one
/// near-identical warning per round, crowding out the transcript.
pub(crate) fn push_or_replace_loop_warning(history: &mut Vec<ChatMessage>, text: String) {
    if let Some(last) = history.last_mut()
        && last.role == "system"
        && last.content.starts_with("[Loop warning:")
    {
        last.content = text;
    } else {
        history.push(ChatMessage::new("system", text));
    }
}

/// True when a mutating tool's result reflects real forward progress —
/// not merely a reported success. A failed edit changed nothing, and
/// neither did an idempotent no-op (PR #306's "already applied" signal
/// for an edit that was already applied). Both cases must be treated the
/// same by every consumer that gates on "did this round move the task
/// forward": the no-progress safety budget, and the loop detector's
/// reset-on-progress rule. Otherwise a model that keeps re-submitting an
/// already-applied edit gets a "success" every round that resets the loop
/// detector, so it never trips — defeating the detector entirely.
pub(crate) fn mutation_made_progress(success: bool, content: &str) -> bool {
    if !success {
        return false;
    }
    let lower = content.trim_start().to_ascii_lowercase();
    !lower.starts_with("error") && !lower.contains("already applied")
}

pub(crate) fn failure_replan_message(tool: &str, category: &str, repeats: usize) -> String {
    format!(
        "[Replan required: {repeats} equivalent mutation attempts for '{tool}' ({category}) failed. These failed attempts changed no files. Do not retry the same edit. Inspect the current workspace and use a materially different safe approach. If inspection shows the change still cannot be applied safely, explain the exact blocker to the user.]"
    )
}

/// True when a tool result has already been reduced to a stub (nothing left to prune).
fn is_fully_stubbed(m: &ChatMessage) -> bool {
    let rest = m
        .content
        .split_once(':')
        .map(|x| x.1)
        .unwrap_or("")
        .trim_start();
    rest.starts_with("[Tool output truncated") || rest.starts_with("[superseded")
}

/// Reduce one tool message a single notch toward a stub (full → 2 lines → fully
/// stubbed). Returns the new token count. Idempotent on already-stubbed messages.
async fn reduce_tool_msg(m: &mut ChatMessage, current_tokens: u32) -> u32 {
    let tool_name = m
        .content
        .split(':')
        .next()
        .unwrap_or("tool")
        .trim()
        .to_string();
    let rest = m
        .content
        .split_once(':')
        .map(|x| x.1)
        .unwrap_or("")
        .to_string();

    if is_fully_stubbed(m) {
        return current_tokens;
    }

    let lines: Vec<&str> = rest.lines().collect();
    if lines.len() > 2 {
        let truncated = format!("{}: {}\n{}", tool_name, lines[0], lines[1]);
        let t = count_tokens(&truncated);
        m.content = truncated;
        t
    } else {
        let stubbed = format!(
            "{}: [Tool output truncated: {} tokens pruned to maintain context window]",
            tool_name, current_tokens
        );
        count_tokens(&stubbed)
    }
}

/// Repeatedly reduce the oldest non-stubbed tool result of `class` until under budget
/// or the class is exhausted. Mutates `history`, `tokens`, and `total` in place.
async fn prune_class(
    history: &mut [ChatMessage],
    tokens: &mut [u32],
    total: &mut u32,
    budget: u32,
    class: &'static str,
) {
    let candidates: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, m)| classify_tool_msg(m) == Some(class) && !is_fully_stubbed(m))
        .map(|(i, _)| i)
        .collect();

    for idx in candidates {
        while *total > budget && !is_fully_stubbed(&history[idx]) {
            let before = tokens[idx];
            let new_t = reduce_tool_msg(&mut history[idx], before).await;
            if new_t >= before {
                // Defensive: nothing more we can do here.
                return;
            }
            *total = total.saturating_sub(before).saturating_add(new_t);
            tokens[idx] = new_t;
        }
        if *total <= budget {
            return;
        }
    }
}

const DETERMINISTIC_RECORD_MAX_CHARS: usize = 6_000;

fn compact_history_deterministically(history: &mut Vec<ChatMessage>, budget: u32) -> bool {
    if history.len() < 4 {
        return false;
    }

    let keep_target = (budget / 3).max(64);
    let mut suffix_tokens = 0u32;
    let mut suffix_messages = 0usize;
    let mut suffix_start = history.len();
    for index in (0..history.len()).rev() {
        let message_tokens = u32::try_from(
            crate::network::compaction::estimate_message_tokens(&history[index]),
        )
        .unwrap_or(u32::MAX);
        if suffix_messages < crate::network::compaction::KEEP_RECENT_TURNS
            || suffix_tokens.saturating_add(message_tokens) <= keep_target
        {
            suffix_start = index;
            suffix_tokens = suffix_tokens.saturating_add(message_tokens);
            suffix_messages += 1;
        } else {
            break;
        }
    }
    if suffix_start == 0 {
        return false;
    }

    // Never split an old conversation in the middle of a user turn when a
    // clean boundary is available. The retained suffix therefore keeps the
    // current follow-up and its recent tool activity together.
    let boundary = (0..=suffix_start)
        .rev()
        .find(|&index| {
            history[index].role == "user"
                && !history[index].content.starts_with("<tool_result>")
        })
        .unwrap_or(suffix_start);
    if boundary == 0 {
        return false;
    }

    let record_limit = budget
        .saturating_mul(3)
        .min(DETERMINISTIC_RECORD_MAX_CHARS as u32) as usize;
    let record = deterministic_context_record(&history[..boundary], record_limit);
    history.splice(
        0..boundary,
        [ChatMessage::new("system", record)],
    );
    true
}

fn deterministic_context_record(history: &[ChatMessage], max_chars: usize) -> String {
    crate::network::compaction::StructuredSessionMemory::extract_from_history(history)
        .format_record(max_chars)
}

pub(crate) async fn compact_history_to_budget(history: &mut Vec<ChatMessage>, budget: u32) {
    if history.is_empty() {
        return;
    }

    // Strip <think> blocks from all assistant messages first to free up budget.
    for m in history.iter_mut() {
        if m.role == "assistant" {
            m.content = strip_think_blocks(&m.content);
        }
    }

    // Deterministic, content-aware pruning runs before the class-based fallback
    // below. Keep the newest raw suffix and only collapse exact unchanged reads;
    // a read with different content may reflect a real edit and must survive.
    let duplicate_reads = crate::network::compaction::prune_duplicate_tool_results(
        history,
        crate::network::compaction::KEEP_RECENT_TURNS,
    );

    let mut tokens = Vec::with_capacity(history.len());
    for m in history.iter() {
        tokens.push(crate::network::compaction::estimate_message_tokens(m) as u32);
    }
    let mut total: u32 = tokens.iter().sum();
    if total <= budget {
        return;
    }

    dbg_log!(
        "History tokens ({}) exceed budget ({}). Compacting tool outputs by priority.",
        total,
        budget
    );

    // Prune lowest-value outputs first: throwaway snapshots, then file contents,
    // then anything else still taking space. Each class is flattened oldest-first.
    prune_class(history, &mut tokens, &mut total, budget, "throwaway").await;
    prune_class(history, &mut tokens, &mut total, budget, "file").await;
    prune_class(history, &mut tokens, &mut total, budget, "other").await;

    let deterministic_record = if total > budget {
        compact_history_deterministically(history, budget)
    } else {
        false
    };
    if deterministic_record {
        tokens = history
            .iter()
            .map(|m| crate::network::compaction::estimate_message_tokens(m) as u32)
            .collect();
        total = tokens.iter().sum();
        prune_class(history, &mut tokens, &mut total, budget, "throwaway").await;
        prune_class(history, &mut tokens, &mut total, budget, "file").await;
        prune_class(history, &mut tokens, &mut total, budget, "other").await;
    }

    dbg_log!(
        "Compact finished. New history tokens: {} (deterministic_record={})",
        total,
        deterministic_record
    );
    crate::logger::operational_event(
        "context.compaction",
        serde_json::json!({
            "history_tokens": total,
            "budget": budget,
            "duplicate_reads_collapsed": duplicate_reads,
            "summary_generated": false,
            "deterministic_record": deterministic_record,
            "hard_trim": false,
        }),
    );
}

/// Extract a context length from ollama's /api/show `model_info` blob;
/// the key is architecture-prefixed, e.g. "llama.context_length".
fn context_length_from_model_info(info: &serde_json::Value) -> Option<u32> {
    info.as_object()?
        .iter()
        .find(|(k, _)| k.ends_with(".context_length"))
        .and_then(|(_, v)| v.as_u64())
        .map(|n| n as u32)
}

/// Ask an ollama server for a model's context window. Returns None for
/// non-ollama endpoints or on any error.
pub async fn fetch_context_window(
    client: &reqwest::Client,
    chat_url: &str,
    model: &str,
    engine: Option<&str>,
) -> Option<u32> {
    let base = chat_url.strip_suffix("/v1/chat/completions")?;

    if let Some(eng) = engine {
        match eng.to_lowercase().as_str() {
            "ollama" => {
                let show_url = format!("{base}/api/show");
                let resp = client
                    .post(&show_url)
                    .json(&serde_json::json!({"model": model}))
                    .send()
                    .await
                    .ok()?;
                if resp.status().is_success() {
                    let body: serde_json::Value = resp.json().await.ok()?;
                    if let Some(ctx) = context_length_from_model_info(body.get("model_info")?) {
                        return Some(ctx);
                    }
                }
            }
            "llamacpp" | "llama.cpp" | "llama" => {
                let props_url = format!("{base}/props");
                let resp = client.get(&props_url).send().await.ok()?;
                if resp.status().is_success() {
                    let body: serde_json::Value = resp.json().await.ok()?;
                    if let Some(n) = body
                        .get("default_generation_settings")
                        .and_then(|v| v.get("n_ctx"))
                        .and_then(|v| v.as_u64())
                    {
                        return Some(n as u32);
                    }
                    if let Some(n) = body.get("n_ctx").and_then(|v| v.as_u64()) {
                        return Some(n as u32);
                    }
                }
            }
            _ => {}
        }
    }

    // Fallback: try llama.cpp first, then Ollama
    let props_url = format!("{base}/props");
    if let Ok(resp) = client.get(&props_url).send().await
        && resp.status().is_success()
        && let Ok(body) = resp.json::<serde_json::Value>().await
    {
        if let Some(n) = body
            .get("default_generation_settings")
            .and_then(|v| v.get("n_ctx"))
            .and_then(|v| v.as_u64())
        {
            return Some(n as u32);
        }
        if let Some(n) = body.get("n_ctx").and_then(|v| v.as_u64()) {
            return Some(n as u32);
        }
    }

    let show_url = format!("{base}/api/show");
    let resp = client
        .post(&show_url)
        .json(&serde_json::json!({"model": model}))
        .send()
        .await
        .ok()?;
    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.ok()?;
        if let Some(ctx) = context_length_from_model_info(body.get("model_info")?) {
            return Some(ctx);
        }
    }

    None
}

/// Read-only tools whose results can be safely short-circuited by the repeat guard.
pub(crate) fn is_read_only_tool(name: &str) -> bool {
    matches!(
        crate::tools::tool_safety(name),
        crate::tools::ToolSafety::ReadOnly
    )
}

pub(crate) fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
            | "generate_sound_effect"
            | "generate_music"
            | "spawn_agent"
            | "send_agent"
            | "cancel_agent"
    )
}

/// True only if we have read this file before AND its mtime is unchanged since.
/// A re-read is allowed whenever the file is new, missing, or modified on disk —
/// so the agent can always refresh after a (possibly partial) edit.
pub(crate) fn view_file_unchanged_since_last_read(
    stored: Option<std::time::SystemTime>,
    current: Option<std::time::SystemTime>,
) -> bool {
    matches!((stored, current), (Some(a), Some(b)) if a == b)
}

/// Best-effort mtime of the resolved tool path (None if it can't be stat'd).
pub(crate) fn path_mtime(raw_path: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(crate::tools::resolve_tool_path(raw_path))
        .and_then(|m| m.modified())
        .ok()
}

/// A canonical key identifying "the same call" for the repeat guard.
pub(crate) fn tool_signature(name: &str, args: &serde_json::Value) -> String {
    let key = match name {
        // Bucket full/default reads together so paging can't bypass the guard.
        "view_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let start = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1);
            let end_str = args
                .get("end_line")
                .and_then(|v| v.as_u64())
                .map(|e| e.to_string())
                .unwrap_or_else(|| "end".to_string());
            format!("{path}|{start}-{end_str}")
        }
        _ => serde_json::to_string(args).unwrap_or_default(),
    };
    format!("{name}:{key}")
}

pub(crate) fn align_alternating_messages(
    raw_msgs: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    if raw_msgs.is_empty() {
        return raw_msgs;
    }

    let mut msgs = Vec::new();
    let mut system_content = String::new();

    // 1. Merge the leading system messages into the prompt, and keep any later
    //    one where it happened, as a user turn.
    //
    //    A harness note earns its meaning from its position: "this action has
    //    repeated 5 times" answers the call above it. Hoisting it into the
    //    system prompt files it 12k characters away from the thing it is about,
    //    behind the skill catalogue, where it reads as a standing instruction
    //    rather than a response. Providers that demand strict alternation reject
    //    a mid-conversation system role, so it is carried as user text instead.
    let mut still_leading = true;
    for msg in raw_msgs {
        if let Some(role) = msg.get("role").and_then(|r| r.as_str()) {
            if role == "system" && still_leading {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    if !system_content.is_empty() {
                        system_content.push_str("\n\n");
                    }
                    system_content.push_str(content);
                }
            } else if role == "system" {
                let content = msg
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string();
                msgs.push(serde_json::json!({ "role": "user", "content": content }));
            } else {
                still_leading = false;
                msgs.push(msg);
            }
        }
    }

    let mut final_msgs = Vec::new();
    if !system_content.is_empty() {
        final_msgs.push(serde_json::json!({
            "role": "system",
            "content": system_content,
        }));
    }

    if msgs.is_empty() {
        return final_msgs;
    }

    // 2. Ensure the first message is a "user" message
    let first_role = msgs[0]
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("user");
    if first_role != "user" {
        final_msgs.push(serde_json::json!({
            "role": "user",
            "content": "[Context initialization]",
        }));
    }

    // 3. Alternate roles, merging consecutive same-role non-tool messages
    for msg in msgs {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let content = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        // A message carrying structured tool calls can never be merged: the
        // merge keeps only text, so folding it into a neighbour would silently
        // drop the calls and leave the following tool results answering nothing.
        let carries_calls = msg.get("tool_calls").is_some();
        if let Some(last) = final_msgs.last_mut() {
            let last_role = last.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let last_carries_calls = last.get("tool_calls").is_some();
            if last_role == role && role != "tool" && !carries_calls && !last_carries_calls {
                if let Some(last_content) = last.get_mut("content") {
                    let mut new_content = last_content.as_str().unwrap_or("").to_string();
                    new_content.push_str("\n\n");
                    new_content.push_str(&content);
                    *last_content = serde_json::Value::String(new_content);
                }
                continue;
            }
        }
        final_msgs.push(msg);
    }

    final_msgs
}

/// Ask an endpoint whether it implements OpenAI-style function calling.
///
/// Hostnames cannot answer this: a gateway on `localhost:3000` may front a
/// model with full tool support, and an endpoint at a well-known provider's
/// address may be a proxy that strips the field. So the endpoint is asked
/// directly, once, with the smallest request that still carries a tool schema.
/// Anything other than a clean acceptance counts as unsupported — staying on
/// the text protocol costs quality, while wrongly assuming tool support breaks
/// every turn.
pub async fn probe_function_calling(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    url: &str,
    model: &str,
) -> bool {
    let resolved_url = {
        let trimmed = url.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") || trimmed.ends_with("/chats/completion") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/chat/completions")
        }
    };
    let api_key = {
        let s = state.lock().await;
        s.config
            .models
            .iter()
            .find(|m| m.url == url || m.endpoint_url() == resolved_url)
            .and_then(|m| m.resolved_api_key())
    };

    let payload = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
        "stream": false,
        "tools": [{
            "type": "function",
            "function": {
                "name": "probe",
                "description": "capability probe",
                "parameters": {"type": "object", "properties": {}},
            },
        }],
        "tool_choice": "none",
    });

    let mut req = client
        .post(&resolved_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(20));
    if let Some(ref key) = api_key {
        req = req
            .header("Authorization", format!("Bearer {key}"))
            .header("X-Api-Key", key);
    }

    match req.send().await {
        Ok(response) if response.status().is_success() => {
            dbg_log!(
                "probe_function_calling: {} accepts tool schemas",
                resolved_url
            );
            true
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            dbg_log!(
                "probe_function_calling: {} rejected tool schema ({}): {}",
                resolved_url,
                status,
                body
            );
            false
        }
        Err(error) => {
            dbg_log!(
                "probe_function_calling: {} probe failed: {}",
                resolved_url,
                error
            );
            false
        }
    }
}

fn push_status_line(s: &mut AppState, text: String) {
    s.history.push(ChatMessage::new("system", text));
    crate::config::save_history(&s.history);
}

/// Assemble the full provider request for one agent turn.
///
/// Runs AI compaction if the history is long enough, snapshots the eligible
/// history, then builds the message array: a STATIC system prefix (tool
/// protocol + agent mode only, so the provider prompt cache stays warm) plus
/// the conversation, with all turn-varying context (environment delta,
/// files-in-context, task plan) appended to the last message. Finally trims to
/// the context-window budget and injects the system reminder. `tool_rounds` is
/// only used to decide whether a one-time "context window full" notice is shown.
pub(crate) async fn prepare_turn_request(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    tool_rounds: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<Vec<serde_json::Value>, String> {
    // Try AI-driven compaction if history is long enough.
    //
    // The summarizer is a network round-trip, so the AppState mutex must NOT be
    // held while it runs: the TUI draw loop locks the same mutex every frame and
    // would freeze for the whole call. Instead we take a snapshot of the history
    // under a short lock, compact the owned copy with the lock released, then
    // re-acquire and merge the result back in.
    {
        let (
            api_url,
            model_name,
            budget,
            local_model,
            active_session_id,
            captured_history,
        ) = {
            let s = state.lock().await;
            let local_model = s
                .active_model_profile()
                .is_some_and(|profile| profile.is_local())
                || {
                    let lower = s.api_base_url.to_ascii_lowercase();
                    lower.contains("11434") || lower.contains("ollama")
                };
            (
                s.api_base_url.clone(),
                s.model_name.clone(),
                s.get_history_token_budget() as usize,
                local_model,
                s.active_session_id.clone(),
                s.history.clone(),
            )
        };
        let pre_len = captured_history.len();
        let captured_revision = captured_history.revision();
        let mut working_history = captured_history;

        // Lock released here: this await performs I/O.
        let compacted = compaction::maybe_compact_with_local_policy(
            client,
            &api_url,
            &model_name,
            working_history.as_mut_vec(),
            budget,
            cancel_token,
            local_model,
        )
        .await;

        // Merge policy for history appended while the lock was down (a tool
        // result or a user message can land mid-compaction):
        //
        //  - unchanged history  -> write the compacted copy back wholesale;
        //  - history grew, and the prefix we compacted is still intact -> keep
        //    the compacted prefix and re-append the new tail verbatim. Nothing
        //    that arrived during the call is lost;
        //  - anything else (history shrank or was rewritten underneath us, e.g.
        //    /new, /compact, a rollback) -> discard our copy entirely and log
        //    the miss. Compaction is best-effort and will retry on the next
        //    turn; clobbering the live history is not an acceptable trade.
        //
        // Note that `maybe_compact` also performs local tool-output pruning even
        // when it returns false, so the write-back is attempted regardless of
        // the return value; the flag only gates the cache invalidation below.
        let mut s = state.lock().await;
        let live_session_id = s.active_session_id.clone();
        let prefix_intact = live_session_id == active_session_id
            && s.history.len() >= pre_len
            && s.history.is_append_only_since(captured_revision);
        if prefix_intact {
            if s.history.len() > pre_len {
                working_history.extend(s.history[pre_len..].iter().cloned());
            }
            let history_changed = working_history.revision() != captured_revision;
            if history_changed {
                s.history.replace(working_history.into_vec());
            }
            if compacted {
                dbg_log!("History compacted. Clearing read/dedup cache.");
                s.recent_read_calls.clear();
                s.recent_read_outputs.clear();
                s.read_file_mtimes.clear();
                crate::config::save_history(&s.history);
            } else if history_changed {
                // Deterministic pruning can change the request history without
                // invoking the summarizer. Persist that rewrite as well, but
                // keep the read cache: no filesystem state changed.
                crate::config::save_history(&s.history);
            }
        } else {
            dbg_log!(
                "Skipping automatic compaction write-back: active session or history changed underneath the summarizer (captured session '{}', live session '{}', {} messages before, {} now). Live history kept as-is.",
                active_session_id,
                live_session_id,
                pre_len,
                s.history.len()
            );
        }
        drop(s);
    }

    // Everything the request needs from AppState is read in one guarded block so
    // the lock is taken a couple of times instead of once per field. The
    // environment snapshot is captured first because it touches the filesystem.
    let workspace_root = {
        let s = state.lock().await;
        s.workspace_root
            .clone()
            .or_else(|| std::env::current_dir().ok())
    };
    let current_snapshot = workspace_root
        .as_deref()
        .map(crate::context::ContextSnapshot::capture_at)
        .unwrap_or_else(crate::context::ContextSnapshot::capture);
    let (
        mut history_snapshot,
        budget_token_limit,
        read_files,
        todos,
        volatile_usage,
        volatile_quota,
        volatile_window,
        mut context_section,
        system_prompt,
        skill_metadata,
        native_schema_policy,
        active_profile,
        vision_profile,
        mut image_cache,
    ) = {
        let mut s = state.lock().await;
        let history_snapshot = s.history.clone();
        let consumed_wakeups = s.consume_observed_background_wakeups();
        if consumed_wakeups > 0 {
            dbg_log!(
                "Consumed {} background wakeup(s) already present in the request history snapshot",
                consumed_wakeups
            );
        }
        let budget_token_limit = s.get_history_token_budget();
        let mut read_files: Vec<String> = s
            .read_file_mtimes
            .iter()
            .map(|(path, snapshot_mtime)| {
                format_read_file_context_entry(path, Some(*snapshot_mtime), path_mtime(path))
            })
            .collect();
        read_files.sort();
        let todos = s.todos.clone();
        let volatile_usage = s.current_token_usage.clone();
        let volatile_quota = s.model_quota_remaining;
        let volatile_window = s.active_context_window();
        let context_section = match &s.context_snapshot {
            Some(prev) => prev
                .diff(&current_snapshot)
                .unwrap_or_else(|| "# Environment\n(unchanged since session start)".to_string()),
            None => workspace_root
                .as_deref()
                .map(crate::context::environment_context_at)
                .unwrap_or_else(crate::context::environment_context),
        };
        let protocol = s.active_tool_protocol();
        let agent_mode = s.agent_mode;
        let delegation_active = s.delegation_active;
        let system_prompt = s
            .prompt_cache
            .system_prompt(delegation_active, protocol, agent_mode)
            .to_string();
        let skill_metadata = s.prompt_cache.skill_metadata();
        let native_schema_policy = if matches!(protocol, crate::config::ToolProtocol::ApiNative) {
            Some(crate::tools::ToolSchemaPolicy::root(delegation_active))
        } else {
            None
        };
        // Store the snapshot if this is the first turn.
        if s.context_snapshot.is_none() {
            s.context_snapshot = Some(current_snapshot.clone());
        }
        (
            history_snapshot,
            budget_token_limit,
            read_files,
            todos,
            volatile_usage,
            volatile_quota,
            volatile_window,
            context_section,
            system_prompt,
            skill_metadata,
            native_schema_policy,
            s.active_model_profile(),
            s.vision_model_profile(),
            s.image_analysis_cache.clone(),
        )
    };

    // ContextSnapshot deliberately emits only environment deltas after the
    // first request. Project instructions are different: they are durable
    // constraints, so keep the current bounded document available after
    // compaction/resume without putting it into every summary message.
    if let Some(instructions) = current_snapshot.project_instructions()
        && !context_section.contains("# Project instructions")
    {
        context_section.push_str("\n\n");
        context_section.push_str(&instructions);
    }

    compact_history_to_budget(history_snapshot.as_mut_vec(), budget_token_limit).await;

    if history_snapshot
        .iter()
        .any(|m| m.role == "user" && m.content.contains("![image](file://"))
    {
        let active_profile = active_profile.ok_or_else(|| {
            "image analysis failed: active model profile is not configured".to_string()
        })?;
        if active_profile.image_input_supported() != Some(true) {
            let vision_profile = vision_profile.ok_or_else(|| {
                "image analysis failed: configure a dedicated vision_model profile".to_string()
            })?;
            let request_client = client.clone();
            let request_cancel = cancel_token.clone();
            history_snapshot = image_fallback::prepare_history_for_model(
                &history_snapshot,
                &active_profile,
                &vision_profile,
                &mut image_cache,
                |profile, bytes| {
                    let profile = profile.clone();
                    let request_client = request_client.clone();
                    let request_cancel = request_cancel.clone();
                    async move {
                        image_fallback::request_vision_analysis(
                            &request_client,
                            &profile,
                            bytes,
                            &request_cancel,
                        )
                        .await
                    }
                },
            )
            .await?
            .into();
            let mut guard = state.lock().await;
            guard.image_analysis_cache.extend(image_cache);
            let session_id = guard.active_session_id.clone();
            let current_cache = guard.image_analysis_cache.clone();
            drop(guard);
            crate::config::save_session_image_cache(&session_id, &current_cache);
        }
    }

    history_snapshot.retain(|m| {
        (matches!(m.role.as_str(), "user" | "assistant" | "tool") && !m.content.starts_with('/'))
            || is_model_directed_note(m)
    });

    let skill_hint = if let Some(latest_user_prompt) = history_snapshot
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
    {
        crate::skills::skill_routing_hint(latest_user_prompt, skill_metadata.as_slice())
    } else {
        None
    };

    // The base system prompt is kept STATIC across turns (it only depends on
    // the tool protocol and agent mode, which don't change mid-task). The
    // priority skill route and other turn-varying context are appended to the
    // LAST message instead, so they never invalidate the cached system prompt.
    //
    // The static system prompt is served from AppState's PromptCache: it's only
    // rebuilt when the protocol, agent mode, or MCP tool set changes, not on
    // every turn. Skill metadata is also loaded lazily once by PromptCache and
    // remains separate from the fresh list_skills/use_skill discovery paths.
    //
    // Build the turn-varying context tail (appended to the last message
    // after the history is assembled, to preserve the cached prefix). The
    // volatile runtime block (clock/cwd/quota) goes last, as the explicit cache
    // divider at the very end of the payload.
    let memory_query = history_snapshot
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.clone())
        .unwrap_or_default();
    let project_memory = crate::memory::render_relevant_async(
        workspace_root.clone(),
        memory_query,
        (budget_token_limit / 16).min(192) as usize,
    )
    .await;
    let mut dynamic_context = build_dynamic_context_tail_with_memory(
        context_section,
        &read_files,
        &todos,
        project_memory,
    );
    prepend_skill_routing_hint(&mut dynamic_context, skill_hint.as_deref());
    let volatile_block =
        build_volatile_context_block(volatile_usage.as_ref(), volatile_quota, volatile_window);
    if !dynamic_context.is_empty() {
        dynamic_context.push_str("\n\n");
    }
    dynamic_context.push_str(&volatile_block);
    let mut msgs = history::to_messages(&history_snapshot, system_prompt.clone());

    // Attach turn-varying context to the tail so the static system prefix
    // and historical conversation remain cache-stable. Done before budget
    // trimming so its size counts toward the budget.
    attach_request_context_tail(&mut msgs, &dynamic_context);

    let (native_tool_schemas, context_budget) = {
        let mut s = state.lock().await;
        let native_tool_schemas = native_schema_policy
            .map(|policy| {
                let session_id = s.active_session_id.clone();
                s.prompt_cache
                    .native_tool_schemas(policy, &msgs, &session_id)
                    .0
            })
            .unwrap_or_default();
        (native_tool_schemas, s.active_context_budget())
    };
    let preflight = compaction::calculate_preflight_budget(
        &system_prompt,
        &native_tool_schemas,
        &history_snapshot,
        &dynamic_context,
        0,
        &context_budget,
    );
    crate::logger::operational_event(
        "context.preflight_budget",
        serde_json::to_value(&preflight).unwrap_or_default(),
    );

    // `budget_token_limit` already reserves completion, thinking, tool-schema,
    // and provider safety headroom from the active model profile. Keep this
    // final trim on the effective history budget.
    inject_system_reminder(&mut msgs);
    let schema_over_reserve = preflight
        .tool_schema_tokens
        .saturating_sub(context_budget.tool_reserve as usize);
    let schema_adjusted_history_budget = budget_token_limit
        .min(context_budget.history_tokens)
        .saturating_sub(schema_over_reserve as u32);
    let budget = schema_adjusted_history_budget;
    let dropped = trim_msgs_to_budget(&mut msgs, budget);
    if dropped > 0 {
        dbg_log!(
            "context budget {} tokens exceeded: dropped {} oldest message(s)",
            budget,
            dropped
        );
        crate::logger::operational_event(
            "context.hard_trim",
            serde_json::json!({
                "reason": "model_aware_history_budget_after_deterministic_pruning",
                "budget": budget,
                "dropped_messages": dropped,
                "preflight": preflight,
            }),
        );
        if tool_rounds == 0 {
            let mut s = state.lock().await;
            s.history.push(ChatMessage::new(
                "system",
                format!(
                    "context window full: dropped {} oldest message(s) from the request. Use /new to start fresh.",
                    dropped
                ),
            ));
        }
    }

    Ok(msgs)
}

/// Execute a batch of tool calls and return `(name, result, diff)` per call.
///
/// When `approved` is false every call resolves to a denial message. Otherwise
/// calls execute in model order. Read-only calls could be parallelized safely,
/// but preserving one ordering rule for every batch prevents edits, commands,
/// and reads from racing each other or hiding dependencies. If any mutating
/// tool ran, a single cached compiler check is appended to the first mutating
/// tool's result so build errors surface inline.

/// One result message per call that will never run, so no call is left
/// unanswered.
///
/// A structured transcript replays an assistant message together with the calls
/// it made; a call with no matching result is a protocol violation the provider
/// rejects, and — worse — leaves the model free to assume whatever it likes
/// about what happened. Answering with the failure keeps the record honest.
/// What to tell a model whose completion claim wrote nothing.
///
/// The branch that matters is the second one. A model whose edits all failed is
/// often looking at a workspace that already holds the requested state — left
/// from an earlier run — and if the only sanctioned ways out are "make the
/// change" or "it cannot be made", neither fits, so it manufactures a mutation
/// to satisfy the check. In one session the cheapest mutation available was
/// deleting the very line it had been asked to add, which it then reported as
/// having added and removed.
pub(crate) fn completion_block_message(failed: usize) -> String {
    format!(
        "[Finish blocked — {failed} edit(s) were attempted in this task and every one failed, so this task \
wrote nothing. Exactly one of these is true; establish which from a fresh read, then act.\n\
 1. The change still needs making — make it, verify it, then finish.\n\
 2. The workspace is already in the requested state, possibly from before this task began — say exactly \
that and finish. This is a valid outcome and requires no edit.\n\
 3. The change cannot be made — finish by stating why.\n\
Do NOT edit something else, delete existing content, or reverse the request in order to clear this check. \
An edit that moves the workspace further from what was asked is worse than writing nothing.]"
    )
}

/// Whether a `system` history entry is something the model must actually read.
///
/// Everything the harness tells the model — loop warnings, rejected tool calls,
/// blocked completions, compaction summaries — is written into history as a
/// system message. Those were filtered out of the request, so the harness spent
/// entire sessions correcting a model that never received a word of it: one
/// session issued 25 loop warnings while the model repeated the same read 25
/// times, having been told nothing.
///
/// Session chatter that only makes sense in the TUI (command output, model
/// switches) stays out: it is noise in the prompt and was never meant for the
/// model. Harness notes are bracketed; compaction summaries carry their marker.
pub(crate) fn is_model_directed_note(message: &ChatMessage) -> bool {
    message.role == "system"
        && (message.content.starts_with('[')
            || message
                .content
                .starts_with(crate::network::compaction::SUMMARY_MARKER))
}

/// Largest read output kept for replay to an identical repeat call. Small reads
/// are cheaper to repeat than to argue about; large ones stay behind a notice so
/// a loop cannot re-send a whole file every turn. Sized to hold up to an 800-line
/// view_file window.
pub(crate) const REPLAYABLE_READ_LIMIT: usize = 24_576;

/// How many times the completion gate argues before letting a claim through.
const MAX_COMPLETION_BLOCKS: u8 = 2;

/// Whether a `complete_task` claim describes work that never reached disk.
///
/// True when every mutating call in the task failed: the workspace is untouched,
/// yet the model is reporting the job done — usually because it read the file,
/// found the state it wanted already there for some unrelated reason, and took
/// that as proof of its own edit. Capped so the gate cannot argue forever with a
/// model that insists.
pub(crate) fn completion_claims_unapplied_work(
    made_edits: bool,
    failed: usize,
    blocks: u8,
) -> bool {
    !made_edits && failed > 0 && blocks < MAX_COMPLETION_BLOCKS
}

pub(crate) fn record_provider_error(ctx: &mut TurnContext, error: &str) {
    ctx.provider_errors += 1;
    let is_quota =
        error.contains("429") || error.to_ascii_lowercase().contains("too many requests");
    if is_quota {
        ctx.provider_429s += 1;
        ctx.stop_reason = Some(lifecycle::StopReason::ProviderError(Some(429)));
    } else if ctx.stop_reason.is_none() {
        ctx.stop_reason = Some(lifecycle::StopReason::ProviderError(None));
    }
}

pub(crate) fn active_todo_checkpoint(todos: &[crate::app::TodoItem]) -> Option<String> {
    todos
        .iter()
        .find(|todo| todo.status == "in_progress")
        .map(|todo| todo.content.clone())
}

/// Pair calls with provider ids. Native calls carry their id directly; the
/// positional fallback covers the older stream bookkeeping path.
pub(crate) fn call_refs_for(
    calls: &[crate::tools::ToolCall],
    ids: &[String],
) -> Vec<crate::app::ToolCallRef> {
    calls
        .iter()
        .enumerate()
        .filter_map(|(position, call)| {
            let id = call
                .call_id
                .clone()
                .or_else(|| ids.get(position).cloned())?;
            Some(crate::app::ToolCallRef {
                id,
                name: call.name.clone(),
                arguments: call.arguments.to_string(),
            })
        })
        .collect()
}

pub(crate) fn unanswered_call_results(
    calls: &[crate::app::ToolCallRef],
    reason: &str,
) -> Vec<ChatMessage> {
    unanswered_call_results_with_kind(calls, reason, crate::tools::ToolErrorKind::Internal)
}

pub(crate) fn unanswered_call_results_with_kind(
    calls: &[crate::app::ToolCallRef],
    reason: &str,
    error_kind: crate::tools::ToolErrorKind,
) -> Vec<ChatMessage> {
    calls
        .iter()
        .map(|call| {
            ChatMessage::new("tool", format!("{}: error: {reason}", call.name))
                .answering(Some(call.id.clone()))
                .with_tool_result(crate::app::ToolResultRecord {
                    tool_name: call.name.clone(),
                    success: false,
                    error_kind: Some(format!("{error_kind:?}")),
                    retryable: matches!(
                        error_kind,
                        crate::tools::ToolErrorKind::Cancelled
                            | crate::tools::ToolErrorKind::Internal
                    ),
                    ..Default::default()
                })
        })
        .collect()
}

/// Replacement transcript text for a response whose tool batch was truncated.
/// Records only which tools survived, never the model's prose: a response that
/// plans a whole session ahead also narrates results for calls that never ran,
/// and replaying that text lets the next turn treat its own fiction as
/// observed fact.
pub(crate) fn truncated_batch_summary(kept: &[crate::tools::ToolCall], dropped: usize) -> String {
    let names = kept
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[Oversized response: only the first {} tool calls were kept ({names}); {dropped} more were dropped. Anything the response claimed about their results was imagined — continue from the real results below.]",
        kept.len()
    )
}

#[cfg(test)]
#[path = "network/tests.rs"]
mod tests;
