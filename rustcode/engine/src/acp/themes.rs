//! Themes over ACP (`docs/themes.md`, *ACP*): `_rustcode/themes` lists every
//! theme with its desktop tokens resolved, `_rustcode/themes/set` makes one
//! active, and `_rustcode/themes/changed` tells the client when a theme file
//! or the active theme changed, so the app restyles live.

use crate::themes::{ThemesSnapshot, watch};
use agent_client_protocol::{Client, ConnectionTo};
use std::path::PathBuf;

/// `_rustcode/themes`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/themes", response = serde_json::Value)]
pub(crate) struct ThemesRequest {
    /// The client's project, whose `.rustcode/themes` are listed too.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// `_rustcode/themes/set`: answers with the snapshot.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/themes/set", response = serde_json::Value)]
pub(crate) struct SetThemeRequest {
    pub name: String,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// `_rustcode/themes/changed`: the snapshot, as `_rustcode/themes` answers.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/themes/changed")]
pub(crate) struct ThemesChangedNotification {
    #[serde(flatten)]
    pub snapshot: ThemesSnapshot,
}

fn workspace(cwd: Option<PathBuf>) -> Option<PathBuf> {
    let cwd = cwd.filter(|cwd| cwd.is_absolute());
    watch::set_workspace(cwd.as_deref());
    cwd.or_else(watch::workspace)
}

pub(crate) fn answer(request: ThemesRequest) -> serde_json::Value {
    let workspace = workspace(request.cwd);
    serde_json::to_value(crate::themes::snapshot(workspace.as_deref())).unwrap_or_default()
}

pub(crate) fn answer_set(
    request: SetThemeRequest,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let workspace = workspace(request.cwd);
    crate::themes::set_active(&request.name, workspace.as_deref())
        .map(|snapshot| serde_json::to_value(snapshot).unwrap_or_default())
        .map_err(|message| agent_client_protocol::Error::invalid_params().data(message))
}

/// Start watching themes for this connection's client, once per engine.
pub(crate) fn start_watching(connection: &ConnectionTo<Client>) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let notify = connection.clone();
    let _ = connection.spawn(async move {
        watch::run(|snapshot| {
            let _ = notify.send_notification(ThemesChangedNotification { snapshot });
        })
        .await;
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::JsonRpcMessage;
    use serde_json::json;

    #[test]
    fn requests_use_the_rustcode_extension_methods() {
        assert!(ThemesRequest::matches_method("_rustcode/themes"));
        assert!(SetThemeRequest::matches_method("_rustcode/themes/set"));
        assert!(ThemesChangedNotification::matches_method(
            "_rustcode/themes/changed"
        ));
        let empty: ThemesRequest = serde_json::from_value(json!({})).unwrap();
        assert!(empty.cwd.is_none());
        let set: SetThemeRequest = serde_json::from_value(json!({ "name": "nord" })).unwrap();
        assert_eq!(set.name, "nord");
    }

    #[test]
    fn the_answers_and_the_notification_carry_the_snapshot() {
        let listed = answer(ThemesRequest::default());
        assert_eq!(listed["active"], crate::themes::DEFAULT_THEME_NAME);
        assert!(listed["themes"][0]["tokens"]["accent"].is_string());

        let set = answer_set(SetThemeRequest {
            name: "light".into(),
            cwd: None,
        })
        .unwrap();
        assert_eq!(set["active"], "light");
        assert!(
            answer_set(SetThemeRequest {
                name: "nope".into(),
                cwd: None
            })
            .is_err()
        );

        let snapshot: ThemesSnapshot = serde_json::from_value(set.clone()).unwrap();
        let notification = serde_json::to_value(ThemesChangedNotification { snapshot }).unwrap();
        assert_eq!(notification, set, "the params are the snapshot itself");
    }
}
