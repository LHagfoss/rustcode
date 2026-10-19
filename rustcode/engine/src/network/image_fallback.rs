use crate::app::ChatMessage;
use crate::config::ModelProfile;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use tokio_util::sync::CancellationToken;

const IMAGE_MARKER: &str = "![image](file://";
pub(crate) const MAX_IMAGE_ANALYSIS_CACHE_ENTRIES: usize = 128;

pub(crate) fn has_image_markers(history: &[ChatMessage]) -> bool {
    history
        .iter()
        .any(|message| message.role == "user" && message.content.contains(IMAGE_MARKER))
}

pub(crate) fn extend_bounded_cache(
    cache: &mut HashMap<String, String>,
    additions: HashMap<String, String>,
) {
    while cache.len() > MAX_IMAGE_ANALYSIS_CACHE_ENTRIES {
        if let Some(evicted) = cache.keys().next().cloned() {
            cache.remove(&evicted);
        }
    }
    for (key, value) in additions {
        if !cache.contains_key(&key) && cache.len() >= MAX_IMAGE_ANALYSIS_CACHE_ENTRIES {
            if let Some(evicted) = cache.keys().next().cloned() {
                cache.remove(&evicted);
            }
        }
        cache.insert(key, value);
    }
}

/// Image messages, newest first, whose uncached images are still sent to the
/// vision model. Older ones keep a placeholder: re-describing a long session's
/// whole image history one by one stalled a model switch for minutes.
const ANALYZED_IMAGE_MESSAGES: usize = 4;
/// Vision requests in flight at once.
const VISION_CONCURRENCY: usize = 4;
const CANCELLED: &str = "cancelled";
const IMAGE_MISSING: &str = "[Attached image analysis unavailable: image missing]";
const EARLIER_IMAGE_SKIPPED: &str =
    "[Earlier image not analyzed: only the most recent images are described for this model]";

struct Marker {
    message: usize,
    range: std::ops::Range<usize>,
    hash: Option<String>,
}

pub async fn preprocess_history_with<F, Fut>(
    history: &mut [ChatMessage],
    active_profile: &ModelProfile,
    vision_profile: &ModelProfile,
    cache: &mut HashMap<String, String>,
    mut request: F,
) -> Result<(), String>
where
    F: FnMut(&ModelProfile, Vec<u8>) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    use futures_util::StreamExt;

    // Naming the active model as its own vision_model says it reads images,
    // even when the profile never set supports_vision: send them natively
    // instead of asking the same model to describe each one first.
    let sees_images = match active_profile.image_input_supported() {
        Some(supported) => supported,
        None => vision_profile.name == active_profile.name,
    };
    if sees_images {
        return Ok(());
    }
    if !has_image_markers(history) {
        return Ok(());
    }

    let vision_usable = vision_profile.image_input_supported() != Some(false);
    let latest = history.len() - 1;
    let image_messages: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == "user" && message.content.contains(IMAGE_MARKER))
        .map(|(index, _)| index)
        .collect();
    let analyzed_from = image_messages
        .iter()
        .rev()
        .nth(ANALYZED_IMAGE_MESSAGES - 1)
        .copied()
        .unwrap_or(0);

    let mut markers = Vec::new();
    let mut pending: Vec<(String, Vec<u8>)> = Vec::new();
    for &message_index in &image_messages {
        let content = &history[message_index].content;
        let mut offset = 0;
        while let Some(found) = content[offset..].find(IMAGE_MARKER) {
            let start = offset + found;
            let path_start = start + IMAGE_MARKER.len();
            let Some(path_len) = content[path_start..].find(')') else {
                break;
            };
            let end = path_start + path_len + 1;
            let is_latest = message_index == latest;
            let hash = match std::fs::read(&content[path_start..end - 1]) {
                Ok(bytes) => {
                    let hash = image_hash(&bytes);
                    let wanted = is_latest || message_index >= analyzed_from;
                    if is_latest && !vision_usable {
                        return Err(unusable_vision_error(active_profile, vision_profile));
                    }
                    if wanted
                        && vision_usable
                        && cached(cache, &hash).is_none()
                        && !pending.iter().any(|(queued, _)| queued == &hash)
                    {
                        pending.push((hash.clone(), bytes));
                    }
                    Some(hash)
                }
                Err(e) if is_latest => {
                    return Err(format!("image analysis failed: could not read image: {e}"));
                }
                Err(_) => None,
            };
            markers.push(Marker {
                message: message_index,
                range: start..end,
                hash,
            });
            offset = end;
        }
    }

    let latest_hashes: Vec<&String> = markers
        .iter()
        .filter(|marker| marker.message == latest)
        .filter_map(|marker| marker.hash.as_ref())
        .collect();
    let requests = pending.into_iter().map(|(hash, bytes)| {
        let analysis = request(vision_profile, bytes);
        async move { (hash, analysis.await) }
    });
    let results: Vec<(String, Result<String, String>)> = futures_util::stream::iter(requests)
        .buffered(VISION_CONCURRENCY)
        .collect()
        .await;

    let mut pending_cache = HashMap::new();
    for (hash, result) in results {
        let for_latest = latest_hashes.contains(&&hash);
        let analysis = match result {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) if for_latest => {
                return Err("image analysis failed: vision model returned empty output".to_string());
            }
            Ok(_) => "[Attached image analysis unavailable]".to_string(),
            // A cancelled turn says nothing about the image; caching it would
            // hide the image from every later turn.
            Err(e) if for_latest || e == CANCELLED => {
                return Err(format!("image analysis failed: {e}"));
            }
            Err(e) => format!("[Attached image analysis unavailable: {e}]"),
        };
        pending_cache.insert(hash, analysis);
    }

    let mut rewritten: Vec<(usize, String)> = Vec::new();
    let mut image_number = 0usize;
    let mut cursor = 0;
    for (position, marker) in markers.iter().enumerate() {
        let content = &history[marker.message].content;
        if position == 0 || markers[position - 1].message != marker.message {
            rewritten.push((marker.message, String::new()));
            cursor = 0;
        }
        let output = &mut rewritten.last_mut().expect("pushed above").1;
        output.push_str(&content[cursor..marker.range.start]);
        cursor = marker.range.end;
        if let Some(hash) = &marker.hash {
            let analysis = pending_cache
                .get(hash)
                .or_else(|| cached(cache, hash))
                .map(String::as_str)
                .unwrap_or(EARLIER_IMAGE_SKIPPED);
            image_number += 1;
            output.push_str(&format_analysis(image_number, analysis));
        } else {
            output.push_str(IMAGE_MISSING);
        }
        let is_last_in_message = markers
            .get(position + 1)
            .is_none_or(|next| next.message != marker.message);
        if is_last_in_message {
            output.push_str(&content[cursor..]);
        }
    }

    extend_bounded_cache(cache, pending_cache);
    for (message_index, content) in rewritten {
        history[message_index].content = content;
    }
    Ok(())
}

/// A cached analysis, ignoring cancellations that older builds stored as if
/// they described the image.
fn cached<'a>(cache: &'a HashMap<String, String>, hash: &str) -> Option<&'a String> {
    cache
        .get(hash)
        .filter(|value| !value.ends_with(&format!("unavailable: {CANCELLED}]")))
}

fn unusable_vision_error(active: &ModelProfile, vision: &ModelProfile) -> String {
    if vision.name == active.name {
        format!(
            "image analysis failed: '{}' cannot read images and is also the configured vision_model. Set vision_model to a vision-capable profile.",
            active.name
        )
    } else {
        format!(
            "image analysis failed: vision_model '{}' does not support images. Set vision_model to a vision-capable profile.",
            vision.name
        )
    }
}

fn image_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn format_analysis(number: usize, analysis: &str) -> String {
    format!("[Attached image analysis]\nImage {number}\n\n{analysis}\n\nEnd image analysis.",)
}

pub const VISION_PROMPT: &str = "Analyze this image for a coding agent. Return concise, information-dense structured text, not a generic caption. Extract exact visible text when readable; UI structure and hierarchy; layout and relative positioning; errors, stack traces, terminal output, code, filenames, buttons, labels, and state; diagrams and relationships; and visual details relevant to reproducing or debugging it. Mark uncertain or unreadable text explicitly. Use short labeled sections such as Visible text, Layout and visual structure, Important details, and Relevant errors/code/UI state.";

pub(crate) async fn request_vision_analysis(
    client: &reqwest::Client,
    profile: &ModelProfile,
    bytes: Vec<u8>,
    cancel_token: &CancellationToken,
) -> Result<String, String> {
    if profile
        .credential
        .as_ref()
        .is_some_and(crate::provider_auth::CredentialRef::is_chatgpt)
    {
        return Err(
            "ChatGPT plan image analysis requires the streaming main Responses request; configure a vision-capable main model or use an API-key vision profile.".to_owned(),
        );
    }
    use base64::{Engine as _, engine::general_purpose};
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "application/octet-stream"
    };
    if profile.resolved_api_protocol() == crate::config::ApiProtocol::AnthropicMessages
        && mime == "application/octet-stream"
    {
        return Err("Messages image input requires PNG, JPEG, GIF, or WebP".into());
    }
    let data_url = format!(
        "data:{mime};base64,{}",
        general_purpose::STANDARD.encode(bytes)
    );
    // The vision endpoint speaks whatever protocol the profile declares:
    // sending a chat-completions body to a Responses endpoint fails with
    // 400 ("Either input or instructions must be provided"), and vice versa.
    let payload = vision_request_payload(
        &profile.model,
        profile.resolved_api_protocol(),
        profile.resolved_output_token_field().wire_name(),
        data_url,
    );
    let copilot = profile
        .credential
        .as_ref()
        .is_some_and(crate::provider_auth::CredentialRef::is_copilot);
    let protected_client = if copilot {
        Some(
            crate::provider_auth::github_copilot::http_client()
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let mut request = protected_client
        .as_ref()
        .unwrap_or(client)
        .post(profile.endpoint_url())
        .json(&payload);
    if copilot {
        request = request.headers(crate::provider_auth::github_copilot::request_headers(
            &payload, None,
        ));
    }
    if profile.resolved_api_protocol() == crate::config::ApiProtocol::AnthropicMessages {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let credential = tokio::select! {
        _ = cancel_token.cancelled() => return Err("cancelled".to_string()),
        result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            crate::provider_auth::resolve_profile_credential(profile),
        ) => match result {
            Err(_) => return Err("credential resolution timed out before image analysis was sent".to_string()),
            Ok(Err(error)) => return Err(format!("credential resolution failed before image analysis was sent: {error:#}")),
            Ok(Ok(credential)) => credential,
        }
    };
    if profile.credential.is_some() && credential.is_none() {
        return Err(
            "the configured vision provider credential could not be resolved; no request was sent"
                .to_string(),
        );
    }
    if let Some(key) = credential {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    let response = tokio::select! {
        _ = cancel_token.cancelled() => return Err("cancelled".to_string()),
        result = request.send() => result.map_err(|e| e.to_string())?,
    };
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let detail = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("provider rejected image analysis");
        return Err(format!("vision provider returned {status}: {detail}"));
    }
    vision_response_text(profile.resolved_api_protocol(), &body)
}

fn vision_request_payload(
    model: &str,
    protocol: crate::config::ApiProtocol,
    token_field: &str,
    data_url: String,
) -> serde_json::Value {
    match protocol {
        crate::config::ApiProtocol::AnthropicMessages => crate::network::anthropic_messages::request_payload(model, &[serde_json::json!({"role":"user","content":[{"type":"text","text":VISION_PROMPT},{"type":"image_url","image_url":{"url":data_url}}]})], &[], 2048, false).expect("vision input is a supported image data URL"),
        crate::config::ApiProtocol::Responses => {
            let mut map = serde_json::Map::new();
            map.insert(
                "model".to_owned(),
                serde_json::Value::String(model.to_owned()),
            );
            map.insert(
                "input".to_owned(),
                serde_json::json!([{
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": VISION_PROMPT},
                        {"type": "input_image", "image_url": data_url},
                    ]
                }]),
            );
            map.insert("stream".to_owned(), serde_json::Value::Bool(false));
            map.insert(
                token_field.to_owned(),
                serde_json::Value::Number(2048.into()),
            );
            serde_json::Value::Object(map)
        }
        _ => serde_json::json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": VISION_PROMPT},
                    {"type": "image_url", "image_url": {"url": data_url}}
                ]
            }],
            "stream": false,
            "max_tokens": 2048
        }),
    }
}

fn vision_response_text(
    protocol: crate::config::ApiProtocol,
    body: &serde_json::Value,
) -> Result<String, String> {
    if protocol == crate::config::ApiProtocol::AnthropicMessages {
        return crate::network::anthropic_messages::response_text(body)
            .ok_or_else(|| "vision provider returned no text content".to_owned());
    }
    if matches!(protocol, crate::config::ApiProtocol::Responses) {
        let mut texts = Vec::new();
        if let Some(items) = body.get("output").and_then(|o| o.as_array()) {
            for item in items {
                if item.get("type").and_then(|t| t.as_str()) != Some("message") {
                    continue;
                }
                if let Some(parts) = item.get("content").and_then(|c| c.as_array()) {
                    for part in parts {
                        if part.get("type").and_then(|t| t.as_str()) == Some("output_text")
                            && let Some(text) = part.get("text").and_then(|t| t.as_str())
                        {
                            texts.push(text);
                        }
                    }
                }
            }
        }
        if texts.is_empty() {
            return Err("vision provider returned no text content".to_string());
        }
        return Ok(texts.join("\n"));
    }
    body.get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|content| content.as_str())
        .map(str::to_string)
        .ok_or_else(|| "vision provider returned no text content".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatMessage;
    use crate::config::ModelProfile;
    use std::collections::HashMap;
    use std::path::Path;
    use tempfile::tempdir;

    fn profile(supports_vision: Option<bool>) -> ModelProfile {
        ModelProfile {
            name: "main".into(),
            url: "http://main/v1/chat/completions".into(),
            model: "main-model".into(),
            context_window: None,
            engine: None,
            api_key: None,
            env_key: None,
            tool_protocol: None,
            enable_thinking: None,
            reasoning_effort: None,
            max_tokens: None,
            supports_vision,
            ..Default::default()
        }
    }

    fn image(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        format!("![image](file://{})", path.display())
    }

    #[test]
    fn vision_payload_matches_profile_protocol() {
        use crate::config::ApiProtocol;

        let responses = vision_request_payload(
            "deepseek-flash",
            ApiProtocol::Responses,
            "max_output_tokens",
            "data:image/png;base64,AAA".to_string(),
        );
        // Responses shape: input[], never messages[] — the mismatch caused
        // 400 "Either input or instructions must be provided".
        assert!(responses.get("messages").is_none(), "{responses}");
        let input = responses
            .get("input")
            .and_then(|i| i.as_array())
            .expect("input");
        assert_eq!(input.len(), 1);
        let content = input[0]
            .get("content")
            .and_then(|c| c.as_array())
            .expect("content");
        assert!(
            content
                .iter()
                .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("input_text"))
        );
        assert!(content.iter().any(|p| {
            p.get("type").and_then(|t| t.as_str()) == Some("input_image")
                && p.get("image_url").and_then(|u| u.as_str()) == Some("data:image/png;base64,AAA")
        }));
        assert_eq!(
            responses.get("max_output_tokens").and_then(|v| v.as_u64()),
            Some(2048)
        );

        let chat = vision_request_payload(
            "m",
            ApiProtocol::ChatCompletions,
            "max_tokens",
            "data:image/png;base64,AAA".to_string(),
        );
        assert!(chat.get("input").is_none(), "{chat}");
        assert!(chat.get("messages").and_then(|m| m.as_array()).is_some());
        assert_eq!(chat.get("max_tokens").and_then(|v| v.as_u64()), Some(2048));
    }

    #[test]
    fn vision_response_text_parses_both_protocols() {
        use crate::config::ApiProtocol;

        let responses_body = serde_json::json!({
            "output": [
                {"type": "reasoning", "summary": []},
                {"type": "message", "content": [
                    {"type": "output_text", "text": "a rustcode panel"},
                    {"type": "refusal", "refusal": "no"}
                ]},
            ]
        });
        assert_eq!(
            vision_response_text(ApiProtocol::Responses, &responses_body).as_deref(),
            Ok("a rustcode panel")
        );
        assert!(
            vision_response_text(ApiProtocol::Responses, &serde_json::json!({"output": []}))
                .is_err()
        );

        let chat_body = serde_json::json!({
            "choices": [{"message": {"content": "hello"}}]
        });
        assert_eq!(
            vision_response_text(ApiProtocol::ChatCompletions, &chat_body).as_deref(),
            Ok("hello")
        );
    }

    #[test]
    fn image_analysis_cache_is_bounded() {
        let mut cache = HashMap::new();
        for index in 0..(MAX_IMAGE_ANALYSIS_CACHE_ENTRIES + 10) {
            extend_bounded_cache(
                &mut cache,
                HashMap::from([(format!("hash-{index}"), format!("analysis-{index}"))]),
            );
        }
        assert_eq!(cache.len(), MAX_IMAGE_ANALYSIS_CACHE_ENTRIES);
    }

    #[tokio::test]
    async fn vision_capable_model_keeps_native_image_marker() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "one.png", b"one");
        let mut history = vec![ChatMessage::new("user", format!("Look\n{marker}"))];
        let mut cache = HashMap::new();
        let mut calls = 0;

        preprocess_history_with(
            &mut history,
            &profile(Some(true)),
            &profile(Some(true)),
            &mut cache,
            |_, _| {
                calls += 1;
                async { Ok("should not run".to_string()) }
            },
        )
        .await
        .unwrap();

        assert_eq!(calls, 0);
        assert!(history[0].content.contains("![image](file://"));
    }

    #[tokio::test]
    async fn text_only_model_uses_vision_and_preserves_original_text() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "one.png", b"one");
        let main = profile(Some(false));
        let mut vision = profile(Some(true));
        vision.model = "vision-model".to_string();
        let mut history = vec![ChatMessage::new(
            "user",
            format!("Please inspect\n{marker}\nThanks"),
        )];
        let mut cache = HashMap::new();
        let mut seen = Vec::new();
        let mut seen_models = Vec::new();

        preprocess_history_with(
            &mut history,
            &main,
            &vision,
            &mut cache,
            |profile, bytes| {
                seen_models.push(profile.model.clone());
                seen.push(bytes.clone());
                async { Ok("Visible text: Save\nLayout: toolbar above editor".to_string()) }
            },
        )
        .await
        .unwrap();

        assert_eq!(seen, vec![b"one".to_vec()]);
        assert_eq!(seen_models, vec!["vision-model"]);
        assert!(history[0].content.contains("[Attached image analysis]"));
        assert!(history[0].content.contains("Visible text: Save"));
        assert!(history[0].content.contains("Please inspect"));
        assert!(history[0].content.contains("Thanks"));
        assert!(!history[0].content.contains("file://"));
    }

    #[tokio::test]
    async fn request_preparation_rewrites_a_snapshot_without_replacing_display_history() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "one.png", b"one");
        let source = vec![ChatMessage::new("user", format!("Look at {marker}"))];
        let mut cache = HashMap::new();

        let mut prepared = source.clone();
        preprocess_history_with(
            &mut prepared,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, _| async { Ok("snapshot analysis".to_string()) },
        )
        .await
        .unwrap();

        assert!(source[0].content.contains("![image](file://"));
        assert!(prepared[0].content.contains("snapshot analysis"));
        assert!(!prepared[0].content.contains("file://"));
    }

    #[tokio::test]
    async fn multiple_images_are_analyzed_in_source_order() {
        let dir = tempdir().unwrap();
        let first = image(dir.path(), "one.png", b"one");
        let second = image(dir.path(), "two.png", b"two");
        let mut history = vec![ChatMessage::new("user", format!("{first}\nthen\n{second}"))];
        let mut cache = HashMap::new();
        let mut seen = Vec::new();

        preprocess_history_with(
            &mut history,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, bytes| {
                let label = String::from_utf8_lossy(&bytes).to_string();
                seen.push(label.clone());
                async move { Ok(format!("analysis {label}")) }
            },
        )
        .await
        .unwrap();

        assert_eq!(seen, vec!["one", "two"]);
        let content = &history[0].content;
        assert!(content.find("analysis one").unwrap() < content.find("analysis two").unwrap());
    }

    #[tokio::test]
    async fn successful_analysis_is_reused_from_hash_cache() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "one.png", b"same");
        let main = profile(Some(false));
        let vision = profile(Some(true));
        let mut cache = HashMap::new();
        let mut first_history = vec![ChatMessage::new("user", marker.clone())];
        let mut calls = 0;
        preprocess_history_with(&mut first_history, &main, &vision, &mut cache, |_, _| {
            calls += 1;
            async { Ok("cached analysis".to_string()) }
        })
        .await
        .unwrap();
        let mut second_history = vec![ChatMessage::new("user", marker)];
        preprocess_history_with(&mut second_history, &main, &vision, &mut cache, |_, _| {
            calls += 1;
            async { Ok("should not run".to_string()) }
        })
        .await
        .unwrap();

        assert_eq!(calls, 1);
        assert_eq!(first_history[0].content, second_history[0].content);
    }

    #[tokio::test]
    async fn vision_failure_does_not_rewrite_history() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "one.png", b"one");
        let original = format!("before {marker} after");
        let mut history = vec![ChatMessage::new("user", original.clone())];
        let mut cache = HashMap::new();
        let result = preprocess_history_with(
            &mut history,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, _| async { Err("vision unavailable".to_string()) },
        )
        .await;

        assert_eq!(
            result.unwrap_err(),
            "image analysis failed: vision unavailable"
        );
        assert_eq!(history[0].content, original);
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn missing_image_in_older_history_does_not_abort_the_new_turn() {
        let missing = std::env::temp_dir().join("rustcode-missing-image.png");
        let mut history = vec![
            ChatMessage::new(
                "user",
                format!("Earlier attachment: ![image](file://{})", missing.display()),
            ),
            ChatMessage::new("user", "What time is it?"),
        ];
        let mut cache = HashMap::new();
        let mut calls = 0;

        preprocess_history_with(
            &mut history,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, _| {
                calls += 1;
                async { Ok("unused".to_string()) }
            },
        )
        .await
        .unwrap();

        assert_eq!(calls, 0);
        assert!(
            history[0]
                .content
                .contains("[Attached image analysis unavailable: image missing]")
        );
        assert_eq!(history[1].content, "What time is it?");
    }

    #[tokio::test]
    async fn cancellation_aborts_without_caching_the_image() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "older.png", b"older");
        let mut history = vec![
            ChatMessage::new("user", marker),
            ChatMessage::new("user", "What time is it?"),
        ];
        let mut cache = HashMap::new();

        let result = preprocess_history_with(
            &mut history,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, _| async { Err(CANCELLED.to_string()) },
        )
        .await;

        assert_eq!(result.unwrap_err(), "image analysis failed: cancelled");
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn stale_cancelled_cache_entries_are_analyzed_again() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "one.png", b"one");
        let mut history = vec![ChatMessage::new("user", marker)];
        let mut cache = HashMap::from([(
            image_hash(b"one"),
            "[Attached image analysis unavailable: cancelled]".to_string(),
        )]);

        preprocess_history_with(
            &mut history,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, _| async { Ok("fresh analysis".to_string()) },
        )
        .await
        .unwrap();

        assert!(history[0].content.contains("fresh analysis"));
        assert_eq!(cache[&image_hash(b"one")], "fresh analysis");
    }

    #[tokio::test]
    async fn only_recent_image_messages_are_sent_for_analysis() {
        let dir = tempdir().unwrap();
        let mut history = Vec::new();
        for index in 0..ANALYZED_IMAGE_MESSAGES + 2 {
            let name = format!("{index}.png");
            let marker = image(dir.path(), &name, name.as_bytes());
            history.push(ChatMessage::new("user", format!("shot {index} {marker}")));
        }
        history.push(ChatMessage::new("user", "Summarize"));
        let mut cache = HashMap::new();
        let mut seen = Vec::new();

        preprocess_history_with(
            &mut history,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, bytes| {
                seen.push(String::from_utf8_lossy(&bytes).to_string());
                async { Ok("analysis".to_string()) }
            },
        )
        .await
        .unwrap();

        assert_eq!(seen, vec!["2.png", "3.png", "4.png", "5.png"]);
        assert!(history[0].content.contains(EARLIER_IMAGE_SKIPPED));
        assert!(history[0].content.starts_with("shot 0 "));
        assert!(history[5].content.contains("analysis"));
        assert!(!history[5].content.contains("file://"));
    }

    #[tokio::test]
    async fn active_model_named_as_its_own_vision_model_gets_native_images() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "one.png", b"one");
        let main = profile(None);
        let mut history = vec![ChatMessage::new("user", marker.clone())];
        let mut calls = 0;

        preprocess_history_with(&mut history, &main, &main, &mut HashMap::new(), |_, _| {
            calls += 1;
            async { Ok("unused".to_string()) }
        })
        .await
        .unwrap();

        assert_eq!(calls, 0);
        assert_eq!(history[0].content, marker);
    }

    #[tokio::test]
    async fn text_only_model_as_its_own_vision_model_fails_fast() {
        let dir = tempdir().unwrap();
        let older = image(dir.path(), "older.png", b"older");
        let latest = image(dir.path(), "latest.png", b"latest");
        let main = profile(Some(false));
        let mut calls = 0;

        let mut history = vec![
            ChatMessage::new("user", older.clone()),
            ChatMessage::new("user", "Continue"),
        ];
        preprocess_history_with(&mut history, &main, &main, &mut HashMap::new(), |_, _| {
            calls += 1;
            async { Ok("unused".to_string()) }
        })
        .await
        .unwrap();
        assert!(history[0].content.contains(EARLIER_IMAGE_SKIPPED));

        let mut history = vec![ChatMessage::new("user", latest)];
        let error =
            preprocess_history_with(&mut history, &main, &main, &mut HashMap::new(), |_, _| {
                calls += 1;
                async { Ok("unused".to_string()) }
            })
            .await
            .unwrap_err();
        assert!(
            error.contains("is also the configured vision_model"),
            "{error}"
        );
        assert_eq!(calls, 0);
    }

    #[tokio::test]
    async fn vision_failure_in_older_history_does_not_abort_the_new_turn() {
        let dir = tempdir().unwrap();
        let marker = image(dir.path(), "older.png", b"older");
        let mut history = vec![
            ChatMessage::new("user", marker),
            ChatMessage::new("user", "What time is it?"),
        ];
        let mut cache = HashMap::new();
        let mut calls = 0;

        preprocess_history_with(
            &mut history,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, _| {
                calls += 1;
                async { Err("vision provider returned no text content".to_string()) }
            },
        )
        .await
        .unwrap();

        assert_eq!(calls, 1);
        assert!(history[0].content.contains(
            "[Attached image analysis unavailable: vision provider returned no text content]"
        ));
        assert_eq!(history[1].content, "What time is it?");
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn text_without_images_is_unchanged_and_does_not_call_vision() {
        let mut history = vec![ChatMessage::new("user", "plain text")];
        let original = history[0].content.clone();
        let mut cache = HashMap::new();
        let mut calls = 0;
        preprocess_history_with(
            &mut history,
            &profile(Some(false)),
            &profile(Some(true)),
            &mut cache,
            |_, _| {
                calls += 1;
                async { Ok("unused".to_string()) }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, 0);
        assert_eq!(history[0].content, original);
    }
}
