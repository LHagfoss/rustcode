//! `_rustcode/remote/revoke_machine`: end every pairing another computer has
//! with this one. Any paired computer may ask, so that when a computer is lost
//! the owner revokes it on the whole fleet from one place
//! (`docs/remote-security.md` › Credential storage and revocation). A paired
//! computer with ACP access can already run commands here, so this gives it
//! nothing new.

use crate::remote_gateway::lifecycle::RemoteLifecycle;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Revoke the device records of the computer whose gateway id is `host_id`.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/revoke_machine", response = serde_json::Value)]
pub(crate) struct RevokeMachineRequest {
    pub host_id: String,
}

/// Revoke on this computer, answering `{revoked, devices, errors}` or `{error}`.
pub(crate) async fn revoke_machine(request: RevokeMachineRequest) -> Value {
    let Some(dir) = crate::config::get_config_dir() else {
        return json!({"error": "there is no configuration directory"});
    };
    revoke_on(
        &RemoteLifecycle::new(&dir),
        &crate::fleet::own_id(&dir),
        &request.host_id,
    )
    .await
}

async fn revoke_on(lifecycle: &RemoteLifecycle, own_id: &str, host_id: &str) -> Value {
    let host_id = host_id.trim();
    if host_id.is_empty() {
        return json!({"error": "name the machine to revoke by its hostId"});
    }
    if host_id == own_id {
        return json!({"error": "a machine cannot revoke itself"});
    }
    match lifecycle.revoke_host(host_id).await {
        Ok((revoked, errors)) => {
            json!({"revoked": revoked.len(), "devices": revoked, "errors": errors})
        }
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_gateway::devices::DeviceProfile;

    const LOST: &str = "lost-laptop-0123456789";
    const KEPT: &str = "kept-desktop-0123456789";

    fn host_with_devices() -> (tempfile::TempDir, RemoteLifecycle) {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let store = lifecycle.device_store();
        let mut devices = store.load().unwrap();
        for (name, host) in [
            ("Laptop", Some(LOST)),
            ("Desktop", Some(KEPT)),
            ("Phone", None),
        ] {
            let profile = DeviceProfile::from_wire(Some("desktop"), Some("macos"), host);
            devices.enroll_as(name, None, &profile).unwrap();
        }
        store.save(&devices).unwrap();
        (dir, lifecycle)
    }

    fn names(lifecycle: &RemoteLifecycle) -> Vec<String> {
        let mut names: Vec<String> = lifecycle
            .devices()
            .unwrap()
            .into_iter()
            .map(|device| device.name)
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn only_the_named_computer_loses_its_pairing() {
        let (_dir, lifecycle) = host_with_devices();
        let answer = revoke_on(&lifecycle, "this-host-0123456789", LOST).await;
        assert_eq!(answer["revoked"], 1, "{answer}");
        assert_eq!(answer["devices"], json!(["Laptop"]));
        assert_eq!(names(&lifecycle), ["Desktop", "Phone"]);
        let again = revoke_on(&lifecycle, "this-host-0123456789", LOST).await;
        assert_eq!(again["revoked"], 0, "{again}");
    }

    #[tokio::test]
    async fn a_machine_is_never_asked_to_revoke_itself_or_nothing() {
        let (_dir, lifecycle) = host_with_devices();
        assert!(revoke_on(&lifecycle, KEPT, KEPT).await["error"].is_string());
        assert!(revoke_on(&lifecycle, KEPT, "  ").await["error"].is_string());
        assert_eq!(names(&lifecycle).len(), 3);
    }

    #[test]
    fn the_request_reads_what_the_app_sends() {
        let request: RevokeMachineRequest =
            serde_json::from_str(r#"{"hostId":"lost-laptop-0123456789"}"#).unwrap();
        assert_eq!(request.host_id, LOST);
    }
}
