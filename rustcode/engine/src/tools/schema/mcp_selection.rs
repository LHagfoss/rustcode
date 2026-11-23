use super::*;

fn context_terms(messages: &[Value]) -> std::collections::HashSet<String> {
    // Function words say nothing about which tool is wanted, and nearly every
    // tool description contains a few of them: "and", "of" and "can" alone
    // cleared the relevance threshold and filled the menu for any prompt.
    use crate::tools::mcp_discovery::STOP_WORDS;
    let mut terms = std::collections::HashSet::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        // A `user` row the harness wrote (a tool result rendered as text, a
        // tool attachment, a runtime notice) is tool output, not the user's
        // words: a `manage_mcp_servers` listing there named every server.
        if role == "user" && !crate::network::messages::is_user_turn_boundary(message) {
            continue;
        }
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        let content = content.split("<rustcode_context>").next().unwrap_or("");
        let mut message_terms = std::collections::HashSet::new();
        for token in content
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .map(str::to_ascii_lowercase)
        {
            if token.len() >= 2 && !STOP_WORDS.contains(&token.as_str()) {
                if matches!(token.as_str(), "mail" | "mails" | "emails") {
                    message_terms.insert("email".to_string());
                }
                message_terms.insert(token);
            }
        }
        if message_terms.contains("email")
            && ["latest", "newest", "recent"]
                .iter()
                .any(|term| message_terms.contains(*term))
        {
            message_terms.insert("list".to_string());
        }
        terms.extend(message_terms);
        // Structured tool turns often have an empty assistant content field.
        // Keep their names in the relevance set so the next request retains
        // the exact schema needed to continue the active tool exchange.
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                if let Some(name) = call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .or_else(|| call.get("name").and_then(Value::as_str))
                {
                    terms.insert(name.to_ascii_lowercase());
                }
            }
        }
    }
    terms
}

fn tool_name_was_used(name: &str, messages: &[Value]) -> bool {
    messages.iter().any(|message| {
        message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    call.get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        .is_some_and(|call_name| call_name.eq_ignore_ascii_case(name))
                        || call
                            .get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|call_name| call_name.eq_ignore_ascii_case(name))
                })
            })
    })
}

fn user_message_mentions_name(messages: &[Value], name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    messages.iter().any(|message| {
        // Only what the user typed. A tool result the request carries as a
        // `user` row (`<tool_result>…`) is not: a `manage_mcp_servers` listing
        // rendered that way named every configured server, and each server's
        // tools were then admitted as if the user had asked for them.
        if !crate::network::messages::is_user_turn_boundary(message) {
            return false;
        }
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .split("<rustcode_context>")
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut offset = 0;
        while let Some(found) = content[offset..].find(&name) {
            let start = offset + found;
            let end = start + name.len();
            let before_is_name_char = content[..start]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            let after_is_name_char = content[end..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            if !before_is_name_char && !after_is_name_char {
                return true;
            }
            offset = end;
        }
        false
    })
}

fn canonical_mcp_server(name: &str) -> Option<&str> {
    name.strip_prefix("mcp__")?
        .split_once("__")
        .map(|(server, _)| server)
}

fn user_mentions_mcp_server(messages: &[Value], server: &str) -> bool {
    user_message_mentions_name(messages, server)
        || user_message_mentions_name(messages, &format!("{server}_mcp"))
}

/// Keep schemas returned by completed MCP discovery calls in the next
/// request's tool block. The discovery result itself is tool output, which
/// relevance scoring intentionally ignores; without this hint, a tool found
/// by query could be omitted before the model gets a chance to call it.
///
/// Every discovery call in the request counts, newest first: binding only
/// the latest one let finding tool B evict tool A, and binding only the
/// current turn's dropped what an earlier turn found, so the model listed
/// the same tools again on every turn (#2502).
fn mcp_tools_selected_by_discovery_calls(
    tools: &[(String, String, Value)],
    owners: &[String],
    raw_names: &[String],
    messages: &[Value],
) -> Vec<String> {
    let docs = tools
        .iter()
        .zip(owners)
        .zip(raw_names)
        .map(|(((name, description, _), owner), raw_name)| {
            crate::tools::mcp_discovery::McpToolDoc {
                server: owner,
                raw_name,
                advertised: name,
                description,
            }
        })
        .collect::<Vec<_>>();
    crate::tools::mcp_discovery::bound_by_discovery_calls(&docs, messages)
        .into_iter()
        .map(|index| tools[index].0.clone())
        .collect()
}

/// Find MCP schemas named by the user, including every tool from a named MCP
/// server. This intentionally reads user messages only: assistant/tool output
/// can describe a tool without being an instruction to make its whole server
/// sticky.
///
/// Returns the tools asked for one by one, in the order they must be admitted
/// (a discovery result first, then names the user wrote), and separately the
/// tools that are only wanted because their server was named. A server can
/// have more tools than a request may carry, so those are ranked by the
/// caller instead of all being treated as equally required.
fn explicitly_requested_mcp_tool_names(
    tools: &[(String, String, Value)],
    owners: &[String],
    raw_names: &[String],
    messages: &[Value],
) -> (Vec<String>, std::collections::HashSet<String>) {
    let mut named = mcp_tools_selected_by_discovery_calls(tools, owners, raw_names, messages);
    let mut requested = std::collections::HashSet::new();
    for (name, _, _) in tools {
        if user_message_mentions_name(messages, name) {
            if !named.contains(name) {
                named.push(name.clone());
            }
        } else if canonical_mcp_server(name)
            .is_some_and(|server| user_mentions_mcp_server(messages, server))
        {
            requested.insert(name.clone());
        }
    }

    // Unique MCP tool names are intentionally sent without a server prefix.
    // Recover their server here so an explicit server name still pins them.
    if let Ok(registry) = crate::mcp::get_mcp_registry().lock() {
        let mut clients = registry.values().cloned().collect::<Vec<_>>();
        clients.sort_by(|left, right| left.name.cmp(&right.name));
        for client in &clients {
            let server_requested = user_mentions_mcp_server(messages, &client.name)
                || user_message_mentions_name(
                    messages,
                    &client
                        .name
                        .chars()
                        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                        .collect::<String>(),
                );
            if !server_requested {
                continue;
            }
            let Ok(client_tools) = client.get_tools() else {
                continue;
            };
            for tool in client_tools {
                let Some(raw_name) = tool.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let provider_name =
                    mcp_canonical_name_for_clients(&client.name, raw_name, &clients);
                if tools.iter().any(|(name, _, _)| name == &provider_name)
                    || tools.iter().any(|(name, _, _)| name == raw_name)
                {
                    requested.insert(
                        tools
                            .iter()
                            .find(|(name, _, _)| name == &provider_name || name == raw_name)
                            .map(|(name, _, _)| name.clone())
                            .unwrap_or(provider_name),
                    );
                }
            }
        }
    }
    requested.retain(|name| !named.contains(name));
    (named, requested)
}

/// Words that say what to do or name a kind of thing nearly every messaging,
/// file or task server offers. They say nothing about *which* tool is meant:
/// "chat", "msg", "reply" and "before" in a Minecraft permissions transcript
/// pulled a Snapchat reader into the menu (#2576). Compared after stemming.
const GENERIC_RELEVANCE_TERMS: &[&str] = &[
    "add",
    "after",
    "all",
    "before",
    "chat",
    "check",
    "clear",
    "close",
    "com",
    "conversation",
    "create",
    "data",
    "default",
    "delete",
    "detail",
    "dm",
    "entry",
    "fetch",
    "fill",
    "find",
    "first",
    "get",
    "http",
    "https",
    "id",
    "info",
    "item",
    "last",
    "latest",
    "limit",
    "list",
    "load",
    "member",
    "message",
    "msg",
    "name",
    "net",
    "new",
    "next",
    "open",
    "org",
    "page",
    "read",
    "recent",
    "remove",
    "reply",
    "result",
    "run",
    "save",
    "search",
    "send",
    "set",
    "show",
    "start",
    "status",
    "stop",
    "take",
    "text",
    "update",
    "user",
    "value",
    "view",
    "write",
    "www",
];

/// How strongly the user's words point at one MCP tool. Only the tool's name
/// and description count: its schema's parameter names (`before`, `limit`,
/// `chat_id`) are shared by unrelated tools and are not what a user asks for.
fn mcp_tool_relevance(
    name: &str,
    description: &str,
    terms: &std::collections::HashSet<String>,
) -> usize {
    mcp_tool_score(name, description, terms, false)
}

/// Rank tools of a server the user already named. There the generic verbs
/// are what tells `send_chat_message` from `list_chats`, so they count.
fn mcp_tool_rank_within_server(
    name: &str,
    description: &str,
    terms: &std::collections::HashSet<String>,
) -> usize {
    mcp_tool_score(name, description, terms, true)
}

/// Whether one of `terms` (not a generic one) is a word of the tool's name.
/// Description words alone are weak: "issue" in a sentence about "issue
/// tracking" or "token" in an auth note matched tools nobody asked for.
fn mcp_tool_name_matches(name: &str, terms: &std::collections::HashSet<String>) -> bool {
    use crate::tools::mcp_discovery::{stem, tokens};
    let name = name.strip_prefix("mcp__").unwrap_or(name);
    let name_terms = tokens(name)
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    terms
        .iter()
        .map(|term| stem(term))
        .any(|term| !GENERIC_RELEVANCE_TERMS.contains(&term.as_str()) && name_terms.contains(&term))
}

fn mcp_tool_score(
    name: &str,
    description: &str,
    terms: &std::collections::HashSet<String>,
    count_generic: bool,
) -> usize {
    use crate::tools::mcp_discovery::{stem, tokens};
    // `mcp__` is a transport prefix, not evidence that every MCP tool is
    // relevant whenever a user names an MCP server. The tokenizer is the one
    // discovery uses, so `snake_case`, `camelCase` and plurals match alike.
    let name = name.strip_prefix("mcp__").unwrap_or(name);
    let name_terms = tokens(name)
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let description_terms = tokens(description)
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let mut score = 0;
    let mut seen = std::collections::HashSet::new();
    for term in terms {
        // The prompt may carry the verb inflected ("takes" for `take_…`).
        let term = stem(term);
        let generic = GENERIC_RELEVANCE_TERMS.contains(&term.as_str());
        if (generic && !count_generic) || !seen.insert(term.clone()) {
            continue;
        }
        if name_terms.contains(&term) {
            score += if generic { 3 } else { 8 };
        } else if description_terms.contains(&term) {
            score += 3;
        }
    }
    score
}

#[cfg(test)]
pub(in crate::tools) fn select_mcp_tools_for_context_in_phase(
    tools: &[(String, String, Value)],
    messages: &[Value],
    phase: ToolSchemaPhase,
) -> (Vec<usize>, McpSchemaSelectionStats) {
    select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
        tools,
        &[],
        &[],
        messages,
        &[],
        phase,
    )
}

/// Which candidate bucket admitted a tool into this request's `tools` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admitted {
    /// Carried over from the previous request's pinned menu.
    Retained,
    Requested,
    Previous,
    Relevant,
}

#[cfg(test)]
pub(crate) fn select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
    tools: &[(String, String, Value)],
    owners: &[String],
    always_include_servers: &[String],
    messages: &[Value],
    sticky_names: &[String],
    phase: ToolSchemaPhase,
) -> (Vec<usize>, McpSchemaSelectionStats) {
    let raw_names = tools
        .iter()
        .zip(owners)
        .map(|((name, _, _), owner)| {
            name.strip_prefix(&mcp_canonical_name(owner, ""))
                .unwrap_or(name)
                .to_string()
        })
        .collect::<Vec<_>>();
    select_mcp_tools_for_context_with_raw_names(
        tools,
        owners,
        &raw_names,
        always_include_servers,
        messages,
        sticky_names,
        phase,
    )
}

#[cfg(test)]
pub(in crate::tools) fn select_mcp_tools_for_context_with_raw_names(
    tools: &[(String, String, Value)],
    owners: &[String],
    raw_names: &[String],
    always_include_servers: &[String],
    messages: &[Value],
    sticky_names: &[String],
    phase: ToolSchemaPhase,
) -> (Vec<usize>, McpSchemaSelectionStats) {
    select_mcp_tools_for_context_with_anchors(
        tools,
        owners,
        raw_names,
        always_include_servers,
        messages,
        sticky_names,
        &McpSessionAnchors::default(),
        phase,
    )
}

/// The server a tool belongs to: its registry owner, else the server in its
/// canonical `mcp__server__tool` name.
fn mcp_tool_server<'a>(
    tools: &'a [(String, String, Value)],
    owners: &'a [String],
    index: usize,
) -> Option<&'a str> {
    owners
        .get(index)
        .map(String::as_str)
        .filter(|owner| !owner.is_empty())
        .or_else(|| canonical_mcp_server(&tools[index].0))
}

#[allow(clippy::too_many_arguments)]
pub(in crate::tools) fn select_mcp_tools_for_context_with_anchors(
    tools: &[(String, String, Value)],
    owners: &[String],
    raw_names: &[String],
    always_include_servers: &[String],
    messages: &[Value],
    sticky_names: &[String],
    anchors: &McpSessionAnchors,
    phase: ToolSchemaPhase,
) -> (Vec<usize>, McpSchemaSelectionStats) {
    // Once a menu is pinned, only what the user wrote may add to it. Assistant
    // prose, tool calls and harness notices grow every round, and each tool
    // they pulled in rewrote the `tools` block the provider had cached.
    let authored = messages
        .iter()
        .filter(|message| crate::network::messages::is_user_turn_boundary(message))
        .cloned()
        .collect::<Vec<_>>();
    let terms = if sticky_names.is_empty() {
        context_terms(messages)
    } else {
        context_terms(&authored)
    };
    // The pinned menu outlives the turn, so a full one would never admit a
    // tool by relevance again. What the newest user message points at is
    // therefore seated ahead of the pin. It is the same on every round of a
    // turn, so it changes the menu at most once per user message.
    let fresh_terms = match authored.last() {
        Some(latest) if !sticky_names.is_empty() => context_terms(std::slice::from_ref(latest)),
        _ => std::collections::HashSet::new(),
    };
    let (mut named, server_wide) =
        explicitly_requested_mcp_tool_names(tools, owners, raw_names, messages);
    // What the user asked for earlier in the session stays asked for after a
    // compaction archived the message that said so (#2576).
    for wanted in &anchors.requested {
        if tools.iter().any(|(name, _, _)| name == wanted) && !named.contains(wanted) {
            named.push(wanted.clone());
        }
    }
    let mut requested = named
        .iter()
        .filter_map(|wanted| tools.iter().position(|(name, _, _)| name == wanted))
        .collect::<Vec<_>>();
    let mut server_requested = Vec::new();
    let mut previous = Vec::new();
    let mut relevant = Vec::new();
    let mut fresh = Vec::new();
    for (index, (name, description, _)) in tools.iter().enumerate() {
        if named.contains(name) {
            continue;
        }
        if !fresh_terms.is_empty() {
            let score = mcp_tool_relevance(name, description, &fresh_terms);
            if score >= MCP_RELEVANCE_THRESHOLD {
                fresh.push((index, score));
            }
        }
        // The model may call a tool by its advertised or its canonical
        // `mcp__server__tool` name; either is a use (#2502). A bare raw name
        // that is not the advertised one could be a built-in's, so it is not.
        // A call a compaction archived still counts (#2576).
        let raw = raw_names.get(index).map(String::as_str).unwrap_or(name);
        let canonical = owners
            .get(index)
            .map(|owner| mcp_canonical_name(owner, raw))
            .unwrap_or_else(|| name.clone());
        let was_used = anchors.used.contains(name)
            || tool_name_was_used(name, messages)
            || tool_name_was_used(&canonical, messages);
        if server_wide.contains(name) {
            server_requested.push((
                index,
                was_used,
                mcp_tool_rank_within_server(name, description, &terms),
            ));
        } else if was_used {
            previous.push(index);
        } else {
            let score = mcp_tool_relevance(name, description, &terms);
            // A single generic description word such as "result", "file", or
            // "project" must not pull a large MCP suite into an unrelated
            // request. Exact name terms score eight; two strong description
            // matches score six.
            if score >= MCP_RELEVANCE_THRESHOLD {
                relevant.push((index, score));
            }
        }
    }

    // Relevance from the transcript may only widen the menu within servers the
    // session has already engaged: one it used, one the user or a discovery
    // call named, or a reserved one. A server new to the session is admitted
    // only by what the user just wrote ("check my latest mails"), never by
    // words older turns, assistant prose or a compaction left behind; the
    // model can find anything else with `list_mcp_tools` (#2576).
    let latest_terms = authored
        .last()
        .map(|latest| context_terms(std::slice::from_ref(latest)))
        .unwrap_or_default();
    let mut engaged_servers = std::collections::HashSet::new();
    for index in requested
        .iter()
        .chain(server_requested.iter().map(|(index, _, _)| index))
        .chain(previous.iter())
        .copied()
    {
        if let Some(server) = mcp_tool_server(tools, owners, index) {
            engaged_servers.insert(server);
        }
    }
    for index in 0..tools.len() {
        if let Some(server) = mcp_tool_server(tools, owners, index) {
            if always_include_servers
                .iter()
                .any(|reserved| reserved == server)
                || user_mentions_mcp_server(messages, server)
            {
                engaged_servers.insert(server);
            }
        }
    }
    for ranked in [&mut relevant, &mut fresh] {
        ranked.retain(|(index, _)| {
            mcp_tool_server(tools, owners, *index)
                .is_some_and(|server| engaged_servers.contains(server))
                || (mcp_tool_relevance(&tools[*index].0, &tools[*index].1, &latest_terms)
                    >= MCP_RELEVANCE_THRESHOLD
                    // A server new to the session needs the message to name
                    // what the tool is; description words alone added an
                    // unrelated tool and rewrote the cached `tools` block.
                    && mcp_tool_name_matches(&tools[*index].0, &latest_terms))
        });
        // Two servers offering the same tool name with the same score are an
        // ambiguous match: neither is admitted rather than the one whose
        // server sorts first (#2576).
        let snapshot = ranked.clone();
        ranked.retain(|(index, score)| {
            let raw = raw_names
                .get(*index)
                .map(String::as_str)
                .unwrap_or(&tools[*index].0);
            !snapshot.iter().any(|(other, other_score)| {
                other != index
                    && other_score == score
                    && raw_names
                        .get(*other)
                        .map(String::as_str)
                        .unwrap_or(&tools[*other].0)
                        == raw
                    && mcp_tool_server(tools, owners, *other)
                        != mcp_tool_server(tools, owners, *index)
            })
        });
    }
    // A named server can have more tools than the request may carry. The ones
    // already in use and the ones the request's words point at go first;
    // alphabetical order used to drop `send_…` behind `get_…` and `list_…`.
    server_requested.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| tools[left.0].0.cmp(&tools[right.0].0))
    });
    requested.extend(server_requested.into_iter().map(|(index, _, _)| index));
    previous.sort_by(|left, right| tools[*left].0.cmp(&tools[*right].0));
    for ranked in [&mut relevant, &mut fresh] {
        // Tied duplicates across servers were dropped above, so the name order
        // here only seats two different tools, never picks a server.
        ranked.sort_by(|(left_index, left_score), (right_index, right_score)| {
            right_score
                .cmp(left_score)
                .then_with(|| tools[*left_index].0.cmp(&tools[*right_index].0))
        });
    }

    // Reserve complete configured server toolsets first. A reservation is
    // all-or-none: if adding the complete set would exceed either hard limit,
    // the server is reported as rejected and none of its tools are bound for
    // this request.
    let budget_bytes = mcp_schema_budget_bytes();
    let mut selected = Vec::new();
    let mut selected_indices = std::collections::HashSet::new();
    let mut rejected_indices = std::collections::HashSet::new();
    let mut selected_schema_bytes: usize = 0;
    let mut per_server_bytes: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    let mut schema_budget_exhausted = false;
    let mut reserved_servers = Vec::new();
    let mut rejected_reservations = Vec::new();
    let mut seen_servers = std::collections::HashSet::new();
    for server in always_include_servers {
        if !seen_servers.insert(server.as_str()) {
            continue;
        }
        let group = owners
            .iter()
            .enumerate()
            .filter_map(|(index, owner)| (owner == server).then_some(index))
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        let group_bytes = group.iter().fold(0usize, |total, index| {
            let (name, description, schema) = &tools[*index];
            total.saturating_add(mcp_schema_bytes(name, description, schema))
        });
        // A single heavy server must not starve the rest of the catalog.
        if group_bytes > MAX_MCP_PER_SERVER_SCHEMA_BYTES {
            rejected_reservations.push(server.clone());
            schema_budget_exhausted = true;
            rejected_indices.extend(group);
            continue;
        }
        let count_fits = selected.len().saturating_add(group.len()) <= MAX_MCP_NATIVE_SCHEMAS;
        let bytes_fit = selected_schema_bytes.saturating_add(group_bytes) <= budget_bytes;
        if count_fits && bytes_fit {
            for index in group {
                if selected_indices.insert(index) {
                    selected.push(index);
                }
            }
            selected_schema_bytes = selected_schema_bytes.saturating_add(group_bytes);
            per_server_bytes.insert(server.as_str(), group_bytes);
            reserved_servers.push(server.clone());
        } else {
            rejected_reservations.push(server.clone());
            schema_budget_exhausted |= !bytes_fit;
            rejected_indices.extend(group);
        }
    }

    // Explicitly requested tools stay first: naming one is a hard requirement, so it
    // must outrank a pinned menu from a previous turn. Retention then follows.
    // Because the pinned menu is itself the previous request's whole `tools` block,
    // a requested name is almost always already pinned, so this ordering cannot
    // shrink the pinned set — it only re-seats its members. Chaining retention last
    // instead left it as a mere preference competing for an already-full budget,
    // which is why consecutive requests selected disjoint name sets. (#1591)
    let mut candidates = Vec::new();
    candidates.extend(
        requested
            .iter()
            .copied()
            .filter(|index| !rejected_indices.contains(index))
            .map(|index| (index, Admitted::Requested)),
    );
    candidates.extend(
        fresh
            .iter()
            .map(|(index, _)| *index)
            .filter(|index| !rejected_indices.contains(index))
            .map(|index| {
                // A pinned tool the message also points at was retained, not
                // newly admitted; only its seat moved.
                if sticky_names.contains(&tools[index].0) {
                    (index, Admitted::Retained)
                } else {
                    (index, Admitted::Relevant)
                }
            }),
    );
    // Tools the model has called in this conversation outrank the rest of
    // the pin: a full pin of tools nobody used kept a used one out, and the
    // model listed it again every turn (#2502). A used tool is almost always
    // pinned already, so this rarely changes the menu.
    candidates.extend(
        previous
            .iter()
            .copied()
            .filter(|index| !rejected_indices.contains(index))
            // A used tool is admitted for being used, pinned or not, so the
            // log says why it is there (#2576).
            .map(|index| (index, Admitted::Previous)),
    );
    candidates.extend(
        sticky_names
            .iter()
            .filter_map(|sticky| tools.iter().position(|(name, _, _)| name == sticky))
            .filter(|index| !rejected_indices.contains(index))
            .map(|index| (index, Admitted::Retained)),
    );
    candidates.extend(
        relevant
            .iter()
            .map(|(index, _)| *index)
            .filter(|index| !rejected_indices.contains(index))
            .map(|index| (index, Admitted::Relevant)),
    );
    // Attribute by the bucket that actually admitted the tool. Deriving the
    // counts from the candidate lists instead skipped tools a reservation had
    // already inserted, so a `previous` tool admitted by a reservation was
    // selected but never reported as previously used. (#1591)
    let mut admitted = std::collections::HashMap::new();
    for (index, bucket) in candidates {
        if selected.len() >= MAX_MCP_NATIVE_SCHEMAS {
            break;
        }
        if !selected_indices.insert(index) {
            continue;
        }
        let (name, description, schema) = &tools[index];
        let bytes = mcp_schema_bytes(name, description, schema);
        if selected_schema_bytes.saturating_add(bytes) > budget_bytes {
            selected_indices.remove(&index);
            schema_budget_exhausted = true;
            continue;
        }
        // Per-server fairness: an explicit request always wins, but relevance
        // alone must not let one heavy server consume the shared budget.
        let owner = owners.get(index).map(String::as_str).unwrap_or("");
        let server_bytes = per_server_bytes.get(owner).copied().unwrap_or(0);
        if bucket != Admitted::Requested
            && server_bytes.saturating_add(bytes) > MAX_MCP_PER_SERVER_SCHEMA_BYTES
        {
            selected_indices.remove(&index);
            schema_budget_exhausted = true;
            continue;
        }
        admitted.entry(index).or_insert(bucket);
        selected.push(index);
        selected_schema_bytes = selected_schema_bytes.saturating_add(bytes);
        if !owner.is_empty() {
            per_server_bytes.insert(owner, server_bytes.saturating_add(bytes));
        }
    }

    let mut fallback_count = 0;
    if selected.is_empty() && phase == ToolSchemaPhase::Established {
        for preferred in MCP_DISCOVERY_CORE {
            if selected.len() >= MCP_DISCOVERY_FALLBACK_COUNT {
                break;
            }
            if let Some(index) = tools.iter().position(|(name, _, _)| name == preferred) {
                if rejected_indices.contains(&index) {
                    continue;
                }
                if !selected_indices.insert(index) {
                    continue;
                }
                let (name, description, schema) = &tools[index];
                let bytes = mcp_schema_bytes(name, description, schema);
                if selected_schema_bytes.saturating_add(bytes) <= budget_bytes {
                    selected.push(index);
                    selected_schema_bytes = selected_schema_bytes.saturating_add(bytes);
                } else {
                    selected_indices.remove(&index);
                    schema_budget_exhausted = true;
                }
            }
        }
        fallback_count = selected.len();
    }

    // Sticky and explicit tools can be promoted within the already selected
    // surface. Reserved server tools stay first so a follow-up cannot break a
    // complete configured reservation.
    if !sticky_names.is_empty() {
        let reserved_indices = owners
            .iter()
            .enumerate()
            .filter_map(|(index, owner)| reserved_servers.contains(owner).then_some(index))
            .filter(|index| selected_indices.contains(index))
            .collect::<Vec<_>>();
        let sticky_indices = sticky_names
            .iter()
            .filter_map(|name| tools.iter().position(|(tool_name, _, _)| tool_name == name))
            .collect::<Vec<_>>();
        let mut prioritized = Vec::with_capacity(selected.len());
        let mut prioritized_set = std::collections::HashSet::with_capacity(selected.len());
        for index in reserved_indices
            .into_iter()
            .chain(requested.iter().copied())
            .chain(sticky_indices)
            .chain(selected.iter().copied())
        {
            if prioritized.len() >= MAX_MCP_NATIVE_SCHEMAS {
                break;
            }
            if selected_indices.contains(&index) && prioritized_set.insert(index) {
                prioritized.push(index);
            }
        }
        selected = prioritized;
        selected_schema_bytes = 0;
        let mut within_budget = Vec::with_capacity(selected.len());
        for index in selected {
            let (name, description, schema) = &tools[index];
            let bytes = mcp_schema_bytes(name, description, schema);
            if selected_schema_bytes.saturating_add(bytes) > budget_bytes {
                schema_budget_exhausted = true;
                continue;
            }
            selected_schema_bytes = selected_schema_bytes.saturating_add(bytes);
            within_budget.push(index);
        }
        selected = within_budget;
    }

    // Emit in a stable order: the pinned menu keeps its seats in the order the
    // previous request sent them, and a newly admitted tool goes after them.
    // Sorting by catalog position put a new tool in the middle, which changed
    // every byte after it and cost the provider's cached prefix.
    let mut seen = std::collections::HashSet::with_capacity(selected.len());
    selected.retain(|index| seen.insert(*index));
    let pinned_seat = |index: usize| {
        sticky_names
            .iter()
            .position(|name| name == &tools[index].0)
            .unwrap_or(usize::MAX)
    };
    selected.sort_by_key(|index| (pinned_seat(*index), *index));
    debug_assert!(selected.len() <= MAX_MCP_NATIVE_SCHEMAS);
    let selected_set = selected
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let selected_names = selected
        .iter()
        .map(|index| tools[*index].0.clone())
        .collect::<Vec<_>>();
    let omitted_names = tools
        .iter()
        .enumerate()
        .filter_map(|(index, (name, _, _))| {
            (!selected_set.contains(&index)).then_some(name.clone())
        })
        .collect::<Vec<_>>();
    // Each selected tool under the one bucket that admitted it; the counts
    // are the sizes of these lists, so they always add up to `selected`.
    let mut admitted_by: std::collections::BTreeMap<&'static str, Vec<String>> =
        std::collections::BTreeMap::new();
    for index in &selected {
        // Having been called is the strongest reason a tool is offered, so a
        // used tool is reported as such unless the user asked for it by name.
        let used = previous.contains(index) && admitted.get(index) != Some(&Admitted::Requested);
        let bucket = match admitted.get(index) {
            _ if used => "previously_used",
            Some(Admitted::Requested) => "requested",
            Some(Admitted::Previous) => "previously_used",
            Some(Admitted::Relevant) => "relevant",
            Some(Admitted::Retained) => "retained",
            None if mcp_tool_server(tools, owners, *index)
                .is_some_and(|server| reserved_servers.iter().any(|name| name == server)) =>
            {
                "reserved"
            }
            None => "fallback",
        };
        admitted_by
            .entry(bucket)
            .or_default()
            .push(tools[*index].0.clone());
    }
    let bucket_len = |bucket: &str| admitted_by.get(bucket).map_or(0, Vec::len);
    let stats = McpSchemaSelectionStats {
        available: tools.len(),
        selected: selected.len(),
        requested: bucket_len("requested"),
        relevant: bucket_len("relevant"),
        previously_used: bucket_len("previously_used"),
        fallback: fallback_count.min(selected.len()),
        omitted: tools.len().saturating_sub(selected.len()),
        selected_names,
        omitted_names,
        reserved_servers,
        rejected_reservations,
        phase,
        mcp_schema_bytes: selected_schema_bytes,
        mcp_schema_budget_bytes: budget_bytes,
        schema_budget_exhausted,
        admitted_by,
        ..Default::default()
    };
    (selected, stats)
}

#[cfg(test)]
pub(in crate::tools) fn select_mcp_tools_for_context(
    tools: &[(String, String, Value)],
    messages: &[Value],
) -> (Vec<usize>, McpSchemaSelectionStats) {
    select_mcp_tools_for_context_in_phase(tools, messages, ToolSchemaPhase::Established)
}

#[cfg(test)]
pub(in crate::tools) fn select_mcp_tools_for_context_with_sticky_in_phase(
    tools: &[(String, String, Value)],
    messages: &[Value],
    sticky_names: &[String],
    phase: ToolSchemaPhase,
) -> (Vec<usize>, McpSchemaSelectionStats) {
    select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
        tools,
        &[],
        &[],
        messages,
        sticky_names,
        phase,
    )
}

#[cfg(test)]
pub(in crate::tools) fn select_mcp_tools_for_context_with_sticky(
    tools: &[(String, String, Value)],
    messages: &[Value],
    sticky_names: &[String],
) -> (Vec<usize>, McpSchemaSelectionStats) {
    select_mcp_tools_for_context_with_sticky_in_phase(
        tools,
        messages,
        sticky_names,
        ToolSchemaPhase::Established,
    )
}
