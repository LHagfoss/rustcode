//! OAuth Client ID Metadata Documents (CIMD,
//! draft-ietf-oauth-client-id-metadata-document): the client id is an HTTPS
//! URL that serves RustCode's client metadata, so an authorization server can
//! identify RustCode without dynamic client registration.

use serde_json::Value;

/// RustCode's published client metadata document; its URL is its client id.
pub(super) const RUSTCODE_CLIENT_METADATA_URL: &str =
    "https://rustcode.lhagfoss.com/oauth/client-metadata.json";

/// Authorization servers that refuse dynamically registered clients they
/// have not reviewed but accept a client metadata document, without
/// advertising `client_id_metadata_document_supported`.
const ISSUERS_REQUIRING_CLIENT_METADATA: &[&str] = &["https://vercel.com"];

/// The grants RustCode's metadata document declares.
pub(super) fn client_metadata_registration() -> Value {
    serde_json::json!({
        "client_id": RUSTCODE_CLIENT_METADATA_URL,
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    })
}

/// Whether to identify as RustCode's metadata document instead of
/// registering a client: the server advertises support, or is known to
/// accept nothing else from an unreviewed client.
pub(super) fn prefers_client_metadata(server_meta: &Value) -> bool {
    if server_meta
        .get("client_id_metadata_document_supported")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    server_meta
        .get("issuer")
        .and_then(Value::as_str)
        .map(|issuer| issuer.trim_end_matches('/'))
        .is_some_and(|issuer| ISSUERS_REQUIRING_CLIENT_METADATA.contains(&issuer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn advertised_support_prefers_the_metadata_document() {
        assert!(prefers_client_metadata(
            &json!({"issuer": "https://auth.example.com", "client_id_metadata_document_supported": true})
        ));
    }

    #[test]
    fn vercel_prefers_the_metadata_document_without_advertising_it() {
        assert!(prefers_client_metadata(
            &json!({"issuer": "https://vercel.com"})
        ));
        assert!(prefers_client_metadata(
            &json!({"issuer": "https://vercel.com/"})
        ));
    }

    #[test]
    fn other_servers_keep_dynamic_registration() {
        assert!(!prefers_client_metadata(
            &json!({"issuer": "https://mcp.linear.app"})
        ));
        assert!(!prefers_client_metadata(&json!({
            "issuer": "https://auth.example.com",
            "client_id_metadata_document_supported": false
        })));
    }

    #[test]
    fn metadata_registration_names_the_document_as_client_id() {
        let registration = client_metadata_registration();
        assert_eq!(registration["client_id"], RUSTCODE_CLIENT_METADATA_URL);
        assert_eq!(registration["token_endpoint_auth_method"], "none");
    }

    #[test]
    fn live_vercel_with_the_metadata_document_uses_a_plain_code_redirect() {
        let server_meta: Value = serde_json::from_str(include_str!(
            "../fixtures/mcp_oauth_vercel/authorization_server.2026-09-30.json"
        ))
        .expect("captured server metadata");
        assert!(prefers_client_metadata(&server_meta));
        let flow = super::super::select_registered_oauth_flow(
            &server_meta,
            Some(&client_metadata_registration()),
        )
        .expect("flow");
        assert_eq!(flow, super::super::OAuthFlow::Loopback(None));
    }
}
