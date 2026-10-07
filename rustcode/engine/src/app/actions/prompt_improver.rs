//! Opt-in prompt improver (`/pi`).
//!
//! When enabled, a prompt that starts a turn is rewritten by the active model
//! before the turn runs. The transcript records the original beside the
//! rewrite, so the change is never silent.

use crate::app::AppState;
use std::sync::Arc;
use tokio::sync::Mutex;

const INSTRUCTION: &str = "You rewrite a user's prompt for a coding agent. Make it clear, specific and unambiguous while keeping the user's intent. Keep the user's language. Copy file paths, code, commands, URLs, names, numbers and image markers such as ![image](file://...) exactly. Do not add requirements the user did not ask for, do not answer the prompt, and do not address the user. If the prompt is already clear, return it unchanged. Output only the rewritten prompt, with no preamble, quotes or explanation.";

const IMAGE_MARKER: &str = "![image](file://";
/// Shorter prompts are replies such as "yes" or "continue": rewriting them
/// can only invent intent.
const MIN_WORDS: usize = 4;

/// Whether a prompt that starts a turn should be sent to the improver.
pub(crate) fn should_improve(prompt: &str) -> bool {
    let prompt = prompt.trim();
    !prompt.starts_with('/')
        && !prompt.starts_with("__task_wakeup__:")
        && prompt.split_whitespace().count() >= MIN_WORDS
}

/// Accept a model rewrite only when it is usable as the prompt: non-empty,
/// actually different, not a runaway answer, and still carrying every image
/// attachment of the original.
pub(crate) fn accept_rewrite(original: &str, response: &str) -> Option<String> {
    let rewrite = rustcode_tool_protocol::text::strip_think_blocks(
        &rustcode_tool_protocol::text::promote_bare_thought_markers(response),
    );
    let rewrite = rewrite.trim().trim_matches('"').trim();
    if rewrite.is_empty() || rewrite == original.trim() {
        return None;
    }
    if rewrite.chars().count() > original.chars().count().saturating_mul(4) + 600 {
        return None;
    }
    let mut remaining = original;
    while let Some(start) = remaining.find(IMAGE_MARKER) {
        let end = remaining[start..].find(')').map(|end| start + end + 1)?;
        if !rewrite.contains(&remaining[start..end]) {
            return None;
        }
        remaining = &remaining[end..];
    }
    Some(rewrite.to_owned())
}

/// Transcript notice that shows what the improver changed.
pub(crate) fn improvement_notice(model: &str, before: &str, after: &str) -> String {
    format!(
        "Prompt improved by {model} · /pi off to disable\n\nBefore:\n{}\n\nAfter:\n{}",
        before.trim(),
        after.trim()
    )
}

/// Ask the active model to rewrite `prompt`. Returns `None` when the improver
/// is off, the prompt is not eligible, the request fails or is cancelled, or
/// the rewrite is not usable; the caller then keeps the original.
pub(crate) async fn improve_prompt(
    state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel: &tokio_util::sync::CancellationToken,
    prompt: &str,
) -> Option<String> {
    let (temporary, request_id) = {
        let live = state.lock().await;
        if !live.config.prompt_improver || !should_improve(prompt) {
            return None;
        }
        let request_id = format!(
            "prompt-improver-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let workspace = live
            .effective_workspace_root()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        // An explicit temporary ID keeps the request out of the session store,
        // the same way a manual recap owns its own empty state.
        let mut temporary = AppState::new_with_workspace_session(&workspace, Some(&request_id));
        temporary.config = live.config.clone();
        temporary.api_base_url = live.api_base_url.clone();
        temporary.model_name = live.model_name.clone();
        (Arc::new(Mutex::new(temporary)), request_id)
    };
    let (url, model) = {
        let temp = temporary.lock().await;
        (temp.api_base_url.clone(), temp.model_name.clone())
    };
    let buffer = Arc::new(Mutex::new(crate::network::StreamBuffer::new()));
    let messages = vec![
        serde_json::json!({"role": "system", "content": INSTRUCTION}),
        serde_json::json!({"role": "user", "content": prompt}),
    ];
    let result = crate::network::stream_request(
        client,
        temporary,
        cancel.clone(),
        &url,
        &model,
        messages,
        Arc::clone(&buffer),
        true,
        false,
        crate::network::stream_request::ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::root(false),
        Some(&request_id),
        None,
    )
    .await;
    if result.is_err() || cancel.is_cancelled() {
        return None;
    }
    let response = buffer.lock().await.content.clone();
    accept_rewrite(prompt, &response)
}

#[cfg(test)]
mod tests {
    use super::{accept_rewrite, improvement_notice, should_improve};

    #[test]
    fn only_real_prompts_are_sent_to_the_improver() {
        assert!(should_improve("fix the flaky test in the parser module"));
        for skipped in [
            "yes",
            "continue please",
            "/model fast one please now",
            "__task_wakeup__:task_1 a b c",
        ] {
            assert!(!should_improve(skipped), "{skipped}");
        }
    }

    #[test]
    fn unusable_rewrites_fall_back_to_the_original() {
        let original = "fix teh parser bug in src/parse.rs";
        assert_eq!(
            accept_rewrite(
                original,
                "<think>plan</think>\nFix the parser bug in src/parse.rs."
            )
            .as_deref(),
            Some("Fix the parser bug in src/parse.rs.")
        );
        assert_eq!(accept_rewrite(original, "  "), None);
        assert_eq!(accept_rewrite(original, original), None);
        assert_eq!(accept_rewrite(original, &"word ".repeat(400)), None);

        let with_image = "what is wrong here ![image](file:///tmp/a.png) please check";
        assert_eq!(
            accept_rewrite(with_image, "Explain what is wrong in the screenshot."),
            None
        );
        assert!(
            accept_rewrite(
                with_image,
                "Explain what is wrong in this screenshot: ![image](file:///tmp/a.png)"
            )
            .is_some()
        );
    }

    #[test]
    fn notice_shows_before_and_after_and_stays_out_of_the_model_context() {
        let notice = improvement_notice("deepseek-flash", "fix teh bug", "Fix the bug.");
        assert!(notice.contains("Before:\nfix teh bug"), "{notice}");
        assert!(notice.contains("After:\nFix the bug."), "{notice}");
        let message = crate::app::ChatMessage::new("system", notice);
        assert!(!crate::network::is_model_directed_note(&message));
    }
}
