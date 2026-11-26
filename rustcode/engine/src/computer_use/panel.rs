//! Engine-side browser preview panel controls, screencast streaming, and
//! sanitized element picking over CDP (ADR 0012).

use super::{Cdp, Settings, grants};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Minimum gap between broadcast screencast frames (~15 fps cap per ADR 0012).
pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(66);

const MAX_STR_LEN: usize = 500;
const MAX_SELECTOR_LEN: usize = 240;
const MAX_TEXT_LEN: usize = 280;
const MAX_CLASS_LEN: usize = 200;
const MAX_REACT_COMPONENTS: usize = 6;
const MAX_ATTRIBUTES: usize = 24;
const MAX_STYLES: usize = 24;

const SAFE_ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "role",
    "type",
    "name",
    "href",
    "src",
    "action",
    "placeholder",
    "alt",
    "title",
    "disabled",
    "checked",
    "selected",
    "data-testid",
    "data-test",
    "data-cy",
    "data-slot",
    "data-state",
    "data-variant",
];

const SECRET_PATTERNS: &[&str] = &[
    "bearer ",
    "sk-",
    "sk_live_",
    "sk_test_",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "akia",
    "eyj",
    "token=",
    "apikey=",
    "api_key=",
    "secret=",
    "password=",
    "passwd=",
    "private key",
];

/// Responsive device viewport presets supported by the preview panel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewportPreset {
    #[default]
    Desktop,
    Tablet,
    Mobile,
}

/// Emulated device metrics for one viewport preset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportSpec {
    pub preset: ViewportPreset,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub mobile: bool,
}

impl ViewportPreset {
    /// Returns the default dimensions and device flags for a viewport preset.
    pub fn spec(self, custom_width: Option<u32>, custom_height: Option<u32>) -> ViewportSpec {
        let (default_w, default_h, scale, mobile) = match self {
            Self::Desktop => (1280, 800, 1.0, false),
            Self::Tablet => (768, 1024, 2.0, true),
            Self::Mobile => (390, 844, 2.0, true),
        };
        ViewportSpec {
            preset: self,
            width: custom_width.unwrap_or(default_w).clamp(320, 2560),
            height: custom_height.unwrap_or(default_h).clamp(320, 2560),
            device_scale_factor: scale,
            mobile,
        }
    }
}

/// One JPEG frame received from CDP `Page.screencastFrame`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScreencastFrame {
    pub data: String,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub timestamp_ms: u64,
}

/// Current browser panel status returned to the desktop UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelState {
    pub url: String,
    pub title: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub viewport: ViewportPreset,
    pub width: u32,
    pub height: u32,
    pub inspect_mode: bool,
    pub screencast_active: bool,
    pub frame: Option<ScreencastFrame>,
}

/// Viewport-relative bounding box of a picked DOM element.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ElementRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Sanitized bundle produced by the preview panel's element picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PickedElement {
    pub tag_name: String,
    pub selector: String,
    pub accessible_name: Option<String>,
    pub role: Option<String>,
    pub react_components: Vec<String>,
    pub text_snippet: String,
    pub attributes: BTreeMap<String, String>,
    pub computed_styles: BTreeMap<String, String>,
    pub rect: ElementRect,
    pub url: String,
    pub title: String,
    pub screenshot_png: Option<String>,
}

/// Whether `host` refers to the local machine's loopback interface.
pub fn is_loopback_host(host: &str) -> bool {
    let lower = host
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    lower == "localhost" || lower == "127.0.0.1" || lower == "::1" || lower.ends_with(".localhost")
}

/// Whether the user-driven preview panel may navigate to `address`.
pub fn is_panel_url_allowed(address: &str, settings: &Settings, session_id: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if is_loopback_host(host) {
        return true;
    }
    if settings.allows(address) {
        return true;
    }
    grants::origin(address).is_some_and(|origin| grants::has_origin(session_id, &origin))
}

fn contains_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    SECRET_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

fn clamp_string(raw: Option<&str>, max_len: usize) -> String {
    let text = raw.unwrap_or("").trim();
    if text.chars().count() <= max_len {
        return text.to_owned();
    }
    let prefix: String = text.chars().take(max_len).collect();
    format!("{prefix}…")
}

fn safe_text(raw: Option<&str>, max_len: usize) -> String {
    let clamped = clamp_string(raw, max_len);
    if !clamped.is_empty() && contains_secret(&clamped) {
        "[redacted]".to_owned()
    } else {
        clamped
    }
}

/// Strips query parameters and fragments from an HTTP(S) URL; rejects other schemes.
pub fn sanitize_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed == "about:blank" {
        return trimmed.to_owned();
    }
    let Ok(mut url) = url::Url::parse(trimmed) else {
        return String::new();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return String::new();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn is_default_style_value(prop: &str, value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    match prop {
        "margin" | "padding" | "borderRadius" | "gap" => {
            matches!(trimmed, "0px" | "0px 0px" | "0px 0px 0px 0px" | "normal")
        }
        "position" => trimmed == "static",
        "zIndex" => trimmed == "auto",
        "backgroundColor" => matches!(trimmed, "rgba(0, 0, 0, 0)" | "transparent"),
        "border" => trimmed.starts_with("0px none"),
        "opacity" => trimmed == "1",
        "overflow" => trimmed == "visible",
        "textAlign" => matches!(trimmed, "start" | "left"),
        "fontWeight" => matches!(trimmed, "400" | "normal"),
        _ => matches!(trimmed, "none" | "normal" | "auto"),
    }
}

/// Sanitizes untrusted element inspection output from the page.
pub fn sanitize_element_payload(
    raw: &Value,
    screenshot_png: Option<String>,
) -> Result<PickedElement, String> {
    let obj = raw
        .as_object()
        .ok_or_else(|| "No element found at that position".to_owned())?;
    let tag_name =
        clamp_string(obj.get("tagName").and_then(Value::as_str), 48).to_ascii_lowercase();
    if tag_name.is_empty() {
        return Err("No element found at that position".to_owned());
    }
    let selector = safe_text(
        obj.get("selector").and_then(Value::as_str),
        MAX_SELECTOR_LEN,
    );
    let accessible_name = {
        let name = safe_text(
            obj.get("accessibleName").and_then(Value::as_str),
            MAX_STR_LEN,
        );
        (!name.is_empty()).then_some(name)
    };
    let role = {
        let r = safe_text(obj.get("role").and_then(Value::as_str), 64);
        (!r.is_empty()).then_some(r)
    };
    let react_components: Vec<String> = obj
        .get("reactComponents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| safe_text(Some(s), 96))
                .filter(|s| !s.is_empty() && s != "[redacted]")
                .take(MAX_REACT_COMPONENTS)
                .collect()
        })
        .unwrap_or_default();
    let text_snippet = safe_text(obj.get("textSnippet").and_then(Value::as_str), MAX_TEXT_LEN);

    let mut attributes = BTreeMap::new();
    if let Some(raw_attrs) = obj.get("attributes").and_then(Value::as_object) {
        for (key, val) in raw_attrs {
            if attributes.len() >= MAX_ATTRIBUTES {
                break;
            }
            let name = key.to_ascii_lowercase();
            if !name.starts_with("aria-") && !SAFE_ATTRIBUTES.contains(&name.as_str()) {
                continue;
            }
            let raw_str = val.as_str().unwrap_or("");
            let cleaned = if contains_secret(raw_str) {
                "[redacted]".to_owned()
            } else if matches!(name.as_str(), "href" | "src" | "action") {
                sanitize_url(raw_str)
            } else if name == "class" {
                clamp_string(Some(raw_str), MAX_CLASS_LEN)
            } else {
                clamp_string(Some(raw_str), MAX_STR_LEN)
            };
            if !cleaned.is_empty() {
                attributes.insert(name, cleaned);
            }
        }
    }

    let mut computed_styles = BTreeMap::new();
    if let Some(raw_styles) = obj.get("computedStyles").and_then(Value::as_object) {
        for (prop, val) in raw_styles {
            if computed_styles.len() >= MAX_STYLES {
                break;
            }
            let Some(style_val) = val.as_str() else {
                continue;
            };
            if is_default_style_value(prop, style_val) {
                continue;
            }
            let cleaned = safe_text(Some(style_val), 160);
            if !cleaned.is_empty() {
                computed_styles.insert(prop.clone(), cleaned);
            }
        }
    }

    let rect_obj = obj.get("rect").and_then(Value::as_object);
    let num = |field: &str| {
        rect_obj
            .and_then(|r| r.get(field))
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    };
    let rect = ElementRect {
        x: num("x"),
        y: num("y"),
        width: num("width").max(0.0),
        height: num("height").max(0.0),
    };

    let url = obj
        .get("url")
        .and_then(Value::as_str)
        .map(sanitize_url)
        .unwrap_or_default();
    let title = safe_text(obj.get("title").and_then(Value::as_str), MAX_STR_LEN);

    Ok(PickedElement {
        tag_name,
        selector,
        accessible_name,
        role,
        react_components,
        text_snippet,
        attributes,
        computed_styles,
        rect,
        url,
        title,
        screenshot_png,
    })
}

/// Computes a padded CDP `Page.captureScreenshot` clip rectangle for a picked element.
pub fn cropped_clip(rect: &ElementRect, viewport_w: f64, viewport_h: f64) -> Option<Value> {
    if rect.width <= 1.0 || rect.height <= 1.0 || viewport_w <= 1.0 || viewport_h <= 1.0 {
        return None;
    }
    let pad = 8.0;
    let x = (rect.x - pad).clamp(0.0, viewport_w - 1.0);
    let y = (rect.y - pad).clamp(0.0, viewport_h - 1.0);
    let max_w = (viewport_w - x).max(1.0);
    let max_h = (viewport_h - y).max(1.0);
    let width = (rect.width + pad * 2.0).clamp(1.0, max_w);
    let height = (rect.height + pad * 2.0).clamp(1.0, max_h);
    Some(json!({
        "x": x,
        "y": y,
        "width": width,
        "height": height,
        "scale": 1,
    }))
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn element_extract_expression(x: f64, y: f64) -> String {
    format!(
        r#"(() => {{
  const el = document.elementFromPoint({x}, {y});
  if (!el) return null;
  const cssEscape = (v) => (window.CSS && CSS.escape ? CSS.escape(v) : String(v).replace(/[^a-zA-Z0-9_-]/g, '\\$&'));
  const buildSelector = (node) => {{
    if (node.id) return '#' + cssEscape(node.id);
    const testId = node.getAttribute('data-testid') || node.getAttribute('data-test');
    if (testId) return node.tagName.toLowerCase() + '[data-testid="' + testId.replace(/"/g, '\\"') + '"]';
    const parts = [];
    let cur = node;
    for (let depth = 0; cur && cur.nodeType === 1 && depth < 5; depth++) {{
      let part = cur.tagName.toLowerCase();
      if (cur.id) {{ parts.unshift('#' + cssEscape(cur.id)); break; }}
      const parent = cur.parentElement;
      if (parent) {{
        const same = Array.from(parent.children).filter((c) => c.tagName === cur.tagName);
        if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(cur) + 1) + ')';
      }}
      parts.unshift(part);
      cur = parent;
    }}
    return parts.join(' > ');
  }};
  const getReactStack = (node) => {{
    const key = Object.keys(node).find((k) => k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$'));
    if (!key) return [];
    const names = [];
    let fiber = node[key];
    for (let steps = 0; fiber && names.length < 6 && steps < 40; steps++) {{
      const type = fiber.type;
      const name = typeof type === 'function'
        ? (type.displayName || type.name || '')
        : (type && typeof type === 'object' ? (type.displayName || (type.render && (type.render.displayName || type.render.name)) || '') : '');
      if (name && /^[A-Z]/.test(name) && !names.includes(name)) names.push(name);
      fiber = fiber.return;
    }}
    return names;
  }};
  const getAccessibleName = (node) => {{
    const aria = node.getAttribute('aria-label');
    if (aria && aria.trim()) return aria.trim();
    const labelledBy = node.getAttribute('aria-labelledby');
    if (labelledBy) {{
      const joined = labelledBy.split(/\s+/).map((id) => document.getElementById(id)?.textContent?.trim() || '').filter(Boolean).join(' ');
      if (joined) return joined;
    }}
    if (node.id) {{
      const lbl = document.querySelector('label[for="' + cssEscape(node.id) + '"]');
      if (lbl && lbl.textContent && lbl.textContent.trim()) return lbl.textContent.trim();
    }}
    return (node.getAttribute('alt') || node.getAttribute('title') || node.getAttribute('placeholder') || node.innerText || node.textContent || '').trim().slice(0, 200);
  }};
  const attrs = {{}};
  for (const attr of Array.from(el.attributes || [])) {{
    attrs[attr.name] = attr.value;
  }}
  const cs = window.getComputedStyle(el);
  const styleKeys = ['display','position','width','height','margin','padding','gap','color','backgroundColor','border','borderRadius','fontFamily','fontSize','fontWeight','lineHeight','textAlign','opacity','zIndex','overflow'];
  const computedStyles = {{}};
  for (const k of styleKeys) computedStyles[k] = cs[k] || '';
  const r = el.getBoundingClientRect();
  return {{
    tagName: el.tagName.toLowerCase(),
    selector: buildSelector(el),
    accessibleName: getAccessibleName(el),
    role: el.getAttribute('role') || '',
    reactComponents: getReactStack(el),
    textSnippet: (el.innerText || el.textContent || '').trim().slice(0, 280),
    attributes: attrs,
    computedStyles,
    rect: {{ x: r.x, y: r.y, width: r.width, height: r.height }},
    url: location.href,
    title: document.title || '',
  }};
}})()"#
    )
}

async fn read_navigation_state(
    cdp: &mut Cdp,
    latest_frame: Option<ScreencastFrame>,
) -> Result<PanelState, String> {
    let history = cdp.command("Page.getNavigationHistory", json!({})).await?;
    let index = history["currentIndex"].as_i64().unwrap_or(0);
    let entries = history["entries"].as_array();
    let count = entries.map_or(0, Vec::len) as i64;
    let current = entries.and_then(|list| list.get(usize::try_from(index).ok()?));
    let url = current
        .and_then(|e| e["url"].as_str())
        .unwrap_or("about:blank")
        .to_owned();
    let title = current
        .and_then(|e| e["title"].as_str())
        .unwrap_or("")
        .to_owned();
    Ok(PanelState {
        url,
        title,
        can_go_back: index > 0,
        can_go_forward: index + 1 < count,
        viewport: cdp.viewport.preset,
        width: cdp.viewport.width,
        height: cdp.viewport.height,
        inspect_mode: cdp.inspect_mode,
        screencast_active: cdp.screencast_active,
        frame: latest_frame,
    })
}

async fn start_screencast(cdp: &mut Cdp) -> Result<(), String> {
    let _ = cdp.command("Page.stopScreencast", json!({})).await;
    cdp.command(
        "Page.startScreencast",
        json!({
            "format": "jpeg",
            "quality": 72,
            "maxWidth": cdp.viewport.width,
            "maxHeight": cdp.viewport.height,
            "everyNthFrame": 1,
        }),
    )
    .await?;
    cdp.screencast_active = true;
    Ok(())
}

async fn apply_viewport(cdp: &mut Cdp, spec: ViewportSpec) -> Result<(), String> {
    cdp.command(
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": spec.width,
            "height": spec.height,
            "deviceScaleFactor": spec.device_scale_factor,
            "mobile": spec.mobile,
        }),
    )
    .await?;
    cdp.viewport = spec;
    if cdp.screencast_active {
        start_screencast(cdp).await?;
    }
    Ok(())
}

async fn set_inspect_mode(cdp: &mut Cdp, enabled: bool) -> Result<(), String> {
    let _ = cdp.command("DOM.enable", json!({})).await;
    let _ = cdp.command("Overlay.enable", json!({})).await;
    let params = if enabled {
        json!({
            "mode": "searchForNode",
            "highlightConfig": {
                "showInfo": true,
                "contentColor": { "r": 111, "g": 168, "b": 220, "a": 0.45 },
                "paddingColor": { "r": 147, "g": 196, "b": 125, "a": 0.4 },
                "borderColor": { "r": 255, "g": 229, "b": 153, "a": 0.5 },
                "marginColor": { "r": 246, "g": 178, "b": 107, "a": 0.4 },
            }
        })
    } else {
        json!({ "mode": "none" })
    };
    cdp.command("Overlay.setInspectMode", params).await?;
    cdp.inspect_mode = enabled;
    Ok(())
}

/// Executes a preview panel CDP action on the thread's shared headless Chrome.
pub(super) async fn handle_cdp_action(
    cdp: &mut Cdp,
    tool: &str,
    args: &Value,
    session_id: &str,
    latest_frame: Option<ScreencastFrame>,
) -> Result<Value, String> {
    match tool {
        "panel_open" => {
            let url = args["url"].as_str().ok_or("Missing URL")?;
            let settings = Settings::load();
            if !is_panel_url_allowed(url, &settings, session_id) {
                return Err("URL is outside allowed domains and not loopback".into());
            }
            if let Some(origin) = grants::origin(url) {
                grants::grant(session_id, &origin);
            }
            apply_viewport(cdp, cdp.viewport).await?;
            cdp.loaded = false;
            cdp.refs.clear();
            let response = cdp.command("Page.navigate", json!({ "url": url })).await?;
            if response.get("errorText").is_some() {
                return Err(response.to_string());
            }
            if response.get("loaderId").is_some() {
                let _ = cdp.wait_for_load().await;
            }
            start_screencast(cdp).await?;
            let state = read_navigation_state(cdp, latest_frame).await?;
            serde_json::to_value(state).map_err(|e| e.to_string())
        }
        "panel_state" => {
            let state = read_navigation_state(cdp, latest_frame).await?;
            serde_json::to_value(state).map_err(|e| e.to_string())
        }
        "panel_stream" => {
            let active = args["active"].as_bool().unwrap_or(true);
            if active {
                apply_viewport(cdp, cdp.viewport).await?;
                start_screencast(cdp).await?;
            } else {
                let _ = cdp.command("Page.stopScreencast", json!({})).await;
                cdp.screencast_active = false;
            }
            let state = read_navigation_state(cdp, latest_frame).await?;
            serde_json::to_value(state).map_err(|e| e.to_string())
        }
        "panel_navigate" => {
            let action = args["action"].as_str().unwrap_or("reload");
            match action {
                "goto" => {
                    let url = args["url"].as_str().ok_or("Missing URL")?;
                    let settings = Settings::load();
                    if !is_panel_url_allowed(url, &settings, session_id) {
                        return Err("URL is outside allowed domains and not loopback".into());
                    }
                    if let Some(origin) = grants::origin(url) {
                        grants::grant(session_id, &origin);
                    }
                    cdp.loaded = false;
                    cdp.refs.clear();
                    let response = cdp.command("Page.navigate", json!({ "url": url })).await?;
                    if response.get("errorText").is_some() {
                        return Err(response.to_string());
                    }
                    if response.get("loaderId").is_some() {
                        let _ = cdp.wait_for_load().await;
                    }
                }
                "reload" => {
                    cdp.loaded = false;
                    cdp.command("Page.reload", json!({})).await?;
                    let _ = cdp.wait_for_load().await;
                }
                "back" | "forward" => {
                    let history = cdp.command("Page.getNavigationHistory", json!({})).await?;
                    let index = history["currentIndex"].as_i64().unwrap_or(0);
                    let target = if action == "back" {
                        index - 1
                    } else {
                        index + 1
                    };
                    if let Some(entry_id) = usize::try_from(target)
                        .ok()
                        .and_then(|i| history["entries"].as_array()?.get(i))
                        .and_then(|entry| entry["id"].as_i64())
                    {
                        cdp.loaded = false;
                        cdp.command(
                            "Page.navigateToHistoryEntry",
                            json!({ "entryId": entry_id }),
                        )
                        .await?;
                        let _ = cdp.wait_for_load().await;
                    }
                }
                _ => return Err(format!("Unknown navigation action: {action}")),
            }
            let state = read_navigation_state(cdp, latest_frame).await?;
            serde_json::to_value(state).map_err(|e| e.to_string())
        }
        "panel_viewport" => {
            let preset: ViewportPreset = serde_json::from_value(
                args.get("preset")
                    .cloned()
                    .unwrap_or_else(|| json!("desktop")),
            )
            .unwrap_or_default();
            let width = args
                .get("width")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok());
            let height = args
                .get("height")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok());
            apply_viewport(cdp, preset.spec(width, height)).await?;
            let state = read_navigation_state(cdp, latest_frame).await?;
            serde_json::to_value(state).map_err(|e| e.to_string())
        }
        "panel_input" => {
            let event = args.get("event").ok_or("Missing input event")?;
            let kind = event["kind"].as_str().ok_or("Missing input kind")?;
            match kind {
                "mouse" => {
                    let mut params = json!({
                        "type": event["type"].as_str().unwrap_or("mouseMoved"),
                        "x": event["x"].as_f64().unwrap_or(0.0),
                        "y": event["y"].as_f64().unwrap_or(0.0),
                    });
                    if let Some(button) = event.get("button").and_then(Value::as_str) {
                        params["button"] = json!(button);
                    }
                    if let Some(clicks) = event.get("clickCount").and_then(Value::as_i64) {
                        params["clickCount"] = json!(clicks);
                    }
                    if let Some(dx) = event.get("deltaX").and_then(Value::as_f64) {
                        params["deltaX"] = json!(dx);
                    }
                    if let Some(dy) = event.get("deltaY").and_then(Value::as_f64) {
                        params["deltaY"] = json!(dy);
                    }
                    if let Some(modifiers) = event.get("modifiers").and_then(Value::as_i64) {
                        params["modifiers"] = json!(modifiers);
                    }
                    cdp.command("Input.dispatchMouseEvent", params).await?;
                }
                "key" => {
                    let mut params = json!({
                        "type": event["type"].as_str().unwrap_or("keyDown"),
                    });
                    for field in ["key", "code", "text"] {
                        if let Some(val) = event.get(field).and_then(Value::as_str) {
                            params[field] = json!(val);
                        }
                    }
                    if let Some(vk) = event.get("windowsVirtualKeyCode").and_then(Value::as_i64) {
                        params["windowsVirtualKeyCode"] = json!(vk);
                    }
                    if let Some(modifiers) = event.get("modifiers").and_then(Value::as_i64) {
                        params["modifiers"] = json!(modifiers);
                    }
                    cdp.command("Input.dispatchKeyEvent", params).await?;
                }
                "text" => {
                    let text = event["text"].as_str().unwrap_or("");
                    if !text.is_empty() {
                        cdp.command("Input.insertText", json!({ "text": text }))
                            .await?;
                    }
                }
                _ => return Err(format!("Unknown input kind: {kind}")),
            }
            Ok(json!({ "ok": true }))
        }
        "panel_inspect" => {
            let enabled = args["enabled"].as_bool().unwrap_or(false);
            set_inspect_mode(cdp, enabled).await?;
            let state = read_navigation_state(cdp, latest_frame).await?;
            serde_json::to_value(state).map_err(|e| e.to_string())
        }
        "panel_pick" => {
            let x = args["x"].as_f64().ok_or("Missing x coordinate")?;
            let y = args["y"].as_f64().ok_or("Missing y coordinate")?;
            if cdp.inspect_mode {
                let _ = set_inspect_mode(cdp, false).await;
            }
            let eval = cdp
                .command(
                    "Runtime.evaluate",
                    json!({
                        "expression": element_extract_expression(x, y),
                        "returnByValue": true,
                    }),
                )
                .await?;
            if eval.get("exceptionDetails").is_some() {
                return Err("Could not inspect element at that position".into());
            }
            let raw = &eval["result"]["value"];
            let mut picked = sanitize_element_payload(raw, None)?;
            if let Some(clip) = cropped_clip(
                &picked.rect,
                f64::from(cdp.viewport.width),
                f64::from(cdp.viewport.height),
            ) && let Ok(shot) = cdp
                .command(
                    "Page.captureScreenshot",
                    json!({ "format": "png", "clip": clip }),
                )
                .await
            {
                picked.screenshot_png = shot["data"].as_str().map(str::to_owned);
            }
            serde_json::to_value(picked).map_err(|e| e.to_string())
        }
        _ => Err(format!("Unknown panel action: {tool}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_and_allowlist_rules_permit_local_dev_servers_and_block_credentials() {
        let empty = Settings::default();
        assert!(is_panel_url_allowed(
            "http://localhost:5173/app",
            &empty,
            "s1"
        ));
        assert!(is_panel_url_allowed("http://127.0.0.1:3000/", &empty, "s1"));
        assert!(is_panel_url_allowed("http://[::1]:8080/", &empty, "s1"));
        assert!(!is_panel_url_allowed("https://example.com", &empty, "s1"));
        assert!(!is_panel_url_allowed("file:///etc/passwd", &empty, "s1"));
        assert!(!is_panel_url_allowed(
            "http://user:pass@localhost:3000/",
            &empty,
            "s1"
        ));

        let allow = Settings {
            enabled: true,
            allowed_domains: vec!["staging.example.com".into()],
            ..Default::default()
        };
        assert!(is_panel_url_allowed(
            "https://staging.example.com/home",
            &allow,
            "s1"
        ));
    }

    #[test]
    fn viewport_presets_clamp_and_set_mobile_metrics() {
        let desktop = ViewportPreset::Desktop.spec(None, None);
        assert_eq!(
            (desktop.width, desktop.height, desktop.mobile),
            (1280, 800, false)
        );
        let tablet = ViewportPreset::Tablet.spec(None, None);
        assert_eq!(
            (tablet.width, tablet.height, tablet.mobile),
            (768, 1024, true)
        );
        let mobile = ViewportPreset::Mobile.spec(None, None);
        assert_eq!(
            (mobile.width, mobile.height, mobile.mobile),
            (390, 844, true)
        );
        let custom = ViewportPreset::Desktop.spec(Some(100), Some(9999));
        assert_eq!((custom.width, custom.height), (320, 2560));
    }

    #[test]
    fn sanitize_element_payload_redacts_secrets_filters_defaults_and_strips_url_queries() {
        let raw = json!({
            "tagName": "BUTTON",
            "selector": "button[data-testid=\"checkout\"]",
            "accessibleName": "Submit order",
            "role": "button",
            "reactComponents": ["CheckoutButton", "CartFooter"],
            "textSnippet": "Submit order",
            "attributes": {
                "data-testid": "checkout",
                "aria-label": "Bearer supersecrettoken123",
                "onclick": "alert(1)",
                "data-token": "ghp_secretvalue",
                "href": "https://app.example.com/checkout?token=abc#top",
                "src": "javascript:alert(1)"
            },
            "computedStyles": {
                "display": "flex",
                "position": "static",
                "margin": "0px",
                "backgroundColor": "rgba(0, 0, 0, 0)",
                "color": "rgb(255, 255, 255)",
                "borderRadius": "8px",
                "fontWeight": "600"
            },
            "rect": { "x": 20.0, "y": 40.0, "width": 120.0, "height": 36.0 },
            "url": "http://localhost:5173/cart?session=secret#pay",
            "title": "Cart"
        });

        let picked = sanitize_element_payload(&raw, Some("pngdata".into())).unwrap();
        assert_eq!(picked.tag_name, "button");
        assert_eq!(
            picked.react_components,
            vec!["CheckoutButton", "CartFooter"]
        );
        assert_eq!(
            picked.attributes.get("data-testid").map(String::as_str),
            Some("checkout")
        );
        assert_eq!(
            picked.attributes.get("aria-label").map(String::as_str),
            Some("[redacted]")
        );
        assert_eq!(
            picked.attributes.get("href").map(String::as_str),
            Some("https://app.example.com/checkout")
        );
        assert!(!picked.attributes.contains_key("onclick"));
        assert!(!picked.attributes.contains_key("data-token"));
        assert!(!picked.attributes.contains_key("src"));
        assert_eq!(
            picked.computed_styles.get("display").map(String::as_str),
            Some("flex")
        );
        assert_eq!(
            picked
                .computed_styles
                .get("borderRadius")
                .map(String::as_str),
            Some("8px")
        );
        assert!(!picked.computed_styles.contains_key("position"));
        assert!(!picked.computed_styles.contains_key("margin"));
        assert!(!picked.computed_styles.contains_key("backgroundColor"));
        assert_eq!(picked.url, "http://localhost:5173/cart");
        assert_eq!(picked.screenshot_png.as_deref(), Some("pngdata"));
    }

    #[test]
    fn cropped_clip_pads_and_clamps_to_viewport() {
        let clip = cropped_clip(
            &ElementRect {
                x: 2.0,
                y: 4.0,
                width: 100.0,
                height: 40.0,
            },
            390.0,
            844.0,
        )
        .unwrap();
        assert_eq!(clip["x"], json!(0.0));
        assert_eq!(clip["y"], json!(0.0));
        assert_eq!(clip["width"], json!(116.0));
        assert_eq!(clip["height"], json!(56.0));
        assert!(cropped_clip(&ElementRect::default(), 390.0, 844.0).is_none());
    }
}
