//! Provider credentials live in the OS credential store. This module keeps
//! only non-secret account bindings in RustCode's owner-only config folder.

pub(crate) mod claude_cli;
pub(crate) mod github_copilot;
mod openai;
mod rate_limits;
mod store;

use crate::config::{ApiProtocol, AppConfig, ModelProfile, ProviderDefinition};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

pub(crate) use rate_limits::rate_limit_header_pairs;
pub use rate_limits::{ProviderRateLimits, RateLimitWindow};
pub use store::{CredentialStore, NativeCredentialStore};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ApiKey,
    ChatGpt,
    #[serde(rename = "github_copilot")]
    GitHubCopilot,
    /// Requests are served by the locally installed Claude Code CLI, which
    /// keeps its own sign-in. RustCode holds no credential for this method.
    ClaudeCli,
}

impl AuthMethod {
    pub fn is_chatgpt(self) -> bool {
        matches!(self, Self::ChatGpt)
    }
}

impl fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ApiKey => "ApiKey",
            Self::ChatGpt => "ChatGpt",
            Self::GitHubCopilot => "GitHubCopilot",
            Self::ClaudeCli => "ClaudeCli",
        })
    }
}

/// How a sign-in reaches a browser. `/login <provider> --browser` and
/// `--headless` choose; with neither the session decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoginMode {
    /// Open the browser unless the session is remote or has no display, and
    /// show the URL instead when it cannot be opened.
    Auto,
    /// Open the browser on this machine, and fail if that is not possible.
    Browser,
    /// Never open a browser; show what to open on any device.
    Headless,
}

/// Whether `word` is one of the `/login` flags that choose a [`LoginMode`].
pub fn is_login_mode_flag(word: &str) -> bool {
    word.eq_ignore_ascii_case("--browser") || word.eq_ignore_ascii_case("--headless")
}

impl LoginMode {
    /// Take the mode flags out of a `/login` command, wherever they stand.
    fn take(words: &mut Vec<&str>) -> Result<Self> {
        let browser = words
            .iter()
            .any(|word| word.eq_ignore_ascii_case("--browser"));
        let headless = words
            .iter()
            .any(|word| word.eq_ignore_ascii_case("--headless"));
        words.retain(|word| !is_login_mode_flag(word));
        match (browser, headless) {
            (true, true) => bail!("choose one of --browser and --headless"),
            (true, false) => Ok(Self::Browser),
            (false, true) => Ok(Self::Headless),
            (false, false) => Ok(Self::Auto),
        }
    }

    pub(super) fn opens_browser(self) -> bool {
        match self {
            Self::Browser => true,
            Self::Headless => false,
            Self::Auto => !session_has_no_browser(|name| std::env::var_os(name).is_some()),
        }
    }
}

/// A session reached over SSH, or a Linux session without a display, has no
/// browser the person signing in can see.
fn session_has_no_browser(is_set: impl Fn(&str) -> bool) -> bool {
    is_set("SSH_CONNECTION")
        || is_set("SSH_TTY")
        || (cfg!(target_os = "linux") && !is_set("DISPLAY") && !is_set("WAYLAND_DISPLAY"))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRef {
    pub provider: String,
    pub account: String,
    pub method: AuthMethod,
}

impl CredentialRef {
    pub fn is_chatgpt(&self) -> bool {
        self.method.is_chatgpt()
    }

    pub fn is_copilot(&self) -> bool {
        self.provider == "github-copilot" && self.method == AuthMethod::GitHubCopilot
    }

    pub fn is_claude_cli(&self) -> bool {
        self.provider == claude_cli::PROVIDER && self.method == AuthMethod::ClaudeCli
    }
}

impl fmt::Debug for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialRef")
            .field("provider", &self.provider)
            .field("account", &"[redacted]")
            .field("method", &self.method)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct AuthCommandResult {
    pub message: String,
    pub profile: Option<ModelProfile>,
    /// Every profile exposed by a successful account catalog fetch. `profile`
    /// remains the compatibility selection for callers that activate one.
    pub profiles: Vec<ModelProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountStatus {
    pub provider: String,
    pub account: String,
    pub method: AuthMethod,
    pub display: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default = "default_true")]
    pub active: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) fn config_dir() -> Result<std::path::PathBuf> {
    crate::config::get_config_dir()
        .ok_or_else(|| anyhow!("could not locate RustCode config directory"))
}

pub async fn resolve_profile_credential(profile: &ModelProfile) -> Result<Option<String>> {
    let Some(binding) = &profile.credential else {
        return Ok(profile.resolved_api_key());
    };
    validate_binding_shape(binding)?;
    let accounts = load_accounts()?;
    let account = accounts
        .iter()
        .find(|a| {
            a.provider == binding.provider
                && a.account == binding.account
                && a.method == binding.method
        })
        .ok_or_else(|| anyhow!("credential binding is unavailable; sign in again with /login"))?;
    if !account.active {
        bail!("this credential has been signed out; run /login to reconnect it");
    }
    validate_profile_endpoint(profile, account)?;

    match binding.method {
        AuthMethod::ApiKey => {
            resolve_api_key_with_store(profile, account, Arc::new(NativeCredentialStore))
                .await
                .map(Some)
        }
        AuthMethod::ChatGpt => openai::resolve_access_token(binding, account)
            .await
            .map(Some),
        AuthMethod::GitHubCopilot => github_copilot::resolve_access_token(profile, account)
            .await
            .map(Some),
        // The CLI authenticates its own requests; there is nothing to resolve.
        AuthMethod::ClaudeCli => Ok(None),
    }
}

/// Whether an opaque account ID belongs to provider metadata already saved by
/// RustCode. Frontends use this before passing user input into auth commands.
pub fn has_saved_account(provider: &str, account: &str) -> bool {
    load_accounts().is_ok_and(|rows| {
        rows.iter()
            .any(|row| row.provider == provider && row.account == account)
    })
}

pub async fn execute_command(input: &str, config: &AppConfig) -> Result<AuthCommandResult> {
    execute_command_with_progress(
        input,
        config,
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
}

pub async fn execute_command_with_progress(
    input: &str,
    config: &AppConfig,
    cancel: &tokio_util::sync::CancellationToken,
    progress: Option<tokio::sync::mpsc::Sender<String>>,
) -> Result<AuthCommandResult> {
    let mut words = input
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(first) = words.first_mut() {
        *first = first.trim_start_matches('/').to_owned();
    }
    let mut words = words.iter().map(String::as_str).collect::<Vec<_>>();
    let mode = if words.first() == Some(&"login") {
        LoginMode::take(&mut words)?
    } else {
        LoginMode::Auto
    };
    match words.as_slice() {
        ["login"] | ["login", "list"] => Ok(AuthCommandResult {
            message: status_message(config).await?,
            profile: None,
            profiles: Vec::new(),
        }),
        ["auth", "list"] | ["auth", "status"] => Ok(AuthCommandResult {
            message: status_message(config).await?,
            profile: None,
            profiles: Vec::new(),
        }),
        ["accounts"] => Ok(AuthCommandResult {
            message: accounts_message().await?,
            profile: None,
            profiles: Vec::new(),
        }),
        ["account"] => Ok(AuthCommandResult {
            message: account_panel_message(config),
            profile: None,
            profiles: Vec::new(),
        }),
        ["login", provider] if provider.eq_ignore_ascii_case("github-copilot") => {
            github_copilot::login(config, None, cancel, progress, mode).await
        }
        ["login", provider, account] if provider.eq_ignore_ascii_case("github-copilot") => {
            github_copilot::login(config, Some(account), cancel, progress, mode).await
        }
        ["login", provider] | ["refresh", provider] | ["account", "refresh", provider]
            if provider.eq_ignore_ascii_case(claude_cli::PROVIDER) =>
        {
            claude_cli::login(config, cancel).await
        }
        ["account", "refresh"] | ["refresh"]
            if config
                .models
                .iter()
                .find(|profile| profile.name == config.default.big())
                .and_then(|profile| profile.credential.as_ref())
                .is_some_and(CredentialRef::is_claude_cli) =>
        {
            claude_cli::login(config, cancel).await
        }
        ["account", "refresh"] | ["refresh"]
            if config
                .models
                .iter()
                .find(|profile| profile.name == config.default.big())
                .and_then(|profile| profile.credential.as_ref())
                .is_some_and(CredentialRef::is_copilot) =>
        {
            github_copilot::refresh_catalog(config, None)
                .await?
                .ok_or_else(|| anyhow!("select an active Copilot account"))
        }
        ["account", "refresh", provider] | ["refresh", provider]
            if provider.eq_ignore_ascii_case("github-copilot") =>
        {
            github_copilot::refresh_catalog(config, None)
                .await?
                .ok_or_else(|| anyhow!("select an active Copilot account"))
        }
        ["account", "refresh", provider, account] | ["refresh", provider, account]
            if provider.eq_ignore_ascii_case("github-copilot") =>
        {
            github_copilot::refresh_catalog(config, Some(account))
                .await?
                .ok_or_else(|| anyhow!("no active Copilot account has that account ID"))
        }
        ["account", "refresh"] | ["refresh"] => openai::refresh_catalog(config, None)
            .await?
            .ok_or_else(|| anyhow!("select an active ChatGPT profile or provide its account ID")),
        ["account", "refresh", provider] | ["refresh", provider] => {
            openai::refresh_catalog_for_provider(config, provider, None).await
        }
        ["account", "refresh", provider, account] | ["refresh", provider, account] => {
            openai::refresh_catalog_for_provider(config, provider, Some(account)).await
        }
        ["login", provider] if provider.eq_ignore_ascii_case("openai") => {
            if let Some(result) = openai::refresh_catalog(config, None).await? {
                return Ok(result);
            }
            let guard = openai::login_guard().await?;
            openai::login(config, None, guard, mode, progress).await
        }
        ["login", provider, "new"] if provider.eq_ignore_ascii_case("openai") => {
            let guard = openai::login_guard().await?;
            openai::login(config, Some("new"), guard, mode, progress).await
        }
        ["login", provider] => {
            let definition = provider_definition(config, provider)?;
            if definition.auth_methods == [AuthMethod::ApiKey] {
                let env = definition.api_key_env.as_deref().ok_or_else(|| anyhow!("provider has no default API-key environment variable; use /login <provider> api-key <ENV_VAR>"))?;
                store_api_key(config, &definition.id, env).await
            } else {
                bail!("specify an authentication method, such as api-key <ENV_VAR>");
            }
        }
        ["login", provider, account] if provider.eq_ignore_ascii_case("openai") => {
            let guard = openai::login_guard().await?;
            openai::login(config, Some(account), guard, mode, progress).await
        }
        ["login", provider, method, env_var] if method.eq_ignore_ascii_case("api-key") => {
            store_api_key(config, provider, env_var).await
        }
        ["logout", provider] => logout(provider, None).await,
        ["logout", provider, account] => logout(provider, Some(account)).await,
        _ => bail!(
            "use /login [list], /login openai [account|new] [--browser|--headless], /login github-copilot [account|new] [--browser|--headless], /login claude, /login <provider> api-key <ENV_VAR>, /auth status, /accounts, /account, /refresh [provider] [account], or /logout <provider> [account]"
        ),
    }
}

async fn store_api_key(
    config: &AppConfig,
    provider: &str,
    env_var: &str,
) -> Result<AuthCommandResult> {
    let definition = provider_definition(config, provider)?;
    if !definition.auth_methods.contains(&AuthMethod::ApiKey) {
        bail!("provider does not support API-key authentication");
    }
    let endpoint = definition.base_url.as_deref().ok_or_else(|| {
        anyhow!("provider has no base_url; configure one before storing credentials")
    })?;
    validate_base_url(endpoint)?;
    if !valid_env_name(env_var) {
        bail!("provide the name of an environment variable, not a literal secret");
    }
    let secret = crate::shell_env::env_var(env_var)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| anyhow!("the named environment variable is unset or empty"))?;
    let account = "api-key-default";
    let metadata = AccountStatus {
        provider: definition.id.clone(),
        account: account.into(),
        method: AuthMethod::ApiKey,
        display: "API key".into(),
        endpoint: endpoint.into(),
        client_id: None,
        scopes: Vec::new(),
        expires_at: None,
        active: true,
    };
    let old_key = secret_store()
        .get(&definition.id, account, "api-key")
        .await
        .ok();
    let old_endpoint = secret_store()
        .get(&definition.id, account, "endpoint")
        .await
        .ok();
    secret_store()
        .set(&definition.id, account, "api-key", secret)
        .await?;
    if let Err(error) = secret_store()
        .set(&definition.id, account, "endpoint", endpoint.to_owned())
        .await
    {
        if let Some(old) = old_key {
            let _ = secret_store()
                .set(&definition.id, account, "api-key", old)
                .await;
        } else {
            let _ = secret_store()
                .delete(&definition.id, account, "api-key")
                .await;
        }
        return Err(error);
    }
    if let Err(error) = upsert_account(metadata) {
        if let Some(old) = old_key {
            let _ = secret_store()
                .set(&definition.id, account, "api-key", old)
                .await;
        } else {
            let _ = secret_store()
                .delete(&definition.id, account, "api-key")
                .await;
        }
        if let Some(old) = old_endpoint {
            let _ = secret_store()
                .set(&definition.id, account, "endpoint", old)
                .await;
        } else {
            let _ = secret_store()
                .delete(&definition.id, account, "endpoint")
                .await;
        }
        return Err(error);
    }
    let matching = config
        .models
        .iter()
        .filter(|profile| url_fits_base(&profile.url, endpoint))
        .collect::<Vec<_>>();
    let selected = config
        .models
        .iter()
        .find(|profile| {
            profile.name == config.default.big() && url_fits_base(&profile.url, endpoint)
        })
        .or_else(|| {
            if matching.len() == 1 {
                matching.first().copied()
            } else {
                None
            }
        });
    let profile = selected.cloned().map(|mut profile| {
        profile.credential = Some(CredentialRef {
            provider: definition.id.clone(),
            account: account.into(),
            method: AuthMethod::ApiKey,
        });
        profile.api_key = None;
        profile.env_key = None;
        profile
    });
    let message = if profile.is_some() {
        format!(
            "Stored {} API key securely and bound the matching profile.",
            definition.display_name
        )
    } else {
        format!(
            "Stored {} API key securely. Add a model profile using {} and set its credential reference to provider='{}', account='{}', method='api_key'.",
            definition.display_name, endpoint, definition.id, account
        )
    };
    Ok(AuthCommandResult {
        message,
        profile: profile.clone(),
        profiles: profile.into_iter().collect(),
    })
}

async fn logout(provider: &str, account: Option<&str>) -> Result<AuthCommandResult> {
    let claude_cli = provider.eq_ignore_ascii_case(claude_cli::PROVIDER);
    if claude_cli {
        crate::network::claude_cli::shutdown_all();
    }
    let _copilot_lock = if provider.eq_ignore_ascii_case("github-copilot") {
        github_copilot::cancel_login();
        Some(github_copilot::account_lock().await)
    } else {
        None
    };
    let rows = load_accounts()?;
    let matches = rows
        .into_iter()
        .filter(|row| {
            row.provider.eq_ignore_ascii_case(provider)
                && account.is_none_or(|id| row.account == id)
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(AuthCommandResult {
            message: "No matching saved account.".into(),
            profile: None,
            profiles: Vec::new(),
        });
    }
    let mut removed = Vec::new();
    let mut cleanup_failed = false;
    for row in matches {
        let _account_lock = if row.method == AuthMethod::ChatGpt {
            Some(openai::account_lock(&row.account).await?)
        } else {
            None
        };
        if row.method == AuthMethod::ChatGpt {
            // Revoke while holding the same per-account process lock used by
            // refresh; local deletion always completes even on network error.
            let revoke = openai::revoke_locked(&row).await;
            if revoke.is_err() {
                removed.push(format!("{} (remote revocation not confirmed)", row.account));
            }
        }
        for kind in [
            "api-key",
            "endpoint",
            "access-token",
            "refresh-token",
            "id-token",
            "copilot-token",
        ] {
            if secret_store()
                .delete(&row.provider, &row.account, kind)
                .await
                .is_err()
            {
                cleanup_failed = true;
            }
        }
        let mut signed_out = row.clone();
        signed_out.active = false;
        signed_out.expires_at = None;
        upsert_account(signed_out)?;
        if !removed.iter().any(|s| s.starts_with(&row.account)) {
            removed.push(row.account);
        }
    }
    let status = if claude_cli {
        " The Claude Code CLI itself stays signed in; run `claude auth logout` to sign it out."
    } else if cleanup_failed {
        " Local keychain cleanup was incomplete; the account is disabled in RustCode."
    } else {
        ""
    };
    Ok(AuthCommandResult {
        message: format!("Signed out: {}.{status}", removed.join(", ")),
        profile: None,
        profiles: Vec::new(),
    })
}

async fn status_message(config: &AppConfig) -> Result<String> {
    let rows = load_accounts()?;
    Ok(status_message_for(config, &rows))
}

fn status_message_for(config: &AppConfig, rows: &[AccountStatus]) -> String {
    let methods = config
        .providers
        .iter()
        .map(|provider| {
            format!(
                "{} ({})",
                provider.id,
                provider
                    .auth_methods
                    .iter()
                    .map(|method| auth_method_label(*method))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let accounts = if rows.is_empty() {
        "No saved provider accounts.".into()
    } else {
        format!(
            "Saved provider accounts:\n{}",
            rows.iter()
                .map(|row| format!(
                    "- {} / {} [{}] ({}, {})",
                    row.provider,
                    row.display,
                    row.account,
                    auth_method_label(row.method),
                    if row.active {
                        "connected"
                    } else {
                        "signed out"
                    }
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        "Configured providers and methods: {methods}\n{accounts}\nUse /accounts to list accounts or /account for the active profile."
    )
}

pub fn provider_summary(config: &AppConfig) -> String {
    let Some(profile) = config
        .models
        .iter()
        .find(|p| p.name == config.default.big())
    else {
        return format!(
            "Active profile '{}' is not configured.",
            config.default.big()
        );
    };
    let Some(binding) = profile.credential.as_ref() else {
        return format!(
            "Profile: {}\nProvider: {}\nAccount: none (profile-configured)\nMethod: configured endpoint\nState: configured",
            profile.name,
            profile.engine.as_deref().unwrap_or("custom provider")
        );
    };
    let status = load_accounts().ok().and_then(|rows| {
        rows.into_iter().find(|row| {
            row.provider == binding.provider
                && row.account == binding.account
                && row.method == binding.method
        })
    });
    let state = match status.as_ref() {
        Some(row) if row.active => "connected",
        Some(_) => "signed out",
        None => "unavailable",
    };
    let display = status
        .as_ref()
        .map(|row| row.display.as_str())
        .unwrap_or("saved account");
    format!(
        "Profile: {}\nProvider: {}\nAccount: {} ({})\nMethod: {}\nState: {}",
        profile.name,
        binding.provider,
        display,
        binding.account,
        auth_method_label(binding.method),
        state
    )
}

/// `/account` panel text: the summary plus the catalog refresh command for
/// accounts whose model list comes from the provider.
fn account_panel_message(config: &AppConfig) -> String {
    let summary = provider_summary(config);
    let refreshable = config
        .models
        .iter()
        .find(|p| p.name == config.default.big())
        .and_then(|profile| profile.credential.as_ref())
        .is_some_and(|binding| {
            matches!(
                binding.method,
                AuthMethod::ChatGpt | AuthMethod::GitHubCopilot | AuthMethod::ClaudeCli
            )
        });
    if refreshable {
        format!("{summary}\n\nRefresh the model catalog: /refresh")
    } else {
        summary
    }
}

pub fn provider_usage_summary(config: &AppConfig) -> String {
    let active = config
        .models
        .iter()
        .find(|profile| profile.name == config.default.big());
    let provider = active
        .and_then(|profile| {
            profile
                .credential
                .as_ref()
                .map(|binding| binding.provider.as_str())
        })
        .or_else(|| active.and_then(|profile| profile.engine.as_deref()))
        .unwrap_or("selected provider");
    if active
        .and_then(|profile| profile.credential.as_ref())
        .is_some_and(CredentialRef::is_copilot)
    {
        return "GitHub Copilot subscription usage, AI credits, and model access are managed by GitHub. Local token totals do not measure remaining Copilot quota; check your GitHub Copilot usage settings.".into();
    }
    if active
        .and_then(|profile| profile.credential.as_ref())
        .is_some_and(CredentialRef::is_claude_cli)
    {
        return "Claude usage and plan limits are managed by Anthropic and counted against the account signed in to the Claude Code CLI. Local token totals do not measure remaining plan quota; the limit windows the CLI reports appear here after a response.".into();
    }
    if active
        .and_then(|profile| profile.credential.as_ref())
        .is_some_and(CredentialRef::is_chatgpt)
    {
        "ChatGPT subscription usage and limits are managed by OpenAI; RustCode does not expose quota data. See ChatGPT Settings → Usage: https://chatgpt.com/settings/usage. API-key usage is billed separately through the OpenAI API platform.".into()
    } else {
        format!(
            "Usage and billing for {provider} are managed by that provider's API platform. Local session token totals do not report provider quota or billing."
        )
    }
}

/// Merge a fetched account catalog into the user's profiles without replacing
/// their names or per-model preferences. Returns the profile selected for the
/// active/default model.
pub fn apply_auth_result(
    config: &mut AppConfig,
    result: &AuthCommandResult,
) -> Option<ModelProfile> {
    if result.profiles.is_empty() {
        if let Some(profile) = &result.profile {
            upsert_profile(config, profile.clone());
            config.default.set_big(profile.name.clone());
            return config
                .models
                .iter()
                .find(|p| p.name == profile.name)
                .cloned();
        }
        return None;
    }

    let binding = result
        .profile
        .as_ref()
        .and_then(|profile| profile.credential.as_ref())
        .or_else(|| {
            result
                .profiles
                .first()
                .and_then(|profile| profile.credential.as_ref())
        })?;
    let mut merged = Vec::with_capacity(result.profiles.len());
    let mut legacy_renames = Vec::new();
    for incoming in &result.profiles {
        if !incoming.credential.as_ref().is_some_and(|candidate| {
            candidate.provider == binding.provider
                && candidate.account == binding.account
                && candidate.method == binding.method
        }) {
            continue;
        }
        let existing_index = config.models.iter().position(|candidate| {
            candidate.model == incoming.model
                && candidate
                    .credential
                    .as_ref()
                    .is_some_and(|candidate_binding| {
                        candidate_binding.provider == binding.provider
                            && candidate_binding.account == binding.account
                            && candidate_binding.method == binding.method
                    })
        });
        let (mut profile, legacy_old_name) = if let Some(index) = existing_index {
            let existing = config.models[index].clone();
            let legacy_generated =
                is_legacy_generated_name(&existing.name, &incoming.model, &binding.account);
            let mut profile = incoming.clone();
            preserve_profile_settings(&existing, &mut profile);
            if !legacy_generated {
                profile.name = existing.name.clone();
            }
            (profile, legacy_generated.then_some(existing.name))
        } else {
            (incoming.clone(), None)
        };
        if name_is_taken_by_other_account(config, &profile.name, &incoming.model, binding) {
            profile.name =
                unique_account_scoped_name(config, &incoming.model, &binding.account, binding);
        }
        if let Some(old_name) = legacy_old_name {
            legacy_renames.push((old_name, profile.name.clone()));
        }
        upsert_profile(config, profile.clone());
        merged.push(profile);
    }
    for (old_name, new_name) in legacy_renames {
        if config.default.big() == old_name {
            config.default.set_big(new_name.clone());
        }
        if config.default.small() == old_name {
            config.default.set_small(new_name);
        }
    }
    if merged.is_empty() {
        return None;
    }
    if binding.is_copilot() || binding.is_claude_cli() {
        config.models.retain(|candidate| {
            candidate.credential.as_ref() != Some(binding)
                || merged.iter().any(|model| model.model == candidate.model)
        });
    }

    let preferred_model = config
        .models
        .iter()
        .find(|profile| profile.name == config.default.big())
        .filter(|profile| {
            profile.credential.as_ref().is_some_and(|current| {
                current.provider == binding.provider
                    && current.account == binding.account
                    && current.method == binding.method
            })
        })
        .map(|profile| profile.model.as_str())
        .or_else(|| {
            result
                .profile
                .as_ref()
                .map(|profile| profile.model.as_str())
        });
    let selected = preferred_model
        .and_then(|model| merged.iter().find(|profile| profile.model == model))
        .or_else(|| merged.first())?
        .clone();
    config.default.set_big(selected.name.clone());
    Some(selected)
}

fn upsert_profile(config: &mut AppConfig, profile: ModelProfile) {
    let index = config
        .models
        .iter()
        .position(|candidate| {
            candidate.model == profile.model && candidate.credential == profile.credential
        })
        .or_else(|| {
            config.models.iter().position(|candidate| {
                candidate.name == profile.name
                    && candidate.model == profile.model
                    && candidate.url == profile.url
                    && candidate.credential.is_none()
            })
        });
    if let Some(existing) = index.and_then(|index| config.models.get_mut(index)) {
        *existing = profile;
    } else {
        config.models.push(profile);
    }
}

fn preserve_profile_settings(existing: &ModelProfile, profile: &mut ModelProfile) {
    let name = profile.name.clone();
    let url = profile.url.clone();
    let model = profile.model.clone();
    let credential = profile.credential.clone();
    let engine = profile.engine.clone();
    let tool_protocol = profile.tool_protocol.clone();
    let api_protocol = profile.api_protocol;
    let provider_context_window = profile.provider_context_window;
    let supports_reasoning_effort = profile.supports_reasoning_effort;
    let reasoning_efforts = profile.reasoning_efforts.clone();
    let max_output_tokens = profile.max_output_tokens;
    let supports_thinking_budget = profile.supports_thinking_budget;
    let hard_effective_limit = profile.hard_effective_limit;
    let supports_vision = profile.supports_vision;
    let context_window = profile.context_window;
    *profile = existing.clone();
    profile.name = name;
    profile.url = url;
    profile.model = model;
    profile.credential = credential;
    profile.api_key = None;
    profile.env_key = None;
    profile.engine = existing.engine.clone().or(engine);
    profile.api_protocol = api_protocol;
    profile.provider_context_window = provider_context_window;
    profile.supports_reasoning_effort = supports_reasoning_effort;
    profile.reasoning_efforts = reasoning_efforts;
    profile.supports_thinking_budget = supports_thinking_budget;
    profile.hard_effective_limit = existing
        .hard_effective_limit
        .or(hard_effective_limit)
        .map(|limit| hard_effective_limit.map_or(limit, |max| limit.min(max)));
    profile.max_output_tokens = existing
        .max_output_tokens
        .or(max_output_tokens)
        .map(|limit| max_output_tokens.map_or(limit, |max| limit.min(max)));
    profile.tool_protocol = existing.tool_protocol.clone().or(tool_protocol);
    profile.supports_vision = if profile
        .credential
        .as_ref()
        .is_some_and(CredentialRef::is_copilot)
    {
        supports_vision
    } else {
        existing.supports_vision.or(supports_vision)
    };
    profile.context_window = existing.context_window.or(context_window);
}

fn is_legacy_generated_name(name: &str, model: &str, account: &str) -> bool {
    let slug = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let suffix = account.chars().take(10).collect::<String>();
    name == format!("openai-chatgpt-{slug}-{suffix}")
}

fn name_is_taken_by_other_account(
    config: &AppConfig,
    name: &str,
    model: &str,
    binding: &CredentialRef,
) -> bool {
    config.models.iter().any(|candidate| {
        candidate.name == name
            && !(candidate.model == model
                && candidate.credential.as_ref().is_some_and(|other| {
                    other.provider == binding.provider
                        && other.account == binding.account
                        && other.method == binding.method
                }))
    })
}

fn unique_account_scoped_name(
    config: &AppConfig,
    model: &str,
    account: &str,
    binding: &CredentialRef,
) -> String {
    let slug = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let account_suffix = account
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>();
    let prefix = if binding.is_copilot() {
        "copilot"
    } else if binding.is_claude_cli() {
        "claude"
    } else {
        "chatgpt"
    };
    let base = format!("{prefix}/{slug}-{account_suffix}");
    let mut name = base.clone();
    let mut suffix = 2;
    while name_is_taken_by_other_account(config, &name, model, binding) {
        name = format!("{base}-{suffix}");
        suffix += 1;
    }
    name
}

async fn accounts_message() -> Result<String> {
    let rows = load_accounts()?;
    if rows.is_empty() {
        return Ok("No saved provider accounts.".into());
    }
    Ok(format!(
        "Saved provider accounts:\n{}",
        rows.iter()
            .map(|row| format!(
                "- {} / {} [{}] ({}, {})",
                row.provider,
                row.display,
                row.account,
                auth_method_label(row.method),
                if row.active {
                    "connected"
                } else {
                    "signed out"
                }
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn auth_method_label(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::ApiKey => "api-key",
        AuthMethod::ChatGpt => "ChatGPT",
        AuthMethod::GitHubCopilot => "GitHub Copilot",
        AuthMethod::ClaudeCli => "Claude Code CLI",
    }
}

fn provider_definition<'a>(config: &'a AppConfig, name: &str) -> Result<&'a ProviderDefinition> {
    config
        .providers
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow!("unknown provider; configure it in the user config first"))
}

fn validate_binding_shape(binding: &CredentialRef) -> Result<()> {
    if binding.provider.trim().is_empty()
        || binding.provider.len() > 128
        || binding.account.trim().is_empty()
        || binding.account.len() > 256
    {
        bail!("invalid provider credential binding");
    }
    Ok(())
}

fn validate_profile_endpoint(profile: &ModelProfile, account: &AccountStatus) -> Result<()> {
    let profile_url = url::Url::parse(&profile.url).context("profile URL is invalid")?;
    let base_url =
        url::Url::parse(&account.endpoint).context("credential provider endpoint is invalid")?;
    if profile_url.scheme() != base_url.scheme()
        || profile_url.host_str() != base_url.host_str()
        || profile_url.port_or_known_default() != base_url.port_or_known_default()
    {
        bail!("profile endpoint does not match the endpoint bound to this credential");
    }
    let base_path = base_url.path().trim_end_matches('/');
    let path = profile_url.path();
    if !path.starts_with(base_path)
        || (path.len() > base_path.len() && !path[base_path.len()..].starts_with('/'))
    {
        bail!("profile endpoint does not match the endpoint bound to this credential");
    }
    if !profile_url.username().is_empty()
        || profile_url.password().is_some()
        || profile_url.query().is_some()
        || profile_url.fragment().is_some()
    {
        bail!("credential-bound profile URLs cannot contain userinfo, query, or fragment data");
    }
    if account.method == AuthMethod::GitHubCopilot {
        if account.provider != "github-copilot" {
            bail!("Copilot credentials require the github-copilot provider");
        }
        github_copilot::validate_endpoint(&account.endpoint)?;
        let allowed = ["/chat/completions", "/responses", "/v1/messages"];
        if !allowed.contains(&profile_url.path())
            || !allowed.contains(&url::Url::parse(&profile.endpoint_url())?.path())
        {
            bail!("Copilot credentials require a supported model endpoint");
        }
    }
    if account.method == AuthMethod::ClaudeCli
        && (account.provider != claude_cli::PROVIDER
            || account.endpoint != claude_cli::ENDPOINT
            || profile.url != claude_cli::ENDPOINT)
    {
        bail!("Claude Code CLI profiles cannot be pointed at a network endpoint");
    }
    if account.method == AuthMethod::ChatGpt && !is_canonical_chatgpt_responses_url(&profile.url) {
        bail!("ChatGPT credentials may only be used with https://api.openai.com/v1/responses");
    }
    Ok(())
}

pub fn is_canonical_chatgpt_responses_url(value: &str) -> bool {
    value == "https://api.openai.com/v1/responses"
}

fn url_fits_base(value: &str, base: &str) -> bool {
    let (Ok(url), Ok(base_url)) = (url::Url::parse(value), url::Url::parse(base)) else {
        return false;
    };
    let url_path = url.path().trim_end_matches('/');
    let base_path = base_url.path().trim_end_matches('/');
    url.scheme() == base_url.scheme()
        && url.host_str() == base_url.host_str()
        && url.port_or_known_default() == base_url.port_or_known_default()
        && (url_path == base_path
            || url_path
                .strip_prefix(base_path)
                .is_some_and(|suffix| suffix.starts_with('/')))
}

fn validate_base_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).context("provider base_url is invalid")?;
    let loopback_http = url.scheme() == "http"
        && matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        );
    if (url.scheme() != "https" && !loopback_http)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!(
            "provider base_url must use HTTPS (HTTP is allowed for loopback providers only) and omit credentials, query, and fragment"
        );
    }
    Ok(())
}

fn valid_env_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !value.starts_with(|c: char| c.is_ascii_digit())
}

async fn store_get(
    store: Arc<dyn CredentialStore>,
    provider: String,
    account: String,
    kind: String,
) -> Result<String> {
    tokio::task::spawn_blocking(move || store.get_secret(&provider, &account, &kind))
        .await
        .context("credential-store worker stopped")?
}

async fn resolve_api_key_with_store(
    profile: &ModelProfile,
    account: &AccountStatus,
    store: Arc<dyn CredentialStore>,
) -> Result<String> {
    validate_profile_endpoint(profile, account)?;
    let endpoint = store_get(
        store.clone(),
        account.provider.clone(),
        account.account.clone(),
        "endpoint".into(),
    )
    .await?;
    if endpoint != account.endpoint {
        bail!("stored API-key endpoint binding does not match provider metadata");
    }
    store_get(
        store,
        account.provider.clone(),
        account.account.clone(),
        "api-key".into(),
    )
    .await
}

fn load_accounts() -> Result<Vec<AccountStatus>> {
    store::load_accounts()
}
fn upsert_account(value: AccountStatus) -> Result<()> {
    store::upsert_account(value)
}
fn secret_store() -> &'static NativeCredentialStore {
    NativeCredentialStore::global()
}

impl ModelProfile {
    pub(crate) fn for_chatgpt(account: &AccountStatus, model: String) -> Self {
        let slug = model
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let name = format!("chatgpt/{slug}");
        Self {
            name,
            url: "https://api.openai.com/v1/responses".into(),
            model,
            engine: Some("openai".into()),
            api_protocol: Some(ApiProtocol::Responses),
            tool_protocol: Some(rustcode_core::ToolProtocol::ApiNative),
            supports_vision: Some(true),
            credential: Some(CredentialRef {
                provider: "openai".into(),
                account: account.account.clone(),
                method: AuthMethod::ChatGpt,
            }),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn login_mode_flags_are_taken_from_any_position() {
        let take = |command: &str| {
            let mut words = command.split_whitespace().collect::<Vec<_>>();
            LoginMode::take(&mut words).map(|mode| (mode, words.join(" ")))
        };
        assert_eq!(
            take("login openai").unwrap(),
            (LoginMode::Auto, "login openai".to_owned())
        );
        assert_eq!(
            take("login openai new --headless").unwrap(),
            (LoginMode::Headless, "login openai new".to_owned())
        );
        assert_eq!(
            take("login --BROWSER github-copilot").unwrap(),
            (LoginMode::Browser, "login github-copilot".to_owned())
        );
        assert!(take("login openai --browser --headless").is_err());
        assert!(LoginMode::Browser.opens_browser());
        assert!(!LoginMode::Headless.opens_browser());
    }

    #[test]
    fn a_remote_or_displayless_session_has_no_browser() {
        let with =
            |set: &'static [&'static str]| session_has_no_browser(|name| set.contains(&name));
        assert!(with(&["SSH_CONNECTION", "DISPLAY"]));
        assert!(with(&["SSH_TTY", "WAYLAND_DISPLAY"]));
        assert!(!with(&["DISPLAY"]));
        assert!(!with(&["WAYLAND_DISPLAY"]));
        // Only Linux needs a display variable to have a browser.
        assert_eq!(with(&[]), cfg!(target_os = "linux"));
    }

    #[tokio::test]
    async fn two_login_modes_are_refused_before_anything_starts() {
        let config = AppConfig::default();
        let error = execute_command("/login openai --browser --headless", &config)
            .await
            .err()
            .expect("two modes")
            .to_string();
        assert!(error.contains("--browser and --headless"), "{error}");
    }

    #[derive(Default)]
    pub(super) struct MemoryStore(Mutex<HashMap<(String, String, String), String>>);

    impl CredentialStore for MemoryStore {
        fn get_secret(&self, provider: &str, account: &str, kind: &str) -> Result<String> {
            self.0
                .lock()
                .unwrap()
                .get(&(provider.into(), account.into(), kind.into()))
                .cloned()
                .ok_or_else(|| anyhow!("missing test credential"))
        }
        fn set_secret(&self, provider: &str, account: &str, kind: &str, value: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert((provider.into(), account.into(), kind.into()), value.into());
            Ok(())
        }
        fn delete_secret(&self, provider: &str, account: &str, kind: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .remove(&(provider.into(), account.into(), kind.into()));
            Ok(())
        }
    }

    #[test]
    fn catalog_refresh_preserves_advertised_chat_protocol_and_provider_limits() {
        let mut config = AppConfig::default();
        let mut profile = ModelProfile {
            name: "copilot/chat".into(),
            model: "chat-model".into(),
            url: "https://api.githubcopilot.com/chat/completions".into(),
            api_protocol: Some(ApiProtocol::ChatCompletions),
            credential: Some(CredentialRef {
                provider: "github-copilot".into(),
                account: "github-1".into(),
                method: AuthMethod::ApiKey,
            }),
            provider_context_window: Some(32000),
            ..Default::default()
        };
        config.models.push(profile.clone());
        profile.provider_context_window = Some(64000);
        apply_auth_result(
            &mut config,
            &AuthCommandResult {
                message: String::new(),
                profile: Some(profile.clone()),
                profiles: vec![profile],
            },
        );
        let refreshed = config
            .models
            .iter()
            .find(|p| p.model == "chat-model")
            .unwrap();
        assert_eq!(refreshed.api_protocol, Some(ApiProtocol::ChatCompletions));
        assert_eq!(refreshed.provider_context_window, Some(64000));
    }

    #[test]
    fn credential_types_serialize_with_stable_snake_case_methods() {
        let binding = CredentialRef {
            provider: "openai".into(),
            account: "acct".into(),
            method: AuthMethod::ChatGpt,
        };
        let json = serde_json::to_string(&binding).unwrap();
        assert!(json.contains("\"chat_gpt\""));
        assert!(binding.method.is_chatgpt());
        assert!(!format!("{binding:?}").contains("acct"));
    }

    #[test]
    fn claude_cli_binding_is_keyless_and_rejects_network_endpoints() {
        let binding = CredentialRef {
            provider: "claude".into(),
            account: "org1".into(),
            method: AuthMethod::ClaudeCli,
        };
        assert!(
            serde_json::to_string(&binding)
                .unwrap()
                .contains("\"claude_cli\"")
        );
        assert!(binding.is_claude_cli() && !binding.is_chatgpt() && !binding.is_copilot());
        let account = AccountStatus {
            provider: "claude".into(),
            account: "org1".into(),
            method: AuthMethod::ClaudeCli,
            display: "Claude pro plan".into(),
            endpoint: claude_cli::ENDPOINT.into(),
            client_id: None,
            scopes: vec![],
            expires_at: None,
            active: true,
        };
        let mut profile = ModelProfile {
            url: claude_cli::ENDPOINT.into(),
            credential: Some(binding),
            ..Default::default()
        };
        validate_profile_endpoint(&profile, &account).unwrap();
        profile.url = "https://api.anthropic.com/v1/messages".into();
        assert!(validate_profile_endpoint(&profile, &account).is_err());
        let mut moved = account.clone();
        moved.endpoint = "https://attacker.example".into();
        profile.url = moved.endpoint.clone();
        assert!(validate_profile_endpoint(&profile, &moved).is_err());

        let status = status_message_for(&AppConfig::default(), &[account]);
        assert!(status.contains("claude (Claude Code CLI)"));
        assert!(status.contains("Claude pro plan"));
    }

    #[test]
    fn oauth_credentials_accept_only_the_canonical_chatgpt_responses_endpoint() {
        assert!(is_canonical_chatgpt_responses_url(
            "https://api.openai.com/v1/responses"
        ));
        for value in [
            "http://api.openai.com/v1/responses",
            "https://api.openai.com/v1/responses?x=1",
            "https://api.openai.com/v1/responses/",
            "https://attacker.example/v1/responses",
            "https://api.openai.com:443/v1/responses",
        ] {
            assert!(
                !is_canonical_chatgpt_responses_url(value),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn api_key_environment_names_cannot_be_literal_secrets() {
        assert!(valid_env_name("OPENAI_API_KEY"));
        assert!(!valid_env_name("sk-test"));
        assert!(!valid_env_name("1SECRET"));
    }

    fn chatgpt_account(account: &str) -> AccountStatus {
        AccountStatus {
            provider: "openai".into(),
            account: account.into(),
            method: AuthMethod::ChatGpt,
            display: format!("Test {account}"),
            endpoint: "https://api.openai.com/v1".into(),
            client_id: Some("oaiapp_fixture".into()),
            scopes: vec![],
            expires_at: Some(99),
            active: true,
        }
    }

    fn catalog_result(account: &AccountStatus, slugs: &[&str]) -> AuthCommandResult {
        let profiles = slugs
            .iter()
            .map(|slug| ModelProfile::for_chatgpt(account, (*slug).into()))
            .collect::<Vec<_>>();
        AuthCommandResult {
            message: "catalog fetched".into(),
            profile: profiles.first().cloned(),
            profiles,
        }
    }

    #[test]
    fn catalog_apply_preserves_custom_name_settings_and_selected_model() {
        let account = chatgpt_account("account-custom-a");
        let mut config = AppConfig::default();
        config.models.clear();
        let mut custom = ModelProfile::for_chatgpt(&account, "gpt-6-astra".into());
        custom.name = "astra chatgpt/gpt-6-astra".into();
        custom.max_tokens = Some(4096);
        custom.reasoning_effort = Some("high".into());
        custom.enable_thinking = Some(true);
        config.default.set_big(custom.name.clone());
        config.models.push(custom);

        let result = catalog_result(&account, &["gpt-6", "gpt-6-astra", "gpt-5"]);
        let selected = apply_auth_result(&mut config, &result).unwrap();
        assert_eq!(selected.name, "astra chatgpt/gpt-6-astra");
        assert_eq!(config.default.big(), selected.name);
        assert_eq!(config.models.len(), 3);
        let preserved = config
            .models
            .iter()
            .find(|p| p.model == "gpt-6-astra")
            .unwrap();
        assert_eq!(preserved.max_tokens, Some(4096));
        assert_eq!(preserved.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(preserved.enable_thinking, Some(true));
        assert_eq!(preserved.url, "https://api.openai.com/v1/responses");
    }

    #[test]
    fn catalog_apply_fills_missing_context_window_and_keeps_configured_one() {
        let account = chatgpt_account("acct-a");
        let mut config = AppConfig::default();
        config.models.clear();
        let unset = ModelProfile::for_chatgpt(&account, "gpt-6".into());
        let mut configured = ModelProfile::for_chatgpt(&account, "gpt-6-astra".into());
        configured.context_window = Some(64_000);
        config.models.push(unset);
        config.models.push(configured);

        let mut result = catalog_result(&account, &["gpt-6", "gpt-6-astra"]);
        for profile in &mut result.profiles {
            profile.context_window = Some(400_000);
        }
        apply_auth_result(&mut config, &result).unwrap();
        let window = |model: &str| {
            config
                .models
                .iter()
                .find(|p| p.model == model)
                .unwrap()
                .context_window
        };
        assert_eq!(window("gpt-6"), Some(400_000));
        assert_eq!(window("gpt-6-astra"), Some(64_000));
    }

    #[test]
    fn catalog_apply_migrates_legacy_name_and_updates_both_defaults() {
        let account = chatgpt_account("acctabcdef1234");
        let mut config = AppConfig::default();
        config.models.clear();
        let mut legacy = ModelProfile::for_chatgpt(&account, "gpt-6-astra".into());
        legacy.name = "openai-chatgpt-gpt-6-astra-acctabcdef".into();
        let unrelated = ModelProfile {
            name: "chatgpt/gpt-6-astra".into(),
            url: "https://other.example/v1/chat/completions".into(),
            model: "gpt-6-astra".into(),
            ..Default::default()
        };
        config.default.set_big(legacy.name.clone());
        config.default.set_small(legacy.name.clone());
        config.models.push(legacy);
        config.models.push(unrelated.clone());

        let selected = apply_auth_result(
            &mut config,
            &catalog_result(&account, &["gpt-6", "gpt-6-astra"]),
        )
        .unwrap();
        assert_eq!(selected.model, "gpt-6-astra");
        assert_eq!(selected.name, "chatgpt/gpt-6-astra-acctabcd");
        assert_eq!(config.default.big(), selected.name);
        assert_eq!(config.default.small(), selected.name);
        assert!(
            config
                .models
                .iter()
                .any(|profile| profile.name == unrelated.name)
        );
        assert_eq!(config.models.len(), 3);
    }

    #[test]
    fn catalog_apply_scopes_names_without_replacing_an_unbound_profile() {
        let first = chatgpt_account("accountx-first");
        let second = chatgpt_account("accountx-second");
        let mut config = AppConfig::default();
        config.models.clear();
        let mut unrelated = ModelProfile {
            name: "chatgpt/gpt-6".into(),
            url: "https://custom.example/v1/chat/completions".into(),
            model: "gpt-6".into(),
            ..Default::default()
        };
        unrelated.max_tokens = Some(777);
        config.models.push(unrelated.clone());
        let mut first_account_profile = ModelProfile::for_chatgpt(&first, "gpt-6".into());
        first_account_profile.name = "chatgpt/gpt-6-accountx".into();
        config.models.push(first_account_profile);
        config.default.set_big("chatgpt/gpt-6".into());

        let selected =
            apply_auth_result(&mut config, &catalog_result(&second, &["gpt-6"])).unwrap();
        assert_eq!(selected.name, "chatgpt/gpt-6-accountx-2");
        assert_eq!(
            config
                .models
                .iter()
                .find(|p| p.name == unrelated.name)
                .unwrap()
                .max_tokens,
            Some(777)
        );
        assert_eq!(
            config.models.iter().filter(|p| p.model == "gpt-6").count(),
            3
        );
    }

    #[test]
    fn provider_summary_reports_binding_without_secret_data() {
        let account = chatgpt_account("copyable-account-id");
        let profile = ModelProfile::for_chatgpt(&account, "gpt-6".into());
        let mut config = AppConfig::default();
        config.default.set_big(profile.name.clone());
        config.models.push(profile);
        let summary = provider_summary(&config);
        assert!(summary.contains("Provider: openai"));
        assert!(summary.contains("Account: saved account (copyable-account-id)"));
        assert!(summary.contains("State: unavailable"));
        assert!(!summary.contains("access-token"));
        assert!(provider_usage_summary(&config).contains("chatgpt.com/settings/usage"));
        assert!(account_panel_message(&config).ends_with(": /refresh"));
        assert!(!account_panel_message(&AppConfig::default()).contains("/refresh"));
    }

    #[test]
    fn login_status_always_shows_methods_and_saved_accounts() {
        let config = AppConfig::default();
        let status = status_message_for(&config, &[chatgpt_account("copyable-id")]);
        assert!(status.contains("Configured providers and methods:"));
        assert!(status.contains("openai"));
        assert!(status.contains("Saved provider accounts:"));
        assert!(status.contains("copyable-id"));
    }

    #[tokio::test]
    async fn api_key_resolution_uses_injected_store_and_enforces_saved_endpoint_binding() {
        let account = AccountStatus {
            provider: "openai".into(),
            account: "api-key-default".into(),
            method: AuthMethod::ApiKey,
            display: "API key".into(),
            endpoint: "https://api.openai.com/v1".into(),
            client_id: None,
            scopes: vec![],
            expires_at: None,
            active: true,
        };
        let profile = ModelProfile {
            url: "https://api.openai.com/v1/responses".into(),
            credential: Some(CredentialRef {
                provider: "openai".into(),
                account: account.account.clone(),
                method: AuthMethod::ApiKey,
            }),
            ..Default::default()
        };
        let store = Arc::new(MemoryStore::default());
        store
            .set_secret("openai", &account.account, "endpoint", &account.endpoint)
            .unwrap();
        store
            .set_secret(
                "openai",
                &account.account,
                "api-key",
                "test-secret-never-logged",
            )
            .unwrap();
        let resolved = resolve_api_key_with_store(&profile, &account, store.clone())
            .await
            .unwrap();
        assert_eq!(resolved, "test-secret-never-logged");
        let mut rewritten = account.clone();
        rewritten.endpoint = "https://attacker.example/v1".into();
        let error = resolve_api_key_with_store(&profile, &rewritten, store)
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("test-secret"));
    }
}
