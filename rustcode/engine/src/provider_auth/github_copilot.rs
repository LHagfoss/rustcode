//! GitHub Copilot credentials, device authorization, and authenticated catalog.

use super::{
    AccountStatus, AuthCommandResult, AuthMethod, CredentialRef, CredentialStore,
    NativeCredentialStore,
};
use crate::config::{ApiProtocol, AppConfig, ModelProfile, ToolProtocol};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

pub(super) const PROVIDER: &str = "github-copilot";
const ENDPOINT: &str = "https://api.githubcopilot.com";
const DEVICE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const IDENTITY_URL: &str = "https://api.github.com/user";
const USER_AGENT: &str = concat!("rustcode/", env!("CARGO_PKG_VERSION"));
const API_VERSION: &str = "2026-06-01";
const SECRET_KIND: &str = "copilot-token";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()
        .context("could not initialize GitHub transport")
}

pub(super) fn validate_endpoint(endpoint: &str) -> Result<()> {
    if !matches!(
        endpoint,
        "https://api.githubcopilot.com"
            | "https://api.business.githubcopilot.com"
            | "https://api.enterprise.githubcopilot.com"
    ) {
        bail!("Copilot credential endpoint must be a supported GitHub Copilot HTTPS origin");
    }
    Ok(())
}

pub(super) async fn account_lock() -> tokio::sync::OwnedMutexGuard<()> {
    static LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(Mutex::new(())))
        .clone()
        .lock_owned()
        .await
}

fn pending_login() -> &'static std::sync::Mutex<Option<CancellationToken>> {
    static PENDING: OnceLock<std::sync::Mutex<Option<CancellationToken>>> = OnceLock::new();
    PENDING.get_or_init(|| std::sync::Mutex::new(None))
}

pub(crate) fn cancel_login() {
    if let Ok(guard) = pending_login().lock() {
        if let Some(token) = guard.as_ref() {
            token.cancel();
        }
    }
}

fn progress(sender: &Option<mpsc::Sender<String>>, message: String) {
    if let Some(sender) = sender {
        let _ = sender.try_send(message);
    }
}

#[derive(Deserialize)]
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    #[serde(default = "poll_interval")]
    interval: u64,
}
fn poll_interval() -> u64 {
    5
}

#[derive(Default, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    refresh_token_expires_in: Option<u64>,
    token_type: Option<String>,
    scope: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}
impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenResponse([redacted])")
    }
}

#[derive(Serialize, Deserialize)]
struct SecretTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
    refresh_expires_at: Option<u64>,
}
impl TokenResponse {
    fn into_tokens(self) -> Result<SecretTokens> {
        if self
            .token_type
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
        {
            bail!("GitHub returned an unsupported credential type");
        }
        let access_token = self
            .access_token
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| anyhow!("GitHub did not return an access token; sign in again"))?;
        Ok(SecretTokens {
            access_token,
            refresh_token: self.refresh_token,
            expires_at: self.expires_in.map(|seconds| now().saturating_add(seconds)),
            refresh_expires_at: self
                .refresh_token_expires_in
                .map(|seconds| now().saturating_add(seconds)),
        })
    }
}

async fn token_request(client: &reqwest::Client, url: &str, body: &Value) -> Result<TokenResponse> {
    let response = client
        .post(url)
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await
        .context("GitHub authorization request failed")?;
    if !response.status().is_success() {
        bail!(
            "GitHub authorization returned HTTP {}",
            response.status().as_u16()
        );
    }
    response
        .json()
        .await
        .map_err(|_| anyhow!("GitHub authorization response was invalid"))
}

async fn poll_device(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    device: DeviceAuthorization,
    cancel: &CancellationToken,
) -> Result<TokenResponse> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(device.expires_in.min(900));
    let mut interval = device.interval;
    let body = json!({"client_id":client_id,"device_code":device.device_code,"grant_type":"urn:ietf:params:oauth:grant-type:device_code"});
    loop {
        let response = tokio::select! {
            _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"),
            _ = tokio::time::sleep_until(deadline) => bail!("GitHub device authorization expired; run /login github-copilot again"),
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {
                tokio::select! {
                    _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"),
                    _ = tokio::time::sleep_until(deadline) => bail!("GitHub device authorization expired; run /login github-copilot again"),
                    response = token_request(client, token_url, &body) => response?,
                }
            }
        };
        if response.access_token.is_some() {
            return Ok(response);
        }
        match response.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                interval = interval
                    .saturating_add(5)
                    .max(response.interval.unwrap_or(0))
            }
            Some("access_denied") => bail!("GitHub authorization was denied"),
            Some("expired_token") => {
                bail!("GitHub device authorization expired; run /login github-copilot again")
            }
            _ => bail!("GitHub device authorization could not be completed"),
        }
    }
}

async fn device_login(
    client: &reqwest::Client,
    client_id: &str,
    cancel: &CancellationToken,
    sender: &Option<mpsc::Sender<String>>,
) -> Result<(SecretTokens, Vec<String>)> {
    let response = tokio::select! {
        _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"),
        response = client.post(DEVICE_URL).header("Accept","application/json").json(&json!({"client_id":client_id,"scope":"read:user offline_access"})).send() => response.context("could not begin GitHub device authorization")?,
    };
    if !response.status().is_success() {
        bail!(
            "GitHub device authorization returned HTTP {}",
            response.status().as_u16()
        );
    }
    let device: DeviceAuthorization = response
        .json()
        .await
        .map_err(|_| anyhow!("GitHub device authorization response was invalid"))?;
    if device.verification_uri != "https://github.com/login/device"
        || !valid_user_code(&device.user_code)
        || device.device_code.is_empty()
        || device.expires_in == 0
    {
        bail!("GitHub returned an invalid device authorization challenge");
    }
    progress(
        sender,
        format!(
            "GitHub Copilot sign-in\nOpen {} and enter code: {}\nWaiting for authorization… Press Esc to cancel.",
            device.verification_uri, device.user_code
        ),
    );
    let _ = open::that_detached(&device.verification_uri);
    let token = poll_device(client, TOKEN_URL, client_id, device, cancel).await?;
    let scopes = token
        .scope
        .as_deref()
        .unwrap_or("")
        .split([',', ' '])
        .filter(|scope| !scope.is_empty())
        .map(str::to_owned)
        .collect();
    Ok((token.into_tokens()?, scopes))
}
fn valid_user_code(code: &str) -> bool {
    code.len() >= 6
        && code.len() <= 16
        && code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
}

async fn gh_token(program: &std::ffi::OsStr, cancel: &CancellationToken) -> Result<Option<String>> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(["auth", "token", "--hostname", "github.com"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    // Read only gh's documented credential output; never echo it or inspect
    // arbitrary entries in another application's credential store.
    let output = tokio::select! {
        _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"),
        output = tokio::time::timeout(Duration::from_secs(30),command.output()) => output.map_err(|_| anyhow!("GitHub CLI credential lookup timed out"))?.context("install GitHub CLI (gh), sign in with gh auth login, or configure RUSTCODE_COPILOT_CLIENT_ID")?,
    };
    if !output.status.success() {
        return Ok(None);
    }
    let token = String::from_utf8(output.stdout)
        .map_err(|_| anyhow!("GitHub CLI returned an invalid credential"))?;
    let token = token.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        bail!("GitHub CLI returned an invalid credential");
    }
    Ok(Some(token.to_owned()))
}

async fn gh_login(
    cancel: &CancellationToken,
    sender: &Option<mpsc::Sender<String>>,
) -> Result<SecretTokens> {
    gh_login_using(std::ffi::OsStr::new("gh"), cancel, sender).await
}

async fn gh_login_using(
    program: &std::ffi::OsStr,
    cancel: &CancellationToken,
    sender: &Option<mpsc::Sender<String>>,
) -> Result<SecretTokens> {
    if let Some(access_token) = gh_token(program, cancel).await? {
        return Ok(SecretTokens {
            access_token,
            refresh_token: None,
            expires_at: None,
            refresh_expires_at: None,
        });
    }
    progress(
        sender,
        "GitHub Copilot sign-in\nStarting GitHub CLI browser authorization… Press Esc to cancel."
            .into(),
    );
    let mut child = tokio::process::Command::new(program)
        .args([
            "auth",
            "login",
            "--hostname",
            "github.com",
            "--git-protocol",
            "https",
            "--web",
            "--skip-ssh-key",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("could not start GitHub CLI sign-in; run gh auth login outside RustCode")?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("GitHub CLI device-code output is unavailable"))?;
    let mut lines = BufReader::new(stderr).lines();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let line = tokio::select! {
            _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"),
            _ = tokio::time::sleep_until(deadline) => bail!("GitHub CLI sign-in timed out; run gh auth login outside RustCode"),
            line = lines.next_line() => line.context("GitHub CLI sign-in output stopped")?,
        };
        let Some(line) = line else {
            break;
        };
        // Never relay arbitrary subprocess output. gh emits the public device
        // challenge on stderr; display only the validated code and fixed URL.
        if let Some(code) = line
            .split_whitespace()
            .find(|word| word.contains('-') && valid_user_code(word))
        {
            progress(
                sender,
                format!(
                    "GitHub Copilot sign-in\nOpen https://github.com/login/device and enter code: {code}\nWaiting for GitHub CLI authorization… Press Esc to cancel."
                ),
            );
        }
    }
    let status = tokio::select! {
        _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"),
        _ = tokio::time::sleep_until(deadline) => bail!("GitHub CLI sign-in timed out"),
        status = child.wait() => status.context("GitHub CLI sign-in stopped")?,
    };
    if !status.success() {
        bail!(
            "GitHub CLI sign-in failed; run gh auth login outside RustCode and retry /login github-copilot"
        );
    }
    let access_token = gh_token(program, cancel)
        .await?
        .ok_or_else(|| anyhow!("GitHub CLI has no saved credential after sign-in"))?;
    Ok(SecretTokens {
        access_token,
        refresh_token: None,
        expires_at: None,
        refresh_expires_at: None,
    })
}

fn ensure_enabled(config: &AppConfig) -> Result<&str> {
    let provider = super::provider_definition(config, PROVIDER)?;
    if !provider.auth_methods.contains(&AuthMethod::GitHubCopilot) {
        bail!("GitHub Copilot sign-in is disabled in provider configuration");
    }
    let endpoint = provider.base_url.as_deref().unwrap_or(ENDPOINT);
    validate_endpoint(endpoint)?;
    Ok(endpoint)
}

pub(super) async fn login(
    config: &AppConfig,
    requested: Option<&str>,
    caller_cancel: &CancellationToken,
    sender: Option<mpsc::Sender<String>>,
) -> Result<AuthCommandResult> {
    let endpoint = ensure_enabled(config)?.to_owned();
    if requested != Some("new") {
        if let Some(result) = refresh_catalog(config, requested).await? {
            return Ok(result);
        }
        if requested.is_some() {
            bail!("no active Copilot account has that account ID; use /login github-copilot new");
        }
    }
    let cancel = caller_cancel.child_token();
    {
        let mut pending = pending_login()
            .lock()
            .map_err(|_| anyhow!("GitHub sign-in lock unavailable"))?;
        if let Some(old) = pending.replace(cancel.clone()) {
            old.cancel();
        }
    }
    let _guard = tokio::select! { _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"), guard = account_lock() => guard };
    let client = http_client()?;
    let client_id =
        crate::shell_env::env_var("RUSTCODE_COPILOT_CLIENT_ID").filter(|id| !id.trim().is_empty());
    if client_id.as_ref().is_some_and(|id| {
        id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    }) {
        bail!("RUSTCODE_COPILOT_CLIENT_ID is invalid");
    }
    let (tokens, scopes) = if let Some(id) = client_id.as_deref() {
        device_login(&client, id, &cancel, &sender).await?
    } else {
        (gh_login(&cancel, &sender).await?, Vec::new())
    };
    #[derive(Deserialize)]
    struct Identity {
        id: u64,
        login: String,
    }
    let response = tokio::select! {
        _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"),
        response = client.get(IDENTITY_URL).header("Accept","application/vnd.github+json").bearer_auth(&tokens.access_token).send() => response.context("GitHub account lookup failed")?,
    };
    if !response.status().is_success() {
        bail!(
            "GitHub account lookup returned HTTP {}; run gh auth login again if this credential expired",
            response.status().as_u16()
        );
    }
    let identity: Identity = response
        .json()
        .await
        .map_err(|_| anyhow!("GitHub account response was invalid"))?;
    let account = AccountStatus {
        provider: PROVIDER.into(),
        account: format!("github-{}", identity.id),
        method: AuthMethod::GitHubCopilot,
        display: identity.login,
        endpoint,
        client_id,
        scopes,
        expires_at: tokens.expires_at,
        active: true,
    };
    let profiles = tokio::select! { _ = cancel.cancelled() => bail!("GitHub sign-in was cancelled"), profiles = fetch_catalog(&client,&account,&tokens.access_token) => profiles? };
    if cancel.is_cancelled() {
        bail!("GitHub sign-in was cancelled");
    }
    persist_connection(
        Arc::new(NativeCredentialStore),
        account.clone(),
        tokens,
        _guard,
    )
    .await?;
    if cancel.is_cancelled() {
        bail!("GitHub sign-in was cancelled");
    }
    Ok(catalog_result(&account, profiles))
}

async fn persist_connection(
    store: Arc<dyn CredentialStore>,
    account: AccountStatus,
    tokens: SecretTokens,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        store_connection(store.as_ref(), &account, &tokens, || {
            super::upsert_account(account.clone())
        })
    })
    .await
    .context("Copilot credential-store worker stopped")?
}

fn store_connection(
    store: &dyn CredentialStore,
    account: &AccountStatus,
    tokens: &SecretTokens,
    commit_metadata: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let values = [
        ("endpoint", account.endpoint.clone()),
        (
            SECRET_KIND,
            serde_json::to_string(tokens)
                .map_err(|_| anyhow!("could not encode Copilot credentials"))?,
        ),
    ];
    let old: Vec<_> = values
        .iter()
        .map(|(kind, _)| store.get_secret(PROVIDER, &account.account, kind).ok())
        .collect();
    let save = (|| {
        for (kind, value) in &values {
            store.set_secret(PROVIDER, &account.account, kind, value)?;
        }
        commit_metadata()
    })();
    if save.is_err() {
        for ((kind, _), prior) in values.iter().zip(old) {
            if let Some(value) = prior {
                let _ = store.set_secret(PROVIDER, &account.account, kind, &value);
            } else {
                let _ = store.delete_secret(PROVIDER, &account.account, kind);
            }
        }
    }
    save
}

async fn load_bound_tokens(
    store: Arc<dyn CredentialStore>,
    profile: &ModelProfile,
    account: &AccountStatus,
) -> Result<SecretTokens> {
    validate_endpoint(&account.endpoint)?;
    super::validate_profile_endpoint(profile, account)?;
    let account = account.clone();
    let store = tokio::task::spawn_blocking(move || {
        let endpoint = store.get_secret(PROVIDER, &account.account, "endpoint")?;
        if endpoint != account.endpoint {
            bail!("stored Copilot endpoint binding does not match provider metadata");
        }
        let serialized = store.get_secret(PROVIDER, &account.account, SECRET_KIND)?;
        serde_json::from_str(&serialized)
            .map_err(|_| anyhow!("stored Copilot credential is invalid; sign in again"))
    });
    store
        .await
        .context("Copilot credential-store worker stopped")?
}

pub(super) async fn resolve_access_token(
    profile: &ModelProfile,
    account: &AccountStatus,
) -> Result<String> {
    let _guard = account_lock().await;
    let account = super::load_accounts()?
        .into_iter()
        .find(|row| row.provider == PROVIDER && row.account == account.account && row.active)
        .ok_or_else(|| anyhow!("Copilot account is signed out; run /login github-copilot"))?;
    let tokens = load_bound_tokens(Arc::new(NativeCredentialStore), profile, &account).await?;
    if tokens
        .expires_at
        .is_none_or(|expiry| expiry > now().saturating_add(60))
    {
        return Ok(tokens.access_token);
    }
    let refresh = tokens.refresh_token.as_deref().filter(|_| tokens.refresh_expires_at.is_none_or(|expiry| expiry > now())).ok_or_else(|| anyhow!("Copilot credential expired; run /login github-copilot new (or gh auth login for CLI credentials)"))?;
    let client_id = account
        .client_id
        .as_deref()
        .ok_or_else(|| anyhow!("Copilot credential has no refresh registration; sign in again"))?;
    let response = token_request(
        &http_client()?,
        TOKEN_URL,
        &json!({"client_id":client_id,"grant_type":"refresh_token","refresh_token":refresh}),
    )
    .await?;
    let mut fresh = response.into_tokens()?;
    if fresh.refresh_token.is_none() {
        fresh.refresh_token = tokens.refresh_token;
        fresh.refresh_expires_at = tokens.refresh_expires_at;
    }
    let access = fresh.access_token.clone();
    let mut updated = account;
    updated.expires_at = fresh.expires_at;
    persist_connection(Arc::new(NativeCredentialStore), updated, fresh, _guard).await?;
    Ok(access)
}

pub(super) async fn refresh_catalog(
    config: &AppConfig,
    requested: Option<&str>,
) -> Result<Option<AuthCommandResult>> {
    ensure_enabled(config)?;
    let accounts: Vec<_> = super::load_accounts()?
        .into_iter()
        .filter(|row| {
            row.provider == PROVIDER && row.method == AuthMethod::GitHubCopilot && row.active
        })
        .collect();
    let selected = if let Some(id) = requested {
        accounts.iter().find(|row| row.account == id)
    } else {
        let current = config
            .models
            .iter()
            .find(|profile| profile.name == config.default.big())
            .and_then(|profile| profile.credential.as_ref())
            .filter(|binding| binding.provider == PROVIDER);
        current
            .and_then(|binding| accounts.iter().find(|row| row.account == binding.account))
            .or_else(|| (accounts.len() == 1).then(|| &accounts[0]))
    };
    let Some(account) = selected else {
        if requested.is_none() && accounts.len() > 1 {
            bail!("multiple Copilot accounts are saved; choose an account from /auth status");
        }
        return Ok(None);
    };
    validate_endpoint(&account.endpoint)?;
    let profile = ModelProfile {
        url: format!("{}/chat/completions", account.endpoint),
        credential: Some(CredentialRef {
            provider: PROVIDER.into(),
            account: account.account.clone(),
            method: AuthMethod::GitHubCopilot,
        }),
        ..Default::default()
    };
    let token = super::resolve_profile_credential(&profile)
        .await?
        .ok_or_else(|| anyhow!("Copilot credential is unavailable"))?;
    let profiles = fetch_catalog(&http_client()?, account, &token).await?;
    Ok(Some(catalog_result(account, profiles)))
}
fn catalog_result(account: &AccountStatus, profiles: Vec<ModelProfile>) -> AuthCommandResult {
    AuthCommandResult {
        message: format!(
            "GitHub Copilot connected as {}. Loaded {} usable models; select one with /model. Account: {}.",
            account.display,
            profiles.len(),
            account.account
        ),
        profile: profiles.first().cloned(),
        profiles,
    }
}

async fn fetch_catalog(
    client: &reqwest::Client,
    account: &AccountStatus,
    token: &str,
) -> Result<Vec<ModelProfile>> {
    validate_endpoint(&account.endpoint)?;
    let response = client
        .get(format!("{}/models", account.endpoint))
        .header("X-GitHub-Api-Version", API_VERSION)
        .bearer_auth(token)
        .send()
        .await
        .context("Copilot model catalog request failed")?;
    if !response.status().is_success() {
        bail!(catalog_error(response.status().as_u16()));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("Copilot model catalog response was invalid"))?;
    let profiles = catalog_profiles(&value, account)?;
    if profiles.is_empty() {
        bail!(
            "Copilot returned no enabled streaming tool models with a supported protocol; check your plan and organization model policies"
        );
    }
    Ok(profiles)
}
fn catalog_error(status: u16) -> String {
    match status {
        401 => "Copilot rejected the GitHub credential; run gh auth login and /login github-copilot new, or reconnect your RustCode OAuth app".into(),
        403 => "Copilot access is unavailable; check your subscription, organization policy, and the authorized app's Copilot permissions".into(),
        _ => format!("Copilot model catalog returned HTTP {status}"),
    }
}

fn catalog_profiles(value: &Value, account: &AccountStatus) -> Result<Vec<ModelProfile>> {
    validate_endpoint(&account.endpoint)?;
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Copilot catalog has no model list"))?;
    let mut profiles = Vec::new();
    for item in items {
        if item.pointer("/policy/state").and_then(Value::as_str) == Some("disabled")
            || item
                .pointer("/capabilities/supports/tool_calls")
                .and_then(Value::as_bool)
                != Some(true)
            || item
                .pointer("/capabilities/supports/streaming")
                .and_then(Value::as_bool)
                == Some(false)
        {
            continue;
        }
        let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        // Legacy catalog records omit supported_endpoints. Fall back to Chat
        // Completions only when the record is explicitly a chat model with
        // valid streaming/tool capabilities and limits (checked below).
        let legacy_chat = item.get("supported_endpoints").is_none()
            && item.pointer("/capabilities/type").and_then(Value::as_str) == Some("chat");
        let endpoints = item.get("supported_endpoints").and_then(Value::as_array);
        let supports = |endpoint: &str| {
            endpoints.is_some_and(|endpoints| {
                endpoints
                    .iter()
                    .any(|value| value.as_str() == Some(endpoint))
            })
        };
        let (protocol, path) = if supports("/v1/messages") {
            (ApiProtocol::AnthropicMessages, "/v1/messages")
        } else if supports("/responses") {
            (ApiProtocol::Responses, "/responses")
        } else if supports("/chat/completions") || legacy_chat {
            (ApiProtocol::ChatCompletions, "/chat/completions")
        } else {
            continue;
        };
        let number = |name: &str| {
            item.pointer(&format!("/capabilities/limits/{name}"))
                .and_then(Value::as_u64)
                .and_then(|number| u32::try_from(number).ok())
                .filter(|number| *number > 0)
        };
        let (Some(input), Some(output)) =
            (number("max_prompt_tokens"), number("max_output_tokens"))
        else {
            continue;
        };
        let context = number("max_context_window_tokens").unwrap_or(input);
        let vision = item
            .pointer("/capabilities/supports/vision")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || item
                .pointer("/capabilities/limits/vision/supported_media_types")
                .and_then(Value::as_array)
                .is_some_and(|types| {
                    types
                        .iter()
                        .any(|kind| kind.as_str().is_some_and(|kind| kind.starts_with("image/")))
                });
        let reasoning = protocol != ApiProtocol::AnthropicMessages
            && item
                .pointer("/capabilities/supports/reasoning_effort")
                .and_then(Value::as_array)
                .is_some_and(|efforts| !efforts.is_empty());
        profiles.push(ModelProfile {
            name: format!("copilot/{id}"),
            model: id.into(),
            url: format!("{}{path}", account.endpoint),
            engine: Some(PROVIDER.into()),
            api_protocol: Some(protocol),
            tool_protocol: Some(ToolProtocol::ApiNative),
            credential: Some(CredentialRef {
                provider: PROVIDER.into(),
                account: account.account.clone(),
                method: AuthMethod::GitHubCopilot,
            }),
            context_window: Some(context),
            provider_context_window: Some(context),
            hard_effective_limit: Some(input.min(context)),
            max_output_tokens: Some(output),
            supports_vision: Some(vision),
            supports_reasoning_effort: Some(reasoning),
            supports_thinking_budget: Some(false),
            enable_thinking: None,
            ..Default::default()
        });
    }
    Ok(profiles)
}

pub(crate) fn request_headers(payload: &Value, session_id: Option<&str>) -> HeaderMap {
    let messages = payload
        .get("messages")
        .or_else(|| payload.get("input"))
        .and_then(Value::as_array);
    let last = messages.and_then(|messages| messages.last());
    let tool_results = last
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            !blocks.is_empty() && blocks.iter().all(|block| block["type"] == "tool_result")
        });
    let agent = last.is_some_and(|message| {
        message["role"] != "user" || tool_results || message["type"] == "function_call_output"
    });
    fn has_image(value: &Value) -> bool {
        match value {
            Value::Array(items) => items.iter().any(has_image),
            Value::Object(map) => {
                map.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "image" | "image_url" | "input_image"))
                    || map.values().any(has_image)
            }
            _ => false,
        }
    }
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static(API_VERSION),
    );
    headers.insert(
        "openai-intent",
        HeaderValue::from_static("conversation-edits"),
    );
    headers.insert(
        "x-initiator",
        HeaderValue::from_static(if agent { "agent" } else { "user" }),
    );
    if has_image(payload) {
        headers.insert("copilot-vision-request", HeaderValue::from_static("true"));
    }
    if let Some(id) = session_id.filter(|id| {
        id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }) {
        if let Ok(id) = HeaderValue::from_str(id) {
            headers.insert("x-interaction-id", id);
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn account() -> AccountStatus {
        AccountStatus {
            provider: PROVIDER.into(),
            account: "github-42".into(),
            method: AuthMethod::GitHubCopilot,
            display: "octocat".into(),
            endpoint: ENDPOINT.into(),
            client_id: Some("owned-app-id".into()),
            scopes: vec!["read:user".into()],
            expires_at: None,
            active: true,
        }
    }

    #[test]
    fn catalog_uses_advertised_protocols_limits_and_excludes_disabled_or_unusable_models() {
        let model = |id: &str, endpoints: Value, tools: bool| json!({"id":id,"name":id,"model_picker_enabled":true,"supported_endpoints":endpoints,"capabilities":{"limits":{"max_context_window_tokens":128000,"max_prompt_tokens":120000,"max_output_tokens":8192},"supports":{"streaming":true,"tool_calls":tools,"vision":true,"reasoning_effort":["low","high"]}}});
        let mut disabled = model("disabled", json!(["/responses"]), true);
        disabled["policy"] = json!({"state":"disabled"});
        let profiles = catalog_profiles(&json!({"data":[model("chat",json!(["/chat/completions"]),true),model("responses",json!(["/responses"]),true),model("messages",json!(["/v1/messages"]),true),model("ws",json!(["ws:/responses"]),true),model("no-tools",json!(["/chat/completions"]),false),disabled]}), &account()).unwrap();
        assert_eq!(profiles.len(), 3);
        assert_eq!(
            profiles[0].resolved_api_protocol(),
            ApiProtocol::ChatCompletions
        );
        assert_eq!(profiles[1].resolved_api_protocol(), ApiProtocol::Responses);
        assert_eq!(
            profiles[2].endpoint_url(),
            "https://api.githubcopilot.com/v1/messages"
        );
        assert_eq!(profiles[0].provider_context_window, Some(128000));
        assert_eq!(profiles[0].hard_effective_limit, Some(120000));
        assert_eq!(profiles[0].supports_vision, Some(true));
        assert_eq!(profiles[0].supports_reasoning_effort, Some(true));
        assert_eq!(profiles[2].supports_reasoning_effort, Some(false));
        assert!(
            profiles
                .iter()
                .all(|profile| profile.api_key.is_none() && profile.credential.is_some())
        );
    }

    #[test]
    fn credential_endpoints_reject_userinfo_ports_queries_and_lookalike_hosts() {
        for endpoint in [
            "https://api.githubcopilot.com.attacker.example",
            "http://api.githubcopilot.com",
            "https://api.githubcopilot.com:444",
            "https://user@api.githubcopilot.com",
            "https://api.githubcopilot.com?token=steal",
            "https://api.githubcopilot.com/evil",
            "https://attacker.example",
        ] {
            assert!(validate_endpoint(endpoint).is_err(), "{endpoint}");
        }
        validate_endpoint(ENDPOINT).unwrap();
        validate_endpoint("https://api.business.githubcopilot.com").unwrap();
    }

    #[test]
    fn request_headers_distinguish_tool_continuations_and_nested_images() {
        let headers = request_headers(
            &json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":[{"type":"image","source":{}}]}]}]}),
            Some("session-42"),
        );
        assert_eq!(headers["x-initiator"], "agent");
        assert_eq!(headers["copilot-vision-request"], "true");
        assert_eq!(headers["x-interaction-id"], "session-42");
        assert_eq!(
            request_headers(&json!({"input":[{"role":"user","content":"hello"}]}), None)["x-initiator"],
            "user"
        );
    }

    #[test]
    fn catalog_accepts_usable_models_even_when_picker_flag_is_false() {
        let value = json!({"data":[{"id":"available","model_picker_enabled":false,"supported_endpoints":["/responses"],"capabilities":{"limits":{"max_prompt_tokens":128000,"max_output_tokens":8192},"supports":{"tool_calls":true,"streaming":true}}}]});
        assert_eq!(catalog_profiles(&value, &account()).unwrap().len(), 1);
    }

    #[test]
    fn legacy_chat_fallback_requires_chat_type_and_valid_tool_streaming_limits() {
        let chat = |id: &str| json!({"id":id,"capabilities":{"type":"chat","limits":{"max_prompt_tokens":64000,"max_output_tokens":4096,"max_context_window_tokens":128000},"supports":{"tool_calls":true,"streaming":true}}});
        let mut legacy =
            catalog_profiles(&json!({"data":[chat("legacy-chat")]}), &account()).unwrap();
        assert_eq!(legacy.len(), 1);
        assert_eq!(
            legacy.pop().unwrap().endpoint_url(),
            "https://api.githubcopilot.com/chat/completions"
        );
        // Embeddings, completion-only, tool-less, non-streaming, and
        // limit-less records without endpoints stay excluded.
        let bad = vec![
            json!({"id":"emb","capabilities":{"type":"embeddings","limits":{"max_inputs":512},"supports":{}}}),
            json!({"id":"comp","capabilities":{"type":"completion","limits":{"max_prompt_tokens":100,"max_output_tokens":100},"supports":{"streaming":true}}}),
            json!({"id":"no-tools","capabilities":{"type":"chat","limits":{"max_prompt_tokens":100,"max_output_tokens":100},"supports":{"tool_calls":false,"streaming":true}}}),
            json!({"id":"no-stream","capabilities":{"type":"chat","limits":{"max_prompt_tokens":100,"max_output_tokens":100},"supports":{"tool_calls":true,"streaming":false}}}),
            json!({"id":"no-limits","capabilities":{"type":"chat","supports":{"tool_calls":true,"streaming":true}}}),
            json!({"id":"unknown-ws","supported_endpoints":["ws:/responses"],"capabilities":{"type":"chat","limits":{"max_prompt_tokens":100,"max_output_tokens":100},"supports":{"tool_calls":true,"streaming":true}}}),
        ];
        assert!(
            catalog_profiles(&json!({"data":bad}), &account())
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn bound_tokens_reject_rewritten_metadata_before_returning_a_secret() {
        let store = Arc::new(super::super::tests::MemoryStore::default());
        let account = account();
        let profile = ModelProfile {
            url: format!("{ENDPOINT}/responses"),
            api_protocol: Some(ApiProtocol::Responses),
            ..Default::default()
        };
        store
            .set_secret(PROVIDER, &account.account, "endpoint", ENDPOINT)
            .unwrap();
        store.set_secret(PROVIDER,&account.account,SECRET_KIND,r#"{"access_token":"secret-not-logged","refresh_token":null,"expires_at":null,"refresh_expires_at":null}"#).unwrap();
        let tokens = load_bound_tokens(store.clone(), &profile, &account)
            .await
            .unwrap();
        assert_eq!(tokens.access_token, "secret-not-logged");
        let mut rewritten = account;
        rewritten.endpoint = "https://api.business.githubcopilot.com".into();
        let mut profile = profile;
        profile.url = "https://api.business.githubcopilot.com/responses".into();
        let error = load_bound_tokens(store, &profile, &rewritten)
            .await
            .err()
            .unwrap();
        assert!(!error.to_string().contains("secret-not-logged"));
    }

    #[test]
    fn failed_metadata_commit_restores_previous_secret_and_endpoint() {
        let store = super::super::tests::MemoryStore::default();
        let account = account();
        store
            .set_secret(PROVIDER, &account.account, "endpoint", "old-endpoint")
            .unwrap();
        store
            .set_secret(PROVIDER, &account.account, SECRET_KIND, "old-token")
            .unwrap();
        let tokens = SecretTokens {
            access_token: "fresh-secret".into(),
            refresh_token: Some("fresh-refresh".into()),
            expires_at: None,
            refresh_expires_at: None,
        };
        assert!(
            store_connection(&store, &account, &tokens, || bail!("metadata write failed")).is_err()
        );
        assert_eq!(
            store
                .get_secret(PROVIDER, &account.account, "endpoint")
                .unwrap(),
            "old-endpoint"
        );
        assert_eq!(
            store
                .get_secret(PROVIDER, &account.account, SECRET_KIND)
                .unwrap(),
            "old-token"
        );
    }

    #[tokio::test]
    async fn signout_cancels_pending_device_flow_and_waits_for_its_store_guard() {
        let cancel = CancellationToken::new();
        *pending_login().lock().unwrap() = Some(cancel.clone());
        let guard = account_lock().await;
        let running = tokio::spawn(async move {
            let _guard = guard;
            let device = DeviceAuthorization {
                device_code: "secret".into(),
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://github.com/login/device".into(),
                expires_in: 300,
                interval: 60,
            };
            poll_device(
                &http_client().unwrap(),
                "http://127.0.0.1:1",
                "owned",
                device,
                &cancel,
            )
            .await
        });
        cancel_login();
        let _signout_guard = tokio::time::timeout(Duration::from_secs(1), account_lock())
            .await
            .unwrap();
        assert!(
            running
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
    }

    #[test]
    fn token_response_preserves_nonexpiring_credentials_and_expiring_refresh_pairs() {
        let nonexpiring: TokenResponse =
            serde_json::from_value(json!({"access_token":"nonexpiring","token_type":"bearer"}))
                .unwrap();
        let tokens = nonexpiring.into_tokens().unwrap();
        assert!(tokens.expires_at.is_none() && tokens.refresh_token.is_none());
        let expiring: TokenResponse = serde_json::from_value(json!({"access_token":"expiring","refresh_token":"refresh","expires_in":3600,"refresh_token_expires_in":86400})).unwrap();
        let tokens = expiring.into_tokens().unwrap();
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
        assert!(tokens.expires_at.unwrap() >= now() + 3599);
        assert!(tokens.refresh_expires_at.unwrap() >= now() + 86399);
    }

    async fn server(responses: Vec<Value>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let mut bodies = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buf = [0; 2048];
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]);
                        let len = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|length| length.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                bodies.push(String::from_utf8_lossy(&bytes).into_owned());
                let body = response.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
            }
            bodies
        });
        (url, handle)
    }

    #[tokio::test]
    #[ignore = "explicit opt-in: uses existing gh credential and consumes Copilot quota; does not write accounts or credentials"]
    async fn live_copilot_stream_smoke_all_protocols() {
        assert_eq!(
            std::env::var("RUSTCODE_COPILOT_LIVE_TEST").as_deref(),
            Ok("1"),
            "set RUSTCODE_COPILOT_LIVE_TEST=1 for this opt-in smoke test"
        );
        let cancel = CancellationToken::new();
        let token = gh_token(std::ffi::OsStr::new("gh"), &cancel)
            .await
            .unwrap()
            .expect("sign in with gh auth login first");
        let client = http_client().unwrap();
        let profiles = fetch_catalog(&client, &account(), &token).await.unwrap();
        for (protocol, preferred, override_var) in [
            (
                ApiProtocol::ChatCompletions,
                "gpt-5-mini",
                "RUSTCODE_COPILOT_SMOKE_CHAT_MODEL",
            ),
            (
                ApiProtocol::Responses,
                "gpt-5.4-mini",
                "RUSTCODE_COPILOT_SMOKE_RESPONSES_MODEL",
            ),
            (
                ApiProtocol::AnthropicMessages,
                "claude-haiku-4.5",
                "RUSTCODE_COPILOT_SMOKE_MESSAGES_MODEL",
            ),
        ] {
            let requested = std::env::var(override_var).unwrap_or_else(|_| preferred.into());
            let profile = profiles
                .iter()
                .find(|profile| {
                    profile.resolved_api_protocol() == protocol && profile.model == requested
                })
                .or_else(|| {
                    profiles
                        .iter()
                        .find(|profile| profile.resolved_api_protocol() == protocol)
                })
                .expect("account has no usable model for a required protocol");
            let history = vec![json!({"role":"user","content":"Reply with exactly OK"})];
            let mut payload = match protocol {
                ApiProtocol::AnthropicMessages => {
                    crate::network::anthropic_messages::request_payload(
                        &profile.model,
                        &history,
                        &[],
                        256,
                        true,
                    )
                    .unwrap()
                }
                ApiProtocol::Responses => {
                    json!({"model":profile.model,"input":history,"stream":true,"max_output_tokens":256})
                }
                ApiProtocol::ChatCompletions => {
                    json!({"model":profile.model,"messages":history,"stream":true,"stream_options":{"include_usage":true},"max_tokens":256})
                }
            };
            // Prefer the least expensive advertised reasoning mode for this
            // tiny diagnostic; native Messages thinking remains disabled.
            if protocol == ApiProtocol::Responses && profile.supports_reasoning_effort == Some(true)
            {
                payload["reasoning"] = json!({"effort":"low"});
            }
            let mut request = client
                .post(profile.endpoint_url())
                .headers(request_headers(&payload, Some("rustcode-opt-in-smoke")))
                .bearer_auth(&token)
                .json(&payload);
            if protocol == ApiProtocol::AnthropicMessages {
                request = request.header("anthropic-version", "2023-06-01");
            }
            let response = request.send().await.unwrap();
            assert!(
                response.status().is_success(),
                "Copilot {:?} model {} returned HTTP {}",
                protocol,
                profile.model,
                response.status().as_u16()
            );
            let body = response.text().await.unwrap();
            assert!(body.len() < 1_000_000, "smoke stream unexpectedly large");
            let mut text = String::new();
            let mut terminal = false;
            let mut messages = crate::network::anthropic_messages::MessagesStream::default();
            let mut events = 0;
            for line in body.lines().filter_map(|line| line.strip_prefix("data: ")) {
                if line == "[DONE]" {
                    continue;
                }
                let value: Value = serde_json::from_str(line).unwrap();
                assert!(value.get("error").is_none(), "Copilot sent an error event");
                events += 1;
                match protocol {
                    ApiProtocol::AnthropicMessages => {
                        if let Some(value) = messages.normalize(&value) {
                            if let Some(delta) = value
                                .pointer("/choices/0/delta/content")
                                .and_then(Value::as_str)
                            {
                                text.push_str(delta);
                            }
                            assert!(
                                value.get("error").is_none(),
                                "Messages stream returned an unsupported event"
                            );
                        }
                        terminal = messages.completed;
                    }
                    ApiProtocol::Responses => {
                        if value["type"] == "response.output_text.delta" {
                            text.push_str(value["delta"].as_str().unwrap());
                        }
                        if value["type"] == "response.completed" {
                            terminal = true;
                        }
                    }
                    ApiProtocol::ChatCompletions => {
                        if let Some(delta) = value
                            .pointer("/choices/0/delta/content")
                            .and_then(Value::as_str)
                        {
                            text.push_str(delta);
                        }
                        if value
                            .pointer("/choices/0/finish_reason")
                            .and_then(Value::as_str)
                            .is_some()
                        {
                            terminal = true;
                        }
                    }
                }
            }
            assert!(
                terminal && text.contains("OK"),
                "Copilot {:?} stream failed to deliver a completed OK response",
                protocol
            );
            eprintln!(
                "Copilot {:?}: {} completed, {} events",
                protocol, profile.model, events
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn gh_browser_login_publishes_device_code_without_leaking_credential_or_inheriting_stdin()
    {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gh");
        std::fs::write(&path,r#"#!/bin/sh
marker="${0}.authorized"
case "$2" in
  token) if test -f "$marker"; then printf 'fake-gh-secret'; exit 0; else exit 1; fi ;;
  login) if read value; then exit 3; fi; printf '! First copy your one-time code: ABCD-EFGH\n' >&2; touch "$marker"; exit 0 ;;
esac
exit 4
"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (sender, mut progress) = mpsc::channel(8);
        let tokens = gh_login_using(path.as_os_str(), &CancellationToken::new(), &Some(sender))
            .await
            .unwrap();
        assert_eq!(tokens.access_token, "fake-gh-secret");
        let mut messages = Vec::new();
        while let Ok(message) = progress.try_recv() {
            messages.push(message);
        }
        assert!(messages.iter().any(|message| message.contains("ABCD-EFGH")
            && message.contains("https://github.com/login/device")));
        assert!(
            messages
                .iter()
                .all(|message| !message.contains("fake-gh-secret"))
        );
    }

    #[tokio::test]
    async fn expired_device_challenge_never_sends_a_token_request() {
        let device = DeviceAuthorization {
            device_code: "secret".into(),
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: 0,
            interval: 60,
        };
        let error = poll_device(
            &http_client().unwrap(),
            "http://127.0.0.1:1",
            "owned",
            device,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("expired"));
    }

    #[tokio::test]
    async fn device_poll_handles_pending_and_slow_down_then_returns_expiring_token() {
        let (url, requests) = server(vec![json!({"error":"authorization_pending"}),json!({"error":"slow_down","interval":1}),json!({"access_token":"test-secret","token_type":"bearer","expires_in":3600,"refresh_token":"refresh-secret","scope":"read:user,offline_access"})]).await;
        let device = DeviceAuthorization {
            device_code: "device-secret".into(),
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: 30,
            interval: 0,
        };
        let token = poll_device(
            &http_client().unwrap(),
            &url,
            "owned-app-id",
            device,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(token.access_token.as_deref(), Some("test-secret"));
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-secret"));
        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests
                .iter()
                .all(|body| body.contains("owned-app-id") && body.contains("device-secret"))
        );
    }

    #[tokio::test]
    async fn device_poll_returns_safe_denied_expired_errors_and_cancels_without_request() {
        for error in ["access_denied", "expired_token"] {
            let (url, requests) = server(vec![
                json!({"error":error,"error_description":"sensitive-echo"}),
            ])
            .await;
            let device = DeviceAuthorization {
                device_code: "secret".into(),
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://github.com/login/device".into(),
                expires_in: 30,
                interval: 0,
            };
            let error = poll_device(
                &http_client().unwrap(),
                &url,
                "owned",
                device,
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
            assert!(!error.to_string().contains("sensitive-echo"));
            requests.await.unwrap();
        }
        let token = CancellationToken::new();
        token.cancel();
        let device = DeviceAuthorization {
            device_code: "secret".into(),
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: 30,
            interval: 60,
        };
        assert!(
            poll_device(
                &http_client().unwrap(),
                "http://127.0.0.1:1",
                "owned",
                device,
                &token
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("cancelled")
        );
    }
}
