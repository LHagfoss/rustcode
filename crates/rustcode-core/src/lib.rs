//! Stable, protocol-neutral domain values shared by RustCode subsystems.
//!
//! This crate deliberately has no UI, networking, filesystem, or tool
//! execution dependencies.  Keep behavior-specific helpers at the boundary
//! that owns them; the values here are safe to use from any future subsystem
//! crate.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn current_timestamp() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

/// Usage reported by a provider for one completion request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
}

/// Completeness of the output delivered to the model for one tool result.
///
/// This is deliberately separate from the presentation used by the terminal:
/// a collapsed UI row must never make a complete result look truncated (or
/// vice versa) when the conversation is replayed to a provider.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultCompleteness {
    #[default]
    Complete,
    /// The requested range was complete, but the caller deliberately limited
    /// it (for example, an explicit end_line before the end of a file).
    UserLimited,
    /// The read window omitted content because of the line safety cap.
    LineTruncated,
    /// Output was bounded by a byte/line payload limit after execution.
    ByteTruncated,
}

impl ToolResultCompleteness {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::UserLimited => "user_limited",
            Self::LineTruncated => "line_truncated",
            Self::ByteTruncated => "byte_truncated",
        }
    }
}

/// A line-oriented range carried by a model-facing inspection result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InspectionRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
}

/// Canonical, bounded facts about a read-only inspection. The terminal/UI
/// presentation remains the separate `ChatMessage::content` field.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InspectionResultMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_range: Option<InspectionRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_range: Option<InspectionRange>,
    /// True when the requested inspection was fully delivered. A
    /// `user_limited` read is complete for its explicitly requested range.
    #[serde(default)]
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_range: Option<InspectionRange>,
    /// Contiguous source ranges whose complete numbered lines were actually
    /// delivered to the model. `returned_range` remains the first range for
    /// compatibility; this preserves head/tail delivery without claiming the
    /// omitted middle was returned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delivered_ranges: Vec<InspectionRange>,
    /// Stable semantic identity shared by equivalent inspection tools.
    pub fingerprint: String,
}

/// Identity of one structured tool call, retained for transcript replay.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallRef {
    pub id: String,
    pub name: String,
    /// Arguments exactly as the provider sent them.
    pub arguments: String,
}

/// Stable spelling of a tool failure persisted in session history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolErrorKind {
    Validation,
    InvalidArguments,
    EditMismatch,
    PermissionDenied,
    CommandFailed,
    CompilerFailed,
    Cancelled,
    McpFailed,
    Internal,
    OutputLimit,
    ProviderFailed,
    UnavailableDependency,
    Unknown,
}

impl ToolErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "Validation",
            Self::InvalidArguments => "InvalidArguments",
            Self::EditMismatch => "EditMismatch",
            Self::PermissionDenied => "PermissionDenied",
            Self::CommandFailed => "CommandFailed",
            Self::CompilerFailed => "CompilerFailed",
            Self::Cancelled => "Cancelled",
            Self::McpFailed => "McpFailed",
            Self::Internal => "Internal",
            Self::OutputLimit => "OutputLimit",
            Self::ProviderFailed => "ProviderFailed",
            Self::UnavailableDependency => "UnavailableDependency",
            Self::Unknown => "Unknown",
        }
    }

    pub fn from_persisted(value: &str) -> Option<Self> {
        Some(match value {
            "Validation" => Self::Validation,
            "InvalidArguments" => Self::InvalidArguments,
            "EditMismatch" => Self::EditMismatch,
            "PermissionDenied" => Self::PermissionDenied,
            "CommandFailed" => Self::CommandFailed,
            "CompilerFailed" => Self::CompilerFailed,
            "Cancelled" => Self::Cancelled,
            "McpFailed" => Self::McpFailed,
            "Internal" => Self::Internal,
            "OutputLimit" => Self::OutputLimit,
            "ProviderFailed" => Self::ProviderFailed,
            "UnavailableDependency" => Self::UnavailableDependency,
            "Unknown" => Self::Unknown,
            _ => return None,
        })
    }

    #[allow(dead_code)]
    pub fn from_message(msg: &str) -> Self {
        let lower = msg.to_ascii_lowercase();
        if lower.contains("missing") || lower.contains("invalid argument") {
            Self::InvalidArguments
        } else if lower.contains("edit") && lower.contains("mismatch") {
            Self::EditMismatch
        } else if lower.contains("permission") || lower.contains("denied") {
            Self::PermissionDenied
        } else if lower.contains("compiler") || lower.contains("cargo check") {
            Self::CompilerFailed
        } else if lower.contains("cancelled") {
            Self::Cancelled
        } else if lower.contains("not found") || lower.contains("unavailable") {
            Self::UnavailableDependency
        } else if lower.contains("exit code") {
            Self::CommandFailed
        } else {
            Self::Unknown
        }
    }
}

/// Authoritative metadata for one tool result, persisted with history.
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
    /// Machine-readable completeness of the model-facing output. Older
    /// sessions omit this field and deserialize as `complete`.
    #[serde(default)]
    pub completeness: ToolResultCompleteness,
    pub full_output_artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub replayed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection: Option<InspectionResultMetadata>,
}

impl ToolResultRecord {
    /// Map records from before the completeness field was introduced to an
    /// honest state instead of treating their old `truncated` bit as complete.
    pub fn resolved_completeness(&self) -> ToolResultCompleteness {
        if self.completeness == ToolResultCompleteness::Complete && self.truncated {
            ToolResultCompleteness::ByteTruncated
        } else {
            self.completeness
        }
    }

    /// Convert a persisted spelling to the typed error contract.
    pub fn parsed_error_kind(&self) -> Option<ToolErrorKind> {
        self.error_kind
            .as_deref()
            .and_then(ToolErrorKind::from_persisted)
    }
}

/// A durable conversation message. The diff and file preview fields are
/// intentionally ephemeral and retain their historical serde behavior.
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
    #[serde(skip)]
    pub file_preview: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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

    pub fn with_tool_calls(mut self, calls: Vec<ToolCallRef>) -> Self {
        self.tool_calls = calls;
        self
    }

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
}

/// Durable history with a cheap mutation marker for snapshot consumers.
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

    pub fn snapshot(&self) -> Self {
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

/// Agent execution mode selected in configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    #[default]
    Build,
    Plan,
}

/// Tool protocol selected for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolProtocol {
    #[default]
    Json,
    Native,
    ApiNative,
}

/// Transcript verbosity preference.
#[derive(Debug, PartialEq, Clone, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Low,
    #[default]
    High,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_revision_distinguishes_append_and_rewrite() {
        let mut history = History::from(vec![ChatMessage::new("user", "hello")]);
        let revision = history.revision();
        history.push(ChatMessage::new("assistant", "hi"));
        assert!(history.is_append_only_since(revision));

        history.replace(vec![ChatMessage::new("system", "reset")]);
        assert!(!history.is_append_only_since(revision));
    }

    #[test]
    fn persisted_message_shape_keeps_legacy_optional_fields() {
        let message = ChatMessage::new("tool", "done").with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_string(),
            arguments_hash: "hash".to_string(),
            success: true,
            ..Default::default()
        });
        let json = serde_json::to_string(&message).expect("serialize message");
        assert!(json.contains("\"tool_result\""));
        assert!(!json.contains("\"diff\""));
        assert_eq!(serde_json::from_str::<ChatMessage>(&json).unwrap(), message);
    }

    #[test]
    fn tool_error_spelling_round_trips() {
        assert_eq!(
            ToolErrorKind::from_persisted("CommandFailed"),
            Some(ToolErrorKind::CommandFailed)
        );
        assert_eq!(ToolErrorKind::CommandFailed.as_str(), "CommandFailed");
    }
}
