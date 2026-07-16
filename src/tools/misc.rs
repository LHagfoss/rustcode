use chrono;
use reqwest;
use scraper;
use urlencoding;

pub fn get_time(_args: &Value) -> Result<String, String> {
    Ok(chrono::Local::now()
        .format("%A %Y-%m-%d %H:%M:%S")
        .to_string())
}

pub fn check_match(args: &Value) -> Result<String, String> {
    use reqwest::blocking::Client;
    use std::fmt::Write as _;

    let team = args.get("team").and_then(|v| v.as_str()).unwrap_or("");
    let date = args.get("date").and_then(|v| v.as_str()).unwrap_or("");

    if date.is_empty() {
        return Err("date parameter required (YYYY-MM-DD format)".to_string());
    }

    let client = Client::new();
    let api_key = "fb492b51acab4d134f2d33ef9777865a";
    let url = if !team.is_empty() {
        format!(
            "https://v3.football.api-sports.io/fixtures?date={}&team={}",
            date, team
        )
    } else {
        format!("https://v3.football.api-sports.io/fixtures?date={}", date)
    };

    let response = client
        .get(&url)
        .header("x-apisports-key", api_key)
        .send()
        .map_err(|e| format!("API request failed: {}", e))?
        .json::<serde_json::Value>()
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    let matches = match response.get("response") {
        Some(serde_json::Value::Array(matches)) => matches,
        _ => return Ok("No matches found".to_string()),
    };

    if matches.is_empty() {
        return Ok(format!("Found 0 match(es) for {}", date));
    }

    let mut output = String::new();
    write!(output, "Found {} match(es),\n", matches.len()).unwrap();
    writeln!(
        output,
        "══════════════════════════════════════════════════════"
    )
    .unwrap();

    for (i, match_data) in matches.iter().enumerate() {
        let teams = &match_data["teams"];
        let home = &teams["home"]["name"];
        let away = &teams["away"]["name"];

        let goals = &match_data["goals"];
        let score_home = goals
            .get("home")
            .and_then(|v| v.as_i64())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());
        let score_away = goals
            .get("away")
            .and_then(|v| v.as_i64())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());

        let status_info = &match_data["status"];
        let long_status = status_info
            .get("long")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let elapsed = status_info.get("elapsed").and_then(|v| v.as_i64());

        let ts = match_data["fixture"]["timestamp"].as_i64().unwrap_or(0);
        let dt = chrono::DateTime::from_timestamp(ts, 0)
            .map(|d| d.format("%H:%M UTC").to_string())
            .unwrap_or_else(|| "Invalid time".to_string());

        let league_name = match_data["league"]["name"].as_str().unwrap_or("Unknown");
        let country = match_data["league"]["country"].as_str().unwrap_or("");

        if i > 0 {
            writeln!(
                output,
                "\n──────────────────────────────────────────────────────"
            )
            .unwrap();
        }

        write!(output, "Match {}:\n", i + 1).unwrap();
        write!(output, "League: {}\n", league_name).unwrap();
        write!(output, "Country: {}\n", country).unwrap();
        write!(output, "Time: {}\n", dt).unwrap();

        if let Some(minutes) = elapsed {
            writeln!(output, "Status: LIVE - Minute {}", minutes).unwrap();
        } else {
            writeln!(output, "Status: {}", long_status).unwrap();
        }

        writeln!(output, "Teams: {} vs {}", home, away).unwrap();
        writeln!(output, "Score: {} - {}", score_home, score_away).unwrap();
    }

    Ok(output)
}

pub fn complete_task_tool(args: &Value) -> Result<String, String> {
    let result = args
        .get("result")
        .and_then(|r| r.as_str())
        .ok_or("missing 'result' argument")?;
    Ok(format!("Task successfully marked as complete! Result: {result}"))
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
