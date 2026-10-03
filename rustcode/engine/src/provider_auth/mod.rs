//! Provider credentials live in the OS credential store. This module keeps
//! only non-secret account bindings in RustCode's owner-only config folder.

mod openai;
mod store;

use crate::config::{ApiProtocol, AppConfig, ModelProfile, ProviderDefinition};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

pub use store::{CredentialStore, NativeCredentialStore};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ApiKey,
    ChatGpt,
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
        })
    }
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
    }
}

pub async fn execute_command(input: &str, config: &AppConfig) -> Result<AuthCommandResult> {
    let mut words = input
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(first) = words.first_mut() {
        *first = first.trim_start_matches('/').to_owned();
    }
    let words = words.iter().map(String::as_str).collect::<Vec<_>>();
    match words.as_slice() {
        ["login"] => Ok(AuthCommandResult {
            message: status_message(config).await?,
            profile: None,
        }),
        ["auth", "list"] | ["auth", "status"] => Ok(AuthCommandResult {
            message: status_message(config).await?,
            profile: None,
        }),
        ["login", provider] if provider.eq_ignore_ascii_case("openai") => {
            let guard = openai::login_guard().await?;
            openai::login(config, None, guard).await
        }
        ["login", provider, "new"] if provider.eq_ignore_ascii_case("openai") => {
            let guard = openai::login_guard().await?;
            openai::login(config, Some("new"), guard).await
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
            openai::login(config, Some(account), guard).await
        }
        ["login", provider, method, env_var] if method.eq_ignore_ascii_case("api-key") => {
            store_api_key(config, provider, env_var).await
        }
        ["logout", provider] => logout(provider, None).await,
        ["logout", provider, account] => logout(provider, Some(account)).await,
        _ => bail!(
            "use /login, /login openai [account|new], /login <provider> api-key <ENV_VAR>, /auth status, or /logout <provider> [account]"
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
    Ok(AuthCommandResult { message, profile })
}

async fn logout(provider: &str, account: Option<&str>) -> Result<AuthCommandResult> {
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
    let status = if cleanup_failed {
        " Local keychain cleanup was incomplete; the account is disabled in RustCode."
    } else {
        ""
    };
    Ok(AuthCommandResult {
        message: format!("Signed out: {}.{status}", removed.join(", ")),
        profile: None,
    })
}

async fn status_message(config: &AppConfig) -> Result<String> {
    let rows = load_accounts()?;
    if rows.is_empty() {
        let methods = config
            .providers
            .iter()
            .map(|p| {
                format!(
                    "{} ({})",
                    p.id,
                    p.auth_methods
                        .iter()
                        .map(|m| match m {
                            AuthMethod::ApiKey => "api-key",
                            AuthMethod::ChatGpt => "ChatGPT",
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Ok(format!(
            "No saved provider accounts. Available methods: {methods}. Use /login openai for ChatGPT sign-in."
        ));
    }
    Ok(format!(
        "Saved provider accounts:\n{}",
        rows.iter()
            .map(|r| format!(
                "- {} / {} [{}] ({}, {})",
                r.provider,
                r.display,
                r.account,
                match r.method {
                    AuthMethod::ApiKey => "api-key",
                    AuthMethod::ChatGpt => "ChatGPT",
                },
                if r.active { "connected" } else { "signed out" }
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ))
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
        let slug = model.clone();
        let name = format!(
            "openai-chatgpt-{}-{}",
            slug.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                })
                .collect::<String>(),
            &account.account[..account.account.len().min(10)]
        );
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

    #[derive(Default)]
    struct MemoryStore(Mutex<HashMap<(String, String, String), String>>);

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
