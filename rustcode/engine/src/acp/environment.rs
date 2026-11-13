//! `_rustcode/environment/sync`: the environment fleet's metadata exchange
//! (`crate::fleet`, `docs/remote.md` › Environment names and the fleet).
//!
//! One request does both halves of a sync. The caller says who it is and what
//! it knows; this computer folds that in (a newer name for itself is adopted,
//! the rest is remembered) and answers with who it is and what it knows. Only
//! names, platforms, versions and when a computer was last seen travel: no
//! address, no token, no access. A computer that is only *known* through a
//! peer stays unpaired until it is paired on its own.

use crate::fleet::{FleetEntry, FleetView};
use chrono::Utc;

/// The sync request. `from` is the caller's own record (its id and name); the
/// entries are what the caller knows about every other computer.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/environment/sync", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnvironmentSyncRequest {
    #[serde(default)]
    pub from: Option<FleetEntry>,
    #[serde(default)]
    pub entries: Vec<FleetEntry>,
}

/// Merge the caller's report and answer with this computer's own.
pub(crate) fn sync(request: EnvironmentSyncRequest) -> Result<serde_json::Value, String> {
    let dir = crate::fleet::config_dir().ok_or("there is no configuration directory")?;
    let from_id = request
        .from
        .as_ref()
        .map(|from| from.host_id.clone())
        .unwrap_or_default();
    let mut incoming = request.entries;
    incoming.extend(request.from);
    let view: FleetView = crate::fleet::sync(&dir, &from_id, &incoming, Utc::now())
        .map_err(|error| format!("{error:#}"))?;
    serde_json::to_value(view).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_reads_what_the_app_sends() {
        let request: EnvironmentSyncRequest = serde_json::from_str(
            r#"{"from":{"hostId":"a","name":"Laptop","platform":"macos"},"entries":[{"hostId":"c","name":"mini","nameUpdatedAt":"2026-11-13T10:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(request.from.unwrap().host_id, "a");
        assert_eq!(request.entries[0].platform, "unknown");
        // Nothing at all is a plain read.
        let empty: EnvironmentSyncRequest = serde_json::from_str("{}").unwrap();
        assert!(empty.from.is_none() && empty.entries.is_empty());
    }
}
