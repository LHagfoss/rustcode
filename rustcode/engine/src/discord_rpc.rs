//! Best-effort Discord Rich Presence through the local desktop IPC socket.
//!
//! The IPC client is deliberately isolated on a standard-library worker
//! thread. Discord may be stopped, starting, or disconnected at any time, and
//! none of those cases should stall the TUI or require a Discord login flow.

use crate::app::TokenUsage;
use crate::app::activity::{ActivityKind, ActivitySnapshot};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const DISCORD_CLIENT_ID: &str = "1533154312622964970";
const DISCORD_LARGE_IMAGE: &str = "rustcode_logo";
const DISCORD_LARGE_IMAGE_TEXT: &str = "RustCode — GitHub repository";
const DISCORD_REPOSITORY_BUTTON_LABEL: &str = "Visit repo";
const DISCORD_REPOSITORY_URL: &str = "https://github.com/LHagfoss/rustcode";
const MAX_ACTIVITY_CHARS: usize = 128;
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscordPresence {
    pub(crate) state: String,
    pub(crate) details: String,
}

impl DiscordPresence {
    pub(crate) fn from_activity_with_usage(
        activity: &ActivitySnapshot,
        session_title: &str,
        usage: Option<&TokenUsage>,
    ) -> Self {
        let state = match activity.kind {
            ActivityKind::Ready => "Idle",
            ActivityKind::Queued => "Queued",
            ActivityKind::Working => "Thinking",
            ActivityKind::RunningTool => "Running tools",
            ActivityKind::ActionRequired => "Action required",
        };
        let detail = activity
            .detail
            .as_deref()
            .filter(|detail| !detail.trim().is_empty())
            .map(|detail| format!("{session_title} · {detail}"))
            .unwrap_or_else(|| session_title.to_owned());
        let detail = match format_token_usage(usage) {
            Some(usage) => format!("{detail} · {usage}"),
            None => detail,
        };

        Self {
            state: sanitize(state),
            details: sanitize(if detail.trim().is_empty() {
                "RustCode session"
            } else {
                &detail
            }),
        }
    }
}

/// Return a display-safe workspace identity without ever exposing parent
/// directories. This is intentionally based on the final path component, not
/// on a path with the user's home directory replaced or abbreviated.
pub(crate) fn workspace_basename(path: Option<&Path>) -> String {
    let Some(path) = path else {
        return "workspace".to_owned();
    };

    let raw = path.to_string_lossy();
    let basename = raw
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let basename = sanitize(basename);
    if basename.is_empty() || basename == "." || basename == ".." {
        "workspace".to_owned()
    } else {
        basename
    }
}

fn format_token_usage(usage: Option<&TokenUsage>) -> Option<String> {
    let usage = usage?;
    let mut parts = Vec::with_capacity(2);
    if usage.completion_tokens > 0 {
        parts.push(format!(
            "out {}",
            compact_token_count(usage.completion_tokens)
        ));
    }
    if usage.total_tokens > 0 {
        parts.push(format!("total {}", compact_token_count(usage.total_tokens)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Round usage to checkpoints that are useful in a compact presence. Provider
/// usage can arrive while a response is being rendered; the buckets prevent a
/// Discord update for every small counter change or streaming delta.
fn compact_token_count(tokens: u32) -> String {
    let checkpoint = if tokens < 100 {
        10
    } else if tokens < 1_000 {
        50
    } else if tokens < 10_000 {
        100
    } else if tokens < 1_000_000 {
        1_000
    } else {
        100_000
    };
    let rounded = tokens
        .saturating_add(checkpoint / 2)
        .checked_div(checkpoint)
        .unwrap_or_default()
        .saturating_mul(checkpoint);

    if rounded < 1_000 {
        rounded.to_string()
    } else if rounded < 1_000_000 {
        format_one_decimal(rounded, 1_000, 'k')
    } else {
        format_one_decimal(rounded, 1_000_000, 'm')
    }
}

fn format_one_decimal(value: u32, unit: u32, suffix: char) -> String {
    let whole = value / unit;
    let tenth = value % unit / (unit / 10);
    if tenth == 0 {
        format!("{whole}{suffix}")
    } else {
        format!("{whole}.{tenth}{suffix}")
    }
}

fn sanitize(value: &str) -> String {
    let compact = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut value = compact.chars().take(MAX_ACTIVITY_CHARS).collect::<String>();
    if compact.chars().count() > MAX_ACTIVITY_CHARS {
        value.push('…');
    }
    value
}

fn activity_payload<'a>(presence: &'a DiscordPresence, start_time: u64) -> activity::Activity<'a> {
    activity::Activity::new()
        .state(&presence.state)
        .details(&presence.details)
        .assets(
            activity::Assets::new()
                .large_image(DISCORD_LARGE_IMAGE)
                .large_text(DISCORD_LARGE_IMAGE_TEXT),
        )
        .buttons(vec![activity::Button::new(
            DISCORD_REPOSITORY_BUTTON_LABEL,
            DISCORD_REPOSITORY_URL,
        )])
        .timestamps(activity::Timestamps::new().start(start_time as i64))
}

/// Candidate Unix IPC sockets used by Discord's desktop client. This is only
/// a read-only status probe; actual connection remains delegated to the RPC
/// crate. Windows uses named pipes, so there is no filesystem probe there.
pub(crate) fn ipc_socket_candidates() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let mut roots = Vec::new();
        for key in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
            if let Some(value) = std::env::var_os(key) {
                let root = PathBuf::from(value);
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        if !roots.iter().any(|root| root == "/tmp") {
            roots.push(PathBuf::from("/tmp"));
        }
        roots
            .into_iter()
            .flat_map(|root| (0..10).map(move |index| root.join(format!("discord-ipc-{index}"))))
            .collect()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

pub(crate) fn ipc_socket_detected() -> bool {
    ipc_socket_detected_in(&ipc_socket_candidates())
}

fn ipc_socket_detected_in(candidates: &[PathBuf]) -> bool {
    candidates.iter().any(|path| path.exists())
}

/// The synchronous IPC implementation, kept private to the worker thread.
struct DiscordRpcHandler {
    client: Option<DiscordIpcClient>,
    start_time: u64,
    enabled: bool,
}

impl DiscordRpcHandler {
    fn new() -> Self {
        Self {
            client: None,
            start_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
            enabled: false,
        }
    }

    fn connect(&mut self) -> bool {
        if !self.enabled || self.client.is_some() {
            return self.client.is_some();
        }
        let Ok(mut client) = DiscordIpcClient::new(DISCORD_CLIENT_ID) else {
            return false;
        };
        if client.connect().is_ok() {
            self.client = Some(client);
            true
        } else {
            false
        }
    }

    fn set_activity(&mut self, presence: &DiscordPresence) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.connect() {
            return false;
        }
        let Some(client) = &mut self.client else {
            return false;
        };
        let payload = activity_payload(presence, self.start_time);
        if client.set_activity(payload).is_ok() {
            true
        } else {
            self.disconnect();
            false
        }
    }

    fn clear_activity(&mut self) {
        if let Some(client) = &mut self.client {
            let _ = client.clear_activity();
        }
    }

    fn disconnect(&mut self) {
        if let Some(mut client) = self.client.take() {
            let _ = client.close();
        }
    }

    fn shutdown(&mut self) {
        self.clear_activity();
        self.enabled = false;
        self.disconnect();
    }
}

impl Drop for DiscordRpcHandler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum Command {
    Update(DiscordPresence),
    Shutdown,
}

/// Non-blocking handle used by the TUI. Presence updates are deduplicated
/// before they reach the worker, so streaming frames do not spam Discord IPC.
pub(crate) struct DiscordRpcWorker {
    sender: Sender<Command>,
    last_presence: Arc<Mutex<Option<DiscordPresence>>>,
    thread: Option<JoinHandle<()>>,
}

impl DiscordRpcWorker {
    pub(crate) fn new(enabled: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("rustcode-discord-rpc".to_owned())
            .spawn(move || run_worker(receiver, enabled))
            .expect("Discord RPC worker thread should start");
        Self {
            sender,
            last_presence: Arc::new(Mutex::new(None)),
            thread: Some(thread),
        }
    }

    pub(crate) fn update(&self, presence: DiscordPresence) {
        let Ok(mut last_presence) = self.last_presence.lock() else {
            return;
        };
        if last_presence.as_ref() == Some(&presence) {
            return;
        }
        *last_presence = Some(presence.clone());
        let _ = self.sender.send(Command::Update(presence));
    }

    pub(crate) fn shutdown(mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for DiscordRpcWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_worker(receiver: mpsc::Receiver<Command>, initially_enabled: bool) {
    let mut handler = DiscordRpcHandler::new();
    let enabled = initially_enabled;
    handler.enabled = initially_enabled;
    let mut desired = None;
    let mut retry_at = Instant::now();
    let mut retry_delay = INITIAL_RETRY_DELAY;

    loop {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(Command::Update(presence)) => desired = Some(presence),
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        if !enabled {
            continue;
        }
        let Some(presence) = desired.as_ref() else {
            continue;
        };
        if Instant::now() < retry_at {
            continue;
        }
        if handler.set_activity(presence) {
            retry_delay = INITIAL_RETRY_DELAY;
        } else {
            retry_at = Instant::now() + retry_delay;
            retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(kind: ActivityKind, detail: Option<&str>) -> ActivitySnapshot {
        ActivitySnapshot {
            kind,
            label: "test".to_owned(),
            detail: detail.map(str::to_owned),
            animated: false,
        }
    }

    #[test]
    fn activity_mapping_includes_session_title_and_state() {
        let presence = DiscordPresence::from_activity_with_usage(
            &snapshot(ActivityKind::RunningTool, Some("run_command")),
            "Fix parser",
            None,
        );
        assert_eq!(presence.state, "Running tools");
        assert_eq!(presence.details, "Fix parser · run_command");
    }

    #[test]
    fn activity_mapping_sanitizes_and_bounds_titles() {
        let presence = DiscordPresence::from_activity_with_usage(
            &snapshot(ActivityKind::Ready, None),
            &format!("bad\n{}", "x".repeat(200)),
            None,
        );
        assert!(!presence.details.contains('\n'));
        assert!(presence.details.chars().count() <= MAX_ACTIVITY_CHARS + 1);
    }

    #[test]
    fn activity_payload_contains_repository_button_and_safe_asset_metadata() {
        let presence = DiscordPresence {
            state: "Thinking".to_owned(),
            details: "rustcode · out 1.2k".to_owned(),
        };
        let payload = serde_json::to_value(activity_payload(&presence, 42))
            .expect("Discord activity should serialize");

        assert_eq!(payload["state"], "Thinking");
        assert_eq!(payload["details"], "rustcode · out 1.2k");
        assert_eq!(payload["assets"]["large_image"], "rustcode_logo");
        assert_eq!(
            payload["assets"]["large_text"],
            "RustCode — GitHub repository"
        );
        assert_eq!(payload["timestamps"]["start"], 42);
        assert_eq!(payload["buttons"][0]["label"], "Visit repo");
        assert_eq!(
            payload["buttons"][0]["url"],
            "https://github.com/LHagfoss/rustcode"
        );
        assert!(!payload.to_string().contains("/Users/"));
        assert!(!payload.to_string().contains("token"));
    }

    #[test]
    fn workspace_fallback_uses_only_a_sanitized_basename() {
        assert_eq!(
            workspace_basename(Some(Path::new("/Users/alice/private/rustcode"))),
            "rustcode"
        );
        assert_eq!(
            workspace_basename(Some(Path::new(r"C:\Users\alice\private\rustcode"))),
            "rustcode"
        );
        assert_eq!(
            workspace_basename(Some(Path::new("/tmp/repo\nwith-control"))),
            "repo with-control"
        );
        assert!(
            !workspace_basename(Some(Path::new("/Users/alice/private/rustcode")))
                .contains("/Users/alice")
        );
    }

    #[test]
    fn token_usage_is_compact_and_checkpointed() {
        let usage = TokenUsage {
            completion_tokens: 1_234,
            total_tokens: 12_345,
            ..Default::default()
        };
        assert_eq!(
            format_token_usage(Some(&usage)).as_deref(),
            Some("out 1.2k · total 12k")
        );

        let nearby = TokenUsage {
            completion_tokens: 1_249,
            total_tokens: 12_399,
            ..usage
        };
        assert_eq!(
            format_token_usage(Some(&nearby)),
            format_token_usage(Some(&usage))
        );

        let presence = DiscordPresence::from_activity_with_usage(
            &snapshot(ActivityKind::Working, None),
            "rustcode",
            Some(&usage),
        );
        assert_eq!(presence.details, "rustcode · out 1.2k · total 12k");
    }

    #[test]
    fn disabled_worker_accepts_updates_without_connecting() {
        let worker = DiscordRpcWorker::new(false);
        worker.update(DiscordPresence {
            state: "Idle".to_owned(),
            details: "session".to_owned(),
        });
        worker.shutdown();
    }

    #[test]
    fn no_discord_socket_is_a_normal_unavailable_state() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let candidates = (0..10)
            .map(|index| temporary.path().join(format!("discord-ipc-{index}")))
            .collect::<Vec<_>>();
        assert!(!ipc_socket_detected_in(&candidates));
    }

    #[test]
    fn handler_shutdown_without_connection_is_safe() {
        DiscordRpcHandler::new().shutdown();
    }
}
