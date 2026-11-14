//! `_rustcode/outside/*`: Settings › Outside agents (`docs/mcp-outside-agents.md`).
//!
//! The app lists the apps connected to RustCode's MCP tools, makes a grant
//! (its token and the config to paste into the other app come back once),
//! changes or revokes one, and answers the calls that wait for the user.
//! The grants, their scope checks and the audit live in
//! `orchestration/outside.rs`; this only exposes them to a client of the engine.

use serde_json::{Value, json};

use crate::orchestration::outside;

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
    /// `read_only` (default), `delegate` or `manage`.
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

/// How many audited calls the list carries.
const RECENT_CALLS: u32 = 25;

fn path() -> Result<std::path::PathBuf, String> {
    outside::grant_path().ok_or_else(|| "no RustCode configuration directory".to_owned())
}

fn scope_of(text: &str) -> Result<outside::Scope, String> {
    outside::Scope::parse(text)
        .ok_or_else(|| format!("access must be read_only, delegate or manage, not “{text}”"))
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
    Ok(json!({
        "grants": grants,
        "pending": outside::pending_view(),
        "recent": crate::data_store::recent_outside_calls(None, RECENT_CALLS),
        "platforms": outside::PLATFORMS,
        "command": command(),
        "httpUrl": crate::orchestration::outside_http::ensure().await.ok(),
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
