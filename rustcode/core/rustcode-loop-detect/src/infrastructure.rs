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

/// Fingerprint for harness workspace-boundary rejections (`path ... escapes
/// the workspace root`). These are unreachable paths, not model stagnation:
/// they belong to the infrastructure guard (reset + hint) rather than the
/// evidence-recovery streak.
pub const WORKSPACE_BOUNDARY_FINGERPRINT: &str = "workspace:boundary";

/// Whether tool output reports a workspace-boundary rejection. Matched on the
/// harness message text so it holds regardless of the error kind the tool
/// layer attached (validation, permission, sandbox denial).
pub fn is_workspace_boundary_failure(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("escapes the workspace root")
        || lower.contains("escapes the project workspace")
        || lower.contains("outside the task working directory")
}

/// Whether tool output reports the harness killing a local command at its
/// timeout (`command timed out after N ms and was killed`). That is a command
/// that ran too long on this machine (for example a traversal stalled on a
/// protected folder), not an unreachable service or a broken transport, so it
/// never counts toward an infrastructure outage (#2022).
pub fn is_command_timeout(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("command timed out after") && lower.contains("and was killed")
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
    // Workspace-boundary rejections are deterministic harness guardrails, not
    // provider/MCP outages, so they classify as infrastructure unconditionally:
    // no retryable flag or error-kind cooperation is required.
    if is_workspace_boundary_failure(content) {
        return Some(InfrastructureFailure {
            fingerprint: WORKSPACE_BOUNDARY_FINGERPRINT.to_string(),
            dependency: "workspace".to_string(),
            class: "boundary".to_string(),
        });
    }
    if is_command_timeout(content) {
        return None;
    }
    let kind = error_kind.unwrap_or_default().to_ascii_lowercase();
    let lower = content.to_ascii_lowercase();
    let provider_failure = kind == "providerfailed";
    let mcp_failure = kind == "mcpfailed";
    // An MCP server that relays an upstream 4xx (`HTTP 404: No active time
    // entry`) reached its service and got an answer: that is the tool's
    // result, not a transport or service outage (#1970).
    if mcp_failure && reports_upstream_client_error(&lower) {
        return None;
    }
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

/// Whether failure text carries an upstream HTTP 4xx status, e.g.
/// `returned HTTP 404: ...` or `status 403`. Request timeout (408) and rate
/// limiting (429) are transient service conditions and do not count.
fn reports_upstream_client_error(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    [
        "http ",
        "http/1.1 ",
        "http/2 ",
        "status ",
        "status code ",
        "status: ",
    ]
    .iter()
    .any(|prefix| {
        lower.match_indices(prefix).any(|(index, _)| {
            let rest = &lower[index + prefix.len()..];
            let digits = rest.as_bytes();
            digits.len() >= 3
                && digits[0] == b'4'
                && digits[1].is_ascii_digit()
                && digits[2].is_ascii_digit()
                && !digits.get(3).is_some_and(u8::is_ascii_digit)
                && !matches!(&rest[..3], "408" | "429")
        })
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
    fn workspace_boundary_rejection_is_infrastructure_regardless_of_error_kind() {
        use super::{WORKSPACE_BOUNDARY_FINGERPRINT, is_workspace_boundary_failure};
        let content = "path '/etc/passwd' escapes the workspace root '/work/proj'";
        assert!(is_workspace_boundary_failure(content));
        for kind in [None, Some("Validation"), Some("PermissionDenied")] {
            let mut tracker = InfrastructureFailureTracker::default();
            let decision = tracker.observe("read_file", kind, false, false, content);
            match decision {
                InfrastructureFailureDecision::Allowed { failure, streak } => {
                    assert_eq!(failure.fingerprint, WORKSPACE_BOUNDARY_FINGERPRINT);
                    assert_eq!(failure.dependency, "workspace");
                    assert_eq!(streak, 1);
                }
                other => panic!("expected Allowed, got {other:?}"),
            }
        }
    }

    #[test]
    fn command_timeout_is_not_a_transport_outage() {
        // #2022: a run_command killed at its timeout arrives as a retryable
        // CommandFailed whose text contains "timed out"; it must not be
        // recorded as `unknown:transport`.
        let content = "error: command timed out after 120000 ms and was killed";
        assert!(super::is_command_timeout(content));
        let mut tracker = InfrastructureFailureTracker::default();
        for _ in 0..InfrastructureFailureTracker::MAX_STREAK {
            assert_eq!(
                tracker.observe("run_command", Some("CommandFailed"), true, false, content),
                InfrastructureFailureDecision::NotInfrastructure
            );
        }
        assert_eq!(tracker.streak(), 0);
    }

    #[test]
    fn a_service_timeout_reported_by_a_command_still_counts() {
        let mut tracker = InfrastructureFailureTracker::default();
        assert!(matches!(
            tracker.observe(
                "run_command",
                Some("CommandFailed"),
                true,
                false,
                "curl: (28) Connection timed out after 10001 milliseconds",
            ),
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

    // Session 01a130ea (#1970): the server answered "no active timer" with a
    // 404, which was counted as an MCP transport failure.
    #[test]
    fn mcp_upstream_4xx_is_a_tool_answer_not_an_outage() {
        let mut tracker = InfrastructureFailureTracker::default();
        let content = "error: MCP tool call failed: Solidtime /api/v1/users/me/time-entries/active returned HTTP 404: {\"message\": \"No active time entry\"}";
        assert!(matches!(
            tracker.observe(
                "solidtime_get_active_timer",
                Some("McpFailed"),
                true,
                false,
                content
            ),
            InfrastructureFailureDecision::NotInfrastructure
        ));
        assert_eq!(tracker.streak(), 0);
    }

    #[test]
    fn upstream_client_error_detection_skips_transient_and_server_statuses() {
        use super::reports_upstream_client_error;
        assert!(reports_upstream_client_error(
            "returned HTTP 404: not found"
        ));
        assert!(reports_upstream_client_error("upstream status code 422"));
        assert!(reports_upstream_client_error("Status: 401 Unauthorized"));
        assert!(!reports_upstream_client_error(
            "returned HTTP 429: slow down"
        ));
        assert!(!reports_upstream_client_error("returned HTTP 408"));
        assert!(!reports_upstream_client_error("returned HTTP 503"));
        assert!(!reports_upstream_client_error("status 4000 items"));
        assert!(!reports_upstream_client_error("MCP channel closed"));
    }

    #[test]
    fn mcp_upstream_5xx_still_counts_as_infrastructure() {
        let mut tracker = InfrastructureFailureTracker::default();
        assert!(matches!(
            tracker.observe(
                "solidtime_get_active_timer",
                Some("McpFailed"),
                true,
                false,
                "error: MCP tool call failed: returned HTTP 503: service unavailable",
            ),
            InfrastructureFailureDecision::Allowed { .. }
        ));
    }
}
