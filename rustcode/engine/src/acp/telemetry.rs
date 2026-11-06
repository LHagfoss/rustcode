//! `_rustcode/config/telemetry`: the two payload-capture switches
//! (`docs/data-store.md` › Captured payloads), which Settings turns on instead
//! of the user editing `~/.config/rustcode/config.toml`.
//!
//! Both keys are global only — a checked-out project cannot turn payload
//! capture on — and off is the default. Only the two keys are written, through
//! the same load-edit-save path the other settings use, so the rest of the file
//! is kept as it was.
//!
//! The engine reads them from the config it loaded when it started:
//! `capture_harness_rpc_payloads` when a harness peer starts, the provider flag
//! on each request. A change therefore applies after the app restarts the
//! engine (Settings › Advanced).

use crate::config::AppConfig;
use serde_json::{Value, json};
use std::path::Path;

/// `_rustcode/config/telemetry`: the two switches as they are now.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/config/telemetry", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelemetryRequest {}

/// `_rustcode/config/telemetry/set`: both switches, saved together; answers
/// like `_rustcode/config/telemetry`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/config/telemetry/set", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetTelemetryRequest {
    /// `capture_provider_payloads`: the built-in harness's request bodies.
    pub provider_payloads: bool,
    /// `capture_harness_rpc_payloads`: each message exchanged with a harness.
    pub harness_rpc_payloads: bool,
}

fn describe(config: &AppConfig) -> Value {
    json!({
        "providerPayloads": config.capture_provider_payloads,
        "harnessRpcPayloads": config.capture_harness_rpc_payloads,
    })
}

/// The switches in the user config at `dir`.
pub(crate) fn answer_in(dir: &Path) -> Result<Value, String> {
    let (_, _, config) = crate::config::load_config_from(dir);
    Ok(describe(&config))
}

/// The switches in the user config the engine runs with.
pub(crate) fn answer() -> Result<Value, String> {
    Ok(describe(&crate::config::load_config().2))
}

/// Save both switches in the user config the engine runs with.
pub(crate) fn answer_set(request: SetTelemetryRequest) -> Result<Value, String> {
    let dir = crate::config::get_config_dir()
        .ok_or_else(|| "could not determine the config directory".to_owned())?;
    answer_set_in(&dir, request)
}

/// Save both switches in the user config at `dir`, then answer with what was
/// written. A config that cannot be read is refused rather than replaced with
/// defaults, so a hand-edited file with a typo is not silently lost.
pub(crate) fn answer_set_in(dir: &Path, request: SetTelemetryRequest) -> Result<Value, String> {
    crate::config::edit_user_config_in(dir, |config| {
        config.capture_provider_payloads = request.provider_payloads;
        config.capture_harness_rpc_payloads = request.harness_rpc_payloads;
        Ok(())
    })?;
    answer_in(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::JsonRpcMessage;
    use serde_json::json;
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("rustcode-tests").join(format!(
            "{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set(provider_payloads: bool, harness_rpc_payloads: bool) -> SetTelemetryRequest {
        SetTelemetryRequest {
            provider_payloads,
            harness_rpc_payloads,
        }
    }

    #[test]
    fn requests_use_the_rustcode_config_extension_methods() {
        assert!(TelemetryRequest::matches_method(
            "_rustcode/config/telemetry"
        ));
        assert!(SetTelemetryRequest::matches_method(
            "_rustcode/config/telemetry/set"
        ));
        let _: TelemetryRequest = serde_json::from_value(json!({})).unwrap();
        let update: SetTelemetryRequest =
            serde_json::from_value(json!({"providerPayloads": true, "harnessRpcPayloads": false}))
                .unwrap();
        assert!(update.provider_payloads);
        assert!(!update.harness_rpc_payloads);
    }

    #[test]
    fn both_switches_are_off_until_turned_on_and_round_trip() {
        let dir = temp_dir("telemetry-switches");
        assert_eq!(
            answer_in(&dir).unwrap(),
            json!({"providerPayloads": false, "harnessRpcPayloads": false})
        );

        let saved = answer_set_in(&dir, set(true, false)).unwrap();
        assert_eq!(
            saved,
            json!({"providerPayloads": true, "harnessRpcPayloads": false}),
            "the answer is what was written"
        );
        assert_eq!(
            answer_in(&dir).unwrap(),
            saved,
            "a fresh read sees the same choice"
        );

        let saved = answer_set_in(&dir, set(true, true)).unwrap();
        assert_eq!(
            saved,
            json!({"providerPayloads": true, "harnessRpcPayloads": true})
        );

        // Turning one off leaves the other alone.
        let saved = answer_set_in(&dir, set(false, true)).unwrap();
        assert_eq!(
            saved,
            json!({"providerPayloads": false, "harnessRpcPayloads": true})
        );
        let contents = fs::read_to_string(dir.join(crate::config::CONFIG_TOML_FILE)).unwrap();
        assert!(
            contents.contains("capture_provider_payloads = false"),
            "{contents}"
        );
        assert!(
            contents.contains("capture_harness_rpc_payloads = true"),
            "{contents}"
        );
    }

    #[test]
    fn a_config_that_cannot_be_read_is_refused_rather_than_replaced() {
        let dir = temp_dir("telemetry-broken-config");
        fs::write(
            dir.join(crate::config::CONFIG_TOML_FILE),
            "capture_provider_payloads = ",
        )
        .unwrap();
        let refused = answer_set_in(&dir, set(true, true)).unwrap_err();
        assert!(refused.contains("could not be read"), "{refused}");
        // The typo is still there: nothing was overwritten with defaults.
        assert_eq!(
            fs::read_to_string(dir.join(crate::config::CONFIG_TOML_FILE)).unwrap(),
            "capture_provider_payloads = "
        );
    }
}
