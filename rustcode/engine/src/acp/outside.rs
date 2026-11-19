//! `_rustcode/outside/*`: Settings › Outside agents (`docs/mcp-outside-agents.md`).
//!
//! The app lists the apps connected to RustCode's MCP tools, makes a grant
//! (its token and the config to paste into the other app come back once),
//! changes or revokes one, and answers the calls that wait for the user.
//! The grants, their scope checks and the audit live in
//! `orchestration/outside.rs`; this only exposes them to a client of the engine.

use serde_json::{Value, json};

use crate::orchestration::{outside, outside_signin};

/// `_rustcode/outside/list`: the grants, the calls waiting for an answer, the
/// newest audited calls, and what the setup flow needs.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/list", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsideListRequest {}

/// `_rustcode/outside/create`: connect an app. The answer carries `token` and
/// `snippet`, the only time either is shown.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/create", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsideCreateRequest {
    pub name: String,
    #[serde(default)]
    pub platform: String,
    /// `read_only` (default), `delegate`, `manage` or `admin`.
    #[serde(default)]
    pub scope: Option<String>,
    /// Ask in the app before each change; on by default above read-only.
    #[serde(default)]
    pub ask: Option<bool>,
}

/// `_rustcode/outside/update`: change an app's access, or whether it asks.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/update", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsideUpdateRequest {
    pub id: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub ask: Option<bool>,
}

/// `_rustcode/outside/revoke`: the app's token stops working at once.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/revoke", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsideRevokeRequest {
    pub id: String,
}

/// `_rustcode/outside/answer`: allow or decline a call an app is waiting on.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/answer", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsideAnswerRequest {
    pub id: String,
    pub allow: bool,
}

/// `_rustcode/outside/signin`: approve or decline a sign-in waiting on the
/// public sign-in page. `scope` overrides the level the page asked for.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/signin", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsideSignInRequest {
    pub id: String,
    pub allow: bool,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub ask: Option<bool>,
}

/// `_rustcode/outside/code`: a one-time sign-in code for the public sign-in
/// page, as `rustcode outside code` makes one.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/code", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsideCodeRequest {
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub ask: Option<bool>,
}

/// `_rustcode/outside/passkey/add`: a link that adds an owner passkey on the
/// public sign-in page, as `rustcode outside owner add-passkey` makes one.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/passkey/add", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsidePasskeyAddRequest {
    #[serde(default)]
    pub name: Option<String>,
}

/// `_rustcode/outside/passkey/remove`: the passkey stops approving sign-ins.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/passkey/remove", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsidePasskeyRemoveRequest {
    pub id: String,
}

/// `_rustcode/outside/web/sign-out`: a browser signed in to the web app is
/// signed out; its open socket closes within seconds.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/outside/web/sign-out", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutsideWebSignOutRequest {
    pub id: String,
}

/// `_rustcode/outside/changed`: something now waits for the owner, or no
/// longer does; ask `_rustcode/outside/list` again.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/outside/changed")]
pub(crate) struct OutsideChangedNotification {}

/// Tell every client that listed outside agents when something waiting for
/// the owner changes, once per engine.
pub(crate) fn start_publishing(clients: &super::clients::Clients) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let notify = clients.clone();
    let mut changes = outside::watch_changes();
    tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            notify.publish(
                super::clients::Topic::Outside,
                OutsideChangedNotification {},
            );
        }
    });
}

/// How many audited calls the list carries.
const RECENT_CALLS: u32 = 25;

fn path() -> Result<std::path::PathBuf, String> {
    outside::grant_path().ok_or_else(|| "no RustCode configuration directory".to_owned())
}

fn scope_of(text: &str) -> Result<outside::Scope, String> {
    outside::Scope::parse(text)
        .ok_or_else(|| format!("access must be read_only, delegate, manage or admin, not “{text}”"))
}

/// The command another app runs: this program, by absolute path.
fn command() -> String {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "rustcode".to_owned())
}

pub(crate) async fn list(_request: OutsideListRequest) -> Result<Value, String> {
    // The bridge finds the engine through the hub; opening Settings makes sure
    // it is there to be found.
    outside::ensure_hub().await;
    let grants: Vec<Value> = outside::list().iter().map(outside::Grant::view).collect();
    let public = crate::orchestration::outside_http::configured_public_base();
    Ok(json!({
        "grants": grants,
        "pending": outside::pending_view(),
        "recent": crate::data_store::recent_outside_calls(None, RECENT_CALLS),
        "platforms": outside::PLATFORMS,
        "command": command(),
        "httpUrl": crate::orchestration::outside_http::ensure().await.ok(),
        "signIns": outside_signin::view(),
        "passkeys": crate::orchestration::outside_passkey::list()
            .iter()
            .map(crate::orchestration::outside_passkey::Passkey::view)
            .collect::<Vec<_>>(),
        "webSessions": crate::orchestration::outside_web::list()
            .iter()
            .map(crate::orchestration::outside_web::WebSession::view)
            .collect::<Vec<_>>(),
        "webInstalled": crate::orchestration::outside_web::web_dir().is_some(),
        "publicUrl": public.as_ref().and_then(|p| p.as_ref().ok()).map(|base| format!("{base}/mcp")),
        "publicError": public.and_then(Result::err),
        "machineName": crate::fleet::own_name_override()
            .or_else(crate::remote_gateway::gateway::os_host_name),
        "levels": levels(),
        "maxGrants": outside::MAX_GRANTS,
        "available": cfg!(unix),
    }))
}

pub(crate) async fn create(request: OutsideCreateRequest) -> Result<Value, String> {
    if !cfg!(unix) {
        return Err("outside agents need macOS or Linux".to_owned());
    }
    let scope = match request
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(text) => scope_of(text)?,
        None => outside::Scope::ReadOnly,
    };
    let (grant, token) = outside::create_at(
        &path()?,
        &request.name,
        &request.platform,
        scope,
        request.ask,
    )?;
    outside::ensure_hub().await;
    let snippet = outside::snippet(&grant.platform, &command(), &token);
    Ok(json!({ "grant": grant.view(), "token": token, "snippet": snippet }))
}

pub(crate) fn update(request: OutsideUpdateRequest) -> Result<Value, String> {
    let scope = match request
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(text) => Some(scope_of(text)?),
        None => None,
    };
    match outside::update_at(&path()?, &request.id, scope, request.ask)? {
        Some(grant) => Ok(json!({ "grant": grant.view() })),
        None => Err("no connected app has that id".to_owned()),
    }
}

pub(crate) fn revoke(request: OutsideRevokeRequest) -> Result<Value, String> {
    let revoked = outside::revoke_at(&path()?, &request.id)?;
    // The questions that app has open end with it.
    outside::deny_all_for(&request.id);
    Ok(json!({ "revoked": revoked }))
}

pub(crate) fn answer(request: OutsideAnswerRequest) -> Result<Value, String> {
    Ok(json!({ "answered": outside::answer_pending(&request.id, request.allow) }))
}

/// What each level allows and does not, as the sign-in page lists it.
fn levels() -> Vec<Value> {
    use outside::Scope;
    [
        Scope::ReadOnly,
        Scope::Delegate,
        Scope::Manage,
        Scope::Admin,
    ]
    .into_iter()
    .map(|scope| {
        let (summary, can, cannot) = outside_signin::scope_points(scope);
        json!({
            "scope": scope.as_str(),
            "label": scope.label(),
            "summary": summary,
            "can": can,
            "cannot": cannot,
        })
    })
    .collect()
}

pub(crate) fn sign_in(request: OutsideSignInRequest) -> Result<Value, String> {
    let scope = match request
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(text) => Some(scope_of(text)?),
        None => None,
    };
    Ok(json!({
        "answered": outside_signin::answer(&request.id, request.allow, scope, request.ask)
    }))
}

pub(crate) fn code(request: OutsideCodeRequest) -> Result<Value, String> {
    let made = crate::orchestration::outside_http::sign_in_code(
        request
            .scope
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        request.ask,
    )?;
    Ok(json!({
        "code": made.code,
        "scope": made.scope,
        "minutes": made.minutes,
    }))
}

pub(crate) fn passkey_add(request: OutsidePasskeyAddRequest) -> Result<Value, String> {
    let link = crate::orchestration::outside_passkey::add_link(
        request.name.as_deref().unwrap_or("Passkey"),
    )?;
    Ok(json!({
        "link": link,
        "minutes": crate::orchestration::outside_passkey::ENROLLMENT_SECS / 60,
    }))
}

pub(crate) fn passkey_remove(request: OutsidePasskeyRemoveRequest) -> Result<Value, String> {
    let removed = crate::orchestration::outside_passkey::remove(&request.id)?;
    outside::changed();
    Ok(json!({ "removed": removed.view() }))
}

pub(crate) fn web_sign_out(request: OutsideWebSignOutRequest) -> Result<Value, String> {
    let removed = crate::orchestration::outside_web::remove(&request.id)?;
    Ok(json!({
        "removed": removed.iter().map(crate::orchestration::outside_web::WebSession::view).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_access_level_is_refused_by_name() {
        let error = scope_of("root").unwrap_err();
        assert!(
            error.contains("read_only") && error.contains("root"),
            "{error}"
        );
        assert_eq!(scope_of("delegate").unwrap(), outside::Scope::Delegate);
    }

    #[test]
    fn answering_a_call_that_is_not_waiting_says_so() {
        let answered = answer(OutsideAnswerRequest {
            id: "nothing".to_owned(),
            allow: true,
        })
        .unwrap();
        assert_eq!(answered["answered"], json!(false));
    }
}
