//! Discord Rich Presence for ACP clients that ask for it (#2179), such as the
//! macOS app. The engine publishes through the same local IPC worker as the
//! terminal UI, so presence needs no Discord login and works off the UI
//! thread. Only the project's folder name and whether a turn is running are
//! published: never prompts, chat titles or paths.
//!
//! Other ACP clients (editors) get no presence unless they advertise
//! `_meta.rustcode.presence`, so the engine never publishes for them on its
//! own. `discord_rpc_enabled` is read again at every turn and every
//! `_rustcode/presence` request, so a change in Settings applies at once.

use crate::discord_rpc::{DiscordPresence, DiscordRpcWorker, workspace_basename};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const PRESENCE_CAPABILITY_VERSION: u64 = 1;

/// The client advertised `_meta.rustcode.presence` in `initialize`.
pub(crate) fn client_meta_wants_presence(meta: Option<&Map<String, Value>>) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("presence"))
        .and_then(Value::as_u64)
        .is_some_and(|version| version >= PRESENCE_CAPABILITY_VERSION)
}

/// Whether presence is on, and whether Discord's desktop IPC is visible.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/presence", response = serde_json::Value)]
pub(crate) struct PresenceRequest {
    /// The client's project, whose config may override the global setting.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// Turn presence on or off; answers like `_rustcode/presence`.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/presence/set", response = serde_json::Value)]
pub(crate) struct SetPresenceRequest {
    pub enabled: bool,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

#[derive(Default)]
struct State {
    client_wants_presence: bool,
    /// Turns running per session.
    running: HashMap<String, usize>,
    /// The project of the most recent turn or request.
    project: Option<PathBuf>,
    worker: Option<DiscordRpcWorker>,
}

impl State {
    /// What to publish, or `None` to clear presence.
    fn presence(&self, enabled: bool) -> Option<DiscordPresence> {
        (self.client_wants_presence && enabled).then(|| {
            DiscordPresence::for_project(
                !self.running.is_empty(),
                &workspace_basename(self.project.as_deref()),
            )
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct Presence(Arc<Mutex<State>>);

impl Presence {
    pub(crate) fn set_client(&self, wants_presence: bool) {
        self.with_state(|state| state.client_wants_presence = wants_presence);
    }

    pub(crate) fn turn_started(&self, session_id: &str, cwd: &Path) {
        self.with_state(|state| {
            *state.running.entry(session_id.to_owned()).or_default() += 1;
            state.project = Some(cwd.to_path_buf());
        });
    }

    pub(crate) fn turn_finished(&self, session_id: &str, cwd: &Path) {
        self.with_state(|state| {
            finish_turn(&mut state.running, session_id);
            state.project = Some(cwd.to_path_buf());
        });
    }

    pub(crate) fn session_closed(&self, session_id: &str) {
        self.with_state(|state| {
            state.running.remove(session_id);
        });
    }

    /// Answer `_rustcode/presence`, publishing with the current config.
    pub(crate) fn status(&self, cwd: Option<PathBuf>) -> Value {
        self.with_state(|state| {
            if cwd.is_some() {
                state.project = cwd;
            }
        });
        json!({
            "enabled": enabled_for(self.project().as_deref()),
            "discordDetected": crate::discord_rpc::ipc_socket_detected(),
        })
    }

    /// Answer `_rustcode/presence/set`: save the setting where the project's
    /// config keeps it, else globally, then publish or clear at once.
    pub(crate) fn set_enabled(
        &self,
        request: SetPresenceRequest,
    ) -> Result<Value, agent_client_protocol::Error> {
        let workspace = request.cwd.clone().filter(|cwd| cwd.is_absolute());
        let mut config = match &workspace {
            Some(workspace) => crate::config::load_config_for_workspace(workspace).2,
            None => crate::config::load_config().2,
        };
        config.discord_rpc_enabled = request.enabled;
        crate::config::save_discord_rpc_enabled(&config, workspace.as_deref())
            .map_err(|error| agent_client_protocol::Error::internal_error().data(error))?;
        Ok(self.status(workspace))
    }

    fn project(&self) -> Option<PathBuf> {
        self.0.lock().ok().and_then(|state| state.project.clone())
    }

    /// Change the state, then publish it with the setting as now configured.
    fn with_state<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let Ok(mut state) = self.0.lock() else {
            // A poisoned lock only loses presence, never the turn.
            return change(&mut State::default());
        };
        let result = change(&mut state);
        let enabled = state.client_wants_presence && enabled_for(state.project.as_deref());
        match state.presence(enabled) {
            Some(presence) => {
                let worker = state
                    .worker
                    .get_or_insert_with(|| DiscordRpcWorker::new(true));
                worker.set_enabled(true);
                worker.update(presence);
            }
            None => {
                if let Some(worker) = &state.worker {
                    worker.set_enabled(false);
                }
            }
        }
        result
    }
}

fn finish_turn(running: &mut HashMap<String, usize>, session_id: &str) {
    if let Some(count) = running.get_mut(session_id) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            running.remove(session_id);
        }
    }
}

fn enabled_for(project: Option<&Path>) -> bool {
    match project {
        Some(project) => {
            crate::config::load_config_for_workspace(project)
                .2
                .discord_rpc_enabled
        }
        None => crate::config::load_config().2.discord_rpc_enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::JsonRpcMessage;

    #[test]
    fn only_clients_that_ask_get_presence() {
        let meta = |value: Value| value.as_object().cloned();
        assert!(client_meta_wants_presence(
            meta(json!({"rustcode": {"presence": 1}})).as_ref()
        ));
        assert!(!client_meta_wants_presence(None));
        assert!(!client_meta_wants_presence(
            meta(json!({"rustcode": {"preview": 1}})).as_ref()
        ));

        let state = State {
            project: Some(PathBuf::from("/Users/me/code/app")),
            ..State::default()
        };
        assert_eq!(state.presence(true), None);
    }

    #[test]
    fn presence_shows_the_project_name_and_whether_a_turn_runs() {
        let mut state = State {
            client_wants_presence: true,
            project: Some(PathBuf::from("/Users/me/secret/client-app")),
            ..State::default()
        };
        let idle = state.presence(true).unwrap();
        assert_eq!(
            idle,
            DiscordPresence::for_project(false, "client-app"),
            "the folder name only, never the path"
        );
        state.running.insert("s1".into(), 1);
        assert_eq!(
            state.presence(true),
            Some(DiscordPresence::for_project(true, "client-app"))
        );
        assert_eq!(state.presence(false), None, "the setting turns it off");
    }

    #[test]
    fn a_session_is_idle_once_each_of_its_turns_ends() {
        let mut running = HashMap::from([("s1".to_owned(), 2)]);
        finish_turn(&mut running, "s1");
        assert!(running.contains_key("s1"));
        finish_turn(&mut running, "s1");
        assert!(running.is_empty());
        finish_turn(&mut running, "unknown");
        assert!(running.is_empty());
    }

    #[test]
    fn requests_use_the_rustcode_extension_methods() {
        assert!(PresenceRequest::matches_method("_rustcode/presence"));
        assert!(SetPresenceRequest::matches_method("_rustcode/presence/set"));
        let request: SetPresenceRequest =
            serde_json::from_value(json!({"enabled": false, "cwd": "/tmp/p"})).unwrap();
        assert!(!request.enabled);
        let empty: PresenceRequest = serde_json::from_value(json!({})).unwrap();
        assert!(empty.cwd.is_none());
    }
}
