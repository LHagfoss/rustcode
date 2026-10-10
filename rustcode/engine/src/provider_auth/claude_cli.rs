//! Claude subscription access through the locally installed Claude Code CLI.
//!
//! RustCode never reads, copies, or stores the CLI's credential. Sign-in state
//! stays inside the CLI; this module only asks it who is signed in and which
//! models that account offers. Requests are served by a `claude` child process
//! (see `network::claude_cli`).

use super::{AccountStatus, AuthCommandResult, AuthMethod, CredentialRef};
use crate::config::{ApiProtocol, AppConfig, ModelProfile};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

pub(crate) const PROVIDER: &str = "claude";
/// Profiles bound to the CLI carry this marker instead of an HTTP endpoint;
/// no request is ever sent to it.
pub(crate) const ENDPOINT: &str = "claude-cli://local";
const PROGRAM_ENV: &str = "RUSTCODE_CLAUDE_CLI";
const FALLBACK_ACCOUNT: &str = "claude-cli";
const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn program() -> std::ffi::OsString {
    crate::shell_env::env_var(PROGRAM_ENV)
        .filter(|value| !value.trim().is_empty())
        .map(Into::into)
        .unwrap_or_else(|| "claude".into())
}

/// Base `claude` invocation. API-key variables are removed so the child uses
/// the CLI's own sign-in rather than silently billing an API key.
pub(crate) fn command() -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program());
    command
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .kill_on_drop(true);
    command
}

/// Arguments shared by every headless child: stream-json both ways, no
/// built-in tools, and none of the user's Claude Code settings, hooks, MCP
/// servers, slash commands, or saved sessions.
pub(crate) fn headless_args() -> [&'static str; 14] {
    [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--include-partial-messages",
    ]
}

/// The reasoning effort to pass to the CLI, when the profile names one it
/// accepts.
pub(crate) fn effort_arg(profile: &ModelProfile) -> Option<&str> {
    let effort = profile.reasoning_effort.as_deref()?.trim();
    EFFORT_LEVELS
        .iter()
        .find(|level| level.eq_ignore_ascii_case(effort))
        .copied()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliStatus {
    #[serde(default)]
    logged_in: bool,
    #[serde(default)]
    auth_method: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    org_id: Option<String>,
    #[serde(default)]
    subscription_type: Option<String>,
}

fn install_hint() -> String {
    format!(
        "could not run the Claude Code CLI; install it and sign in with `claude auth login`, or point {PROGRAM_ENV} at the binary"
    )
}

async fn cli_status(cancel: &CancellationToken) -> Result<CliStatus> {
    let mut command = command();
    command
        .args(["auth", "status", "--json"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let output = tokio::select! {
        _ = cancel.cancelled() => bail!("Claude sign-in check was cancelled"),
        output = tokio::time::timeout(Duration::from_secs(30), command.output()) => output
            .map_err(|_| anyhow!("Claude Code CLI status check timed out"))?
            .with_context(install_hint)?,
    };
    // A signed-out CLI may exit non-zero while still printing its status.
    serde_json::from_slice(&output.stdout).map_err(|_| {
        anyhow!("Claude Code CLI returned an unreadable sign-in status; update the CLI and retry")
    })
}

fn account_from_status(status: &CliStatus) -> AccountStatus {
    let account = status
        .org_id
        .as_deref()
        .map(|id| {
            id.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect::<String>()
        })
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| FALLBACK_ACCOUNT.into());
    let plan = match (
        status.subscription_type.as_deref(),
        status.auth_method.as_deref(),
    ) {
        (Some(plan), _) if !plan.is_empty() => format!("Claude {plan} plan"),
        (_, Some(method)) if !method.is_empty() => format!("Claude CLI ({method})"),
        _ => "Claude CLI".into(),
    };
    let display = match status.email.as_deref().filter(|email| !email.is_empty()) {
        Some(email) => format!("{plan}, {email}"),
        None => plan,
    };
    AccountStatus {
        provider: PROVIDER.into(),
        account,
        method: AuthMethod::ClaudeCli,
        display,
        endpoint: ENDPOINT.into(),
        client_id: None,
        scopes: Vec::new(),
        expires_at: None,
        active: true,
    }
}

/// Ask a short-lived child for the signed-in account's model list. The
/// `initialize` handshake answers without sending anything to a model.
async fn fetch_models(cancel: &CancellationToken) -> Result<Value> {
    let mut command = command();
    command
        .args(headless_args())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().with_context(install_hint)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Claude Code CLI input is unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Claude Code CLI output is unavailable"))?;
    let handshake = async {
        let request = json!({
            "type": "control_request",
            "request_id": "rustcode-catalog",
            "request": {"subtype": "initialize"},
        });
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .context("could not query the Claude Code CLI")?;
        stdin.flush().await.ok();
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .context("could not read the Claude Code CLI response")?
        {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value["type"] != "control_response" {
                continue;
            }
            if value.pointer("/response/subtype").and_then(Value::as_str) != Some("success") {
                bail!(
                    "Claude Code CLI rejected the model catalog request; update the CLI and retry"
                );
            }
            return value
                .pointer("/response/response/models")
                .cloned()
                .ok_or_else(|| {
                    anyhow!("Claude Code CLI did not report a model list; update the CLI and retry")
                });
        }
        bail!(
            "Claude Code CLI exited before reporting its models; run `claude auth status` to check the sign-in"
        )
    };
    tokio::select! {
        _ = cancel.cancelled() => bail!("Claude sign-in check was cancelled"),
        result = tokio::time::timeout(Duration::from_secs(45), handshake) => {
            result.map_err(|_| anyhow!("Claude Code CLI model catalog request timed out"))?
        }
    }
}

fn catalog_profiles(models: &Value, account: &AccountStatus) -> Vec<ModelProfile> {
    let mut profiles: Vec<ModelProfile> = Vec::new();
    for item in models.as_array().into_iter().flatten() {
        let Some(id) = item
            .get("value")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && *id != "default")
        else {
            continue;
        };
        let valid = id.len() <= 96
            && id.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'[' | b']')
            });
        if !valid || profiles.iter().any(|profile| profile.model == id) {
            continue;
        }
        profiles.push(ModelProfile {
            name: format!("claude/{}", id.strip_prefix("claude-").unwrap_or(id)),
            url: ENDPOINT.into(),
            model: id.into(),
            engine: Some(PROVIDER.into()),
            api_protocol: Some(ApiProtocol::AnthropicMessages),
            tool_protocol: Some(rustcode_core::ToolProtocol::ApiNative),
            credential: Some(CredentialRef {
                provider: PROVIDER.into(),
                account: account.account.clone(),
                method: AuthMethod::ClaudeCli,
            }),
            context_window: Some(CONTEXT_WINDOW),
            supports_vision: Some(true),
            supports_reasoning_effort: Some(
                item.get("supportsEffort").and_then(Value::as_bool) == Some(true),
            ),
            reasoning_efforts: (item.get("supportsEffort").and_then(Value::as_bool) == Some(true))
                .then(|| {
                    EFFORT_LEVELS
                        .iter()
                        .map(|effort| (*effort).to_owned())
                        .collect()
                }),
            supports_thinking_budget: Some(false),
            ..Default::default()
        });
    }
    profiles
}

/// The CLI does not report a context window and manages its own context, so
/// this only bounds RustCode's local history before it compacts.
const CONTEXT_WINDOW: u32 = 200_000;

fn catalog_result(account: &AccountStatus, profiles: Vec<ModelProfile>) -> AuthCommandResult {
    AuthCommandResult {
        message: format!(
            "Connected through the Claude Code CLI ({}). Loaded {} models; select one with /model. Requests use the CLI's own sign-in and its plan limits.",
            account.display,
            profiles.len(),
        ),
        profile: profiles.first().cloned(),
        profiles,
    }
}

fn ensure_enabled(config: &AppConfig) -> Result<()> {
    let enabled = config.providers.iter().any(|provider| {
        provider.id == PROVIDER && provider.auth_methods.contains(&AuthMethod::ClaudeCli)
    });
    if !enabled {
        bail!("the claude provider is not configured for Claude Code CLI sign-in");
    }
    Ok(())
}

/// `/login claude` and `/refresh claude`: read the CLI's sign-in state, save
/// the non-secret account row, and install one profile per offered model.
pub(super) async fn login(
    config: &AppConfig,
    cancel: &CancellationToken,
) -> Result<AuthCommandResult> {
    ensure_enabled(config)?;
    let status = cli_status(cancel).await?;
    if !status.logged_in {
        bail!(
            "the Claude Code CLI is not signed in; run `claude auth login` in a terminal, then /login claude"
        );
    }
    let account = account_from_status(&status);
    let profiles = catalog_profiles(&fetch_models(cancel).await?, &account);
    if profiles.is_empty() {
        bail!("the Claude Code CLI reported no usable models for this account");
    }
    // A different CLI account replaces the previous row: the CLI holds one
    // sign-in at a time, so an older row could never serve a request.
    for mut stale in super::load_accounts()?
        .into_iter()
        .filter(|row| row.provider == PROVIDER && row.method == AuthMethod::ClaudeCli && row.active)
    {
        if stale.account != account.account {
            stale.active = false;
            super::upsert_account(stale)?;
        }
    }
    super::upsert_account(account.clone())?;
    Ok(catalog_result(&account, profiles))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(json: Value) -> CliStatus {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn account_row_uses_the_cli_org_and_never_a_credential() {
        let account = account_from_status(&status(json!({
            "loggedIn": true,
            "authMethod": "claude.ai",
            "email": "dev@example.com",
            "orgId": "0b6c1f0e-1111-2222-3333-444455556666",
            "subscriptionType": "pro",
        })));
        assert_eq!(account.provider, "claude");
        assert_eq!(account.account, "0b6c1f0e-1111-2222-3333-444455556666");
        assert_eq!(account.method, AuthMethod::ClaudeCli);
        assert_eq!(account.display, "Claude pro plan, dev@example.com");
        assert_eq!(account.endpoint, ENDPOINT);

        let bare = account_from_status(&status(json!({"loggedIn": true, "orgId": "../x y"})));
        assert_eq!(bare.account, "xy");
        let none = account_from_status(&status(json!({"loggedIn": true})));
        assert_eq!(none.account, FALLBACK_ACCOUNT);
        assert_eq!(none.display, "Claude CLI");
    }

    #[test]
    fn catalog_builds_cli_bound_profiles_and_skips_the_default_alias() {
        let account = account_from_status(&status(json!({"loggedIn": true, "orgId": "org1"})));
        let profiles = catalog_profiles(
            &json!([
                {"value": "default", "resolvedModel": "claude-opus-5-5"},
                {"value": "opus", "supportsEffort": true},
                {"value": "claude-fable-5-1", "supportsEffort": true},
                {"value": "haiku"},
                {"value": "opus"},
                {"value": "bad id; rm"},
                {"displayName": "no value"},
            ]),
            &account,
        );
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["claude/opus", "claude/fable-5-1", "claude/haiku"]);
        let opus = &profiles[0];
        assert_eq!(opus.model, "opus");
        assert_eq!(opus.url, ENDPOINT);
        assert_eq!(opus.api_protocol, Some(ApiProtocol::AnthropicMessages));
        assert_eq!(opus.supports_reasoning_effort, Some(true));
        assert_eq!(profiles[2].supports_reasoning_effort, Some(false));
        assert!(opus.credential.as_ref().unwrap().is_claude_cli());
        assert!(opus.api_key.is_none() && opus.env_key.is_none());
    }

    #[test]
    fn effort_is_passed_only_when_the_cli_accepts_it() {
        let mut profile = ModelProfile::default();
        assert_eq!(effort_arg(&profile), None);
        profile.reasoning_effort = Some("High".into());
        assert_eq!(effort_arg(&profile), Some("high"));
        profile.reasoning_effort = Some("minimal".into());
        assert_eq!(effort_arg(&profile), None);
    }
}
