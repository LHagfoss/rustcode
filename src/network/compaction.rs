use crate::app::ChatMessage;

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().saturating_div(4)
}

pub fn prune_old_tool_outputs(history: &mut [ChatMessage]) {
    let mut total_tool_tokens = 0;
    // Walk backward through history
    for m in history.iter_mut().rev() {
        if m.role == "tool" {
            let tokens = estimate_tokens(&m.content);
            total_tool_tokens += tokens;
            // Protect the last ~30k tokens of tool outputs (approx 120k chars).
            // Prune older ones to save context window space.
            if total_tool_tokens > 30_000 && !m.content.contains("content cleared to save context") {
                if let Some(pos) = m.content.find(": ") {
                    let tool_name = &m.content[..pos];
                    m.content = format!("{}: [Old tool result content cleared to save context]", tool_name);
                } else {
                    m.content = "[Old tool result content cleared to save context]".to_string();
                }
            }
        }
    }
}

/// Check if history needs compaction and compact if so.
/// Returns true if compaction was performed.
pub async fn maybe_compact(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    budget: usize,
) -> bool {
    // 1. First run local, zero-cost tool output pruning
    prune_old_tool_outputs(history);

    // 2. Estimate total tokens in the history
    let total_tokens: usize = history.iter().map(|m| estimate_tokens(&m.content)).sum();
    if total_tokens < budget {
        return false;
    }

    // Determine how many messages to summarize.
    // We want to keep at least the 8 most recent messages verbatim, but also
    // retain a recent suffix of up to 30% of the token budget verbatim.
    let mut accumulated_tokens = 0;
    let keep_token_limit = (budget as f64 * 0.3) as usize; // Keep 30% of budget verbatim

    let mut keep_count = 0;
    for m in history.iter().rev() {
        let tokens = estimate_tokens(&m.content);
        if accumulated_tokens + tokens <= keep_token_limit || keep_count < 8 {
            accumulated_tokens += tokens;
            keep_count += 1;
        } else {
            break;
        }
    }

    let summarize_count = history.len().saturating_sub(keep_count);
    if summarize_count < 4 {
        return false;
    }

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

    // Pin the original task (first user message) so the goal is never blurred away.
    let first_user_task = history
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone());

    // Only summarize messages that aren't the prior summary itself.
    let to_summarize: Vec<&ChatMessage> = history[..summarize_count]
        .iter()
        .filter(|m| !(m.role == "system" && m.content.starts_with(SUMMARY_MARKER)))
        .collect();

    let summary = match generate_summary(
        client,
        url,
        model,
        prior_summary.as_deref(),
        &to_summarize,
    )
    .await
    {
        Some(s) => s,
        None => return false,
    };

    let tail: Vec<ChatMessage> = history[summarize_count..].to_vec();
    let task_in_tail = first_user_task
        .as_ref()
        .is_some_and(|t| tail.iter().any(|m| m.role == "user" && &m.content == t));

    // Replace the summarized range with a single summary message.
    history.clear();
    history.push(ChatMessage::new(
        "system",
        format!("{SUMMARY_MARKER}\n{summary}\n[End Summary — the following messages are the most recent conversation]"),
    ));
    // Re-inject the original task verbatim if it fell inside the summarized range.
    if let Some(task) = first_user_task
        && !task_in_tail
    {
        history.push(ChatMessage::new(
            "system",
            format!("[Original task — do not lose sight of this]\n{task}"),
        ));
    }
    history.extend(tail);

    true
}

/// Prefix that marks a compaction summary message, used to detect and preserve
/// prior summaries during incremental compaction.
const SUMMARY_MARKER: &str = "[Session History Summary]";

async fn generate_summary(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    prior_summary: Option<&str>,
    messages: &[&ChatMessage],
) -> Option<String> {
    let mut conversation_text = String::new();
    for m in messages {
        let role_label = match m.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            "tool" => "Tool Result",
            "system" => "System",
            _ => "Unknown",
        };
        // Limit each message to ~2000 chars to keep the prompt bounded. Use a
        // char boundary (not a byte slice) — byte slicing panics when the cut
        // lands inside a multi-byte UTF-8 sequence.
        let content = if m.content.chars().count() > 2000 {
            let head: String = m.content.chars().take(2000).collect();
            format!("{head}... [truncated]")
        } else {
            m.content.clone()
        };
        conversation_text.push_str(&format!("{}:\n{}\n\n", role_label, content));
    }

    // Incremental: hand the model the prior summary to fold in, so earlier facts
    // survive instead of being re-compressed away.
    let user_content = match prior_summary {
        Some(prev) => format!(
            "Existing summary of earlier context (preserve every fact, do NOT drop details):\n{prev}\n\n\
             New messages since then to fold into the summary:\n\n{conversation_text}"
        ),
        None => format!("Summarize this conversation:\n\n{conversation_text}"),
    };

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are a conversation summarizer for a coding session. Produce a concise bullet-point summary. Always preserve: the original user request/goal; every file read, created, or modified (with exact paths); key tool results, findings, and errors; and the current state of the work plus the next step. Be specific about file paths and code changes. Never invent facts and never drop facts from an existing summary. Do NOT include tool call syntax or JSON."
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

    let resp = client.post(url).json(&payload).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()
        .map(|s| s.trim().to_string())
}