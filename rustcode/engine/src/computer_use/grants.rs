//! Per-thread site grants for the browser tools (#4367).
//!
//! "Allow <host> for this thread" on a browser card lets later reads and
//! screenshots on that origin, and opening pages there, run without a card.
//! Clicks and typing still ask unless `computer_use.grant_covers_actions` is
//! on. Grants live in memory only: they end when the thread closes, the user
//! revokes them, or the engine restarts.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Mutex, OnceLock},
};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// The option id of the thread-wide site grant on a browser card.
pub(super) const SITE_OPTION: &str = "allow-site";

/// The prefix of a browser grant's id in `_rustcode/permissions`.
pub const ID_PREFIX: &str = "browser:";

#[derive(Default)]
struct Thread {
    origins: BTreeSet<String>,
    revoked: CancellationToken,
}

fn all() -> std::sync::MutexGuard<'static, HashMap<String, Thread>> {
    static ALL: OnceLock<Mutex<HashMap<String, Thread>>> = OnceLock::new();
    ALL.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The origin (`scheme://host[:port]`) a grant covers, for an HTTP(S) URL.
pub(super) fn origin(address: &str) -> Option<String> {
    let url = url::Url::parse(address).ok()?;
    matches!(url.scheme(), "https" | "http")
        .then(|| url.origin().ascii_serialization())
        .filter(|origin| origin != "null")
}

/// The host a grant's card names: "staging.example.com".
pub(super) fn host(origin: &str) -> Option<String> {
    url::Url::parse(origin).ok()?.host_str().map(str::to_owned)
}

/// Give a thread an origin.
pub(super) fn grant(session_id: &str, origin: &str) {
    all()
        .entry(session_id.to_owned())
        .or_default()
        .origins
        .insert(origin.to_owned());
}

/// Whether `session_id` has an active site grant for `origin`.
pub(super) fn has_origin(session_id: &str, origin: &str) -> bool {
    all()
        .get(session_id)
        .is_some_and(|thread| thread.origins.contains(origin))
}

/// Whether `session_id` has an active site grant covering `host`.
pub(super) fn has_host(session_id: &str, target_host: &str) -> bool {
    let target = target_host.to_ascii_lowercase();
    all().get(session_id).is_some_and(|thread| {
        thread
            .origins
            .iter()
            .filter_map(|origin| host(origin))
            .any(|granted| granted.to_ascii_lowercase() == target)
    })
}

/// Whether a call on `origin` runs without a card under the thread's grant.
/// Clicks and typing need `actions_too`; an unknown origin never does.
pub(super) fn skips_card(
    session_id: &str,
    tool: &str,
    origin: Option<&str>,
    actions_too: bool,
) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    let reads = matches!(tool, "browser_open" | "browser_read" | "browser_screenshot");
    (reads || actions_too)
        && all()
            .get(session_id)
            .is_some_and(|thread| thread.origins.contains(origin))
}

/// Cancelled when the thread's grants are revoked, so a card still waiting
/// for an answer ends then.
pub(super) fn revoked(session_id: &str) -> CancellationToken {
    all()
        .entry(session_id.to_owned())
        .or_default()
        .revoked
        .clone()
}

/// Drop every grant of a thread and end the cards it is waiting on.
pub(super) fn revoke(session_id: &str) {
    if let Some(thread) = all().remove(session_id) {
        thread.revoked.cancel();
    }
}

/// The threads holding a grant, for `_rustcode/permissions`.
pub fn snapshot() -> Vec<Value> {
    let mut rows: Vec<Value> = all()
        .iter()
        .filter(|(_, thread)| !thread.origins.is_empty())
        .map(|(session_id, thread)| {
            json!({
                "id": format!("{ID_PREFIX}{session_id}"),
                "sessionId": session_id,
                "origins": thread.origins,
                "hosts": thread.origins.iter().filter_map(|o| host(o)).collect::<Vec<_>>(),
            })
        })
        .collect();
    rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_keep_scheme_host_and_port_only() {
        assert_eq!(
            origin("https://Staging.Example.com/login?x=1").as_deref(),
            Some("https://staging.example.com")
        );
        assert_eq!(
            origin("http://localhost:8080/a").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(origin("file:///etc/passwd"), None);
        assert_eq!(origin("not a url"), None);
        assert_eq!(
            host("https://staging.example.com").as_deref(),
            Some("staging.example.com")
        );
    }

    #[test]
    fn a_grant_covers_reads_on_its_origin_in_its_thread_only() {
        let thread = "grant-test-reads";
        let site = "https://staging.example.com";
        assert!(!skips_card(thread, "browser_read", Some(site), false));
        grant(thread, site);
        assert!(skips_card(thread, "browser_read", Some(site), false));
        assert!(skips_card(thread, "browser_screenshot", Some(site), false));
        assert!(skips_card(thread, "browser_open", Some(site), false));
        assert!(!skips_card(thread, "browser_click", Some(site), false));
        assert!(!skips_card(thread, "browser_type", Some(site), false));
        assert!(skips_card(thread, "browser_click", Some(site), true));
        assert!(!skips_card(
            thread,
            "browser_read",
            Some("https://example.com"),
            false
        ));
        assert!(!skips_card(
            thread,
            "browser_read",
            Some("http://staging.example.com"),
            false
        ));
        assert!(!skips_card(thread, "browser_read", None, true));
        assert!(!skips_card(
            "grant-test-other",
            "browser_read",
            Some(site),
            true
        ));
        revoke(thread);
    }

    #[test]
    fn revoking_ends_the_grant_and_the_cards_waiting_on_it() {
        let thread = "grant-test-revoke";
        let site = "https://staging.example.com";
        let waiting = revoked(thread);
        grant(thread, site);
        assert!(
            snapshot()
                .iter()
                .any(|row| row["id"] == format!("{ID_PREFIX}{thread}")
                    && row["hosts"][0] == "staging.example.com")
        );
        revoke(thread);
        assert!(waiting.is_cancelled());
        assert!(!skips_card(thread, "browser_read", Some(site), true));
        assert!(!snapshot().iter().any(|row| row["sessionId"] == thread));
        assert!(!revoked(thread).is_cancelled());
        revoke(thread);
    }
}
