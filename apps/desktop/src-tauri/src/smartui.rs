//! Smart UI pages (docs/smart-ui.md). The app's own CSP would also bind a
//! `srcdoc` frame (it inherits its parent's policy, and the app allows no
//! inline script), so a page is served from its own `rcui://` scheme with
//! its own policy instead. The webview hands over the page with
//! `smartui_page`, gets a one-time id, and points a sandboxed iframe
//! (`allow-scripts` only, an opaque origin) at `rcui://localhost/<id>`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::http::{Request, Response};

/// The page's policy; the page repeats it in a meta tag.
pub const PAGE_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";

/// A block is at most 64 KB; Chart.js and the runtime add about 220 KB.
const MAX_PAGE_BYTES: usize = 1024 * 1024;
const MAX_PAGES: usize = 16;

#[derive(Default)]
pub struct SmartUiPages {
    next: Mutex<u64>,
    pages: Mutex<HashMap<String, String>>,
}

impl SmartUiPages {
    pub fn put(&self, html: String) -> Result<String, String> {
        if html.len() > MAX_PAGE_BYTES {
            return Err("The page is too big.".into());
        }
        let mut next = self.next.lock().map_err(|e| e.to_string())?;
        *next += 1;
        // Unguessable enough for a local, one-time id: a counter and the clock's nanoseconds.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let id = format!("{:x}{:08x}", *next, nanos);
        let mut pages = self.pages.lock().map_err(|e| e.to_string())?;
        if pages.len() >= MAX_PAGES {
            pages.clear();
        }
        pages.insert(id.clone(), html);
        Ok(id)
    }

    /// The page for a request path, once: a reload gets nothing.
    pub fn take(&self, path: &str) -> Option<String> {
        let id = path.trim_start_matches('/');
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        self.pages.lock().ok()?.remove(id)
    }
}

#[tauri::command]
pub fn smartui_page(pages: tauri::State<'_, SmartUiPages>, html: String) -> Result<String, String> {
    pages.put(html)
}

pub fn respond(pages: &SmartUiPages, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let builder = Response::builder()
        .header("Content-Security-Policy", PAGE_CSP)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-store")
        .header("Referrer-Policy", "no-referrer");
    match pages.take(request.uri().path()) {
        Some(html) => builder
            .status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(Cow::Owned(html.into_bytes())),
        None => builder
            .status(404)
            .header("Content-Type", "text/plain")
            .body(Cow::Borrowed(&b"Not found"[..])),
    }
    .unwrap_or_else(|_| Response::new(Cow::Borrowed(&b""[..])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_a_page_once() {
        let pages = SmartUiPages::default();
        let id = pages.put("<p>hi</p>".into()).unwrap();
        let request = Request::builder()
            .uri(format!("rcui://localhost/{id}"))
            .body(Vec::new())
            .unwrap();
        let first = respond(&pages, &request);
        assert_eq!(first.status(), 200);
        assert_eq!(
            first.headers()["Content-Security-Policy"].to_str().unwrap(),
            PAGE_CSP
        );
        assert_eq!(respond(&pages, &request).status(), 404);
        assert!(pages.take("/../x").is_none());
        assert!(pages.put("x".repeat(MAX_PAGE_BYTES + 1)).is_err());
    }
}
