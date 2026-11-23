use super::*;
use crate::config::{AgentMode, ModelProfile, ToolProtocol};

#[test]
fn mutation_guidance_matches_effective_policy_for_every_protocol() {
    for protocol in [
        ToolProtocol::ApiNative,
        ToolProtocol::Json,
        ToolProtocol::Native,
    ] {
        for policy in [
            ToolSchemaPolicy::root_for_mode(false, AgentMode::Build),
            ToolSchemaPolicy::subagent(),
        ] {
            let base = tool_system_prompt_for_policy(policy, protocol, AgentMode::Build);
            // Reuse the same base as the root prompt cache does, including
            // switching back to the default after a profile override.
            for configured in [None, Some(3), Some(0), Some(usize::MAX), None] {
                let profile = ModelProfile {
                    max_mutating_calls_per_response: configured,
                    ..ModelProfile::default()
                };
                let limit = profile.max_mutating_calls_per_response();
                let mut prompt = base.clone();
                append_tool_response_limit(&mut prompt, limit);
                assert!(prompt.contains(&format!(
                    "effective max_mutating_calls_per_response is {limit}."
                )));
                assert_eq!(prompt.matches("# Tool response limit").count(), 1);
                assert!(prompt.contains("Read-only inspection never consumes this limit"));
                assert!(prompt.contains("Never assume an unexecuted call ran"));

                // The shell guidance must agree with the actual executor:
                // mutating commands consume the mutation allowance while
                // read-only inspection does not.
                let calls = (0..=limit)
                    .map(|_| crate::tools::ToolCall {
                        name: "run_command".into(),
                        arguments: serde_json::json!({"command": "cargo test"}),
                        call_id: None,
                    })
                    .collect();
                let (kept, dropped) = crate::tools::partition_tool_batch(calls, limit);
                assert_eq!(kept.len(), limit);
                assert_eq!(dropped.len(), 1);
            }
        }
    }
}

#[test]
fn native_protocol_batches_reads_and_limits_mutations() {
    let prompt = tool_system_prompt(false, ToolProtocol::Native, AgentMode::Build);

    assert!(prompt.contains("multiple [TOOL_CALLS] markers"));
    assert!(prompt.contains("mutations run in order"));
    assert!(!prompt.contains("exactly one [TOOL_CALLS] marker"));
}
