//! Cross-tool detection for failures caused by an unavailable dependency.
//!
//! Tool names and command strings are deliberately not part of the primary
//! identity. An MCP call followed by a shell diagnostic should still count as
//! the same outage when both identify the same service or transport failure.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfrastructureFailure {
    pub fingerprint: String,
    pub dependency: String,
    pub class: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InfrastructureFailureDecision {
    NotInfrastructure,
    Cleared,
    Allowed {
        failure: InfrastructureFailure,
        streak: usize,
    },
    Stop {
        failure: InfrastructureFailure,
        streak: usize,
    },
}

#[derive(Debug, Default)]
pub struct InfrastructureFailureTracker {
    last_fingerprint: Option<String>,
    streak: usize,
}

impl InfrastructureFailureTracker {
    /// Permit a few distinct diagnostics before forcing an actionable stop.
    pub const MAX_STREAK: usize = 4;

    pub fn observe(
        &mut self,
        tool_name: &str,
        error_kind: Option<&str>,
        retryable: bool,
        success: bool,
        content: &str,
    ) -> InfrastructureFailureDecision {
        if success {
            self.reset();
            return InfrastructureFailureDecision::Cleared;
        }

        let Some(failure) = classify(tool_name, error_kind, retryable, content) else {
            self.reset();
            return InfrastructureFailureDecision::NotInfrastructure;
        };
        if self.last_fingerprint.as_deref() == Some(failure.fingerprint.as_str()) {
            self.streak = self.streak.saturating_add(1);
        } else {
            self.last_fingerprint = Some(failure.fingerprint.clone());
            self.streak = 1;
        }
        let streak = self.streak;
        if streak >= Self::MAX_STREAK {
            InfrastructureFailureDecision::Stop { failure, streak }
        } else {
            InfrastructureFailureDecision::Allowed { failure, streak }
        }
    }

    pub fn reset(&mut self) {
        self.last_fingerprint = None;
        self.streak = 0;
    }

    pub fn streak(&self) -> usize {
        self.streak
    }
}

fn classify(
    tool_name: &str,
    error_kind: Option<&str>,
    retryable: bool,
    content: &str,
) -> Option<InfrastructureFailure> {
    let kind = error_kind.unwrap_or_default().to_ascii_lowercase();
    let lower = content.to_ascii_lowercase();
    let provider_failure = kind == "providerfailed";
    let mcp_failure = kind == "mcpfailed";
    let dependency = if lower.contains("qdrant") {
        "qdrant"
    } else if lower.contains("socraticode") || lower.contains("socrati") {
        "socraticode"
    } else if mcp_failure {
        "mcp"
    } else if provider_failure {
        "provider"
    } else {
        "unknown"
    };
    let class = if lower.contains("qdrant")
        || lower.contains("service unavailable")
        || lower.contains("internal server error")
        || lower.contains("bad gateway")
        || lower.contains("gateway timeout")
        || provider_failure
    {
        "service"
    } else if lower.contains("channel closed")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("fetch failed")
        || lower.contains("transport")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("eof")
        || mcp_failure
    {
        "transport"
    } else {
        "dependency"
    };

    let explicit_marker = [
        "channel closed",
        "connection refused",
        "connection reset",
        "fetch failed",
        "service unavailable",
        "internal server error",
        "bad gateway",
        "gateway timeout",
        "transport",
        "timed out",
        "timeout",
        "qdrant",
        "unavailable",
        "providerfailed",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let typed_infrastructure = provider_failure || (mcp_failure && retryable);
    let retryable_command = retryable
        && matches!(kind.as_str(), "commandfailed" | "unavailabledependency")
        && explicit_marker;
    if !typed_infrastructure && !retryable_command {
        return None;
    }

    // Named services share an outage across tools: an MCP call followed by
    // a shell diagnostic for the same service is one streak. Anonymous
    // transport failures carry no service name, so scope them per tool —
    // otherwise two unrelated flapping MCP servers merge into one streak and
    // stop the turn prematurely.
    let fingerprint = if dependency == "mcp" && class == "transport" {
        format!("mcp:transport:{}", tool_name.to_ascii_lowercase())
    } else {
        format!("{dependency}:{class}")
    };
    Some(InfrastructureFailure {
        fingerprint,
        dependency: dependency.to_string(),
        class: class.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{InfrastructureFailureDecision, InfrastructureFailureTracker};

    fn stop_after_four(
        tracker: &mut InfrastructureFailureTracker,
        tool: &str,
        content: &str,
    ) -> InfrastructureFailureDecision {
        let mut decision = InfrastructureFailureDecision::NotInfrastructure;
        for _ in 0..4 {
            decision = tracker.observe(tool, Some("McpFailed"), true, false, content);
        }
        decision
    }

    #[test]
    fn changing_mcp_tools_keeps_the_same_dependency_streak() {
        let mut tracker = InfrastructureFailureTracker::default();
        tracker.observe(
            "codebase_status",
            Some("McpFailed"),
            true,
            false,
            "error: MCP channel closed while reaching SocratiCode",
        );
        tracker.observe(
            "codebase_graph",
            Some("McpFailed"),
            true,
            false,
            "error: fetch failed for SocratiCode codebase graph",
        );
        assert_eq!(tracker.streak(), 2);
    }

    #[test]
    fn anonymous_transport_failures_do_not_merge_across_tools() {
        let mut tracker = InfrastructureFailureTracker::default();
        tracker.observe(
            "server_a_tool",
            Some("McpFailed"),
            true,
            false,
            "error: MCP channel closed",
        );
        tracker.observe(
            "server_a_tool",
            Some("McpFailed"),
            true,
            false,
            "error: MCP channel closed",
        );
        // A different tool with an equally anonymous transport error is a
        // different scope, not streak 3 of the same outage.
        tracker.observe(
            "server_b_tool",
            Some("McpFailed"),
            true,
            false,
            "error: MCP channel closed",
        );
        assert_eq!(tracker.streak(), 1);
    }

    #[test]
    fn qdrant_failure_can_stop_after_bounded_diagnostics() {
        let mut tracker = InfrastructureFailureTracker::default();
        let decision = stop_after_four(
            &mut tracker,
            "run_command",
            "Qdrant service unavailable: connection refused",
        );
        assert!(matches!(
            decision,
            InfrastructureFailureDecision::Stop { .. }
        ));
    }

    #[test]
    fn successful_changed_retry_clears_the_streak() {
        let mut tracker = InfrastructureFailureTracker::default();
        tracker.observe(
            "codebase_status",
            Some("McpFailed"),
            true,
            false,
            "MCP transport error: channel closed",
        );
        assert!(matches!(
            tracker.observe(
                "run_command",
                Some("CommandFailed"),
                false,
                true,
                "new diagnostic evidence",
            ),
            InfrastructureFailureDecision::Cleared
        ));
        assert_eq!(tracker.streak(), 0);
    }

    #[test]
    fn provider_5xx_is_classified_without_parsing_tool_names() {
        let mut tracker = InfrastructureFailureTracker::default();
        let decision = tracker.observe(
            "any_tool",
            Some("ProviderFailed"),
            true,
            false,
            "upstream returned status 503",
        );
        assert!(matches!(
            decision,
            InfrastructureFailureDecision::Allowed { .. }
        ));
    }

    #[test]
    fn ordinary_command_failure_is_not_an_infrastructure_outage() {
        let mut tracker = InfrastructureFailureTracker::default();
        assert!(matches!(
            tracker.observe(
                "run_command",
                Some("CommandFailed"),
                true,
                false,
                "error: cargo test failed with exit code 1",
            ),
            InfrastructureFailureDecision::NotInfrastructure
        ));
    }
}
