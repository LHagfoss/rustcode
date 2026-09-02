use super::compact::SUMMARY_MARKER;
use crate::app::ChatMessage;

pub const STRUCTURED_MEMORY_MARKER: &str = "[Deterministic context record]";

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StructuredSessionMemory {
    pub initial_goal: String,
    pub current_task: Option<String>,
    pub user_constraints: Vec<String>,
    pub key_architecture: Vec<String>,
    pub inspected_files: Vec<String>,
    pub modified_files: Vec<String>,
    pub decisions: Vec<String>,
    pub failures_and_errors: Vec<String>,
    pub verification_state: Vec<String>,
}

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

        for message in history {
            if message.role == "system" {
                if message.content.starts_with(STRUCTURED_MEMORY_MARKER)
                    || message.content.starts_with(SUMMARY_MARKER)
                    || message.content.starts_with("[Structured Session Memory]")
                    || message
                        .content
                        .starts_with("[Deterministic context record]")
                {
                    memory.merge_from_text(&message.content);
                } else if !message.content.starts_with('[') {
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
                    if !task.is_empty() && task != memory.initial_goal {
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
                if !result.success || result.error_kind.is_some() {
                    let err = format!(
                        "{} ({}, exit={:?})",
                        result.tool_name,
                        result.error_kind.as_deref().unwrap_or("failed"),
                        result.exit_code
                    );
                    if !memory.failures_and_errors.contains(&err) {
                        memory.failures_and_errors.push(err);
                    }
                }
                if result.tool_name == "run_command" {
                    let v = format!(
                        "command ({}, exit={:?})",
                        if result.success { "success" } else { "failed" },
                        result.exit_code
                    );
                    if !memory.verification_state.contains(&v) {
                        memory.verification_state.push(v);
                    }
                }
            }

            if message.role == "tool" {
                if let Some((name, body)) = message.content.split_once(": ") {
                    if matches!(name, "view_file" | "read_file") {
                        if let Some(first_line) = body.lines().next() {
                            if let Some(path) = first_line
                                .strip_prefix("[File: ")
                                .and_then(|s| s.split(']').next())
                            {
                                if !memory.inspected_files.iter().any(|f| f == path) {
                                    memory.inspected_files.push(path.to_string());
                                }
                            }
                        }
                    }
                    if body.contains("error:")
                        || body.contains("exit code: 1")
                        || body.contains("FAILED")
                    {
                        let snippet = compact_context_line(body, 200);
                        if !snippet.is_empty() && !memory.failures_and_errors.contains(&snippet) {
                            memory.failures_and_errors.push(snippet);
                        }
                    }
                }
            }

            if message.role == "assistant" {
                for call in &message.tool_calls {
                    if call.name == "run_command"
                        && let Ok(arguments) =
                            serde_json::from_str::<serde_json::Value>(&call.arguments)
                        && let Some(command) =
                            arguments.get("command").and_then(|value| value.as_str())
                    {
                        let v = compact_context_line(command, 240);
                        if !v.is_empty() && !memory.verification_state.contains(&v) {
                            memory.verification_state.push(v);
                        }
                    }
                }
                let prose = crate::network::text::strip_think_blocks(&message.content);
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

        memory
    }

    pub fn merge_from_text(&mut self, text: &str) {
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(goal) = trimmed.strip_prefix("Goal: ") {
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
            } else if let Some(files) = trimmed.strip_prefix("Inspected files: ") {
                for f in files.split(", ") {
                    if !f.is_empty() && !self.inspected_files.iter().any(|existing| existing == f) {
                        self.inspected_files.push(f.to_string());
                    }
                }
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
            } else if let Some(verifications) = trimmed.strip_prefix("Verification state: ") {
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
            } else if let Some(decisions) =
                trimmed.strip_prefix("Architecture/decisions/next steps: ")
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
                "Inspected files: {}\n",
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
                "Verification state: {}\n",
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
            out.push_str("Architecture/decisions/next steps: ");
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
        out.chars().take(max_chars).collect()
    }
}

pub fn compact_with_structured_memory(
    history: &mut Vec<ChatMessage>,
    keep_recent_count: usize,
    budget: usize,
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
    let memory = StructuredSessionMemory::extract_from_history(&history[..cutoff]);
    let max_chars = budget.saturating_mul(3).clamp(1000, 8000);
    let record = memory.format_record(max_chars);

    let tail = history[cutoff..].to_vec();
    let summary_message = super::compact::durable_compaction_record_message(&record, &tail);
    history.clear();
    history.push(summary_message);
    history.extend(tail);
    true
}
