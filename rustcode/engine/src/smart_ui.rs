//! `smart_ui_preview`: the agent sees its Smart UI block before the user does.
//!
//! A `rustcode-ui v1` block (docs/smart-ui.md) used to be written blind: the
//! agent never saw the page, so layout bugs, clipped content and script
//! errors reached the user. This tool forwards the block's HTML to the macOS
//! app as a `_rustcode/smartUi/preview` request. The app renders it offscreen
//! in the same sandbox and at the same width as the chat card, and answers
//! with a screenshot, the fitted height, the element count and what the page
//! threw. The screenshot is attached after the tool batch like a
//! `localhost_preview` one.
//!
//! The tool is offered only to root sessions whose client advertised
//! `_meta.rustcode.smartUi >= 2` while Smart UI is on. Rendering has no side
//! effects, so it needs no approval.

use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::preview::{PreviewClient, decode_screenshot, one_line, page_errors};
use crate::tools::{ToolErrorKind, ToolExecutionOutput};

pub const PREVIEW_TOOL_NAME: &str = "smart_ui_preview";
/// The extension request the app answers.
pub const PREVIEW_METHOD: &str = "_rustcode/smartUi/preview";
/// The first `_meta.rustcode.smartUi` version whose client answers
/// [`PREVIEW_METHOD`]. Version 1 runs blocks but cannot preview them.
pub const PREVIEW_CAPABILITY_VERSION: u64 = 2;
/// The app's own limit on a block's source (`SmartUI.maxSourceBytes`).
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
/// Rendering waits for the page to load and settle; the app bounds its own
/// waits well under this.
const PREVIEW_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) const UNAVAILABLE_MESSAGE: &str = "error: smart_ui_preview needs the RustCode macOS app with Smart UI turned on. Nothing was rendered.";

/// Appended to root prompts that offer the tool.
pub(crate) const PREVIEW_PROMPT_HINT: &str = "\n# Smart UI preview\n\
`smart_ui_preview` renders a rustcode-ui v1 body exactly as the chat card will and returns a screenshot, its height and any page errors. \
Preview each block before you write it, fix what the screenshot or errors show (one or two rounds, not more), then write the fence with the same HTML.\n";

pub(crate) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "html": {
                "type": "string",
                "description": "The block's body: the self-contained HTML that goes between the ```rustcode-ui v1 fences."
            }
        },
        "required": ["html"],
        "additionalProperties": false
    })
}

/// The session's client for [`PREVIEW_METHOD`], when it advertised one.
pub(crate) fn client_meta_supports_preview(meta: Option<&serde_json::Map<String, Value>>) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("smartUi"))
        .and_then(Value::as_u64)
        .is_some_and(|version| version >= PREVIEW_CAPABILITY_VERSION)
}

fn parse(args: &Value) -> Result<String, String> {
    let html = args
        .get("html")
        .and_then(Value::as_str)
        .ok_or("`html` must be a string")?;
    if html.trim().is_empty() {
        return Err("`html` is empty".to_owned());
    }
    if html.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "`html` is {} KB; a block may hold at most {} KB",
            html.len().div_ceil(1024),
            MAX_SOURCE_BYTES / 1024
        ));
    }
    Ok(html.to_owned())
}

fn failure(
    text: String,
    kind: ToolErrorKind,
    retryable: bool,
) -> (ToolExecutionOutput, Option<PathBuf>) {
    (
        ToolExecutionOutput::failure_with_kind(text, kind, retryable),
        None,
    )
}

/// Turn the app's answer into the tool result. Only `"ok": true` with a
/// screenshot is a success.
pub(crate) fn interpret_response(
    response: &Value,
    save_image: impl FnOnce(&[u8], &str) -> std::io::Result<PathBuf>,
) -> (ToolExecutionOutput, Option<PathBuf>) {
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        let reason = response
            .get("error")
            .and_then(|error| error.get("message").or(Some(error)))
            .and_then(Value::as_str)
            .map(one_line)
            .filter(|reason| !reason.is_empty())
            .unwrap_or_else(|| "no reason given".to_owned());
        let mut text = format!("error: the block did not render: {reason}");
        text.push_str(&page_errors(response));
        return failure(text, ToolErrorKind::Validation, false);
    }
    let Some(image) = response.get("image") else {
        return failure(
            "error: the app answered without a screenshot".to_owned(),
            ToolErrorKind::Internal,
            false,
        );
    };
    let (bytes, extension) = match decode_screenshot(image) {
        Ok(decoded) => decoded,
        Err(reason) => {
            return failure(format!("error: {reason}"), ToolErrorKind::Internal, false);
        }
    };
    let path = match save_image(&bytes, extension) {
        Ok(path) => path,
        Err(error) => {
            return failure(
                format!("error: could not save the screenshot: {error}"),
                ToolErrorKind::Internal,
                false,
            );
        }
    };
    let number = |key: &str| response.get(key).and_then(Value::as_u64);
    let mut out = String::from("Rendered");
    if let Some(title) = response
        .get("title")
        .and_then(Value::as_str)
        .map(one_line)
        .filter(|title| !title.is_empty())
    {
        out.push_str(&format!(" \"{title}\""));
    }
    if let Some(width) = number("width") {
        out.push_str(&format!(" at the card's width, {width} px"));
    }
    out.push('.');
    if let Some(height) = number("contentHeight") {
        out.push_str(&format!(" Content is {height} px tall"));
        if let Some(max) = number("maxHeight")
            && height > max
        {
            out.push_str(&format!(
                ", taller than the card's {max} px, so the rest scrolls inside the card"
            ));
        }
        out.push('.');
    }
    if let Some(elements) = number("elements") {
        out.push_str(&format!(" {elements} elements."));
    }
    out.push_str(&format!(
        "\nScreenshot saved to {}. It is attached as an image right after this tool result.",
        path.display()
    ));
    out.push_str(&page_errors(response));
    (ToolExecutionOutput::success(out), Some(path))
}

/// Run one `smart_ui_preview` call against the session's client.
pub(crate) async fn execute(
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel: &tokio_util::sync::CancellationToken,
    args: &Value,
) -> ToolExecutionOutput {
    let client = state.lock().await.smart_ui_client.clone();
    let (output, image) = execute_with(
        client,
        cancel,
        args,
        PREVIEW_REQUEST_TIMEOUT,
        |bytes, ext| crate::controller::save_image_attachment(bytes, ext),
    )
    .await;
    if let Some(path) = image {
        state.lock().await.pending_preview_images.push(path);
    }
    output
}

pub(crate) async fn execute_with(
    client: Option<Arc<dyn PreviewClient>>,
    cancel: &tokio_util::sync::CancellationToken,
    args: &Value,
    timeout: Duration,
    save_image: impl FnOnce(&[u8], &str) -> std::io::Result<PathBuf>,
) -> (ToolExecutionOutput, Option<PathBuf>) {
    let html = match parse(args) {
        Ok(html) => html,
        Err(reason) => {
            return failure(
                format!("error: invalid smart_ui_preview call: {reason}. Nothing was rendered."),
                ToolErrorKind::InvalidArguments,
                false,
            );
        }
    };
    let Some(client) = client else {
        return failure(
            UNAVAILABLE_MESSAGE.to_owned(),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    };
    let request = client.request(json!({ "html": html }));
    let answer = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            return failure(
                "error: the Smart UI preview was cancelled before the app answered".to_owned(),
                ToolErrorKind::Cancelled,
                false,
            );
        }
        answer = tokio::time::timeout(timeout, request) => answer,
    };
    match answer {
        Err(_) => failure(
            format!(
                "error: the app did not render the block within {}s",
                timeout.as_secs()
            ),
            ToolErrorKind::Unknown,
            true,
        ),
        Ok(Err(reason)) => failure(
            format!(
                "error: the Smart UI preview did not run: {}",
                one_line(&reason)
            ),
            ToolErrorKind::UnavailableDependency,
            false,
        ),
        Ok(Ok(response)) => interpret_response(&response, save_image),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::PreviewFuture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";

    struct Scripted {
        answer: Value,
        calls: AtomicUsize,
        last: std::sync::Mutex<Option<Value>>,
    }

    impl PreviewClient for Scripted {
        fn request(&self, params: Value) -> PreviewFuture {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() = Some(params);
            let answer = self.answer.clone();
            Box::pin(async move { Ok(answer) })
        }
    }

    fn answering(answer: Value) -> Arc<Scripted> {
        Arc::new(Scripted {
            answer,
            calls: AtomicUsize::new(0),
            last: std::sync::Mutex::new(None),
        })
    }

    fn png_base64() -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        STANDARD.encode(PNG)
    }

    async fn run(
        client: Option<Arc<Scripted>>,
        args: Value,
    ) -> (ToolExecutionOutput, Option<PathBuf>) {
        execute_with(
            client.map(|client| client as Arc<dyn PreviewClient>),
            &tokio_util::sync::CancellationToken::new(),
            &args,
            Duration::from_secs(5),
            |bytes, ext| {
                assert_eq!(bytes, PNG);
                Ok(PathBuf::from(format!("/tmp/shot.{ext}")))
            },
        )
        .await
    }

    /// The prompt hint and the request's `tools` come from one policy.
    #[test]
    fn request_policy_offers_the_tool_only_with_a_client() {
        let mut state = crate::app::AppState::new();
        assert!(!crate::network::root_schema_policy(&state, false).include_smart_ui_preview_tool);
        state.smart_ui_client = Some(answering(Value::Null) as Arc<dyn PreviewClient>);
        let policy = crate::network::root_schema_policy(&state, false);
        assert!(policy.include_smart_ui_preview_tool);
        let mut prompt = String::new();
        crate::tools::schema::append_preview_hint(&mut prompt, policy);
        assert!(prompt.contains(PREVIEW_TOOL_NAME));
        let tools = crate::tools::native_tools_schema_for_context(policy, &[]).0;
        assert!(
            tools
                .iter()
                .any(|tool| tool.to_string().contains(PREVIEW_TOOL_NAME))
        );
        assert!(!crate::network::root_schema_policy(&state, true).include_smart_ui_preview_tool);
        let subagent = crate::tools::ToolSchemaPolicy::subagent().with_smart_ui_preview_tool();
        assert!(!subagent.include_smart_ui_preview_tool);
    }

    #[test]
    fn capability_needs_version_two() {
        let meta = |value: Value| value.as_object().cloned();
        assert!(client_meta_supports_preview(
            meta(json!({"rustcode": {"smartUi": 2}})).as_ref()
        ));
        for old in [json!(1), json!(0), json!(true)] {
            assert!(!client_meta_supports_preview(
                meta(json!({"rustcode": {"smartUi": old}})).as_ref()
            ));
        }
        assert!(!client_meta_supports_preview(None));
    }

    #[tokio::test]
    async fn invalid_html_never_reaches_the_app() {
        let client = answering(json!({"ok": true}));
        for args in [
            json!({}),
            json!({"html": 3}),
            json!({"html": "  \n"}),
            json!({"html": "x".repeat(MAX_SOURCE_BYTES + 1)}),
        ] {
            let (output, image) = run(Some(client.clone()), args).await;
            assert!(!output.success, "{}", output.content);
            assert!(output.content.contains("Nothing was rendered"));
            assert!(image.is_none());
        }
        assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn without_a_client_nothing_is_rendered() {
        let (output, image) = run(None, json!({"html": "<p>hi</p>"})).await;
        assert!(!output.success);
        assert_eq!(output.content, UNAVAILABLE_MESSAGE);
        assert!(image.is_none());
    }

    #[tokio::test]
    async fn a_render_reports_size_and_errors_and_attaches_the_screenshot() {
        let client = answering(json!({
            "ok": true,
            "title": "Slowest tests",
            "width": 360,
            "contentHeight": 900,
            "maxHeight": 720,
            "elements": 42,
            "errors": ["TypeError: rows is undefined"],
            "image": {"data": png_base64(), "mimeType": "image/png"},
        }));
        let (output, image) = run(Some(client.clone()), json!({"html": "<p>hi</p>"})).await;
        assert!(output.success, "{}", output.content);
        assert_eq!(image, Some(PathBuf::from("/tmp/shot.png")));
        assert!(output.content.contains("\"Slowest tests\""));
        assert!(output.content.contains("360 px"));
        assert!(output.content.contains("taller than the card's 720 px"));
        assert!(output.content.contains("42 elements"));
        assert!(output.content.contains("TypeError: rows is undefined"));
        assert_eq!(
            client.last.lock().unwrap().clone(),
            Some(json!({"html": "<p>hi</p>"}))
        );
    }

    #[tokio::test]
    async fn a_failed_render_is_an_error_with_its_reason() {
        let client = answering(json!({
            "ok": false,
            "error": {"message": "the page stopped responding"},
            "errors": ["RangeError: too much recursion"],
        }));
        let (output, image) = run(Some(client), json!({"html": "<p>hi</p>"})).await;
        assert!(!output.success);
        assert!(output.content.contains("the page stopped responding"));
        assert!(output.content.contains("RangeError"));
        assert!(image.is_none());
        let (output, _) = run(
            Some(answering(json!({"ok": true}))),
            json!({"html": "<p>x</p>"}),
        )
        .await;
        assert!(!output.success);
        assert!(output.content.contains("without a screenshot"));
    }
}
