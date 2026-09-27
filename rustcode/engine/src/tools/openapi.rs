use serde_json::Value;

use super::{Tool, ToolCapability, ToolSafety};

const MAX_API_OUTPUT_BYTES: usize = 20_000;
const SPEC_TIMEOUT_SECS: u64 = 10;
const CALL_TIMEOUT_SECS: u64 = 30;

fn openapi_call_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "spec_url": { "type": "string", "description": "https URL of the OpenAPI 3.x JSON spec" },
            "method": { "type": "string", "description": "HTTP method (GET, POST, PUT, PATCH, DELETE)" },
            "path": { "type": "string", "description": "API path from the spec (must start with /)" },
            "query": { "type": "string", "description": "Optional raw query string without leading ?" },
            "body": { "type": "object", "description": "Optional JSON body for POST/PUT/PATCH" }
        },
        "required": ["spec_url", "method", "path"]
    })
}

pub const OPENAPI_CALL: Tool = Tool {
    name: "openapi_call",
    description: "Call an HTTP endpoint described by an OpenAPI 3.x JSON spec: validates method+path against the spec, then performs the request. Any API becomes one tool.",
    arguments: r#"{"spec_url": "https://api.example.com/openapi.json", "method": "GET", "path": "/v1/things", "query": "limit=10", "body": {}}"#,
    handler: openapi_call,
    requires_confirmation: true,
    schema: openapi_call_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::Unknown,
};

fn validate_request(spec_url: &str, method: &str, path: &str) -> Result<String, String> {
    if !(spec_url.starts_with("https://") || spec_url.starts_with("http://localhost")) {
        return Err("spec_url must be https (http is allowed only for localhost)".to_string());
    }
    if spec_url.contains('\0') || path.contains('\0') {
        return Err("NUL byte in request".to_string());
    }
    let method = method.to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
        return Err(format!("unsupported method: '{method}'"));
    }
    if !path.starts_with('/') {
        return Err("path must start with '/'".to_string());
    }
    Ok(method)
}

fn spec_base_url(spec: &Value, spec_url: &str) -> String {
    spec.get("servers")
        .and_then(Value::as_array)
        .and_then(|servers| servers.first())
        .and_then(|server| server.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            spec_url
                .split_once("://")
                .map(|(scheme, rest)| {
                    let host = rest.split('/').next().unwrap_or(rest);
                    format!("{scheme}://{host}")
                })
                .unwrap_or_default()
        })
}

fn truncate_output(text: &str) -> String {
    super::truncate_bytes(text, MAX_API_OUTPUT_BYTES)
}

async fn fetch_spec(client: &reqwest::Client, spec_url: &str) -> Result<Value, String> {
    let response = client
        .get(spec_url)
        .timeout(std::time::Duration::from_secs(SPEC_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("failed to fetch spec: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("spec fetch failed: HTTP {}", response.status()));
    }
    response
        .json::<Value>()
        .await
        .map_err(|e| format!("spec is not valid JSON: {e}"))
}

fn validate_against_spec(spec: &Value, method: &str, path: &str) -> Result<(), String> {
    let paths = spec
        .get("paths")
        .and_then(Value::as_object)
        .ok_or("spec has no 'paths' object (not OpenAPI 3.x?)")?;
    let item = paths.get(path).ok_or_else(|| {
        let mut available: Vec<&String> = paths.keys().collect();
        available.sort();
        let sample = available
            .iter()
            .take(8)
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("path '{path}' not in spec. Sample paths: {sample}")
    })?;
    let method_key = method.to_ascii_lowercase();
    if item.get(&method_key).is_none() {
        return Err(format!("method {method} not defined for path '{path}'"));
    }
    Ok(())
}

async fn perform_call(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    body: Option<&Value>,
) -> Result<String, String> {
    let mut request = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "PATCH" => client.patch(url),
        "DELETE" => client.delete(url),
        _ => return Err(format!("unsupported method: '{method}'")),
    }
    .timeout(std::time::Duration::from_secs(CALL_TIMEOUT_SECS));
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("API call failed: {e}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("failed to read response: {e}"))?;
    Ok(truncate_output(&format!("HTTP {status}\n{text}")))
}

async fn openapi_call_async(args: &Value) -> Result<String, String> {
    let spec_url = args
        .get("spec_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or("missing 'spec_url'")?;
    let method_raw = args
        .get("method")
        .and_then(Value::as_str)
        .ok_or("missing 'method'")?;
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or("missing 'path'")?;
    if spec_url.is_empty() || path.is_empty() {
        return Err("spec_url and path must not be empty".to_string());
    }
    let method = validate_request(spec_url, method_raw, path)?;
    if matches!(method.as_str(), "GET" | "DELETE") && args.get("body").is_some() {
        return Err("body is only allowed for POST/PUT/PATCH".to_string());
    }
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty());

    let client = reqwest::Client::new();
    let spec = fetch_spec(&client, spec_url).await?;
    validate_against_spec(&spec, &method, path)?;

    let mut url = format!("{}{}", spec_base_url(&spec, spec_url), path);
    if let Some(query) = query {
        if query.contains('\0') || query.contains(['<', '>', '"']) {
            return Err("invalid characters in query".to_string());
        }
        url.push('?');
        url.push_str(query.trim_start_matches('?'));
    }
    perform_call(&client, &method, &url, args.get("body")).await
}

pub fn openapi_call(args: &Value) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to create async runtime: {e}"))?;
    runtime.block_on(openapi_call_async(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_https_spec_urls() {
        assert!(validate_request("http://example.com/spec.json", "GET", "/v1/x").is_err());
        assert!(validate_request("https://example.com/spec.json", "GET", "/v1/x").is_ok());
        assert!(validate_request("http://localhost:3000/spec.json", "GET", "/v1/x").is_ok());
    }

    #[test]
    fn rejects_bad_methods_and_paths() {
        assert!(validate_request("https://example.com/s.json", "BREW", "/v1/x").is_err());
        assert!(validate_request("https://example.com/s.json", "GET", "v1/x").is_err());
    }

    #[test]
    fn spec_validation_reports_available_paths() {
        let spec = serde_json::json!({
            "paths": { "/v1/things": { "get": {} } }
        });
        assert!(validate_against_spec(&spec, "GET", "/v1/things").is_ok());
        assert!(validate_against_spec(&spec, "POST", "/v1/things").is_err());
        let err = validate_against_spec(&spec, "GET", "/v1/missing").unwrap_err();
        assert!(err.contains("/v1/things"));
    }

    #[test]
    fn spec_base_prefers_servers_url_then_origin() {
        let spec = serde_json::json!({"servers": [{"url": "https://api.example.com"}]});
        assert_eq!(
            spec_base_url(&spec, "https://other.example.com/spec.json"),
            "https://api.example.com"
        );
        let empty = serde_json::json!({});
        assert_eq!(
            spec_base_url(&empty, "https://other.example.com/a/b.json"),
            "https://other.example.com"
        );
    }

    #[test]
    fn rejects_body_on_get() {
        let args = serde_json::json!({
            "spec_url": "https://example.com/spec.json",
            "method": "GET",
            "path": "/v1/x",
            "body": {}
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(runtime.block_on(openapi_call_async(&args)).is_err());
    }
}
