use super::compact::SUMMARY_MARKER;
use crate::app::ChatMessage;

pub const STRUCTURED_MEMORY_MARKER: &str = "[Deterministic context record]";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReadEvidence {
    pub path: String,
    /// `documentation`, `source`, or `other`; this classifies the artifact
    /// inspected and says nothing about whether it was tested or run.
    pub evidence_type: String,
    pub completeness: String,
    pub ranges: Vec<rustcode_core::InspectionRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_epoch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_path: Option<String>,
    pub archive_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommandEvidence {
    pub command: String,
    pub success: bool,
    pub pending: bool,
    pub exit_code: Option<i32>,
    pub completeness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_artifact: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StructuredSessionMemory {
    pub initial_goal: String,
    pub current_task: Option<String>,
    pub user_constraints: Vec<String>,
    pub key_architecture: Vec<String>,
    pub inspected_files: Vec<String>,
    #[serde(default)]
    pub read_evidence: Vec<ReadEvidence>,
    pub modified_files: Vec<String>,
    pub decisions: Vec<String>,
    pub failures_and_errors: Vec<String>,
    pub verification_state: Vec<String>,
    #[serde(default)]
    pub command_evidence: Vec<CommandEvidence>,
}

/// System messages the harness writes as turn bookkeeping. They are not
/// project instructions and must not be carried into the record as such.
fn is_harness_notice(content: &str) -> bool {
    matches!(
        content.trim(),
        "Request cancelled by user" | "YOLO mode enabled" | "YOLO mode disabled"
    )
}

/// A follow-up that only says "keep going" or acknowledges the last answer
/// ("looks good"). It carries no task, so it must not replace the last
/// follow-up that did.
fn is_bare_continuation(message: &str) -> bool {
    let normalized = message
        .trim()
        .trim_end_matches(['.', '!'])
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "continue"
            | "go on"
            | "keep going"
            | "proceed"
            | "go ahead"
            | "ok"
            | "okay"
            | "yes"
            | "looks good"
            | "lgtm"
            | "good"
            | "great"
            | "nice"
            | "perfect"
            | "cool"
            | "thanks"
            | "thank you"
            | "sounds good"
    )
}

/// What a later success of the same kind resolves: a command by its text, a
/// file edit by any later edit, any other tool by its name.
fn failure_key(tool_name: &str, command: Option<&str>) -> String {
    match tool_name {
        "run_command" | "background_task" => format!("{tool_name}:{}", command.unwrap_or("")),
        "replace_file_content"
        | "multi_replace_file_content"
        | "write_to_file"
        | "write_file_chunk" => "file_edit".to_owned(),
        _ => tool_name.to_owned(),
    }
}

const READ_EVIDENCE_ARCHIVE_PREFIX: &str = "Read evidence archive: ";
const PROGRESS_NOTES_PREFIX: &str = "Recent progress notes (assistant narration, not verified): ";

pub(crate) fn compact_context_line(content: &str, max_chars: usize) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    line.chars().take(max_chars).collect()
}

pub(crate) fn compact_context_block(content: &str, max_chars: usize) -> String {
    let block = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    block.chars().take(max_chars).collect()
}

impl StructuredSessionMemory {
    pub fn extract_from_history(history: &[ChatMessage]) -> Self {
        let mut memory = Self::default();
        // Failures seen in this history, keyed so a later success of the same
        // kind can resolve them; only the unresolved ones reach the record.
        let mut failures: Vec<(Option<String>, String)> = Vec::new();

        for (index, message) in history.iter().enumerate() {
            if message.conversation_recap {
                continue;
            }
            if message.role == "system" {
                if message.content.starts_with(STRUCTURED_MEMORY_MARKER)
                    || message.content.starts_with(SUMMARY_MARKER)
                    || message.content.starts_with("[Structured Session Memory]")
                    || message
                        .content
                        .starts_with("[Deterministic context record]")
                {
                    memory.merge_from_text(&message.content);
                } else if !message.content.starts_with('[') && !is_harness_notice(&message.content)
                {
                    let block = compact_context_block(&message.content, 900);
                    if !block.is_empty() && !memory.user_constraints.contains(&block) {
                        memory.user_constraints.push(block);
                    }
                }
            }

            if message.role == "user" && !message.content.starts_with("<tool_result>") {
                if memory.initial_goal.is_empty() {
                    memory.initial_goal = compact_context_line(&message.content, 700);
                } else {
                    let task = compact_context_line(&message.content, 700);
                    if !task.is_empty()
                        && task != memory.initial_goal
                        && !is_bare_continuation(&task)
                    {
                        memory.current_task = Some(task);
                    }
                }

                let lower = message.content.to_ascii_lowercase();
                if lower.contains("never")
                    || lower.contains("do not")
                    || lower.contains("don't")
                    || lower.contains("must")
                    || lower.contains("always")
                    || lower.contains("constraint")
                    || lower.contains("rule")
                    || lower.contains("preference")
                {
                    for line in message.content.lines() {
                        let trimmed = line.trim();
                        let l = trimmed.to_ascii_lowercase();
                        if (l.contains("never")
                            || l.contains("do not")
                            || l.contains("don't")
                            || l.contains("must")
                            || l.contains("always")
                            || l.contains("rule")
                            || l.contains("constraint")
                            || l.contains("preference"))
                            && !memory.user_constraints.iter().any(|c| c == trimmed)
                        {
                            memory.user_constraints.push(trimmed.to_string());
                        }
                    }
                }
            }

            if let Some(ref result) = message.tool_result {
                for path in &result.changed_paths {
                    if !memory.modified_files.contains(path) {
                        memory.modified_files.push(path.clone());
                    }
                }
                let key = failure_key(&result.tool_name, result.command.as_deref());
                if !result.success || result.error_kind.is_some() {
                    let err = format!(
                        "{} ({}, exit={:?})",
                        result.tool_name,
                        result.error_kind.as_deref().unwrap_or("failed"),
                        result.exit_code
                    );
                    if !failures.iter().any(|(_, existing)| existing == &err) {
                        failures.push((Some(key), err));
                    }
                } else {
                    failures.retain(|(failed, _)| failed.as_deref() != Some(key.as_str()));
                }
                if matches!(result.tool_name.as_str(), "run_command" | "background_task") {
                    if let Some(command) = result.command.as_ref() {
                        let evidence = CommandEvidence {
                            command: command.clone(),
                            success: result.success,
                            pending: result.pending,
                            exit_code: result.exit_code,
                            completeness: result.resolved_completeness().as_str().to_string(),
                            tool_call_id: message.tool_call_id.clone(),
                            evidence_hash: result.evidence_hash.clone(),
                            full_output_artifact: result.full_output_artifact.clone(),
                        };
                        if !memory.command_evidence.contains(&evidence) {
                            memory.command_evidence.push(evidence);
                        }
                    }
                }

                if matches!(result.tool_name.as_str(), "view_file" | "read_file")
                    && result.success
                    && let Some(inspection) = result.inspection.as_ref()
                    && let Some(path) = inspection
                        .returned_path
                        .as_ref()
                        .or(inspection.requested_path.as_ref())
                {
                    let ranges = if !inspection.delivered_ranges.is_empty() {
                        inspection.delivered_ranges.clone()
                    } else {
                        inspection
                            .returned_range
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                    };
                    let evidence_type = match std::path::Path::new(path)
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .map(str::to_ascii_lowercase)
                        .as_deref()
                    {
                        Some("md" | "markdown" | "rst" | "txt") => "documentation",
                        Some(
                            "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "c" | "h"
                            | "cpp" | "hpp" | "cs" | "rb" | "php" | "swift" | "kt" | "scala",
                        ) => "source",
                        _ => "other",
                    };
                    let evidence = ReadEvidence {
                        path: path.clone(),
                        evidence_type: evidence_type.to_string(),
                        completeness: result.resolved_completeness().as_str().to_string(),
                        ranges,
                        workspace_epoch: result.workspace_epoch.clone(),
                        workspace_generation: result.workspace_generation,
                        tool_call_id: message.tool_call_id.clone(),
                        evidence_hash: result.evidence_hash.clone(),
                        full_output_artifact: result.full_output_artifact.clone(),
                        archive_path: None,
                        archive_line: index + 1,
                    };
                    if !memory.read_evidence.contains(&evidence) {
                        memory.read_evidence.push(evidence);
                    }
                    if !memory.inspected_files.contains(path) {
                        memory.inspected_files.push(path.clone());
                    }
                }
            }

            // Text matching is a fallback for failures the typed result does
            // not mark; a successful read of code that mentions `error:` is not
            // a failure.
            if message.role == "tool"
                && message
                    .tool_result
                    .as_ref()
                    .is_none_or(|result| !result.success || result.error_kind.is_some())
            {
                if let Some((tool, body)) = message.content.split_once(": ") {
                    if body.contains("error:")
                        || body.contains("exit code: 1")
                        || body.contains("FAILED")
                    {
                        let snippet = compact_context_line(body, 200);
                        let key = message.tool_result.as_ref().map_or_else(
                            || failure_key(tool, None),
                            |result| failure_key(&result.tool_name, result.command.as_deref()),
                        );
                        if !snippet.is_empty()
                            && !failures.iter().any(|(_, existing)| existing == &snippet)
                        {
                            failures.push((Some(key), snippet));
                        }
                    }
                }
            }

            if message.role == "assistant" {
                let prose = rustcode_tool_protocol::text::strip_think_blocks(&message.content);
                let line = compact_context_line(&prose, 300);
                if !line.is_empty()
                    && !line.starts_with("```tool")
                    && !line.starts_with('{')
                    && !line.starts_with('!')
                    && !line.starts_with("• ")
                    && !memory.decisions.contains(&line)
                {
                    memory.decisions.push(line);
                }
            }
        }

        for (_, failure) in failures {
            if !memory.failures_and_errors.contains(&failure) {
                memory.failures_and_errors.push(failure);
            }
        }
        memory
    }

    pub fn merge_from_text(&mut self, text: &str) {
        let mut read_archive: Option<String> = None;
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(archive) = trimmed.strip_prefix(READ_EVIDENCE_ARCHIVE_PREFIX) {
                read_archive = Some(archive.to_owned());
            } else if let Some(goal) = trimmed.strip_prefix("Goal: ") {
                if self.initial_goal.is_empty() {
                    self.initial_goal = goal.to_string();
                }
            } else if let Some(task) = trimmed.strip_prefix("Current follow-up: ") {
                if self.current_task.is_none() {
                    self.current_task = Some(task.to_string());
                }
            } else if let Some(constraints) =
                trimmed.strip_prefix("Project instructions/constraints: ")
            {
                for c in constraints.split("; ") {
                    if !self.user_constraints.iter().any(|existing| existing == c) {
                        self.user_constraints.push(c.to_string());
                    }
                }
            } else if let Some(files) = trimmed.strip_prefix("Modified files: ") {
                for f in files.split(", ") {
                    if !self.modified_files.iter().any(|existing| existing == f) {
                        self.modified_files.push(f.to_string());
                    }
                }
            } else if let Some(files) = trimmed
                .strip_prefix("Inspected files: ")
                .or_else(|| trimmed.strip_prefix("Inspected file paths: "))
            {
                for f in files.split(", ") {
                    if !f.is_empty() && !self.inspected_files.iter().any(|existing| existing == f) {
                        self.inspected_files.push(f.to_string());
                    }
                }
            } else if let Some(evidence) = trimmed.strip_prefix("Read evidence: ")
                && let Ok(mut evidence) = serde_json::from_str::<ReadEvidence>(evidence)
                && {
                    if evidence.archive_path.is_none() {
                        evidence.archive_path.clone_from(&read_archive);
                    }
                    !self.read_evidence.contains(&evidence)
                }
            {
                if !self.inspected_files.contains(&evidence.path) {
                    self.inspected_files.push(evidence.path.clone());
                }
                self.read_evidence.push(evidence);
            } else if let Some(evidence) = trimmed.strip_prefix("Command evidence: ")
                && let Ok(evidence) = serde_json::from_str::<CommandEvidence>(evidence)
                && !self.command_evidence.contains(&evidence)
            {
                self.command_evidence.push(evidence);
            } else if let Some(failures) = trimmed.strip_prefix("Failures/unresolved work: ") {
                for fail in failures.split("; ") {
                    if !self
                        .failures_and_errors
                        .iter()
                        .any(|existing| existing == fail)
                    {
                        self.failures_and_errors.push(fail.to_string());
                    }
                }
            } else if let Some(verifications) = trimmed
                .strip_prefix("Verification state: ")
                .or_else(|| trimmed.strip_prefix("Command notes (outcome unverified): "))
            {
                for v in verifications.split("; ") {
                    if !self.verification_state.iter().any(|existing| existing == v) {
                        self.verification_state.push(v.to_string());
                    }
                }
            } else if let Some(arch) = trimmed.strip_prefix("Key architecture: ") {
                for a in arch.split("; ") {
                    if !self.key_architecture.iter().any(|existing| existing == a) {
                        self.key_architecture.push(a.to_string());
                    }
                }
            } else if let Some(decisions) = trimmed
                .strip_prefix(PROGRESS_NOTES_PREFIX)
                .or_else(|| trimmed.strip_prefix("Architecture/decisions/next steps: "))
            {
                for d in decisions.split("; ") {
                    if !self.decisions.iter().any(|existing| existing == d) {
                        self.decisions.push(d.to_string());
                    }
                }
            } else if let Some(constraint) = trimmed.strip_prefix("- Constraint: ") {
                if !self.user_constraints.iter().any(|c| c == constraint) {
                    self.user_constraints.push(constraint.to_string());
                }
            } else if let Some(arch) = trimmed.strip_prefix("- Architecture: ") {
                if !self.key_architecture.iter().any(|a| a == arch) {
                    self.key_architecture.push(arch.to_string());
                }
            } else if let Some(decision) = trimmed.strip_prefix("- Decision: ") {
                if !self.decisions.iter().any(|d| d == decision) {
                    self.decisions.push(decision.to_string());
                }
            } else if let Some(failure) = trimmed.strip_prefix("- Failure: ") {
                if !self.failures_and_errors.iter().any(|f| f == failure) {
                    self.failures_and_errors.push(failure.to_string());
                }
            }
        }
    }

    pub fn format_record(&self, max_chars: usize) -> String {
        let mut out = format!("{STRUCTURED_MEMORY_MARKER}\n");
        if !self.initial_goal.is_empty() {
            out.push_str(&format!("Goal: {}\n", self.initial_goal));
        }
        if let Some(ref task) = self.current_task {
            out.push_str(&format!("Current follow-up: {}\n", task));
        }
        if !self.user_constraints.is_empty() {
            out.push_str("Project instructions/constraints: ");
            out.push_str(
                &self
                    .user_constraints
                    .iter()
                    .take(12)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            out.push('\n');
        }
        if !self.modified_files.is_empty() {
            out.push_str(&format!(
                "Modified files: {}\n",
                self.modified_files
                    .iter()
                    .take(20)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.inspected_files.is_empty() {
            out.push_str(&format!(
                "Inspected file paths: {}\nRead path inventory does not imply full-file inspection; see read evidence for delivered ranges and completeness.\n",
                self.inspected_files
                    .iter()
                    .take(30)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.failures_and_errors.is_empty() {
            out.push_str("Failures/unresolved work: ");
            out.push_str(
                &self
                    .failures_and_errors
                    .iter()
                    .rev()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            out.push('\n');
        }
        if !self.verification_state.is_empty() {
            out.push_str(&format!(
                "Command notes (outcome unverified): {}\n",
                self.verification_state
                    .iter()
                    .rev()
                    .take(6)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if !self.key_architecture.is_empty() {
            out.push_str("Key architecture: ");
            out.push_str(
                &self
                    .key_architecture
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            out.push('\n');
        }
        if !self.decisions.is_empty() {
            out.push_str(PROGRESS_NOTES_PREFIX);
            out.push_str(
                &self
                    .decisions
                    .iter()
                    .rev()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            out.push('\n');
        }
        let base_chars = out.chars().count();
        if base_chars >= max_chars {
            return out.chars().take(max_chars).collect();
        }
        // Command results say what passed or failed; they go first so a long
        // read inventory cannot crowd them out.
        out.push_str(&self.format_command_evidence(max_chars - base_chars));
        let used_chars = out.chars().count();
        if used_chars < max_chars {
            out.push_str(&self.format_read_evidence(max_chars - used_chars));
        }
        out
    }

    pub fn attach_archive_to_unlinked_reads(&mut self, archive_path: &str) {
        for evidence in &mut self.read_evidence {
            if evidence.archive_path.is_none() {
                evidence.archive_path = Some(archive_path.to_string());
            }
        }
    }

    pub fn format_read_evidence(&self, max_chars: usize) -> String {
        if self.read_evidence.is_empty() {
            return String::new();
        }
        let mut out = String::from(
            "Read evidence (document/source inspection only; this does not establish testing or runtime validation):\n",
        );
        // Records from one compaction share an archive; name it once instead
        // of on every line.
        let shared_archive = self
            .read_evidence
            .first()
            .and_then(|evidence| evidence.archive_path.as_deref())
            .filter(|archive| {
                self.read_evidence
                    .iter()
                    .all(|evidence| evidence.archive_path.as_deref() == Some(*archive))
            });
        if let Some(archive) = shared_archive {
            out.push_str(&format!("{READ_EVIDENCE_ARCHIVE_PREFIX}{archive}\n"));
        }
        if out.chars().count() > max_chars {
            return String::new();
        }
        let mut included = 0usize;
        for evidence in self.read_evidence.iter().rev().take(12) {
            let json = if shared_archive.is_some() {
                let mut line = evidence.clone();
                line.archive_path = None;
                serde_json::to_string(&line)
            } else {
                serde_json::to_string(evidence)
            };
            let Ok(json) = json else {
                continue;
            };
            let line = format!("Read evidence: {json}\n");
            let omitted = self.read_evidence.len().saturating_sub(included + 1);
            let note = format!(
                "{omitted} additional read records remain in the exact JSONL transcript archive.\n"
            );
            if out.chars().count() + line.chars().count() + note.chars().count() > max_chars {
                continue;
            }
            out.push_str(&line);
            included += 1;
        }
        let omitted = self.read_evidence.len().saturating_sub(included);
        if omitted > 0 {
            let note = format!(
                "{omitted} additional read records remain in the exact JSONL transcript archive; search by tool_call_id or evidence_hash.\n"
            );
            if out.chars().count() + note.chars().count() <= max_chars {
                out.push_str(&note);
            }
        }
        out
    }

    pub fn format_command_evidence(&self, max_chars: usize) -> String {
        if self.command_evidence.is_empty() {
            return String::new();
        }
        let heading = "Command evidence (observed commands/results; classification does not assert testing or runtime validation):\n";
        if heading.chars().count() > max_chars {
            return String::new();
        }
        let mut out = heading.to_string();
        let mut included = 0usize;
        for evidence in self.command_evidence.iter().rev().take(8) {
            let Ok(json) = serde_json::to_string(evidence) else {
                continue;
            };
            let line = format!("Command evidence: {json}\n");
            if out.chars().count() + line.chars().count() > max_chars {
                continue;
            }
            out.push_str(&line);
            included += 1;
        }
        let omitted = self.command_evidence.len().saturating_sub(included);
        if omitted > 0 {
            let note = format!(
                "{omitted} earlier command results remain in the exact JSONL transcript archive.\n"
            );
            if out.chars().count() + note.chars().count() <= max_chars {
                out.push_str(&note);
            }
        }
        out
    }
}

pub fn compact_with_structured_memory(
    history: &mut Vec<ChatMessage>,
    keep_recent_count: usize,
    budget: usize,
) -> bool {
    let archival_source = history.clone();
    compact_with_structured_memory_from_archive(
        history,
        keep_recent_count,
        budget,
        &archival_source,
    )
}

pub(super) fn compact_with_structured_memory_from_archive(
    history: &mut Vec<ChatMessage>,
    keep_recent_count: usize,
    budget: usize,
    archival_source: &[ChatMessage],
) -> bool {
    if history.len() <= keep_recent_count || history.len() < 4 {
        return false;
    }
    let desired_cutoff = history.len().saturating_sub(keep_recent_count);
    let cutoff = super::compact::bounded_recent_suffix_start(
        history,
        desired_cutoff,
        (budget as f64 * 0.3) as usize,
    );
    if cutoff == 0 {
        return false;
    }
    let Some(archival_prefix) = archival_source.get(..cutoff) else {
        crate::dbg_log!(
            "Skipping structured compaction because archival history boundary is invalid"
        );
        return false;
    };
    let history_archive = match crate::config::archive_history_prefix(archival_prefix) {
        Ok(path) => path,
        Err(error) => {
            crate::dbg_log!(
                "Skipping structured compaction because history archive failed: {error}"
            );
            return false;
        }
    };
    let mut memory = StructuredSessionMemory::extract_from_history(archival_prefix);
    memory.attach_archive_to_unlinked_reads(&history_archive);
    let max_chars = budget.saturating_mul(3).clamp(1000, 8000);
    let record = memory.format_record(max_chars);

    let tail = history[cutoff..].to_vec();
    let summary_message =
        super::compact::durable_compaction_record_message(&record, &tail, &history_archive);
    history.clear();
    history.push(summary_message);
    history.extend(tail);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcode_core::{InspectionRange, InspectionResultMetadata, ToolResultCompleteness};

    #[test]
    fn harness_notices_and_successful_reads_are_not_carried_as_constraints_or_failures() {
        let mut read = ChatMessage::new(
            "tool",
            "view_file: 1: try? await respond(id: id, error: .methodNotFound(method))",
        );
        read.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "view_file".into(),
            success: true,
            ..Default::default()
        });
        let history = vec![
            ChatMessage::new("user", "build the app"),
            ChatMessage::new("system", "Request cancelled by user"),
            ChatMessage::new("system", "Keep the engine headless."),
            read,
            ChatMessage::new("tool", "run_command: error: Could not find Package.swift"),
        ];
        let memory = StructuredSessionMemory::extract_from_history(&history);
        assert_eq!(memory.user_constraints, ["Keep the engine headless."]);
        assert_eq!(
            memory.failures_and_errors,
            ["error: Could not find Package.swift"]
        );
    }

    #[test]
    fn bare_continuation_keeps_the_last_substantive_follow_up() {
        let history = vec![
            ChatMessage::new("user", "build the app"),
            ChatMessage::new("user", "also run a review"),
            ChatMessage::new("user", "Continue."),
        ];
        let memory = StructuredSessionMemory::extract_from_history(&history);
        assert_eq!(memory.current_task.as_deref(), Some("also run a review"));
    }

    // Session 01a16b37: the record carried edit mismatches that later edits
    // had fixed, "looks good" as the current task, and the archive path on
    // every read line.
    #[test]
    fn record_drops_resolved_failures_and_acknowledgements() {
        let failed_edit = |content: &str| {
            let mut message = ChatMessage::new("tool", content);
            message.tool_result = Some(crate::app::ToolResultRecord {
                tool_name: "multi_replace_file_content".into(),
                success: false,
                error_kind: Some("Internal".into()),
                ..Default::default()
            });
            message
        };
        let mut fixed_edit = ChatMessage::new("tool", "replace_file_content: applied");
        fixed_edit.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "replace_file_content".into(),
            success: true,
            ..Default::default()
        });
        let mut failed_test = ChatMessage::new("tool", "run_command: error: 1 test failed");
        failed_test.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "run_command".into(),
            command: Some("bun test".into()),
            success: false,
            exit_code: Some(1),
            ..Default::default()
        });
        let mut other_command = ChatMessage::new("tool", "run_command: ok");
        other_command.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "run_command".into(),
            command: Some("ls".into()),
            success: true,
            exit_code: Some(0),
            ..Default::default()
        });
        let history = vec![
            ChatMessage::new("user", "build the game"),
            ChatMessage::new("user", "add a sell button"),
            failed_edit("multi_replace_file_content: error: Discrepancy at replacement index 0"),
            fixed_edit,
            failed_test,
            other_command,
            ChatMessage::new("user", "looks good"),
        ];
        let memory = StructuredSessionMemory::extract_from_history(&history);
        assert_eq!(memory.current_task.as_deref(), Some("add a sell button"));
        assert_eq!(
            memory.failures_and_errors,
            ["run_command (failed, exit=Some(1))", "error: 1 test failed"]
        );
    }

    #[test]
    fn record_names_a_shared_archive_once_and_lists_commands_first() {
        let read = |path: &str| {
            let mut message = ChatMessage::new("tool", "view_file: ok");
            message.tool_result = Some(crate::app::ToolResultRecord {
                tool_name: "view_file".into(),
                success: true,
                inspection: Some(InspectionResultMetadata {
                    returned_path: Some(path.into()),
                    ..Default::default()
                }),
                ..Default::default()
            });
            message
        };
        let mut command = ChatMessage::new("tool", "run_command: ok");
        command.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "run_command".into(),
            command: Some("bun test".into()),
            success: true,
            exit_code: Some(0),
            ..Default::default()
        });
        let history = vec![
            ChatMessage::new("user", "inspect"),
            read("src/a.js"),
            read("src/b.js"),
            command,
        ];
        let archive = "/config/history_archive/shared.jsonl";
        let mut memory = StructuredSessionMemory::extract_from_history(&history);
        memory.attach_archive_to_unlinked_reads(archive);
        let record = memory.format_record(4_000);
        assert_eq!(record.matches(archive).count(), 1, "{record}");
        assert!(!record.contains("null"), "{record}");
        assert!(
            record.find("Command evidence:") < record.find("Read evidence:"),
            "{record}"
        );
        let mut restored = StructuredSessionMemory::default();
        restored.merge_from_text(&record);
        let mut restored_reads = restored.read_evidence.clone();
        let mut original_reads = memory.read_evidence.clone();
        restored_reads.sort_by(|left, right| left.path.cmp(&right.path));
        original_reads.sort_by(|left, right| left.path.cmp(&right.path));
        assert_eq!(restored_reads, original_reads);
    }

    #[test]
    fn read_and_command_receipts_survive_structured_memory_round_trip() {
        let mut read = ChatMessage::new("tool", "view_file: lines 8-12");
        read.tool_call_id = Some("read-call".into());
        read.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "view_file".into(),
            success: true,
            completeness: ToolResultCompleteness::LineTruncated,
            workspace_epoch: Some("epoch-a".into()),
            workspace_generation: Some(42),
            evidence_hash: Some("sha256:read".into()),
            full_output_artifact: Some("artifact://read".into()),
            inspection: Some(InspectionResultMetadata {
                requested_path: Some("src/lib.rs".into()),
                returned_path: Some("src/lib.rs".into()),
                returned_range: Some(InspectionRange {
                    start: Some(8),
                    end: Some(12),
                }),
                delivered_ranges: vec![InspectionRange {
                    start: Some(8),
                    end: Some(12),
                }],
                complete: false,
                fingerprint: "fingerprint".into(),
                ..Default::default()
            }),
            ..Default::default()
        });

        let mut command = ChatMessage::new("tool", "run_command: cargo test -p rustcode; exit 0");
        command.tool_call_id = Some("command-call".into());
        command.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "run_command".into(),
            command: Some("cargo test -p rustcode".into()),
            success: true,
            exit_code: Some(0),
            completeness: ToolResultCompleteness::Complete,
            evidence_hash: Some("sha256:command".into()),
            full_output_artifact: Some("artifact://command".into()),
            ..Default::default()
        });

        let mut background = ChatMessage::new("tool", "background_task: check complete");
        background.tool_call_id = Some("background-call".into());
        background.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "background_task".into(),
            command: Some("cargo check --tests".into()),
            success: true,
            pending: false,
            exit_code: Some(0),
            evidence_hash: Some("sha256:background".into()),
            full_output_artifact: Some("artifact://background".into()),
            ..Default::default()
        });

        let history = vec![
            ChatMessage::new("user", "inspect and check"),
            read,
            command,
            background,
        ];
        let archive = "/config/history_archive/example.jsonl";
        let mut original = StructuredSessionMemory::extract_from_history(&history);
        original.attach_archive_to_unlinked_reads(archive);
        let record = original.format_record(4_000);
        assert!(record.contains("Read evidence:"));
        assert!(record.contains("Command evidence:"));
        assert!(record.contains("cargo test -p rustcode"));
        assert!(record.contains("cargo check --tests"));
        assert!(record.contains("\"exit_code\":0"));
        assert!(!record.contains("tested"));
        assert!(!record.contains("runtime-validated"));

        let mut restored = StructuredSessionMemory::default();
        restored.merge_from_text(&record);
        assert_eq!(restored.read_evidence, original.read_evidence);
        let mut restored_commands = restored.command_evidence.clone();
        let mut original_commands = original.command_evidence.clone();
        restored_commands.sort_by(|left, right| left.command.cmp(&right.command));
        original_commands.sort_by(|left, right| left.command.cmp(&right.command));
        assert_eq!(restored_commands, original_commands);
        let evidence = &restored.read_evidence[0];
        assert_eq!(evidence.path, "src/lib.rs");
        assert_eq!(evidence.archive_path.as_deref(), Some(archive));
        assert_eq!(evidence.tool_call_id.as_deref(), Some("read-call"));
        assert_eq!(evidence.ranges[0].start, Some(8));
        assert_eq!(evidence.ranges[0].end, Some(12));
        assert_eq!(evidence.completeness, "line_truncated");
        assert_eq!(
            restored
                .command_evidence
                .iter()
                .find(|evidence| evidence.tool_call_id.as_deref() == Some("command-call"))
                .map(|evidence| (evidence.command.as_str(), evidence.exit_code)),
            Some(("cargo test -p rustcode", Some(0)))
        );
        let background = restored
            .command_evidence
            .iter()
            .find(|evidence| evidence.tool_call_id.as_deref() == Some("background-call"))
            .expect("background command receipt");
        assert_eq!(background.command, "cargo check --tests");
        assert!(!background.pending);
        assert_eq!(background.exit_code, Some(0));
        assert_eq!(
            background.evidence_hash.as_deref(),
            Some("sha256:background")
        );
        assert_eq!(
            background.full_output_artifact.as_deref(),
            Some("artifact://background")
        );
    }
}
