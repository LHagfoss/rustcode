use super::{TOOLS, ToolCapability, allowed_in_plan_mode};
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock};

/// Agent tools that live outside the `TOOLS` table. `(name, description, args)`
/// mirrors what `tool_system_prompt` lists for the text protocols, reused here
/// to build the native function schema.
pub(super) const AGENT_TOOL_SPECS: &[(&str, &str, &str)] = &[
    (
        "spawn_agent",
        "Start an asynchronous read-only subagent and return its id. Use wait_agent for completion. Write access, allowed paths, and verification must be explicit.",
        r#"{"task": "task description", "write_access": false, "allowed_paths": ["src/"], "verification_command": "cargo test"}"#,
    ),
    (
        "send_agent",
        "Start one follow-up turn for a completed subagent. Running subagents reject follow-ups.",
        r#"{"id": "subagent id", "message": "message text"}"#,
    ),
    (
        "set_goal",
        "Set a new long-running task and switch the agent to continuous autoloop mode.",
        r#"{"goal": "goal description"}"#,
    ),
    (
        "todo_write",
        "Replace the persistent task plan with a list of steps.",
        r#"{"todos": "list of steps, each with content, status and priority"}"#,
    ),
];

/// Selects the tool schemas visible to one provider request.
///
/// This is intentionally request-scoped: a child request must not infer its
/// capabilities from the parent session's mutable delegation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolSchemaPolicy {
    pub(crate) include_agent_tools: bool,
    pub(crate) include_mcp_tools: bool,
}

impl ToolSchemaPolicy {
    pub(crate) const fn root(include_agent_tools: bool) -> Self {
        Self {
            include_agent_tools,
            include_mcp_tools: true,
        }
    }

    pub(crate) const fn subagent() -> Self {
        Self {
            include_agent_tools: false,
            include_mcp_tools: false,
        }
    }
}

/// Derive a permissive JSON Schema object from a tool's human-readable
/// `arguments` string (e.g. `{"path": "file path", "start_line": optional}`).
/// Every parameter is declared as an optional `string`; the tool handlers
/// already coerce strings to numbers/bools (see `parse_json_number`/
/// `parse_json_bool`), so this stays correct without a real schema per tool.
pub(super) fn schema_from_arguments(arguments: &str) -> Value {
    let mut properties = serde_json::Map::new();
    let bytes = arguments.as_bytes();
    let read_string = |start: usize| -> (String, usize) {
        // `start` points just past the opening quote; returns (contents, index of closing quote).
        let mut j = start;
        while j < bytes.len() && bytes[j] != b'"' {
            if bytes[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        (arguments[start..j.min(arguments.len())].to_string(), j)
    };
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let (token, end) = read_string(i + 1);
        // A key is a string immediately followed (past whitespace) by ':'.
        let mut k = end + 1;
        while k < bytes.len() && (bytes[k] as char).is_whitespace() {
            k += 1;
        }
        if k < bytes.len() && bytes[k] == b':' {
            // Optional description: the following string literal, if any.
            let mut m = k + 1;
            while m < bytes.len() && (bytes[m] as char).is_whitespace() {
                m += 1;
            }
            // An array-literal value (`[...]`) marks a structured param even
            // when no description string follows it (e.g. `options`).
            let is_array_literal = m < bytes.len() && bytes[m] == b'[';
            let desc = if m < bytes.len() && bytes[m] == b'"' {
                read_string(m + 1).0
            } else {
                String::new()
            };
            let mut prop = serde_json::Map::new();
            // Params whose description says "array" carry structured JSON (e.g.
            // `edits`, `replacements`). Advertising them as `string` makes
            // strict ApiNative providers stringify the value, which the handlers
            // then fail to read via `as_array()`. Emit a real array schema so the
            // model passes structured data. Everything else stays an optional
            // string (handlers coerce scalars via parse_json_number/bool).
            if is_array_literal || desc.to_lowercase().contains("array") {
                prop.insert("type".into(), Value::String("array".into()));
                prop.insert("items".into(), serde_json::json!({ "type": "object" }));
            } else {
                prop.insert("type".into(), Value::String("string".into()));
            }
            if !desc.is_empty() {
                prop.insert("description".into(), Value::String(desc));
            }
            properties
                .entry(token)
                .or_insert_with(|| Value::Object(prop));
        }
        i = end + 1;
    }
    serde_json::json!({ "type": "object", "properties": properties })
}

/// Build the OpenAI-style `tools` array sent in the request when the tool
/// protocol is `ApiNative`. Covers the built-in `TOOLS`, any MCP tools (which
/// carry real JSON Schemas), and the agent tools.
/// Gather all connected MCP tools as `(name, description, input_schema)`,
/// sorted by name for a deterministic, cache-stable ordering. Shared by both
/// the native schema builder and the text-protocol prompt listing.
pub(super) fn mcp_canonical_name(server: &str, tool: &str) -> String {
    let server = server
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    format!("mcp__{server}__{tool}")
}

pub(super) fn mcp_canonical_name_for_clients(
    server: &str,
    tool: &str,
    clients: &[Arc<crate::mcp::McpClient>],
) -> String {
    let base = mcp_canonical_name(server, tool);
    let collides = clients
        .iter()
        .filter(|client| mcp_canonical_name(&client.name, tool) == base)
        .count()
        > 1;
    if !collides {
        return base;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    server.hash(&mut hasher);
    format!("{base}__{:016x}", hasher.finish())
}

pub(super) fn mcp_raw_name_is_unique(name: &str, clients: &[Arc<crate::mcp::McpClient>]) -> bool {
    if TOOLS.iter().any(|tool| tool.name == name)
        || AGENT_TOOL_SPECS.iter().any(|(tool, _, _)| *tool == name)
    {
        return false;
    }
    clients
        .iter()
        .filter_map(|client| client.get_tools().ok())
        .flatten()
        .filter(|tool| tool.get("name").and_then(|value| value.as_str()) == Some(name))
        .count()
        == 1
}

pub(super) fn collect_mcp_tools() -> Vec<(String, String, Value)> {
    let mut discovered = Vec::new();
    let mut clients_for_names = Vec::new();
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        let mut clients = reg.values().cloned().collect::<Vec<_>>();
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        clients_for_names = clients.clone();
        for client in &clients {
            if let Ok(mcp_tools) = client.get_tools() {
                for tool in mcp_tools {
                    let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name.is_empty() {
                        continue;
                    }
                    let desc = tool
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    let schema = tool
                        .get("inputSchema")
                        .filter(|schema| {
                            schema.is_object()
                                && schema.get("type").and_then(Value::as_str) == Some("object")
                        })
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
                    discovered.push((client.name.clone(), name.to_string(), desc, schema));
                }
            }
        }
    }
    discovered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut counts = HashMap::new();
    for (_, name, _, _) in &discovered {
        *counts.entry(name.clone()).or_insert(0usize) += 1;
    }
    let mut out = Vec::new();
    let mut emitted = std::collections::HashSet::new();
    for (server, raw_name, desc, schema) in discovered {
        let qualified = counts.get(&raw_name).copied().unwrap_or(0) > 1
            || TOOLS.iter().any(|tool| tool.name == raw_name)
            || AGENT_TOOL_SPECS
                .iter()
                .any(|(name, _, _)| *name == raw_name);
        let name = if qualified {
            mcp_canonical_name_for_clients(&server, &raw_name, &clients_for_names)
        } else {
            raw_name
        };
        if emitted.insert(name.clone()) {
            out.push((name, desc, schema));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub(crate) const MAX_MCP_NATIVE_SCHEMAS: usize = 16;
pub(super) const MCP_DISCOVERY_FALLBACK_COUNT: usize = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct McpSchemaSelectionStats {
    pub available: usize,
    pub selected: usize,
    pub relevant: usize,
    pub previously_used: usize,
    pub fallback: usize,
    pub omitted: usize,
    pub selected_names: Vec<String>,
}

fn build_builtin_native_tools_schema(include_agent_tools: bool) -> Vec<Value> {
    let mut tools = Vec::new();
    for t in TOOLS {
        if t.capabilities.contains(&ToolCapability::AgentDelegation) && !include_agent_tools {
            continue;
        }
        tools.push(serde_json::json!({
            "type": "function",
            "function": {
                "name": t.name,
                "description": t.description,
                "parameters": provider_compatible_schema(schema_for_tool(t.name)),
            }
        }));
    }
    tools
}

fn builtin_native_tools_schema(include_agent_tools: bool) -> Vec<Value> {
    static WITHOUT_AGENT_TOOLS: LazyLock<Vec<Value>> =
        LazyLock::new(|| build_builtin_native_tools_schema(false));
    static WITH_AGENT_TOOLS: LazyLock<Vec<Value>> =
        LazyLock::new(|| build_builtin_native_tools_schema(true));

    if include_agent_tools {
        WITH_AGENT_TOOLS.clone()
    } else {
        WITHOUT_AGENT_TOOLS.clone()
    }
}

fn build_agent_native_tools_schema(include_agent_tools: bool) -> Vec<Value> {
    let mut tools = Vec::new();
    if include_agent_tools {
        for (name, desc, _args) in AGENT_TOOL_SPECS {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": desc,
                    "parameters": provider_compatible_schema(schema_for_agent_tool(name)),
                }
            }));
        }
    }
    tools
}

fn agent_native_tools_schema(include_agent_tools: bool) -> Vec<Value> {
    static WITHOUT_AGENT_TOOLS: LazyLock<Vec<Value>> = LazyLock::new(Vec::new);
    static WITH_AGENT_TOOLS: LazyLock<Vec<Value>> =
        LazyLock::new(|| build_agent_native_tools_schema(true));

    if include_agent_tools {
        WITH_AGENT_TOOLS.clone()
    } else {
        WITHOUT_AGENT_TOOLS.clone()
    }
}

fn mcp_schema_value(name: &str, desc: &str, schema: &Value) -> Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": name,
            "description": desc,
            "parameters": provider_compatible_schema(schema.clone())
        }
    })
}

/// Return a provider-facing copy using the common subset of JSON Schema
/// accepted by OpenAI-compatible function-calling endpoints. Canonical tool
/// schemas remain unchanged for RustCode's stricter runtime validation.
pub(super) fn provider_compatible_schema(mut schema: Value) -> Value {
    match &mut schema {
        Value::Object(object) => {
            object.remove("$schema");
            object.remove("$id");
            if let Some(bound) = object.remove("exclusiveMinimum") {
                object.entry("minimum").or_insert(bound);
            }
            for value in object.values_mut() {
                *value = provider_compatible_schema(value.take());
            }
        }
        Value::Array(values) => {
            for value in values {
                *value = provider_compatible_schema(value.take());
            }
        }
        _ => {}
    }
    schema
}

fn context_terms(messages: &[Value]) -> std::collections::HashSet<String> {
    const STOP_WORDS: &[&str] = &[
        "about", "after", "again", "also", "been", "before", "being", "could", "from", "have",
        "into", "just", "like", "more", "most", "only", "please", "should", "that", "their",
        "there", "these", "this", "through", "using", "want", "what", "when", "where", "which",
        "with", "would", "your",
    ];
    let mut terms = std::collections::HashSet::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        for token in content
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .map(str::to_ascii_lowercase)
        {
            if token.len() >= 2 && !STOP_WORDS.contains(&token.as_str()) {
                terms.insert(token);
            }
        }
    }
    terms
}

fn tool_name_was_used(name: &str, messages: &[Value]) -> bool {
    let needle = name.to_ascii_lowercase();
    messages.iter().any(|message| {
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array)
            && calls.iter().any(|call| {
                call.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .is_some_and(|call_name| call_name.eq_ignore_ascii_case(name))
                    || call
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|call_name| call_name.eq_ignore_ascii_case(name))
            })
        {
            return true;
        }
        message
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.to_ascii_lowercase().contains(&needle))
    })
}

fn mcp_tool_relevance(
    name: &str,
    description: &str,
    schema: &Value,
    terms: &std::collections::HashSet<String>,
) -> usize {
    fn token_matches(candidate: &str, term: &str) -> bool {
        candidate == term
            || (candidate.len() > 3 && candidate.strip_suffix('s') == Some(term))
            || (term.len() > 3 && term.strip_suffix('s') == Some(candidate))
    }
    let name_terms: Vec<String> = name
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let description_terms: Vec<String> = description
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let schema_terms: Vec<String> = serde_json::to_string(schema)
        .unwrap_or_default()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut score = 0;
    for term in terms {
        if name_terms
            .iter()
            .any(|candidate| token_matches(candidate, term))
        {
            score += 8;
        } else if description_terms
            .iter()
            .any(|candidate| token_matches(candidate, term))
        {
            score += 3;
        } else if schema_terms
            .iter()
            .any(|candidate| token_matches(candidate, term))
        {
            score += 1;
        }
    }
    score
}

pub(super) fn select_mcp_tools_for_context(
    tools: &[(String, String, Value)],
    messages: &[Value],
) -> (Vec<usize>, McpSchemaSelectionStats) {
    let terms = context_terms(messages);
    let mut previous = Vec::new();
    let mut relevant = Vec::new();
    for (index, (name, description, schema)) in tools.iter().enumerate() {
        if tool_name_was_used(name, messages) {
            previous.push(index);
        } else {
            let score = mcp_tool_relevance(name, description, schema, &terms);
            if score > 0 {
                relevant.push((index, score));
            }
        }
    }
    previous.sort_by(|left, right| tools[*left].0.cmp(&tools[*right].0));
    relevant.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| tools[*left_index].0.cmp(&tools[*right_index].0))
    });

    let mut selected = previous
        .iter()
        .copied()
        .take(MAX_MCP_NATIVE_SCHEMAS)
        .collect::<Vec<_>>();
    let previously_used_count = selected.len();
    selected.extend(
        relevant
            .iter()
            .map(|(index, _)| *index)
            .take(MAX_MCP_NATIVE_SCHEMAS.saturating_sub(selected.len())),
    );
    let relevant_count = selected.len().saturating_sub(previously_used_count);
    let mut fallback_count = 0;
    if selected.is_empty() {
        fallback_count = tools.len().min(MCP_DISCOVERY_FALLBACK_COUNT);
        selected.extend(0..fallback_count);
    }
    selected.sort_unstable();
    selected.dedup();
    debug_assert!(selected.len() <= MAX_MCP_NATIVE_SCHEMAS);
    let selected_names = selected
        .iter()
        .map(|index| tools[*index].0.clone())
        .collect();
    let stats = McpSchemaSelectionStats {
        available: tools.len(),
        selected: selected.len(),
        relevant: relevant_count,
        previously_used: previously_used_count,
        fallback: fallback_count.min(selected.len()),
        omitted: tools.len().saturating_sub(selected.len()),
        selected_names,
    };
    (selected, stats)
}

pub(super) fn select_mcp_tools_for_context_with_sticky(
    tools: &[(String, String, Value)],
    messages: &[Value],
    sticky_names: &[String],
) -> (Vec<usize>, McpSchemaSelectionStats) {
    let (mut selected, mut stats) = select_mcp_tools_for_context(tools, messages);
    if sticky_names.is_empty() {
        return (selected, stats);
    }

    let mut selected_indices = std::collections::HashSet::with_capacity(selected.len());
    selected_indices.extend(selected.iter().copied());
    for name in sticky_names {
        if selected.len() >= MAX_MCP_NATIVE_SCHEMAS {
            break;
        }
        let Some(index) = tools.iter().position(|(tool_name, _, _)| tool_name == name) else {
            continue;
        };
        if selected_indices.insert(index) {
            selected.push(index);
        }
    }
    selected.sort_unstable();
    stats.selected = selected.len();
    stats.omitted = tools.len().saturating_sub(selected.len());
    stats.selected_names = selected
        .iter()
        .map(|index| tools[*index].0.clone())
        .collect();
    (selected, stats)
}

pub(crate) fn native_tools_schema_for_context(
    policy: ToolSchemaPolicy,
    messages: &[Value],
) -> (Vec<Value>, McpSchemaSelectionStats) {
    native_tools_schema_for_context_with_sticky(policy, messages, &[])
}

pub(crate) fn native_tools_schema_for_context_with_sticky(
    policy: ToolSchemaPolicy,
    messages: &[Value],
    sticky_names: &[String],
) -> (Vec<Value>, McpSchemaSelectionStats) {
    let mut tools = builtin_native_tools_schema(policy.include_agent_tools);
    // MCP tools are selected from the current request context. The registry is
    // a HashMap, so collection is sorted before scoring and emission to keep
    // both selection and the provider-facing payload deterministic.
    let stats = if policy.include_mcp_tools {
        let mcp_tools = collect_mcp_tools();
        let (selected, stats) =
            select_mcp_tools_for_context_with_sticky(&mcp_tools, messages, sticky_names);
        for index in selected {
            let (name, desc, schema) = &mcp_tools[index];
            tools.push(mcp_schema_value(name, desc, schema));
        }
        stats
    } else {
        McpSchemaSelectionStats::default()
    };
    tools.extend(agent_native_tools_schema(policy.include_agent_tools));
    (tools, stats)
}

pub fn native_tools_schema(include_agent_tools: bool) -> Vec<Value> {
    let mut tools = builtin_native_tools_schema(include_agent_tools);
    // MCP tools, emitted in a deterministic (name-sorted) order. The registry is
    // a HashMap, so iterating it directly yields a hash-dependent order that can
    // shift after a rehash and silently break the provider's prefix cache. A
    // stable byte-for-byte layout keeps the cached prefix valid across turns.
    for (name, desc, schema) in collect_mcp_tools() {
        tools.push(mcp_schema_value(&name, &desc, &schema));
    }
    tools.extend(agent_native_tools_schema(include_agent_tools));
    tools
}
/// Canonical JSON Schema for a built-in tool, resolved from its `Tool`
/// definition. Unknown names fall back to an empty permissive object schema.
pub(super) fn schema_for_tool(name: &str) -> Value {
    TOOLS
        .iter()
        .find(|t| t.name == name)
        .map(|t| (t.schema)())
        .unwrap_or_else(|| schema_from_arguments("{}"))
}

pub(super) fn schema_for_agent_tool(name: &str) -> Value {
    match name {
        "spawn_agent" => {
            serde_json::json!({"type":"object","properties":{"task":{"type":"string"},"write_access":{"type":"boolean","default":false},"allowed_paths":{"type":"array","items":{"type":"string"}},"verification_command":{"type":"string"}},"required":["task"]})
        }
        "send_agent" => {
            serde_json::json!({"type":"object","properties":{"id":{"type":"string"},"message":{"type":"string"}},"required":["id","message"]})
        }
        "set_goal" => {
            serde_json::json!({"type":"object","properties":{"goal":{"type":"string"}},"required":["goal"]})
        }
        "todo_write" => serde_json::json!({
            "type":"object", "properties": {"todos": {"type":"array", "items": {"type":"object", "properties": {
                "content":{"type":"string"}, "status":{"type":"string","enum":["pending","in_progress","completed"]},
                "priority":{"type":"string","enum":["high","medium","low"]}
            }, "required":["content"]}}}, "required":["todos"]
        }),
        _ => schema_from_arguments("{}"),
    }
}

pub(crate) fn tool_system_prompt_for_policy(
    policy: ToolSchemaPolicy,
    protocol: crate::config::ToolProtocol,
    agent_mode: crate::config::AgentMode,
) -> String {
    let mut p = String::new();

    p.push_str(
        "\n# Skills\n\
Skills are discovered on demand so their catalog and instruction bodies stay out of the base prompt. \
If a priority skill route appears in the request context, follow it; otherwise, at the START of a task that may match a specialized workflow, call `list_skills` first. \
Review its names and descriptions, then call `use_skill` with the exact matching name before taking other task actions. \
`list_skills` returns metadata only; `use_skill` loads the selected SKILL.md and its available files.\n\n",
    );

    if agent_mode == crate::config::AgentMode::Plan {
        p.push_str(
            "CRITICAL: You are operating in PLAN MODE (Read-only / Design mode).\n\
             - File writing, deletion, shell commands, delegation, and unknown tools are disabled; you can read, search, ask questions, and design, but CANNOT modify files or execute commands.\n\
             - Investigate before planning with `grep`, `glob`, and `view_file`: read the manifest, real call sites, crates, and existing patterns.\n\
             - Make the plan specific to THIS repository: name files, functions/structs, and inspected line ranges; resolve unknowns now, never guess dependencies or module layout, and state verified facts and uncertainties.\n\
             - Explain the plan and tell the user to switch to Build Mode (press Tab) to implement it.\n\n"
        );
    }

    p.push_str(
        "You are rustcode, a terminal-based coding assistant.\n\
- Use `sandbox/` for temporary scripts/builds and `artifacts/` for persistent designs/reports. For commands over 2s (build/test/install), set `\"background\": true` in `run_command`.\n\
- `run_command` sends its complete `command` through the platform shell. Use `&&` for dependent commands and `;` for independent observations; keep destructive operations inspectable and never hide a required failure with `;`.\n\
# Rules\n\
- Be concise and direct; execute tools without filler. Include changed files, verification, blockers, and next steps when relevant.\n\
- Do not add code comments unless requested. After edits, inspect the result, run the safest relevant check, and report changes and verification.\n\
- If `git-feature-workflow` is available and files change, load it and follow its branch/status, focused-staging, verification, publish, and return-to-main steps. Preserve unrelated work; never use `git add .`, `git add -A`, or `git add --all`.\n\
- Tool results are authoritative: claim checks only after an observed exit code 0. Fix compiler/tool errors first and rerun fresh checks after stale or failed verification. Subagent reports are advisory; inspect the workspace yourself.\n\
- Locate the nearest project manifest (`Cargo.toml`, `package.json`, `pyproject.toml`, etc.) and check from its root. Never run `cargo check` on a standalone `.rs` file outside a Cargo project.\n\
- Explore exact definitions with `grep`/`glob` before reading; do not page through large files. Before editing existing files, inspect their actual content and use the repository's editing tool with non-empty exact targets; do not guess lines, imports, dependencies, or fields.\n\
- ISSUE INDEPENDENT READS TOGETHER: request `view_file`, `grep`, `glob`, `list_directory`, `find_symbol`, `get_project_map`, `search_web`, and `use_skill` together; independent reads run in parallel. Wait for dependent results.\n\
- ONE CHANGE AT A TIME: each write, command, or delegation must be grounded in known results and execute alone; never speculate dependent calls; at most 4 such calls per response; reads may batch.\n\
- Chained shell commands are fine for small, inspectable observations. Prefer native `view_file`/`grep` tools for targeted inspection.\n\
- Match project style and mirror neighboring code patterns (signatures, state/locks, errors).\n\
- Prefer the smallest focused sequence: locate, inspect, change, verify.\n\
- Run focused tests/checks after changes and cover boundaries for complex logic.\n\
- Ask before expensive or externally visible operations. Read-only tools run immediately; modifying/destructive tools require confirmation.\n\
- Use `ask_question` only for ambiguous requirements/design or explicit validation, never routine confirmation. The UI supplies the `write your own answer` slot; do not include `Other`/`Write your own` options. Finish with a plain-text summary.\n\n\
# Working memory & avoiding loops\n\
- Background completion notifications are automatic; do not poll `manage_task` status/list while waiting.\n\
- If a tool execution or compiler check returns compilation errors or warnings, prioritize fixing them immediately before proceeding to other steps.
- Do not reread unchanged files or repeat identical calls; on errors correct arguments, and on empty results change the query.
- An edit that reports \"already applied\" changed nothing on disk; re-issuing the identical edit will report the same no-op again, not succeed differently. Neither a no-op nor a failed edit counts as progress, and the harness ends the turn after a handful of either in a row — re-read the file or change your approach instead of repeating the call.
- Use `todo_write` ONLY for complex code refactors or multi-stage tasks (3+ steps). For routine tasks, git operations, single-file edits, or simple questions, DO NOT use `todo_write` — execute tools directly. Do not update `todo_write` after every single command; only update it when completing major milestones.\n\n"
    );

    p.push_str(
        "# Delegation Policy\n\\
- Do not spawn subagents unless the user explicitly requests delegation/parallel agent work or applicable project instructions require it.\n\\
- Before delegating, identify the critical path and keep blockers in the main agent. Delegate only bounded, self-contained side tasks with clear outputs and disjoint write scopes.\n\\
- Review every subagent result and inspect its workspace changes before treating the task as complete.\n\\n",
    );

    p.push_str("# Tool Format\n");
    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "Call tools only as fenced `tool` blocks containing one JSON object; emit no prose before/after.\n\n\
                ```tool\n\
                {\"name\": \"tool_name\", \"arguments\": {...}}\n\
                ```\n\n\
                Rules: keys are \"name\" and \"arguments\"; argument values use their proper JSON types. Use only the ```tool fence (never ```tool_code, ```json, or another fence) and never duplicate a call. Several fences are allowed: independent reads run in parallel, while workspace changes/commands run serially; ground each call in results already received.\n\n"
            );
        }
        crate::config::ToolProtocol::Native => {
            p.push_str(
                "Call tools only with native tags; emit no prose before/after.\n\n\
                [TOOL_CALLS]tool_name[ARGS]{\"arg_name\": \"value\"}\n\n\
                Rules: use exactly [TOOL_CALLS]tool_name[ARGS]{...}; arguments must be a valid JSON object matching the tool parameters.\n\n"
            );
        }
        crate::config::ToolProtocol::ApiNative => {
            p.push_str(
                "Tools use the API's native function-calling interface: invoke them directly; do NOT print tool calls as text or JSON. When complete, reply with a plain-text summary and no tool call.\n\n"
            );
        }
    }

    // Text protocols enumerate tools in the prompt. ApiNative carries the full
    // tool schema in the request's `tools` field instead, so listing them here
    // would only duplicate that and waste context.
    if matches!(protocol, crate::config::ToolProtocol::ApiNative) {
        return p;
    }

    p.push_str("Available tools:\n");
    for t in TOOLS {
        if t.capabilities.contains(&ToolCapability::AgentDelegation) && !policy.include_agent_tools
        {
            continue;
        }
        if agent_mode == crate::config::AgentMode::Plan && !allowed_in_plan_mode(t.name) {
            continue;
        }
        p.push_str(&format!(
            "- {} | Args: {} | {}\n",
            t.name, t.arguments, t.description
        ));
    }
    if policy.include_mcp_tools {
        for (name, desc, schema) in collect_mcp_tools() {
            if agent_mode == crate::config::AgentMode::Plan {
                continue;
            }
            p.push_str(&format!(
                "- {} | Args: {} | {}\n",
                name,
                serde_json::to_string(&schema).unwrap_or_default(),
                desc
            ));
        }
    }
    if policy.include_agent_tools && agent_mode != crate::config::AgentMode::Plan {
        p.push_str(
            "- spawn_agent | Args: {\"task\": \"task description\"} | Delegate task to a fresh subagent.\n\
            - send_agent | Args: {\"id\": subagent_id, \"message\": \"message\"} | Start a follow-up for a completed subagent; running subagents reject it.\n\
            - set_goal | Args: {\"goal\": \"goal description\"} | Set a new long-running task and switch the agent to continuous autoloop mode.\n\
            - todo_write | Args: {\"todos\": [{\"content\": \"step\", \"status\": \"pending|in_progress|completed\", \"priority\": \"high|medium|low\"}]} | Replace the persistent task plan. Use this at the start of multi-step work and update it as steps finish.\n",
        );
    }

    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "\nExample (task — needs a tool):\n\
User: Where is the agent loop implemented?\n\
Assistant:\n\
```tool\n\
{\"name\": \"grep\", \"arguments\": {\"pattern\": \"agent loop\", \"include\": \"*.rs\"}}\n\
```\n\n\
Example (conversation — no tool):\n\
User: hello, how are you?\n\
Assistant: Hi! Ready to help with your code. What are you working on?\n",
            );
        }
        crate::config::ToolProtocol::Native => {
            p.push_str(
                "\nExample (task — needs a tool):\n\
User: Where is the agent loop implemented?\n\
Assistant:\n\
[TOOL_CALLS]grep[ARGS]{\"pattern\": \"agent loop\", \"include\": \"*.rs\"}\n\n\
Example (conversation — no tool):\n\
User: hello, how are you?\n\
Assistant: Hi! Ready to help with your code. What are you working on?\n",
            );
        }
        // ApiNative returns early above (tools come from the request schema, not
        // the prompt), so this arm is unreachable but keeps the match exhaustive.
        crate::config::ToolProtocol::ApiNative => {}
    }

    p
}

pub fn tool_system_prompt(
    include_agent_tools: bool,
    protocol: crate::config::ToolProtocol,
    agent_mode: crate::config::AgentMode,
) -> String {
    tool_system_prompt_for_policy(
        ToolSchemaPolicy::root(include_agent_tools),
        protocol,
        agent_mode,
    )
}
