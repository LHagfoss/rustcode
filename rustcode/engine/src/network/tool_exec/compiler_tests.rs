use super::*;

#[test]
fn standalone_compiler_source_diagnostics_keep_the_edit_successful() {
    let mut result = crate::tools::ToolExecutionOutput::success("wrote lib.rs".to_owned());

    super::append_standalone_compiler_result(
        &mut result,
        "error: this file contains an unclosed delimiter",
    );

    assert!(result.success);
    assert_eq!(result.error_kind, None);
    assert!(!result.retryable);
    assert!(result.content.contains("Compiler errors/warnings:"));
    assert!(result.content.contains("unclosed delimiter"));
}

#[test]
fn standalone_compiler_infrastructure_keeps_edit_success_and_unverified_notice() {
    let mut result = crate::tools::ToolExecutionOutput::success("wrote lib.rs".to_owned());

    super::append_standalone_compiler_result(
        &mut result,
        "__BUILD_UNVERIFIED__: `cargo check` failed to start",
    );

    assert!(result.success);
    assert_eq!(result.error_kind, None);
    assert!(!result.retryable);
    assert!(result.content.contains("__BUILD_UNVERIFIED__:"));
}

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

async fn build_state(project: &tempfile::TempDir) -> Arc<Mutex<AppState>> {
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut app = state.lock().await;
        app.workspace_root = Some(project.path().to_path_buf());
        app.agent_mode = crate::config::AgentMode::Build;
        app.auto_confirm = true;
        app.config.sandbox_mode = crate::config::SandboxMode::ReadOnly;
    }
    state
}

async fn run_batch(
    state: &Arc<Mutex<AppState>>,
    calls: &[crate::tools::ToolCall],
    dirty: &mut bool,
    cache: &mut Option<(std::path::PathBuf, Option<String>)>,
) -> Vec<ToolResult> {
    let mut user_wait = std::time::Duration::ZERO;
    execute_tool_batch(
        &reqwest::Client::new(),
        state,
        &tokio_util::sync::CancellationToken::new(),
        calls,
        true,
        &None,
        dirty,
        cache,
        &mut user_wait,
        None,
    )
    .await
}

fn write_call(project: &tempfile::TempDir, file: &str, content: &str) -> crate::tools::ToolCall {
    crate::tools::ToolCall {
        name: "write_to_file".to_string(),
        arguments: serde_json::json!({
            "path": project.path().join(file),
            "content": content,
            "overwrite": true,
        }),
        call_id: None,
    }
}

#[tokio::test]
async fn three_writes_in_one_batch_are_checked_once_after_the_last() {
    if !crate::tools::exec::sandbox::runtime_tests_available() {
        return;
    }
    // The crate only compiles once all three files exist: a check after the
    // first or second write would report the missing modules (#1887).
    let project = compiler_project();
    let state = build_state(&project).await;
    let calls = [
        write_call(&project, "lib.rs", "mod a;\nmod b;\n"),
        write_call(&project, "a.rs", "pub fn a() {}\n"),
        write_call(&project, "b.rs", "pub fn b() {}\n"),
    ];
    let (mut dirty, mut cache) = (false, None);
    let results = run_batch(&state, &calls, &mut dirty, &mut cache).await;

    assert_eq!(results.len(), 3);
    for result in &results {
        assert!(result.metadata.success, "{}", result.content);
        assert_eq!(result.metadata.error_kind, None, "{}", result.content);
        assert!(
            !result.content.contains("LSP/Compiler errors detected"),
            "{}",
            result.content
        );
    }
    assert!(!dirty);
    assert_eq!(cache, Some((project.path().canonicalize().unwrap(), None)));

    // A batch that ends broken reports the diagnostics once, next to the last
    // write, and every write still counts as the success it was.
    let calls = [
        write_call(&project, "a.rs", "pub fn a() {}\n// touched\n"),
        write_call(&project, "b.rs", "pub fn b() {}\n// touched\n"),
        write_call(&project, "lib.rs", "pub fn broken( {"),
    ];
    let results = run_batch(&state, &calls, &mut dirty, &mut cache).await;

    assert_eq!(results.len(), 3);
    let reports = |result: &ToolResult| {
        result
            .content
            .matches("LSP/Compiler errors detected")
            .count()
    };
    assert_eq!(reports(&results[0]), 0, "{}", results[0].content);
    assert_eq!(reports(&results[1]), 0, "{}", results[1].content);
    assert_eq!(reports(&results[2]), 1, "{}", results[2].content);
    assert!(results[2].content.contains("unclosed delimiter"));
    for result in &results {
        assert!(result.metadata.success, "{}", result.content);
        assert_eq!(result.metadata.error_kind, None, "{}", result.content);
        assert!(!result.metadata.retryable);
    }
    assert!(dirty);
}

#[tokio::test]
async fn a_two_chunk_write_is_checked_only_after_the_last_chunk() {
    if !crate::tools::exec::sandbox::runtime_tests_available() {
        return;
    }
    let project = compiler_project();
    let state = build_state(&project).await;
    let path = project.path().join("lib.rs");
    let first = "pub fn chunked() -> u8 {\n";
    let chunk = |arguments: serde_json::Value| crate::tools::ToolCall {
        name: "write_file_chunk".to_string(),
        arguments,
        call_id: None,
    };
    let (mut dirty, mut cache) = (false, None);

    // The first chunk leaves an unclosed delimiter by construction.
    let results = run_batch(
        &state,
        &[chunk(serde_json::json!({
            "path": path, "content": first, "offset": 0, "truncate": true, "more": true,
        }))],
        &mut dirty,
        &mut cache,
    )
    .await;
    assert!(results[0].metadata.success, "{}", results[0].content);
    assert_eq!(results[0].metadata.error_kind, None);
    assert!(
        !results[0].content.contains("LSP/Compiler errors detected"),
        "{}",
        results[0].content
    );
    assert!(dirty, "the skipped check must still be owed");
    assert_eq!(cache, None);

    let results = run_batch(
        &state,
        &[chunk(serde_json::json!({
            "path": path, "content": "    1\n}\n", "offset": first.len(),
        }))],
        &mut dirty,
        &mut cache,
    )
    .await;
    assert!(results[0].metadata.success, "{}", results[0].content);
    assert_eq!(results[0].metadata.error_kind, None);
    assert!(
        !results[0].content.contains("LSP/Compiler errors detected"),
        "{}",
        results[0].content
    );
    assert!(!dirty, "the last chunk runs the check");
    assert_eq!(cache, Some((project.path().canonicalize().unwrap(), None)));
}

#[test]
fn only_a_chunk_marked_more_holds_back_the_compiler_check() {
    let chunk = |path: &str, more: Option<bool>| {
        let mut arguments = serde_json::json!({"path": path, "content": "x"});
        if let Some(more) = more {
            arguments["more"] = more.into();
        }
        crate::tools::ToolCall {
            name: "write_file_chunk".to_string(),
            arguments,
            call_id: None,
        }
    };
    assert!(super::leaves_chunked_file_incomplete(&[chunk(
        "a.rs",
        Some(true)
    )]));
    assert!(!super::leaves_chunked_file_incomplete(&[chunk(
        "a.rs", None
    )]));
    assert!(!super::leaves_chunked_file_incomplete(&[
        chunk("a.rs", Some(true)),
        chunk("a.rs", Some(false)),
    ]));
    // Another file finishing does not complete the one still being written.
    assert!(super::leaves_chunked_file_incomplete(&[
        chunk("a.rs", Some(true)),
        chunk("b.rs", None),
    ]));
}

#[tokio::test]
async fn standalone_edit_preserves_compiler_check() {
    let project = compiler_project();
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(project.path().to_path_buf());
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
        result.content.matches("Compiler errors/warnings:").count(),
        1
    );
    // Diagnostics or an explicit unverified notice, never a failure status.
    assert_eq!(result.error_kind, None, "{}", result.content);
    assert!(!result.retryable);
}
