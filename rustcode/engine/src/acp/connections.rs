//! `_rustcode/connections` requests: the macOS Settings "Connections" page
//! lists GitHub and the saved model accounts and signs them in and out
//! through the engine (see `crate::connections`).

use serde::{Deserialize, Serialize};

/// GitHub and model accounts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/connections", response = serde_json::Value)]
pub(crate) struct ConnectionsRequest {}

/// Start `gh auth login --web`; answers with the one-time code and URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/connections/github/login", response = serde_json::Value)]
pub(crate) struct GitHubLoginRequest {}

/// Wait for the running sign-in; answers with the fresh snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/connections/github/wait", response = serde_json::Value)]
pub(crate) struct GitHubWaitRequest {}

/// Stop the running sign-in.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/connections/github/cancel", response = serde_json::Value)]
pub(crate) struct GitHubCancelRequest {}

/// Sign a GitHub account out of `gh`; answers with the fresh snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/connections/github/logout", response = serde_json::Value)]
pub(crate) struct GitHubLogoutRequest {
    #[serde(default = "github_host")]
    pub host: String,
    pub login: String,
}

fn github_host() -> String {
    crate::connections::GITHUB_HOST.to_owned()
}

/// Sign a saved model account out; answers with the fresh snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/connections/account/logout", response = serde_json::Value)]
pub(crate) struct AccountLogoutRequest {
    pub provider: String,
    pub account: String,
}

/// Refetch a saved model account's model list, as `/account refresh` does
/// in the terminal; answers with the fresh snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/connections/account/refresh", response = serde_json::Value)]
pub(crate) struct AccountRefreshRequest {
    pub provider: String,
    pub account: String,
}

fn failed(message: impl Into<String>) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(message.into())
}

pub(crate) async fn snapshot_value() -> Result<serde_json::Value, agent_client_protocol::Error> {
    serde_json::to_value(crate::connections::snapshot().await)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))
}

pub(crate) async fn github_login() -> Result<serde_json::Value, agent_client_protocol::Error> {
    let code = crate::connections::start_github_login()
        .await
        .map_err(failed)?;
    serde_json::to_value(code)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))
}

pub(crate) async fn github_wait() -> Result<serde_json::Value, agent_client_protocol::Error> {
    crate::connections::wait_github_login()
        .await
        .map_err(failed)?;
    snapshot_value().await
}

pub(crate) async fn github_logout(
    request: GitHubLogoutRequest,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    crate::connections::github_logout(&request.host, &request.login)
        .await
        .map_err(failed)?;
    snapshot_value().await
}

pub(crate) async fn account_logout(
    request: AccountLogoutRequest,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    crate::provider_auth::sign_out(&request.provider, &request.account)
        .await
        .map_err(|error| failed(error.to_string()))?;
    snapshot_value().await
}

pub(crate) async fn account_refresh(
    request: AccountRefreshRequest,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let command = refresh_command(&request.provider, &request.account);
    let (_, _, mut config) = crate::config::load_config();
    let result = crate::provider_auth::execute_command(&command, &config)
        .await
        .map_err(|error| failed(format!("{error:#}")))?;
    // Merged into the saved profiles, keeping their names and settings; new
    // chats list the refreshed models, open ones when reopened.
    if result.profile.is_some() || !result.profiles.is_empty() {
        crate::provider_auth::apply_auth_result(&mut config, &result);
        crate::config::save_entire_config(&config);
    }
    snapshot_value().await
}

/// The `/account refresh` words for an account. Claude CLI has one sign-in,
/// so it is named by provider alone.
fn refresh_command(provider: &str, account: &str) -> String {
    if provider.eq_ignore_ascii_case(crate::provider_auth::claude_cli::PROVIDER) {
        format!("account refresh {provider}")
    } else {
        format!("account refresh {provider} {account}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::JsonRpcMessage;

    #[test]
    fn requests_use_the_rustcode_extension_methods() {
        assert!(ConnectionsRequest::matches_method("_rustcode/connections"));
        assert!(GitHubLoginRequest::matches_method(
            "_rustcode/connections/github/login"
        ));
        assert!(GitHubWaitRequest::matches_method(
            "_rustcode/connections/github/wait"
        ));
        assert!(GitHubCancelRequest::matches_method(
            "_rustcode/connections/github/cancel"
        ));
        let logout: GitHubLogoutRequest =
            serde_json::from_value(serde_json::json!({"login": "amy"})).unwrap();
        assert_eq!(logout.host, "github.com");
        let account: AccountLogoutRequest =
            serde_json::from_value(serde_json::json!({"provider": "openai", "account": "default"}))
                .unwrap();
        assert_eq!(account.provider, "openai");
        assert!(AccountRefreshRequest::matches_method(
            "_rustcode/connections/account/refresh"
        ));
        assert_eq!(
            refresh_command("openai", "abc"),
            "account refresh openai abc"
        );
        assert_eq!(
            refresh_command(crate::provider_auth::claude_cli::PROVIDER, "x"),
            format!(
                "account refresh {}",
                crate::provider_auth::claude_cli::PROVIDER
            )
        );
    }
}
