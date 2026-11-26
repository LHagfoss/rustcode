//! How a harness account is billed, read from the sign-in files the harness
//! itself writes. A subscription (Claude Pro/Max, a ChatGPT plan) is not billed
//! per token, so the cost a thread reports on it is what the tokens would cost
//! at API prices, not a bill.

use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) const SUBSCRIPTION: &str = "subscription";
pub(crate) const API: &str = "api";
pub(crate) const UNKNOWN: &str = "unknown";

/// The billing kind of `harness`'s account in `dir`; `own_dir` is the
/// machine's default login (`~/.claude`, `~/.codex`).
pub(crate) fn of(harness: &str, dir: &Path, own_dir: bool) -> &'static str {
    let files = match harness {
        "claude-code" => claude_files(dir, own_dir),
        "codex" => vec![dir.join("auth.json")],
        _ => return UNKNOWN,
    };
    files
        .into_iter()
        .find_map(|path| {
            let json = serde_json::from_slice::<Value>(&std::fs::read(path).ok()?).ok()?;
            Some(from_json(harness, &json))
        })
        .unwrap_or(UNKNOWN)
}

/// The billing kind of `harness`'s default login on this machine.
pub(crate) fn of_default(harness: &str) -> &'static str {
    let Some(home) = crate::home::home_dir() else {
        return UNKNOWN;
    };
    match harness {
        "claude-code" => of(harness, &home.join(".claude"), true),
        "codex" => {
            let dir =
                std::env::var_os("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from);
            of(harness, &dir, true)
        }
        _ => UNKNOWN,
    }
}

fn claude_files(dir: &Path, own_dir: bool) -> Vec<PathBuf> {
    let mut files = vec![dir.join(".claude.json")];
    if own_dir && let Some(home) = crate::home::home_dir() {
        files.insert(0, home.join(".claude.json"));
    }
    files
}

/// The billing kind a harness's sign-in file says.
pub(crate) fn from_json(harness: &str, json: &Value) -> &'static str {
    match harness {
        "claude-code" => match json.get("oauthAccount") {
            Some(account) if account.is_object() => SUBSCRIPTION,
            _ if json.get("primaryApiKey").is_some_and(Value::is_string) => API,
            _ => UNKNOWN,
        },
        "codex" => match json.get("auth_mode").and_then(Value::as_str) {
            Some("chatgpt") => SUBSCRIPTION,
            Some("apikey" | "api_key") => API,
            _ if json.get("tokens").is_some_and(Value::is_object) => SUBSCRIPTION,
            _ if json.get("OPENAI_API_KEY").is_some_and(Value::is_string) => API,
            _ => UNKNOWN,
        },
        _ => UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_oauth_login_is_a_subscription() {
        let file =
            json!({"oauthAccount": {"accountUuid": "a", "billingType": "stripe_subscription"}});
        assert_eq!(from_json("claude-code", &file), SUBSCRIPTION);
    }

    #[test]
    fn claude_api_key_is_api() {
        assert_eq!(
            from_json("claude-code", &json!({"primaryApiKey": "k"})),
            API
        );
        assert_eq!(from_json("claude-code", &json!({})), UNKNOWN);
    }

    #[test]
    fn codex_chatgpt_sign_in_is_a_subscription() {
        let file = json!({"auth_mode": "chatgpt", "OPENAI_API_KEY": null, "tokens": {}});
        assert_eq!(from_json("codex", &file), SUBSCRIPTION);
        assert_eq!(
            from_json("codex", &json!({"tokens": {"id_token": "x"}})),
            SUBSCRIPTION
        );
    }

    #[test]
    fn codex_api_key_is_api() {
        assert_eq!(from_json("codex", &json!({"auth_mode": "apikey"})), API);
        assert_eq!(from_json("codex", &json!({"OPENAI_API_KEY": "sk"})), API);
    }

    #[test]
    fn other_harnesses_are_unknown() {
        assert_eq!(
            from_json("gemini", &json!({"auth_mode": "chatgpt"})),
            UNKNOWN
        );
    }

    #[test]
    fn reads_the_account_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.json"), r#"{"auth_mode":"chatgpt"}"#).unwrap();
        assert_eq!(of("codex", dir.path(), false), SUBSCRIPTION);
        assert_eq!(of("claude-code", dir.path(), false), UNKNOWN);
        std::fs::write(dir.path().join(".claude.json"), r#"{"oauthAccount":{}}"#).unwrap();
        assert_eq!(of("claude-code", dir.path(), false), SUBSCRIPTION);
    }
}
