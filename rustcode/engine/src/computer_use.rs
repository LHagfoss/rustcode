//! Opt-in browser tools carried by the existing authenticated RustCode MCP hub.
use crate::orchestration::{
    ask_user::Asker,
    hub::{CallAnswer, CallRequest},
};
use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    net::TcpStream,
    process::{Child, Command},
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub allowed_domains: Vec<String>,
    pub executable: Option<PathBuf>,
}
impl Settings {
    pub fn load() -> Self {
        #[derive(Default, Deserialize, serde::Serialize)]
        struct File {
            #[serde(default)]
            computer_use: Settings,
        }
        crate::config::get_config_dir()
            .and_then(|dir| std::fs::read_to_string(dir.join("config.toml")).ok())
            .and_then(|body| toml::from_str::<File>(&body).ok())
            .unwrap_or_default()
            .computer_use
    }
    fn allows(&self, address: &str) -> bool {
        let Ok(url) = url::Url::parse(address) else {
            return false;
        };
        if !matches!(url.scheme(), "https" | "http")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return false;
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        self.allowed_domains.iter().any(|entry| {
            let entry = entry.to_ascii_lowercase();
            if let Some(base) = entry.strip_prefix("*.") {
                !base.is_empty() && host.ends_with(&format!(".{base}"))
            } else {
                host == entry
            }
        })
    }
}
pub(crate) fn settings_value() -> Value {
    let settings = Settings::load();
    json!({"enabled": settings.enabled, "allowed_domains": settings.allowed_domains,
        "browser_available": executable(&settings).is_some()})
}

pub(crate) fn save_settings(value: &Value) -> Result<(), String> {
    let enabled = value["enabled"]
        .as_bool()
        .ok_or("enabled must be a boolean")?;
    let domains: Vec<String> = serde_json::from_value(value["allowed_domains"].clone())
        .map_err(|_| "allowed_domains must be a list of hosts")?;
    if domains.iter().any(|host| !valid_domain(host)) {
        return Err(
            "Use a host such as example.com or *.example.org, without a URL or port".into(),
        );
    }
    let dir = crate::config::get_config_dir().ok_or("No config directory")?;
    let path = dir.join("config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let mut file: toml::Value = toml::from_str(&text).map_err(|e| e.to_string())?;
    if file.get("computer_use").is_none() {
        file["computer_use"] = toml::Value::Table(Default::default());
    }
    file["computer_use"]["enabled"] = toml::Value::Boolean(enabled);
    file["computer_use"]["allowed_domains"] =
        toml::Value::Array(domains.into_iter().map(toml::Value::String).collect());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(
        &temporary,
        toml::to_string_pretty(&file).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    crate::atomic_file::replace_file(&temporary, &path).map_err(|e| e.to_string())
}

fn valid_domain(entry: &str) -> bool {
    let host = entry.strip_prefix("*.").unwrap_or(entry);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

const NAMES: [&str; 5] = [
    "browser_open",
    "browser_read",
    "browser_screenshot",
    "browser_click",
    "browser_type",
];
pub fn is_tool(name: &str) -> bool {
    NAMES.contains(&name)
}
pub fn tools(settings: &Settings) -> Vec<Value> {
    if !settings.enabled {
        return vec![];
    }
    NAMES.iter().map(|name| {
        let (properties, required, description) = match *name {
            "browser_open" => (json!({"url":{"type":"string"}}), json!(["url"]), "Open an allowed HTTP(S) URL in RustCode's isolated headless browser. Requires approval."),
            "browser_click" => (json!({"selector":{"type":"string"}}), json!(["selector"]), "Click an element by CSS selector. Requires approval."),
            "browser_type" => (json!({"selector":{"type":"string"},"text":{"type":"string"}}), json!(["selector","text"]), "Focus an element by CSS selector and insert text. Requires approval."),
            "browser_read" => (json!({}), json!([]), "Read visible page text (up to 32000 characters). Requires approval."),
            _ => (json!({}), json!([]), "Capture the browser viewport as a PNG. Returns base64 PNG data. Requires approval."),
        };
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
    }).collect()
}

type Job = (String, Value, oneshot::Sender<Result<Value, String>>);
struct Browser {
    sender: mpsc::Sender<Job>,
    stop: tokio_util::sync::CancellationToken,
}
impl Drop for Browser {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
type Browsers = Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<Browser>>>>>;
fn browsers() -> &'static Browsers {
    static ALL: OnceLock<Browsers> = OnceLock::new();
    ALL.get_or_init(Default::default)
}
pub fn unregister(id: &str) {
    browsers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
}

pub async fn answer(
    request: &CallRequest,
    asker: Option<Asker>,
    cancel: &tokio_util::sync::CancellationToken,
) -> CallAnswer {
    let result = async {
        let settings = Settings::load();
        if !settings.enabled { return Err("Computer use is disabled".to_owned()) }
        let text = |key: &str| request.args.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| format!("Missing string: {key}"));
        match request.tool.as_str() {
            "browser_open" => { if !settings.allows(&text("url")?) { return Err("URL is outside computer_use.allowed_domains".into()) } },
            "browser_click" => { text("selector")?; },
            "browser_type" => { text("selector")?; text("text")?; },
            _ => {},
        }
        let asker = asker.ok_or("No approval channel is attached; nothing was run")?;
        let mut preview = request.args.clone();
        if let Some(text) = preview.get_mut("text") { *text = json!("[redacted]"); }
        let permission = RequestPermissionRequest::new(request.session_id.clone(), ToolCallUpdate::new(
            format!("computer-use-{:016x}", rand::random::<u64>()),
            ToolCallUpdateFields::new().title(format!("{}: {}", request.tool, preview)).raw_input(preview),
        ), vec![PermissionOption::new("allow", "Allow once", PermissionOptionKind::AllowOnce), PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce)]);
        let approved = tokio::select! { _ = cancel.cancelled() => return Err("Cancelled".into()), response = asker(permission) => response };
        if !approved.is_some_and(|r| matches!(r.outcome, RequestPermissionOutcome::Selected(ref choice) if choice.option_id.to_string() == "allow")) { return Err("Browser action denied".into()) }
        // Recheck the switch after the user answers the card.
        if !Settings::load().enabled { return Err("Computer use is disabled".into()) }
        run_approved(&request.session_id, &request.tool, &request.args, cancel).await
    }.await;
    match result {
        Ok(content) => CallAnswer {
            success: true,
            content,
        },
        Err(error) => CallAnswer::failure(error),
    }
}

struct Chrome {
    child: Child,
    profile: PathBuf,
}
impl Drop for Chrome {
    fn drop(&mut self) {
        // kill_on_drop also covers startup errors; reap before removing the profile.
        let _ = self.child.start_kill();
        let profile = self.profile.clone();
        let _ = std::fs::remove_dir_all(profile);
    }
}
pub(crate) fn executable(settings: &Settings) -> Option<PathBuf> {
    settings
        .executable
        .clone()
        .or_else(|| {
            [
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
                "/usr/bin/google-chrome",
            ]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
        })
        .filter(|path| path.is_file())
}

async fn start(settings: &Settings) -> Result<Browser, String> {
    let executable =
        executable(settings).ok_or("Chrome/Chromium not found; set computer_use.executable")?;
    let profile = crate::config::get_config_dir()
        .ok_or("No config directory")?
        .join("computer-use")
        .join(format!("profile-{:032x}", rand::random::<u128>()));
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let child = Command::new(executable)
        .args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--disable-extensions",
            "--disable-background-networking",
            "--block-new-web-contents",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("about:blank")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let chrome = Chrome { child, profile };
    let port = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(body) =
                tokio::fs::read_to_string(chrome.profile.join("DevToolsActivePort")).await
            {
                if let Some(port) = body.lines().next().and_then(|s| s.parse::<u16>().ok()) {
                    break port;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "Chrome startup timed out")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let pages: Value = client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let endpoint = pages
        .as_array()
        .and_then(|pages| pages.iter().find(|p| p["type"] == "page"))
        .and_then(|p| p["webSocketDebuggerUrl"].as_str())
        .ok_or("Chrome did not expose a page")?;
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| e.to_string())?;
    let (ws, _) = tokio_tungstenite::client_async(endpoint, stream)
        .await
        .map_err(|e| e.to_string())?;
    let (sender, mut jobs) = mpsc::channel::<Job>(1);
    let stop = tokio_util::sync::CancellationToken::new();
    let stopped = stop.clone();
    tokio::spawn(async move {
        let mut chrome = chrome;
        let mut cdp = Cdp { ws, next: 0 };
        if cdp
            .command("Fetch.enable", json!({"patterns":[{"urlPattern":"*"}]}))
            .await
            .is_err()
        {
            return;
        }
        let _ = cdp
            .command(
                "Network.setBlockedURLs",
                json!({"urls":["file:*","ftp:*","data:*"]}),
            )
            .await;
        let _ = cdp
            .command("Page.setDownloadBehavior", json!({"behavior":"deny"}))
            .await;
        loop {
            tokio::select! {
                _ = stopped.cancelled() => break,
                message = cdp.ws.next() => {
                    match message { Some(Ok(Message::Text(body))) => { if let Ok(event) = serde_json::from_str::<Value>(&body) { if cdp.intercept(&event).await.is_err() { break } } }, Some(Ok(_)) => {}, _ => break }
                },
                job = jobs.recv() => {
                    let Some((tool, args, answer)) = job else { break };
                    let result = tokio::select! {
                        _ = stopped.cancelled() => break,
                        result = tokio::time::timeout(Duration::from_secs(40), cdp.action(&tool, &args)) => result.unwrap_or_else(|_| Err("CDP timed out".into())),
                    };
                    let failed = result.is_err(); let _ = answer.send(result); if failed { break }
                }
            }
        }
        let _ = chrome.child.kill().await;
        let _ = chrome.child.wait().await;
        let _ = std::fs::remove_dir_all(&chrome.profile);
    });
    Ok(Browser { sender, stop })
}
struct Cdp {
    ws: WebSocketStream<TcpStream>,
    next: u64,
}
impl Cdp {
    async fn send(&mut self, method: &str, params: Value) -> Result<u64, String> {
        self.next += 1;
        self.ws
            .send(Message::Text(
                json!({"id":self.next,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(self.next)
    }
    async fn intercept(&mut self, event: &Value) -> Result<(), String> {
        if event["method"] == "Fetch.requestPaused" {
            let allowed = Settings::load();
            let allowed = allowed.enabled
                && allowed.allows(event["params"]["request"]["url"].as_str().unwrap_or(""));
            let mut params = json!({"requestId":event["params"]["requestId"]});
            if !allowed {
                params["errorReason"] = json!("BlockedByClient");
            }
            self.send(
                if allowed {
                    "Fetch.continueRequest"
                } else {
                    "Fetch.failRequest"
                },
                params,
            )
            .await?;
        }
        Ok(())
    }
    async fn command(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.send(method, params).await?;
        while let Some(message) = self.ws.next().await {
            let message = message.map_err(|e| e.to_string())?;
            if let Message::Text(body) = message {
                let event: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                self.intercept(&event).await?;
                if event["id"] == id {
                    if let Some(error) = event.get("error") {
                        return Err(error.to_string());
                    }
                    return Ok(event["result"].clone());
                }
            }
        }
        Err("Chrome disconnected".into())
    }
    async fn evaluate(&mut self, expression: String) -> Result<Value, String> {
        let response = self
            .command(
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true}),
            )
            .await?;
        if response.get("exceptionDetails").is_some() {
            return Err(response.to_string());
        }
        Ok(response["result"]["value"].clone())
    }
    async fn action(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        if tool == "browser_open" {
            let url = args["url"].as_str().ok_or("Missing URL")?;
            if !Settings::load().allows(url) {
                return Err("URL refused".into());
            }
            let response = self.command("Page.navigate", json!({"url":url})).await?;
            if response.get("errorText").is_some() {
                return Err(response.to_string());
            }
            return Ok(response);
        }
        let location = self.evaluate("location.href".into()).await?;
        if !Settings::load().allows(location.as_str().unwrap_or("")) {
            return Err("Current page is outside the allowlist; open an allowed URL".into());
        }
        match tool {
            "browser_read" => {
                self.evaluate("document.body?.innerText.slice(0,32000) || ''".into())
                    .await
            }
            "browser_screenshot" => {
                self.command("Page.captureScreenshot", json!({"format":"png"}))
                    .await
            }
            "browser_click" | "browser_type" => {
                let selector =
                    serde_json::to_string(&args["selector"]).map_err(|e| e.to_string())?;
                self.evaluate(format!("(()=>{{const e=document.querySelector({selector});if(!e)throw Error('Selector not found');e.scrollIntoView({{block:'center'}});e.focus();{};return true}})()", if tool == "browser_click" { "e.click()" } else { "" })).await?;
                if tool == "browser_type" {
                    self.command("Input.insertText", json!({"text":args["text"]}))
                        .await?;
                }
                Ok(json!({"ok":true}))
            }
            _ => Err("Unknown browser tool".into()),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_by_default() {
        assert!(!Settings::default().enabled);
        assert!(tools(&Settings::default()).is_empty());
        assert_eq!(
            tools(&Settings {
                enabled: true,
                ..Default::default()
            })
            .len(),
            5
        );
    }
    #[test]
    fn allowlist_checks_scheme_credentials_and_domain_boundaries() {
        let s = Settings {
            allowed_domains: vec!["example.com".into(), "*.example.org".into()],
            ..Default::default()
        };
        for url in [
            "https://example.com/a",
            "http://EXAMPLE.COM:8080/",
            "https://a.example.org/",
        ] {
            assert!(s.allows(url), "{url}");
        }
        for url in [
            "https://example.com.evil.test",
            "https://evilexample.com",
            "https://example.org",
            "file:///etc/passwd",
            "data:text/html,hello",
            "https://user@example.com",
            "https://example.com@evil.test",
            "not a url",
        ] {
            assert!(!s.allows(url), "{url}");
        }
        assert!(!Settings::default().allows("https://example.com"));
    }
}

/// The built-in harness reaches this only after its normal tool permission gate.
pub async fn run_approved(
    session_id: &str,
    tool: &str,
    args: &Value,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    let settings = Settings::load();
    if !settings.enabled {
        return Err("Computer use is disabled".into());
    }
    if !is_tool(tool) {
        return Err("Unknown browser tool".into());
    }
    for key in match tool {
        "browser_open" => vec!["url"],
        "browser_click" => vec!["selector"],
        "browser_type" => vec!["selector", "text"],
        _ => vec![],
    } {
        if args.get(key).and_then(Value::as_str).is_none() {
            return Err(format!("Missing string: {key}"));
        }
    }
    if tool == "browser_open" && !settings.allows(args["url"].as_str().unwrap_or("")) {
        return Err("URL refused".into());
    }
    let slot = browsers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(session_id.to_owned())
        .or_default()
        .clone();
    let mut browser = slot.lock().await;
    if browser.is_none() {
        *browser = Some(tokio::select! {
            _ = cancel.cancelled() => return Err("Cancelled".into()),
            started = tokio::time::timeout(Duration::from_secs(20), start(&settings)) => started.map_err(|_| "Browser startup timed out")??,
        });
    }
    let (tx, rx) = oneshot::channel();
    browser
        .as_ref()
        .unwrap()
        .sender
        .send((tool.to_owned(), args.clone(), tx))
        .await
        .map_err(|_| "Browser stopped")?;
    let result = tokio::select! {
        _ = cancel.cancelled() => Err("Cancelled".to_owned()),
        result = tokio::time::timeout(Duration::from_secs(45), rx) => result.map_err(|_| "Browser action timed out".to_owned()).and_then(|r| r.map_err(|_| "Browser stopped".to_owned())).and_then(|r| r),
    };
    if result.is_err() {
        *browser = None;
    }
    result.map(|value| value.to_string())
}

#[cfg(test)]
mod settings_tests {
    #[test]
    fn domain_entries_are_hosts_and_optional_subdomain_wildcards() {
        for host in ["example.com", "*.example.org", "localhost", "EXAMPLE.COM"] {
            assert!(super::valid_domain(host));
        }
        for host in [
            "",
            "*",
            "https://example.com",
            "example.com:443",
            "a..com",
            "-a.com",
            "a/com",
        ] {
            assert!(!super::valid_domain(host));
        }
        assert!(!super::Settings::default().enabled);
    }
}
