use crate::app::AppState;
use std::sync::Arc;
tokio::sync::Mutex;

pub async fn fetch_model_quota(state: &Arc<Mutex<AppState>>, client: &reqwest::Client) {
    let (api_base, model_name) = {
        let s = state.lock().await;
        (s.api_base_url.clone(), s.model_name.clone())
    };

    if !api_base.contains("localhost:3000") {
        return;
    }

    if let Ok(res) = client.get(format!("{}/auth/status", api_base)).send().await {
        if let Ok(json) = res.json::<serde_json::Value>().await {
            if let Some(buckets) = json.get("buckets").and_then(|b| b.as_object()) {
                if let Some(val) = buckets.get(&model_name).and_then(|m| m.get("remainingFraction")).and_then(|f| f.as_f64()) {
                    let mut s = state.lock().await;
                    s.model_quota_remaining = Some((val * 100.0) as f32);
                }
            }
        }
    }
}
