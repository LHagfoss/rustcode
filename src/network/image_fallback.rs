use crate::app::ChatMessage;
use crate::config::ModelProfile;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const IMAGE_MARKER: &str = "![image](file://";

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
    if active_profile.image_input_supported() == Some(true) {
        return Ok(());
    }

    let mut rewritten = Vec::with_capacity(history.len());
    let mut pending_cache = HashMap::new();
    let mut image_number = 0usize;

    for message in history.iter() {
        if message.role != "user" || !message.content.contains(IMAGE_MARKER) {
            rewritten.push(message.content.clone());
            continue;
        }

        let mut output = String::new();
        let mut remaining = message.content.as_str();
        while let Some(start) = remaining.find(IMAGE_MARKER) {
            output.push_str(&remaining[..start]);
            let after_marker = &remaining[start + IMAGE_MARKER.len()..];
            let Some(end) = after_marker.find(')') else {
                output.push_str(&remaining[start..]);
                remaining = "";
                break;
            };
            let path = &after_marker[..end];
            let bytes = std::fs::read(path)
                .map_err(|e| format!("image analysis failed: could not read image: {e}"))?;
            let hash = image_hash(&bytes);
            let analysis =
                if let Some(value) = cache.get(&hash).or_else(|| pending_cache.get(&hash)) {
                    value.clone()
                } else {
                    let value = request(vision_profile, bytes)
                        .await
                        .map_err(|e| format!("image analysis failed: {e}"))?;
                    if value.trim().is_empty() {
                        return Err(
                            "image analysis failed: vision model returned empty output".to_string()
                        );
                    }
                    pending_cache.insert(hash.clone(), value.clone());
                    value
                };
            image_number += 1;
            output.push_str(&format_analysis(image_number, &analysis));
            remaining = &after_marker[end + 1..];
        }
        output.push_str(remaining);
        rewritten.push(output);
    }

    cache.extend(pending_cache);
    for (message, content) in history.iter_mut().zip(rewritten) {
        message.content = content;
    }
    Ok(())
}

fn image_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn format_analysis(number: usize, analysis: &str) -> String {
    format!("[Attached image analysis]\nImage {number}\n\n{analysis}\n\nEnd image analysis.",)
}

pub const VISION_PROMPT: &str = "Analyze this image for a coding agent. Return concise, information-dense structured text, not a generic caption. Extract exact visible text when readable; UI structure and hierarchy; layout and relative positioning; errors, stack traces, terminal output, code, filenames, buttons, labels, and state; diagrams and relationships; and visual details relevant to reproducing or debugging it. Mark uncertain or unreadable text explicitly. Use short labeled sections such as Visible text, Layout and visual structure, Important details, and Relevant errors/code/UI state.";

pub async fn preprocess_history(
    client: &reqwest::Client,
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel_token: &CancellationToken,
) -> Result<(), String> {
    let (active, captured_history, mut cache) = {
        let s = state.lock().await;
        (
            s.active_model_profile(),
            s.history.clone(),
            s.image_analysis_cache.clone(),
        )
    };
    if !captured_history
        .iter()
        .any(|m| m.role == "user" && m.content.contains(IMAGE_MARKER))
    {
        return Ok(());
    }
    let active = active.ok_or_else(|| {
        "image analysis failed: active model profile is not configured".to_string()
    })?;
    if active.image_input_supported() == Some(true) {
        return Ok(());
    }
    let vision = {
        let s = state.lock().await;
        s.vision_model_profile().ok_or_else(|| {
            "image analysis failed: configure a dedicated vision_model profile".to_string()
        })?
    };

    let mut working_history = captured_history.clone();
    preprocess_history_with(
        &mut working_history,
        &active,
        &vision,
        &mut cache,
        |profile, bytes| {
            let profile = profile.clone();
            let client = client.clone();
            let cancel_token = cancel_token.clone();
            async move { request_vision_analysis(&client, &profile, bytes, &cancel_token).await }
        },
    )
    .await?;

    let mut s = state.lock().await;
    if s.history != captured_history {
        return Err(
            "image analysis failed: conversation changed while the image was being analyzed"
                .to_string(),
        );
    }
    s.history = working_history;
    s.image_analysis_cache = cache;
    Ok(())
}

async fn request_vision_analysis(
    client: &reqwest::Client,
    profile: &ModelProfile,
    bytes: Vec<u8>,
    cancel_token: &CancellationToken,
) -> Result<String, String> {
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
    let payload = serde_json::json!({
        "model": profile.model,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": VISION_PROMPT},
                {"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{}", general_purpose::STANDARD.encode(bytes))}}
            ]
        }],
        "stream": false,
        "max_tokens": 2048
    });
    let mut request = client.post(profile.endpoint_url()).json(&payload);
    if let Some(key) = profile.resolved_api_key() {
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
            max_tokens: None,
            supports_vision,
        }
    }

    fn image(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        format!("![image](file://{})", path.display())
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
