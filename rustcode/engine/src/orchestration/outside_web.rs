//! The app served from a hub (#3958, `docs/architecture/hub-plan.md`
//! phase 6): the owner opens the public address in a browser, signs in with
//! their passkey (`outside_passkey.rs`) and uses RustCode there.
//!
//! - **Login.** `GET /login` asks for the owner's passkey; `POST /login`
//!   checks the answer and sets a session cookie (`__Host-`, `HttpOnly`,
//!   `Secure`, `SameSite=Strict`, 30 days). Only its hash is kept, in
//!   `<config>/owner-web-sessions.json`, where Settings › Outside agents and
//!   `rc outside owner sessions` list it and sign it out.
//! - **The app.** The production web build (`bun run build:hub`) is served
//!   from [`web_dir`] to a signed-in browser only.
//! - **ACP.** The page trades its cookie for a ticket (`POST /app/ticket`,
//!   single use, 30 seconds) and opens `GET /app/acp?ticket=…` with it. The
//!   socket is relayed to this machine's server with the same method list a
//!   paired desktop gets (`remote_gateway/acp_relay.rs`), and closes once the
//!   session is signed out or expires.
//! - **Separate audiences.** An outside agent's bearer token is refused on
//!   these routes, and a web session is refused on `/mcp`.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SESSION_FILE: &str = "owner-web-sessions.json";
/// The cookie's name. `__Host-` makes the browser keep it to this host,
/// over HTTPS, at path `/`, with no `Domain`.
pub const COOKIE: &str = "__Host-rustcode-session";
/// How long a browser stays signed in.
pub const SESSION_DAYS: i64 = 30;
const MAX_SESSIONS: usize = 20;
/// How long a socket ticket can be used, and how many can be open at once.
const TICKET_LIFETIME: Duration = Duration::from_secs(30);
const MAX_TICKETS: usize = 64;
/// How often `last_used_at` is written while a browser keeps using it.
const TOUCH_SECS: i64 = 300;

pub const LOGIN_PATH: &str = "/login";
pub const LOGOUT_PATH: &str = "/logout";
pub const TICKET_PATH: &str = "/app/ticket";
pub const SOCKET_PATH: &str = "/app/acp";

// ---------------------------------------------------------------- storage

/// One signed-in browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSession {
    pub id: String,
    hash: String,
    /// The browser and system it said it was, such as "Safari on macOS".
    pub name: String,
    /// The passkey that signed it in; removing that passkey signs it out.
    pub passkey: String,
    pub created_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
    pub expires_at: i64,
}

impl WebSession {
    /// What Settings and the CLI show: never the secret's hash.
    pub fn view(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "passkey": self.passkey,
            "createdAt": self.created_at,
            "lastUsedAt": self.last_used_at,
            "expiresAt": chrono::DateTime::from_timestamp(self.expires_at, 0)
                .map(|at| at.to_rfc3339()),
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
struct SessionStore {
    #[serde(default)]
    sessions: Vec<WebSession>,
}

static STORE_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn session_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(SESSION_FILE))
}

fn read(path: &Path) -> SessionStore {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write(path: &Path, store: &SessionStore) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(store).map_err(|error| error.to_string())?;
    crate::service::write_private(path, &bytes).map_err(|error| error.to_string())
}

fn random_b64(bytes: usize) -> String {
    let mut out = Vec::with_capacity(bytes);
    while out.len() < bytes {
        out.extend_from_slice(&rand::random::<u128>().to_le_bytes());
    }
    out.truncate(bytes);
    URL_SAFE_NO_PAD.encode(out)
}

fn hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The browsers signed in now, newest first.
pub fn list_at(path: &Path) -> Vec<WebSession> {
    let now = now();
    let mut sessions: Vec<WebSession> = read(path)
        .sessions
        .into_iter()
        .filter(|session| session.expires_at > now)
        .collect();
    sessions.reverse();
    sessions
}

pub fn list() -> Vec<WebSession> {
    session_path()
        .map(|path| list_at(&path))
        .unwrap_or_default()
}

/// Sign a browser in. Returns the session's id and the cookie's secret,
/// which is kept only as a hash. The oldest session goes past
/// [`MAX_SESSIONS`].
pub(super) fn create_at(
    path: &Path,
    name: &str,
    passkey: &str,
) -> Result<(String, String), String> {
    let id = random_b64(9);
    let secret = random_b64(32);
    let now = now();
    let _guard = lock();
    let mut store = read(path);
    store.sessions.retain(|session| session.expires_at > now);
    while store.sessions.len() >= MAX_SESSIONS {
        store.sessions.remove(0);
    }
    store.sessions.push(WebSession {
        id: id.clone(),
        hash: hash(&secret),
        name: name.chars().take(60).collect(),
        passkey: passkey.to_owned(),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_used_at: None,
        expires_at: now + SESSION_DAYS * 86_400,
    });
    write(path, &store)?;
    Ok((id, secret))
}

/// The session a cookie's secret belongs to, if it is still signed in.
pub(super) fn check_at(path: &Path, secret: &str) -> Option<String> {
    if secret.is_empty() {
        return None;
    }
    let wanted = hash(secret);
    let now = now();
    let _guard = lock();
    let mut store = read(path);
    let session = store
        .sessions
        .iter_mut()
        .find(|session| session.expires_at > now && super::outside::same(&session.hash, &wanted))?;
    let id = session.id.clone();
    let stale = session
        .last_used_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_none_or(|at| now - at.timestamp() >= TOUCH_SECS);
    if stale {
        session.last_used_at = Some(chrono::Utc::now().to_rfc3339());
        let _ = write(path, &store);
    }
    Some(id)
}

/// Whether the session `id` is still signed in.
pub(crate) fn active_at(path: &Path, id: &str) -> bool {
    let now = now();
    read(path)
        .sessions
        .iter()
        .any(|session| session.id == id && session.expires_at > now)
}

/// Sign out the browser holding `secret` (its own *Sign out*).
pub(super) fn sign_out_secret_at(path: &Path, secret: &str) {
    let wanted = hash(secret);
    let _guard = lock();
    let mut store = read(path);
    let before = store.sessions.len();
    store
        .sessions
        .retain(|session| !super::outside::same(&session.hash, &wanted));
    if store.sessions.len() != before {
        let _ = write(path, &store);
    }
}

/// Sign out by id, a unique id prefix, or `all`. Returns what was signed out.
pub fn remove_at(path: &Path, selector: &str) -> Result<Vec<WebSession>, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("name a session id, or `all`".into());
    }
    let _guard = lock();
    let mut store = read(path);
    let chosen = |session: &WebSession| selector == "all" || session.id == selector;
    let exact = store.sessions.iter().any(chosen);
    let prefixed = store
        .sessions
        .iter()
        .filter(|session| session.id.starts_with(selector))
        .count();
    if !exact && prefixed > 1 {
        return Err(format!(
            "{selector} matches several sessions; give more of the id"
        ));
    }
    let (removed, kept): (Vec<WebSession>, Vec<WebSession>) = store
        .sessions
        .into_iter()
        .partition(|session| chosen(session) || (!exact && session.id.starts_with(selector)));
    if removed.is_empty() {
        return Err(format!("no browser session matches {selector}"));
    }
    store.sessions = kept;
    write(path, &store)?;
    Ok(removed)
}

pub fn remove(selector: &str) -> Result<Vec<WebSession>, String> {
    let path = session_path().ok_or("configuration unavailable")?;
    let removed = remove_at(&path, selector)?;
    super::outside::changed();
    Ok(removed)
}

/// Sign out every browser a removed passkey signed in.
pub(super) fn forget_passkey_at(path: &Path, passkey: &str) {
    let _guard = lock();
    let mut store = read(path);
    let before = store.sessions.len();
    store.sessions.retain(|session| session.passkey != passkey);
    if store.sessions.len() != before {
        let _ = write(path, &store);
    }
}

pub(super) fn forget_passkey(passkey: &str) {
    if let Some(path) = session_path() {
        forget_passkey_at(&path, passkey);
    }
}

// ---------------------------------------------------------------- tickets

static TICKETS: Mutex<Vec<(String, String, Instant)>> = Mutex::new(Vec::new());

/// A single-use ticket that opens one socket for `session`.
pub(super) fn new_ticket(session: &str) -> String {
    let ticket = random_b64(32);
    let mut open = TICKETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    open.retain(|(_, _, expires)| *expires > now);
    if open.len() >= MAX_TICKETS {
        open.remove(0);
    }
    open.push((hash(&ticket), session.to_owned(), now + TICKET_LIFETIME));
    ticket
}

/// The session a ticket was made for; the ticket works only once.
pub(super) fn spend_ticket(ticket: &str) -> Option<String> {
    let wanted = hash(ticket);
    let mut open = TICKETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    open.retain(|(_, _, expires)| *expires > now);
    let index = open
        .iter()
        .position(|(hash, _, _)| super::outside::same(hash, &wanted))?;
    Some(open.remove(index).1)
}

// ---------------------------------------------------------------- cookies

/// The session cookie's value in a request's `Cookie` header.
pub(super) fn cookie(headers: &HashMap<String, String>) -> Option<&str> {
    headers.get("cookie")?.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == COOKIE).then_some(value.trim())
    })
}

pub(super) fn set_cookie(secret: &str) -> String {
    format!(
        "Set-Cookie: {COOKIE}={secret}; Path=/; Max-Age={}; HttpOnly; Secure; SameSite=Strict\r\n",
        SESSION_DAYS * 86_400
    )
}

pub(super) fn clear_cookie() -> String {
    format!("Set-Cookie: {COOKIE}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Strict\r\n")
}

/// "Safari on macOS" from a `User-Agent`, for the list of sessions.
pub(super) fn browser_name(agent: &str) -> String {
    let browser = [
        ("Edg/", "Edge"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]
    .iter()
    .find(|(marker, _)| agent.contains(marker))
    .map_or("A browser", |(_, name)| name);
    let system = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Mac OS X", "macOS"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(marker, _)| agent.contains(marker))
    .map(|(_, name)| *name);
    match system {
        Some(system) => format!("{browser} on {system}"),
        None => browser.to_owned(),
    }
}

// ------------------------------------------------------------------ pages

/// The login page: the owner's passkeys for this host, or how to add one.
pub(super) fn login_page(machine: &str, passkeys: &Path, base: &str) -> (String, String) {
    use super::outside_signin::{escape, message_page, page};
    let Some(prompt) = super::outside_passkey::PasskeyPrompt::for_origin(passkeys, base) else {
        return (
            message_page(
                "Add a passkey first",
                &format!(
                    "RustCode on {machine} signs you in with your passkey. Add one with `rc outside owner add-passkey` on that machine, or in RustCode › Settings › Outside agents, then come back."
                ),
            ),
            super::outside_signin::PAGE_HEADERS.to_owned(),
        );
    };
    let body = format!(
        r#"<div class="pair"><div class="avatar"><img src="/logo.png" alt="RustCode"></div></div><p class="eyebrow">RustCode on {machine}</p><h1>Sign in</h1><p class="sub">Use your passkey to open RustCode on {machine} in this browser. It stays signed in for {SESSION_DAYS} days, until you sign out or remove it in Settings › Outside agents.</p><form method="post" action="{LOGIN_PATH}">{controls}</form>"#,
        machine = escape(machine),
        controls = prompt.controls_labeled(
            "Sign in with passkey",
            "The passkey did not answer. Try again."
        ),
    );
    (
        page("Sign in to RustCode", "", &body),
        super::outside_passkey::script_headers(&prompt.nonce),
    )
}

// ----------------------------------------------------------- the web build

/// Where the production web build is: `$RUSTCODE_WEB_DIR`, else
/// `<config>/web`, else `share/rustcode/web` next to the binary's `bin/`.
/// Only a directory with an `index.html` counts.
pub fn web_dir() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("RUSTCODE_WEB_DIR").map(PathBuf::from),
        crate::config::get_config_dir().map(|dir| dir.join("web")),
        std::env::current_exe().ok().and_then(|exe| {
            exe.parent()?
                .parent()
                .map(|prefix| prefix.join("share/rustcode/web"))
        }),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|dir| dir.join("index.html").is_file())
}

/// One file of the web build for a request path, with its content type and
/// whether it may be cached for good (Vite's hashed `assets/`). A path with
/// no extension is a route in the app, answered with `index.html`. Hidden
/// names, `..` and anything outside `dir` are refused.
pub(super) fn asset(dir: &Path, path: &str) -> Option<(Vec<u8>, &'static str, bool)> {
    let relative = path.trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let safe = relative.split('/').all(|segment| {
        !segment.is_empty()
            && !segment.starts_with('.')
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '~'))
    });
    if !safe {
        return None;
    }
    let file = dir.join(relative);
    let (file, relative) = if file.is_file() {
        (file, relative)
    } else if !relative.rsplit('/').next().unwrap_or("").contains('.') {
        (dir.join("index.html"), "index.html")
    } else {
        return None;
    };
    let root = dir.canonicalize().ok()?;
    let real = file.canonicalize().ok()?;
    if !real.starts_with(&root) {
        return None;
    }
    let extension = relative.rsplit('.').next().unwrap_or("");
    let kind = match extension {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    let bytes = std::fs::read(&real).ok()?;
    Some((bytes, kind, relative.starts_with("assets/")))
}

/// The headers the app's own files go out with.
pub(super) fn app_headers(immutable: bool) -> String {
    let cache = if immutable {
        "Cache-Control: public, max-age=31536000, immutable\r\n"
    } else {
        ""
    };
    format!(
        "{cache}X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: same-origin\r\nContent-Security-Policy: frame-ancestors 'none'; base-uri 'none'; object-src 'none'\r\n"
    )
}

/// The `Sec-WebSocket-Accept` answer to a socket's key.
pub(super) fn accept_key(key: &str) -> String {
    tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_check_expire_and_sign_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SESSION_FILE);
        let (id, secret) = create_at(&path, "Safari on macOS", "pk1").unwrap();
        assert_eq!(check_at(&path, &secret).as_deref(), Some(id.as_str()));
        assert_eq!(check_at(&path, "wrong"), None);
        assert_eq!(check_at(&path, ""), None);
        assert!(active_at(&path, &id));
        let listed = list_at(&path);
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].view().to_string().contains(&hash(&secret)));

        // Expired: no longer signed in, and not listed.
        let mut store = read(&path);
        store.sessions[0].expires_at = now() - 1;
        write(&path, &store).unwrap();
        assert_eq!(check_at(&path, &secret), None);
        assert!(!active_at(&path, &id));
        assert!(list_at(&path).is_empty());

        let (_, secret) = create_at(&path, "Firefox on Linux", "pk1").unwrap();
        sign_out_secret_at(&path, &secret);
        assert_eq!(check_at(&path, &secret), None);
    }

    #[test]
    fn sessions_are_removed_by_id_prefix_all_or_passkey() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SESSION_FILE);
        let (a, a_secret) = create_at(&path, "A", "pk1").unwrap();
        let (_, b_secret) = create_at(&path, "B", "pk2").unwrap();
        let removed = remove_at(&path, &a[..6]).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(check_at(&path, &a_secret), None);
        assert!(check_at(&path, &b_secret).is_some());
        assert!(remove_at(&path, "nothing-like-it").is_err());
        forget_passkey_at(&path, "pk2");
        assert_eq!(check_at(&path, &b_secret), None);
        create_at(&path, "C", "pk1").unwrap();
        create_at(&path, "D", "pk1").unwrap();
        assert_eq!(remove_at(&path, "all").unwrap().len(), 2);
    }

    #[test]
    fn sessions_are_capped_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SESSION_FILE);
        let (_, first) = create_at(&path, "first", "pk").unwrap();
        for index in 0..MAX_SESSIONS {
            create_at(&path, &format!("{index}"), "pk").unwrap();
        }
        assert_eq!(list_at(&path).len(), MAX_SESSIONS);
        assert_eq!(check_at(&path, &first), None);
    }

    #[test]
    fn tickets_work_once_for_their_session() {
        let ticket = new_ticket("session-1");
        assert_eq!(spend_ticket("not-a-ticket"), None);
        assert_eq!(spend_ticket(&ticket).as_deref(), Some("session-1"));
        assert_eq!(spend_ticket(&ticket), None);
    }

    #[test]
    fn the_cookie_is_read_from_among_others_and_set_strictly() {
        let mut headers = HashMap::new();
        headers.insert(
            "cookie".to_owned(),
            format!("other=1; {COOKIE}=abc ; last=2"),
        );
        assert_eq!(cookie(&headers), Some("abc"));
        headers.insert("cookie".to_owned(), "rustcode-session=abc".to_owned());
        assert_eq!(cookie(&headers), None);
        let set = set_cookie("abc");
        for part in ["__Host-", "Path=/", "HttpOnly", "Secure", "SameSite=Strict"] {
            assert!(set.contains(part), "{set}");
        }
        assert!(!set.contains("Domain"));
        assert!(clear_cookie().contains("Max-Age=0"));
    }

    #[test]
    fn assets_stay_inside_the_build() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html>app</html>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/index-1a2b.js"), "x").unwrap();
        std::fs::write(dir.path().join(".env"), "secret").unwrap();
        std::fs::write(dir.path().parent().unwrap().join("outside.txt"), "no").ok();

        let (body, kind, immutable) = asset(dir.path(), "/").unwrap();
        assert_eq!(body, b"<html>app</html>");
        assert!(kind.starts_with("text/html"));
        assert!(!immutable);
        let (_, kind, immutable) = asset(dir.path(), "/assets/index-1a2b.js").unwrap();
        assert!(kind.starts_with("text/javascript"));
        assert!(immutable);
        // A route in the app is the app.
        assert_eq!(
            asset(dir.path(), "/settings").unwrap().0,
            b"<html>app</html>"
        );
        for refused in [
            "/.env",
            "/../outside.txt",
            "/assets/../../outside.txt",
            "/assets//index-1a2b.js",
            "/missing.js",
            "/a\\b.js",
            "/%2e%2e/outside.txt",
        ] {
            assert!(asset(dir.path(), refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn browsers_are_named_briefly() {
        assert_eq!(
            browser_name(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"
            ),
            "Safari on macOS"
        );
        assert_eq!(
            browser_name(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36 Edg/130.0"
            ),
            "Edge on Windows"
        );
        assert_eq!(browser_name(""), "A browser");
    }
}
