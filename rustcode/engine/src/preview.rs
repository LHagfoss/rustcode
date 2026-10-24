//! Agent access to the macOS app's localhost preview (#2138).
//!
//! The preview is a web view inside the RustCode macOS app. The engine never
//! loads pages itself: a `localhost_preview` call is forwarded to the ACP
//! client as the `_rustcode/preview` extension request, and the client runs
//! it in the preview the user is looking at. The client owns the security
//! decisions that need the user (the approval prompt, the approved origin,
//! revocation); the engine owns the parts the model sees:
//!
//! - The tool is offered only to sessions whose client advertised
//!   `clientCapabilities._meta.rustcode.preview` at initialize. The terminal
//!   UI, the desktop shell and every other ACP client never see it.
//! - Arguments are validated before anything is sent, so a malformed call
//!   never reaches the app.
//! - Every call is bounded by [`PREVIEW_REQUEST_TIMEOUT`] and by the turn's
//!   cancellation, and only an explicit `{"ok": true}` answer is reported as
//!   success. A denial, a timeout, a transport failure or a malformed answer
//!   is an error result that says nothing ran or that the outcome is unknown.
//!
//! Tool results are text, so a screenshot is saved as an image attachment and
//! attached to the conversation right after the tool batch, through the same
//! `![image](file://…)` marker a pasted image uses. A model without image
//! input gets it described by the configured vision model.

use serde_json::{Value, json};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::tools::{ToolErrorKind, ToolExecutionOutput};

pub const PREVIEW_TOOL_NAME: &str = "localhost_preview";
/// The `clientCapabilities._meta.rustcode.preview` version this engine
/// speaks. Bump it on any change to the request or response shape.
pub const PREVIEW_CAPABILITY_VERSION: u64 = 1;
/// The ACP extension method a preview action is sent as.
pub const PREVIEW_METHOD: &str = "_rustcode/preview";

pub(crate) const UNAVAILABLE_MESSAGE: &str = "error: localhost_preview is only available in the RustCode macOS app, which runs it in its preview panel. This client has no preview; nothing was inspected or changed.";

/// Upper bound on one forwarded call, the user's approval of a first call
/// included. The app bounds its own waits (approval and each action) well
/// below this, so it only fires when the app stops answering.
pub(crate) const PREVIEW_REQUEST_TIMEOUT: Duration = Duration::from_secs(150);

const ACTIONS: &[&str] = &[
    "inspect",
    "screenshot",
    "navigate",
    "click",
    "fill",
    "select",
    "scroll",
];
const DEFAULT_TEXT_CHARS: u64 = 6_000;
const MAX_TEXT_CHARS: u64 = 20_000;
const MAX_ARGUMENT_CHARS: usize = 2_048;
const MAX_ELEMENTS: usize = 80;
const MAX_ELEMENT_FIELD_CHARS: usize = 160;
const MAX_SCROLL: i64 = 100_000;
/// Decoded screenshot bytes accepted from the app.
const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;

/// The forwarded request, as a boxed future so the trait stays object safe.
pub type PreviewFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

/// Sends one preview action to the client that owns the preview. The ACP
/// server installs one per session when the client advertised the capability.
pub trait PreviewClient: Send + Sync {
    fn request(&self, params: Value) -> PreviewFuture;
}

/// Whether initialize `clientCapabilities._meta` advertises the preview
/// bridge. Absent, malformed or older values mean no.
pub fn client_meta_supports_preview(meta: Option<&serde_json::Map<String, Value>>) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("preview"))
        .and_then(Value::as_u64)
        .is_some_and(|version| version >= PREVIEW_CAPABILITY_VERSION)
}

pub(crate) fn operation_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ACTIONS,
                "description": "inspect: URL, title, visible text and interactive elements with CSS selectors. screenshot: an image of the visible page. navigate: load a local URL or a path on the approved origin. click/fill/select: act on the element a selector names. scroll: by dy pixels, or to the element a selector names."
            },
            "url": { "type": "string", "description": "navigate: a path such as /settings, or a full http://localhost URL on the approved origin" },
            "selector": { "type": "string", "description": "CSS selector, usually one inspect returned" },
            "value": { "type": "string", "description": "fill: the text to enter; select: the option's value or label" },
            "dy": { "type": "integer", "description": "scroll: pixels down (negative scrolls up)" },
            "max_chars": { "type": "integer", "minimum": 200, "maximum": MAX_TEXT_CHARS, "description": "inspect: visible text budget (default 6000)" }
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

/// System prompt guidance, added only for sessions that can use the tool.
pub(crate) const PREVIEW_PROMPT_HINT: &str = "\n# Localhost preview\n\
The user's RustCode app has a live preview panel for local web apps, and `localhost_preview` drives it. It is a built-in tool, not an MCP tool: never look for it with `list_mcp_tools`. \
To show the user a page (\"open it in my preview\", \"live preview\"), start the dev server, then call `navigate` with its full URL (e.g. http://localhost:5173): that opens the panel and loads the page even when nothing is shown yet. \
Actions: `inspect` (URL, title, visible text, interactive elements with selectors), `screenshot` (the image is attached after the tool result), `navigate` (a path or local URL on the approved origin), `click`/`fill`/`select` (a CSS selector from inspect) and `scroll`. \
The user approves access before the first action and can deny or revoke it at any time; the user watches every action. Only the approved localhost origin is reachable, never external sites. \
A denied, failed or timed-out action did nothing you may rely on: never claim to have seen or changed the page unless the result says so. Use it to check UI work once the dev server runs.\n\
Call shape: {\"action\": \"inspect\"} or {\"action\": \"click\", \"selector\": \"#save\"}.\n";

/// One validated preview action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreviewOperation {
    Inspect {
        max_chars: u64,
    },
    Screenshot,
    Navigate {
        url: String,
    },
    Click {
        selector: String,
    },
    Fill {
        selector: String,
        value: String,
    },
    Select {
        selector: String,
        value: String,
    },
    Scroll {
        selector: Option<String>,
        dy: Option<i64>,
    },
}

fn bounded_string(args: &Value, key: &str, allow_empty: bool) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            if text.chars().count() > MAX_ARGUMENT_CHARS {
                return Err(format!(
                    "`{key}` is longer than {MAX_ARGUMENT_CHARS} characters"
                ));
            }
            if !allow_empty && text.trim().is_empty() {
                return Err(format!("`{key}` is empty"));
            }
            Ok(Some(text.clone()))
        }
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn required(value: Option<String>, key: &str, action: &str) -> Result<String, String> {
    value.ok_or_else(|| format!("`{action}` needs `{key}`"))
}

/// Whether `url` is something navigate may forward: a path, query or
/// fragment on the current origin, or an absolute http(s) URL on a loopback
/// host. The app also checks the approved origin; this only keeps an
/// obviously external target from ever leaving the engine.
pub(crate) fn navigation_target_is_local(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.starts_with("//") {
        return false;
    }
    if trimmed.starts_with('/') || trimmed.starts_with('?') || trimmed.starts_with('#') {
        return true;
    }
    let Ok(parsed) = url::Url::parse(trimmed) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

impl PreviewOperation {
    pub(crate) fn parse(args: &Value) -> Result<Self, String> {
        let Some(object) = args.as_object() else {
            return Err("arguments must be a JSON object".to_owned());
        };
        // `summary` is the display-only note every tool schema advertises
        // (tools::schema::CALL_SUMMARY_KEY); rejecting it failed every call.
        if let Some(unknown) = object.keys().find(|key| {
            !matches!(
                key.as_str(),
                "action" | "url" | "selector" | "value" | "dy" | "max_chars"
            ) && key.as_str() != crate::tools::schema::CALL_SUMMARY_KEY
        }) {
            return Err(format!("unknown argument `{unknown}`"));
        }
        let action = object
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("`action` must be one of {}", ACTIONS.join(", ")))?;
        let selector = bounded_string(args, "selector", false)?;
        let url = bounded_string(args, "url", false)?;
        let value = bounded_string(args, "value", true)?;
        let dy = match object.get("dy") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_i64()
                    .filter(|dy| dy.abs() <= MAX_SCROLL)
                    .ok_or_else(|| format!("`dy` must be an integer within ±{MAX_SCROLL}"))?,
            ),
        };
        let max_chars = match object.get("max_chars") {
            None | Some(Value::Null) => DEFAULT_TEXT_CHARS,
            Some(value) => value
                .as_u64()
                .map(|chars| chars.clamp(200, MAX_TEXT_CHARS))
                .ok_or_else(|| "`max_chars` must be a positive integer".to_owned())?,
        };
        Ok(match action {
            "inspect" => Self::Inspect { max_chars },
            "screenshot" => Self::Screenshot,
            "navigate" => {
                let url = required(url, "url", action)?;
                if !navigation_target_is_local(&url) {
                    return Err(format!(
                        "`{url}` is not a local address; the preview only opens paths and http(s) URLs on localhost, 127.0.0.1 or [::1]"
                    ));
                }
                Self::Navigate { url }
            }
            "click" => Self::Click {
                selector: required(selector, "selector", action)?,
            },
            "fill" => Self::Fill {
                selector: required(selector, "selector", action)?,
                value: required(value, "value", action)?,
            },
            "select" => Self::Select {
                selector: required(selector, "selector", action)?,
                value: required(value, "value", action)?,
            },
            "scroll" => {
                if selector.is_none() && dy.is_none() {
                    return Err("`scroll` needs `dy` or `selector`".to_owned());
                }
                Self::Scroll { selector, dy }
            }
            other => {
                return Err(format!(
                    "unknown action `{other}`; use one of {}",
                    ACTIONS.join(", ")
                ));
            }
        })
    }

    pub(crate) fn action(&self) -> &'static str {
        match self {
            Self::Inspect { .. } => "inspect",
            Self::Screenshot => "screenshot",
            Self::Navigate { .. } => "navigate",
            Self::Click { .. } => "click",
            Self::Fill { .. } => "fill",
            Self::Select { .. } => "select",
            Self::Scroll { .. } => "scroll",
        }
    }

    /// The `operation` object sent to the client.
    pub(crate) fn to_params(&self) -> Value {
        let mut params = json!({ "action": self.action() });
        let object = params
            .as_object_mut()
            .expect("operation params are an object");
        match self {
            Self::Inspect { max_chars } => {
                object.insert("maxChars".into(), json!(max_chars));
            }
            Self::Screenshot => {}
            Self::Navigate { url } => {
                object.insert("url".into(), json!(url));
            }
            Self::Click { selector } => {
                object.insert("selector".into(), json!(selector));
            }
            Self::Fill { selector, value } | Self::Select { selector, value } => {
                object.insert("selector".into(), json!(selector));
                object.insert("value".into(), json!(value));
            }
            Self::Scroll { selector, dy } => {
                if let Some(selector) = selector {
                    object.insert("selector".into(), json!(selector));
                }
                if let Some(dy) = dy {
                    object.insert("dy".into(), json!(dy));
                }
            }
        }
        params
    }
}

fn clip(text: &str, limit: usize) -> (String, bool) {
    match text.char_indices().nth(limit) {
        Some((index, _)) => (format!("{}…", &text[..index]), true),
        None => (text.to_owned(), false),
    }
}

fn one_line(text: &str) -> String {
    clip(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        MAX_ELEMENT_FIELD_CHARS,
    )
    .0
}

/// The failure the model sees for an error the client reported. The client
/// sends a stable `code`; its `message` is shown as the reason.
fn client_failure(error: &Value) -> ToolExecutionOutput {
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(one_line)
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| "no reason given".to_owned());
    let (prefix, kind, retryable) = match code {
        "denied" => (
            "the user denied access to the preview. Nothing was inspected or changed; do not retry unless the user asks",
            ToolErrorKind::PermissionDenied,
            false,
        ),
        "revoked" => (
            "the user revoked access to the preview. Nothing was inspected or changed; do not retry unless the user asks",
            ToolErrorKind::PermissionDenied,
            false,
        ),
        "approval_timeout" => (
            "nobody answered the request for preview access; the user is likely away. Nothing was inspected or changed. Do not call localhost_preview again this turn: finish without it, and ask the user in your reply to check the page or allow preview access",
            ToolErrorKind::PermissionDenied,
            false,
        ),
        "out_of_scope" => (
            "the action would leave the approved local preview origin and was blocked",
            ToolErrorKind::PermissionDenied,
            false,
        ),
        "no_page" => (
            "no page is open in the preview",
            ToolErrorKind::UnavailableDependency,
            false,
        ),
        "not_found" => (
            "no element matches that selector",
            ToolErrorKind::Validation,
            false,
        ),
        "invalid" => (
            "the app rejected the action",
            ToolErrorKind::InvalidArguments,
            false,
        ),
        "timeout" => (
            "the preview action timed out; its outcome is unknown",
            ToolErrorKind::Unknown,
            true,
        ),
        "busy" => (
            "the preview is running another action",
            ToolErrorKind::UnavailableDependency,
            true,
        ),
        _ => ("the preview action failed", ToolErrorKind::Internal, false),
    };
    ToolExecutionOutput::failure_with_kind(format!("error: {prefix}: {message}"), kind, retryable)
}

fn page_line(response: &Value) -> String {
    let url = response.get("url").and_then(Value::as_str).unwrap_or("");
    let title = response
        .get("title")
        .and_then(Value::as_str)
        .map(one_line)
        .unwrap_or_default();
    match (title.is_empty(), url.is_empty()) {
        (true, true) => "The preview shows no page.".to_owned(),
        (true, false) => format!("Page: {}", one_line(url)),
        (false, true) => format!("Page: {title}"),
        (false, false) => format!("Page: {title} — {}", one_line(url)),
    }
}

fn format_elements(elements: &[Value], out: &mut String) {
    if elements.is_empty() {
        out.push_str("\n\nInteractive elements: none found.");
        return;
    }
    out.push_str("\n\nInteractive elements (selector, element, label):");
    for element in elements.iter().take(MAX_ELEMENTS) {
        let field = |key: &str| {
            element
                .get(key)
                .and_then(Value::as_str)
                .map(one_line)
                .filter(|value| !value.is_empty())
        };
        let Some(selector) = field("selector") else {
            continue;
        };
        let mut kind = field("tag").unwrap_or_else(|| "element".to_owned());
        if let Some(input_type) = field("type") {
            kind.push_str(&format!("[type={input_type}]"));
        }
        if let Some(role) = field("role") {
            kind.push_str(&format!(" role={role}"));
        }
        let mut line = format!("\n- `{selector}` {kind}");
        if let Some(text) = field("text").or_else(|| field("name")) {
            line.push_str(&format!(" \"{text}\""));
        }
        if let Some(value) = field("value") {
            line.push_str(&format!(" value=\"{value}\""));
        }
        if let Some(href) = field("href") {
            line.push_str(&format!(" → {href}"));
        }
        if element.get("disabled").and_then(Value::as_bool) == Some(true) {
            line.push_str(" (disabled)");
        }
        out.push_str(&line);
    }
    if elements.len() > MAX_ELEMENTS {
        out.push_str(&format!(
            "\n… {} more not listed",
            elements.len() - MAX_ELEMENTS
        ));
    }
}

fn decode_screenshot(image: &Value) -> Result<(Vec<u8>, &'static str), String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let data = image
        .get("data")
        .and_then(Value::as_str)
        .ok_or("the screenshot has no image data")?;
    if data.len() > MAX_SCREENSHOT_BYTES.div_ceil(3) * 4 {
        return Err("the screenshot is larger than 8 MiB".to_owned());
    }
    let bytes = STANDARD
        .decode(data.trim())
        .map_err(|error| format!("the screenshot is not valid base64: {error}"))?;
    let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpg"
    } else {
        return Err("the screenshot is neither PNG nor JPEG".to_owned());
    };
    Ok((bytes, extension))
}

/// Turn the client's answer into the tool result. Only `"ok": true` is a
/// success; anything else, malformed answers included, is a failure.
/// `save_image` stores a screenshot and returns its path.
pub(crate) fn interpret_response(
    operation: &PreviewOperation,
    response: &Value,
    save_image: impl FnOnce(&[u8], &str) -> std::io::Result<PathBuf>,
) -> (ToolExecutionOutput, Option<PathBuf>) {
    match response.get("ok").and_then(Value::as_bool) {
        Some(true) => {}
        Some(false) => {
            return (
                client_failure(response.get("error").unwrap_or(&Value::Null)),
                None,
            );
        }
        None => {
            return (
                ToolExecutionOutput::failure_with_kind(
                    "error: the app sent a malformed preview answer; the action's outcome is unknown".to_owned(),
                    ToolErrorKind::Internal,
                    false,
                ),
                None,
            );
        }
    }
    let mut out = page_line(response);
    let mut saved = None;
    match operation {
        PreviewOperation::Inspect { max_chars } => {
            let text = response.get("text").and_then(Value::as_str).unwrap_or("");
            let (text, clipped) = clip(text.trim(), *max_chars as usize);
            let clipped =
                clipped || response.get("truncated").and_then(Value::as_bool) == Some(true);
            out.push_str("\n\nVisible text");
            if clipped {
                out.push_str(" (truncated)");
            }
            out.push_str(":\n");
            out.push_str(if text.is_empty() { "(none)" } else { &text });
            let elements = response
                .get("elements")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            format_elements(elements, &mut out);
        }
        PreviewOperation::Screenshot => {
            let Some(image) = response.get("image") else {
                return (
                    ToolExecutionOutput::failure_with_kind(
                        "error: the app answered without a screenshot".to_owned(),
                        ToolErrorKind::Internal,
                        false,
                    ),
                    None,
                );
            };
            let (bytes, extension) = match decode_screenshot(image) {
                Ok(decoded) => decoded,
                Err(reason) => {
                    return (
                        ToolExecutionOutput::failure_with_kind(
                            format!("error: {reason}"),
                            ToolErrorKind::Internal,
                            false,
                        ),
                        None,
                    );
                }
            };
            let path = match save_image(&bytes, extension) {
                Ok(path) => path,
                Err(error) => {
                    return (
                        ToolExecutionOutput::failure_with_kind(
                            format!("error: could not save the screenshot: {error}"),
                            ToolErrorKind::Internal,
                            false,
                        ),
                        None,
                    );
                }
            };
            let size = match (
                image.get("width").and_then(Value::as_u64),
                image.get("height").and_then(Value::as_u64),
            ) {
                (Some(width), Some(height)) => format!(" ({width}×{height})"),
                _ => String::new(),
            };
            // A narrow panel is laid out at desktop width for the capture, so
            // a phone-shaped page is not judged as the app's layout.
            let viewport = response.get("viewport");
            let laid_out = viewport
                .and_then(|v| v.get("width"))
                .and_then(Value::as_f64);
            let panel = viewport
                .and_then(|v| v.get("panelWidth"))
                .and_then(Value::as_f64);
            if let (Some(width), Some(panel)) = (laid_out, panel) {
                if viewport
                    .and_then(|v| v.get("desktop"))
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    out.push_str(&format!(
                        "\nLaid out {width:.0} px wide, as on a desktop; the user's panel is {panel:.0} px wide."
                    ));
                } else {
                    out.push_str(&format!(
                        "\nLaid out {width:.0} px wide, the width of the user's panel."
                    ));
                }
            }
            out.push_str(&format!(
                "\nScreenshot{size} saved to {}. It is attached as an image right after this tool result.",
                path.display()
            ));
            saved = Some(path);
        }
        PreviewOperation::Navigate { url } => {
            out = format!("Navigated to {}. {out}", one_line(url));
        }
        PreviewOperation::Click { selector } => {
            out = format!("Clicked `{}`. {out}", one_line(selector));
        }
        PreviewOperation::Fill { selector, .. } => {
            out = format!("Filled `{}`. {out}", one_line(selector));
        }
        PreviewOperation::Select { selector, value } => {
            out = format!(
                "Selected \"{}\" in `{}`. {out}",
                one_line(value),
                one_line(selector)
            );
        }
        PreviewOperation::Scroll { .. } => {
            let position = response
                .get("scrollY")
                .and_then(Value::as_f64)
                .map(|y| format!(" Scroll position: {y:.0}px."))
                .unwrap_or_default();
            out = format!("Scrolled.{position} {out}");
        }
    }
    if let Some(note) = response
        .get("note")
        .and_then(Value::as_str)
        .map(one_line)
        .filter(|note| !note.is_empty())
    {
        out.push_str(&format!("\nNote: {note}"));
    }
    out.push_str(&page_errors(response));
    (ToolExecutionOutput::success(out), saved)
}

/// What the page threw since it loaded. A blank page with no reason sent
/// the agent through builds and tests for ten rounds before it found a
/// runtime error (session 01a16b37); the browser already knew.
fn page_errors(response: &Value) -> String {
    const SHOWN: usize = 8;
    let errors: Vec<String> = response
        .get("errors")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(one_line)
        .filter(|error| !error.is_empty())
        .collect();
    if errors.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\nPage errors (from the browser, newest last):");
    let skipped = errors.len().saturating_sub(SHOWN);
    if skipped > 0 {
        out.push_str(&format!("\n- … {skipped} earlier"));
    }
    for error in &errors[skipped..] {
        let (error, _) = clip(error, 300);
        out.push_str(&format!("\n- {error}"));
    }
    out
}

/// Run one `localhost_preview` call against the session's client.
pub(crate) async fn execute(
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel: &tokio_util::sync::CancellationToken,
    args: &Value,
) -> ToolExecutionOutput {
    let client = state.lock().await.preview_client.clone();
    let (output, image) = execute_with(
        client,
        cancel,
        args,
        PREVIEW_REQUEST_TIMEOUT,
        |bytes, ext| crate::controller::save_image_attachment(bytes, ext),
    )
    .await;
    if let Some(path) = image {
        state.lock().await.pending_preview_images.push(path);
    }
    output
}

pub(crate) async fn execute_with(
    client: Option<Arc<dyn PreviewClient>>,
    cancel: &tokio_util::sync::CancellationToken,
    args: &Value,
    timeout: Duration,
    save_image: impl FnOnce(&[u8], &str) -> std::io::Result<PathBuf>,
) -> (ToolExecutionOutput, Option<PathBuf>) {
    let operation = match PreviewOperation::parse(args) {
        Ok(operation) => operation,
        Err(reason) => {
            return (
                ToolExecutionOutput::failure_with_kind(
                    format!(
                        "error: invalid localhost_preview call: {reason}. No preview action ran."
                    ),
                    ToolErrorKind::InvalidArguments,
                    false,
                ),
                None,
            );
        }
    };
    let Some(client) = client else {
        return (
            ToolExecutionOutput::failure_with_kind(
                UNAVAILABLE_MESSAGE.to_owned(),
                ToolErrorKind::UnavailableDependency,
                false,
            ),
            None,
        );
    };
    let request = client.request(json!({ "operation": operation.to_params() }));
    let answer = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            return (
                ToolExecutionOutput::failure_with_kind(
                    format!("error: the preview {} was cancelled before the app answered; its outcome is unknown", operation.action()),
                    ToolErrorKind::Cancelled,
                    false,
                ),
                None,
            );
        }
        answer = tokio::time::timeout(timeout, request) => answer,
    };
    match answer {
        Err(_) => (
            ToolExecutionOutput::failure_with_kind(
                format!(
                    "error: the app did not answer the preview {} within {}s; its outcome is unknown",
                    operation.action(),
                    timeout.as_secs()
                ),
                ToolErrorKind::Unknown,
                true,
            ),
            None,
        ),
        Ok(Err(reason)) => (
            ToolExecutionOutput::failure_with_kind(
                format!(
                    "error: the preview {} did not run: {}",
                    operation.action(),
                    one_line(&reason)
                ),
                ToolErrorKind::UnavailableDependency,
                false,
            ),
            None,
        ),
        Ok(Ok(response)) => interpret_response(&operation, &response, save_image),
    }
}

/// The user message that shows the model the screenshots a tool batch took.
/// Tool results are text in every provider protocol this engine speaks, so
/// the images follow the batch as an attachment, marked as coming from the
/// tool rather than typed by the user.
pub(crate) fn screenshot_attachment_message(paths: &[PathBuf]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let mut text = format!(
        "{}localhost_preview tool, not typed by the user: the screenshot(s) it just took]",
        rustcode_core::TOOL_ATTACHMENT_PREFIX
    );
    for path in paths {
        text.push_str(&format!("\n![image](file://{})", path.display()));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Scripted {
        answer: Box<dyn Fn() -> PreviewFuture + Send + Sync>,
        calls: AtomicUsize,
        last: std::sync::Mutex<Option<Value>>,
    }

    impl PreviewClient for Scripted {
        fn request(&self, params: Value) -> PreviewFuture {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() = Some(params);
            (self.answer)()
        }
    }

    fn scripted(answer: impl Fn() -> PreviewFuture + Send + Sync + 'static) -> Arc<Scripted> {
        Arc::new(Scripted {
            answer: Box::new(answer),
            calls: AtomicUsize::new(0),
            last: std::sync::Mutex::new(None),
        })
    }

    fn answering(value: Value) -> Arc<Scripted> {
        scripted(move || {
            let value = value.clone();
            Box::pin(async move { Ok(value) })
        })
    }

    /// The prompt's hint and the provider request's `tools` come from one
    /// policy: a session with a preview offers the tool it is told about.
    #[test]
    fn request_policy_offers_preview_with_a_client() {
        let mut state = crate::app::AppState::new();
        assert!(!crate::network::root_schema_policy(&state, false).include_preview_tool);
        state.preview_client = Some(answering(Value::Null) as Arc<dyn PreviewClient>);
        let policy = crate::network::root_schema_policy(&state, false);
        assert!(policy.include_preview_tool);
        let mut prompt = String::new();
        crate::tools::schema::append_preview_hint(&mut prompt, policy);
        assert!(prompt.contains(PREVIEW_TOOL_NAME));
        let tools = crate::tools::native_tools_schema_for_context(policy, &[]).0;
        assert!(
            tools
                .iter()
                .any(|tool| tool.to_string().contains(PREVIEW_TOOL_NAME))
        );
        // A compact prompt describes no preview, so offers none.
        assert!(!crate::network::root_schema_policy(&state, true).include_preview_tool);
    }

    fn no_save(_: &[u8], _: &str) -> std::io::Result<PathBuf> {
        panic!("nothing should be saved")
    }

    async fn run(
        client: Option<Arc<Scripted>>,
        args: Value,
    ) -> (ToolExecutionOutput, Option<PathBuf>) {
        execute_with(
            client.map(|client| client as Arc<dyn PreviewClient>),
            &tokio_util::sync::CancellationToken::new(),
            &args,
            Duration::from_secs(5),
            no_save,
        )
        .await
    }

    #[test]
    fn capability_requires_the_advertised_version() {
        let meta = |value: Value| value.as_object().cloned();
        assert!(client_meta_supports_preview(
            meta(json!({"rustcode": {"preview": 1}})).as_ref()
        ));
        assert!(client_meta_supports_preview(
            meta(json!({"rustcode": {"preview": 2, "visualizations": 1}})).as_ref()
        ));
        assert!(!client_meta_supports_preview(None));
        assert!(!client_meta_supports_preview(
            meta(json!({"rustcode": {"visualizations": 1}})).as_ref()
        ));
        assert!(!client_meta_supports_preview(
            meta(json!({"rustcode": {"preview": "1"}})).as_ref()
        ));
        assert!(!client_meta_supports_preview(
            meta(json!({"rustcode": {"preview": 0}})).as_ref()
        ));
    }

    #[test]
    fn malformed_operations_are_rejected_before_forwarding() {
        for args in [
            json!("inspect"),
            json!({}),
            json!({"action": "hover"}),
            json!({"action": "click"}),
            json!({"action": "click", "selector": "  "}),
            json!({"action": "fill", "selector": "#a"}),
            json!({"action": "select", "value": "x"}),
            json!({"action": "scroll"}),
            json!({"action": "scroll", "dy": 1.5}),
            json!({"action": "scroll", "dy": 10_000_000}),
            json!({"action": "navigate"}),
            json!({"action": "navigate", "url": "https://example.com"}),
            json!({"action": "navigate", "url": "//example.com/x"}),
            json!({"action": "navigate", "url": "file:///etc/passwd"}),
            json!({"action": "navigate", "url": "javascript:alert(1)"}),
            json!({"action": "inspect", "script": "1"}),
            json!({"action": "click", "selector": "a".repeat(MAX_ARGUMENT_CHARS + 1)}),
        ] {
            assert!(
                PreviewOperation::parse(&args).is_err(),
                "{args} was accepted"
            );
        }
    }

    // Session 01a16b37: the model sends the `summary` every tool schema
    // offers, and the preview rejected it on every call.
    #[test]
    fn parse_accepts_the_call_summary_every_schema_offers() {
        PreviewOperation::parse(&json!({"action": "inspect", "summary": "Check the board"}))
            .expect("summary is display-only");
    }

    #[test]
    fn navigation_accepts_paths_and_loopback_urls_only() {
        for url in [
            "/settings",
            "?tab=2",
            "#top",
            "http://localhost:3000/",
            "https://app.localhost/x",
            "http://127.0.0.1:8080",
            "http://[::1]:5173/",
        ] {
            assert!(navigation_target_is_local(url), "{url}");
        }
        for url in [
            "http://example.com",
            "http://localhost.example.com",
            "http://10.0.0.2",
            "ftp://localhost",
            "settings",
            "//localhost:3000",
        ] {
            assert!(!navigation_target_is_local(url), "{url}");
        }
    }

    #[test]
    fn operations_serialize_for_the_client() {
        let operation =
            PreviewOperation::parse(&json!({"action": "fill", "selector": "#email", "value": ""}))
                .unwrap();
        assert_eq!(
            operation.to_params(),
            json!({"action": "fill", "selector": "#email", "value": ""})
        );
        let operation =
            PreviewOperation::parse(&json!({"action": "inspect", "max_chars": 50})).unwrap();
        assert_eq!(
            operation.to_params(),
            json!({"action": "inspect", "maxChars": 200})
        );
    }

    #[tokio::test]
    async fn without_a_client_the_tool_fails_and_nothing_is_sent() {
        let (output, image) = run(None, json!({"action": "inspect"})).await;
        assert!(!output.success);
        assert_eq!(
            output.error_kind,
            Some(ToolErrorKind::UnavailableDependency)
        );
        assert!(
            output
                .content
                .contains("only available in the RustCode macOS app")
        );
        assert!(image.is_none());
    }

    #[tokio::test]
    async fn invalid_arguments_never_reach_the_client() {
        let client = answering(json!({"ok": true}));
        let (output, _) = run(Some(client.clone()), json!({"action": "click"})).await;
        assert!(!output.success);
        assert_eq!(output.error_kind, Some(ToolErrorKind::InvalidArguments));
        assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_operation_is_forwarded_and_success_reported() {
        let client = answering(json!({
            "ok": true,
            "url": "http://localhost:3000/",
            "title": "Todo",
            "text": "Buy milk\nWalk dog",
            "elements": [
                {"selector": "#add", "tag": "button", "text": "Add"},
                {"selector": "input[name=\"todo\"]", "tag": "input", "type": "text", "name": "todo"},
                {"tag": "div"}
            ]
        }));
        let (output, _) = run(Some(client.clone()), json!({"action": "inspect"})).await;
        assert!(output.success, "{}", output.content);
        assert_eq!(
            client.last.lock().unwrap().clone(),
            Some(json!({"operation": {"action": "inspect", "maxChars": 6000}}))
        );
        assert!(
            output
                .content
                .contains("Page: Todo — http://localhost:3000/")
        );
        assert!(output.content.contains("Buy milk"));
        assert!(output.content.contains("- `#add` button \"Add\""));
        assert!(output.content.contains("input[type=text] \"todo\""));
    }

    #[tokio::test]
    async fn denial_and_client_errors_are_failures() {
        for (code, kind) in [
            ("denied", ToolErrorKind::PermissionDenied),
            ("revoked", ToolErrorKind::PermissionDenied),
            ("approval_timeout", ToolErrorKind::PermissionDenied),
            ("out_of_scope", ToolErrorKind::PermissionDenied),
            ("no_page", ToolErrorKind::UnavailableDependency),
            ("not_found", ToolErrorKind::Validation),
            ("timeout", ToolErrorKind::Unknown),
            ("whatever", ToolErrorKind::Internal),
        ] {
            let client =
                answering(json!({"ok": false, "error": {"code": code, "message": "because"}}));
            let (output, _) = run(Some(client), json!({"action": "click", "selector": "#a"})).await;
            assert!(!output.success, "{code}");
            assert_eq!(output.error_kind, Some(kind), "{code}");
            assert!(!output.content.contains("Clicked"), "{code}");
            assert!(output.content.contains("because"), "{code}");
        }
    }

    #[tokio::test]
    async fn malformed_answers_are_never_success() {
        for answer in [json!({}), json!({"ok": "yes"}), json!(null), json!([1])] {
            let (output, _) = run(
                Some(answering(answer.clone())),
                json!({"action": "screenshot"}),
            )
            .await;
            assert!(!output.success, "{answer}");
        }
        let (output, _) = run(
            Some(answering(json!({"ok": true}))),
            json!({"action": "screenshot"}),
        )
        .await;
        assert!(!output.success);
        let (output, _) = run(
            Some(answering(
                json!({"ok": true, "image": {"data": "bm90IGFuIGltYWdl"}}),
            )),
            json!({"action": "screenshot"}),
        )
        .await;
        assert!(!output.success);
        assert!(output.content.contains("neither PNG nor JPEG"));
    }

    #[tokio::test]
    async fn transport_errors_are_failures() {
        let client = scripted(|| Box::pin(async { Err("method not found".to_owned()) }));
        let (output, _) = run(Some(client), json!({"action": "scroll", "dy": 300})).await;
        assert!(!output.success);
        assert!(output.content.contains("did not run: method not found"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_client_times_out() {
        let client = scripted(|| Box::pin(std::future::pending()));
        let (output, _) = execute_with(
            Some(client as Arc<dyn PreviewClient>),
            &tokio_util::sync::CancellationToken::new(),
            &json!({"action": "inspect"}),
            Duration::from_secs(30),
            no_save,
        )
        .await;
        assert!(!output.success);
        assert!(output.retryable);
        assert!(output.content.contains("did not answer"));
        assert!(output.content.contains("outcome is unknown"));
    }

    #[tokio::test]
    async fn cancellation_ends_the_wait() {
        let client = scripted(|| Box::pin(std::future::pending()));
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let (output, _) = execute_with(
            Some(client as Arc<dyn PreviewClient>),
            &cancel,
            &json!({"action": "navigate", "url": "/"}),
            Duration::from_secs(30),
            no_save,
        )
        .await;
        assert!(!output.success);
        assert_eq!(output.error_kind, Some(ToolErrorKind::Cancelled));
    }

    #[tokio::test]
    async fn a_screenshot_is_saved_and_attached() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
        let client = answering(json!({
            "ok": true, "url": "http://localhost:3000/", "title": "App",
            "image": {"mimeType": "image/png", "data": STANDARD.encode(&png), "width": 800, "height": 600}
        }));
        let saved = std::sync::Mutex::new(None);
        let (output, image) = execute_with(
            Some(client as Arc<dyn PreviewClient>),
            &tokio_util::sync::CancellationToken::new(),
            &json!({"action": "screenshot"}),
            Duration::from_secs(5),
            |bytes, extension| {
                *saved.lock().unwrap() = Some((bytes.to_vec(), extension.to_owned()));
                Ok(PathBuf::from("/tmp/shot.png"))
            },
        )
        .await;
        assert!(output.success, "{}", output.content);
        assert_eq!(saved.into_inner().unwrap(), Some((png, "png".to_owned())));
        assert_eq!(image, Some(PathBuf::from("/tmp/shot.png")));
        assert!(output.content.contains("(800×600)"));
        let message = screenshot_attachment_message(&[PathBuf::from("/tmp/shot.png")]).unwrap();
        assert!(message.ends_with("![image](file:///tmp/shot.png)"));
        assert!(screenshot_attachment_message(&[]).is_none());
        assert!(
            rustcode_core::is_tool_attachment(&message),
            "it is not the user's words"
        );
    }

    /// A blank page's runtime error is in the answer, so the model reads the
    /// cause instead of hunting for it.
    #[test]
    fn page_errors_and_a_desktop_layout_are_reported() {
        let png = {
            use base64::{Engine as _, engine::general_purpose::STANDARD};
            STANDARD.encode(b"\x89PNG\r\n\x1a\nrest")
        };
        let response = json!({
            "ok": true, "url": "http://localhost:5173/", "title": "Game",
            "image": {"mimeType": "image/png", "data": png, "width": 1280, "height": 2800},
            "viewport": {"width": 1280, "panelWidth": 381, "desktop": true},
            "errors": ["ReferenceError: Can't find variable: tutorial (/src/main.jsx:212:9)"],
        });
        let (output, _) = interpret_response(&PreviewOperation::Screenshot, &response, |_, _| {
            Ok(PathBuf::from("/tmp/shot.png"))
        });
        assert!(output.success, "{}", output.content);
        assert!(
            output.content.contains(
                "Laid out 1280 px wide, as on a desktop; the user's panel is 381 px wide."
            )
        );
        assert!(output.content.contains("Page errors (from the browser, newest last):\n- ReferenceError: Can't find variable: tutorial"));
        let quiet = json!({"ok": true, "url": "http://localhost:5173/", "title": "Game", "text": "Hi", "errors": []});
        let (output, _) = interpret_response(
            &PreviewOperation::Inspect { max_chars: 100 },
            &quiet,
            |_, _| unreachable!(),
        );
        assert!(!output.content.contains("Page errors"));
    }

    #[test]
    fn an_unanswered_access_request_says_to_stop_and_ask() {
        let output =
            client_failure(&json!({"code": "approval_timeout", "message": "Nobody answered."}));
        assert!(!output.success);
        assert!(
            output
                .content
                .contains("Do not call localhost_preview again this turn")
        );
    }
}
