//! MCP Apps (SEP-1865) a tool result carries: an embedded `ui://` resource
//! whose HTML a client may render in a sandboxed frame next to the result's
//! text. Only the shape and the size cap live here; rendering and the
//! host/app message boundary belong to the client that draws it.

use serde::{Deserialize, Serialize};

/// The MIME type the MCP Apps extension gives an app's HTML.
pub const MCP_APP_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// The largest app HTML carried with a result. Larger apps are recorded
/// without their HTML, so a client says the view is unavailable instead of
/// carrying megabytes of markup through every transcript frame.
pub const MCP_APP_MAX_BYTES: usize = 512 * 1024;

/// An interactive view a tool result offered.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpAppResource {
    /// The resource's `ui://` URI.
    pub uri: String,
    pub mime_type: String,
    /// The app's HTML; absent when it was over [`MCP_APP_MAX_BYTES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// The HTML's size in bytes, whether or not it was kept.
    pub total_bytes: u64,
}

/// Whether a resource's URI and MIME type name an MCP App: a `ui://` URI
/// with the extension's HTML type, or plain `text/html` (some servers omit
/// the profile parameter).
pub fn is_mcp_app(uri: &str, mime_type: Option<&str>) -> bool {
    if !uri.starts_with("ui://") {
        return false;
    }
    let Some(mime) = mime_type else {
        return false;
    };
    let mime: String = mime
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    mime == MCP_APP_MIME_TYPE || mime == "text/html"
}

/// The app an embedded text resource is, if it is one.
pub fn mcp_app_resource(uri: &str, mime_type: Option<&str>, text: &str) -> Option<McpAppResource> {
    if !is_mcp_app(uri, mime_type) {
        return None;
    }
    Some(McpAppResource {
        uri: uri.to_owned(),
        mime_type: mime_type.unwrap_or_default().to_owned(),
        html: (text.len() <= MCP_APP_MAX_BYTES).then(|| text.to_owned()),
        total_bytes: text.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_ui_resources_with_an_html_type() {
        assert!(is_mcp_app("ui://weather/view", Some(MCP_APP_MIME_TYPE)));
        assert!(is_mcp_app(
            "ui://weather/view",
            Some("text/html; profile=mcp-app")
        ));
        assert!(is_mcp_app("ui://weather/view", Some("text/html")));
        assert!(!is_mcp_app("file:///view.html", Some(MCP_APP_MIME_TYPE)));
        assert!(!is_mcp_app("ui://weather/view", Some("text/plain")));
        assert!(!is_mcp_app("ui://weather/view", None));
    }

    #[test]
    fn drops_html_over_the_cap_but_keeps_the_app() {
        let small = mcp_app_resource("ui://a", Some(MCP_APP_MIME_TYPE), "<p>hi</p>").unwrap();
        assert_eq!(small.html.as_deref(), Some("<p>hi</p>"));
        let big = "x".repeat(MCP_APP_MAX_BYTES + 1);
        let large = mcp_app_resource("ui://a", Some(MCP_APP_MIME_TYPE), &big).unwrap();
        assert_eq!(large.html, None);
        assert_eq!(large.total_bytes, big.len() as u64);
        assert!(mcp_app_resource("https://a", Some(MCP_APP_MIME_TYPE), "x").is_none());
    }
}
