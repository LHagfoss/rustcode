use crate::app::ChatMessage;

const COMPACTION_THRESHOLD: usize = 20;
const TAIL_MESSAGES_TO_KEEP: usize = 8;

/// Check if history needs compaction and compact if so.
/// Returns true if compaction was performed.
pub async fn maybe_compact(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
) -> bool {
    if history.len() < COMPACTION_THRESHOLD {
        return false;
    }

    // Find how many messages to summarize (all except the tail)
    let summarize_count = history.len().saturating_sub(TAIL_MESSAGES_TO_KEEP);
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