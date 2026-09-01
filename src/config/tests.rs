use super::*;
use crate::app::ChatMessage;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("rustcode-tests").join(format!(
        "{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn session_id_allocator_advances_when_clock_repeats() {
    assert_eq!(next_session_id_value(1_000, 0), 1_000);
    assert_eq!(next_session_id_value(1_000, 1_000), 1_001);
    assert_eq!(next_session_id_value(999, 1_001), 1_002);
}

#[test]
fn test_config_directory_is_unique_to_the_test_thread() {
    let dir = get_config_dir().expect("test config directory");
    assert!(
        dir.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rustcode_test_config_")),
        "unexpected test config directory: {}",
        dir.display()
    );
}

#[test]
fn test_config_save_load() {
    let dir = temp_dir("config");
    let config = AppConfig {
        default: DefaultConfig::Simple("gemma4:e2b-it-qat".to_string()),
        ..AppConfig::default()
    };
    save_config_to(&dir, &config);

    let (url, model, loaded) = load_config_from(&dir);
    assert_eq!(loaded.default.big(), "gemma4:e2b-it-qat");
    let expected = &loaded
        .models
        .iter()
        .find(|m| m.name == "gemma4:e2b-it-qat")
        .unwrap();
    assert_eq!(url, expected.url);
    assert_eq!(model, expected.model);
}

#[test]
fn test_default_profile_is_source_of_truth() {
    let dir = temp_dir("latest");
    let config = AppConfig {
        default: DefaultConfig::Simple("gemma4:e2b-it-qat".to_string()),
        ..AppConfig::default()
    };
    save_config_to(&dir, &config);

    let (url, model, _) = load_config_from(&dir);
    let expected = &config
        .models
        .iter()
        .find(|m| m.name == "gemma4:e2b-it-qat")
        .unwrap();
    assert_eq!(url, expected.url);
    assert_eq!(model, expected.model);
}

#[test]
fn test_context_window_optional() {
    let dir = temp_dir("ctxwin");
    let mut config = AppConfig::default();
    config.models[0].context_window = Some(4096);
    save_config_to(&dir, &config);
    let (_, _, loaded) = load_config_from(&dir);
    assert_eq!(
        loaded
            .models
            .iter()
            .find(|m| m.name == "qwen3.6-dense")
            .unwrap()
            .context_window,
        Some(4096)
    );
}

#[test]
fn model_sampling_controls_round_trip_through_toml() {
    let dir = temp_dir("sampling");
    let mut config = AppConfig::default();
    let profile = &mut config.models[0];
    profile.temperature = Some(1.0);
    profile.top_p = Some(0.95);
    profile.top_k = Some(20);
    profile.presence_penalty = Some(1.5);
    profile.frequency_penalty = Some(0.0);
    profile.force_sampling = Some(true);
    profile.preserve_thinking = Some(true);

    save_config_to(&dir, &config);
    let (_, _, loaded) = load_config_from(&dir);
    let profile = &loaded.models[0];

    assert_eq!(profile.temperature, Some(1.0));
    assert_eq!(profile.top_p, Some(0.95));
    assert_eq!(profile.top_k, Some(20));
    assert_eq!(profile.presence_penalty, Some(1.5));
    assert_eq!(profile.frequency_penalty, Some(0.0));
    assert_eq!(profile.force_sampling, Some(true));
    assert_eq!(profile.preserve_thinking, Some(true));
}

#[test]
fn context_budget_reserves_completion_thinking_tools_and_safety() {
    let mut profile = AppConfig::default().models[0].clone();
    profile.context_window = Some(4096);
    profile.max_tokens = Some(2048);
    profile.enable_thinking = Some(true);
    let budget = profile.context_budget();
    assert_eq!(budget.context_window, 4096);
    assert!(budget.completion_reserve > 0);
    assert!(budget.thinking_reserve > 0);
    assert!(budget.tool_reserve > 0);
    assert!(budget.history_tokens < budget.context_window);
    assert_eq!(
        budget.history_tokens
            + budget.completion_reserve
            + budget.thinking_reserve
            + budget.tool_reserve
            + budget.safety_reserve,
        budget.context_window
    );

    profile.context_window = Some(512);
    let tiny = profile.context_budget();
    assert_eq!(
        tiny.history_tokens
            + tiny.completion_reserve
            + tiny.thinking_reserve
            + tiny.tool_reserve
            + tiny.safety_reserve,
        tiny.context_window
    );
}

#[test]
fn local_default_completion_cap_is_4096_and_explicit_max_tokens_is_preserved() {
    let mut profile = ModelProfile {
        name: "local-ollama".to_string(),
        url: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
        model: "qwen2.5:32b".to_string(),
        context_window: Some(128_000),
        engine: Some("ollama".to_string()),
        ..ModelProfile::default()
    };

    assert_eq!(profile.context_budget().completion_reserve, 4096);

    profile.max_tokens = Some(8192);
    assert_eq!(profile.context_budget().completion_reserve, 8192);
}

#[test]
fn every_model_tool_round_cap_preserves_full_final_cap() {
    let profile = ModelProfile {
        context_window: Some(128_000),
        max_tokens: Some(16_000),
        ..ModelProfile::default()
    };

    assert_eq!(profile.completion_token_limit(true), 8192);
    assert_eq!(profile.completion_token_limit(false), 16_000);
}

#[test]
fn context_budget_scales_without_double_reserving_large_or_small_windows() {
    let mut profile = AppConfig::default().models[0].clone();
    profile.max_tokens = Some(u32::MAX);
    for window in [
        1, 32, 64, 128, 256, 512, 4_096, 8_192, 32_768, 128_000, 262_144,
    ] {
        profile.context_window = Some(window);
        profile.enable_thinking = Some(false);
        let budget = profile.context_budget();
        assert_eq!(budget.context_window, window.max(1));
        assert_eq!(
            budget.history_tokens
                + budget.completion_reserve
                + budget.thinking_reserve
                + budget.tool_reserve
                + budget.safety_reserve,
            budget.context_window
        );
        assert!(
            budget.context_window < 4 || budget.completion_reserve <= budget.context_window / 4
        );
        if window > 1 {
            assert!(budget.history_tokens > 0);
        }

        profile.enable_thinking = Some(true);
        let thinking = profile.context_budget();
        if window >= 64 {
            assert!(thinking.thinking_reserve > 0);
            assert!(thinking.history_tokens < budget.history_tokens);
        }
    }
}

#[test]
fn tool_round_limit_round_trips_through_runtime_config() {
    let dir = temp_dir("tool_round_limit");
    let mut config = AppConfig::default();
    config.max_tool_rounds = 17;
    save_config_to(&dir, &config);

    let (_, _, loaded) = load_config_from(&dir);
    assert_eq!(loaded.max_tool_rounds, 17);
}

#[test]
fn older_runtime_config_defaults_subagent_concurrency_limit() {
    let dir = temp_dir("legacy_subagent_concurrency_limit");
    std::fs::write(dir.join(CONFIG_FILE), "{}").unwrap();

    let (_, _, loaded) = load_config_from(&dir);

    assert_eq!(loaded.subagent_concurrency_limit, 4);
}

#[test]
fn subagent_concurrency_limit_round_trips_through_runtime_config() {
    let dir = temp_dir("subagent_concurrency_limit");
    let mut config = AppConfig::default();
    config.subagent_concurrency_limit = 2;
    save_config_to(&dir, &config);

    let (_, _, loaded) = load_config_from(&dir);

    assert_eq!(loaded.subagent_concurrency_limit, 2);
}

#[test]
fn image_input_capability_is_explicit_and_vision_profile_is_configurable() {
    let mut profile = AppConfig::default().models[0].clone();
    assert_eq!(profile.image_input_supported(), Some(false));

    profile.supports_vision = Some(true);
    assert_eq!(profile.image_input_supported(), Some(true));

    let mut config = AppConfig::default();
    config.vision_model = Some("vision-helper".to_string());
    let json = serde_json::to_string(&config).unwrap();
    let decoded: AppConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.vision_model.as_deref(), Some("vision-helper"));
}

#[test]
fn test_history_save_load() {
    let dir = temp_dir("history");
    let msgs = vec![
        ChatMessage::new("user", "Hello"),
        ChatMessage::new("assistant", "Hi there"),
    ];
    write_history_file(&dir.join(HISTORY_FILE), &msgs);
    let loaded = load_session_file(&dir.join(HISTORY_FILE));
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].role, "user");
    assert_eq!(loaded[0].content, "Hello");
    assert_eq!(loaded[1].role, "assistant");
    assert_eq!(loaded[1].content, "Hi there");
}

#[test]
fn test_history_is_written_compactly_and_atomically() {
    let dir = temp_dir("history-compact");
    let msgs = vec![ChatMessage::new("user", "Hello")];
    write_history_file(&dir.join(HISTORY_FILE), &msgs);

    let raw = fs::read_to_string(dir.join(HISTORY_FILE)).unwrap();
    assert!(
        !raw.contains('\n'),
        "history must be compact JSON, got: {raw}"
    );
    assert_eq!(load_session_file(&dir.join(HISTORY_FILE)).len(), 1);

    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != HISTORY_FILE)
        .collect();
    assert!(
        leftovers.is_empty(),
        "atomic write left temp files behind: {leftovers:?}"
    );
}

#[test]
fn test_queued_history_writes_coalesce_and_flush() {
    let dir = temp_dir("history-queue");
    let path = dir.join(HISTORY_FILE);

    for i in 0..5 {
        let msgs: Vec<ChatMessage> = (0..=i)
            .map(|n| ChatMessage::new("user", format!("msg {n}")))
            .collect();
        queue_history_write(path.clone(), &msgs, None);
    }
    flush_history();

    // The flush must persist the newest snapshot, not an earlier one.
    let loaded = load_session_file(&path);
    assert_eq!(loaded.len(), 5);
    assert_eq!(loaded[4].content, "msg 4");
}

#[test]
fn revisioned_history_write_skips_an_identical_pending_snapshot() {
    let dir = temp_dir("history-queue-dedup");
    let path = dir.join(HISTORY_FILE);
    let mut history = crate::app::History::default();
    history.push(ChatMessage::new("user", "same"));
    let revision = history.revision();

    assert!(queue_history_write(
        path.clone(),
        history.as_slice(),
        Some(revision)
    ));
    assert!(!queue_history_write(
        path,
        history.as_slice(),
        Some(revision)
    ));
    flush_history();
}

#[test]
fn test_session_has_content_ignores_commands() {
    let cmds_only = vec![
        ChatMessage::new("user", "/help"),
        ChatMessage::new("system", "help text"),
    ];
    assert!(!session_has_content(&cmds_only));
    let real = vec![ChatMessage::new("user", "fix the bug")];
    assert!(session_has_content(&real));
}

#[test]
fn test_session_title_first_prompt_truncated() {
    let history = vec![
        ChatMessage::new("user", "/model"),
        ChatMessage::new("user", "x".repeat(100)),
    ];
    let title = session_title(&history);
    assert!(title.ends_with("..."));
    assert_eq!(title.chars().count(), 48);
    assert_eq!(session_title(&[]), "(no prompt)");
}

#[test]
fn test_delete_session_file_only_in_sessions_dir() {
    let dir = temp_dir("delete-guard");
    let outside = dir.join("history.json");
    fs::write(&outside, "[]").unwrap();
    delete_session_file(&outside);
    assert!(outside.exists(), "live history file must not be deleted");

    let sessions = dir.join(SESSIONS_DIR);
    fs::create_dir_all(&sessions).unwrap();
    let inside = sessions.join("123.json");
    fs::write(&inside, "[]").unwrap();
    delete_session_file(&inside);
    assert!(!inside.exists());
}

#[test]
fn test_history_persists_full_log() {
    let dir = temp_dir("history-full");
    let msgs: Vec<ChatMessage> = (0..80)
        .map(|i| ChatMessage::new("user", format!("msg {}", i)))
        .collect();
    write_history_file(&dir.join(HISTORY_FILE), &msgs);
    let loaded = load_session_file(&dir.join(HISTORY_FILE));
    assert_eq!(loaded.len(), msgs.len());
    assert_eq!(loaded[0].content, "msg 0");
}

#[test]
fn test_default_config_parsing() {
    // String format
    let toml_str1 = r#"default = "my-big-model""#;
    #[derive(Deserialize)]
    struct TempConfig {
        default: DefaultConfig,
    }
    let parsed1: TempConfig = toml::from_str(toml_str1).unwrap();
    assert_eq!(parsed1.default.big(), "my-big-model");
    assert_eq!(parsed1.default.small(), "my-big-model");

    // Table format
    let toml_str2 = r#"
            [default]
            big_model = "my-big-model"
            small_model = "my-small-model"
        "#;
    let parsed2: TempConfig = toml::from_str(toml_str2).unwrap();
    assert_eq!(parsed2.default.big(), "my-big-model");
    assert_eq!(parsed2.default.small(), "my-small-model");

    // Table format (alternate names)
    let toml_str2_alt = r#"
            [default]
            big = "alt-big"
            small = "alt-small"
        "#;
    let parsed2_alt: TempConfig = toml::from_str(toml_str2_alt).unwrap();
    assert_eq!(parsed2_alt.default.big(), "alt-big");
    assert_eq!(parsed2_alt.default.small(), "alt-small");

    // Double brackets format [[default]]
    let toml_str3 = r#"
            [[default]]
            big_model = "my-big-model"
            small_model = "my-small-model"
        "#;
    let parsed3: TempConfig = toml::from_str(toml_str3).unwrap();
    assert_eq!(parsed3.default.big(), "my-big-model");
    assert_eq!(parsed3.default.small(), "my-small-model");
}

use tempfile::TempDir;

#[test]
fn test_load_valid_config() {
    let dir = TempDir::new().unwrap();
    let models = r#"{
            "default": {"big": "test_model", "small": "test_small"},
            "models": [{"name": "test_model", "url": "http://test/v1/chat/completions", "model": "test"}]
        }"#;
    fs::write(dir.path().join(MODELS_FILE), models).unwrap();

    let (url, model, config) = load_config_from(dir.path());
    assert_eq!(config.default.big(), "test_model");
    assert_eq!(config.models[0].name, "test_model");
    assert_eq!(url, "http://test/v1/chat/completions");
    assert_eq!(model, "test");
}

#[test]
fn test_load_invalid_config_returns_default() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join(CONFIG_FILE);
    fs::write(&config_path, b"invalid json content").unwrap();

    let (_url, _model, config) = load_config_from(dir.path());
    assert_eq!(config.default.big(), AppConfig::default().default.big());
    assert!(!config.is_valid);

    assert_eq!(fs::read(&config_path).unwrap(), b"invalid json content");
    assert!(!dir.path().join("config.json.bak").exists());
}

#[test]
fn test_load_missing_config_returns_default() {
    let dir = TempDir::new().unwrap();
    let (_url, _model, config) = load_config_from(dir.path());
    assert_eq!(config.default.big(), AppConfig::default().default.big());

    assert!(!dir.path().join("models.json").exists());
    assert!(!dir.path().join("config.json").exists());
    assert!(!dir.path().join("config.toml").exists());
}

#[test]
fn test_load_json_configuration_files() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("models.json"),
        r#"{
                "default": {"big": "custom", "small": "custom-small"},
                "models": [{
                    "name": "custom",
                    "url": "http://custom/v1/chat/completions",
                    "model": "custom-model"
                }]
            }"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("config.json"),
        r#"{
                "theme": "nord",
                "tool_protocol": "native"
            }"#,
    )
    .unwrap();

    let (url, model, config) = load_config_from(dir.path());

    assert_eq!(config.default.big(), "custom");
    assert_eq!(config.default.small(), "custom-small");
    assert_eq!(config.models[0].name, "custom");
    assert_eq!(config.theme, "nord");
    assert_eq!(config.tool_protocol, ToolProtocol::Native);
    assert_eq!(url, "http://custom/v1/chat/completions");
    assert_eq!(model, "custom-model");
}

#[test]
fn test_malformed_json_is_preserved() {
    let dir = TempDir::new().unwrap();
    let malformed_models = b"{ malformed models";
    let malformed_runtime = b"{ malformed runtime";
    fs::write(dir.path().join("models.json"), malformed_models).unwrap();
    fs::write(dir.path().join("config.json"), malformed_runtime).unwrap();

    let (_, _, config) = load_config_from(dir.path());

    let defaults = AppConfig::default();
    assert_eq!(config.default.big(), defaults.default.big());
    assert_eq!(config.models, defaults.models);
    assert_eq!(config.theme, defaults.theme);
    assert_eq!(config.tool_protocol, defaults.tool_protocol);
    assert!(!config.is_valid);
    assert_eq!(
        fs::read(dir.path().join("models.json")).unwrap(),
        malformed_models
    );
    assert_eq!(
        fs::read(dir.path().join("config.json")).unwrap(),
        malformed_runtime
    );
    assert!(!dir.path().join("models.json.bak").exists());
    assert!(!dir.path().join("config.json.bak").exists());
}

#[test]
fn test_malformed_models_preserves_valid_runtime_config() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(MODELS_FILE), b"not json").unwrap();
    fs::write(
        dir.path().join(CONFIG_FILE),
        r#"{"theme":"nord","tool_protocol":"native"}"#,
    )
    .unwrap();

    let (_, _, config) = load_config_from(dir.path());

    assert_eq!(config.theme, "nord");
    assert_eq!(config.tool_protocol, ToolProtocol::Native);
    assert_eq!(config.default.big(), AppConfig::default().default.big());
    assert!(!config.is_valid);
}

#[test]
fn test_empty_models_use_default_endpoint() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(MODELS_FILE),
        r#"{"default":{"big":"missing","small":"missing"},"models":[]}"#,
    )
    .unwrap();

    let (url, model, config) = load_config_from(dir.path());
    let defaults = AppConfig::default();
    assert!(config.models.is_empty());
    assert_eq!(url, defaults.models[0].url);
    assert_eq!(model, defaults.models[0].model);
}

#[test]
fn test_config_save_writes_versioned_toml_without_split_json() {
    let dir = TempDir::new().unwrap();
    let mut config = AppConfig::default();
    config.default = DefaultConfig::Simple("custom".to_string());
    config.models[0].name = "custom".to_string();
    config.theme = "nord".to_string();
    config.tool_protocol = ToolProtocol::Native;

    save_config_to(dir.path(), &config);

    let path = dir.path().join(CONFIG_TOML_FILE);
    assert!(path.exists());
    assert!(!dir.path().join(MODELS_FILE).exists());
    assert!(!dir.path().join(CONFIG_FILE).exists());
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("version = 1"));
    let (_, _, loaded) = load_config_from(dir.path());
    assert_eq!(loaded.default.big(), "custom");
    assert_eq!(loaded.theme, "nord");
    assert_eq!(loaded.tool_protocol, ToolProtocol::Native);
}

#[test]
fn test_legacy_json_is_migrated_when_saved() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(MODELS_FILE),
        r#"{
                "default": "legacy-model",
                "models": [{"name":"legacy-model","url":"http://legacy","model":"legacy"}]
            }"#,
    )
    .unwrap();

    let (_, _, config) = load_config_from(dir.path());
    assert_eq!(config.default.big(), "legacy-model");
    save_config_to(dir.path(), &config);

    assert!(dir.path().join(CONFIG_TOML_FILE).exists());
    assert!(dir.path().join(MODELS_FILE).exists());
    let (_, _, migrated) = load_config_from(dir.path());
    assert_eq!(migrated.default.big(), "legacy-model");
}

#[test]
fn test_invalid_toml_is_preserved_and_does_not_fall_back_to_legacy_json() {
    let dir = TempDir::new().unwrap();
    let invalid = b"[models\nnot valid";
    fs::write(dir.path().join(CONFIG_TOML_FILE), invalid).unwrap();
    fs::write(
        dir.path().join(MODELS_FILE),
        r#"{"default":"legacy","models":[]}"#,
    )
    .unwrap();

    let (_, _, config) = load_config_from(dir.path());

    assert_eq!(config.default.big(), AppConfig::default().default.big());
    assert!(!config.is_valid);
    assert_eq!(
        fs::read(dir.path().join(CONFIG_TOML_FILE)).unwrap(),
        invalid
    );
}

#[test]
fn test_unsupported_toml_version_is_rejected() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(CONFIG_TOML_FILE),
        format!("version = {}\n", CONFIG_FORMAT_VERSION + 1),
    )
    .unwrap();

    let (_, _, config) = load_config_from(dir.path());

    assert_eq!(config.default.big(), AppConfig::default().default.big());
    assert!(!config.is_valid);
}

#[test]
fn project_config_overrides_global_defaults_from_near_to_far() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("nested");
    fs::create_dir_all(workspace.join(PROJECT_CONFIG_DIR)).unwrap();
    fs::create_dir_all(root.path().join(PROJECT_CONFIG_DIR)).unwrap();
    fs::write(
        root.path()
            .join(PROJECT_CONFIG_DIR)
            .join(PROJECT_CONFIG_FILE),
        "version = 1\n[default]\nbig = \"parent\"\nsmall = \"parent-small\"\n",
    )
    .unwrap();
    fs::write(
        workspace.join(PROJECT_CONFIG_DIR).join(PROJECT_CONFIG_FILE),
        "version = 1\n[default]\nbig = \"child\"\n",
    )
    .unwrap();

    let (_, _, config) = load_config_for_workspace(&workspace);

    assert_eq!(config.default.big(), "child");
    assert_eq!(config.default.small(), "parent-small");
}

#[test]
fn project_overrides_are_not_persisted_into_global_config() {
    let global = AppConfig::default();
    let mut merged = global.clone();
    let project: TomlConfig =
        toml::from_str("version = 1\n[default]\nbig = \"project\"\nsmall = \"project-small\"\n")
            .unwrap();
    apply_project_toml_config(&mut merged, project.clone());
    assert_eq!(merged.default.big(), "project");

    preserve_project_overrides(&mut merged, &global, &project);

    assert_eq!(merged.default.big(), global.default.big());
    assert_eq!(merged.default.small(), global.default.small());
}

#[test]
fn project_init_writes_safe_template_and_gitignore_entry() {
    let workspace = TempDir::new().unwrap();
    let path = init_project_config(workspace.path()).unwrap();

    assert_eq!(
        path,
        fs::canonicalize(workspace.path())
            .unwrap()
            .join(PROJECT_CONFIG_DIR)
            .join(PROJECT_CONFIG_FILE)
    );
    let contents = fs::read_to_string(path).unwrap();
    assert!(contents.contains("version = 1"));
    assert!(contents.contains("[default]"));
    assert!(!contents.contains("api_key"));
    assert!(!contents.contains("mcp_servers"));
    assert_eq!(
        fs::read_to_string(workspace.path().join(".gitignore")).unwrap(),
        ".rustcode/config.toml\n"
    );
    assert!(init_project_config(workspace.path()).is_err());
}

#[test]
fn test_provider_supports_function_calling_includes_zai() {
    assert!(provider_supports_function_calling(
        "https://open.bigmodel.cn/api/paas/v4/chat/completions"
    ));
    assert!(provider_supports_function_calling(
        "https://api.z.ai/v1/chat/completions"
    ));
}

#[test]
fn test_ensure_sync_gitignore_creates_and_updates() {
    let dir = TempDir::new().unwrap();
    ensure_sync_gitignore(dir.path()).unwrap();
    let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.contains("sessions/*/sandbox/"));
    assert!(content.contains("sessions/*/artifacts/"));
    assert!(content.contains("sessions/*/subagents/"));
    assert!(content.contains("sessions/*/image_cache.json"));
    assert!(content.contains("*.bak"));

    // Test updating existing with missing entries
    let custom_dir = TempDir::new().unwrap();
    fs::write(custom_dir.path().join(".gitignore"), "custom_entry\n").unwrap();
    ensure_sync_gitignore(custom_dir.path()).unwrap();
    let updated = fs::read_to_string(custom_dir.path().join(".gitignore")).unwrap();
    assert!(updated.starts_with("custom_entry\n"));
    assert!(updated.contains("sessions/*/sandbox/"));
}

#[test]
fn test_get_sync_branch_fallback() {
    let dir = TempDir::new().unwrap();
    // Non-git directory falls back to main
    assert_eq!(get_sync_branch(dir.path()), "main");
}

#[test]
fn test_load_session_meta_fast_path() {
    let dir = TempDir::new().unwrap();
    let session_dir = dir.path().join(SESSIONS_DIR).join("12345");
    fs::create_dir_all(&session_dir).unwrap();
    let history_file = session_dir.join(HISTORY_FILE);

    let json = r#"[
            {"role": "user", "content": "hello world\nsecond line", "timestamp": "12:00", "images": ["massive_base64_data_12345"]},
            {"role": "assistant", "content": "hi there", "timestamp": "12:01"}
        ]"#;
    fs::write(&history_file, json).unwrap();

    let meta = load_session_meta(&history_file).expect("should parse meta");
    assert_eq!(meta.title, "hello world");
    assert_eq!(meta.when, "12:00");
    assert_eq!(meta.message_count, 2);
    assert_eq!(meta.path, history_file);
    assert_eq!(
        session_id_from_path(&history_file).as_deref(),
        Some("12345")
    );
}

#[test]
fn test_load_session_meta_unresumable_abandoned_session() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("test.json");
    // User prompt with no assistant reply is not resumable
    let json = r#"[{"role": "user", "content": "unfinished", "timestamp": "12:00"}]"#;
    fs::write(&file, json).unwrap();
    assert!(load_session_meta(&file).is_none());
}

#[test]
fn test_session_id_from_path_variations() {
    let path1 = PathBuf::from("/home/user/.config/rustcode/sessions/sess-abc/history.json");
    assert_eq!(session_id_from_path(&path1).as_deref(), Some("sess-abc"));

    let path2 = PathBuf::from("/home/user/.config/rustcode/sessions/sess-xyz.json");
    assert_eq!(session_id_from_path(&path2).as_deref(), Some("sess-xyz"));

    let path3 = PathBuf::from("/tmp/history.json");
    assert_eq!(session_id_from_path(&path3), None);
}
