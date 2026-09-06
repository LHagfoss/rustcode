use super::*;

fn compiler_project() -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"compiler_check_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"lib.rs\"\n[workspace]\n",
    )
    .unwrap();
    project
}

#[test]
fn unverified_build_notice_preserves_successful_edit_metadata() {
    let mut result = tool_result_from_execution(
        "write_to_file",
        &serde_json::json!({"path": "lib.rs"}),
        crate::tools::ToolExecutionOutput::success("wrote lib.rs".to_string()),
        None,
    );
    let notice = "__BUILD_UNVERIFIED__: `cargo check` timed out. The build was NOT verified.";
    append_compiler_diagnostics(&mut result, notice);
    assert!(result.content.ends_with(notice));
    assert!(result.metadata.success);
    assert_eq!(result.metadata.error_kind, None);
    assert!(!result.metadata.retryable);
    assert_eq!(
        super::super::compiler::compiler_diagnostic_fingerprint(&result.content),
        None
    );
}

#[tokio::test]
async fn batch_compiler_diagnostics_are_once_per_edit_and_refresh_after_fix() {
    let project = compiler_project();
    let unrelated_project = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.agent_mode = crate::config::AgentMode::Build;
    let calls =
        ["pub fn broken( {", "pub fn repaired() {}"].map(|content| crate::tools::ToolCall {
            name: "write_to_file".to_string(),
            arguments: serde_json::json!({
                "path": project.path().join("lib.rs"),
                "content": content,
                "overwrite": true,
            }),
            call_id: None,
        });
    let mut dirty = false;
    let mut cache = None;
    let mut user_wait = std::time::Duration::ZERO;
    let results = execute_tool_batch(
        &reqwest::Client::new(),
        &state,
        &tokio_util::sync::CancellationToken::new(),
        &calls,
        true,
        &Some(unrelated_project.path().to_path_buf()),
        &mut dirty,
        &mut cache,
        &mut user_wait,
        None,
    )
    .await;

    assert_eq!(results.len(), 2);
    let broken = &results[0];
    assert!(broken.metadata.success, "{}", broken.content);
    assert_eq!(
        broken.metadata.error_kind,
        Some(crate::tools::ToolErrorKind::CompilerFailed),
        "{}",
        broken.content,
    );
    assert!(broken.metadata.retryable);
    assert_eq!(
        broken
            .content
            .matches("LSP/Compiler errors detected")
            .count(),
        1
    );
    assert!(!broken.content.contains("Compiler errors/warnings:"));
    assert_eq!(
        broken
            .content
            .matches("error: this file contains an unclosed delimiter")
            .count(),
        1
    );
    let repaired = &results[1];
    assert!(repaired.metadata.success, "{}", repaired.content);
    assert_eq!(repaired.metadata.error_kind, None, "{}", repaired.content);
    assert!(!repaired.content.contains("LSP/Compiler errors detected"));
    assert!(!dirty);
    assert_eq!(cache, Some((project.path().canonicalize().unwrap(), None)));
}

#[tokio::test]
async fn standalone_edit_preserves_compiler_check() {
    let project = compiler_project();
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.agent_mode = crate::config::AgentMode::Build;
    let (result, _, _) = confirm_and_execute(
        &reqwest::Client::new(),
        &state,
        &tokio_util::sync::CancellationToken::new(),
        "write_to_file",
        &serde_json::json!({
            "path": project.path().join("lib.rs"),
            "content": "pub fn broken( {",
            "overwrite": true,
        }),
        "write_to_file",
        true,
        None,
        None,
    )
    .await;
    assert!(result.success, "{}", result.content);
    assert_eq!(
        result.error_kind,
        Some(crate::tools::ToolErrorKind::CompilerFailed)
    );
    assert!(result.retryable);
    assert_eq!(
        result.content.matches("Compiler errors/warnings:").count(),
        1
    );
}
