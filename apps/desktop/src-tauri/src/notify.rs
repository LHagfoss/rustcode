//! System notifications and the Dock badge. The page decides what deserves
//! one (`src/state/notify.ts`); these commands are the whole surface it has:
//! the app, not the webview, owns the permission the system asks for.

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::{NotificationExt, PermissionState};

/// What a click on one of those notifications emits, with the thread's id: the
/// page opens that thread.
pub const OPEN_EVENT: &str = "notify:open";

/// A notification's title and body are cut to what the system shows anyway.
const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 300;

/// Asks whether the app may notify at all. Desktop systems ask the user the
/// first time one is shown, so this does not prompt on its own.
#[tauri::command(async)]
pub fn notify_permission(app: AppHandle) -> Result<bool, String> {
    app.notification()
        .request_permission()
        .map(|state| state == PermissionState::Granted)
        .map_err(|error| error.to_string())
}

/// Shows one notification. A click on it brings the window forward and emits
/// [`OPEN_EVENT`] with the thread it names.
#[tauri::command(async)]
pub fn notify_show(
    app: AppHandle,
    title: String,
    body: String,
    thread: String,
) -> Result<(), String> {
    let title = clipped(&title, MAX_TITLE);
    if title.is_empty() {
        return Err("A notification needs a title.".to_string());
    }
    app.notification()
        .builder()
        .title(title)
        .body(clipped(&body, MAX_BODY))
        .extra("thread", Value::String(thread))
        .show()
        .map_err(|error| error.to_string())
}

/// The Dock badge: how many threads need the user. 0 removes it.
#[tauri::command(async)]
pub fn notify_badge(window: tauri::WebviewWindow, count: i64) -> Result<(), String> {
    let count = count.clamp(0, 9_999);
    window
        .set_badge_count((count > 0).then_some(count))
        .map_err(|error| error.to_string())
}

/// Arms the click handler, once, before any notification is shown: the plugin
/// reports a click only while something listens. On macOS and Windows a shown
/// notification then keeps a thread of its own until the user acts on it.
pub fn register(handle: &AppHandle) {
    let app = handle.clone();
    let handler = move |action: &tauri_plugin_notification::ActionPerformed| {
        if action.action_id() != "tap" {
            return;
        }
        let thread = action
            .notification()
            .and_then(|shown| shown.extra().get("thread"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
        if !thread.is_empty() {
            let _ = app.emit(OPEN_EVENT, thread);
        }
    };
    if let Err(error) = handle.notification().on_action(handler) {
        eprintln!("notification clicks are unavailable: {error}");
    }
}

/// A title or body cut to `max` characters, at a word where it can.
fn clipped(text: &str, max: usize) -> String {
    let line = text.trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let head: String = line.chars().take(max - 1).collect();
    let cut = head
        .rsplit_once(' ')
        .map(|(words, _)| words)
        .unwrap_or(&head);
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::clipped;

    #[test]
    fn a_short_line_is_kept_as_it_is() {
        assert_eq!(clipped("  Finished  ", 20), "Finished");
    }

    #[test]
    fn a_long_line_is_cut_at_a_word() {
        assert_eq!(
            clipped("Needs your approval before the command runs", 20),
            "Needs your…"
        );
    }

    #[test]
    fn one_long_word_is_cut_where_it_has_to_be() {
        assert_eq!(clipped("averylongwordthatdoesnotfit", 8), "averylo…");
    }
}
