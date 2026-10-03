use super::{
    AccountStatus, AuthCommandResult, AuthMethod, CredentialRef, CredentialStore,
    NativeCredentialStore, Result, config_dir,
};
use crate::config::{AppConfig, ModelProfile};
use anyhow::{Context, anyhow, bail};
use base64::Engine as _;
use fs2::FileExt as _;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use rand::RngExt as _;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write as StdWrite;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};

const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const REDIRECT_PATH: &str = "/auth/callback";
const FLOW_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize)]
struct SecretTokens {
    access_token: String,
    refresh_token: String,
    id_token: String,
}

impl std::fmt::Debug for SecretTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretTokens([redacted])")
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: u64,
    scope: Option<String>,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
    #[serde(default)]
    revocation_endpoint: Option<String>,
}

#[derive(Deserialize)]
struct IdClaims {
    iss: String,
    sub: String,
    #[serde(rename = "exp")]
    _exp: usize,
    nonce: String,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize)]
struct ModelCatalog {
    models: Vec<CatalogModel>,
}

#[derive(Deserialize)]
struct CatalogModel {
    slug: String,
    #[serde(default)]
    visibility: String,
}

struct Callback {
    code: String,
    state: String,
    client_id: Option<String>,
    error: Option<String>,
}

pub(super) async fn login_guard() -> Result<tokio::sync::OwnedMutexGuard<()>> {
    static LOCK: OnceLock<std::sync::Arc<Mutex<()>>> = OnceLock::new();
    Ok(LOCK
        .get_or_init(|| std::sync::Arc::new(Mutex::new(())))
        .clone()
        .lock_owned()
        .await)
}

pub(super) async fn login(
    config: &AppConfig,
    selected_account: Option<&str>,
    login_guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<AuthCommandResult> {
    if !config
        .providers
        .iter()
        .any(|p| p.id == "openai" && p.auth_methods.contains(&AuthMethod::ChatGpt))
    {
        bail!("OpenAI ChatGPT sign-in is disabled in provider configuration");
    }
    let client = http_client()?;
    let host_id = load_or_create_host_id()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("could not start the local sign-in callback")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}{REDIRECT_PATH}");
    let (state, nonce, verifier) = (random_token(), random_token(), random_token());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));

    let saved = super::load_accounts()?
        .into_iter()
        .filter(|row| row.provider == "openai" && row.method == AuthMethod::ChatGpt)
        .collect::<Vec<_>>();
    let prior = match selected_account {
        Some("new") => None,
        Some(account) => Some(
            saved
                .into_iter()
                .find(|row| row.account == account)
                .ok_or_else(|| anyhow!("no saved ChatGPT registration has that account ID"))?,
        ),
        None => {
            let selected = config
                .models
                .iter()
                .find(|profile| profile.name == config.default.big())
                .and_then(|profile| profile.credential.as_ref())
                .filter(|binding| {
                    binding.provider == "openai" && binding.method == AuthMethod::ChatGpt
                })
                .and_then(|binding| saved.iter().find(|row| row.account == binding.account))
                .cloned();
            if selected.is_some() {
                selected
            } else if saved.len() == 1 {
                saved.into_iter().next()
            } else if saved.is_empty() {
                None
            } else {
                bail!(
                    "multiple ChatGPT accounts are saved; choose one from /auth status or use /login openai new"
                );
            }
        }
    };
    let mut authorize = url::Url::parse(AUTHORIZE)?;
    authorize
        .query_pairs_mut()
        .append_pair(
            "client_id",
            prior
                .as_ref()
                .and_then(|row| row.client_id.as_deref())
                .unwrap_or("dynamic_agent_client"),
        )
        .append_pair("ext_agent_host_id", &host_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("resource", RESOURCE)
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &challenge);
    if prior.is_none() {
        authorize
            .query_pairs_mut()
            .append_pair("agent_name_hint", "RustCode");
    } else if let Some(row) = prior.as_ref() {
        if let Ok(id_token) = super::secret_store()
            .get("openai", &row.account, "id-token")
            .await
        {
            authorize
                .query_pairs_mut()
                .append_pair("id_token_hint", &id_token);
        }
    }

    open::that_detached(authorize.as_str())
        .context("could not open the system browser for ChatGPT sign-in")?;
    let callback = timeout(FLOW_TIMEOUT, receive_callback(listener))
        .await
        .map_err(|_| anyhow!("ChatGPT sign-in timed out; run /login openai to try again"))??;
    validate_callback_state(&callback, &state)?;
    if let Some(error) = callback.error {
        // OAuth error strings are protocol-controlled and are kept to a known
        // safe allowlist to prevent callback content leaking into UI/logs.
        bail!(match error.as_str() {
            "access_denied" => "ChatGPT sign-in was cancelled",
            "server_error" | "temporarily_unavailable" => "OpenAI sign-in could not be completed",
            _ => "OpenAI sign-in returned an error",
        });
    }
    let code = callback.code;
    let client_id = if let Some(prior) = prior.as_ref() {
        if let (Some(callback_id), Some(saved_id)) =
            (callback.client_id.as_deref(), prior.client_id.as_deref())
            && callback_id != saved_id
        {
            bail!("OpenAI returned a client ID for a different ChatGPT registration");
        }
        prior.client_id.clone().ok_or_else(|| {
            anyhow!("saved ChatGPT registration has no client ID; sign in without an account ID")
        })?
    } else {
        callback
            .client_id
            .filter(|id| id.starts_with("oaiapp_"))
            .ok_or_else(|| {
                anyhow!("OpenAI did not return an issued client ID; registration was not completed")
            })?
    };
    tokio::spawn(finish_authorization(
        client,
        client_id,
        code,
        verifier,
        redirect_uri,
        nonce,
        prior,
        login_guard,
    ))
    .await
    .context("ChatGPT login task stopped while completing sign-in")?
}

/// Refresh an already connected account's current model catalog without
/// starting an interactive browser flow. `None` means the caller should
/// continue with normal sign-in because no active account could be selected.
pub(super) async fn refresh_catalog(
    config: &AppConfig,
    requested_account: Option<&str>,
) -> Result<Option<AuthCommandResult>> {
    ensure_chatgpt_enabled(config)?;
    let accounts = super::load_accounts()?
        .into_iter()
        .filter(|row| row.provider == "openai" && row.method == AuthMethod::ChatGpt && row.active)
        .collect::<Vec<_>>();
    let selected = requested_account
        .and_then(|account| accounts.iter().find(|row| row.account == account))
        .or_else(|| {
            let default_binding = config
                .models
                .iter()
                .find(|profile| profile.name == config.default.big())
                .and_then(|profile| profile.credential.as_ref())?;
            accounts.iter().find(|row| {
                row.provider == default_binding.provider && row.account == default_binding.account
            })
        })
        .or_else(|| (accounts.len() == 1).then(|| &accounts[0]));
    let Some(account) = selected else {
        return Ok(None);
    };
    refresh_catalog_for_account(account).await.map(Some)
}

pub(super) async fn refresh_catalog_for_provider(
    config: &AppConfig,
    provider: &str,
    requested_account: Option<&str>,
) -> Result<AuthCommandResult> {
    if !provider.eq_ignore_ascii_case("openai") {
        bail!("catalog refresh is currently supported for the OpenAI ChatGPT provider");
    }
    ensure_chatgpt_enabled(config)?;
    let accounts = super::load_accounts()?
        .into_iter()
        .filter(|row| row.provider == "openai" && row.method == AuthMethod::ChatGpt && row.active)
        .collect::<Vec<_>>();
    let account = if let Some(requested) = requested_account {
        accounts
            .iter()
            .find(|row| row.account == requested)
            .ok_or_else(|| anyhow!("no active ChatGPT account has that account ID"))?
    } else {
        let default_account = config
            .models
            .iter()
            .find(|profile| profile.name == config.default.big())
            .and_then(|profile| profile.credential.as_ref())
            .filter(|binding| binding.provider == "openai" && binding.method == AuthMethod::ChatGpt)
            .and_then(|binding| accounts.iter().find(|row| row.account == binding.account));
        default_account
            .or_else(|| (accounts.len() == 1).then(|| &accounts[0]))
            .ok_or_else(|| anyhow!("select an active ChatGPT profile or provide its account ID"))?
    };
    refresh_catalog_for_account(account).await
}

fn ensure_chatgpt_enabled(config: &AppConfig) -> Result<()> {
    if !config.providers.iter().any(|provider| {
        provider.id == "openai" && provider.auth_methods.contains(&AuthMethod::ChatGpt)
    }) {
        bail!("OpenAI ChatGPT sign-in is disabled in provider configuration");
    }
    Ok(())
}

async fn refresh_catalog_for_account(account: &AccountStatus) -> Result<AuthCommandResult> {
    let profile = ModelProfile::for_chatgpt(account, "catalog-refresh-placeholder".into());
    let binding = profile
        .credential
        .as_ref()
        .ok_or_else(|| anyhow!("ChatGPT account binding is unavailable"))?;
    let access_token = super::resolve_profile_credential(&profile)
        .await?
        .ok_or_else(|| anyhow!("ChatGPT account has no usable access token"))?;
    let models = list_models(&http_client()?, &access_token).await?;
    if models.is_empty() {
        bail!("ChatGPT model catalog returned no displayable models");
    }
    let profiles = models
        .into_iter()
        .map(|model| ModelProfile::for_chatgpt(account, model.slug))
        .collect::<Vec<_>>();
    debug_assert!(profiles.iter().all(|candidate| {
        candidate
            .credential
            .as_ref()
            .is_some_and(|candidate_binding| {
                candidate_binding.provider == binding.provider
                    && candidate_binding.account == binding.account
                    && candidate_binding.method == binding.method
            })
    }));
    Ok(AuthCommandResult {
        message: format!(
            "Refreshed the ChatGPT model catalog ({} models). Use /model to choose one.",
            profiles.len()
        ),
        profile: None,
        profiles,
    })
}

async fn finish_authorization(
    client: reqwest::Client,
    client_id: String,
    code: String,
    verifier: String,
    redirect_uri: String,
    nonce: String,
    prior: Option<AccountStatus>,
    _login_guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<AuthCommandResult> {
    let redirect_uri_for_token = redirect_uri.clone();
    let token = client
        .post(TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id.as_str()),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", redirect_uri_for_token.as_str()),
            ("resource", RESOURCE),
        ])
        .send()
        .await
        .context("could not reach the OpenAI token endpoint")?;
    let token = parse_token_response(token).await?;
    if token
        .token_type
        .as_deref()
        .is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
    {
        bail!("OpenAI returned an unsupported token type");
    }
    let granted = token
        .scope
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    validate_granted_scopes(&granted)?;
    let id_token = token
        .id_token
        .as_deref()
        .ok_or_else(|| anyhow!("OpenAI did not return an ID token"))?;
    let identity = validate_id_token(&client, id_token, &client_id, Some(&nonce)).await?;
    let account = account_id(&client_id, &identity.sub);
    if prior.as_ref().is_some_and(|row| row.account != account) {
        bail!("the ChatGPT identity does not match the selected saved account");
    }
    let expiry = unix_now()?.saturating_add(token.expires_in);
    let catalog = list_models(&client, &token.access_token).await?;
    if catalog.is_empty() {
        bail!(
            "ChatGPT sign-in succeeded, but the account returned no displayable models; no usable profile was created"
        );
    }
    let display = identity
        .email
        .clone()
        .unwrap_or_else(|| format!("ChatGPT account {}", &account[..8]));
    let record = AccountStatus {
        provider: "openai".into(),
        account: account.clone(),
        method: AuthMethod::ChatGpt,
        display,
        endpoint: RESOURCE.into(),
        client_id: Some(client_id.clone()),
        scopes: granted,
        expires_at: Some(expiry),
        active: true,
    };
    let profiles = catalog
        .iter()
        .map(|model| ModelProfile::for_chatgpt(&record, model.slug.clone()))
        .collect::<Vec<_>>();
    save_token_set(
        &account,
        &SecretTokens {
            access_token: token.access_token,
            refresh_token: token
                .refresh_token
                .ok_or_else(|| anyhow!("OpenAI did not return a renewable refresh token"))?,
            id_token: id_token.to_string(),
        },
    )
    .await?;
    if let Err(error) = super::upsert_account(record.clone()) {
        // The authorization code has been consumed and these tokens may be
        // the only renewable credentials. Keep them for recovery even if
        // local account metadata could not be atomically updated.
        return Err(error);
    }
    let profile = profiles[0].clone();
    let message = format!(
        "Signed in to ChatGPT; loaded {} models. Use /model to choose one.",
        profiles.len()
    );
    Ok(AuthCommandResult {
        message,
        profile: Some(profile),
        profiles,
    })
}

pub(super) async fn resolve_access_token(
    binding: &CredentialRef,
    _initial: &AccountStatus,
) -> Result<String> {
    let lock = account_lock(&binding.account).await?;
    let current = super::load_accounts()?
        .into_iter()
        .find(|row| {
            row.provider == binding.provider
                && row.account == binding.account
                && row.method == AuthMethod::ChatGpt
        })
        .ok_or_else(|| anyhow!("ChatGPT account was signed out while waiting to refresh"))?;
    if !current.active {
        bail!("this ChatGPT account was signed out; run /login to reconnect it");
    }
    let current_time = unix_now()?;
    if current
        .expires_at
        .is_some_and(|expires| expires > current_time.saturating_add(60))
    {
        return secret_get(&binding.account, "access-token").await;
    }
    // Once refresh starts, a caller timeout/cancellation must not discard a
    // newly rotated token before it reaches the keychain. The detached task
    // owns the cross-process lock until persistence and metadata are complete.
    let binding = binding.clone();
    tokio::spawn(async move { refresh_locked(binding, current, lock).await })
        .await
        .context("ChatGPT refresh task stopped before saving credentials")?
}

async fn refresh_locked(
    binding: CredentialRef,
    current: AccountStatus,
    _lock: File,
) -> Result<String> {
    let tokens = load_token_set(&binding.account).await?;
    let client_id = current
        .client_id
        .clone()
        .ok_or_else(|| anyhow!("ChatGPT account registration is missing; sign in again"))?;
    let client = http_client()?;
    let response = client
        .post(TOKEN)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id.as_str()),
            ("refresh_token", tokens.refresh_token.as_str()),
            ("resource", RESOURCE),
        ])
        .send()
        .await
        .context("could not reach the OpenAI refresh endpoint")?;
    let refreshed = parse_token_response(response).await?;
    if refreshed
        .token_type
        .as_deref()
        .is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
    {
        bail!("OpenAI returned an unsupported token type");
    }
    let new_refresh = refreshed
        .refresh_token
        .ok_or_else(|| anyhow!("OpenAI did not rotate the refresh token; sign in again"))?;
    let new_scopes = refreshed
        .scope
        .map(|scopes| {
            scopes
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| current.scopes.clone());
    if !new_scopes
        .iter()
        .any(|scope| scope == "chatgpt.tokens.use.direct")
    {
        bail!("ChatGPT plan access is no longer granted; run /login openai");
    }
    let mut updated = current;
    updated.scopes = new_scopes;
    updated.expires_at = Some(unix_now()?.saturating_add(refreshed.expires_in));
    let retained_id_token = if let Some(id_token) = refreshed.id_token {
        let identity = validate_id_token(&client, &id_token, &client_id, None).await?;
        if account_id(&client_id, &identity.sub) != binding.account {
            bail!("refreshed ChatGPT ID token changed account identity");
        }
        id_token
    } else {
        tokens.id_token.clone()
    };
    let replacement = SecretTokens {
        access_token: refreshed.access_token,
        refresh_token: new_refresh,
        id_token: retained_id_token,
    };
    save_token_set(&binding.account, &replacement).await?;
    if let Err(error) = super::upsert_account(updated) {
        // The refresh token has already rotated. Keep the new credential set;
        // rolling back could restore an invalidated token.
        return Err(error);
    }
    Ok(replacement.access_token)
}

pub(super) async fn revoke_locked(account: &AccountStatus) -> Result<()> {
    let tokens = load_token_set(&account.account).await?;
    let client_id = account
        .client_id
        .as_deref()
        .ok_or_else(|| anyhow!("ChatGPT account registration is missing"))?;
    let client = http_client()?;
    let discovery = get_discovery(&client).await?;
    let endpoint = discovery
        .revocation_endpoint
        .ok_or_else(|| anyhow!("OpenAI did not provide a session revocation endpoint"))?;
    validate_auth_endpoint(&endpoint)?;
    let response = client
        .post(endpoint)
        .form(&[
            ("token", tokens.refresh_token.as_str()),
            ("token_type_hint", "refresh_token"),
            ("client_id", client_id),
        ])
        .send()
        .await
        .context("could not reach the OpenAI revocation endpoint")?;
    if !response.status().is_success() {
        bail!(
            "OpenAI could not confirm session revocation (HTTP {})",
            response.status().as_u16()
        );
    }
    Ok(())
}

async fn list_models(client: &reqwest::Client, access_token: &str) -> Result<Vec<CatalogModel>> {
    let response = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(access_token)
        .send()
        .await
        .context("could not retrieve the ChatGPT model catalog")?;
    if !response.status().is_success() {
        bail!(
            "ChatGPT model catalog request failed (HTTP {})",
            response.status().as_u16()
        );
    }
    let catalog: ModelCatalog = response
        .json()
        .await
        .context("ChatGPT returned an invalid model catalog")?;
    Ok(displayable_models(catalog))
}

fn displayable_models(catalog: ModelCatalog) -> Vec<CatalogModel> {
    let mut seen = std::collections::HashSet::new();
    catalog
        .models
        .into_iter()
        .filter(|model| {
            model.visibility == "list"
                && !model.slug.trim().is_empty()
                && seen.insert(model.slug.clone())
        })
        .collect()
}

fn validate_callback_state(callback: &Callback, expected: &str) -> Result<()> {
    if callback.state != expected {
        bail!("ChatGPT sign-in callback state did not match this login attempt");
    }
    Ok(())
}

async fn validate_id_token(
    client: &reqwest::Client,
    token: &str,
    audience: &str,
    nonce: Option<&str>,
) -> Result<IdClaims> {
    let discovery = get_discovery(client).await?;
    let jwks: JwkSet = client
        .get(discovery.jwks_uri)
        .send()
        .await
        .context("could not retrieve OpenAI signing keys")?
        .error_for_status()
        .context("OpenAI signing-key request failed")?
        .json()
        .await
        .context("OpenAI signing keys were invalid")?;
    validate_id_token_with_jwks(token, audience, nonce, &jwks)
}

fn validate_granted_scopes(scopes: &[String]) -> Result<()> {
    for required in [
        "openid",
        "offline_access",
        "resource.invoke",
        "chatgpt.tokens.use.direct",
    ] {
        if !scopes.iter().any(|scope| scope == required) {
            bail!("OpenAI did not grant required ChatGPT plan scope '{required}'");
        }
    }
    Ok(())
}

fn validate_id_token_with_jwks(
    token: &str,
    audience: &str,
    nonce: Option<&str>,
    jwks: &JwkSet,
) -> Result<IdClaims> {
    let header = decode_header(token).context("OpenAI ID token was malformed")?;
    if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
        bail!("OpenAI ID token used an unsupported signing algorithm");
    }
    let key_id = header
        .kid
        .as_deref()
        .ok_or_else(|| anyhow!("OpenAI ID token did not identify a signing key"))?;
    let jwk = jwks
        .find(key_id)
        .ok_or_else(|| anyhow!("OpenAI ID token signing key was not recognized"))?;
    let key = DecodingKey::from_jwk(jwk).context("OpenAI ID token signing key was invalid")?;
    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[audience]);
    let claims = decode::<IdClaims>(token, &key, &validation)
        .context("OpenAI ID token signature or claims were invalid")?
        .claims;
    if claims.iss != ISSUER
        || nonce.is_some_and(|expected| claims.nonce != expected)
        || claims.sub.trim().is_empty()
    {
        bail!("OpenAI ID token identity validation failed");
    }
    Ok(claims)
}

async fn get_discovery(client: &reqwest::Client) -> Result<Discovery> {
    let response = client
        .get(format!("{ISSUER}/.well-known/openid-configuration"))
        .send()
        .await
        .context("could not retrieve OpenAI authentication metadata")?;
    if !response.status().is_success() {
        bail!(
            "OpenAI authentication metadata request failed (HTTP {})",
            response.status().as_u16()
        );
    }
    let discovery: Discovery = response
        .json()
        .await
        .context("OpenAI authentication metadata was invalid")?;
    if discovery.issuer != ISSUER {
        bail!("OpenAI authentication metadata returned an unexpected issuer");
    }
    validate_auth_endpoint(&discovery.jwks_uri)?;
    if let Some(endpoint) = discovery.revocation_endpoint.as_deref() {
        validate_auth_endpoint(endpoint)?;
    }
    Ok(discovery)
}

fn validate_auth_endpoint(value: &str) -> Result<()> {
    let url = url::Url::parse(value)
        .context("OpenAI authentication metadata contained an invalid URL")?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("OpenAI authentication metadata returned an untrusted endpoint");
    }
    Ok(())
}

async fn parse_token_response(response: reqwest::Response) -> Result<TokenResponse> {
    if !response.status().is_success() {
        bail!(
            "OpenAI token request failed (HTTP {})",
            response.status().as_u16()
        );
    }
    response
        .json()
        .await
        .context("OpenAI returned an invalid token response")
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("could not initialize the authentication HTTP client")
}

async fn receive_callback(listener: TcpListener) -> Result<Callback> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .context("local callback listener failed")?;
        let target = read_request_target(&mut stream).await?;
        let callback = parse_callback_target(&target)?;
        let body = if callback.is_some() {
            "Sign-in response received. You may return to RustCode."
        } else {
            "This local RustCode endpoint only handles the sign-in callback."
        };
        let html = format!("<!doctype html><title>RustCode</title><p>{body}</p>");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{html}",
            html.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        if let Some(callback) = callback {
            return Ok(callback);
        }
    }
}

async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Result<String> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while bytes.len() < 16 * 1024 {
        let count = stream
            .read(&mut chunk)
            .await
            .context("could not read the local sign-in callback")?;
        if count == 0 {
            bail!("local sign-in callback ended before its headers completed");
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    if !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        bail!("local sign-in callback headers were too large");
    }
    let request =
        std::str::from_utf8(&bytes).context("local sign-in callback headers were invalid")?;
    let mut fields = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    if fields.next() != Some("GET") {
        return Ok("/".into());
    }
    Ok(fields.next().unwrap_or("/").to_owned())
}

fn parse_callback_target(target: &str) -> Result<Option<Callback>> {
    let parsed = url::Url::parse(&format!("http://127.0.0.1{target}"))?;
    if parsed.path() != REDIRECT_PATH {
        return Ok(None);
    }
    let mut pairs = std::collections::HashMap::new();
    for (key, value) in parsed.query_pairs() {
        if pairs.insert(key.into_owned(), value.into_owned()).is_some() {
            bail!("local sign-in callback repeated a query parameter");
        }
    }
    Ok(Some(Callback {
        code: pairs.remove("code").unwrap_or_default(),
        state: pairs.remove("state").unwrap_or_default(),
        client_id: pairs.remove("client_id"),
        error: pairs.remove("error"),
    }))
}

fn account_id(client_id: &str, subject: &str) -> String {
    let mut digest = Sha256::new();
    digest.update((client_id.len() as u64).to_be_bytes());
    digest.update(client_id.as_bytes());
    digest.update((subject.len() as u64).to_be_bytes());
    digest.update(subject.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.finalize())
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn host_id_path() -> Result<std::path::PathBuf> {
    Ok(config_dir()?.join("provider-host-id"))
}

fn load_or_create_host_id() -> Result<String> {
    let path = host_id_path()?;
    if let Ok(value) = std::fs::read_to_string(&path) {
        let value = value.trim();
        if value.starts_with("urn:uuid:") && value.len() < 80 {
            return Ok(value.to_owned());
        }
    }
    let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("config path has no parent"))?,
    )?;
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(id.as_bytes())?;
            file.sync_all()?;
            Ok(id)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::read_to_string(path)
                .map(|id| id.trim().to_owned())
                .context("could not read stable provider host ID")
        }
        Err(error) => Err(error).context("could not save stable provider host ID"),
    }
}

async fn secret_get(account: &str, kind: &str) -> Result<String> {
    super::secret_store().get("openai", account, kind).await
}

async fn load_token_set(account: &str) -> Result<SecretTokens> {
    Ok(SecretTokens {
        access_token: secret_get(account, "access-token").await?,
        refresh_token: secret_get(account, "refresh-token").await?,
        id_token: secret_get(account, "id-token").await?,
    })
}

async fn save_token_set(account: &str, tokens: &SecretTokens) -> Result<()> {
    let copy = SecretTokens {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        id_token: tokens.id_token.clone(),
    };
    persist_token_set_with_store(Arc::new(NativeCredentialStore), account.to_owned(), copy).await
}

fn spawn_token_persistence(
    store: Arc<dyn CredentialStore>,
    account: String,
    tokens: SecretTokens,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::task::spawn_blocking(move || {
        // The refresh token rotates on success. Persist it first and never
        // restore the previous value if a later keyring write fails.
        store.set_secret("openai", &account, "refresh-token", &tokens.refresh_token)?;
        store.set_secret("openai", &account, "access-token", &tokens.access_token)?;
        store.set_secret("openai", &account, "id-token", &tokens.id_token)
    })
}

async fn persist_token_set_with_store(
    store: Arc<dyn CredentialStore>,
    account: String,
    tokens: SecretTokens,
) -> Result<()> {
    // Dropping this JoinHandle detaches the already-started blocking worker.
    spawn_token_persistence(store, account, tokens)
        .await
        .context("credential-store worker stopped")?
}

pub(super) async fn account_lock(account: &str) -> Result<File> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).context("could not create RustCode config directory")?;
    let hash =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(account.as_bytes()));
    let path = dir.join(format!("provider-refresh-{}.lock", &hash[..16]));
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .context("could not open ChatGPT refresh lock")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let attempt = file
            .try_clone()
            .context("could not clone ChatGPT refresh lock")?;
        match tokio::task::spawn_blocking(move || {
            attempt.try_lock_exclusive()?;
            Ok::<_, std::io::Error>(attempt)
        })
        .await
        .context("ChatGPT refresh lock worker stopped")?
        {
            Ok(locked) => return Ok(locked),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && tokio::time::Instant::now() < deadline =>
            {
                sleep(Duration::from_millis(40)).await
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => bail!(
                "timed out waiting for another RustCode process to refresh this ChatGPT session"
            ),
            Err(_) => bail!("could not acquire ChatGPT refresh lock"),
        }
    }
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use tokio::net::TcpStream;

    const TEST_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC0T5ShUWfZfThm\nkZpi7Pr35+Slje9S3EPhmrpF4U3wcYVojEHhIri93ERZaLTM3lcleAKiCLczeE2e\nW7ZlwIOMlr0JcDOQsh7bXVVGasY2GMNvVfja1+RyvAEp1Sf8mtKObgDLBibHtc9f\nzwU0piQaNzOUyfvfTbPB5QmheUeLbLRSU61MpoEqvpIh6HWxX3kOHhFoLnmbHLRF\nDFpWAHNYno4ECeqmvkEOmElrY8iii8EwEdsJzr6fSgZwTZzeACO5h/wolnXNUH/8\nedoLCmGxCGi5Xd0uOTlfxWbiCsUbdL4e3B4q7JPLVGpFb9ve95NIVIa7Ul9FBLCj\nFAe7mSEDAgMBAAECggEBAITL05BXzx9L7R0FgWn4VQH95NTVSvyAwvHGLghHXkqG\noRWVrvNryhnyvtgGmJoF6rLqxy2lM6ARq0DFFPmtpnUFk6X+38tik/1FqQdani67\nYDyAWe57cIHb2xN/LJsLP6WseKMOHcOaMGfEpXXYIuC35SJg/ELDDG/yCnzFQJ76\nq5d9nfDuLzrZvf1s1uR5d5R/eP8k2le1eDcKjxi6Bu+V/B5MSVhsw+HtPMPRxJMz\nsXpTDLespeeBCLBxbpO8wOKTCo/0QVa+Xlm1sozJ41Ix4KRrWp0zJ34H3mmQt4NS\nSigri7xsRsy6kBNoF51RvmNP1XavPKK1pXyduDWr+IECgYEA7BUs2uJmi7uF2lhA\nJ/YoW8mrYzmabI74UB6nYryWtMPJVuxmB60jo0j8Sti9QHJ0kwSjh2iKu3v03l4o\nY0H8H41WPkccPJ/DsrUipKLfLMXePT1hNWm3R+eo4y4JIvf1CNiBzORpSFFuI5Uo\n1R5l4YFT+j/aw0xMFS2F2gF+csMCgYEAw4Xdti4KQwuUIVqyZ99KQBDk+wqy3nNG\nsQVfu0hX5huKPIxpmBd9j26WuVuJHAr7yE86oUjoP2ODvo3PEyaEpvCuL02fnn3g\nyZumbN3SQsmmFYkWYPViNO14/rkjyiPUKJEqN7CfCbl8N2npHzdz3KiVYBztk0t2\n9fDv80rnNMECgYAiVy4wJLCf8MYWrbGfXnoeZ+ZrR4zD78QE+4CDp0UQxE38O+TX\nhwLhFJPGW2KkBkIYxJr47mcHwI8s7WtYjNecy1VZN8TOuLqhuyFv61UlUR7zr4L9\nXwRPDE6PxTmFAaZ+A+hVooACCf5IZMEMxyAwvjw18aXjtKx4hCetP3xiOwKBgA9e\nEyoBfl78pvzkIweU/kIA0e6FTb+8Mb8yG+8dZYM5gOj3ZElG92BxobkZ37HrjxSU\nXZhVoaNxz+YHQVJRAbYZTqd7I2OSoztVV4RQ/viu3rXsm2ytfLWKQKtMo+p8XG1/\n02CjKizafk/grCj+88VRHsR6IZYlJUl5UXK+3WNBAoGAOAHxq2DVSr5LZ//ucLLU\n0pbMJQQuG/g4cAorB0QaoxDCiBB5ue3e4e9fTWdA5aQkLCAfz7+6DJp78nCcl7QH\nnkJpFs5bvcJsAyVi4R0tPtMdQ7ch6rhNFa2viQGnTYnEoxNs5RohZRL+IU+OGmRL\nzceOckMD07ZUsEw8yq3DDvA=\n-----END PRIVATE KEY-----\n";
    const TEST_MODULUS: &str = "tE-UoVFn2X04ZpGaYuz69-fkpY3vUtxD4Zq6ReFN8HGFaIxB4SK4vdxEWWi0zN5XJXgCogi3M3hNnlu2ZcCDjJa9CXAzkLIe211VRmrGNhjDb1X42tfkcrwBKdUn_JrSjm4AywYmx7XPX88FNKYkGjczlMn7302zweUJoXlHi2y0UlOtTKaBKr6SIeh1sV95Dh4RaC55mxy0RQxaVgBzWJ6OBAnqpr5BDphJa2PIoovBMBHbCc6-n0oGcE2c3gAjuYf8KJZ1zVB__HnaCwphsQhouV3dLjk5X8Vm4grFG3S-HtweKuyTy1RqRW_b3veTSFSGu1JfRQSwoxQHu5khAw";

    #[derive(Default)]
    struct MemoryStore(StdMutex<HashMap<(String, String, String), String>>);

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
    fn callback_code_parser_decodes_values_and_enforces_callback_path() {
        let callback =
            parse_callback_target("/auth/callback?code=abc%2B123&state=state&client_id=oaiapp_123")
                .unwrap()
                .unwrap();
        assert_eq!(callback.code, "abc+123");
        assert_eq!(callback.state, "state");
        assert_eq!(callback.client_id.as_deref(), Some("oaiapp_123"));
        assert!(parse_callback_target("/favicon.ico").unwrap().is_none());
        assert!(parse_callback_target("/auth/callback?state=a&state=b").is_err());
    }

    #[test]
    fn token_debug_redacts_all_token_fields() {
        let tokens = SecretTokens {
            access_token: "secret-a".into(),
            refresh_token: "secret-r".into(),
            id_token: "secret-i".into(),
        };
        let debug = format!("{tokens:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("redacted"));
    }

    #[test]
    fn production_id_token_validator_checks_signature_issuer_audience_expiry_and_nonce() {
        let jwks: JwkSet = serde_json::from_value(serde_json::json!({ "keys": [{
            "kty": "RSA", "use": "sig", "alg": "RS256", "kid": "fixture",
            "n": TEST_MODULUS, "e": "AQAB"
        }]}))
        .unwrap();
        let mut header = jsonwebtoken::Header::new(Algorithm::RS256);
        header.kid = Some("fixture".into());
        let valid = serde_json::json!({
            "iss": ISSUER, "sub": "fixture-subject", "aud": "fixture-client",
            "exp": unix_now().unwrap() + 600, "nonce": "expected-nonce", "email": "test@example.com"
        });
        let encoding_key =
            jsonwebtoken::EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).unwrap();
        let token = jsonwebtoken::encode(&header, &valid, &encoding_key).unwrap();
        let claims =
            validate_id_token_with_jwks(&token, "fixture-client", Some("expected-nonce"), &jwks)
                .unwrap();
        assert_eq!(claims.sub, "fixture-subject");

        for invalid in [
            serde_json::json!({ "iss": "https://attacker.example", "sub": "fixture-subject", "aud": "fixture-client", "exp": unix_now().unwrap() + 600, "nonce": "expected-nonce" }),
            serde_json::json!({ "iss": ISSUER, "sub": "fixture-subject", "aud": "other-client", "exp": unix_now().unwrap() + 600, "nonce": "expected-nonce" }),
            serde_json::json!({ "iss": ISSUER, "sub": "fixture-subject", "aud": "fixture-client", "exp": unix_now().unwrap() - 600, "nonce": "expected-nonce" }),
            serde_json::json!({ "iss": ISSUER, "sub": "fixture-subject", "aud": "fixture-client", "exp": unix_now().unwrap() + 600, "nonce": "wrong-nonce" }),
        ] {
            let invalid = jsonwebtoken::encode(&header, &invalid, &encoding_key).unwrap();
            assert!(
                validate_id_token_with_jwks(
                    &invalid,
                    "fixture-client",
                    Some("expected-nonce"),
                    &jwks
                )
                .is_err()
            );
        }
        let mut tampered = token;
        tampered.push('x');
        assert!(
            validate_id_token_with_jwks(&tampered, "fixture-client", Some("expected-nonce"), &jwks)
                .is_err()
        );
    }

    #[test]
    fn callback_state_validator_rejects_other_login_attempts() {
        let callback = Callback {
            code: "code".into(),
            state: "state-a".into(),
            client_id: None,
            error: None,
        };
        assert!(validate_callback_state(&callback, "state-a").is_ok());
        assert!(validate_callback_state(&callback, "state-b").is_err());
    }

    #[test]
    fn model_catalog_uses_only_displayable_provider_slugs() {
        let catalog: ModelCatalog = serde_json::from_value(serde_json::json!({ "models": [
            {"slug":"hidden","display_name":"Hidden","visibility":"private"},
            {"slug":"","display_name":"No slug","visibility":"list"},
            {"slug":"available-model","display_name":"Available Model","visibility":"list"},
            {"slug":"available-model","display_name":"Duplicate","visibility":"list"},
            {"slug":"later-model","display_name":"Later Model","visibility":"list"}
        ]}))
        .unwrap();
        let available = displayable_models(catalog);
        assert_eq!(available.len(), 2);
        assert_eq!(available[0].slug, "available-model");
        assert_eq!(available[1].slug, "later-model");
    }

    #[tokio::test]
    async fn detached_token_persistence_finishes_after_waiter_is_cancelled() {
        let store = Arc::new(MemoryStore::default());
        let worker = spawn_token_persistence(
            store.clone(),
            "test-account".into(),
            SecretTokens {
                access_token: "new-access".into(),
                refresh_token: "rotated-refresh".into(),
                id_token: "validated-id".into(),
            },
        );
        drop(worker);
        for _ in 0..50 {
            if store
                .get_secret("openai", "test-account", "refresh-token")
                .is_ok()
            {
                break;
            }
            sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(
            store
                .get_secret("openai", "test-account", "refresh-token")
                .unwrap(),
            "rotated-refresh"
        );
        assert_eq!(
            store
                .get_secret("openai", "test-account", "access-token")
                .unwrap(),
            "new-access"
        );
        assert_eq!(
            store
                .get_secret("openai", "test-account", "id-token")
                .unwrap(),
            "validated-id"
        );
    }

    #[tokio::test]
    async fn production_callback_handles_noise_and_segmented_headers_on_loopback() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = tokio::spawn(receive_callback(listener));

        let mut noise = TcpStream::connect(address).await.unwrap();
        noise
            .write_all(b"GET /favicon.ico HTTP/1.1\r\n")
            .await
            .unwrap();
        noise.write_all(b"Host: 127.0.0.1\r\n\r\n").await.unwrap();
        let mut noise_response = vec![0; 512];
        let count = noise.read(&mut noise_response).await.unwrap();
        let response = String::from_utf8_lossy(&noise_response[..count]);
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(body.len(), content_length);

        let mut callback = TcpStream::connect(address).await.unwrap();
        callback.write_all(b"GET /auth/callback?code=abc%2B123&state=state-good&client_id=oaiapp_fixture HTTP/1.1\r\nHost: 127.0.0.1\r\n").await.unwrap();
        tokio::task::yield_now().await;
        callback
            .write_all(b"User-Agent: fixture\r\n\r\n")
            .await
            .unwrap();
        let parsed = timeout(Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        validate_callback_state(&parsed, "state-good").unwrap();
        assert_eq!(parsed.code, "abc+123");
        assert_eq!(parsed.client_id.as_deref(), Some("oaiapp_fixture"));
    }
}
