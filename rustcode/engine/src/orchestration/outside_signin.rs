//! The public sign-in a hosted agent (ChatGPT, Claude) opens: the consent
//! page, and the sign-ins waiting for the owner's answer in the app.
//!
//! The page offers two ways to approve. "Approve in RustCode" leaves a
//! sign-in waiting here; every app connected to this engine shows it as a
//! card with the same two match words the page shows, and the page reloads
//! itself until the owner answers. A sign-in code from the app or
//! `rustcode outside code` approves on the spot. Nothing here runs script: the
//! page is plain HTML and CSS under a strict Content-Security-Policy.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::outside::{self, Scope};

/// How long a sign-in waits for the owner before it is given up.
pub const WAIT: Duration = Duration::from_secs(300);
/// Sign-ins waiting at once; another start waits for one to end.
const MAX_WAITING: usize = 2;
/// Sign-ins started in [`START_WINDOW`] before the page stops offering more.
const MAX_STARTS: usize = 10;
const START_WINDOW: Duration = Duration::from_secs(600);
/// How often the waiting page reloads.
pub const REFRESH_SECS: u32 = 2;

/// Short, distinct words: two of them make the match the owner compares.
const WORDS: &[&str] = &[
    "amber", "anchor", "apple", "arrow", "aspen", "badger", "basil", "beacon", "birch", "bison",
    "breeze", "brook", "cactus", "canyon", "cedar", "cello", "cherry", "cobalt", "comet", "coral",
    "cotton", "crane", "delta", "dune", "ember", "falcon", "fern", "fjord", "flint", "fox",
    "garnet", "glacier", "harbor", "hazel", "heron", "indigo", "iris", "jade", "juniper", "kayak",
    "kelp", "lagoon", "lantern", "lemon", "lilac", "lotus", "lynx", "maple", "meadow", "mint",
    "moss", "nectar", "nova", "oak", "olive", "onyx", "orbit", "otter", "pebble", "pepper", "pine",
    "plum", "polar", "quartz", "quill", "raven", "reef", "river", "robin", "saffron", "sage",
    "salmon", "sierra", "slate", "sparrow", "spruce", "summit", "tango", "thistle", "tide",
    "topaz", "tulip", "tundra", "velvet", "violet", "walnut", "willow", "zephyr",
];

/// What became of a sign-in the owner was asked about.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Outcome {
    Waiting,
    Approved { scope: Scope, ask: Option<bool> },
    Declined,
}

struct SignIn {
    id: String,
    /// Hash of the secret only the page that started it holds.
    key: String,
    client_name: String,
    destination: String,
    /// What the page asked for; the owner may lower or raise it.
    scope: Scope,
    words: String,
    asked_at: String,
    started: Instant,
    /// The `/authorize` request, to issue its code once approved.
    query: HashMap<String, String>,
    outcome: Outcome,
}

#[derive(Default)]
struct Store {
    sign_ins: Vec<SignIn>,
    starts: Vec<Instant>,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

fn store() -> std::sync::MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|error| error.into_inner())
}

fn hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn words() -> String {
    let pick = || WORDS[rand::random::<u32>() as usize % WORDS.len()];
    let first = pick();
    let mut second = pick();
    while second == first {
        second = pick();
    }
    format!("{first} {second}")
}

/// A sign-in that waits for the owner; `(id, key, words)`. Refused while too
/// many wait or too many started lately, so the page cannot flood the app.
pub(super) fn start(
    client_name: &str,
    destination: &str,
    scope: Scope,
    query: HashMap<String, String>,
) -> Result<(String, String, String), &'static str> {
    let mut store = store();
    store.sign_ins.retain(|s| s.started.elapsed() < WAIT);
    store.starts.retain(|at| at.elapsed() < START_WINDOW);
    if store.starts.len() >= MAX_STARTS {
        return Err("Too many sign-ins were started. Wait a few minutes, or use a sign-in code.");
    }
    let waiting = store
        .sign_ins
        .iter()
        .filter(|s| s.outcome == Outcome::Waiting)
        .count();
    if waiting >= MAX_WAITING {
        return Err(
            "Another sign-in is already waiting in RustCode. Answer it first, or use a sign-in code.",
        );
    }
    let id = format!("{:032x}", rand::random::<u128>());
    let key = format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    );
    let words = words();
    store.starts.push(Instant::now());
    store.sign_ins.push(SignIn {
        id: id.clone(),
        key: hash(&key),
        client_name: client_name.chars().take(60).collect(),
        destination: destination.chars().take(120).collect(),
        scope,
        words: words.clone(),
        asked_at: chrono::Utc::now().to_rfc3339(),
        started: Instant::now(),
        query,
        outcome: Outcome::Waiting,
    });
    drop(store);
    outside::changed();
    Ok((id, key, words))
}

/// What the page that started `id` sees: the outcome, the words and the
/// request to answer. An answered sign-in is spent by this call.
pub(super) fn poll(
    id: &str,
    key: &str,
) -> Option<(Outcome, String, HashMap<String, String>, String)> {
    let mut store = store();
    let index = store.sign_ins.iter().position(|s| {
        s.id == id && outside::same(&s.key, &hash(key)) && s.started.elapsed() < WAIT
    })?;
    let sign_in = &store.sign_ins[index];
    let found = (
        sign_in.outcome.clone(),
        sign_in.words.clone(),
        sign_in.query.clone(),
        sign_in.client_name.clone(),
    );
    if sign_in.outcome != Outcome::Waiting {
        store.sign_ins.remove(index);
    }
    Some(found)
}

/// The page gave up on `id` (its Cancel link).
pub(super) fn cancel(id: &str, key: &str) {
    let mut store = store();
    let before = store.sign_ins.len();
    store
        .sign_ins
        .retain(|s| !(s.id == id && outside::same(&s.key, &hash(key))));
    let gone = store.sign_ins.len() != before;
    drop(store);
    if gone {
        outside::changed();
    }
}

/// What the app shows for each sign-in waiting for the owner.
pub fn view() -> Vec<Value> {
    let mut store = store();
    store.sign_ins.retain(|s| s.started.elapsed() < WAIT);
    store
        .sign_ins
        .iter()
        .filter(|s| s.outcome == Outcome::Waiting)
        .map(|s| {
            json!({
                "id": s.id,
                "clientName": s.client_name,
                "destination": s.destination,
                "scope": s.scope.as_str(),
                "words": s.words,
                "askedAt": s.asked_at,
                "expiresIn": WAIT.saturating_sub(s.started.elapsed()).as_secs(),
            })
        })
        .collect()
}

/// The owner's answer: approve at `scope` (the page's request when `None`),
/// or decline. Whether that sign-in was still waiting.
pub fn answer(id: &str, allow: bool, scope: Option<Scope>, ask: Option<bool>) -> bool {
    let mut store = store();
    let Some(sign_in) = store
        .sign_ins
        .iter_mut()
        .find(|s| s.id == id && s.outcome == Outcome::Waiting && s.started.elapsed() < WAIT)
    else {
        return false;
    };
    sign_in.outcome = if allow {
        Outcome::Approved {
            scope: scope.unwrap_or(sign_in.scope),
            ask,
        }
    } else {
        Outcome::Declined
    };
    drop(store);
    outside::changed();
    true
}

/// Held by every test that uses the shared store, so they do not interleave.
#[cfg(test)]
pub(super) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(super) fn reset_for_tests() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    *store() = Store::default();
    guard
}

// ------------------------------------------------------------------ pages

pub(super) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The app icon, served at `/logo.png` for the pages.
pub(super) const LOGO: &[u8] =
    include_bytes!("../../../../apps/desktop/src-tauri/icons/128x128.png");

/// Script-free pages: only inline styles and this server's own logo.
///
/// `same-origin`, not `no-referrer`: under `no-referrer` Chromium browsers
/// post the consent form with `Origin: null`, which the server's origin
/// check refuses, so Approve answered `{"error":"forbidden"}`. `same-origin`
/// still sends nothing to another site.
pub(super) const PAGE_HEADERS: &str = "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'\r\nX-Frame-Options: DENY\r\nReferrer-Policy: same-origin\r\n";

/// What a level allows and what it does not, as the page and the app list it.
pub fn scope_points(
    scope: Scope,
) -> (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
) {
    match scope {
        Scope::ReadOnly => (
            "Look, but change nothing",
            &["Read your threads, usage, harnesses and machines"],
            &["Start, message or change anything"],
        ),
        Scope::Delegate => (
            "Start and message threads",
            &[
                "Everything Read-only can",
                "Start threads and send them messages",
                "Threads it starts can edit files, and are declined commands that need approval",
            ],
            &["Merge, clean up, or change settings"],
        ),
        Scope::Manage => (
            "Run work end to end",
            &[
                "Everything Delegate can",
                "Merge, clean up, projects and scheduled work",
                "Threads it starts may run with Full access",
            ],
            &["Change RustCode's own settings"],
        ),
        Scope::Admin => (
            "Everything, settings included",
            &["Everything Manage can", "Change RustCode's own settings"],
            &[],
        ),
    }
}

const SCOPES: [Scope; 4] = [
    Scope::ReadOnly,
    Scope::Delegate,
    Scope::Manage,
    Scope::Admin,
];

const STYLE: &str = r#"
:root{color-scheme:light dark;--bg:#ffffff;--surface:#eaeaea;--text:#171717;--muted:#5c5c5c;--faint:#9a9a9a;--accent:#2f6fe4;--danger:#d93d3d;--success:#1f9d55;--warning:#b7791f;--line:#0000000f}
@media (prefers-color-scheme:dark){:root{--bg:#0b0b0c;--surface:#1d1d20;--text:#ededed;--muted:#a1a1a1;--faint:#6b6b6b;--accent:#93a3e8;--danger:#f26d6d;--success:#4ec27a;--warning:#e5a54b;--line:#ffffff0f}}
:root{--fill-subtle:color-mix(in srgb,var(--text) 4.5%,transparent);--fill:color-mix(in srgb,var(--text) 8%,transparent);--fill-strong:color-mix(in srgb,var(--text) 13%,transparent)}
*{box-sizing:border-box}
body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:var(--bg);color:var(--text);font:14px/1.4 Inter,ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;padding:24px;-webkit-font-smoothing:antialiased}
.card{width:100%;max-width:440px;background:var(--surface);border-radius:20px;padding:24px}
.pair{display:flex;align-items:center;justify-content:center;gap:12px;margin-bottom:16px}
.avatar{width:48px;height:48px;border-radius:12px;display:flex;align-items:center;justify-content:center;font-weight:600;font-size:20px;color:#fff;flex:none}
.avatar img{width:48px;height:48px;border-radius:12px}
.dots{display:flex;gap:4px}.dots i{width:4px;height:4px;border-radius:50%;background:var(--faint)}
.wait .dots i{animation:pulse 1.2s infinite ease-in-out}.wait .dots i:nth-child(2){animation-delay:.2s}.wait .dots i:nth-child(3){animation-delay:.4s}
@media (prefers-reduced-motion:reduce){.wait .dots i{animation:none}}
@keyframes pulse{0%,100%{opacity:.25}50%{opacity:1}}
.eyebrow{text-align:center;color:var(--faint);font-size:12px;margin:0}
h1{text-align:center;font-size:18px;line-height:23px;font-weight:600;margin:4px 0}
.sub{text-align:center;color:var(--muted);margin:0 0 24px;font-size:13px}
.sub b{color:var(--text);font-weight:500}
.label{font-size:12px;font-weight:500;color:var(--muted);margin:0 0 8px}
.opts{background:var(--fill-subtle);border-radius:12px;padding:4px}
.opt{display:block;border-radius:8px;padding:8px 12px;cursor:pointer}
.opt+.opt{margin-top:2px}
.opt:hover{background:var(--fill-subtle)}
.opt input{position:absolute;opacity:0;pointer-events:none}
.opt .head{display:flex;align-items:center;gap:8px}
.opt .radio{width:14px;height:14px;border-radius:50%;box-shadow:inset 0 0 0 1.5px var(--faint);flex:none}
.opt .name{font-weight:500}.opt .tag{color:var(--faint);font-size:12px;margin-left:auto;text-align:right}
.opt ul{display:none;list-style:none;margin:6px 0 2px 22px;padding:0;font-size:13px}
.opt li{margin:2px 0;padding-left:18px;position:relative;color:var(--muted)}
.opt li.yes:before{content:"✓";position:absolute;left:0;color:var(--success)}
.opt li.no:before{content:"✕";position:absolute;left:0;color:var(--danger)}
.opt:has(input:checked){background:var(--fill)}
.opt:has(input:checked) .radio{box-shadow:inset 0 0 0 4px var(--accent)}
.opt:has(input:focus-visible){outline:2px solid color-mix(in srgb,var(--accent) 40%,transparent);outline-offset:2px}
.note{background:color-mix(in srgb,var(--warning) 12%,transparent);color:var(--text);border-radius:12px;padding:8px 12px;font-size:13px;margin:12px 0 0}
.note b{font-weight:600}
.error{background:color-mix(in srgb,var(--danger) 12%,transparent);color:var(--text);border-radius:12px;padding:8px 12px;font-size:13px;margin:0 0 16px}
.actions{display:flex;gap:8px;margin-top:16px}
button,.button{font:inherit;font-weight:500;border-radius:999px;padding:8px 16px;border:0;background:var(--fill);color:var(--text);cursor:pointer;text-decoration:none;text-align:center}
button:hover,.button:hover{background:var(--fill-strong)}
button:focus-visible,.button:focus-visible,.coderow input:focus-visible,summary:focus-visible{outline:2px solid color-mix(in srgb,var(--accent) 40%,transparent);outline-offset:2px}
.primary,.primary:hover{flex:1;background:var(--text);color:var(--bg)}
.primary:hover{opacity:.9}
details{margin-top:16px;border-top:1px solid var(--line);padding-top:12px}
summary{cursor:pointer;color:var(--muted);font-size:13px}
.coderow{display:flex;gap:8px;margin-top:12px}
.coderow input{flex:1;min-width:0;font:inherit;font-size:16px;letter-spacing:.14em;text-transform:uppercase;padding:8px 12px;border-radius:10px;border:0;background:var(--fill);color:var(--text)}
.coderow input::placeholder{color:var(--faint)}
.hint{color:var(--faint);font-size:12px;margin:8px 0 0}
.hint code{font:12px ui-monospace,"SF Mono",monospace;background:var(--fill);padding:1px 4px;border-radius:6px}
.words{display:block;text-align:center;font-size:23px;font-weight:600;letter-spacing:.04em;background:var(--fill-subtle);border-radius:12px;padding:12px;margin:8px 0;text-transform:uppercase}
.foot{text-align:center;color:var(--faint);font-size:12px;margin:16px 0 0}
"#;

fn avatar_color(name: &str) -> &'static str {
    const COLORS: [&str; 6] = [
        "#10a37f", "#d97757", "#4f46e5", "#0ea5e9", "#db2777", "#7c3aed",
    ];
    let lower = name.to_lowercase();
    if lower.contains("chatgpt") || lower.contains("openai") {
        return COLORS[0];
    }
    if lower.contains("claude") || lower.contains("anthropic") {
        return COLORS[1];
    }
    let sum: usize = name.bytes().map(usize::from).sum();
    COLORS[2 + sum % 4]
}

fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

fn pair(name: &str, waiting: bool) -> String {
    format!(
        r#"<div class="pair{wait}"><div class="avatar" style="background:{color}">{initial}</div><div class="dots"><i></i><i></i><i></i></div><div class="avatar"><img src="/logo.png" alt="RustCode"></div></div>"#,
        wait = if waiting { " wait" } else { "" },
        color = avatar_color(name),
        initial = escape(&initial(name)),
    )
}

fn page(title: &str, head: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex">{head}<title>{title}</title><style>{STYLE}</style></head><body><main class="card">{body}</main></body></html>"#,
        title = escape(title),
    )
}

/// Everything the consent page needs to say who asks and where access goes.
pub(super) struct Consent<'a> {
    pub client_name: &'a str,
    /// The callback's host, or `None` for an app on the asking computer.
    pub destination: Option<&'a str>,
    pub machine: &'a str,
    pub host: &'a str,
    pub requested: Option<Scope>,
    /// The `/authorize` parameters, carried as hidden fields.
    pub hidden: &'a [(&'a str, &'a str)],
}

/// The consent page: who asks, a level to choose, and the two ways to approve.
pub(super) fn consent_page(
    c: &Consent<'_>,
    error: Option<&str>,
    selected: Option<Scope>,
) -> String {
    let chosen = selected.or(c.requested).unwrap_or(Scope::ReadOnly);
    let options: String = SCOPES
        .iter()
        .map(|scope| {
            let (tag, yes, no) = scope_points(*scope);
            let items: String = yes
                .iter()
                .map(|t| format!(r#"<li class="yes">{}</li>"#, escape(t)))
                .chain(no.iter().map(|t| format!(r#"<li class="no">{}</li>"#, escape(t))))
                .collect();
            format!(
                r#"<label class="opt"><input type="radio" name="access" value="{value}"{checked}><span class="head"><span class="radio"></span><span class="name">{label}</span><span class="tag">{tag}</span></span><ul>{items}</ul></label>"#,
                value = scope.as_str(),
                checked = if *scope == chosen { " checked" } else { "" },
                label = scope.label(),
                tag = escape(tag),
            )
        })
        .collect();
    let hidden: String = c
        .hidden
        .iter()
        .map(|(k, v)| format!(r#"<input type="hidden" name="{k}" value="{}">"#, escape(v)))
        .collect();
    let note = match c.destination {
        Some(host) => format!(
            "Access goes to <b>{}</b>. Continue only if you started this sign-in in {} just now.",
            escape(host),
            escape(c.client_name)
        ),
        None => "Access goes to an app on the computer that opened this page.".to_owned(),
    };
    let error = error
        .map(|e| format!(r#"<p class="error" role="alert">{}</p>"#, escape(e)))
        .unwrap_or_default();
    let body = format!(
        r#"{pair}<p class="eyebrow">RustCode on {machine}</p><h1>{name} wants to connect</h1><p class="sub">It will use RustCode's tools through <b>{host}</b>.</p>{error}<form method="post" action="/authorize">{hidden}<p class="label">Choose what it can do</p><div class="opts">{options}</div><p class="note">{note}</p><div class="actions"><button name="decision" value="deny" formnovalidate>Cancel</button><button class="primary" name="decision" value="app">Approve in RustCode</button></div><details{open}><summary>Have a sign-in code?</summary><div class="coderow"><input type="text" name="code" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" placeholder="XXXX-XXXX" aria-label="Sign-in code"><button name="decision" value="approve">Use code</button></div><p class="hint">In RustCode open Settings › Outside agents › Get a sign-in code, or run <code>rustcode outside code</code> on {machine}. The code sets the most this app may do.</p></details></form><p class="foot">You can see and revoke it any time in RustCode › Settings › Outside agents.</p>"#,
        pair = pair(c.client_name, false),
        machine = escape(c.machine),
        name = escape(c.client_name),
        host = escape(c.host),
        open = if error.contains("code") { " open" } else { "" },
    );
    page(&format!("Connect {} to RustCode", c.client_name), "", &body)
}

/// The page shown while the owner decides: it reloads itself every
/// [`REFRESH_SECS`], so no script is needed.
pub(super) fn waiting_page(client_name: &str, machine: &str, words: &str, cancel: &str) -> String {
    let body = format!(
        r#"{pair}<p class="eyebrow">RustCode on {machine}</p><h1>Approve in RustCode</h1><p class="sub">Open RustCode on any of your devices. A card asks whether <b>{name}</b> may connect. Check that it shows these words:</p><span class="words">{words}</span><p class="hint" style="text-align:center">This page moves on by itself once you answer. It gives up after five minutes.</p><div class="actions"><a class="button" style="flex:1" href="{cancel}">Cancel</a></div>"#,
        pair = pair(client_name, true),
        machine = escape(machine),
        name = escape(client_name),
        words = escape(words),
        cancel = escape(cancel),
    );
    page(
        "Waiting for RustCode",
        &format!(r#"<meta http-equiv="refresh" content="{REFRESH_SECS}">"#),
        &body,
    )
}

/// A dead end: an expired or unknown sign-in.
pub(super) fn message_page(title: &str, text: &str) -> String {
    let body = format!(
        r#"<div class="pair"><div class="avatar"><img src="/logo.png" alt="RustCode"></div></div><h1>{}</h1><p class="sub">{}</p>"#,
        escape(title),
        escape(text)
    );
    page(title, "", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> HashMap<String, String> {
        HashMap::from([("client_id".to_owned(), "x".to_owned())])
    }

    #[test]
    fn a_sign_in_waits_is_answered_once_and_is_spent_by_the_page() {
        let _serial = reset_for_tests();
        let (id, key, words) = start("ChatGPT", "chatgpt.com", Scope::Delegate, query()).unwrap();
        assert_eq!(words.split(' ').count(), 2);
        assert!(
            poll(&id, "wrong").is_none(),
            "only the page that started it may read it"
        );
        assert_eq!(poll(&id, &key).unwrap().0, Outcome::Waiting);
        let listed = view();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["words"], json!(words));
        assert_eq!(listed[0]["scope"], json!("delegate"));

        assert!(answer(&id, true, Some(Scope::ReadOnly), Some(false)));
        assert!(
            !answer(&id, false, None, None),
            "a second answer changes nothing"
        );
        assert!(view().is_empty(), "an answered sign-in no longer asks");
        assert_eq!(
            poll(&id, &key).unwrap().0,
            Outcome::Approved {
                scope: Scope::ReadOnly,
                ask: Some(false)
            }
        );
        assert!(poll(&id, &key).is_none(), "the approval is spent once");
    }

    #[test]
    fn the_page_cannot_flood_the_app() {
        let _serial = reset_for_tests();
        let a = start("A", "a.example", Scope::ReadOnly, query()).unwrap();
        start("B", "b.example", Scope::ReadOnly, query()).unwrap();
        assert!(start("C", "c.example", Scope::ReadOnly, query()).is_err());
        cancel(&a.0, &a.1);
        assert!(start("C", "c.example", Scope::ReadOnly, query()).is_ok());
        let error = loop {
            let ids: Vec<String> = view()
                .iter()
                .map(|v| v["id"].as_str().unwrap().to_owned())
                .collect();
            for id in ids {
                answer(&id, false, None, None);
            }
            match start("D", "d.example", Scope::ReadOnly, query()) {
                Ok(_) => continue,
                Err(error) => break error,
            }
        };
        assert!(error.contains("Too many"), "{error}");
    }

    #[test]
    fn the_pages_escape_what_the_client_says() {
        let hidden = [("state", "\"><script>")];
        let page = consent_page(
            &Consent {
                client_name: "<b>Evil</b>",
                destination: Some("evil.example"),
                machine: "vps",
                host: "rc.example.com",
                requested: Some(Scope::Manage),
                hidden: &hidden,
            },
            None,
            None,
        );
        assert!(!page.contains("<b>Evil</b>"));
        assert!(!page.contains("\"><script>"));
        assert!(page.contains(r#"value="manage" checked"#));
        assert!(page.contains("evil.example"));
        let waiting = waiting_page(
            "<i>x</i>",
            "vps",
            "amber fox",
            "/authorize/wait?cancel=1&a=<",
        );
        assert!(!waiting.contains("<i>x</i>"));
        assert!(waiting.contains("http-equiv=\"refresh\""));
    }

    #[test]
    fn the_pages_keep_their_origin_on_a_form_post() {
        // Chromium sends `Origin: null` under `no-referrer`, and the server
        // refuses a POST whose origin is not its own.
        assert!(PAGE_HEADERS.contains("Referrer-Policy: same-origin\r\n"));
        assert!(!PAGE_HEADERS.contains("no-referrer"));
    }
}
