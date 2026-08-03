use serde_json::Value;
pub fn ask_question(args: &Value) -> Result<String, String> {
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .ok_or("missing 'question'")?;
    let options = args
        .get("options")
        .and_then(|v| v.as_array())
        .ok_or("missing 'options'")?;
    let is_multi_select = args
        .get("is_multi_select")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut out = format!("ASK_QUESTION: {} | Multi: {}", question, is_multi_select);
    for (i, opt) in options.iter().enumerate() {
        out.push_str(&format!("\n{}. {}", i + 1, opt.as_str().unwrap_or("")));
    }
    out.push_str("\nOther: (type custom response)");
    Ok(out)
}

pub fn get_time(_args: &Value) -> Result<String, String> {
    Ok(chrono::Local::now()
        .format("%A %Y-%m-%d %H:%M:%S")
        .to_string())
}

pub fn complete_task_tool(args: &Value) -> Result<String, String> {
    let result = args
        .get("result")
        .and_then(|r| r.as_str())
        .ok_or("missing 'result' argument")?;
    Ok(format!(
        "Task successfully marked as complete! Result: {result}"
    ))
}

pub fn search_web(args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|q| q.as_str())
        .ok_or("missing 'query' argument")?;
    let domain = args.get("domain").and_then(|d| d.as_str());

    let mut search_query = query.to_string();
    if let Some(dom) = domain {
        search_query.push_str(&format!(" site:{}", dom));
    }

    let exa_key = std::env::var("EXA_API_KEY")
        .unwrap_or_else(|_| "9a49efa5-675c-4684-94c0-3f96979aa2ac".to_string());
    if !exa_key.is_empty() {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

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
            .header("x-api-key", &exa_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            && response.status().is_success()
            && let Ok(res_json) = response.json::<serde_json::Value>()
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
                let snippet = if text.len() > 300 { &text[..300] } else { text };

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

    if let Ok(api_key) = std::env::var("TAVILY_API_KEY") {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let body = serde_json::json!({
            "api_key": api_key,
            "query": search_query,
            "max_results": 5
        });

        let response = client
            .post("https://api.tavily.com/search")
            .json(&body)
            .send()
            .map_err(|e| format!("Tavily request failed: {e}"))?;

        if response.status().is_success() {
            let res_json: serde_json::Value = response
                .json()
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
                    let content = r.get("content").and_then(|c| c.as_str()).unwrap_or("");

                    out.push_str(&format!(
                        "{}. {}\n   Snippet: {}\n   Source: {}\n\n",
                        i + 1,
                        title,
                        content,
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

    let client = reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("failed to request search results: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "web search failed with status: {}",
            response.status()
        ));
    }

    let html_content = response
        .text()
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

pub fn use_skill(args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("missing 'name' argument")?;

    let skill = crate::skills::get_skill_content(name).ok_or_else(|| {
        format!(
            "Skill '{}' not found. Use the skills catalog to see available skills.",
            name
        )
    })?;

    let files = crate::skills::list_skill_files(&skill.path);
    let mut out = format!("<skill_content name=\"{}\">\n", skill.name);
    out.push_str(&skill.content);
    if !files.is_empty() {
        out.push_str("\n---\nFiles in skill directory:\n");
        for f in &files {
            out.push_str(&format!("  - {}\n", f));
        }
    }
    out.push_str(
        "\n---\n<harness_execution_paths>\n\
  <path tool=\"run_command\" available=\"true\">Use this registered tool for CLI workflows explicitly described by the skill.</path>\n\
  <path tool=\"native_registry\" available=\"true\">Only tools listed in the current tool inventory are executable as native tools.</path>\n\
  <path tool=\"unknown_native_tools\" available=\"false\">A skill cannot create or imply a native tool that is absent from the registry.</path>\n\
</harness_execution_paths>\n",
    );
    out.push_str("</skill_content>\n");
    Ok(out)
}
