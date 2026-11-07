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
/// The app's own limit on the elements a block builds (`MAX_ELEMENTS` in
/// `apps/desktop/src/smartui/parse.ts`); a check cannot count them exactly.
pub const MAX_ELEMENTS: usize = 5_000;
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

/// `smart_ui_check`: what the app's sandbox will refuse in a block, without
/// running it. The macOS host's offscreen renderer ([`PREVIEW_TOOL_NAME`]) is
/// gone, so no client can screenshot a block; what every client can answer is
/// static, and an external harness needs it most (it cannot see the chat).
pub const CHECK_TOOL_NAME: &str = "smart_ui_check";

pub(crate) fn check_schema() -> Value {
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

/// What one line of the block does that the sandbox or the card refuses.
fn line_problem(line: &str, in_script: bool) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    // The page is sandboxed: CSP `default-src 'none'`, no navigation. The API
    // comes first, so a line that also names a URL says what it was doing.
    for (needle, what) in [
        ("fetch(", "fetch"),
        ("xmlhttprequest", "XMLHttpRequest"),
        ("websocket", "a socket"),
        ("localstorage", "storage"),
        ("sessionstorage", "storage"),
        ("indexeddb", "storage"),
        ("document.write", "document.write"),
        ("@import", "a stylesheet import"),
        ("url(http", "a remote image or font"),
        ("http://", "a network load"),
        ("https://", "a network load"),
    ] {
        if lower.contains(needle) {
            return Some(format!(
                "the page cannot reach the network, storage or navigation (its CSP blocks them): {what}"
            ));
        }
    }
    // Viewport heights do not exist in a card that sizes to its content.
    let bytes = lower.as_bytes();
    for (i, _) in lower.match_indices("vh") {
        let before = i.checked_sub(1).and_then(|j| bytes.get(j));
        if before.is_some_and(|b| b.is_ascii_digit()) {
            return Some(
                "vh units do not fit the card, which sizes to its content: use px or relative sizes"
                    .to_owned(),
            );
        }
    }
    if in_script {
        for colour in ["CanvasText", "AccentColor"] {
            if lower.contains(&colour.to_ascii_lowercase()) {
                return Some(format!(
                    "a canvas draws nothing with the system colour {colour}: read a --rc-* variable instead"
                ));
            }
        }
    }
    None
}

/// The value of `name` in one tag, unquoted when it is quoted.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    // `to_ascii_lowercase` keeps every byte offset, so the offsets index `tag` too.
    let at = tag.to_ascii_lowercase().find(name)?;
    let rest = tag[at + name.len()..]
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return Some(rest.split_whitespace().next().unwrap_or(""));
    }
    Some(rest[1..].split(quote).next().unwrap_or(""))
}

/// The tag that carries a class or attribute at `at`: from the `<` before it
/// to the `>` after it, or the rest of the line when the tag has not closed yet.
fn tag_at(line: &str, at: usize) -> &str {
    let start = line[..at].rfind('<').unwrap_or(0);
    let end = line[at..].find('>').map_or(line.len(), |end| at + end + 1);
    &line[start..end]
}

/// Kit markup that does nothing as written: a sparkline with no numbers, or a
/// sortable table with nothing to click. Both are quiet until the user tries.
fn kit_problems(line: &str) -> Vec<String> {
    let lower = line.to_ascii_lowercase();
    let mut problems = Vec::new();
    if let Some(at) = lower.find("rc-spark") {
        let tag = tag_at(line, at);
        // Only an element that carries the class: a rule in the view's own <style> is not one.
        let element = tag.starts_with('<')
            && tag[1..].starts_with(|c: char| c.is_ascii_alphabetic())
            && tag.to_ascii_lowercase().contains("class");
        if element {
            match attribute(tag, "data-values") {
                None => problems.push(
                    "`.rc-spark` needs `data-values=\"1,4,2,8\"`: the runtime draws the line from it"
                        .to_owned(),
                ),
                Some(values) => {
                    let numbers = values
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|part| !part.is_empty() && part.parse::<f64>().is_ok())
                        .count();
                    if numbers < 2 {
                        problems.push(
                            "`.rc-spark` needs at least two numbers in `data-values`".to_owned(),
                        );
                    }
                }
            }
        }
    }
    // The runtime sorts on a click in a heading of a table that asked for it.
    if lower.contains("data-rc-sort") && !lower.contains("<table") && !lower.contains("<th") {
        problems.push(
            "`data-rc-sort` goes on `table.rc-table` (every heading then sorts), or on one `th`; here nothing sorts"
                .to_owned(),
        );
    }
    problems
}

/// A static check of one block: its size, the elements it builds, the
/// libraries it names and everything the sandbox will refuse, each with its
/// line. It does not run the page, so it reports no script errors and no
/// screenshot.
pub(crate) fn check(args: &Value) -> ToolExecutionOutput {
    let html = match parse(args) {
        Ok(html) => html,
        Err(reason) => {
            return ToolExecutionOutput::failure_with_kind(
                format!("error: invalid {CHECK_TOOL_NAME} call: {reason}. Nothing was checked."),
                ToolErrorKind::InvalidArguments,
                false,
            );
        }
    };
    let mut problems: Vec<String> = Vec::new();
    let (mut elements, mut scripts, mut styles) = (0usize, 0i32, 0i32);
    let mut in_script = false;
    for (index, line) in html.lines().enumerate() {
        let lower = line.to_ascii_lowercase();
        if let Some(problem) = line_problem(line, in_script) {
            problems.push(format!("line {}: {problem}", index + 1));
        }
        for problem in kit_problems(line) {
            problems.push(format!("line {}: {problem}", index + 1));
        }
        // Only the bundled library loads; any other name stays as written and is blocked.
        for (at, _) in lower.match_indices("rustcode-lib:") {
            let name: String = line[at + "rustcode-lib:".len()..]
                .chars()
                .take_while(|c| !matches!(c, '"' | '\'' | '>' | ' '))
                .collect();
            if name != "chart.js" {
                problems.push(format!(
                    "line {}: `rustcode-lib:{name}` is not bundled; the only library is `rustcode-lib:chart.js`",
                    index + 1
                ));
            }
        }
        elements += line
            .match_indices('<')
            .filter(|(at, _)| line[at + 1..].starts_with(|c: char| c.is_ascii_alphabetic()))
            .count();
        scripts +=
            lower.matches("<script").count() as i32 - lower.matches("</script").count() as i32;
        styles += lower.matches("<style").count() as i32 - lower.matches("</style").count() as i32;
        if lower.contains("<script") && !lower.contains("</script") {
            in_script = true;
        }
        if lower.contains("</script") {
            in_script = false;
        }
    }
    if scripts != 0 {
        problems.push(format!(
            "a <script> tag is not closed ({} more opening than closing)",
            scripts
        ));
    }
    if styles != 0 {
        problems.push(format!(
            "a <style> tag is not closed ({} more opening than closing)",
            styles
        ));
    }
    if elements > MAX_ELEMENTS {
        problems.push(format!(
            "{elements} elements; the app stops a block over {MAX_ELEMENTS} (estimated by counting tags)"
        ));
    }
    let title = html
        .split("<title")
        .nth(1)
        .and_then(|rest| rest.split('>').nth(1))
        .and_then(|rest| rest.split("</title").next())
        .map(|title| one_line(title.trim()))
        .filter(|title| !title.is_empty());
    if title.is_none() {
        problems.push(
            "no <title>: the card would be titled \u{201c}Interactive view\u{201d}, and the reply would not say what it shows"
                .to_owned(),
        );
    }
    let size = html.len();
    let mut out = if problems.is_empty() {
        format!("No problems found in a {size}-byte block")
    } else {
        format!(
            "{size}-byte block: {} problem{}",
            problems.len(),
            if problems.len() == 1 { "" } else { "s" }
        )
    };
    if let Some(title) = title {
        out.push_str(&format!(" titled \u{201c}{title}\u{201d}"));
    }
    out.push_str(&format!(", about {elements} elements."));
    for problem in &problems {
        out.push_str("\n- ");
        out.push_str(problem);
    }
    out.push_str(
        "\nThis is a static check: it does not run the page, so it cannot report script errors, the fitted height or a screenshot. Write the fence only after these problems are gone.",
    );
    ToolExecutionOutput::success(out)
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

    fn checked(html: &str) -> ToolExecutionOutput {
        check(&json!({"html": html}))
    }

    #[test]
    fn a_clean_block_has_no_problems() {
        let output = checked(
            "<title>Costs</title>\n<p>Two plans, one year.</p>\n<style>p{color:var(--rc-muted)}</style>\n\
             <script src=\"rustcode-lib:chart.js\"></script>",
        );
        assert!(output.success, "{}", output.content);
        assert!(
            output.content.starts_with("No problems found"),
            "{}",
            output.content
        );
        assert!(output.content.contains("titled \u{201c}Costs\u{201d}"));
        assert!(output.content.contains("static check"));
    }

    #[test]
    fn the_check_names_what_the_sandbox_refuses() {
        let output = checked(
            "<title>Prices</title>\n<div style=\"height:40vh\">x</div>\n\
             <script src=\"https://cdn.example.com/chart.js\"></script>\n\
             <script>fetch(\"https://api.example.com\").then(r => r.text());</script>",
        );
        assert!(output.success, "the check ran");
        let text = &output.content;
        // One finding per line, each with the line it is on.
        assert!(text.contains("line 2:"), "{text}");
        assert!(text.contains("vh units"), "{text}");
        assert!(text.contains("line 3:"), "{text}");
        assert!(text.contains("network"), "{text}");
        assert!(text.contains("line 4:"), "{text}");
        assert!(text.contains("fetch"), "{text}");
    }

    #[test]
    fn the_check_catches_a_library_the_app_does_not_bundle() {
        let output = checked(
            "<title>X</title><script src=\"rustcode-lib:d3.js\"></script>\n\
             <script>chart = new Chart(c);</script>",
        );
        let text = &output.content;
        assert!(
            text.contains("`rustcode-lib:d3.js` is not bundled"),
            "{text}"
        );
        assert!(!text.contains("chart.js` is not bundled"), "{text}");
    }

    #[test]
    fn the_check_catches_the_blank_canvas_and_an_unclosed_tag() {
        let output = checked(
            "<title>X</title>\n<script>\nChart.defaults.color = \"CanvasText\";\n</script>\n\
             <style>p{color:red}",
        );
        let text = &output.content;
        assert!(text.contains("CanvasText"), "{text}");
        assert!(text.contains("<style> tag is not closed"), "{text}");
    }

    #[test]
    fn the_check_knows_the_kit_s_hover_pieces() {
        // A sortable table, a bar with its hover text and a sparkline with numbers.
        let clean = checked(
            "<title>Runs</title>\n\
             <div class=\"rc-bars\"><div class=\"rc-bar\" data-rc-tip=\"7 failed\"><span>auth</span>\
             <i style=\"width:25%\"></i><b>10.5 s</b></div></div>\n\
             <table class=\"rc-table\" data-rc-sort><thead><tr><th>Suite</th><th class=\"rc-num\">Time</th></tr></thead>\
             <tbody><tr><td>engine</td><td class=\"rc-num\">42 s</td></tr></tbody></table>\n\
             <span class=\"rc-spark\" data-values=\"1,4,2,8\"></span>\n\
             <style>.rc-spark { width: 64px; }</style>",
        );
        assert!(
            clean.content.starts_with("No problems found"),
            "{}",
            clean.content
        );

        // A sparkline with no numbers draws nothing, and data-rc-sort here sorts nothing.
        let broken = checked(
            "<title>Runs</title>\n\
             <span class=\"rc-spark\">no numbers</span>\n\
             <span class=\"rc-spark\" data-values=\"7\"></span>\n\
             <td data-rc-sort>Suite</td>",
        );
        let text = &broken.content;
        assert!(text.contains("line 2:"), "{text}");
        assert!(text.contains("`data-values"), "{text}");
        assert!(text.contains("line 3:"), "{text}");
        assert!(text.contains("at least two numbers"), "{text}");
        assert!(text.contains("line 4:"), "{text}");
        assert!(text.contains("nothing sorts"), "{text}");
    }

    #[test]
    fn the_check_wants_a_title_and_refuses_a_block_over_the_limit() {
        let text = checked("<p>just prose</p>").content;
        assert!(text.contains("no <title>"), "{text}");
        let big = format!(
            "{}{}",
            "<title>Big</title>",
            "<p>x</p>".repeat(MAX_SOURCE_BYTES / 8 + 1)
        );
        let output = checked(&big);
        assert!(!output.success);
        assert!(
            output.content.contains("at most 64 KB"),
            "{}",
            output.content
        );
    }
}
