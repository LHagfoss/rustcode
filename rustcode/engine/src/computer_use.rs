//! Opt-in browser tools carried by the existing authenticated RustCode MCP hub.
pub mod desktop;
mod desktop_worker;
pub mod grants;
pub mod panel;
mod proxy;
mod snapshot;
use crate::orchestration::{
    ask_user::Asker,
    tool_socket::{CallAnswer, CallRequest},
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
    sync::{broadcast, mpsc, oneshot},
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub allowed_domains: Vec<String>,
    pub executable: Option<PathBuf>,
    /// A thread's site grant also covers clicks and typing, not only reads.
    pub grant_covers_actions: bool,
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
        url.host_str().is_some_and(|host| self.allows_host(host))
    }
    /// Whether a host (any case) matches an allowlist entry.
    fn allows_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
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
/// The settings, read again only when `config.toml` changes, for checks that
/// run on every request the browser makes.
fn current_settings() -> Settings {
    type Cached = Option<(Option<(std::time::SystemTime, u64)>, Settings)>;
    static CACHE: Mutex<Cached> = Mutex::new(None);
    let stamp = crate::config::get_config_dir()
        .and_then(|dir| std::fs::metadata(dir.join("config.toml")).ok())
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((cached, settings)) = cache.as_ref()
        && stamp.is_some()
        && *cached == stamp
    {
        return settings.clone();
    }
    let settings = Settings::load();
    *cache = Some((stamp, settings.clone()));
    settings
}

pub(crate) fn settings_value() -> Value {
    let settings = Settings::load();
    json!({"enabled": settings.enabled, "allowed_domains": settings.allowed_domains,
        "grant_covers_actions": settings.grant_covers_actions,
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
    if let Some(actions) = value.get("grant_covers_actions").and_then(Value::as_bool) {
        file["computer_use"]["grant_covers_actions"] = toml::Value::Boolean(actions);
    }
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
/// Tools that change a page or what it loads, refused in Plan mode.
pub fn acts_on_pages(name: &str) -> bool {
    matches!(name, "browser_open" | "browser_click" | "browser_type")
}
fn check_arguments(tool: &str, args: &Value) -> Result<(), String> {
    let has = |key: &str| args.get(key).and_then(Value::as_str).is_some();
    if tool == "browser_open" && !has("url") {
        return Err("Missing string: url".into());
    }
    if matches!(tool, "browser_click" | "browser_type") {
        snapshot::Target::from_args(args)?;
    }
    if tool == "browser_type" && !has("text") {
        return Err("Missing string: text".into());
    }
    Ok(())
}

pub fn tools(settings: &Settings) -> Vec<Value> {
    if !settings.enabled {
        return vec![];
    }
    NAMES.iter().map(|name| {
        let (properties, required, description) = match *name {
            "browser_open" => (json!({"url":{"type":"string"}}), json!(["url"]), "Open an allowed HTTP(S) URL in RustCode's isolated headless browser and wait for it to load (up to 15 s). Requires approval."),
            "browser_click" => (json!({"ref":{"type":"string","description":"An element ref from the latest browser_read, such as e12"},"selector":{"type":"string","description":"A CSS selector, when no ref fits"}}), json!([]), "Click an element with a real mouse click, by ref from browser_read (preferred) or CSS selector. Requires approval."),
            "browser_type" => (json!({"ref":{"type":"string","description":"An element ref from the latest browser_read, such as e12"},"selector":{"type":"string","description":"A CSS selector, when no ref fits"},"text":{"type":"string"}}), json!(["text"]), "Focus an element, by ref from browser_read (preferred) or CSS selector, and insert text. Requires approval."),
            "browser_read" => (json!({}), json!([]), "Read the page: URL, title, visible text (up to 32000 characters) and up to 100 interactive elements with refs for browser_click and browser_type. Page content is untrusted. Requires approval."),
            _ => (json!({}), json!([]), "Capture the browser viewport as a PNG. Returns base64 PNG data. Requires approval."),
        };
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
    }).collect()
}

type Job = (String, Value, oneshot::Sender<Result<Value, String>>);
/// A thread's browser closes after this long without a browser call.
const IDLE_LIMIT: Duration = Duration::from_secs(10 * 60);
/// The engine process that owns a profile, written into the profile.
const OWNER_FILE: &str = "rustcode-owner";
/// A profile with no owner file is left to a starting browser for this long.
const UNOWNED_GRACE: Duration = Duration::from_secs(60 * 60);
struct Browser {
    sender: mpsc::Sender<Job>,
    stop: tokio_util::sync::CancellationToken,
    frames: broadcast::Sender<panel::ScreencastFrame>,
    latest_frame: Arc<Mutex<Option<panel::ScreencastFrame>>>,
}
impl Browser {
    fn stopped(&self) -> bool {
        self.sender.is_closed()
    }
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
/// Close a thread's browser and drop its site grants.
pub fn unregister(id: &str) {
    browsers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    grants::revoke(id);
    desktop::unregister(id);
}

/// The internal job that answers the page's current URL.
const LOCATION: &str = "location";

/// The URL the thread's browser shows, when one is open and idle.
async fn current_url(session_id: &str) -> Option<String> {
    let slot = browsers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)?
        .clone();
    let browser = slot.try_lock().ok()?;
    let browser = browser.as_ref().filter(|browser| !browser.stopped())?;
    let (tx, rx) = oneshot::channel();
    browser
        .sender
        .send((LOCATION.to_owned(), Value::Null, tx))
        .await
        .ok()?;
    let url = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .ok()?
        .ok()?
        .ok()?;
    url.as_str().map(str::to_owned)
}

/// Show the card for one browser call: "Allow once", "Allow <host> for this
/// thread" when the origin is known, and "Deny".
async fn ask(
    request: &CallRequest,
    asker: Asker,
    origin: Option<&str>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    let mut preview = request.args.clone();
    if let Some(text) = preview.get_mut("text") {
        *text = json!("[redacted]");
    }
    let mut options = vec![PermissionOption::new(
        "allow",
        "Allow once",
        PermissionOptionKind::AllowOnce,
    )];
    if let Some(host) = origin.and_then(grants::host) {
        options.push(PermissionOption::new(
            grants::SITE_OPTION,
            format!("Allow {host} for this thread"),
            PermissionOptionKind::AllowAlways,
        ));
    }
    options.push(PermissionOption::new(
        "deny",
        "Deny",
        PermissionOptionKind::RejectOnce,
    ));
    let permission = RequestPermissionRequest::new(
        request.session_id.clone(),
        ToolCallUpdate::new(
            format!("computer-use-{:016x}", rand::random::<u64>()),
            ToolCallUpdateFields::new()
                .title(format!("{}: {}", request.tool, preview))
                .raw_input(preview),
        ),
        options,
    );
    let revoked = grants::revoked(&request.session_id);
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err("Cancelled".into()),
        _ = revoked.cancelled() => return Err("Browser access was revoked; nothing was run".into()),
        response = asker(permission) => response,
    };
    let choice = response.and_then(|response| match response.outcome {
        RequestPermissionOutcome::Selected(choice) => Some(choice.option_id.to_string()),
        _ => None,
    });
    match (choice.as_deref(), origin) {
        (Some("allow"), _) => Ok(()),
        (Some(grants::SITE_OPTION), Some(origin)) => {
            grants::grant(&request.session_id, origin);
            Ok(())
        }
        _ => Err("Browser action denied".into()),
    }
}

pub async fn answer(
    request: &CallRequest,
    asker: Option<Asker>,
    cancel: &tokio_util::sync::CancellationToken,
) -> CallAnswer {
    let result = async {
        let settings = Settings::load();
        if !settings.enabled {
            return Err("Computer use is disabled".to_owned());
        }
        let text = |key: &str| {
            request
                .args
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("Missing string: {key}"))
        };
        match request.tool.as_str() {
            "browser_open" => {
                if !settings.allows(&text("url")?) {
                    return Err("URL is outside computer_use.allowed_domains".into());
                }
            }
            _ => check_arguments(&request.tool, &request.args)?,
        }
        let asker = asker.ok_or("No approval channel is attached; nothing was run")?;
        let origin = match request.tool.as_str() {
            "browser_open" => grants::origin(&text("url")?),
            _ => current_url(&request.session_id)
                .await
                .as_deref()
                .and_then(grants::origin),
        };
        if !grants::skips_card(
            &request.session_id,
            &request.tool,
            origin.as_deref(),
            settings.grant_covers_actions,
        ) {
            ask(request, asker, origin.as_deref(), cancel).await?;
            // Recheck the switch after the user answers the card.
            if !Settings::load().enabled {
                return Err("Computer use is disabled".into());
            }
        }
        run_approved(&request.session_id, &request.tool, &request.args, cancel).await
    }
    .await;
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

async fn start(settings: &Settings, session_id: String) -> Result<Browser, String> {
    let executable =
        executable(settings).ok_or("Chrome/Chromium not found; set computer_use.executable")?;
    let root = crate::config::get_config_dir()
        .ok_or("No config directory")?
        .join("computer-use");
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| sweep_stale_profiles(&root, owner_alive));
    let profile = root.join(format!("profile-{:032x}", rand::random::<u128>()));
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    std::fs::write(profile.join(OWNER_FILE), std::process::id().to_string())
        .map_err(|e| e.to_string())?;
    let stop = tokio_util::sync::CancellationToken::new();
    let stop_on_failure = stop.clone().drop_guard();
    let allow_session = session_id.clone();
    let allow: proxy::Allow = Arc::new(move |host: &str| {
        if panel::is_loopback_host(host) || grants::has_host(&allow_session, host) {
            return true;
        }
        let settings = current_settings();
        settings.enabled && settings.allows_host(host)
    });
    let proxy_port = proxy::bind(allow, stop.clone())
        .await
        .map_err(|e| format!("Could not start the browser's proxy: {e}"))?;
    let child = Command::new(executable)
        .args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--disable-extensions",
            "--disable-background-networking",
            "--block-new-web-contents",
            "--proxy-bypass-list=<-loopback>",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
        ])
        .arg(format!("--proxy-server=http://127.0.0.1:{proxy_port}"))
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
    let (sender, mut jobs) = mpsc::channel::<Job>(16);
    let (frames, _) = broadcast::channel::<panel::ScreencastFrame>(16);
    let latest_frame: Arc<Mutex<Option<panel::ScreencastFrame>>> = Arc::new(Mutex::new(None));
    let frames_out = frames.clone();
    let latest_out = Arc::clone(&latest_frame);
    let stopped = stop.clone();
    tokio::spawn(async move {
        let mut chrome = chrome;
        let mut cdp = Cdp {
            ws,
            next: 0,
            refs: Default::default(),
            loaded: false,
            session_id,
            viewport: panel::ViewportPreset::Desktop.spec(None, None),
            inspect_mode: false,
            screencast_active: false,
            frames: frames_out,
            latest_frame: latest_out,
            last_frame_sent: None,
        };
        if cdp
            .command("Fetch.enable", json!({"patterns":[{"urlPattern":"*"}]}))
            .await
            .is_err()
        {
            return;
        }
        let _ = cdp
            .command(
                "Network.enable",
                json!({"maxTotalBufferSize":0,"maxResourceBufferSize":0}),
            )
            .await;
        let _ = cdp
            .command(
                "Network.setBlockedURLs",
                json!({"urls":["file:*","ftp:*","data:*"]}),
            )
            .await;
        let _ = cdp
            .command("Page.setDownloadBehavior", json!({"behavior":"deny"}))
            .await;
        let _ = cdp.command("Page.enable", json!({})).await;
        let idle = tokio::time::sleep(IDLE_LIMIT);
        tokio::pin!(idle);
        loop {
            tokio::select! {
                _ = stopped.cancelled() => break,
                _ = &mut idle => break,
                message = cdp.ws.next() => {
                    match message { Some(Ok(Message::Text(body))) => { if let Ok(event) = serde_json::from_str::<Value>(&body) { if cdp.intercept(&event).await.is_err() { break } } }, Some(Ok(_)) => {}, _ => break }
                },
                job = jobs.recv() => {
                    let Some((tool, args, answer)) = job else { break };
                    let keep_alive = tool.starts_with("panel_");
                    let result = tokio::select! {
                        _ = stopped.cancelled() => break,
                        result = tokio::time::timeout(Duration::from_secs(40), cdp.action(&tool, &args)) => result.unwrap_or_else(|_| Err("CDP timed out".into())),
                    };
                    idle.as_mut().reset(tokio::time::Instant::now() + IDLE_LIMIT);
                    let failed = result.is_err();
                    let _ = answer.send(result);
                    if failed && !keep_alive { break }
                }
            }
        }
        let _ = chrome.child.kill().await;
        let _ = chrome.child.wait().await;
        let _ = std::fs::remove_dir_all(&chrome.profile);
    });
    let _ = stop_on_failure.disarm();
    Ok(Browser {
        sender,
        stop,
        frames,
        latest_frame,
    })
}
struct Cdp {
    ws: WebSocketStream<TcpStream>,
    next: u64,
    pub(super) refs: snapshot::Refs,
    pub(super) loaded: bool,
    session_id: String,
    pub(super) viewport: panel::ViewportSpec,
    pub(super) inspect_mode: bool,
    pub(super) screencast_active: bool,
    frames: broadcast::Sender<panel::ScreencastFrame>,
    latest_frame: Arc<Mutex<Option<panel::ScreencastFrame>>>,
    last_frame_sent: Option<tokio::time::Instant>,
}
const LOAD_LIMIT: Duration = Duration::from_secs(15);
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
        if event["method"] == "Page.loadEventFired" {
            self.loaded = true;
        }
        if event["method"] == "Page.screencastFrame" {
            let params = &event["params"];
            if let Some(frame_id) = params.get("sessionId").and_then(Value::as_i64) {
                let _ = self
                    .send("Page.screencastFrameAck", json!({ "sessionId": frame_id }))
                    .await;
            }
            let due = self
                .last_frame_sent
                .is_none_or(|prev| prev.elapsed() >= panel::FRAME_INTERVAL);
            if due && let Some(data) = params.get("data").and_then(Value::as_str) {
                let meta = &params["metadata"];
                let frame = panel::ScreencastFrame {
                    data: data.to_owned(),
                    width: meta
                        .get("deviceWidth")
                        .and_then(Value::as_f64)
                        .and_then(|v| u32::try_from(v.round() as i64).ok())
                        .unwrap_or(self.viewport.width),
                    height: meta
                        .get("deviceHeight")
                        .and_then(Value::as_f64)
                        .and_then(|v| u32::try_from(v.round() as i64).ok())
                        .unwrap_or(self.viewport.height),
                    device_scale_factor: meta
                        .get("pageScaleFactor")
                        .and_then(Value::as_f64)
                        .unwrap_or(self.viewport.device_scale_factor),
                    timestamp_ms: panel::now_ms(),
                };
                *self.latest_frame.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame.clone());
                let _ = self.frames.send(frame);
                self.last_frame_sent = Some(tokio::time::Instant::now());
            }
        }
        if event["method"] == "Fetch.requestPaused" {
            let url = event["params"]["request"]["url"].as_str().unwrap_or("");
            let settings = current_settings();
            let allowed = panel::is_panel_url_allowed(url, &settings, &self.session_id)
                || (settings.enabled && settings.allows(url));
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
    pub(super) async fn command(&mut self, method: &str, params: Value) -> Result<Value, String> {
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
    pub(super) async fn wait_for_load(&mut self) -> Result<bool, String> {
        let deadline = tokio::time::Instant::now() + LOAD_LIMIT;
        while !self.loaded {
            let message = match tokio::time::timeout_at(deadline, self.ws.next()).await {
                Err(_) => return Ok(false),
                Ok(None) => return Err("Chrome disconnected".into()),
                Ok(Some(message)) => message.map_err(|e| e.to_string())?,
            };
            if let Message::Text(body) = message
                && let Ok(event) = serde_json::from_str::<Value>(&body)
            {
                self.intercept(&event).await?;
            }
        }
        Ok(true)
    }
    async fn main_frame(&mut self) -> Result<Value, String> {
        let tree = self.command("Page.getFrameTree", json!({})).await?;
        Ok(tree["frameTree"]["frame"].clone())
    }
    /// Evaluate in an isolated world: the page's own scripts can neither see
    /// nor change what this runs or reads.
    async fn evaluate(&mut self, expression: &str) -> Result<Value, String> {
        let frame = self.main_frame().await?;
        let world = self
            .command(
                "Page.createIsolatedWorld",
                json!({"frameId":frame["id"],"worldName":"rustcode"}),
            )
            .await?;
        let response = self
            .command(
                "Runtime.evaluate",
                json!({"expression":expression,"contextId":world["executionContextId"],"returnByValue":true}),
            )
            .await?;
        if response.get("exceptionDetails").is_some() {
            return Err(response.to_string());
        }
        Ok(response["result"]["value"].clone())
    }
    async fn target_node(&mut self, target: &snapshot::Target) -> Result<i64, String> {
        match target {
            snapshot::Target::Ref(reference) => snapshot::resolve(&self.refs, reference),
            snapshot::Target::Selector(selector) => {
                let document = self.command("DOM.getDocument", json!({"depth":0})).await?;
                let found = self
                    .command(
                        "DOM.querySelector",
                        json!({"nodeId":document["root"]["nodeId"],"selector":selector}),
                    )
                    .await?;
                let node = found["nodeId"].as_i64().filter(|id| *id != 0);
                let node = node.ok_or_else(|| format!("Selector not found: {selector}"))?;
                let described = self
                    .command("DOM.describeNode", json!({"nodeId":node}))
                    .await?;
                described["node"]["backendNodeId"]
                    .as_i64()
                    .ok_or_else(|| "Chrome did not describe the element".to_owned())
            }
        }
    }
    async fn on_node(
        &mut self,
        target: &snapshot::Target,
        node: i64,
        method: &str,
        mut params: Value,
    ) -> Result<Value, String> {
        params["backendNodeId"] = json!(node);
        self.command(method, params)
            .await
            .map_err(|error| match target {
                snapshot::Target::Ref(reference) => snapshot::stale(reference),
                snapshot::Target::Selector(_) => error,
            })
    }
    async fn click(&mut self, target: &snapshot::Target) -> Result<(), String> {
        let node = self.target_node(target).await?;
        self.on_node(target, node, "DOM.scrollIntoViewIfNeeded", json!({}))
            .await?;
        let model = self
            .on_node(target, node, "DOM.getBoxModel", json!({}))
            .await?;
        let (x, y) = snapshot::center(&model["model"]["content"])
            .ok_or("The element has no box to click; it may be hidden")?;
        for (kind, clicks) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 1)] {
            let mut event = json!({"type":kind,"x":x,"y":y});
            if clicks > 0 {
                event["button"] = json!("left");
                event["clickCount"] = json!(clicks);
            }
            self.command("Input.dispatchMouseEvent", event).await?;
        }
        Ok(())
    }
    async fn read(&mut self) -> Result<Value, String> {
        let page = self
            .evaluate(&format!(
                "(()=>{{const t=document.body?document.body.innerText:'';return {{url:location.href,title:document.title,text:t.slice(0,{max}),truncated:t.length>{max}}}}})()",
                max = snapshot::MAX_TEXT
            ))
            .await?;
        let tree = self
            .command("Accessibility.getFullAXTree", json!({}))
            .await?;
        let (elements, refs, omitted) = snapshot::elements(&tree, snapshot::MAX_ELEMENTS);
        self.refs = refs;
        Ok(snapshot::read_result(&page, elements, omitted))
    }
    async fn action(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        if tool == LOCATION {
            return Ok(self.main_frame().await?["url"].clone());
        }
        if tool.starts_with("panel_") {
            let latest = self
                .latest_frame
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let session_id = self.session_id.clone();
            return panel::handle_cdp_action(self, tool, args, &session_id, latest).await;
        }
        if tool == "browser_open" {
            let url = args["url"].as_str().ok_or("Missing URL")?;
            if !current_settings().allows(url) {
                return Err("URL refused".into());
            }
            self.loaded = false;
            self.refs.clear();
            let response = self.command("Page.navigate", json!({"url":url})).await?;
            if response.get("errorText").is_some() {
                return Err(response.to_string());
            }
            let loaded = response.get("loaderId").is_none() || self.wait_for_load().await?;
            let mut result = json!({"url":url,"loaded":loaded});
            if !loaded {
                result["note"] = json!(
                    "The page had not finished loading after 15 s; browser_read shows what is there so far."
                );
            }
            return Ok(result);
        }
        let location = self.main_frame().await?;
        if !current_settings().allows(location["url"].as_str().unwrap_or("")) {
            return Err("Current page is outside the allowlist; open an allowed URL".into());
        }
        match tool {
            "browser_read" => self.read().await,
            "browser_screenshot" => {
                self.command("Page.captureScreenshot", json!({"format":"png"}))
                    .await
            }
            "browser_click" => {
                let target = snapshot::Target::from_args(args)?;
                self.click(&target).await?;
                Ok(json!({"ok":true}))
            }
            "browser_type" => {
                let target = snapshot::Target::from_args(args)?;
                let node = self.target_node(&target).await?;
                self.on_node(&target, node, "DOM.scrollIntoViewIfNeeded", json!({}))
                    .await?;
                self.on_node(&target, node, "DOM.focus", json!({})).await?;
                self.command("Input.insertText", json!({"text":args["text"]}))
                    .await?;
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
    check_arguments(tool, args)?;
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
    if browser.as_ref().is_some_and(Browser::stopped) {
        *browser = None;
    }
    if browser.is_none() {
        *browser = Some(tokio::select! {
            _ = cancel.cancelled() => return Err("Cancelled".into()),
            started = tokio::time::timeout(Duration::from_secs(20), start(&settings, session_id.to_owned())) => started.map_err(|_| "Browser startup timed out")??,
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

/// Runs a user-driven preview panel action on `session_id`'s shared headless Chrome.
pub(crate) async fn run_panel_action(
    session_id: &str,
    action: &str,
    args: Value,
) -> Result<Value, String> {
    let settings = Settings::load();
    let slot = browsers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(session_id.to_owned())
        .or_default()
        .clone();
    let mut browser = slot.lock().await;
    if browser.as_ref().is_some_and(Browser::stopped) {
        *browser = None;
    }
    if browser.is_none() {
        if action == "panel_state"
            || (action == "panel_stream" && args["active"].as_bool() == Some(false))
        {
            let spec = panel::ViewportPreset::Desktop.spec(None, None);
            return serde_json::to_value(panel::PanelState {
                url: String::new(),
                title: String::new(),
                can_go_back: false,
                can_go_forward: false,
                viewport: spec.preset,
                width: spec.width,
                height: spec.height,
                inspect_mode: false,
                screencast_active: false,
                frame: None,
            })
            .map_err(|e| e.to_string());
        }
        let started = tokio::time::timeout(
            Duration::from_secs(20),
            start(&settings, session_id.to_owned()),
        )
        .await
        .map_err(|_| "Browser startup timed out".to_owned())??;
        *browser = Some(started);
    }
    let (tx, rx) = oneshot::channel();
    browser
        .as_ref()
        .unwrap()
        .sender
        .send((action.to_owned(), args, tx))
        .await
        .map_err(|_| "Browser stopped".to_owned())?;
    drop(browser);
    tokio::time::timeout(Duration::from_secs(45), rx)
        .await
        .map_err(|_| "Browser action timed out".to_owned())
        .and_then(|r| r.map_err(|_| "Browser stopped".to_owned()))
        .and_then(|r| r)
}

/// Subscribes to live screencast frames from `session_id`'s running browser, if any.
pub(crate) async fn subscribe_frames(
    session_id: &str,
) -> Option<(
    broadcast::Receiver<panel::ScreencastFrame>,
    Option<panel::ScreencastFrame>,
)> {
    let slot = browsers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)?
        .clone();
    let browser = slot.lock().await;
    let running = browser.as_ref().filter(|b| !b.stopped())?;
    let latest = running
        .latest_frame
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Some((running.frames.subscribe(), latest))
}

fn owner_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        i32::try_from(pid).is_ok_and(crate::orchestration::tool_socket::process_alive)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Remove browser profiles left by an engine that is no longer running, such
/// as after a crash or a forced quit, when `Chrome`'s cleanup never ran.
fn sweep_stale_profiles(root: &std::path::Path, alive: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_name().to_string_lossy().starts_with("profile-") || !path.is_dir() {
            continue;
        }
        let owner = std::fs::read_to_string(path.join(OWNER_FILE))
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok());
        let stale = match owner {
            Some(pid) => pid != std::process::id() && !alive(pid),
            None => entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > UNOWNED_GRACE),
        };
        if stale {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::{Browser, OWNER_FILE, sweep_stale_profiles};

    #[test]
    fn sweep_removes_only_profiles_of_engines_that_stopped() {
        let root = tempfile::tempdir().unwrap();
        let profile = |name: &str, owner: Option<String>| {
            let path = root.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            if let Some(owner) = owner {
                std::fs::write(path.join(OWNER_FILE), owner).unwrap();
            }
            path
        };
        let dead = profile("profile-dead", Some("7".into()));
        let live = profile("profile-live", Some("42".into()));
        let own = profile("profile-own", Some(std::process::id().to_string()));
        let starting = profile("profile-starting", None);
        let other = profile("notes", Some("7".into()));
        sweep_stale_profiles(root.path(), |pid| pid == 42);
        assert!(!dead.exists());
        assert!(live.exists());
        assert!(own.exists());
        assert!(
            starting.exists(),
            "a fresh profile without an owner may be starting"
        );
        assert!(other.exists(), "only profile directories are touched");
    }

    #[cfg(unix)]
    #[test]
    fn sweep_removes_old_profiles_without_an_owner() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("profile-old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(7200))
            .unwrap();
        sweep_stale_profiles(root.path(), |_| true);
        assert!(!old.exists());
    }

    #[test]
    fn a_stopped_browser_is_replaced_on_the_next_call() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let (frames, _) = tokio::sync::broadcast::channel(4);
        let browser = Browser {
            sender,
            stop: tokio_util::sync::CancellationToken::new(),
            frames,
            latest_frame: std::sync::Arc::new(std::sync::Mutex::new(None)),
        };
        assert!(!browser.stopped());
        drop(receiver);
        assert!(browser.stopped());
    }
}

/// Turn a native `browser_screenshot` result into a short text result and
/// the saved image, which the turn attaches after the tool batch.
pub(crate) fn attach_screenshot(
    content: &str,
    save_image: impl FnOnce(&[u8], &str) -> std::io::Result<PathBuf>,
) -> (rustcode_tools::ToolExecutionOutput, Option<PathBuf>) {
    use rustcode_tools::ToolExecutionOutput;
    let decoded = serde_json::from_str::<Value>(content)
        .map_err(|error| format!("the browser answered with malformed JSON: {error}"))
        .and_then(|value| crate::preview::decode_screenshot(&value));
    let (bytes, extension) = match decoded {
        Ok(decoded) => decoded,
        Err(error) => {
            return (
                ToolExecutionOutput::failure(format!("error: browser_screenshot failed: {error}")),
                None,
            );
        }
    };
    match save_image(&bytes, extension) {
        Ok(path) => (
            ToolExecutionOutput::success(format!(
                "Captured the browser viewport ({} KB {}); the image is attached after this tool result.",
                bytes.len().div_ceil(1024),
                extension.to_ascii_uppercase()
            )),
            Some(path),
        ),
        Err(error) => (
            ToolExecutionOutput::failure(format!(
                "error: could not save the browser screenshot: {error}"
            )),
            None,
        ),
    }
}

#[cfg(test)]
mod screenshot_tests {
    use super::attach_screenshot;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::path::PathBuf;

    #[test]
    fn screenshot_becomes_an_attachment_not_base64_text() {
        let png = b"\x89PNG\r\n\x1a\nrest-of-image".to_vec();
        let content = serde_json::json!({ "data": STANDARD.encode(&png) }).to_string();
        let mut saved = None;
        let (output, path) = attach_screenshot(&content, |bytes, extension| {
            saved = Some((bytes.to_vec(), extension.to_owned()));
            Ok(PathBuf::from("/tmp/browser.png"))
        });
        assert!(output.success, "{}", output.content);
        assert!(output.content.len() < 1024);
        assert!(!output.content.contains(&STANDARD.encode(&png)));
        assert_eq!(path, Some(PathBuf::from("/tmp/browser.png")));
        assert_eq!(saved, Some((png, "png".to_owned())));
    }

    #[test]
    fn unusable_screenshots_are_errors() {
        for content in [
            "not json".to_owned(),
            serde_json::json!({}).to_string(),
            serde_json::json!({ "data": "!!!" }).to_string(),
            serde_json::json!({ "data": STANDARD.encode(b"GIF89a") }).to_string(),
        ] {
            let (output, path) =
                attach_screenshot(&content, |_, _| Ok(PathBuf::from("/tmp/unused.png")));
            assert!(!output.success, "{content}");
            assert!(path.is_none());
        }
        let content =
            serde_json::json!({ "data": STANDARD.encode(b"\x89PNG\r\n\x1a\n") }).to_string();
        let (output, path) =
            attach_screenshot(&content, |_, _| Err(std::io::Error::other("disk full")));
        assert!(!output.success);
        assert!(output.content.contains("disk full"));
        assert!(path.is_none());
    }
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
