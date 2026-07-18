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

    let to_summarize: Vec<&ChatMessage> = history[..summarize_count].iter().collect();
    let summary = match generate_summary(client, url, model, &to_summarize).await {
        Some(s) => s,
        None => return false,
    };

    // Replace the old messages with a single compaction summary
    let tail: Vec<ChatMessage> = history[summarize_count..].to_vec();
    history.clear();
    history.push(ChatMessage::new(
        "system",
        format!("[Session History Summary]\n{summary}\n[End Summary — the following messages are the most recent conversation]"),
    ));
    history.extend(tail);

    true
}

async fn generate_summary(
    client: &reqwest::Client,
    url: &str,
    model: &str,
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
        // Limit each message to ~500 chars to keep the summarization prompt small
        let content = if m.content.len() > 500 {
            format!("{}... [truncated]", &m.content[..500])
        } else {
            m.content.clone()
        };
        conversation_text.push_str(&format!("{}:\n{}\n\n", role_label, content));
    }

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are a conversation summarizer. Produce a concise 2-3 paragraph summary of the following coding session conversation. Focus on: what the user asked for, what files were modified/created, what tools were used and their key results, and what the current state of the work is. Be specific about file paths and code changes. Do NOT include tool call syntax or JSON."
            },
            {
                "role": "user",
                "content": format!("Summarize this conversation:\n\n{}", conversation_text)
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