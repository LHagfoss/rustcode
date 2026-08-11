use crate::app::AppState;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn fetch_model_quota(client: &reqwest::Client, state: &Arc<Mutex<AppState>>) {
    let (url, model_name, api_key_opt) = {
        let s = state.lock().await;
        let active_url = s.api_base_url.clone();
        let key = s
            .config
            .models
            .iter()
            .find(|m| m.url == active_url || m.model == s.model_name)
            .and_then(|m| m.api_key.clone());
        (active_url, s.model_name.clone(), key)
    };

    if !url.contains("localhost:3000")
        && !url.contains("127.0.0.1:3000")
        && !url.contains("127.0.0.1:10531")
        && !url.contains("localhost:10531")
    {
        return;
    }

    // Construct proxy base URL (remove /v1/chat/completions or trailing slashes)
    let base_url = if let Some(idx) = url.find("/v1") {
        &url[..idx]
    } else {
        url.trim_end_matches('/')
    };
    let status_url = format!("{}/auth/status", base_url);

    let mut req = client.get(&status_url);
    if let Some(key) = api_key_opt {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let Ok(res) = req.send().await else {
        return;
    };
    let Ok(json) = res.json::<serde_json::Value>().await else {
        return;
    };

    let quota_obj = json.get("quota");
    let buckets_arr = quota_obj
        .and_then(|q| q.get("buckets").or_else(|| q.get("quotaBuckets")))
        .and_then(|b| b.as_array());

    if let Some(quota_buckets) = buckets_arr {
        let mut matched_pct = None;
        for bucket in quota_buckets {
            if let Some(model_id) = bucket.get("modelId").and_then(|m| m.as_str())
                && let Some(fraction) = bucket.get("remainingFraction").and_then(|f| f.as_f64())
            {
                let pct = (fraction * 100.0) as f32;
                if matched_pct.is_none() {
                    matched_pct = Some(pct);
                }
                if model_id == model_name
                    || model_name.contains(model_id)
                    || model_id.contains(&model_name)
                {
                    matched_pct = Some(pct);
                    break;
                }
            }
        }
        if let Some(pct) = matched_pct {
            let mut s = state.lock().await;
            s.model_quota_remaining = Some(pct);
            s.request_redraw();
        }
        return;
    }

    // The ChatGPT/Codex usage response reports account-wide rate limits rather
    // than per-model Gemini-style buckets. Use the primary window for the
    // footer quota indicator; /status and /quota display both windows.
    let primary_window = json
        .get("rate_limits")
        .and_then(|r| r.get("primary"))
        .or_else(|| json.get("rate_limit").and_then(|r| r.get("primary_window")));
    if let Some(used_percent) = primary_window
        .and_then(|p| p.get("used_percent"))
        .and_then(|v| v.as_f64())
    {
        let mut s = state.lock().await;
        s.model_quota_remaining = Some((100.0 - used_percent).clamp(0.0, 100.0) as f32);
        s.request_redraw();
    }
}

pub fn parse_multimodal_content(text: &str) -> serde_json::Value {
    let clean_text = if text.contains("<!--PASTE:") {
        let mut out = String::new();
        let mut rest = text;
        while let Some(idx) = rest.find("<!--PASTE:") {
            out.push_str(&rest[..idx]);
            let after = &rest[idx + "<!--PASTE:".len()..];
            if let Some(end) = after.find("-->") {
                let payload = &after[..end];
                if let Some((_, body)) = payload.split_once(':') {
                    out.push_str(body);
                } else {
                    out.push_str(payload);
                }
                rest = &after[end + 3..];
            } else {
                out.push_str(&rest[idx..]);
                rest = "";
                break;
            }
        }
        out.push_str(rest);
        out
    } else {
        text.to_string()
    };

    if !clean_text.contains("![image](file://") {
        return serde_json::Value::String(clean_text);
    }

    let mut parts: Vec<serde_json::Value> = Vec::new();
    let mut remaining = clean_text.as_str();

    while let Some(start_idx) = remaining.find("![image](file://") {
        let text_part = &remaining[..start_idx];
        if !text_part.is_empty() {
            parts.push(serde_json::json!({
                "type": "text",
                "text": text_part.to_string(),
            }));
        }

        let path_start = start_idx + "![image](file://".len();
        let rest = &remaining[path_start..];
        if let Some(end_idx) = rest.find(')') {
            let path_str = &rest[..end_idx];
            if let Ok(bytes) = std::fs::read(path_str) {
                use base64::{Engine as _, engine::general_purpose};
                let base64_str = general_purpose::STANDARD.encode(bytes);
                let mime = if path_str.ends_with(".jpg") || path_str.ends_with(".jpeg") {
                    "image/jpeg"
                } else if path_str.ends_with(".gif") {
                    "image/gif"
                } else if path_str.ends_with(".webp") {
                    "image/webp"
                } else {
                    "image/png"
                };
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{};base64,{}", mime, base64_str),
                    }
                }));
            } else {
                parts.push(serde_json::json!({
                    "type": "text",
                    "text": format!("![image](file://{})", path_str),
                }));
            }
            remaining = &rest[end_idx + 1..];
        } else {
            break;
        }
    }

    if !remaining.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": remaining.to_string(),
        }));
    }

    serde_json::Value::Array(parts)
}
