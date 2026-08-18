use serde_json::Value;

use super::{Tool, ToolCapability, ToolSafety};

fn ask_question_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "question": { "type": "string", "description": "Question to ask the user" },
            "options": { "type": "array", "items": { "type": "string" }, "description": "Choices shown to the user" },
            "is_multi_select": { "type": "boolean", "default": false }
        },
        "required": ["question", "options"]
    })
}

pub const ASK_QUESTION: Tool = Tool {
    name: "ask_question",
    description: "Ask the user a multiple-choice question to clarify underspecified requirements, solicit design choices, or select an option. Only call this when explicit user validation or decision-making is needed. Do not use for trivial yes/no or routine commands. The UI automatically appends a 'write your own answer' slot for free-form text, so never add your own 'Other' option and never pass an empty options list.",
    arguments: r#"{"question": "The question title or description to ask", "options": ["Option 1 text", "Option 2 text", "Option 3 text"], "is_multi_select": false}"#,
    handler: ask_question,
    requires_confirmation: false,
    schema: ask_question_schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

fn get_time_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {}, "additionalProperties": false
    })
}

pub const GET_TIME: Tool = Tool {
    name: "get_time",
    description: "Get the current local date and time",
    arguments: r#"{} (no arguments)"#,
    handler: get_time,
    requires_confirmation: false,
    schema: get_time_schema,
    capabilities: &[],
    safety: ToolSafety::ReadOnly,
};

fn search_web_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "query": { "type": "string" }, "domain": { "type": "string" }
        }, "required": ["query"]
    })
}

pub const SEARCH_WEB: Tool = Tool {
    name: "search_web",
    description: "Performs a web search to look up documentation, API details, or code patterns.",
    arguments: r#"{"query": "search query terms", "domain": "optional domain filter e.g. 'docs.rs'"}"#,
    handler: search_web,
    requires_confirmation: false,
    schema: search_web_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::ReadOnly,
};

fn complete_task_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": { "result": { "type": "string" } }, "required": ["result"]
    })
}

pub const COMPLETE_TASK: Tool = Tool {
    name: "complete_task",
    description: "Mark the continuous goal/task as successfully complete.",
    arguments: r#"{"result": "summary of what was achieved and final results"}"#,
    handler: complete_task_tool,
    requires_confirmation: false,
    schema: complete_task_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::Unknown,
};

fn remember_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "description": "Unique key for the fact (e.g. 'package_manager', 'db_port', 'test_runner')" },
            "value": { "type": "string", "description": "The concise fact or rule to remember (max 512 characters)" },
            "category": { "type": "string", "description": "Optional category tag (e.g. 'build', 'architecture', 'convention', 'preference')", "default": "general" },
            "scope": { "type": "string", "enum": ["project", "global"], "description": "Scope of the memory. 'project' (default) is scoped to the current repository; 'global' applies across all projects.", "default": "project" }
        },
        "required": ["key", "value"]
    })
}

pub const REMEMBER: Tool = Tool {
    name: "remember",
    description: "Store a concise, high-value fact, user preference, architecture detail, or convention into persistent memory. Use this when explicitly asked by the user to remember something, or when a durable project convention is established. Do not store secrets, tokens, or entire files.",
    arguments: r#"{"key": "package_manager", "value": "Use pnpm for all install and build commands", "category": "build", "scope": "project"}"#,
    handler: remember,
    requires_confirmation: false,
    schema: remember_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::Unknown,
};

fn recall_memory_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Search query terms to match against stored memory keys, categories, and values" },
            "scope": { "type": "string", "enum": ["all", "project", "global"], "description": "Scope to search. Defaults to 'all'.", "default": "all" }
        },
        "required": ["query"]
    })
}

pub const RECALL_MEMORY: Tool = Tool {
    name: "recall_memory",
    description: "Search persistent project and global memory for remembered facts, user preferences, architecture decisions, or build instructions matching a query.",
    arguments: r#"{"query": "database port", "scope": "all"}"#,
    handler: recall_memory,
    requires_confirmation: false,
    schema: recall_memory_schema,
    capabilities: &[],
    safety: ToolSafety::ReadOnly,
};

fn forget_memory_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "description": "The exact key or category of the fact to remove" },
            "scope": { "type": "string", "enum": ["project", "global", "all"], "description": "Scope to remove from. Defaults to 'project'.", "default": "project" }
        },
        "required": ["key"]
    })
}

pub const FORGET_MEMORY: Tool = Tool {
    name: "forget_memory",
    description: "Remove a fact from persistent memory by key or category. Use when a remembered fact is obsolete, contradicted, or when asked by the user to forget something.",
    arguments: r#"{"key": "package_manager", "scope": "project"}"#,
    handler: forget_memory,
    requires_confirmation: false,
    schema: forget_memory_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::Unknown,
};

fn use_skill_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"]
    })
}

pub const USE_SKILL: Tool = Tool {
    name: "use_skill",
    description: "Load a skill by name to get its instructions and available files. Read-only call: can be issued in parallel with other read operations or multiple skills.",
    arguments: r#"{"name": "skill name"}"#,
    handler: use_skill,
    requires_confirmation: false,
    schema: use_skill_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::ReadOnly,
};

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

pub fn remember(args: &Value) -> Result<String, String> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("missing required argument 'key'")?;
    let value = args
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or("missing required argument 'value'")?;
    let category = args
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("project");

    let fact = crate::memory::fact(category, key, value, "agent memory tool");
    if scope == "global" {
        crate::memory::upsert_global(fact)?;
        Ok(format!("Remembered globally: [{category}] {key} = {value}"))
    } else {
        crate::memory::upsert(None, fact)?;
        Ok(format!("Remembered for this project: [{category}] {key} = {value}"))
    }
}

pub fn recall_memory(args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("missing required argument 'query'")?;
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("all");

    let facts = crate::memory::search_facts(None, query, scope);
    if facts.is_empty() {
        return Ok(format!("No remembered facts found matching '{query}'."));
    }

    let mut output = format!("Found {} relevant memory item(s):\n", facts.len());
    for (item_scope, fact) in facts {
        output.push_str(&format!(
            "- ({item_scope}) [{}] {}: {}\n",
            fact.category, fact.key, fact.value
        ));
    }
    Ok(output.trim_end().to_string())
}

pub fn forget_memory(args: &Value) -> Result<String, String> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("missing required argument 'key'")?;
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("project");

    let mut total_removed = 0;
    if scope == "all" || scope == "global" {
        if let Ok(removed) = crate::memory::remove_global(key) {
            total_removed += removed;
        }
    }
    if scope == "all" || scope == "project" {
        if let Ok(removed) = crate::memory::remove(None, key) {
            total_removed += removed;
        }
    }

    if total_removed == 0 {
        Ok(format!("No memory facts found for '{key}' in scope '{scope}'."))
    } else {
        Ok(format!("Removed {total_removed} fact(s) matching '{key}'."))
    }
}
