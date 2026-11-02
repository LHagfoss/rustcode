//! Link metadata for the UI's link chips: a page's title, favicon and theme
//! colour. Only the shell fetches; the webview's CSP allows no remote content.
//!
//! Rules: http(s) only, public addresses only (checked after DNS and pinned,
//! so a redirect or rebinding cannot reach localhost or the LAN), at most
//! three redirects each re-checked, 3 s per request, 512 KB of HTML and
//! 64 KB of favicon, no cookies. Results are cached in memory and on disk
//! for a week, failures for an hour.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_HTML: usize = 512 * 1024;
const MAX_ICON: usize = 64 * 1024;
const MAX_REDIRECTS: usize = 3;
const TTL_OK: u64 = 7 * 24 * 3600;
const TTL_FAILED: u64 = 3600;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    at: u64,
    meta: Option<LinkMeta>,
}

#[derive(Default)]
pub struct LinkCache {
    memory: Mutex<HashMap<String, (u64, Option<LinkMeta>)>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fresh(at: u64, meta: &Option<LinkMeta>) -> bool {
    now().saturating_sub(at) < if meta.is_some() { TTL_OK } else { TTL_FAILED }
}

fn cache_file(dir: &std::path::Path, url: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    dir.join(format!("{:016x}.json", h.finish()))
}

/// Whether an address is on the public internet.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // CGNAT
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // benchmarking
                || o[0] >= 224)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80) // link local
        }
    }
}

/// An http(s) URL whose host resolves only to public addresses, and the address to use.
async fn check(url: &reqwest::Url) -> Result<SocketAddr, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only web links.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Links with credentials are not fetched.".into());
    }
    let host = url.host_str().ok_or("No host.")?;
    let port = url.port_or_known_default().ok_or("No port.")?;
    let addrs: Vec<SocketAddr> =
        tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| "DNS timed out.")?
            .map_err(|e| e.to_string())?
            .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !is_public(a.ip())) {
        return Err("Only public hosts are fetched.".into());
    }
    Ok(addrs[0])
}

/// GET with the rules above, following redirects by hand. Returns the final URL and up to `cap` bytes.
async fn get(url: &str, cap: usize) -> Result<(reqwest::Url, Vec<u8>, Option<String>), String> {
    let mut url = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    for _ in 0..=MAX_REDIRECTS {
        let addr = check(&url).await?;
        let host = url.host_str().unwrap_or_default().to_owned();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .resolve(&host, addr)
            .user_agent("Mozilla/5.0 (Macintosh) RustCode link preview")
            .build()
            .map_err(|e| e.to_string())?;
        let mut response = client
            .get(url.clone())
            .header("Accept", "text/html,image/*;q=0.8,*/*;q=0.5")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or("Redirect without a location.")?;
            url = url.join(location).map_err(|e| e.to_string())?;
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status().as_u16()));
        }
        let kind = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut body = Vec::new();
        let read = async {
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                let room = cap - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
                if body.len() >= cap {
                    break;
                }
            }
            Ok::<(), String>(())
        };
        tokio::time::timeout(TIMEOUT, read)
            .await
            .map_err(|_| "Timed out.")??;
        return Ok((url, body, kind));
    }
    Err("Too many redirects.".into())
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find(';').filter(|e| *e <= 10);
        let Some(end) = end else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ if name.starts_with("#x") || name.starts_with("#X") => {
                u32::from_str_radix(&name[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if name.starts_with('#') => name[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The attributes of one tag, lowercased names.
fn attributes(tag: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let bytes = tag.as_bytes();
    let mut i = 0;
    // Skip the tag name.
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < bytes.len()
            && bytes[i] != b'='
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'/'
        {
            i += 1;
        }
        let name = tag[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let vstart = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value = tag[vstart..i].to_owned();
                i += 1;
            } else {
                let vstart = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = tag[vstart..i].to_owned();
            }
        }
        if !name.is_empty() {
            out.entry(name).or_insert_with(|| decode_entities(&value));
        }
    }
    out
}

fn clean(text: &str, max: usize) -> Option<String> {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(max).collect())
}

/// What a page's head says: title, site name, theme colour and the icon URL.
pub fn parse_head(html: &str) -> (LinkMeta, Option<String>) {
    let lower = html.to_ascii_lowercase();
    let end = lower.find("</head").unwrap_or(html.len());
    let head = &html[..end];
    let lower_head = &lower[..end];
    let mut meta = LinkMeta::default();
    let mut og_title = None;
    let mut icon: Option<(u8, String)> = None;
    let mut pos = 0;
    while let Some(at) = lower_head[pos..].find('<') {
        let start = pos + at + 1;
        let Some(close) = lower_head[start..].find('>') else {
            break;
        };
        let tag = &head[start..start + close];
        let lower_tag = &lower_head[start..start + close];
        pos = start + close + 1;
        if lower_tag.starts_with("title") && meta.title.is_none() {
            if let Some(stop) = lower_head[pos..].find("</title") {
                meta.title = clean(&decode_entities(&head[pos..pos + stop]), 160);
            }
        } else if lower_tag.starts_with("meta") {
            let a = attributes(tag);
            let key = a
                .get("property")
                .or_else(|| a.get("name"))
                .map(|k| k.to_ascii_lowercase())
                .unwrap_or_default();
            let content = a.get("content").cloned().unwrap_or_default();
            match key.as_str() {
                "og:title" | "twitter:title" if og_title.is_none() => {
                    og_title = clean(&content, 160)
                }
                "og:site_name" => meta.site_name = clean(&content, 80),
                "theme-color" | "msapplication-tilecolor" if meta.color.is_none() => {
                    let media_dark = a.get("media").is_some_and(|m| m.contains("dark"));
                    if !media_dark {
                        meta.color = clean(&content, 40);
                    }
                }
                _ => {}
            }
        } else if lower_tag.starts_with("link") {
            let a = attributes(tag);
            let rel = a
                .get("rel")
                .map(|r| r.to_ascii_lowercase())
                .unwrap_or_default();
            let Some(href) = a.get("href").filter(|h| !h.is_empty()) else {
                continue;
            };
            // Prefer a PNG icon, then apple-touch, then any icon.
            let rank = if rel.split_whitespace().any(|r| r == "icon") {
                if href.ends_with(".png") || a.get("type").is_some_and(|t| t.contains("png")) {
                    3
                } else {
                    2
                }
            } else if rel.contains("apple-touch-icon") {
                1
            } else {
                0
            };
            if rank > 0 && icon.as_ref().is_none_or(|(r, _)| rank > *r) {
                icon = Some((rank, href.clone()));
            }
        }
    }
    if og_title.is_some() {
        meta.title = og_title;
    }
    (meta, icon.map(|(_, h)| h))
}

fn icon_mime(bytes: &[u8], declared: Option<&str>) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else if declared.is_some_and(|d| d.contains("svg")) {
        Some("image/svg+xml")
    } else {
        None
    }
}

async fn fetch(url: &str) -> Option<LinkMeta> {
    let (final_url, body, kind) = get(url, MAX_HTML).await.ok()?;
    if !kind.as_deref().unwrap_or("text/html").contains("html") {
        return None;
    }
    let html = String::from_utf8_lossy(&body);
    let (mut meta, icon_href) = parse_head(&html);
    let icon_url = icon_href
        .and_then(|h| final_url.join(&h).ok())
        .or_else(|| final_url.join("/favicon.ico").ok());
    if let Some(icon_url) = icon_url {
        if let Ok((_, bytes, kind)) = get(icon_url.as_str(), MAX_ICON).await {
            if bytes.len() < MAX_ICON {
                if let Some(mime) = icon_mime(&bytes, kind.as_deref()) {
                    use base64::Engine as _;
                    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    meta.icon = Some(format!("data:{mime};base64,{data}"));
                }
            }
        }
    }
    Some(meta)
}

#[tauri::command]
pub async fn link_meta(
    app: tauri::AppHandle,
    cache: tauri::State<'_, LinkCache>,
    url: String,
) -> Result<Option<LinkMeta>, String> {
    use tauri::Manager;
    if url.len() > 2048 || !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only web links.".into());
    }
    if let Some((at, meta)) = cache.memory.lock().map_err(|e| e.to_string())?.get(&url) {
        if fresh(*at, meta) {
            return Ok(meta.clone());
        }
    }
    let dir = app.path().app_cache_dir().ok().map(|d| d.join("links"));
    if let Some(dir) = &dir {
        if let Ok(text) = std::fs::read_to_string(cache_file(dir, &url)) {
            if let Ok(cached) = serde_json::from_str::<Cached>(&text) {
                if fresh(cached.at, &cached.meta) {
                    cache
                        .memory
                        .lock()
                        .map_err(|e| e.to_string())?
                        .insert(url.clone(), (cached.at, cached.meta.clone()));
                    return Ok(cached.meta);
                }
            }
        }
    }
    let meta = fetch(&url).await;
    let at = now();
    {
        let mut memory = cache.memory.lock().map_err(|e| e.to_string())?;
        if memory.len() > 2000 {
            memory.clear();
        }
        memory.insert(url.clone(), (at, meta.clone()));
    }
    if let Some(dir) = dir {
        if std::fs::create_dir_all(&dir).is_ok() {
            let cached = Cached {
                at,
                meta: meta.clone(),
            };
            if let Ok(text) = serde_json::to_string(&cached) {
                let _ = std::fs::write(cache_file(&dir, &url), text);
            }
        }
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_head() {
        let html = r##"<html><head><title> Hello &amp; welcome </title>
            <meta property="og:title" content="RustCode &#8212; PR 1">
            <meta name="theme-color" content="#24292f" media="(prefers-color-scheme: light)">
            <meta name="theme-color" content="#000" media="(prefers-color-scheme: dark)">
            <link rel="apple-touch-icon" href="/apple.png">
            <link rel="icon" type="image/png" href="/fav.png">
            </head><body><title>no</title></body>"##;
        let (meta, icon) = parse_head(html);
        assert_eq!(meta.title.as_deref(), Some("RustCode \u{2014} PR 1"));
        assert_eq!(meta.color.as_deref(), Some("#24292f"));
        assert_eq!(icon.as_deref(), Some("/fav.png"));
    }

    #[test]
    fn falls_back_to_the_title() {
        let (meta, icon) = parse_head("<title>Plain</title>");
        assert_eq!(meta.title.as_deref(), Some("Plain"));
        assert!(icon.is_none());
    }

    #[test]
    fn refuses_private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "172.16.5.4",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public("140.82.112.3".parse().unwrap()));
        assert!(is_public("2606:4700::1111".parse().unwrap()));
    }

    #[tokio::test]
    async fn refuses_local_hosts_and_other_schemes() {
        assert!(
            check(&reqwest::Url::parse("http://localhost:1420/").unwrap())
                .await
                .is_err()
        );
        assert!(check(&reqwest::Url::parse("file:///etc/passwd").unwrap())
            .await
            .is_err());
        assert!(
            check(&reqwest::Url::parse("http://u:p@example.com/").unwrap())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "needs the network"]
    async fn fetches_a_real_page() {
        let meta = fetch("https://github.com/rust-lang/rust")
            .await
            .expect("meta");
        println!(
            "{:?} {:?} icon {}",
            meta.title,
            meta.color,
            meta.icon.is_some()
        );
        assert!(meta.title.is_some());
    }
}
