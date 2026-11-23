use super::Value;

const READ_URL_DEFAULT_CHARS: usize = 8_000;
const READ_URL_MAX_CHARS: usize = 20_000;
const READ_URL_MAX_BYTES: usize = 4 * 1024 * 1024;

pub fn read_url(args: &Value) -> Result<String, String> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or("missing 'url' argument")?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("'url' must start with http:// or https://".to_string());
    }
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let max_chars = args
        .get("max_chars")
        .and_then(Value::as_u64)
        .map_or(READ_URL_DEFAULT_CHARS, |value| value as usize)
        .clamp(500, READ_URL_MAX_CHARS);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to create async runtime: {e}"))?;
    let (content_type, body) = runtime.block_on(async {
        let mut response = reqwest::Client::new()
            .get(url)
            .timeout(std::time::Duration::from_secs(20))
            .header(
                "User-Agent",
                concat!("rustcode/", env!("CARGO_PKG_VERSION"), " (read_url)"),
            )
            .send()
            .await
            .map_err(|error| format!("request failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("the server answered {}", response.status()));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("reading the page failed: {error}"))?
        {
            body.extend_from_slice(&chunk);
            if body.len() >= READ_URL_MAX_BYTES {
                body.truncate(READ_URL_MAX_BYTES);
                break;
            }
        }
        Ok((content_type, body))
    })?;
    let is_text = content_type.is_empty()
        || content_type.starts_with("text/")
        || ["json", "xml", "javascript", "html"]
            .iter()
            .any(|kind| content_type.contains(kind));
    if !is_text {
        return Err(format!(
            "the page is '{content_type}', not text; download it with run_command instead"
        ));
    }
    let raw = String::from_utf8_lossy(&body);
    let text = if content_type.contains("html") || raw.trim_start().starts_with('<') {
        readable_page_text(&raw)
    } else {
        raw.into_owned()
    };
    Ok(page_slice(url, &text, offset, max_chars))
}

/// The title and visible text of an HTML page, one block per line.
pub(super) fn readable_page_text(html: &str) -> String {
    let document = scraper::Html::parse_document(html);
    let mut text = String::new();
    if let Ok(selector) = scraper::Selector::parse("title")
        && let Some(title) = document.select(&selector).next()
    {
        let title = title.text().collect::<String>();
        if !title.trim().is_empty() {
            text.push_str(&format!("# {}\n", title.trim()));
        }
    }
    push_readable_text(document.root_element(), &mut text);
    text.trim().to_string()
}

fn push_readable_text(element: scraper::ElementRef<'_>, text: &mut String) {
    const SKIPPED: [&str; 6] = ["script", "style", "noscript", "template", "svg", "head"];
    const BLOCKS: [&str; 16] = [
        "p",
        "div",
        "br",
        "li",
        "tr",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "section",
        "article",
        "pre",
        "table",
        "blockquote",
    ];
    let name = element.value().name();
    if SKIPPED.contains(&name) {
        return;
    }
    let is_block = BLOCKS.contains(&name);
    if is_block && !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for child in element.children() {
        if let Some(child) = scraper::ElementRef::wrap(child) {
            push_readable_text(child, text);
        } else if let Some(content) = child.value().as_text() {
            let words = content.split_whitespace().collect::<Vec<_>>().join(" ");
            if !words.is_empty() {
                if !text.is_empty() && !text.ends_with(['\n', ' ']) {
                    text.push(' ');
                }
                text.push_str(&words);
            }
        }
    }
    if is_block && !text.ends_with('\n') {
        text.push('\n');
    }
}

/// `max_chars` characters of a page from `offset`, with what is left to read.
pub(super) fn page_slice(url: &str, text: &str, offset: usize, max_chars: usize) -> String {
    let total = text.chars().count();
    let slice = text
        .chars()
        .skip(offset)
        .take(max_chars)
        .collect::<String>();
    let end = offset + slice.chars().count();
    let mut result = format!("[{url} — characters {offset}-{end} of {total}]\n{slice}");
    if end < total {
        result.push_str(&format!(
            "\n[{} more characters: call read_url again with offset={end}]",
            total - end
        ));
    } else if total == 0 {
        result.push_str("(the page has no readable text; it may be rendered by scripts)");
    }
    result
}

pub fn search_web(args: &Value) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to create async runtime: {e}"))?;
    runtime.block_on(search_web_async(args, &reqwest::Client::new()))
}

/// The query with a `site:` filter for `domain`, unless it is empty or the
/// query already has it.
pub(super) fn scoped_search_query(query: &str, domain: Option<&str>) -> String {
    let domain = domain.map(str::trim).unwrap_or_default();
    let filter = format!("site:{domain}");
    if domain.is_empty() || query.contains(&filter) {
        query.to_owned()
    } else {
        format!("{query} {filter}")
    }
}

/// At most about 300 bytes of a result's text, cut on a character boundary.
pub(super) fn clip_snippet(text: &str) -> &str {
    const LIMIT: usize = 300;
    if text.len() <= LIMIT {
        return text;
    }
    let mut end = LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub(crate) async fn search_web_async(
    args: &Value,
    client: &reqwest::Client,
) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|q| q.as_str())
        .ok_or("missing 'query' argument")?;
    let domain = args.get("domain").and_then(|d| d.as_str());

    let search_query = scoped_search_query(query, domain);

    // Exa only with the user's own key; without one, fall through to Tavily
    // and DuckDuckGo instead of a request bound to fail.
    if let Some(exa_key) = crate::shell_env::env_var("EXA_API_KEY").filter(|key| !key.is_empty()) {
        let body = serde_json::json!({
            "query": search_query,
            "numResults": 5,
            "useAutoprompt": true,
            "contents": {
                "text": {
                    "maxCharacters": 1000
                }
            }
        });

        if let Ok(response) = client
            .post("https://api.exa.ai/search")
            .timeout(std::time::Duration::from_secs(10))
            .header("x-api-key", &exa_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            && response.status().is_success()
            && let Ok(res_json) = response.json::<serde_json::Value>().await
            && let Some(results) = res_json.get("results").and_then(|r| r.as_array())
        {
            let mut out = String::new();
            out.push_str(&format!(
                "Web Search Results for '{}' (via Exa AI):\n\n",
                search_query
            ));
            for (i, r) in results.iter().enumerate() {
                let title = r
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("No Title");
                let url = r.get("url").and_then(|u| u.as_str()).unwrap_or("");
                let text = r.get("text").and_then(|t| t.as_str()).unwrap_or("");
                let snippet = clip_snippet(text);

                out.push_str(&format!(
                    "{}. {}\n   Snippet: {}\n   Source: {}\n\n",
                    i + 1,
                    title,
                    snippet.trim(),
                    url
                ));
            }
            if !results.is_empty() {
                return Ok(out);
            }
        }
    }

    if let Some(api_key) = crate::shell_env::env_var("TAVILY_API_KEY") {
        let body = serde_json::json!({
            "api_key": api_key,
            "query": search_query,
            "max_results": 5
        });

        let response = client
            .post("https://api.tavily.com/search")
            .timeout(std::time::Duration::from_secs(10))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Tavily request failed: {e}"))?;

        if response.status().is_success() {
            let res_json: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("failed to parse Tavily JSON: {e}"))?;

            if let Some(results) = res_json.get("results").and_then(|r| r.as_array()) {
                let mut out = String::new();
                out.push_str(&format!(
                    "Web Search Results for '{}' (via Tavily):\n\n",
                    search_query
                ));
                for (i, r) in results.iter().enumerate() {
                    let title = r
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("No Title");
                    let url = r.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    let content =
                        clip_snippet(r.get("content").and_then(|c| c.as_str()).unwrap_or(""));

                    out.push_str(&format!(
                        "{}. {}\n   Snippet: {}\n   Source: {}\n\n",
                        i + 1,
                        title,
                        content.trim(),
                        url
                    ));
                }
                if !results.is_empty() {
                    return Ok(out);
                }
            }
        }
    }

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(&search_query)
    );

    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .send()
        .await
        .map_err(|e| format!("failed to request search results: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "web search failed with status: {}",
            response.status()
        ));
    }

    let html_content = response
        .text()
        .await
        .map_err(|e| format!("failed to read search response body: {e}"))?;

    if html_content.contains("anomaly-modal") || html_content.contains("bots use DuckDuckGo too") {
        return Err("Web search failed because DuckDuckGo triggered bot/CAPTCHA protection.\n\
                   To bypass this and get reliable web search, please sign up for a free Tavily account (1,000 free searches/mo) at https://tavily.com and set the TAVILY_API_KEY environment variable.".to_string());
    }

    let document = scraper::Html::parse_document(&html_content);

    let result_selector = scraper::Selector::parse(".result").unwrap();
    let snippet_selector = scraper::Selector::parse(".result__snippet").unwrap();
    let url_selector = scraper::Selector::parse(".result__url").unwrap();

    let mut out = String::new();
    out.push_str(&format!(
        "Web Search Results for '{}' (via DuckDuckGo):\n\n",
        search_query
    ));

    let mut count = 0;
    for element in document.select(&result_selector) {
        if count >= 6 {
            break;
        }

        let snippet_node = element.select(&snippet_selector).next();
        let url_node = element.select(&url_selector).next();

        if let (Some(s_node), Some(u_node)) = (snippet_node, url_node) {
            let snippet = s_node
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            let link = u_node
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();

            count += 1;
            out.push_str(&format!(
                "{}. Snippet: {}\n   Source: https://{}\n\n",
                count, snippet, link
            ));
        }
    }

    if count == 0 {
        return Ok("No results found. Try refining your query.".to_string());
    }

    Ok(out)
}
