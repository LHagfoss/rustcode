use super::super::mcp_discovery::{DiscoveryArgs, McpServerState, McpServerStatus};
use super::{
    LIST_MCP_TOOLS, McpDiscoveryTool, clip_snippet, format_mcp_discovery, list_mcp_tools,
    scoped_search_query, search_web_async,
};
use serde_json::{Value, json};

#[test]
fn search_query_adds_a_site_filter_only_when_useful() {
    assert_eq!(scoped_search_query("rust acp", None), "rust acp");
    assert_eq!(scoped_search_query("rust acp", Some("  ")), "rust acp");
    assert_eq!(
        scoped_search_query("rust acp", Some("docs.rs")),
        "rust acp site:docs.rs"
    );
    assert_eq!(
        scoped_search_query("rust site:docs.rs", Some("docs.rs")),
        "rust site:docs.rs"
    );
}

#[test]
fn snippets_are_clipped_on_a_character_boundary() {
    let text = "ø".repeat(400);
    let snippet = clip_snippet(&text);
    assert!(snippet.len() <= 300);
    assert!(snippet.chars().all(|c| c == 'ø'));
    assert_eq!(clip_snippet("short"), "short");
}

fn tool(server: &str, name: &str, description: &str, schema: Value) -> McpDiscoveryTool {
    McpDiscoveryTool {
        server: server.to_owned(),
        name: name.to_owned(),
        advertised: name.to_owned(),
        callable_name: format!("mcp__{server}__{name}"),
        input_schema: schema,
        description: description.to_owned(),
    }
}

fn discovery_fixture() -> Vec<McpDiscoveryTool> {
    let object = json!({"type":"object"});
    let mut tools = (0..12)
            .map(|index| {
                tool(
                    "calendar",
                    &format!("event_{index:02}"),
                    &format!(
                        "Create and update calendar events with attendee metadata and long description {index}"
                    ),
                    object.clone(),
                )
            })
            .collect::<Vec<_>>();
    tools.extend([
        tool(
            "mail",
            "search_messages",
            "Search email messages by sender or subject",
            json!({"type":"object","properties":{"query":{"type":"string"}}}),
        ),
        tool(
            "mail",
            "send_message",
            "Send an email message",
            object.clone(),
        ),
        tool(
            "snapchat",
            "getFriends",
            "List the people on your Snapchat account",
            object.clone(),
        ),
        tool(
            "snapchat",
            "send_snap",
            "Send a snap to a friend",
            object.clone(),
        ),
        tool("snapchat", "get_stories", "Read stories", object.clone()),
        tool(
            "discord",
            "list_channels",
            "List channels in a guild",
            object.clone(),
        ),
        tool(
            "discord",
            "send_message",
            "Send a message to a Discord channel",
            object,
        ),
    ]);
    tools
}

fn statuses(tools: &[McpDiscoveryTool]) -> Vec<McpServerStatus> {
    let mut running = std::collections::BTreeMap::<String, Vec<String>>::new();
    for tool in tools {
        running
            .entry(tool.server.clone())
            .or_default()
            .push(tool.name.clone());
    }
    let mut statuses = running
        .into_iter()
        .map(|(name, tools)| McpServerStatus {
            name,
            state: McpServerState::Running { tools },
        })
        .collect::<Vec<_>>();
    statuses.extend([
        McpServerStatus {
            name: "daily-dev".to_owned(),
            state: McpServerState::NotRunning,
        },
        McpServerStatus {
            name: "github-old".to_owned(),
            state: McpServerState::Disabled,
        },
        McpServerStatus {
            name: "broken".to_owned(),
            state: McpServerState::Failed("spawn npx ENOENT".to_owned()),
        },
    ]);
    statuses.sort_by(|a, b| a.name.cmp(&b.name));
    statuses
}

fn discover_raw(args: Value) -> String {
    let tools = discovery_fixture();
    format_mcp_discovery(
        &tools,
        &statuses(&tools),
        &DiscoveryArgs::parse(&args).unwrap(),
    )
    .unwrap()
}

fn discover(args: Value) -> Value {
    serde_json::from_str(&discover_raw(args)).unwrap()
}

fn names(result: &Value) -> Vec<String> {
    result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            format!(
                "{}.{}",
                row["server"].as_str().unwrap(),
                row["name"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn mcp_inventory_has_a_strict_discovery_argument_schema() {
    let schema = (LIST_MCP_TOOLS.schema)();
    assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("discovery options should be declared");
    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        ["server", "query", "select", "limit"]
    );
    assert_eq!(properties["server"]["type"], "string");
    assert_eq!(properties["query"]["type"], "string");
    assert_eq!(properties["select"]["type"], "array");
    assert_eq!(properties["limit"]["maximum"], 50);
    assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
    assert_eq!(LIST_MCP_TOOLS.safety, super::ToolSafety::ReadOnly);
    assert!(!LIST_MCP_TOOLS.requires_confirmation);
    // The description keeps the model from using it for anything but MCP.
    assert!(LIST_MCP_TOOLS.description.starts_with("MCP tools only"));
    assert!(LIST_MCP_TOOLS.description.contains("never guess"));
    assert!(LIST_MCP_TOOLS.arguments.contains("select"));
}

#[test]
fn mcp_default_inventory_names_every_server_with_its_state_and_tools() {
    let tools = discovery_fixture();
    let result = discover(json!({}));
    let servers = result["servers"].as_array().unwrap();
    let by_name = |name: &str| {
        servers
            .iter()
            .find(|server| server["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing from {servers:?}"))
    };
    assert_eq!(by_name("calendar")["tool_count"], 12);
    assert_eq!(by_name("calendar")["status"], "running (12 tools)");
    assert!(
        by_name("snapchat")["tools"]
            .as_array()
            .unwrap()
            .contains(&json!("getFriends"))
    );
    assert_eq!(by_name("github-old")["status"], "disabled");
    assert_eq!(by_name("daily-dev")["status"], "not running");
    assert_eq!(by_name("broken")["status"], "failed: spawn npx ENOENT");
    assert!(by_name("broken").get("tools").is_none());

    // Names only: far smaller than every description and schema.
    let legacy = serde_json::to_string(&tools.iter().map(|tool| json!({
            "name": tool.name, "description": tool.description, "input_schema": tool.input_schema
        })).collect::<Vec<_>>())
        .unwrap();
    let raw = discover_raw(json!({}));
    assert!(
        raw.len() < legacy.len(),
        "{} vs {}",
        raw.len(),
        legacy.len()
    );
}

#[test]
fn mcp_discovery_searches_metadata_and_bounds_results() {
    let result = discover(json!({"query": "calendar", "limit": 4}));
    assert_eq!(result["total"], 12);
    assert_eq!(result["results"].as_array().unwrap().len(), 4);
    assert_eq!(result["truncated"], true);

    let result = discover(json!({"query": "sender"}));
    assert_eq!(result["results"][0]["name"], "search_messages");
    assert_eq!(
        result["results"][0]["callable_name"],
        "mcp__mail__search_messages"
    );
    assert_eq!(
        result["results"][0]["input_schema"]["properties"]["query"]["type"],
        "string"
    );
}

#[test]
fn the_server_name_and_camel_case_words_count_as_query_terms() {
    // Both used to find nothing: the server name was not a term and
    // `getFriends` was one word.
    let result = discover(json!({"query": "snapchat friend"}));
    let found = names(&result);
    assert_eq!(
        found.first().map(String::as_str),
        Some("snapchat.getFriends")
    );
    assert!(
        found.iter().all(|name| name.starts_with("snapchat.")),
        "{found:?}"
    );
    assert!(
        !found.contains(&"snapchat.get_stories".to_owned()),
        "{found:?}"
    );

    let result = discover(json!({"query": "discord channels"}));
    assert_eq!(
        names(&result).first().map(String::as_str),
        Some("discord.list_channels")
    );
}

#[test]
fn a_long_noisy_query_returns_few_results_under_the_byte_cap() {
    let raw = discover_raw(json!({
        "query": "please find the mcp tool to send a message to a friend on snapchat with short text",
        "limit": 50
    }));
    let result: Value = serde_json::from_str(&raw).unwrap();
    let found = names(&result);
    assert_eq!(
        found.first().map(String::as_str),
        Some("snapchat.send_snap"),
        "{found:?}"
    );
    assert!(found.len() <= 3, "{found:?}");
    assert!(raw.len() < super::super::mcp_discovery::DISCOVERY_OUTPUT_BYTES);
}

#[test]
fn a_large_catalogue_is_listed_within_the_byte_cap() {
    let huge = json!({"type":"object","properties": (0..200)
            .map(|index| (format!("field_{index}"), json!({"type":"string","description":"x".repeat(40)})))
            .collect::<serde_json::Map<_, _>>()});
    let tools = (0..50)
        .map(|index| {
            tool(
                "big",
                &format!("record_{index:02}"),
                &format!("Record keeping operation {index} {}", "detail ".repeat(40)),
                huge.clone(),
            )
        })
        .collect::<Vec<_>>();
    let raw = format_mcp_discovery(
        &tools,
        &statuses(&tools),
        &DiscoveryArgs::parse(&json!({"query": "record", "limit": 50})).unwrap(),
    )
    .unwrap();
    let result: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(result["total"], 50);
    assert_eq!(result["truncated"], true);
    assert!(
        raw.len() < super::super::mcp_discovery::DISCOVERY_OUTPUT_BYTES + 1024,
        "{} bytes",
        raw.len()
    );
    // The top results are still bound, with their oversized schemas
    // carried by the next request's tool list instead of this result.
    assert!(result["results"][0].get("input_schema_omitted").is_some());
    assert!(result["results"][0].get("input_schema").is_none());
}

#[test]
fn a_guessed_server_lists_the_valid_servers_instead_of_failing() {
    let result = discover(json!({"server": "github"}));
    let hint = result["hint"].as_str().unwrap();
    assert!(hint.contains("Unknown MCP server 'github'"), "{hint}");
    let valid = result["valid_servers"].as_array().unwrap();
    assert!(
        valid.contains(&json!("snapchat: running (3 tools)")),
        "{valid:?}"
    );
    assert!(valid.contains(&json!("github-old: disabled")), "{valid:?}");

    let result = discover(json!({"server": "snapchat-mcp", "query": "friends"}));
    let hint = result["hint"].as_str().unwrap();
    assert!(hint.contains("the closest is 'snapchat'"), "{hint}");
    assert_eq!(
        names(&result).first().map(String::as_str),
        Some("snapchat.getFriends")
    );

    for server in ["*", "any", "all", " "] {
        let result = discover(json!({"server": server, "query": "mail"}));
        assert!(result.get("valid_servers").is_none(), "{server}: {result}");
        assert!(!names(&result).is_empty(), "{server}");
    }
}

#[test]
fn discovery_names_a_server_by_server_or_select_prefix() {
    let parse = |value| super::super::mcp_discovery::DiscoveryArgs::parse(&value).unwrap();
    let names = |value, server| super::discovery_names_server(&parse(value), server);
    assert!(names(json!({"server": "dathost-ftp"}), "dathost-ftp"));
    assert!(names(
        json!({"select": ["dathost-ftp.list-directory"]}),
        "dathost-ftp"
    ));
    assert!(names(
        json!({"query": "select:mcp__dathost_ftp__list-directory"}),
        "dathost-ftp"
    ));
    assert!(!names(
        json!({"select": ["dathost-ftp-old.list"]}),
        "dathost-ftp"
    ));
    assert!(!names(json!({"query": "ftp files"}), "dathost-ftp"));
}

#[test]
fn disabled_failed_and_stopped_servers_are_reported_by_state() {
    let hint = discover(json!({"server": "github-old"}))["hint"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(hint.contains("'github-old' is disabled"), "{hint}");
    let hint = discover(json!({"server": "broken"}))["hint"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(hint.contains("failed to start: spawn npx ENOENT"), "{hint}");
    let hint = discover(json!({"server": "daily-dev"}))["hint"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        hint.contains("'daily-dev' is configured but not running"),
        "{hint}"
    );

    let result = discover(json!({"query": "daily dev feed"}));
    assert_eq!(result["total"], 0);
    assert_eq!(result["not_running"], json!(["broken", "daily-dev"]));
}

#[test]
fn select_resolves_bare_dotted_and_canonical_names_with_full_schemas() {
    let result = discover(json!({"select": [
        "getFriends",
        "discord.list_channels",
        "mcp__mail__search_messages",
        "send_mesage"
    ]}));
    assert_eq!(
        names(&result),
        [
            "snapchat.getFriends",
            "discord.list_channels",
            "mail.search_messages"
        ]
    );
    for row in result["results"].as_array().unwrap() {
        assert!(row.get("input_schema").is_some(), "{row}");
        assert!(
            row["callable_name"].as_str().unwrap().starts_with("mcp__"),
            "{row}"
        );
    }
    let missing = &result["not_found"][0];
    assert_eq!(missing["name"], "send_mesage");
    let suggestions = missing["suggestions"].as_array().unwrap();
    assert!(
        suggestions.contains(&json!("mcp__discord__send_message")),
        "{suggestions:?}"
    );
}

#[test]
fn a_server_listing_names_large_toolsets_without_schemas() {
    let result = discover(json!({"server": "calendar"}));
    let rows = result["results"].as_array().unwrap();
    assert_eq!(rows.len(), 12);
    assert!(rows.iter().all(|row| row["server"] == "calendar"));
    assert!(rows.iter().all(|row| row.get("input_schema").is_none()));
    assert!(result["hint"].as_str().unwrap().contains("select"));

    let result = discover(json!({"server": "snapchat"}));
    let rows = result["results"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| row.get("input_schema").is_some()));
}

#[test]
fn strict_handlers_accept_the_call_summary_every_schema_offers() {
    // Every built-in schema advertises `summary`; rejecting it failed
    // each discovery call a model made with one.
    for (name, args) in [
        (
            "list_mcp_tools",
            json!({"query": "files", "summary": "Find file tools"}),
        ),
        ("zoom_context", json!({"summary": "Open the archive"})),
    ] {
        let output = super::super::execute_with_metadata(name, &args);
        assert!(
            !output.content.contains("invalid argument")
                && !output.content.contains("Unknown zoom_context argument"),
            "{name}: {}",
            output.content
        );
    }
}

#[test]
fn mcp_discovery_rejects_invalid_arguments_explicitly() {
    assert!(
            list_mcp_tools(&json!({"unexpected": true}))
                .unwrap_err()
                .contains(
                    "invalid argument 'unexpected': list_mcp_tools accepts only server, query, select, limit; remove 'unexpected' and retry"
                )
        );
    assert!(
        list_mcp_tools(&json!({"limit": 0}))
            .unwrap_err()
            .contains("'limit' must be an integer")
    );
    assert!(
        list_mcp_tools(&json!({"query": "  "}))
            .unwrap_err()
            .contains("'query' must be a non-empty string")
    );
    assert!(
        list_mcp_tools(&json!({"select": [1]}))
            .unwrap_err()
            .contains("'select' must be an array")
    );
}

#[test]
fn a_page_is_reduced_to_its_title_and_visible_text() {
    let text = super::readable_page_text(
        "<html><head><title> Release notes </title><style>p{color:red}</style></head>\
             <body><script>track()</script><h1>Version 7</h1>\
             <p>Expert   offload is <b>experimental</b>.</p><ul><li>one</li><li>two</li></ul>\
             </body></html>",
    );
    assert_eq!(
        text,
        "# Release notes\nVersion 7\nExpert offload is experimental .\none\ntwo"
    );
}

#[test]
fn a_long_page_is_read_in_slices_that_say_how_to_continue() {
    let first = super::page_slice("https://example.test", "abcdefghij", 0, 4);
    assert!(first.contains("characters 0-4 of 10"), "{first}");
    assert!(first.contains("\nabcd\n"), "{first}");
    assert!(first.contains("offset=4"), "{first}");
    let last = super::page_slice("https://example.test", "abcdefghij", 8, 4);
    assert!(last.ends_with("\nij"), "{last}");
    assert!(super::read_url(&json!({"url": "file:///etc/passwd"})).is_err());
}

#[tokio::test]
async fn async_search_web_requires_query() {
    let error = search_web_async(&json!({}), &reqwest::Client::new())
        .await
        .expect_err("missing query should fail before any request");

    assert_eq!(error, "missing 'query' argument");
}
