use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, PartialEq, Clone)]
pub enum AppStatus {
    Idle,
    Streaming,
    Queued,

    AwaitingToolConfirmation,
    AwaitingQuestion,
    VerbosityPicker,
    ThinkingPicker,
    EffortPicker,
    ProtocolPicker,
    YoloPicker,
}

#[derive(Debug, Clone)]
pub struct ToolConfirmation {
    pub tool_name: String,
    pub path: String,
    pub content_preview: String,
    pub content_bytes: usize,
}

/// An interactive `ask_question` prompt awaiting the user's choice. Rendered as
/// a modal; the selected option(s) are sent back to the agent as the tool result.
#[derive(Debug, Clone)]
pub struct PendingQuestion {
    pub question: String,
    pub options: Vec<String>,
    pub is_multi_select: bool,
    /// Currently highlighted index. Valid range is `0..=options.len()`, where the
    /// final index (`options.len()`) is the always-present "write your own answer" slot.
    pub selected: usize,
    /// For multi-select: which options are ticked (parallel to `options`).
    pub chosen: Vec<bool>,
    /// When `Some`, the user is typing a freeform answer (the "write your own"
    /// slot is active); the string is the in-progress text.
    pub custom_input: Option<String>,
    pub custom_cursor: usize,
}

impl PendingQuestion {
    pub fn new(question: String, options: Vec<String>, is_multi_select: bool) -> Self {
        let chosen = vec![false; options.len()];
        Self {
            question,
            options,
            is_multi_select,
            selected: 0,
            chosen,
            custom_input: None,
            custom_cursor: 0,
        }
    }

    pub fn activate_custom_input(&mut self) {
        if self.custom_input.is_none() {
            self.custom_input = Some(String::new());
            self.custom_cursor = 0;
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.clamp_cursor();
        let cursor = self.custom_cursor;
        let buf = self.custom_input.get_or_insert_with(String::new);
        buf.insert(cursor, c);
        self.custom_cursor = cursor + c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if c != '\n' && c != '\r' {
                self.insert_char(c);
            }
        }
    }

    pub fn delete_char_before(&mut self) {
        self.clamp_cursor();
        let cursor = self.custom_cursor;
        if cursor > 0 {
            let prev = self
                .custom_input
                .as_ref()
                .and_then(|buf| buf[..cursor].chars().next_back())
                .map_or(1, |c| c.len_utf8());
            self.custom_cursor = cursor - prev;
            if let Some(buf) = self.custom_input.as_mut() {
                buf.remove(self.custom_cursor);
            }
        }
    }

    pub fn delete_char_after(&mut self) {
        self.clamp_cursor();
        let cursor = self.custom_cursor;
        let has_char_after = self
            .custom_input
            .as_ref()
            .is_some_and(|buf| cursor < buf.len());
        if has_char_after {
            if let Some(buf) = self.custom_input.as_mut() {
                buf.remove(cursor);
            }
        }
    }

    pub fn move_cursor_left(&mut self) {
        self.clamp_cursor();
        if let Some(buf) = self.custom_input.as_ref() {
            if self.custom_cursor > 0 {
                let prev = buf[..self.custom_cursor]
                    .chars()
                    .next_back()
                    .map_or(1, |c| c.len_utf8());
                self.custom_cursor -= prev;
            }
        }
    }

    pub fn move_cursor_right(&mut self) {
        self.clamp_cursor();
        if let Some(buf) = self.custom_input.as_ref() {
            if self.custom_cursor < buf.len() {
                let next = buf[self.custom_cursor..]
                    .chars()
                    .next()
                    .map_or(1, |c| c.len_utf8());
                self.custom_cursor += next;
            }
        }
    }

    pub fn move_cursor_home(&mut self) {
        self.custom_cursor = 0;
    }

    pub fn move_cursor_end(&mut self) {
        if let Some(buf) = self.custom_input.as_ref() {
            self.custom_cursor = buf.len();
        }
    }

    pub fn delete_word_before(&mut self) {
        self.clamp_cursor();
        let start = self.custom_cursor;
        self.move_cursor_word_left();
        let end = self.custom_cursor;
        if start > end {
            if let Some(buf) = self.custom_input.as_mut() {
                buf.drain(end..start);
            }
        }
    }

    pub fn move_cursor_word_left(&mut self) {
        self.clamp_cursor();
        if let Some(buf) = self.custom_input.as_ref() {
            let mut char_indices: Vec<(usize, char)> =
                buf[..self.custom_cursor].char_indices().collect();
            while let Some((_, c)) = char_indices.last() {
                if c.is_whitespace() {
                    char_indices.pop();
                } else {
                    break;
                }
            }
            while let Some((idx, c)) = char_indices.last() {
                if !c.is_whitespace() {
                    self.custom_cursor = *idx;
                    char_indices.pop();
                } else {
                    break;
                }
            }
            if char_indices.is_empty() {
                self.custom_cursor = 0;
            }
        }
    }

    pub fn move_cursor_word_right(&mut self) {
        self.clamp_cursor();
        if let Some(buf) = self.custom_input.as_ref() {
            let mut cursor = self.custom_cursor;
            while cursor < buf.len() {
                let Some(c) = buf[cursor..].chars().next() else {
                    break;
                };
                if c.is_whitespace() {
                    break;
                }
                cursor += c.len_utf8();
            }
            while cursor < buf.len() {
                let Some(c) = buf[cursor..].chars().next() else {
                    break;
                };
                if !c.is_whitespace() {
                    break;
                }
                cursor += c.len_utf8();
            }
            self.custom_cursor = cursor;
        }
    }

    fn clamp_cursor(&mut self) {
        if let Some(buf) = self.custom_input.as_ref() {
            self.custom_cursor = self.custom_cursor.min(buf.len());
            while !buf.is_char_boundary(self.custom_cursor) {
                if self.custom_cursor == 0 {
                    break;
                }
                self.custom_cursor -= 1;
            }
        } else {
            self.custom_cursor = 0;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
}

/// Approximate token count accumulated during the current streaming reply.
/// Updated incrementally as SSE chunks arrive; used to compute Tokens/s in the footer.
pub const TOKENS_PER_CHAR_APPROX: f64 = 0.25;

#[derive(Debug, Clone)]
pub struct StreamTracker {
    pub tokens_so_far: u32,
    /// Updated each time a chunk is received; used for per-second rate.
    pub last_update: std::time::Instant,
    prev_tokens: u32,
    history: std::collections::VecDeque<(std::time::Instant, u32)>,
}

impl StreamTracker {
    pub fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            tokens_so_far: 0,
            last_update: now,
            prev_tokens: 0,
            history: std::collections::VecDeque::new(),
        }
    }

    /// Called each time a new chunk arrives during streaming. Updates the history.
    pub fn record_chunk(&mut self) {
        let now = std::time::Instant::now();
        let delta = self.tokens_so_far.saturating_sub(self.prev_tokens);
        if delta > 0 {
            self.history.push_back((now, delta));
        }
        self.prev_tokens = self.tokens_so_far;
        self.last_update = now;

        let cutoff = now
            .checked_sub(std::time::Duration::from_millis(1500))
            .unwrap_or(now);
        while let Some(&(time, _)) = self.history.front() {
            if time < cutoff {
                self.history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Returns the current sliding window tokens/sec and total approximated tokens.
    pub fn snapshot(&self) -> (f64, u32) {
        let now = std::time::Instant::now();

        let window_duration = std::time::Duration::from_secs(1);
        let cutoff = now.checked_sub(window_duration).unwrap_or(now);

        let mut total_tokens_in_window = 0;
        let mut first_time_in_window = None;
        let mut last_time_in_window = None;

        for &(time, tokens) in &self.history {
            if time >= cutoff {
                total_tokens_in_window += tokens;
                if first_time_in_window.is_none() {
                    first_time_in_window = Some(time);
                }
                last_time_in_window = Some(time);
            }
        }

        if total_tokens_in_window == 0 {
            return (0.0, self.tokens_so_far);
        }

        // To calculate rate, divide by the actual elapsed time between first and last chunks in the window.
        // If there's only one chunk, default to a minimum time of 0.1s to avoid extreme spikes.
        let elapsed = if let (Some(first), Some(last)) = (first_time_in_window, last_time_in_window)
        {
            (last - first).as_secs_f64().max(0.1)
        } else {
            1.0
        };

        let raw_tps = total_tokens_in_window as f64 / elapsed;

        let silence = (now - self.last_update).as_secs_f64();
        let tps = if silence > 0.5 {
            let decay = (-silence / 0.5).exp();
            raw_tps * decay
        } else {
            raw_tps
        };

        (tps.max(0.0), self.tokens_so_far)
    }
}

fn current_timestamp() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

pub fn random_tip_index() -> usize {
    use rand::RngExt;
    let mut rng = rand::rng();
    rng.random_range(0..TIPS.len())
}

pub const TIPS: &[&str] = &[
    "Use /tools to see what the agent can do",
    "Ask the agent to fix a TODO or explain a file",
    "Press Ctrl+P to open the command palette",
    "Tab auto-completes slash commands",
    "Switch models anytime with /model <name>",
    "Use /usage to see token and response stats",
    "Esc interrupts a running generation",
    "The agent can grep, glob, read, edit, and run commands",
    "Hold Shift+Enter for multi-line input",
    "Type /info for basic info, or /help for all commands and keybindings",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
    #[serde(default = "current_timestamp")]
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_tokens: Option<u32>,
    #[serde(skip)]
    pub diff: Option<String>,
    /// Ephemeral normal code preview for newly written files. It is derived
    /// from the tool arguments and intentionally not persisted in history.
    #[serde(skip)]
    pub file_preview: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultRecord>,
    /// Structured tool calls this assistant message made, in order. Present only
    /// when the provider returned real function calls; the text protocols write
    /// calls as prose, which has no identity to record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRef>,
    /// For a `tool` message: the id of the call this result answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// The durable conversation history with a cheap mutation marker. The marker
/// lets request preparation and the UI distinguish an unchanged snapshot
/// without comparing every message.
#[derive(Clone, Debug, Default)]
pub struct History {
    messages: Arc<Vec<ChatMessage>>,
    revision: u64,
    last_rewrite_revision: u64,
}

impl History {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_append_only_since(&self, revision: u64) -> bool {
        self.last_rewrite_revision <= revision
    }

    pub fn snapshot(&self) -> History {
        self.clone()
    }

    pub fn replace(&mut self, messages: Vec<ChatMessage>) {
        self.messages = Arc::new(messages);
        self.bump_rewrite();
    }

    pub fn as_mut_vec(&mut self) -> &mut Vec<ChatMessage> {
        self.bump_rewrite();
        Arc::make_mut(&mut self.messages)
    }

    pub fn into_vec(self) -> Vec<ChatMessage> {
        Arc::try_unwrap(self.messages).unwrap_or_else(|messages| messages.as_ref().clone())
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn bump_rewrite(&mut self) {
        self.bump();
        self.last_rewrite_revision = self.revision;
    }

    pub fn push(&mut self, message: ChatMessage) {
        Arc::make_mut(&mut self.messages).push(message);
        self.bump();
    }

    pub fn clear(&mut self) {
        if !self.messages.is_empty() {
            Arc::make_mut(&mut self.messages).clear();
            self.bump_rewrite();
        }
    }

    pub fn drain<R>(&mut self, range: R) -> std::vec::Drain<'_, ChatMessage>
    where
        R: std::ops::RangeBounds<usize>,
    {
        self.bump_rewrite();
        Arc::make_mut(&mut self.messages).drain(range)
    }

    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&ChatMessage) -> bool,
    {
        Arc::make_mut(&mut self.messages).retain(f);
        self.bump_rewrite();
    }

    pub fn as_slice(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn as_mut_slice(&mut self) -> &mut [ChatMessage] {
        self.bump_rewrite();
        Arc::make_mut(&mut self.messages).as_mut_slice()
    }
}

impl From<Vec<ChatMessage>> for History {
    fn from(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages: Arc::new(messages),
            revision: 0,
            last_rewrite_revision: 0,
        }
    }
}

impl PartialEq for History {
    fn eq(&self, other: &Self) -> bool {
        self.messages == other.messages
    }
}

impl Eq for History {}

impl std::ops::Deref for History {
    type Target = [ChatMessage];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl std::ops::DerefMut for History {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl Extend<ChatMessage> for History {
    fn extend<T: IntoIterator<Item = ChatMessage>>(&mut self, iter: T) {
        let before = self.messages.len();
        Arc::make_mut(&mut self.messages).extend(iter);
        if self.messages.len() != before {
            self.bump();
        }
    }
}

impl<'a> IntoIterator for &'a History {
    type Item = &'a ChatMessage;
    type IntoIter = std::slice::Iter<'a, ChatMessage>;

    fn into_iter(self) -> Self::IntoIter {
        self.messages.iter()
    }
}

impl<'a> IntoIterator for &'a mut History {
    type Item = &'a mut ChatMessage;
    type IntoIter = std::slice::IterMut<'a, ChatMessage>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

impl std::iter::FromIterator<ChatMessage> for History {
    fn from_iter<T: IntoIterator<Item = ChatMessage>>(iter: T) -> Self {
        Self::from(iter.into_iter().collect::<Vec<_>>())
    }
}

/// Identity of one structured tool call, kept so a result can name the call it
/// answers when the transcript is replayed to the provider.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallRef {
    pub id: String,
    pub name: String,
    /// Arguments exactly as the provider sent them, so the replayed call is
    /// byte-identical to the one the model made.
    pub arguments: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResultRecord {
    pub tool_name: String,
    pub arguments_hash: String,
    pub success: bool,
    #[serde(default)]
    pub pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub changed_paths: Vec<String>,
    pub truncated: bool,
    pub full_output_artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub replayed: bool,
}

impl ToolResultRecord {
    /// Convert the backwards-compatible persisted spelling back to the typed
    /// error contract when a reloaded session needs machine-readable state.
    pub fn parsed_error_kind(&self) -> Option<crate::tools::ToolErrorKind> {
        self.error_kind
            .as_deref()
            .and_then(crate::tools::ToolErrorKind::from_persisted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedReadOutput {
    pub(crate) replayable_content: Option<String>,
    pub(crate) success: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) truncated: bool,
    pub(crate) full_output_artifact: Option<String>,
    pub(crate) error_kind: Option<crate::tools::ToolErrorKind>,
    pub(crate) retryable: bool,
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            token_usage: None,
            timestamp: current_timestamp(),
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            diff: None,
            file_preview: None,
            tool_result: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Attach the structured calls this assistant message made.
    pub fn with_tool_calls(mut self, calls: Vec<ToolCallRef>) -> Self {
        self.tool_calls = calls;
        self
    }

    /// Mark this tool result as the answer to `call_id`.
    pub fn answering(mut self, call_id: Option<String>) -> Self {
        self.tool_call_id = call_id;
        self
    }

    pub fn with_diff(mut self, diff: Option<String>) -> Self {
        self.diff = diff;
        self
    }

    pub fn with_file_preview(mut self, preview: Option<(String, String)>) -> Self {
        self.file_preview = preview;
        self
    }

    pub fn with_tool_result(mut self, record: ToolResultRecord) -> Self {
        self.tool_result = Some(record);
        self
    }

    /// Resolves tool calls from structured `tool_calls` fields (ApiNative) if present,
    /// or parses tool calls from text content for text protocols.
    pub fn resolved_tool_calls(
        &self,
        protocol: crate::config::ToolProtocol,
    ) -> Vec<crate::tools::ToolCall> {
        if !self.tool_calls.is_empty() {
            self.tool_calls
                .iter()
                .filter_map(|tc| {
                    let args =
                        serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                    Some(crate::tools::ToolCall {
                        name: tc.name.clone(),
                        arguments: args,
                        call_id: Some(tc.id.clone()),
                    })
                })
                .collect()
        } else {
            crate::tools::parse_tool_calls(&self.content, protocol)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// A subagent spawned by the main agent via the spawn_agent tool. Keeps its
/// own conversation history and explicit lifecycle state.
pub struct SubAgent {
    pub id: u32,
    pub name: String,
    pub task: String,
    pub model: Option<String>,
    pub history: Arc<Vec<ChatMessage>>,
    pub status: SubAgentStatus,
    pub active_turn: bool,
    pub parent_id: Option<u32>,
    pub write_access: bool,
    pub allowed_paths: Vec<String>,
    pub verification_command: Option<String>,
    pub workspace_root: Option<std::path::PathBuf>,
    pub review_manifest: Option<std::path::PathBuf>,
}

/// One entry of the agent's persistent task plan, managed via the `todo_write` tool.
/// The current list is re-injected into the system prompt every round so the agent
/// can execute its plan across turns instead of re-planning from scratch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct TodoItem {
    pub content: String,
    pub status: String,   // "pending" | "in_progress" | "completed"
    pub priority: String, // "high" | "medium" | "low"
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpEditState {
    pub is_add: bool,
    pub edit_index: Option<usize>,
    pub name_input: String,
    pub command_input: String,
    pub args_input: String,
    pub active_field: usize, // 0 = Name, 1 = Command, 2 = Args
    pub cursor_pos: usize,
}

impl McpEditState {
    pub fn active_buf_and_pos_mut(&mut self) -> (&mut String, &mut usize) {
        match self.active_field {
            0 => (&mut self.name_input, &mut self.cursor_pos),
            1 => (&mut self.command_input, &mut self.cursor_pos),
            _ => (&mut self.args_input, &mut self.cursor_pos),
        }
    }

    pub fn set_active_field(&mut self, field: usize) {
        self.active_field = field % 3;
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = buf.len();
    }

    pub fn insert_char(&mut self, c: char) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        buf.insert(*pos, c);
        *pos += c.len_utf8();
    }

    pub fn delete_char_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos > 0
            && let Some(c) = buf[..*pos].chars().next_back()
        {
            let len = c.len_utf8();
            *pos -= len;
            buf.remove(*pos);
        }
    }

    pub fn delete_char_right(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos < buf.len() {
            buf.remove(*pos);
        }
    }

    pub fn delete_word_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos == 0 {
            return;
        }
        let end = *pos;
        let mut start = *pos;
        while start > 0
            && buf[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace())
        {
            if let Some(c) = buf[..start].chars().next_back() {
                start -= c.len_utf8();
            }
        }
        while start > 0
            && buf[..start]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace())
        {
            if let Some(c) = buf[..start].chars().next_back() {
                start -= c.len_utf8();
            }
        }
        buf.drain(start..end);
        *pos = start;
    }

    pub fn delete_line_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        buf.drain(0..*pos);
        *pos = 0;
    }

    pub fn move_cursor_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos > 0
            && let Some(c) = buf[..*pos].chars().next_back()
        {
            *pos -= c.len_utf8();
        }
    }

    pub fn move_cursor_right(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos < buf.len()
            && let Some(c) = buf[*pos..].chars().next()
        {
            *pos += c.len_utf8();
        }
    }

    pub fn move_cursor_word_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        let mut p = *pos;
        while p > 0
            && buf[..p]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace())
        {
            if let Some(c) = buf[..p].chars().next_back() {
                p -= c.len_utf8();
            }
        }
        while p > 0
            && buf[..p]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace())
        {
            if let Some(c) = buf[..p].chars().next_back() {
                p -= c.len_utf8();
            }
        }
        *pos = p;
    }

    pub fn move_cursor_word_right(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        let mut p = *pos;
        while p < buf.len() && buf[p..].chars().next().is_some_and(|c| c.is_whitespace()) {
            if let Some(c) = buf[p..].chars().next() {
                p += c.len_utf8();
            }
        }
        while p < buf.len() && buf[p..].chars().next().is_some_and(|c| !c.is_whitespace()) {
            if let Some(c) = buf[p..].chars().next() {
                p += c.len_utf8();
            }
        }
        *pos = p;
    }

    pub fn move_cursor_home(&mut self) {
        let (_, pos) = self.active_buf_and_pos_mut();
        *pos = 0;
    }

    pub fn move_cursor_end(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = buf.len();
    }
}

/// Identity of the static prompt artifacts: they only depend on these inputs
/// plus the MCP tool generation. When any differs, the cache rebuilds.
#[derive(Clone, PartialEq)]
struct PromptCacheKey {
    include_agent_tools: bool,
    protocol: crate::config::ToolProtocol,
    agent_mode: crate::config::AgentMode,
    mcp_generation: u64,
}

/// Pre-computed static system prompt.
///
/// The prompt is otherwise rebuilt on every completion turn even though it only
/// changes when the protocol, agent mode, or MCP tool set changes. This caches
/// it and rebuilds lazily only when [`PromptCacheKey`] moves. Native schemas are
/// selected per request from the current conversation and explicit schema policy.
#[derive(Default)]
pub struct PromptCache {
    key: Option<PromptCacheKey>,
    system_prompt: String,
    skill_metadata: Option<Arc<Vec<crate::skills::SkillMetadata>>>,
    mcp_selection_generation: u64,
    mcp_selection_policy: Option<crate::tools::ToolSchemaPolicy>,
    mcp_selection_session_id: Option<String>,
    mcp_selected_names: Vec<String>,
}

impl PromptCache {
    fn ensure(
        &mut self,
        include_agent_tools: bool,
        protocol: crate::config::ToolProtocol,
        agent_mode: crate::config::AgentMode,
    ) {
        let key = PromptCacheKey {
            include_agent_tools,
            protocol,
            agent_mode,
            mcp_generation: crate::mcp::mcp_generation(),
        };
        if self.key.as_ref() != Some(&key) {
            self.system_prompt =
                crate::tools::tool_system_prompt(include_agent_tools, protocol, agent_mode);
            self.key = Some(key);
        }
    }

    /// The cached static system prompt for these inputs, rebuilding if stale.
    pub fn system_prompt(
        &mut self,
        include_agent_tools: bool,
        protocol: crate::config::ToolProtocol,
        agent_mode: crate::config::AgentMode,
    ) -> &str {
        self.ensure(include_agent_tools, protocol, agent_mode);
        &self.system_prompt
    }

    pub(crate) fn skill_metadata(&mut self) -> Arc<Vec<crate::skills::SkillMetadata>> {
        let metadata = self
            .skill_metadata
            .get_or_insert_with(|| Arc::new(crate::skills::discover_skills()));
        Arc::clone(metadata)
    }

    pub(crate) fn native_tool_schemas(
        &mut self,
        policy: crate::tools::ToolSchemaPolicy,
        messages: &[serde_json::Value],
        session_id: &str,
    ) -> (
        Vec<serde_json::Value>,
        crate::tools::McpSchemaSelectionStats,
    ) {
        let generation = crate::mcp::mcp_generation();
        if self.mcp_selection_generation != generation
            || self.mcp_selection_policy != Some(policy)
            || self.mcp_selection_session_id.as_deref() != Some(session_id)
        {
            self.mcp_selected_names.clear();
            self.mcp_selection_generation = generation;
            self.mcp_selection_policy = Some(policy);
            self.mcp_selection_session_id = Some(session_id.to_string());
        }

        let result = crate::tools::native_tools_schema_for_context_with_sticky(
            policy,
            messages,
            &self.mcp_selected_names,
        );
        self.mcp_selected_names = result.1.selected_names.clone();
        result
    }
}

/// What the pointer is currently over. Only clickable things get a variant, so
/// comparing the previous and current target tells the event loop whether a
/// pointer move actually changed anything worth redrawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HoverTarget {
    #[default]
    None,
    /// The jump-to-latest pill.
    ScrollPill,
    /// A code block's `[Copy]` badge, at this screen row.
    CopyBadge(u16),
}

#[derive(Debug, PartialEq, Clone, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Low,
    #[default]
    High,
}

/// Presentation-only state for a tool invocation that has not produced its
/// final history message yet. The provider payload and persisted history keep
/// using [`ToolCallRef`] and [`ToolResultRecord`]; this projection exists so
/// the TUI can update one live activity item instead of appending protocol
/// fragments to the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveToolOutputChunk {
    pub stderr: bool,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveToolCall {
    pub key: String,
    pub provider_call_id: Option<String>,
    pub tool_name: String,
    pub action: String,
    pub target: String,
    pub execution_started: bool,
    pub output: std::collections::VecDeque<LiveToolOutputChunk>,
    pub omitted_output_bytes: usize,
    pub started_at: std::time::Instant,
}

pub(super) const MAX_LIVE_TOOL_OUTPUT_BYTES: usize = 32 * 1024;

impl LiveToolCall {
    pub(crate) fn new(
        key: impl Into<String>,
        provider_call_id: Option<String>,
        tool_name: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            provider_call_id,
            tool_name: tool_name.into(),
            action: action.into(),
            target: target.into(),
            execution_started: true,
            output: std::collections::VecDeque::new(),
            omitted_output_bytes: 0,
            started_at: std::time::Instant::now(),
        }
    }
}
