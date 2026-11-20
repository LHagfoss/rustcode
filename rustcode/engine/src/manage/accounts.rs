//! `rc account|connections|outside …` (#3821): harness accounts, the
//! GitHub and model sign-ins, and the tokens of outside agents, from a
//! terminal only. None of these is a tool: an agent never adds, signs out or
//! mints a credential. Each request goes through the running server when
//! there is one (it owns the live threads and the outside-agent hub), else
//! runs here on the same files.
//!
//! No answer carries a credential. The one exception is `outside create`,
//! whose token is the point of the command and is shown once, as the app's
//! Settings › Outside agents shows it.

use serde_json::{Value, json};

/// The request through the server, else `local`.
async fn ask<F, Fut>(method: &str, params: Value, local: F) -> Result<Value, String>
where
    F: FnOnce(Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    match crate::server::ask_running(method, params.clone()).await {
        crate::server::Asked::Answer(answer) => Ok(answer),
        crate::server::Asked::Refused(message) => Err(message),
        crate::server::Asked::NoServer(_) => local(params).await,
    }
}

fn field(params: &Value, key: &str) -> String {
    params[key].as_str().unwrap_or_default().to_owned()
}

/// Every harness's named accounts: id, name, folder and sign-in state.
fn account_rows(snapshot: &Value) -> Value {
    let rows: Vec<Value> = snapshot["harnesses"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|harness| {
            harness["accounts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |account| {
                    json!({
                        "harness": harness["id"],
                        "id": account["id"],
                        "name": account["name"],
                        "config_dir": account["config_dir"],
                        "auth": account["auth"],
                    })
                })
        })
        .collect();
    json!({ "accounts": rows })
}

pub async fn list_accounts() -> Result<Value, String> {
    let snapshot = ask("_rustcode/harnesses", json!({}), |_| async {
        crate::harness::snapshot().await
    })
    .await?;
    Ok(account_rows(&snapshot))
}

/// Look every account's sign-in up again; nothing is started.
pub async fn check_accounts() -> Result<Value, String> {
    let snapshot = ask(
        "_rustcode/harnesses/accounts/check",
        json!({ "redetect": [] }),
        |_| async { crate::harness::check_accounts(&[]).await },
    )
    .await?;
    Ok(account_rows(&snapshot))
}

/// Register a named login of a harness (its own config folder).
pub async fn add_account(
    harness: &str,
    id: &str,
    name: &str,
    config_dir: Option<&str>,
) -> Result<Value, String> {
    let params = json!({ "harness": harness, "id": id, "name": name, "config_dir": config_dir });
    let snapshot = ask(
        "_rustcode/harnesses/accounts/add",
        params,
        |params| async move {
            crate::harness::add_account(crate::harness::config::AccountChange {
                harness: field(&params, "harness"),
                id: field(&params, "id"),
                name: Some(field(&params, "name")),
                config_dir: params["config_dir"].as_str().map(str::to_owned),
            })?;
            crate::harness::snapshot().await
        },
    )
    .await?;
    Ok(account_rows(&snapshot))
}

/// Forget an account: the config entry goes, its folder and the login in it
/// stay. The server refuses while a thread runs on it.
pub async fn remove_account(harness: &str, id: &str) -> Result<Value, String> {
    let params = json!({ "harness": harness, "id": id });
    let snapshot = ask(
        "_rustcode/harnesses/accounts/remove",
        params,
        |params| async move {
            crate::harness::remove_account(&field(&params, "harness"), &field(&params, "id"))?;
            crate::harness::snapshot().await
        },
    )
    .await?;
    Ok(account_rows(&snapshot))
}

/// The GitHub CLI's accounts and the saved model sign-ins, by name only.
fn connection_rows(snapshot: &Value) -> Value {
    let rows = |value: &Value| value.as_array().cloned().unwrap_or_default();
    json!({
        "github": rows(&snapshot["github"]["accounts"]).iter().map(|account| json!({
            "host": account["host"],
            "login": account["login"],
            "active": account["active"],
            "valid": account["valid"],
        })).collect::<Vec<_>>(),
        "models": rows(&snapshot["accounts"]).iter().map(|account| json!({
            "provider": account["provider"],
            "account": account["account"],
            "display": account["display"],
            "method": account["method"],
            "active": account["active"],
            "needs_sign_in": account["needs_sign_in"],
        })).collect::<Vec<_>>(),
    })
}

async fn connections_snapshot() -> Result<Value, String> {
    serde_json::to_value(crate::connections::snapshot().await).map_err(|error| error.to_string())
}

pub async fn list_connections() -> Result<Value, String> {
    let snapshot = ask("_rustcode/connections", json!({}), |_| {
        connections_snapshot()
    })
    .await?;
    Ok(connection_rows(&snapshot))
}

/// Sign a GitHub account out of the GitHub CLI.
pub async fn logout_github(host: &str, login: &str) -> Result<Value, String> {
    let params = json!({ "host": host, "login": login });
    let snapshot = ask(
        "_rustcode/connections/github/logout",
        params,
        |params| async move {
            crate::connections::github_logout(&field(&params, "host"), &field(&params, "login"))
                .await?;
            connections_snapshot().await
        },
    )
    .await?;
    Ok(connection_rows(&snapshot))
}

/// Sign a saved model account out.
pub async fn logout_model(provider: &str, account: &str) -> Result<Value, String> {
    let params = json!({ "provider": provider, "account": account });
    let snapshot = ask(
        "_rustcode/connections/account/logout",
        params,
        |params| async move {
            crate::provider_auth::sign_out(&field(&params, "provider"), &field(&params, "account"))
                .await
                .map_err(|error| error.to_string())?;
            connections_snapshot().await
        },
    )
    .await?;
    Ok(connection_rows(&snapshot))
}

/// The connected outside agents, without the pending calls or the audit.
pub async fn list_outside() -> Result<Value, String> {
    let answer = ask("_rustcode/outside/list", json!({}), |_| async {
        crate::acp::outside::list(Default::default()).await
    })
    .await?;
    Ok(json!({ "grants": answer["grants"] }))
}

/// Connect an app: its token and config snippet, shown this once.
pub async fn create_outside(
    name: &str,
    platform: &str,
    scope: Option<&str>,
    ask_first: Option<bool>,
) -> Result<Value, String> {
    let params = json!({ "name": name, "platform": platform, "scope": scope, "ask": ask_first });
    ask("_rustcode/outside/create", params, |params| async move {
        let request = serde_json::from_value(params).map_err(|error| error.to_string())?;
        crate::acp::outside::create(request).await
    })
    .await
}

/// Change an app's access, or whether it asks first.
pub async fn update_outside(
    id: &str,
    scope: Option<&str>,
    ask_first: Option<bool>,
) -> Result<Value, String> {
    let params = json!({ "id": id, "scope": scope, "ask": ask_first });
    ask("_rustcode/outside/update", params, |params| async move {
        let request = serde_json::from_value(params).map_err(|error| error.to_string())?;
        crate::acp::outside::update(request)
    })
    .await
}

/// Revoke an app's token; it stops working at once.
pub async fn revoke_outside(id: &str) -> Result<Value, String> {
    ask(
        "_rustcode/outside/revoke",
        json!({ "id": id }),
        |params| async move {
            let request = serde_json::from_value(params).map_err(|error| error.to_string())?;
            crate::acp::outside::revoke(request)
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_and_connection_rows_carry_names_never_credentials() {
        let harnesses = json!({"harnesses": [{"id": "claude-code", "accounts": [{
            "id": "work", "name": "Work", "config_dir": "~/.claude-work", "auth": "ok",
            "token": "sk-secret"
        }]}]});
        let accounts = account_rows(&harnesses).to_string();
        assert!(accounts.contains("\"work\"") && !accounts.contains("sk-secret"));
        let connections = json!({
            "github": {"accounts": [{"host": "github.com", "login": "me", "active": true,
                "valid": true, "token": "gho_secret"}]},
            "accounts": [{"provider": "openai", "account": "me", "display": "Me",
                "method": "chat_gpt", "active": true, "needs_sign_in": false,
                "refresh_token": "rt_secret"}]
        });
        let text = connection_rows(&connections).to_string();
        assert!(text.contains("\"me\""));
        assert!(!text.contains("secret"), "{text}");
    }
}
