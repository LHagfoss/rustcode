mod mcp_selection;
pub(super) use mcp_selection::select_mcp_tools_for_context_with_anchors;
#[cfg(test)]
pub(crate) use mcp_selection::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase;
#[cfg(test)]
pub(super) use mcp_selection::{
    select_mcp_tools_for_context, select_mcp_tools_for_context_in_phase,
    select_mcp_tools_for_context_with_raw_names, select_mcp_tools_for_context_with_sticky,
};

mod prompts;
#[cfg(test)]
use prompts::textual_mcp_listing;
pub use prompts::tool_system_prompt;
pub(crate) use prompts::{
    ANTIGRAVITY_OUTPUT_NOTE, append_output_style_hint, append_preview_hint, append_smart_ui_hint,
    append_tool_response_limit, append_tool_response_policy, append_visualization_hint,
    codex_windows_sandbox_note, harness_instructions, output_style_hint, runtime_reminder,
    session_title_turn_hint, skills_reminder, smart_ui_hint, taught_visualization_hint,
    tool_system_prompt_for_policy, visualization_prompt_hint,
};
#[cfg(test)]
pub(crate) use prompts::{BASE_PROMPT_MAX_BYTES, BASE_PROMPT_MAX_TOKENS};
#[cfg(test)]
pub(crate) use prompts::{
    CODEX_WINDOWS_SANDBOX_NOTE, OUTPUT_STYLE_PROMPT_HINT, OUTPUT_STYLE_SMART_UI_HINT,
    OUTPUT_STYLE_VIZ_HINT, SMART_UI_HARNESS_HINT, SMART_UI_OFF_PROMPT_HINT, SMART_UI_PROMPT_HINT,
    VISUALIZATION_PROMPT_HINT, VISUALIZATION_PROMPT_HINT_LEVEL_2, harness_output_style,
};

use super::{TOOLS, ToolCapability, allowed_in_plan_mode};
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, LazyLock};

#[cfg(test)]
use std::sync::{
    Condvar, Mutex as StdMutex, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[cfg(test)]
static NATIVE_SCHEMA_TEST_GATE: OnceLock<StdMutex<Option<Arc<NativeSchemaTestGateState>>>> =
    OnceLock::new();

#[cfg(test)]
struct NativeSchemaTestGateState {
    marker: String,
    pause_on_call: usize,
    matching_calls: AtomicUsize,
    entered: AtomicBool,
    released: StdMutex<bool>,
    release_cv: Condvar,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct NativeSchemaTestGate {
    state: Arc<NativeSchemaTestGateState>,
}

#[cfg(test)]
impl NativeSchemaTestGate {
    pub(crate) fn is_entered(&self) -> bool {
        self.state.entered.load(Ordering::Acquire)
    }

    pub(crate) fn release(&self) {
        let mut released = self.state.released.lock().unwrap();
        *released = true;
        self.state.release_cv.notify_all();
    }
}

#[cfg(test)]
impl Drop for NativeSchemaTestGate {
    fn drop(&mut self) {
        self.release();
        let slot = NATIVE_SCHEMA_TEST_GATE.get_or_init(|| StdMutex::new(None));
        let mut installed = slot.lock().unwrap();
        if installed
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &self.state))
        {
            *installed = None;
        }
    }
}

#[cfg(test)]
pub(crate) fn install_native_schema_test_gate(
    marker: impl Into<String>,
    pause_on_call: usize,
) -> NativeSchemaTestGate {
    let state = Arc::new(NativeSchemaTestGateState {
        marker: marker.into(),
        pause_on_call,
        matching_calls: AtomicUsize::new(0),
        entered: AtomicBool::new(false),
        released: StdMutex::new(false),
        release_cv: Condvar::new(),
    });
    let slot = NATIVE_SCHEMA_TEST_GATE.get_or_init(|| StdMutex::new(None));
    let mut installed = slot.lock().unwrap();
    assert!(
        installed.is_none(),
        "native schema test gate already installed"
    );
    *installed = Some(state.clone());
    NativeSchemaTestGate { state }
}

#[cfg(test)]
fn maybe_pause_native_schema_test_gate(messages: &[Value]) {
    let slot = NATIVE_SCHEMA_TEST_GATE.get_or_init(|| StdMutex::new(None));
    let Some(state) = slot.lock().unwrap().clone() else {
        return;
    };
    if !messages
        .iter()
        .any(|message| message.to_string().contains(&state.marker))
    {
        return;
    }
    let call = state.matching_calls.fetch_add(1, Ordering::AcqRel) + 1;
    if call != state.pause_on_call {
        return;
    }
    state.entered.store(true, Ordering::Release);
    let mut released = state.released.lock().unwrap();
    while !*released {
        released = state.release_cv.wait(released).unwrap();
    }
}

/// Agent tools that live outside the `TOOLS` table. `(name, description, args)`
/// mirrors what `tool_system_prompt` lists for the text protocols, reused here
/// to build the native function schema.
pub(super) const AGENT_TOOL_SPECS: &[(&str, &str, &str)] = &[
    (
        "spawn_agent",
        "Start an asynchronous subagent and return JSON with its agent_id. It runs on your model with your tools (web, MCP, skills), without write access unless you grant it. Delegate a self-contained task whose result you need but whose intermediate steps you do not: a broad search, an independent investigation, or one of several parallel tasks. Do the work yourself when it is a few tool calls, and never delegate the step you are blocked on and then wait for it. Give each child a complete brief: it sees none of this conversation unless context_inheritance says otherwise; write access, allowed_paths and verification must be explicit, and children that write need disjoint paths. Keep working after spawning: a finished child reports back on its own, and wait_agent is for when you have nothing else to do. agent_type: explorer is read-only investigation, worker presets write_access and still needs allowed_paths; the default is read-only. Omit model: the child then uses your model, provider and login; name a model profile only when the user asked for one. Working outside the session workspace? Pass that directory as cwd.",
        r#"{"task": "task description", "agent_type": "explorer", "write_access": false, "allowed_paths": ["src/"], "verification_command": "cargo test", "workspace_mode": "shared", "base_sha": "origin/main", "cwd": "optional working directory"}"#,
    ),
    (
        "send_agent",
        "Follow up an idle subagent or queue a message for an active child at its next safe boundary. With interrupt, stop an active child's turn and redirect it now.",
        r#"{"id": "subagent id", "message": "message text", "interrupt": false}"#,
    ),
    (
        "set_goal",
        "Set a new long-running task and switch the agent to continuous autoloop mode.",
        r#"{"goal": "goal description"}"#,
    ),
    (
        "todo_write",
        "Replace the persistent task plan with a list of steps, or with merge: true update the steps whose id you give (others stay).",
        r#"{"todos": "list of steps: id (to keep or update one), content, status, priority, blocked_reason", "merge": "optional bool"}"#,
    ),
];

/// Selects the tool schemas visible to one provider request.
///
/// This is intentionally request-scoped: a child request must not infer its
/// capabilities from the parent session's mutable delegation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolSchemaPolicy {
    pub(crate) include_agent_tools: bool,
    pub(crate) include_mcp_tools: bool,
    pub(crate) include_session_title_tool: bool,
    /// `localhost_preview`, offered only when the client has a preview (#2138).
    pub(crate) include_preview_tool: bool,
    pub(crate) include_desktop_tool: bool,
    /// `smart_ui_preview`, offered only while Smart UI is on in a client
    /// that renders previews.
    pub(crate) include_smart_ui_preview_tool: bool,
    /// `list_sessions`, `message_session` and `wait_session`, offered only
    /// when the client hosts other threads (#2637).
    pub(crate) include_orchestration_tools: bool,
    /// `read_session`, `start_session` and `fan_out`, for a client that
    /// advertises orchestration version 2.
    pub(crate) include_orchestration_v2_tools: bool,
    pub(crate) include_orchestration_v3_tools: bool,
    pub(crate) include_orchestration_v4_tools: bool,
    /// `read_activity`, for a client that advertises orchestration version 6.
    pub(crate) include_orchestration_v6_tools: bool,
    /// `manage_queue`, for a client that advertises orchestration version 7.
    pub(crate) include_orchestration_v7_tools: bool,
    pub(crate) include_orchestration_v9_tools: bool,
    /// `answer_request` and `organize_threads`, for orchestration version 10.
    pub(crate) include_orchestration_v10_tools: bool,
    /// `read_project` and the other project tools, plus the scheduler's four,
    /// for a client that advertises orchestration version 11.
    pub(crate) include_orchestration_v11_tools: bool,
    /// `list_machines`, for a client that advertises orchestration version 12.
    pub(crate) include_orchestration_v12_tools: bool,
    /// `worktree_status` and `handoff_worktree`, for orchestration version 13.
    pub(crate) include_orchestration_v13_tools: bool,
    pub(crate) include_orchestration_loops: bool,
    /// What the client advertised, capped at [`crate::orchestration::VERSION`]:
    /// the tool set follows the flags above, and the args follow this one.
    pub(crate) orchestration_version: u8,
    pub(crate) profile: ToolSchemaProfile,
    pub(crate) compact_text_prompt: bool,
}

/// Counts the capabilities advertised by a textual tool contract. Native
/// requests derive the equivalent counts from the selected schema result, but
/// textual requests still need an explicit inventory for diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ToolSurface {
    pub(crate) builtin: usize,
    pub(crate) mcp: usize,
    /// Tools with `ToolCapability::AgentDelegation`. Those that live in
    /// `TOOLS` are also part of `builtin`, so this is not a term of `total`.
    pub(crate) agent: usize,
    /// Tools advertised from outside the `TOOLS` table.
    pub(crate) outside_table: usize,
}

impl ToolSurface {
    pub(crate) const fn total(self) -> usize {
        self.builtin + self.mcp + self.outside_table
    }
}

/// Deterministic provider-facing tool menu for the read-only inspection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum ToolSchemaProfile {
    #[default]
    Coding,
    ReadOnlyInspection,
    /// A child agent without `write_access`: the parent's tools, MCP and
    /// skills included, less the ones its read-only gate refuses (file
    /// writes, mutating built-ins, MCP tools not marked read-only). Its
    /// shell runs only commands the policy classifies as inspection.
    ReadOnlySubagent,
    /// A child agent with `write_access`: the parent's tools, less the
    /// root-only ones in [`ROOT_ONLY_TOOLS`].
    Subagent,
}

/// Request phase used to keep the initial tool menu small for new projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ToolSchemaPhase {
    /// The workspace has no (or only a handful of) source files.
    Bootstrap,
    /// Established repositories retain the broad, context-ranked menu.
    #[default]
    Established,
}

impl ToolSchemaPolicy {
    pub(crate) const fn root(include_agent_tools: bool) -> Self {
        Self {
            include_agent_tools,
            include_mcp_tools: true,
            include_session_title_tool: false,
            include_preview_tool: false,
            include_desktop_tool: false,
            include_smart_ui_preview_tool: false,
            include_orchestration_tools: false,
            include_orchestration_v2_tools: false,
            include_orchestration_v3_tools: false,
            include_orchestration_v4_tools: false,
            include_orchestration_v6_tools: false,
            include_orchestration_v7_tools: false,
            include_orchestration_v9_tools: false,
            include_orchestration_v10_tools: false,
            include_orchestration_v11_tools: false,
            include_orchestration_v12_tools: false,
            include_orchestration_v13_tools: false,
            include_orchestration_loops: false,
            orchestration_version: 0,
            profile: ToolSchemaProfile::Coding,
            compact_text_prompt: false,
        }
    }

    pub(crate) fn root_for_mode(include_agent_tools: bool, mode: crate::config::AgentMode) -> Self {
        if mode == crate::config::AgentMode::Plan {
            Self::read_only_inspection()
        } else {
            Self::root(include_agent_tools)
        }
    }

    pub(crate) const fn read_only_inspection() -> Self {
        Self {
            include_agent_tools: false,
            include_mcp_tools: false,
            include_session_title_tool: false,
            include_preview_tool: false,
            include_desktop_tool: false,
            include_smart_ui_preview_tool: false,
            include_orchestration_tools: false,
            include_orchestration_v2_tools: false,
            include_orchestration_v3_tools: false,
            include_orchestration_v4_tools: false,
            include_orchestration_v6_tools: false,
            include_orchestration_v7_tools: false,
            include_orchestration_v9_tools: false,
            include_orchestration_v10_tools: false,
            include_orchestration_v11_tools: false,
            include_orchestration_v12_tools: false,
            include_orchestration_v13_tools: false,
            include_orchestration_loops: false,
            orchestration_version: 0,
            profile: ToolSchemaProfile::ReadOnlyInspection,
            compact_text_prompt: false,
        }
    }

    /// A read-only child agent without delegation tools.
    pub(crate) const fn subagent() -> Self {
        Self {
            include_agent_tools: false,
            include_mcp_tools: true,
            include_session_title_tool: false,
            include_preview_tool: false,
            include_desktop_tool: false,
            include_smart_ui_preview_tool: false,
            include_orchestration_tools: false,
            include_orchestration_v2_tools: false,
            include_orchestration_v3_tools: false,
            include_orchestration_v4_tools: false,
            include_orchestration_v6_tools: false,
            include_orchestration_v7_tools: false,
            include_orchestration_v9_tools: false,
            include_orchestration_v10_tools: false,
            include_orchestration_v11_tools: false,
            include_orchestration_v12_tools: false,
            include_orchestration_v13_tools: false,
            include_orchestration_loops: false,
            orchestration_version: 0,
            profile: ToolSchemaProfile::ReadOnlySubagent,
            compact_text_prompt: false,
        }
    }

    pub(crate) const fn subagent_with_delegation() -> Self {
        Self {
            include_agent_tools: true,
            ..Self::subagent()
        }
    }

    /// The tools a child agent gets: the parent's, cut only by its explicit
    /// `write_access`. A write-enabled child cannot delegate, so it is not
    /// offered the agent tools.
    pub(crate) const fn for_subagent(write_access: bool) -> Self {
        if write_access {
            Self {
                include_agent_tools: false,
                profile: ToolSchemaProfile::Subagent,
                ..Self::subagent()
            }
        } else {
            Self::subagent_with_delegation()
        }
    }

    /// Whether this is a child agent's policy.
    pub(crate) const fn is_subagent(self) -> bool {
        matches!(
            self.profile,
            ToolSchemaProfile::ReadOnlySubagent | ToolSchemaProfile::Subagent
        )
    }

    /// Whether an MCP tool may be offered: a read-only child only gets the
    /// ones its gate would run.
    fn offers_mcp_tool(self, name: &str) -> bool {
        self.profile != ToolSchemaProfile::ReadOnlySubagent || mcp_tool_is_read_only(name)
    }

    pub(crate) const fn with_session_title_tool(mut self) -> Self {
        self.include_session_title_tool = true;
        self
    }

    /// Offer the tools that message other threads. Only the root coding
    /// profile gets them: Plan mode and subagents never drive another thread.
    /// `version` is what the client advertised: 1 keeps the M1 tools only.
    pub(crate) fn with_orchestration_tools(mut self, version: u8) -> Self {
        self.include_orchestration_tools =
            self.profile == ToolSchemaProfile::Coding && version >= 1;
        self.include_orchestration_v2_tools = self.include_orchestration_tools && version >= 2;
        self.include_orchestration_v3_tools = self.include_orchestration_tools && version >= 3;
        self.include_orchestration_v4_tools = self.include_orchestration_tools && version >= 4;
        self.include_orchestration_v6_tools = self.include_orchestration_tools && version >= 6;
        self.include_orchestration_v7_tools = self.include_orchestration_tools && version >= 7;
        self.include_orchestration_v9_tools = self.include_orchestration_tools && version >= 9;
        self.include_orchestration_v10_tools = self.include_orchestration_tools && version >= 10;
        self.include_orchestration_v11_tools = self.include_orchestration_tools && version >= 11;
        self.include_orchestration_v12_tools = self.include_orchestration_tools && version >= 12;
        self.include_orchestration_v13_tools = self.include_orchestration_tools && version >= 13;
        self.include_orchestration_loops =
            self.include_orchestration_tools && version >= crate::orchestration::LOOPS_VERSION;
        self.orchestration_version = version.min(crate::orchestration::VERSION);
        self
    }

    /// Desktop control is offered only to root coding sessions.
    pub(crate) fn with_desktop_tool(mut self) -> Self {
        self.include_desktop_tool = self.profile == ToolSchemaProfile::Coding;
        self
    }

    pub(crate) fn with_preview_tool(mut self) -> Self {
        self.include_preview_tool = self.profile == ToolSchemaProfile::Coding;
        self
    }

    /// Offer `smart_ui_preview`, to the root coding profile only.
    pub(crate) fn with_smart_ui_preview_tool(mut self) -> Self {
        self.include_smart_ui_preview_tool = self.profile == ToolSchemaProfile::Coding;
        self
    }

    pub(crate) fn root_for_mode_with_compact_prompt(
        include_agent_tools: bool,
        mode: crate::config::AgentMode,
        compact_text_prompt: bool,
    ) -> Self {
        let mut policy = Self::root_for_mode(include_agent_tools, mode);
        policy.compact_text_prompt = compact_text_prompt;
        if compact_text_prompt {
            policy.include_mcp_tools = false;
        }
        policy
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
pub(crate) fn mcp_canonical_name(server: &str, tool: &str) -> String {
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

/// The MCP server connection that served a model-visible tool call named
/// `name` (provider-facing canonical name, or a raw name unique among the
/// connected servers), with the tool's own name. Used
/// to reach the exact connection that ran a call, never a server of the same
/// name that was started for something else.
pub(crate) fn resolve_mcp_tool_call(name: &str) -> Option<(Arc<crate::mcp::McpClient>, String)> {
    let mut clients = crate::mcp::get_mcp_registry()
        .lock()
        .ok()?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    clients.sort_by(|a, b| a.name.cmp(&b.name));
    for client in &clients {
        let Ok(tools) = client.get_tools() else {
            continue;
        };
        for tool in tools {
            if !crate::mcp::tool_visible_to(&tool, "model") {
                continue;
            }
            let Some(raw) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            let canonical = mcp_canonical_name_for_clients(&client.name, raw, &clients);
            if name == canonical || (name == raw && mcp_raw_name_is_unique(name, &clients)) {
                return Some((Arc::clone(client), raw.to_owned()));
            }
        }
    }
    None
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

fn mcp_display_name_from_canonical(name: &str) -> Option<String> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some(format!("{server}.{tool}"))
}

/// Resolve an MCP provider-facing name to the server-qualified label shown in
/// the transcript. Unique raw names need the live registry to recover their
/// server; canonical names remain displayable after a server disconnects.
pub fn mcp_tool_display_name(name: &str) -> Option<String> {
    if let Ok(registry) = crate::mcp::get_mcp_registry().lock() {
        let mut clients = registry.values().cloned().collect::<Vec<_>>();
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        for client in &clients {
            let Ok(tools) = client.get_tools() else {
                continue;
            };
            for tool in tools {
                let Some(raw_name) = tool.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let canonical = mcp_canonical_name_for_clients(&client.name, raw_name, &clients);
                if name == canonical
                    || (name == raw_name && mcp_raw_name_is_unique(raw_name, &clients))
                {
                    return Some(format!("{}.{}", client.name, raw_name));
                }
            }
        }
    }
    mcp_display_name_from_canonical(name)
}

pub(super) fn collect_mcp_tools() -> Vec<(String, String, Value)> {
    collect_mcp_tools_with_servers()
        .into_iter()
        .map(|(name, _, _, description, schema)| (name, description, schema))
        .collect()
}

/// Collect MCP schemas with their configured server name alongside the
/// provider-facing tool name. The owner is needed for per-server reservations;
/// the provider-facing name remains unchanged for compatibility.
pub(super) fn collect_mcp_tools_with_servers() -> Vec<(String, String, String, String, Value)> {
    let mut discovered = Vec::new();
    let mut clients_for_names = Vec::new();
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        let mut clients = reg.values().cloned().collect::<Vec<_>>();
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        clients_for_names = clients.clone();
        for client in &clients {
            if let Ok(mcp_tools) = client.get_tools() {
                for tool in mcp_tools {
                    if !crate::mcp::tool_visible_to(&tool, "model") {
                        continue;
                    }
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
            raw_name.clone()
        };
        if emitted.insert(name.clone()) {
            out.push((name, server, raw_name, desc, schema));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Schema for an MCP tool named either the way requests advertise it or by
/// its canonical `mcp__server__tool` form. `list_mcp_tools` hands out the
/// canonical form and dispatch accepts it, including for a tool whose bare
/// name is unique and is therefore advertised without the prefix.
pub(super) fn mcp_tool_schema(name: &str) -> Option<Value> {
    let mut clients = crate::mcp::get_mcp_registry()
        .lock()
        .map(|registry| registry.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    clients.sort_by(|a, b| a.name.cmp(&b.name));
    find_mcp_tool_schema(collect_mcp_tools_with_servers(), name, |server, raw| {
        mcp_canonical_name_for_clients(server, raw, &clients)
    })
}

pub(super) fn find_mcp_tool_schema(
    tools: Vec<(String, String, String, String, Value)>,
    name: &str,
    canonical: impl Fn(&str, &str) -> String,
) -> Option<Value> {
    tools
        .into_iter()
        .find(|(advertised, server, raw, _, _)| {
            advertised == name || canonical(server, raw) == name
        })
        .map(|(_, _, _, _, schema)| schema)
}

/// Whether an MCP tool is a read. The standard `readOnlyHint` annotation
/// decides when the server sets it; a `destructiveHint: true` marks a
/// mutation. Servers that annotate nothing fall back to a conservative name
/// heuristic (#1970), so `list_clients` or `solidtime_get_active_timer` do not
/// count against the per-response side-effect limit.
pub(crate) fn mcp_tool_is_read_only(name: &str) -> bool {
    let registry_handle = crate::mcp::get_mcp_registry();
    let Ok(registry) = registry_handle.lock() else {
        return false;
    };
    let mut clients = registry.values().cloned().collect::<Vec<_>>();
    clients.sort_by(|a, b| a.name.cmp(&b.name));
    clients.iter().any(|client| {
        client.get_tools().is_ok_and(|tools| {
            tools.iter().any(|tool| {
                let Some(raw) = tool.get("name").and_then(Value::as_str) else {
                    return false;
                };
                let canonical = mcp_canonical_name_for_clients(&client.name, raw, &clients);
                let matches_name =
                    name == canonical || (name == raw && mcp_raw_name_is_unique(name, &clients));
                matches_name && mcp_annotations_mark_read_only(raw, tool.get("annotations"))
            })
        })
    })
}

/// Read classification for one MCP tool from its raw name and annotations.
pub(crate) fn mcp_annotations_mark_read_only(raw_name: &str, annotations: Option<&Value>) -> bool {
    let hint = |key: &str| {
        annotations
            .and_then(|annotations| annotations.get(key))
            .and_then(Value::as_bool)
    };
    if let Some(read_only) = hint("readOnlyHint") {
        return read_only;
    }
    if hint("destructiveHint") == Some(true) {
        return false;
    }
    mcp_name_suggests_read_only(raw_name)
}

/// Conservative read heuristic for unannotated MCP tools: the name starts with
/// a lookup verb (`list_`, `get_`, `search_`, `find_`, `read_`, `show_`),
/// directly or after a `<server>_` prefix, and no word in it names a change.
pub(crate) fn mcp_name_suggests_read_only(raw_name: &str) -> bool {
    const READ_VERBS: &[&str] = &["list", "get", "search", "find", "read", "show"];
    const CHANGE_WORDS: &[&str] = &[
        "accept", "add", "apply", "approve", "archive", "assign", "cancel", "clear", "close",
        "commit", "connect", "create", "delete", "deploy", "destroy", "disable", "edit", "enable",
        "exec", "execute", "import", "insert", "invite", "kill", "lock", "login", "logout", "mark",
        "merge", "modify", "move", "patch", "pause", "post", "publish", "push", "put", "register",
        "reject", "remove", "rename", "replace", "reset", "restart", "restore", "resume", "revoke",
        "run", "save", "send", "set", "start", "stop", "submit", "sync", "toggle", "transfer",
        "trigger", "unlock", "update", "upload", "upsert", "write",
    ];
    let lower = raw_name.to_ascii_lowercase();
    let words = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.iter().any(|word| CHANGE_WORDS.contains(word)) {
        return false;
    }
    match words.as_slice() {
        [verb, ..] if READ_VERBS.contains(verb) => true,
        [_server, verb, _, ..] => READ_VERBS.contains(verb),
        _ => false,
    }
}

pub(crate) const MAX_MCP_NATIVE_SCHEMAS: usize = 16;
/// Keep the selected MCP contract bounded by measured serialized bytes as well
/// as count. A single verbose MCP schema must not consume the whole request
/// budget or make the menu unstable across providers.
pub(crate) const MAX_MCP_NATIVE_SCHEMA_BYTES: usize = 24 * 1024;
/// Per-server cap so one heavy server (e.g. 118-tool gitlab at ~64KB) cannot
/// starve the rest of the catalog (#1633). The global budget still applies.
pub(crate) const MAX_MCP_PER_SERVER_SCHEMA_BYTES: usize = 8 * 1024;

/// Effective MCP schema budget, overridable via
/// `RUSTCODE_MCP_SCHEMA_BUDGET_BYTES` for heavy server mixes (#1633).
pub(crate) fn mcp_schema_budget_bytes() -> usize {
    std::env::var("RUSTCODE_MCP_SCHEMA_BUDGET_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v >= 4096)
        .unwrap_or(MAX_MCP_NATIVE_SCHEMA_BYTES)
}
/// The hard cap before the menu is pruned from the specialised end. The
/// orchestration tools are the tail and the biggest, so the newest version
/// decides it: v13 (worktree status and handoff) measures 56,595 bytes, and
/// this is the next KiB above that — at 54 KiB its own two tools, which the cap
/// drops from the tail first, were the ones withheld. A version over it is a test failure, not a quiet prune: the next tool
/// raises the cap rather than dropping one. It moved from 40 KiB when the
/// delegation descriptions gained what a stalled delegate costs, then to 51 KiB
/// when v9's pull-request tools, v10's answer/organize pair and v11's projects
/// and schedules joined the menu, to 54 KiB for v12 (the machines), and to
/// 57 KiB for v13: at 51 KiB the v11 menu had already lost
/// `delete_scheduled_task` and `run_scheduled_task_now`, as at 41 KiB a v9 menu
/// lost its three v9 tools. Raising it keeps every version- and
/// setting-gated tool advertised (`schema_budget_tests` loops every version);
/// the 25 KiB budget below still keeps a plain root prompt lean. It rose to
/// 62 KiB when the workflow tools (`run_workflow`, `workflow_status`,
/// `stop_workflow`) joined every version 2 menu (#3943), and to 64 KiB when
/// `update_loop` joined v21.
pub(crate) const MAX_BUILTIN_NATIVE_SCHEMA_BYTES: usize = 64 * 1024;
pub(super) const MCP_DISCOVERY_FALLBACK_COUNT: usize = 4;
const MCP_RELEVANCE_THRESHOLD: usize = 6;
const MCP_DISCOVERY_CORE: &[&str] = &[
    "codebase_search",
    "codebase_symbols",
    "codebase_impact",
    "codebase_flow",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct McpSchemaSelectionStats {
    pub available: usize,
    pub selected: usize,
    /// Admitted because discovery returned them or the user named them or
    /// their server. Without this a full menu could report zero for every
    /// other reason and look unexplained.
    pub requested: usize,
    pub relevant: usize,
    pub previously_used: usize,
    pub fallback: usize,
    pub omitted: usize,
    /// Names omitted from the provider schema, included in operational events
    /// so a missing callable MCP tool is diagnosable.
    pub omitted_names: Vec<String>,
    /// Configured servers whose complete toolsets were bound by reservation.
    pub reserved_servers: Vec<String>,
    /// Servers whose reservation could not fit the count or byte budget.
    pub rejected_reservations: Vec<String>,
    pub selected_names: Vec<String>,
    pub phase: ToolSchemaPhase,
    pub builtin_available: usize,
    pub builtin_selected: usize,
    /// Built-ins the measured byte budget had to drop from this request. They
    /// remain executable by exact name, so the model must be told. (#1589)
    pub withheld_builtin_names: Vec<String>,
    pub builtin_schema_bytes: usize,
    pub mcp_schema_bytes: usize,
    pub mcp_schema_budget_bytes: usize,
    pub schema_budget_exhausted: bool,
    /// Each selected tool under the bucket that admitted it: `reserved`,
    /// `requested`, `previously_used`, `relevant`, `retained` or `fallback`.
    /// Counting alone left a newly admitted tool unexplained (#2576).
    pub admitted_by: std::collections::BTreeMap<&'static str, Vec<String>>,
}

/// MCP tools this session has already committed to, kept outside the
/// transcript so a compaction that archives the messages naming or calling
/// them does not hand their slots to something else (#2576).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct McpSessionAnchors {
    /// Tools the user named, or whose server the user named, or that a
    /// discovery call bound.
    pub requested: Vec<String>,
    /// Tools the model has called.
    pub used: Vec<String>,
}

impl McpSessionAnchors {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.requested.is_empty() && self.used.is_empty()
    }

    /// Fold one request's admissions into the session's anchors.
    pub(crate) fn absorb(&mut self, stats: &McpSchemaSelectionStats) {
        for (bucket, into) in [
            ("requested", &mut self.requested),
            ("previously_used", &mut self.used),
        ] {
            for name in stats.admitted_by.get(bucket).into_iter().flatten() {
                if !into.contains(name) {
                    into.push(name.clone());
                }
            }
        }
    }
}

const CORE_CODING_TOOLS: &[&str] = &[
    "grep",
    "glob",
    "list_directory",
    "find_symbol",
    "get_project_map",
    "view_file",
    "write_to_file",
    "write_file_chunk",
    "run_command",
    "manage_task",
    "ask_question",
    "complete_task",
    "list_skills",
    "use_skill",
];

/// The compact text protocol keeps the normal coding surface, but leaves
/// specialized memory, web, and lifecycle tools out of the prompt.
/// Native requests apply the finer-grained relevance filter below.
const TEXT_CODING_TOOLS: &[&str] = &[
    "set_session_title",
    "grep",
    "glob",
    "list_directory",
    "find_symbol",
    "get_project_map",
    "view_file",
    "zoom_context",
    "replace_file_content",
    "multi_replace_file_content",
    "write_to_file",
    "write_file_chunk",
    "delete_file",
    "move_file",
    "copy_file",
    "run_command",
    "manage_task",
    "ask_question",
    "complete_task",
    "list_skills",
    "use_skill",
];

const READ_ONLY_INSPECTION_TOOLS: &[&str] = &[
    "grep",
    "glob",
    "list_directory",
    "find_symbol",
    "get_project_map",
    "view_file",
    "zoom_context",
];

const BOOTSTRAP_SOURCE_FILE_LIMIT: usize = 3;

/// Tools only the root agent uses: they act on the session itself (its
/// title, goal and preview) or talk to the user directly. A child agent
/// reports back to its parent instead.
const ROOT_ONLY_TOOLS: &[&str] = &[
    "set_session_title",
    "complete_task",
    "ask_question",
    crate::preview::PREVIEW_TOOL_NAME,
    crate::desktop_control::TOOL_NAME,
    crate::smart_ui::PREVIEW_TOOL_NAME,
    crate::orchestration::LIST_SESSIONS,
    crate::orchestration::MESSAGE_SESSION,
    crate::orchestration::WAIT_SESSION,
    crate::orchestration::READ_SESSION,
    crate::orchestration::START_SESSION,
    crate::orchestration::FAN_OUT,
    crate::orchestration::LIST_HARNESSES,
    crate::orchestration::SEARCH_SESSIONS,
    crate::orchestration::LIST_WORKTREES,
    crate::orchestration::UPDATE_SESSION,
    crate::orchestration::COMPACT_SESSION,
    crate::orchestration::INTERRUPT_SESSION,
    crate::orchestration::CONFIGURE_SESSION,
    crate::orchestration::LIST_PROJECTS,
    crate::orchestration::LIST_PULL_REQUESTS,
    crate::orchestration::TRANSFER_SESSION,
    crate::orchestration::LIST_PENDING_REQUESTS,
    crate::orchestration::ANSWER_REQUEST,
    crate::orchestration::ORGANIZE_THREADS,
    crate::orchestration::THREAD_STATUS,
    crate::orchestration::MERGE_WHEN_GREEN,
    crate::orchestration::CLEANUP_WORKTREE,
    crate::orchestration::READ_PROJECT,
    crate::orchestration::CREATE_PROJECT,
    crate::orchestration::CLONE_PROJECT,
    crate::orchestration::UPDATE_PROJECT,
    crate::orchestration::REMOVE_PROJECT,
    crate::orchestration::SCHEDULE_TASK,
    crate::orchestration::CREATE_LOOP,
    crate::orchestration::UPDATE_LOOP,
    crate::orchestration::RUN_MACHINE_COMMAND,
    crate::orchestration::LIST_SCHEDULED_TASKS,
    crate::orchestration::DELETE_SCHEDULED_TASK,
    crate::orchestration::RUN_SCHEDULED_TASK_NOW,
    crate::orchestration::WORKTREE_STATUS,
    crate::orchestration::HANDOFF_WORKTREE,
];

/// Whether a read-only child agent's gate runs `tool`, so it is worth
/// advertising: reads (search, files, web, memory recall), skills, and the
/// shell, whose calls are checked one by one.
pub(crate) fn read_only_subagent_runs(tool_name: &str) -> bool {
    matches!(super::tool_safety(tool_name), super::ToolSafety::ReadOnly)
        || matches!(tool_name, "run_command" | "use_skill")
}

/// Built-in tools a policy may advertise, before the measured byte budget.
///
/// This is the single membership rule for both protocols. Membership used to be
/// narrowed by transcript keywords and by a Bootstrap/Established phase flip in
/// the native path only, which hid tools the textual contract listed and made
/// the `tools` block drift mid-session as `context_terms` grew. (#1589)
fn builtin_is_advertised(tool: &super::Tool, policy: ToolSchemaPolicy) -> bool {
    let readonly_profile_allows = match policy.profile {
        ToolSchemaProfile::Coding => true,
        ToolSchemaProfile::ReadOnlyInspection => READ_ONLY_INSPECTION_TOOLS.contains(&tool.name),
        ToolSchemaProfile::ReadOnlySubagent => {
            !ROOT_ONLY_TOOLS.contains(&tool.name) && read_only_subagent_runs(tool.name)
        }
        ToolSchemaProfile::Subagent => !ROOT_ONLY_TOOLS.contains(&tool.name),
    };
    !((crate::computer_use::is_tool(tool.name) && !crate::computer_use::Settings::load().enabled)
        || (tool.name == "set_session_title" && !policy.include_session_title_tool)
        || (tool.name == crate::preview::PREVIEW_TOOL_NAME && !policy.include_preview_tool)
        || (tool.name == crate::desktop_control::TOOL_NAME && !policy.include_desktop_tool)
        || (tool.name == crate::smart_ui::PREVIEW_TOOL_NAME
            && !policy.include_smart_ui_preview_tool)
        // Answered by the `rustcode` MCP server an external harness runs
        // (`orchestration/mcp_server.rs`); a RustCode turn has no renderer for
        // it, so it never reaches a built-in schema.
        || tool.name == crate::smart_ui::CHECK_TOOL_NAME
        // The same for the question tool of a harness without one (#3393):
        // a RustCode turn asks with `ask_question`.
        || tool.name == crate::orchestration::ask_user::TOOL_NAME
        // Workflow runs start delegate threads, so they follow the version 2
        // orchestration tools that do (#3943); the engine runs them.
        || (crate::workflow::is_tool(tool.name) && !policy.include_orchestration_v2_tools)
        // Managing RustCode's settings (#3821) is a tool of the `rustcode`
        // MCP server too: the built-in schema has no room under its budget.
        || crate::manage::is_tool(tool.name)
        || (crate::orchestration::is_tool(tool.name) && !policy.include_orchestration_tools)
        || (crate::orchestration::is_v2_tool(tool.name) && !policy.include_orchestration_v2_tools)
        || (crate::orchestration::is_v3_tool(tool.name) && !policy.include_orchestration_v3_tools)
        || (crate::orchestration::is_v4_tool(tool.name) && !policy.include_orchestration_v4_tools)
        || (crate::orchestration::is_v6_tool(tool.name) && !policy.include_orchestration_v6_tools)
        || (crate::orchestration::is_v7_tool(tool.name) && !policy.include_orchestration_v7_tools)
        || (crate::orchestration::is_v9_tool(tool.name) && !policy.include_orchestration_v9_tools)
        || (crate::orchestration::is_v10_tool(tool.name) && !policy.include_orchestration_v10_tools)
        || (crate::orchestration::is_v11_tool(tool.name) && !policy.include_orchestration_v11_tools)
        || (crate::orchestration::is_v12_tool(tool.name) && !policy.include_orchestration_v12_tools)
        || (crate::orchestration::is_v13_tool(tool.name) && !policy.include_orchestration_v13_tools)
        || (tool.name == crate::orchestration::COMPACT_SESSION
            && policy.orchestration_version < crate::orchestration::COMPACT_VERSION)
        || (tool.name == crate::orchestration::UPDATE_LOOP
            && policy.orchestration_version < crate::orchestration::UPDATE_LOOP_VERSION)
        || (tool.name == crate::orchestration::RUN_MACHINE_COMMAND
            && policy.orchestration_version < crate::orchestration::MACHINE_EXEC_VERSION)
        || (tool.name == crate::orchestration::CREATE_LOOP && !policy.include_orchestration_loops)
        || (policy.compact_text_prompt
            && !TEXT_CODING_TOOLS.contains(&tool.name)
            && !(policy.include_agent_tools
                && tool.capabilities.contains(&ToolCapability::AgentDelegation))))
        && !(!readonly_profile_allows
            && !(policy.include_agent_tools
                && tool.capabilities.contains(&ToolCapability::AgentDelegation)))
        && !(tool.capabilities.contains(&ToolCapability::AgentDelegation)
            && !policy.include_agent_tools)
}

fn text_builtin_is_advertised(
    tool: &super::Tool,
    policy: ToolSchemaPolicy,
    agent_mode: crate::config::AgentMode,
) -> bool {
    // Plan mode is already encoded in the policy as the ReadOnlyInspection
    // profile, which is strictly narrower than `allowed_in_plan_mode`, so this
    // adds nothing the native path would have to duplicate.
    builtin_is_advertised(tool, policy)
        && (agent_mode != crate::config::AgentMode::Plan || allowed_in_plan_mode(tool.name))
}

pub(crate) fn textual_tool_surface(
    policy: ToolSchemaPolicy,
    agent_mode: crate::config::AgentMode,
) -> ToolSurface {
    ToolSurface {
        builtin: TOOLS
            .iter()
            .filter(|tool| text_builtin_is_advertised(tool, policy, agent_mode))
            .count(),
        mcp: if policy.include_mcp_tools && agent_mode != crate::config::AgentMode::Plan {
            collect_mcp_tools()
                .iter()
                .filter(|(name, _, _)| policy.offers_mcp_tool(name))
                .count()
        } else {
            0
        },
        agent: agent_tool_count(policy, agent_mode),
        outside_table: outside_table_tool_count(policy, agent_mode),
    }
}

/// Delegation tools a request advertises: every tool carrying
/// `ToolCapability::AgentDelegation`, whether it lives in `TOOLS` or in
/// `AGENT_TOOL_SPECS`. `set_goal` and `todo_write` are not delegation.
pub(crate) fn agent_tool_count(
    policy: ToolSchemaPolicy,
    agent_mode: crate::config::AgentMode,
) -> usize {
    if !policy.include_agent_tools || agent_mode == crate::config::AgentMode::Plan {
        return 0;
    }
    TOOLS
        .iter()
        .filter(|tool| {
            tool.capabilities.contains(&ToolCapability::AgentDelegation)
                && text_builtin_is_advertised(tool, policy, agent_mode)
        })
        .count()
        + AGENT_TOOL_SPECS
            .iter()
            .filter(|(name, _, _)| {
                super::tool_capabilities(name).contains(&ToolCapability::AgentDelegation)
            })
            .count()
}

/// Tools advertised from `AGENT_TOOL_SPECS`, which `builtin` does not count.
pub(crate) fn outside_table_tool_count(
    policy: ToolSchemaPolicy,
    agent_mode: crate::config::AgentMode,
) -> usize {
    usize::from(policy.include_agent_tools && agent_mode != crate::config::AgentMode::Plan)
        * AGENT_TOOL_SPECS.len()
}

fn source_file_count(root: &Path) -> usize {
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0;
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || matches!(name.as_str(), "target" | "node_modules" | "vendor")
            {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension,
                            "rs" | "ts"
                                | "tsx"
                                | "js"
                                | "jsx"
                                | "py"
                                | "go"
                                | "java"
                                | "c"
                                | "h"
                                | "cpp"
                                | "hpp"
                                | "swift"
                                | "kt"
                                | "rb"
                                | "php"
                                | "cs"
                                | "vue"
                                | "svelte"
                        )
                    })
            {
                count += 1;
                if count > BOOTSTRAP_SOURCE_FILE_LIMIT {
                    return count;
                }
            }
        }
    }
    count
}

fn explicitly_requests_codebase_analysis(messages: &[Value]) -> bool {
    messages
        .iter()
        .filter_map(|message| {
            matches!(
                message.get("role").and_then(Value::as_str),
                Some("user" | "assistant")
            )
            .then(|| message.get("content").and_then(Value::as_str).unwrap_or(""))
        })
        .any(|content| {
            let content = content.to_ascii_lowercase();
            [
                "codebase analysis",
                "call graph",
                "dependency graph",
                "find symbol",
                "impact analysis",
                "trace callers",
                "trace callees",
                "refactor the existing",
                "analyze the repository",
                "analyze this repository",
            ]
            .iter()
            .any(|needle| content.contains(needle))
        })
}

pub(crate) fn tool_schema_phase(
    messages: &[Value],
    workspace_root: Option<&Path>,
) -> ToolSchemaPhase {
    if explicitly_requests_codebase_analysis(messages) {
        return ToolSchemaPhase::Established;
    }
    workspace_root
        .filter(|root| root.is_dir())
        .map(|root| source_file_count(root) <= BOOTSTRAP_SOURCE_FILE_LIMIT)
        .map_or(ToolSchemaPhase::Established, |bootstrap| {
            if bootstrap {
                ToolSchemaPhase::Bootstrap
            } else {
                ToolSchemaPhase::Established
            }
        })
}

/// Build the built-in half of the native `tools` block plus the names the
/// measured byte budget had to drop.
///
/// The membership rule is [`builtin_is_advertised`], the same one the textual
/// contract uses, so the two protocols cannot disagree and a fixed policy always
/// yields a byte-identical block. (#1589)
fn build_builtin_native_tools_schema(policy: ToolSchemaPolicy) -> (Vec<Value>, Vec<String>) {
    let mut tools = Vec::new();
    for t in TOOLS {
        if !builtin_is_advertised(t, policy) {
            continue;
        }
        tools.push(serde_json::json!({
            "type": "function",
            "function": {
                "name": t.name,
                "description": t.description,
                "parameters": provider_compatible_schema(schema_for_tool_version(
                    t.name,
                    policy.orchestration_version,
                )),
            }
        }));
    }
    // Preserve the core menu and deterministic TOOLS order, but drop the last
    // specialized entries if the measured provider contract exceeds its
    // budget. This is only schema pruning; execution validation remains based
    // on the complete authoritative registry, so a pruned tool is still
    // callable and the model must be told it exists. (#1589)
    let mut withheld = Vec::new();
    while serialized_schema_bytes(&tools) > MAX_BUILTIN_NATIVE_SCHEMA_BYTES {
        let Some(index) = tools.iter().rposition(|tool| {
            tool["function"]["name"]
                .as_str()
                .is_some_and(|name| !CORE_CODING_TOOLS.contains(&name))
        }) else {
            break;
        };
        if let Some(name) = tools.remove(index)["function"]["name"]
            .as_str()
            .map(str::to_owned)
        {
            withheld.push(name);
        }
    }
    withheld.sort_unstable();
    (tools, withheld)
}

/// The built-in half of the native `tools` block for one policy, with the
/// measurements every request reports about it.
struct BuiltinNativeSchema {
    tools: Vec<Value>,
    withheld: Vec<String>,
    bytes: usize,
}

/// [`build_builtin_native_tools_schema`] memoized per policy.
///
/// The block is a pure function of the policy and the static `TOOLS` table, and
/// request preparation asks for it several times per model round. A policy is
/// four flags and a three-way profile, so the map stays small.
fn cached_builtin_native_tools_schema(policy: ToolSchemaPolicy) -> Arc<BuiltinNativeSchema> {
    static CACHE: LazyLock<std::sync::Mutex<HashMap<ToolSchemaPolicy, Arc<BuiltinNativeSchema>>>> =
        LazyLock::new(Default::default);
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(cache.entry(policy).or_insert_with(|| {
        let (tools, withheld) = build_builtin_native_tools_schema(policy);
        let bytes = serialized_schema_bytes(&tools);
        Arc::new(BuiltinNativeSchema {
            tools,
            withheld,
            bytes,
        })
    }))
}

fn serialized_schema_bytes(schemas: &[Value]) -> usize {
    serde_json::to_vec(schemas).map_or(0, |bytes| bytes.len())
}

fn mcp_schema_bytes(name: &str, description: &str, schema: &Value) -> usize {
    serde_json::to_vec(&mcp_schema_value(name, description, schema)).map_or(0, |bytes| bytes.len())
}

#[cfg(test)]
fn builtin_native_tools_schema(include_agent_tools: bool) -> Vec<Value> {
    cached_builtin_native_tools_schema(ToolSchemaPolicy::root(include_agent_tools))
        .tools
        .clone()
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
            // Runtime-only: it admits an edit tool's undeclared aliases, and
            // several endpoints reject the keyword.
            object.remove("patternProperties");
            if let Some(bound) = object.remove("exclusiveMinimum") {
                object.entry("minimum").or_insert(bound);
            }
            for value in object.values_mut() {
                *value = provider_compatible_schema(value.take());
            }
            if object.get("type").is_some_and(Value::is_array) {
                let Some(Value::Array(types)) = object.remove("type") else {
                    unreachable!("type was checked as an array");
                };
                let branches = types
                    .into_iter()
                    .map(|schema_type| {
                        let mut branch = if schema_type == "null" {
                            serde_json::Map::new()
                        } else {
                            object.clone()
                        };
                        branch.insert("type".into(), schema_type);
                        Value::Object(branch)
                    })
                    .collect();
                object.clear();
                object.insert("anyOf".into(), Value::Array(branches));
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

#[cfg(test)]
pub(crate) fn native_tools_schema_for_context(
    policy: ToolSchemaPolicy,
    messages: &[Value],
) -> (Vec<Value>, McpSchemaSelectionStats) {
    native_tools_schema_for_context_with_sticky(policy, messages, &[])
}

#[cfg(test)]
pub(crate) fn native_tools_schema_for_context_with_sticky(
    policy: ToolSchemaPolicy,
    messages: &[Value],
    sticky_names: &[String],
) -> (Vec<Value>, McpSchemaSelectionStats) {
    native_tools_schema_for_context_with_sticky_at(policy, messages, sticky_names, None)
}

#[cfg(test)]
pub(crate) fn native_tools_schema_for_context_with_sticky_at(
    policy: ToolSchemaPolicy,
    messages: &[Value],
    sticky_names: &[String],
    workspace_root: Option<&Path>,
) -> (Vec<Value>, McpSchemaSelectionStats) {
    native_tools_schema_for_context_with_sticky_at_and_reserved_servers(
        policy,
        messages,
        sticky_names,
        workspace_root,
        &[],
        &McpSessionAnchors::default(),
    )
}

pub(crate) fn native_tools_schema_for_context_with_sticky_at_and_reserved_servers(
    policy: ToolSchemaPolicy,
    messages: &[Value],
    sticky_names: &[String],
    workspace_root: Option<&Path>,
    always_include_servers: &[String],
    anchors: &McpSessionAnchors,
) -> (Vec<Value>, McpSchemaSelectionStats) {
    #[cfg(test)]
    maybe_pause_native_schema_test_gate(messages);
    let phase = tool_schema_phase(messages, workspace_root);
    // The built-in half depends only on the policy, so a fixed policy produces a
    // byte-identical block for the whole session regardless of transcript
    // growth or a Bootstrap/Established flip. (#1589)
    let builtin = cached_builtin_native_tools_schema(policy);
    let mut tools = builtin.tools.clone();
    let withheld_builtin_names = builtin.withheld.clone();
    let builtin_schema_bytes = builtin.bytes;
    let builtin_available = TOOLS
        .iter()
        .filter(|tool| builtin_is_advertised(tool, policy))
        .count();
    let builtin_selected = tools.len();
    let mut agent_tools = agent_native_tools_schema(policy.include_agent_tools);
    if policy.profile != ToolSchemaProfile::Coding {
        agent_tools.retain(|tool| {
            !matches!(
                tool["function"]["name"].as_str(),
                Some("set_goal" | "todo_write")
            )
        });
    }
    let agent_selected = agent_tools.len();
    // The fixed tools go first and the MCP menu last, so a tool the menu
    // admits mid-session changes only the end of the `tools` block.
    tools.extend(agent_tools);
    // MCP tools are selected from the current request context. The registry is
    // a HashMap, so collection is sorted before scoring and emission to keep
    // both selection and the provider-facing payload deterministic.
    let stats = if policy.include_mcp_tools {
        let mut collected = collect_mcp_tools_with_servers();
        collected.retain(|(name, _, _, _, _)| policy.offers_mcp_tool(name));
        let mcp_tools = collected
            .iter()
            .map(|(name, _, _, description, schema)| {
                (name.clone(), description.clone(), schema.clone())
            })
            .collect::<Vec<_>>();
        let owners = collected
            .iter()
            .map(|(_, server, _, _, _)| server.clone())
            .collect::<Vec<_>>();
        let raw_names = collected
            .iter()
            .map(|(_, _, raw_name, _, _)| raw_name.clone())
            .collect::<Vec<_>>();
        let (selected, stats) = select_mcp_tools_for_context_with_anchors(
            &mcp_tools,
            &owners,
            &raw_names,
            always_include_servers,
            messages,
            sticky_names,
            anchors,
            phase,
        );
        for index in selected {
            let (name, desc, schema) = &mcp_tools[index];
            tools.push(mcp_schema_value(name, desc, schema));
        }
        stats
    } else {
        McpSchemaSelectionStats::default()
    };
    let mut stats = stats;
    stats.phase = phase;
    stats.builtin_available = builtin_available;
    stats.builtin_selected = builtin_selected + agent_selected;
    stats.builtin_schema_bytes = builtin_schema_bytes;
    stats.withheld_builtin_names = withheld_builtin_names;
    (tools, stats)
}

#[cfg(test)]
pub fn native_tools_schema(include_agent_tools: bool) -> Vec<Value> {
    let mut tools = builtin_native_tools_schema(include_agent_tools);
    tools.extend(agent_native_tools_schema(include_agent_tools));
    // MCP tools, emitted in a deterministic (name-sorted) order. The registry is
    // a HashMap, so iterating it directly yields a hash-dependent order that can
    // shift after a rehash and silently break the provider's prefix cache. A
    // stable byte-for-byte layout keeps the cached prefix valid across turns.
    for (name, desc, schema) in collect_mcp_tools() {
        tools.push(mcp_schema_value(&name, &desc, &schema));
    }
    tools
}

/// Request-local notice naming what this request's `tools` block withheld.
///
/// Selection is bounded by a count and a byte budget, so some registered tools
/// never reach the payload. Execution validates against the complete registry,
/// so an omitted tool still executes by exact name and the model must not
/// conclude from its absence that the capability is missing — the previous
/// behaviour surfaced nothing and left a post-hoc "unknown or unavailable tool"
/// rejection as the only feedback. (#1589)
pub(crate) fn withheld_tools_notice(stats: &McpSchemaSelectionStats) -> Option<String> {
    let mut lines = Vec::new();
    if !stats.withheld_builtin_names.is_empty() {
        lines.push(format!(
            "Withheld built-ins, callable by exact name: {}.",
            stats.withheld_builtin_names.join(", ")
        ));
    }
    if stats.available > stats.selected {
        lines.push(format!(
            "{} registered MCP tools are not in this request's schema block; call `list_mcp_tools` for their live names instead of guessing.",
            stats.available - stats.selected
        ));
    }
    if !stats.rejected_reservations.is_empty() {
        lines.push(format!(
            "Per-server schema cap ({} bytes) rejected reservations from: {}. Prefer recently used tools; call `list_mcp_tools` per server instead of assuming availability.",
            MAX_MCP_PER_SERVER_SCHEMA_BYTES,
            stats.rejected_reservations.join(", ")
        ));
    }
    (!lines.is_empty()).then(|| format!("# Tools Not Listed\n{}\n", lines.join("\n")))
}

/// Canonical JSON Schema for a built-in tool, resolved from its `Tool`
/// definition. Unknown names fall back to an empty permissive object schema.
pub(super) fn schema_for_tool(name: &str) -> Value {
    schema_for_tool_version(name, crate::orchestration::VERSION)
}

/// [`schema_for_tool`] for a client of `version`: an orchestration arg added in
/// a later version is not advertised to an older client.
fn schema_for_tool_version(name: &str, version: u8) -> Value {
    with_call_summary(crate::orchestration::schema_for(
        name,
        version,
        TOOLS
            .iter()
            .find(|t| t.name == name)
            .map(|t| (t.schema)())
            .unwrap_or_else(|| schema_from_arguments("{}")),
    ))
}

/// The optional argument every built-in and agent tool takes: a few words on
/// what the call does, which the app and the terminal show on its row. It is
/// display-only; no handler reads it.
pub(crate) const CALL_SUMMARY_KEY: &str = "summary";

fn with_call_summary(mut schema: Value) -> Value {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties
            .entry(CALL_SUMMARY_KEY)
            .or_insert_with(|| serde_json::json!({"type": "string"}));
    }
    schema
}

/// The summary a call carries, trimmed to one line, when it has one.
pub(crate) fn call_summary(arguments: &Value) -> Option<&str> {
    arguments
        .get(CALL_SUMMARY_KEY)?
        .as_str()?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

pub(super) fn schema_for_agent_tool(name: &str) -> Value {
    with_call_summary(agent_tool_schema(name))
}

fn agent_tool_schema(name: &str) -> Value {
    match name {
        "spawn_agent" => {
            serde_json::json!({"type":"object","properties":{"task":{"type":"string"},"model":{"type":"string"},"agent_type":{"type":"string","enum":crate::app::subagent_context::AgentRole::NAMES,"default":"default"},"write_access":{"type":"boolean","default":false},"allowed_paths":{"type":"array","items":{"type":"string"}},"verification_command":{"type":"string"},"workspace_mode":{"type":"string","enum":["shared","isolated"],"default":"shared"},"cwd":{"type":"string","description":"Child working directory; default the session workspace."},"workspace_name":{"type":"string"},"task_id":{"type":"string"},"branch":{"type":"string"},"base_sha":{"type":"string"},"context_inheritance":{"type":"string","enum":["minimal","evidence","recent","fork"],"default":"minimal"},"evidence":{"type":"string","maxLength":8192}},"required":["task"]})
        }
        "send_agent" => {
            serde_json::json!({"type":"object","properties":{"id":{"type":"string"},"message":{"type":"string"},"interrupt":super::misc::interrupt_schema()},"required":["id","message"]})
        }
        "set_goal" => {
            serde_json::json!({"type":"object","properties":{"goal":{"type":"string"}},"required":["goal"]})
        }
        "todo_write" => serde_json::json!({
            "type":"object", "properties": {"todos": {"type":"array", "items": {"type":"object", "properties": {
                "id":{"type":"string"}, "content":{"type":"string"},
                "status":{"type":"string","enum":["pending","in_progress","completed","blocked"]},
                "priority":{"type":"string","enum":["high","medium","low"]},
                "blocked_reason":{"type":"string"}
            }}}, "merge":{"type":"boolean"}}, "required":["todos"]
        }),
        _ => schema_from_arguments("{}"),
    }
}

#[cfg(test)]
mod response_limit_tests;

#[cfg(test)]
mod schema_budget_tests;

#[cfg(test)]
mod mcp_read_classification_tests;
