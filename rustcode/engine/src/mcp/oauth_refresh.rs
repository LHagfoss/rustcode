use std::io::Write as _;
use std::time::Duration;

use super::{OAuthToken, oauth_token_from_response, post_oauth_form_raw, truncate_snippet};

/// Why a refresh produced no token.
#[derive(Debug)]
pub(super) enum RefreshError {
    /// The token endpoint could not be reached (offline, DNS, connection reset).
    /// The saved sign-in is still good; try again later instead of a new login.
    Unreachable(String),
    /// The token endpoint answered and refused the refresh token.
    Refused(String),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(error) | Self::Refused(error) => f.write_str(error),
        }
    }
}

/// The RFC 6749 §6 refresh form. `resource` (RFC 8707) is the MCP server URL the
/// grant was issued for; the MCP authorization spec requires it on every token
/// request, and a server such as Opper answers `invalid_request` without it.
pub(super) fn refresh_form(
    client_id: Option<&str>,
    refresh_token: &str,
    resource: Option<&str>,
) -> Vec<(&'static str, String)> {
    [
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
    ]
    .into_iter()
    .chain(client_id.map(|id| ("client_id", id.to_string())))
    .chain(resource.map(|url| ("resource", url.to_string())))
    .collect()
}

/// The error for a token endpoint that refused a request. It names the
/// parameters that were sent, never their values, so a rejected request can be
/// diagnosed from the log without leaking a token.
pub(super) fn token_request_refused(
    status: reqwest::StatusCode,
    text: &str,
    params: &[(&str, String)],
) -> String {
    let snippet = truncate_snippet(text, 300);
    let sent = params
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    format!("OAuth token request failed (HTTP {status}): {snippet} (sent parameters: {sent})")
}

pub(super) async fn refresh_access_token(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: Option<&str>,
    refresh_token: &str,
    resource: Option<&str>,
) -> Result<OAuthToken, RefreshError> {
    let form = refresh_form(client_id, refresh_token, resource);
    let (status, body, text) = post_oauth_form_raw(http, token_endpoint, &form)
        .await
        .map_err(RefreshError::Unreachable)?;
    if !status.is_success() {
        return Err(RefreshError::Refused(token_request_refused(
            status, &text, &form,
        )));
    }
    if body.is_null() {
        return Err(RefreshError::Refused(
            "OAuth token response was not JSON".to_string(),
        ));
    }
    let mut token = oauth_token_from_response(&body, client_id, token_endpoint)
        .map_err(RefreshError::Refused)?;
    if token.refresh_token.is_none() {
        token.refresh_token = Some(refresh_token.to_string());
    }
    token.resource = resource.map(str::to_string);
    Ok(token)
}

/// Retry delays for a harness thread that starts while the network is still
/// coming up, as it is right after the computer wakes or the app launches.
pub(super) const HARNESS_REFRESH_RETRIES: [Duration; 2] =
    [Duration::from_secs(1), Duration::from_secs(3)];

/// [`refresh_access_token`], tried again after each of `delays` while the token
/// endpoint is unreachable. A refusal is returned at once.
pub(super) async fn refresh_until_reachable(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: Option<&str>,
    refresh_token: &str,
    resource: Option<&str>,
    delays: &[Duration],
) -> Result<OAuthToken, RefreshError> {
    let mut delays = delays.iter();
    loop {
        match refresh_access_token(http, token_endpoint, client_id, refresh_token, resource).await {
            Err(RefreshError::Unreachable(error)) => match delays.next() {
                Some(delay) => {
                    crate::dbg_log!("[mcp] token endpoint unreachable, retrying: {error}");
                    tokio::time::sleep(*delay).await;
                }
                None => return Err(RefreshError::Unreachable(error)),
            },
            result => return result,
        }
    }
}

/// Write `token` to `path` through a private staging file and a rename, so a
/// reader in another RustCode process never sees a half-written sign-in and a
/// crash mid-write cannot lose a rotated refresh token.
pub(super) fn write_token_file(path: &std::path::Path, token: &OAuthToken) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let contents = serde_json::to_vec_pretty(token)
        .map_err(|e| format!("failed to serialize OAuth token: {e}"))?;
    let staging = path.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        let mut file = crate::daemon::private_files::open_private(
            &staging,
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true),
        )?;
        file.write_all(&contents)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&staging, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        format!("failed to write {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CIMD_CLIENT: &str = "https://rustcode.lhagfoss.com/oauth/client-metadata.json";
    const OPPER_MCP: &str = "https://api.opper.ai/mcp";

    #[test]
    fn a_refresh_sends_the_login_client_id_and_resource_as_a_form() {
        let form = refresh_form(Some(CIMD_CLIENT), "rotating-refresh", Some(OPPER_MCP));
        assert_eq!(
            form,
            vec![
                ("grant_type", "refresh_token".to_string()),
                ("refresh_token", "rotating-refresh".to_string()),
                ("client_id", CIMD_CLIENT.to_string()),
                ("resource", OPPER_MCP.to_string()),
            ]
        );
        assert!(
            !form.iter().any(|(name, _)| *name == "client_secret"),
            "a public client sends no client secret"
        );
    }

    #[test]
    fn a_refresh_without_a_known_client_or_resource_leaves_them_out() {
        let names: Vec<_> = refresh_form(None, "r", None)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["grant_type", "refresh_token"]);
    }

    #[test]
    fn a_refused_token_request_names_its_parameters_but_not_their_values() {
        let form = refresh_form(Some(CIMD_CLIENT), "secret-refresh-value", Some(OPPER_MCP));
        let error = token_request_refused(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"invalid_request","error_description":"Invalid request parameters"}"#,
            &form,
        );
        assert!(error.contains("HTTP 400"), "{error}");
        assert!(error.contains("invalid_request"), "{error}");
        assert!(
            error.contains("sent parameters: grant_type, refresh_token, client_id, resource"),
            "{error}"
        );
        assert!(!error.contains("secret-refresh-value"), "{error}");
    }

    #[test]
    fn a_token_saved_before_resource_was_stored_still_loads() {
        let token: OAuthToken = serde_json::from_str(
            r#"{"access_token":"a","refresh_token":"r","client_id":"c","token_endpoint":"t","expires_at":1}"#,
        )
        .unwrap();
        assert_eq!(token.resource, None);
        assert_eq!(token.refresh_token.as_deref(), Some("r"));
    }

    #[test]
    fn a_token_file_is_replaced_whole_and_leaves_no_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-oauth").join("opper-ai.json");
        let mut token: OAuthToken = serde_json::from_str(
            r#"{"access_token":"first","refresh_token":"r1","resource":"https://api.opper.ai/mcp"}"#,
        )
        .unwrap();
        write_token_file(&path, &token).unwrap();
        token.access_token = "second".into();
        token.refresh_token = Some("r2".into());
        write_token_file(&path, &token).unwrap();
        let saved: OAuthToken =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, token);
        assert!(!path.with_extension("json.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn an_unreachable_token_endpoint_is_not_a_refused_sign_in() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        drop(listener);
        let result = refresh_until_reachable(
            &reqwest::Client::new(),
            &endpoint,
            Some(CIMD_CLIENT),
            "r",
            Some(OPPER_MCP),
            &[Duration::ZERO],
        )
        .await;
        assert!(
            matches!(&result, Err(RefreshError::Unreachable(error)) if error.contains("error sending request")),
            "{result:?}"
        );
    }
}
