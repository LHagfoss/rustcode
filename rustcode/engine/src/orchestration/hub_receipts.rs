//! Durable retry receipts for calls the hub delivers from its mailbox (#4398).
//!
//! The hub resends a queued call under the key `hub-<id>` until it hears the
//! result. A member that ran the call and restarted before answering would
//! run it again with only the in-process receipts, so these keys are also
//! kept in the data store, as `start_session` keeps its own.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::data_store::{Receipt, ReceiptRow};

/// The retry-key prefix the hub gives a mailbox delivery.
const HUB_KEY_PREFIX: &str = "hub-";

/// Whether a retry key came from the hub's mailbox and outlives the process.
pub(super) fn durable(key: &str) -> bool {
    key.starts_with(HUB_KEY_PREFIX)
}

fn store() -> Option<PathBuf> {
    crate::data_store::data_path()
}

/// The answer to give instead of running the call: a replay of the earlier
/// one, or a conflict. `None` when the call may run, including when the store
/// cannot be read and the in-process receipts are all there is.
pub(super) fn earlier(caller: &str, key: &str, tool: &str, args: &Value) -> Option<Value> {
    earlier_at(store()?.as_path(), caller, key, tool, args)
}

/// [`earlier`] against one store file, so a test can use a temporary one.
fn earlier_at(path: &Path, caller: &str, key: &str, tool: &str, args: &Value) -> Option<Value> {
    if !durable(key) {
        return None;
    }
    let hash = super::args_hash(tool, args);
    match crate::data_store::check_receipt_at(path, caller, key, tool, &hash) {
        Ok(Receipt::None) => None,
        Ok(Receipt::Replay { result, .. }) => Some(super::local::replay(result)),
        Ok(Receipt::Conflict) => Some(json!({
            "error": {
                "code": "invalid",
                "message": "client_request_id reused with different arguments",
            }
        })),
        Err(error) => {
            crate::dbg_log!("hub receipt {key}: could not read the store: {error}");
            None
        }
    }
}

/// Keep a successful answer under a hub key in the data store.
pub(super) fn record(caller: &str, key: &str, tool: &str, args: &Value, answer: &Value) {
    if let Some(path) = store() {
        record_at(&path, caller, key, tool, args, answer);
    }
}

/// [`record`] against one store file, so a test can use a temporary one.
fn record_at(path: &Path, caller: &str, key: &str, tool: &str, args: &Value, answer: &Value) {
    if !durable(key) {
        return;
    }
    let row = ReceiptRow {
        caller: caller.to_owned(),
        client_request_id: key.to_owned(),
        action: tool.to_owned(),
        args_hash: super::args_hash(tool, args),
        result_json: answer.to_string(),
    };
    if let Err(error) = crate::data_store::record_receipt_at(path, &row) {
        crate::dbg_log!("hub receipt {key}: could not save it: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hub_message_delivered_again_after_a_restart_is_replayed_not_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.sqlite");
        let args = json!({"session_id": "child", "message": "go", "client_request_id": "hub-7"});
        let answer = json!({"session_id": "child", "queued": false});

        assert_eq!(
            earlier_at(&path, "hub", "hub-7", "message_session", &args),
            None
        );
        record_at(&path, "hub", "hub-7", "message_session", &args, &answer);

        let replayed = earlier_at(&path, "hub", "hub-7", "message_session", &args).unwrap();
        assert_eq!(replayed["session_id"], "child");
        assert_eq!(replayed["replayed"], true);

        let other = json!({"session_id": "child", "message": "stop", "client_request_id": "hub-7"});
        let conflict = earlier_at(&path, "hub", "hub-7", "message_session", &other).unwrap();
        assert_eq!(conflict["error"]["code"], "invalid");
    }

    #[test]
    fn only_hub_keys_are_kept_in_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.sqlite");
        let args = json!({"session_id": "child", "message": "go"});
        record_at(
            &path,
            "a",
            "mine-1",
            "message_session",
            &args,
            &json!({"ok": true}),
        );
        assert_eq!(
            earlier_at(&path, "a", "mine-1", "message_session", &args),
            None
        );
        assert!(durable("hub-1"));
        assert!(!durable("mine-1"));
    }
}
