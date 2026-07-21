use super::*;

pub(crate) const RESPONSE_RESERVE_TOKENS: u32 = 1024;

fn estimate_msg_chars(msg: &serde_json::Value) -> usize {
    match msg.get("content") {
        Some(serde_json::Value::String(s)) => s.len(),
        Some(other) => other.to_string().len(),
        None => 0,
    }
}

/// Drop the oldest non-system messages until the payload fits the token
/// budget (~4 chars/token), keeping the system prompt and the latest
/// exchange. Returns how many messages were dropped.
pub(crate) fn trim_msgs_to_budget(msgs: &mut Vec<serde_json::Value>, budget_tokens: u32) -> usize {
    let budget_chars = budget_tokens as usize * 4;
    let mut total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let mut dropped = 0;
    while total > budget_chars && msgs.len() > 3 {
        total -= estimate_msg_chars(&msgs[1]);
        msgs.remove(1);
        dropped += 1;
    }
    dropped
}

/// If the message history has grown long (e.g. >= 4 messages), inject a brief
/// system reminder right before the latest user message or tool result. This
/// prevents the model from forgetting the core guidelines and tool formats
/// due to attention dilution in long contexts.
pub(crate) fn inject_system_reminder(msgs: &mut Vec<serde_json::Value>) {
    if msgs.len() >= 4 {
        let reminder_text = "REMINDER: You are rustcode. Always follow your core instructions:\n\
             - Be extremely concise and direct. No filler or preamble.\n\
             - To call a tool, output exactly one fenced `tool` block containing a single JSON object. Do not output any conversational text or narration before or after the block.\n\
             - Available tools: view_file, replace_file_content, multi_replace_file_content, write_to_file, delete_file, move_file, copy_file, list_directory, grep, glob, run_command, search_web, find_symbol, get_project_map.";
        
        if let Some(last_msg) = msgs.last_mut() {
            if let Some(content) = last_msg.get_mut("content") {
                match content {
                    serde_json::Value::String(s) => {
                        *s = format!("{}\n\n{}", s, reminder_text);
                    }
                    serde_json::Value::Array(arr) => {
                        arr.push(serde_json::json!({
                            "type": "text",
                            "text": format!("\n\n{}", reminder_text)
                        }));
                    }
                    _ => {}
                }
            }
        }
    }
}
