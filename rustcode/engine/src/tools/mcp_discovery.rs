//! MCP discovery shared by `list_mcp_tools` and per-request schema selection.
//!
//! One tokenizer and one scorer decide both what a discovery call lists and
//! which schemas the next request binds, so what the model was shown is what
//! becomes callable. The catalogue of configured servers rendered here is
//! appended to the system prompt so the model never has to guess a server.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Function words say nothing about which tool is wanted, and nearly every
/// tool description contains a few of them.
pub(super) const STOP_WORDS: &[&str] = &[
    "about", "after", "again", "all", "alot", "also", "am", "an", "and", "any", "are", "as", "at",
    "be", "been", "before", "being", "but", "by", "can", "cannot", "could", "did", "do", "does",
    "doing", "done", "for", "from", "had", "has", "have", "he", "her", "here", "him", "his", "how",
    "idk", "if", "in", "into", "is", "it", "its", "just", "let", "lets", "like", "lot", "many",
    "may", "me", "might", "more", "most", "much", "must", "my", "no", "not", "of", "off", "on",
    "only", "or", "our", "out", "over", "please", "she", "should", "so", "some", "than", "thanks",
    "that", "the", "their", "them", "then", "there", "these", "they", "this", "through", "to",
    "too", "under", "up", "us", "using", "very", "want", "was", "we", "were", "what", "when",
    "where", "which", "who", "why", "will", "with", "would", "you", "your",
];

/// Words a discovery query carries about MCP itself rather than about the
/// wanted capability. `find the mcp tool for x` must be scored on `x` alone.
const DISCOVERY_NOISE: &[&str] = &[
    "mcp",
    "tool",
    "server",
    "function",
    "capability",
    "available",
    "use",
    "call",
    "need",
    "able",
    "help",
    "find",
    "show",
    "now",
    "right",
    "quick",
    "quickly",
    "something",
    "thing",
    "way",
];

/// Server values that mean "every server" rather than naming one.
const WILDCARD_SERVERS: &[&str] = &["*", "any", "all", "every", "everything"];

/// Results shown with their full `input_schema`, and bound into the next
/// request, for a ranked search. `select` hits are always shown in full.
pub(super) const FULL_SCHEMA_RESULTS: usize = 5;
pub(super) const DEFAULT_DISCOVERY_RESULTS: usize = 20;
pub(super) const MAX_DISCOVERY_RESULTS: usize = 50;
pub(super) const MAX_SELECT_NAMES: usize = 10;
/// Serialized size a discovery result aims to stay under.
pub(super) const DISCOVERY_OUTPUT_BYTES: usize = 8 * 1024;
/// A single schema larger than this is not echoed in the listing; it is
/// still bound into the next request's tool list.
pub(super) const MAX_LISTED_SCHEMA_BYTES: usize = 3 * 1024;
/// The system-prompt catalogue names every tool only while it stays this
/// small; past it, servers are listed with counts.
pub(super) const CATALOGUE_NAME_BYTES: usize = 4 * 1024;

/// Lowercase plural folding shared by queries and tool metadata.
pub(super) fn stem(token: &str) -> String {
    let token = token.to_ascii_lowercase();
    if token.len() > 4 && token.ends_with("ies") {
        return format!("{}y", &token[..token.len() - 3]);
    }
    if token.len() > 4 && (token.ends_with("sses") || token.ends_with("xes")) {
        return token[..token.len() - 2].to_owned();
    }
    if token.len() > 3 && token.ends_with('s') && !token.ends_with("ss") && !token.ends_with("us") {
        return token[..token.len() - 1].to_owned();
    }
    token
}

/// Split identifiers and prose into stemmed words: `snake_case`, `camelCase`,
/// `kebab-case` and `dotted.names` all become separate terms.
pub(super) fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in text.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        let chars = chunk.chars().collect::<Vec<_>>();
        let mut start = 0;
        for index in 1..=chars.len() {
            let boundary = index == chars.len() || {
                let (prev, cur) = (chars[index - 1], chars[index]);
                let next_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
                (prev.is_ascii_lowercase() && cur.is_ascii_uppercase())
                    || (prev.is_ascii_uppercase() && cur.is_ascii_uppercase() && next_lower)
            };
            if boundary {
                let word = chars[start..index].iter().collect::<String>();
                if word.len() >= 2 {
                    out.push(stem(&word));
                }
                start = index;
            }
        }
    }
    out
}

/// The meaningful terms of a discovery query, deduplicated in query order.
pub(super) fn query_terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    tokens(query)
        .into_iter()
        .filter(|term| {
            !STOP_WORDS.contains(&term.as_str()) && !DISCOVERY_NOISE.contains(&term.as_str())
        })
        .filter(|term| seen.insert(term.clone()))
        .collect()
}

fn compact(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

/// One tool as discovery sees it.
#[derive(Clone, Debug)]
pub(super) struct McpToolDoc<'a> {
    pub server: &'a str,
    pub raw_name: &'a str,
    /// Name the provider request advertises (bare when unique).
    pub advertised: &'a str,
    pub description: &'a str,
}

struct DocTerms {
    name: HashSet<String>,
    name_compact: String,
    server: HashSet<String>,
    description: HashSet<String>,
}

impl DocTerms {
    fn new(doc: &McpToolDoc<'_>) -> Self {
        Self {
            name: tokens(doc.raw_name).into_iter().collect(),
            name_compact: compact(doc.raw_name),
            server: tokens(doc.server).into_iter().collect(),
            description: tokens(doc.description).into_iter().collect(),
        }
    }

    /// Weight of one query term: a name word outranks the server name, which
    /// outranks the description. Zero when the term is absent.
    fn weight(&self, term: &str) -> f64 {
        if self.name.contains(term) {
            3.0
        } else if term.len() >= 4 && self.name_compact.contains(term) {
            2.0
        } else if self.server.contains(term) {
            2.0
        } else if self.description.contains(term) {
            1.0
        } else {
            0.0
        }
    }
}

/// Whether `wanted` names `doc` exactly in any accepted form: the bare tool
/// name, `server.tool`, `server/tool`, `mcp__server__tool` or the advertised
/// provider name.
pub(super) fn names_tool(wanted: &str, doc: &McpToolDoc<'_>) -> bool {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return false;
    }
    let canonical = super::schema::mcp_canonical_name(doc.server, doc.raw_name);
    let forms = [
        doc.raw_name.to_owned(),
        doc.advertised.to_owned(),
        canonical,
        format!("{}.{}", doc.server, doc.raw_name),
        format!("{}/{}", doc.server, doc.raw_name),
        format!("{}:{}", doc.server, doc.raw_name),
    ];
    forms.iter().any(|form| form == wanted)
        || forms.iter().any(|form| form.eq_ignore_ascii_case(wanted))
}

/// Rank `docs` for `query`. A tool qualifies when the query names it exactly
/// or matches at least half of the query's meaningful terms; terms that are
/// rare across the catalogue weigh more than common ones. Results below half
/// the best score are dropped so a broad word cannot drag in a whole server.
pub(super) fn rank(query: &str, docs: &[McpToolDoc<'_>], scope: Option<&str>) -> Vec<usize> {
    let terms = query_terms(query);
    let in_scope = |doc: &McpToolDoc<'_>| scope.is_none_or(|server| doc.server == server);
    let doc_terms = docs.iter().map(DocTerms::new).collect::<Vec<_>>();
    let total = docs.len().max(1) as f64;
    let idf = terms
        .iter()
        .map(|term| {
            let df = doc_terms
                .iter()
                .filter(|doc| doc.weight(term) > 0.0)
                .count() as f64;
            1.0 + ((total + 1.0) / (df + 1.0)).ln()
        })
        .collect::<Vec<_>>();
    let required = terms.len().div_ceil(2).max(1);
    let mut scored = Vec::new();
    for (index, doc) in docs.iter().enumerate() {
        if !in_scope(doc) {
            continue;
        }
        let exact = names_tool(query, doc);
        let mut matched = 0;
        let mut score = 0.0;
        for (term, idf) in terms.iter().zip(&idf) {
            let weight = doc_terms[index].weight(term);
            if weight > 0.0 {
                matched += 1;
                score += weight * idf;
            }
        }
        if exact {
            score += 1000.0;
        } else if terms.is_empty() || matched < required {
            continue;
        }
        scored.push((index, score));
    }
    let best = scored.iter().map(|(_, score)| *score).fold(0.0, f64::max);
    scored.retain(|(_, score)| *score * 2.0 >= best);
    scored.sort_by(|(left, left_score), (right, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| docs[*left].server.cmp(docs[*right].server))
            .then_with(|| docs[*left].raw_name.cmp(docs[*right].raw_name))
    });
    scored.into_iter().map(|(index, _)| index).collect()
}

/// Tools whose names look like `wanted`, for a `select` name that resolved
/// to nothing.
pub(super) fn suggestions(wanted: &str, docs: &[McpToolDoc<'_>]) -> Vec<usize> {
    let wanted_raw = wanted
        .rsplit(['.', '/', ':'])
        .next()
        .unwrap_or(wanted)
        .rsplit("__")
        .next()
        .unwrap_or(wanted);
    let mut ranked = rank(wanted, docs, None);
    if ranked.is_empty() {
        let target = compact(wanted_raw);
        let mut close = docs
            .iter()
            .enumerate()
            .map(|(index, doc)| (index, edit_distance(&target, &compact(doc.raw_name))))
            .filter(|(_, distance)| *distance <= target.len().max(3) / 3 + 1)
            .collect::<Vec<_>>();
        close.sort_by_key(|(index, distance)| (*distance, docs[*index].raw_name));
        ranked = close.into_iter().map(|(index, _)| index).collect();
    }
    ranked.truncate(3);
    ranked
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (i, lc) in left.chars().enumerate() {
        let mut current = vec![i + 1; right.len() + 1];
        for (j, rc) in right.iter().enumerate() {
            current[j + 1] = (previous[j] + usize::from(lc != *rc))
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        previous = current;
    }
    previous[right.len()]
}

/// The configured or running server closest to an unknown name.
pub(super) fn closest_server<'a>(wanted: &str, servers: &[&'a str]) -> Option<&'a str> {
    let target = compact(wanted);
    let target_terms = query_terms(wanted);
    servers
        .iter()
        .map(|server| {
            let candidate = compact(server);
            let shared = tokens(server)
                .iter()
                .filter(|term| target_terms.contains(term))
                .count();
            let distance = edit_distance(&target, &candidate);
            let contains = !target.is_empty()
                && !candidate.is_empty()
                && (candidate.contains(&target) || target.contains(&candidate));
            (server, shared, contains, distance)
        })
        .filter(|(_, shared, contains, distance)| {
            *shared > 0 || *contains || *distance <= target.len().max(3) / 3 + 1
        })
        .min_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| left.0.cmp(right.0))
        })
        .map(|(server, _, _, _)| *server)
}

/// Validated `list_mcp_tools` arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DiscoveryArgs {
    /// Server as written, `None` for no or a wildcard server.
    pub server: Option<String>,
    pub query: Option<String>,
    pub select: Vec<String>,
    pub limit: usize,
}

impl DiscoveryArgs {
    pub(super) fn parse(args: &Value) -> Result<Self, String> {
        let Some(object) = args.as_object() else {
            return Err("arguments must be a JSON object".to_owned());
        };
        for key in object.keys() {
            if !["server", "query", "select", "limit"].contains(&key.as_str()) {
                return Err(format!("invalid argument '{key}'"));
            }
        }
        let server = match object.get("server") {
            None | Some(Value::Null) => None,
            Some(Value::String(server)) => Some(server.trim().to_owned()),
            Some(_) => return Err("'server' must be a string".to_owned()),
        }
        .filter(|server| {
            !server.is_empty() && !WILDCARD_SERVERS.contains(&server.to_ascii_lowercase().as_str())
        });
        let query = match object.get("query") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|query| !query.is_empty())
                    .ok_or("'query' must be a non-empty string")?
                    .to_owned(),
            ),
        };
        let select = match object.get("select") {
            None => Vec::new(),
            Some(Value::String(name)) => vec![name.trim().to_owned()],
            Some(Value::Array(names)) => names
                .iter()
                .map(|name| {
                    name.as_str()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .ok_or("'select' must be an array of tool names")
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("'select' must be an array of tool names".to_owned()),
        };
        if select.len() > MAX_SELECT_NAMES {
            return Err(format!(
                "'select' accepts at most {MAX_SELECT_NAMES} names per call"
            ));
        }
        let mut seen = HashSet::new();
        let select = select
            .into_iter()
            .filter(|name| seen.insert(name.clone()))
            .collect();
        let limit = match object.get("limit") {
            None => DEFAULT_DISCOVERY_RESULTS,
            Some(value) => value
                .as_u64()
                .filter(|value| (1..=MAX_DISCOVERY_RESULTS as u64).contains(value))
                .map(|value| value as usize)
                .ok_or_else(|| {
                    format!("'limit' must be an integer from 1 to {MAX_DISCOVERY_RESULTS}")
                })?,
        };
        Ok(Self {
            server,
            query,
            select,
            limit,
        })
    }
}

/// What one discovery call resolves to, before any formatting.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct DiscoveryOutcome {
    /// Tools named by `select`, in request order.
    pub selected: Vec<usize>,
    /// `select` names that matched no tool, with suggested tool indices.
    pub unresolved: Vec<(String, Vec<usize>)>,
    /// Ranked query/server results, `select` hits excluded, not yet limited.
    pub matches: Vec<usize>,
    /// The running server the results are scoped to.
    pub scope: Option<String>,
    /// A `server` that is not running, so the search covered every server.
    pub unknown_server: Option<String>,
    /// No server, query or select: the caller lists server summaries.
    pub summary: bool,
    /// Whether `matches` came from a ranked query (vs. a server listing).
    pub ranked: bool,
}

impl DiscoveryOutcome {
    /// Indices of the matches listed under `limit`.
    pub(super) fn listed(&self, limit: usize) -> &[usize] {
        &self.matches[..self.matches.len().min(limit)]
    }

    /// Tools this call lists with their full schema, which the next request
    /// binds: every `select` hit, the top ranked results, and a server's whole
    /// toolset when it is small.
    pub(super) fn bound(&self, limit: usize) -> Vec<usize> {
        let listed = self.listed(limit);
        let shown = if self.ranked || listed.len() <= FULL_SCHEMA_RESULTS {
            &listed[..listed.len().min(FULL_SCHEMA_RESULTS)]
        } else {
            &[]
        };
        let mut bound = self.selected.clone();
        for index in shown {
            if !bound.contains(index) {
                bound.push(*index);
            }
        }
        bound
    }
}

/// Resolve a discovery call against the running tools. `running` names every
/// connected server, including one that has no tools.
pub(super) fn discover(
    docs: &[McpToolDoc<'_>],
    running: &[&str],
    args: &DiscoveryArgs,
) -> DiscoveryOutcome {
    let mut outcome = DiscoveryOutcome::default();
    for wanted in &args.select {
        let hits = docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| names_tool(wanted, doc))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if hits.is_empty() {
            outcome
                .unresolved
                .push((wanted.clone(), suggestions(wanted, docs)));
        }
        for index in hits {
            if !outcome.selected.contains(&index) {
                outcome.selected.push(index);
            }
        }
    }

    // A query of only filler ("tools", "mcp server") names no capability;
    // treat it as absent so a server scope still lists that server.
    let mut query = args.query.clone().filter(|query| {
        !query_terms(query).is_empty() || docs.iter().any(|doc| names_tool(query, doc))
    });
    if let Some(server) = args.server.as_deref() {
        let exact = running.iter().find(|name| **name == server);
        let folded = running
            .iter()
            .filter(|name| name.eq_ignore_ascii_case(server))
            .collect::<Vec<_>>();
        match exact.or(if folded.len() == 1 {
            Some(folded[0])
        } else {
            None
        }) {
            Some(name) => outcome.scope = Some((*name).to_owned()),
            None => {
                // Search every server instead of failing: the server value was
                // the model's guess at what it wants.
                outcome.unknown_server = Some(server.to_owned());
                if query.is_none() && args.select.is_empty() {
                    query = Some(server.to_owned());
                }
            }
        }
    }

    if args.select.is_empty() && args.server.is_none() && query.is_none() {
        outcome.summary = true;
        return outcome;
    }
    if let Some(query) = query.as_deref() {
        outcome.ranked = true;
        outcome.matches = rank(query, docs, outcome.scope.as_deref());
    } else if let Some(scope) = outcome.scope.as_deref() {
        outcome.matches = docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.server == scope)
            .map(|(index, _)| index)
            .collect();
        outcome
            .matches
            .sort_by(|left, right| docs[*left].raw_name.cmp(docs[*right].raw_name));
    }
    outcome
        .matches
        .retain(|index| !outcome.selected.contains(index));
    outcome
}

/// Tools the `list_mcp_tools` calls made since the latest user turn bound,
/// newest call first, `select` hits ahead of search hits.
pub(super) fn bound_by_discovery_calls(docs: &[McpToolDoc<'_>], messages: &[Value]) -> Vec<usize> {
    let Some(latest_user) = messages
        .iter()
        .rposition(crate::network::messages::is_user_turn_boundary)
    else {
        return Vec::new();
    };
    let mut running = docs.iter().map(|doc| doc.server).collect::<Vec<_>>();
    running.sort_unstable();
    running.dedup();
    let outcomes = messages[latest_user..]
        .iter()
        .flat_map(|message| {
            message
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|call| call.get("function"))
        .filter(|function| function.get("name").and_then(Value::as_str) == Some("list_mcp_tools"))
        .filter_map(|function| {
            let args = match function.get("arguments")? {
                Value::String(text) => serde_json::from_str::<Value>(text).ok()?,
                other => other.clone(),
            };
            let args = DiscoveryArgs::parse(&args).ok()?;
            Some((discover(docs, &running, &args), args.limit))
        })
        .collect::<Vec<_>>();
    let mut bound = Vec::new();
    for (outcome, _) in outcomes.iter().rev() {
        for index in &outcome.selected {
            if !bound.contains(index) {
                bound.push(*index);
            }
        }
    }
    for (outcome, limit) in outcomes.iter().rev() {
        for index in outcome.bound(*limit) {
            if !bound.contains(&index) {
                bound.push(index);
            }
        }
    }
    bound
}

/// How one configured or connected server stands in this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum McpServerState {
    Running { tools: Vec<String> },
    NotRunning,
    Disabled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpServerStatus {
    pub name: String,
    pub state: McpServerState,
}

impl McpServerStatus {
    pub(super) fn label(&self) -> String {
        match &self.state {
            McpServerState::Running { tools } => format!("running ({} tools)", tools.len()),
            McpServerState::NotRunning => "not running".to_owned(),
            McpServerState::Disabled => "disabled".to_owned(),
            McpServerState::Failed(reason) => format!("failed: {reason}"),
        }
    }
}

/// One line of a startup failure, short enough for a catalogue entry.
pub(super) fn short_reason(reason: &str) -> String {
    let line = reason.lines().next().unwrap_or("").trim();
    let mut chars = line.chars();
    let short = chars.by_ref().take(120).collect::<String>();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// Classify configured and connected servers. `running` maps each connected
/// server to its tool names; `failures` holds startup errors by server.
pub(super) fn server_statuses(
    configured: &[crate::config::McpServerConfig],
    running: &HashMap<String, Vec<String>>,
    failures: &HashMap<String, String>,
) -> Vec<McpServerStatus> {
    let mut statuses = Vec::new();
    let mut seen = HashSet::new();
    for server in configured {
        if !seen.insert(server.name.clone()) {
            continue;
        }
        let state = if let Some(tools) = running.get(&server.name) {
            McpServerState::Running {
                tools: tools.clone(),
            }
        } else if !server.enabled {
            McpServerState::Disabled
        } else if let Some(reason) = failures.get(&server.name) {
            McpServerState::Failed(short_reason(reason))
        } else {
            McpServerState::NotRunning
        };
        statuses.push(McpServerStatus {
            name: server.name.clone(),
            state,
        });
    }
    for (name, tools) in running {
        if seen.insert(name.clone()) {
            statuses.push(McpServerStatus {
                name: name.clone(),
                state: McpServerState::Running {
                    tools: tools.clone(),
                },
            });
        }
    }
    for status in &mut statuses {
        if let McpServerState::Running { tools } = &mut status.state {
            tools.sort();
        }
    }
    statuses.sort_by(|left, right| left.name.cmp(&right.name));
    statuses
}

/// The live server statuses for this session's workspace.
pub(crate) fn current_server_statuses() -> Vec<McpServerStatus> {
    let running = {
        let registry = crate::mcp::get_mcp_registry();
        let Ok(registry) = registry.lock() else {
            return Vec::new();
        };
        registry
            .values()
            .map(|client| {
                let tools = client
                    .get_tools()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                (client.name.clone(), tools)
            })
            .collect::<HashMap<_, _>>()
    };
    let configured = crate::config::get_config_dir()
        .map(|dir| crate::config::workspace_mcp_servers_in(&dir, &super::misc::mcp_workspace(None)))
        .unwrap_or_default();
    server_statuses(&configured, &running, &crate::mcp::mcp_startup_failures())
}

/// Each configured or connected server's name and state label, such as
/// `running (3 tools)`, for command reports.
pub(crate) fn mcp_server_status_labels() -> Vec<(String, String)> {
    current_server_statuses()
        .into_iter()
        .map(|status| {
            let label = status.label();
            (status.name, label)
        })
        .collect()
}

/// System-prompt section naming every configured MCP server and its state.
/// Built only from the registry, config and startup failures, so it is
/// byte-identical across requests until one of those changes.
pub(super) fn render_catalogue(statuses: &[McpServerStatus]) -> Option<String> {
    if statuses.is_empty() {
        return None;
    }
    let header = "# MCP Servers\n\
MCP tools exist only on these servers; never guess a server name. \
Call `list_mcp_tools` only for a listed server matching the task; with no match use built-ins/shell. \
`select` returns schemas, `query` searches; `manage_mcp_servers` starts a stopped server.\n";
    let line = |status: &McpServerStatus, with_names: bool| {
        let mut line = format!("- {}: {}", status.name, status.label());
        if let McpServerState::Running { tools } = &status.state
            && with_names
            && !tools.is_empty()
        {
            line.push_str(": ");
            line.push_str(&tools.join(", "));
        }
        line.push('\n');
        line
    };
    let named = statuses
        .iter()
        .map(|status| line(status, true))
        .collect::<String>();
    let body = if named.len() <= CATALOGUE_NAME_BYTES {
        named
    } else {
        statuses
            .iter()
            .map(|status| line(status, false))
            .collect::<String>()
    };
    Some(format!("{header}{body}"))
}

/// The catalogue for the current session, or `None` without MCP servers.
pub(crate) fn mcp_catalogue() -> Option<String> {
    render_catalogue(&current_server_statuses())
}

/// Append the MCP catalogue to a system prompt when the policy offers MCP.
pub(crate) fn append_mcp_catalogue(prompt: &mut String, policy: super::ToolSchemaPolicy) {
    if !policy.include_mcp_tools {
        return;
    }
    if let Some(catalogue) = mcp_catalogue() {
        prompt.push_str("\n\n");
        prompt.push_str(&catalogue);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(name: &str, enabled: bool) -> crate::config::McpServerConfig {
        crate::config::McpServerConfig {
            name: name.to_owned(),
            command: "npx".to_owned(),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            client_id: None,
            enabled,
            always_include: false,
        }
    }

    #[test]
    fn identifiers_split_into_stemmed_words() {
        assert_eq!(tokens("getFriendsList"), ["get", "friend", "list"]);
        assert_eq!(tokens("list_channels"), ["list", "channel"]);
        assert_eq!(tokens("microsoft-teams-mcp"), ["microsoft", "team", "mcp"]);
        assert_eq!(tokens("HTMLParser"), ["html", "parser"]);
        assert_eq!(stem("entries"), "entry");
        assert_eq!(stem("status"), "status");
        assert_eq!(
            query_terms("find the MCP tools for my snapchat friends"),
            ["snapchat", "friend"]
        );
    }

    #[test]
    fn the_catalogue_names_each_server_with_its_state() {
        let running = HashMap::from([
            (
                "mail".to_owned(),
                vec!["send".to_owned(), "list".to_owned()],
            ),
            ("adhoc".to_owned(), vec!["ping".to_owned()]),
        ]);
        let failures = HashMap::from([(
            "broken".to_owned(),
            "spawn npx ENOENT\nstack trace".to_owned(),
        )]);
        let statuses = server_statuses(
            &[
                configured("mail", true),
                configured("off", false),
                configured("broken", true),
                configured("idle", true),
            ],
            &running,
            &failures,
        );
        let catalogue = render_catalogue(&statuses).unwrap();
        assert!(catalogue.starts_with("# MCP Servers\n"), "{catalogue}");
        assert!(catalogue.contains("never guess a server name"));
        for line in [
            "- adhoc: running (1 tools): ping\n",
            "- broken: failed: spawn npx ENOENT\n",
            "- idle: not running\n",
            "- mail: running (2 tools): list, send\n",
            "- off: disabled\n",
        ] {
            assert!(
                catalogue.contains(line),
                "{line:?} missing from {catalogue}"
            );
        }
        assert!(render_catalogue(&[]).is_none());
    }

    #[test]
    fn the_catalogue_is_byte_identical_while_its_inputs_are_unchanged() {
        let build = |order: &[&str]| {
            let running = order
                .iter()
                .map(|name| {
                    (
                        (*name).to_owned(),
                        vec![format!("{name}_b"), format!("{name}_a")],
                    )
                })
                .collect::<HashMap<_, _>>();
            render_catalogue(&server_statuses(&[], &running, &HashMap::new())).unwrap()
        };
        let first = build(&["zeta", "alpha", "mid", "beta"]);
        for _ in 0..8 {
            assert_eq!(build(&["beta", "mid", "zeta", "alpha"]), first);
        }
    }

    #[test]
    fn a_large_catalogue_lists_counts_instead_of_tool_names() {
        let running = (0..20)
            .map(|server| {
                (
                    format!("server_{server:02}"),
                    (0..40)
                        .map(|tool| format!("a_long_descriptive_tool_name_{tool:02}"))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();
        let catalogue = render_catalogue(&server_statuses(&[], &running, &HashMap::new())).unwrap();
        assert!(
            catalogue.contains("- server_00: running (40 tools)\n"),
            "{catalogue}"
        );
        assert!(!catalogue.contains("a_long_descriptive_tool_name"));
        assert!(catalogue.len() < CATALOGUE_NAME_BYTES);
    }

    #[test]
    fn wildcard_and_empty_servers_mean_every_server() {
        for server in ["*", "any", "ALL", "", "  "] {
            let args = DiscoveryArgs::parse(&serde_json::json!({"server": server})).unwrap();
            assert_eq!(args.server, None, "{server:?}");
        }
        let args = DiscoveryArgs::parse(&serde_json::json!({"select": "mail.send"})).unwrap();
        assert_eq!(args.select, ["mail.send"]);
    }
}
