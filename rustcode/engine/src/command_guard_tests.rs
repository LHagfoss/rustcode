use super::*;

const REPO_POLICY: &str = include_str!("../../../.rustcode/command-policy.toml");

fn mac() -> Host {
    Host {
        os: "macos",
        var: |_| None,
        has_program: |_| true,
    }
}

fn linux() -> Host {
    Host {
        os: "linux",
        var: |_| None,
        has_program: |_| true,
    }
}

fn policy() -> Policy {
    Policy::parse(REPO_POLICY, PathBuf::from(POLICY_PATH)).expect("repo policy parses")
}

fn verdict(command: &str) -> Verdict {
    policy().evaluate(command, &[], &mac())
}

fn denied_rule(command: &str) -> Option<String> {
    match verdict(command) {
        Verdict::Deny { rule, .. } => Some(rule),
        _ => None,
    }
}

fn rewritten(command: &str) -> Option<String> {
    match verdict(command) {
        Verdict::Rewrite { command, .. } => Some(command),
        _ => None,
    }
}

#[test]
fn the_repository_policy_parses_with_every_rule() {
    assert_eq!(policy().rules.len(), 6);
}

#[test]
fn local_tests_are_denied_with_the_rule_and_the_alternative() {
    let Verdict::Deny { rule, message } = verdict("cargo test -p rustcode command_guard") else {
        panic!("cargo test is denied");
    };
    assert_eq!(rule, "no-local-tests");
    assert!(message.contains("`no-local-tests`"), "{message}");
    assert!(
        message.contains("cargo check --workspace --tests"),
        "{message}"
    );
    assert!(message.contains("gh pr checks"), "{message}");
    assert!(
        message.contains("RUSTCODE_ALLOW_LOCAL_TESTS=1"),
        "{message}"
    );
}

#[test]
fn local_tests_are_found_through_lists_wrappers_and_quotes() {
    for command in [
        "cargo test",
        "cargo t -p engine",
        "cargo test --all-features",
        "cargo nextest run -p engine",
        "cd rustcode && cargo test --workspace",
        "cargo test --workspace 2>&1 | tail -20",
        "cargo +nightly test --all",
        "RUST_LOG=debug cargo test --workspace",
        "env RUST_LOG=debug cargo test --workspace",
        "timeout 600 cargo test --workspace",
        "timeout -s KILL 600 cargo test --workspace",
        "taskpolicy -b env CARGO_BUILD_JOBS=2 cargo test --workspace",
        "nice -n 10 cargo nextest run --workspace",
        "/usr/local/bin/cargo test --workspace",
        "bash -lc 'cargo test --workspace'",
        "sh -c \"cd x && cargo test --workspace\"",
        "echo hi; (cargo test --workspace)",
        "cargo test -p engine && cargo test --workspace",
        "true\ncargo test --workspace",
        "cargo test --workspace=true",
    ] {
        assert_eq!(
            denied_rule(command).as_deref(),
            Some("no-local-tests"),
            "{command}"
        );
    }
}

#[test]
fn checks_and_quoted_mentions_pass() {
    for command in [
        "cargo check --workspace --tests",
        "cargo clippy --all-targets -- -D warnings",
        "echo 'cargo test --workspace'",
        "git commit -m \"never run cargo test --workspace\"",
        "rg 'git stash' docs",
        "cargo fmt --all -- --check",
        "# cargo test --workspace",
    ] {
        assert_eq!(denied_rule(command), None, "{command}");
    }
}

#[test]
fn the_override_variable_lets_local_tests_run() {
    let mut allowed = mac();
    allowed.var = |name| (name == "RUSTCODE_ALLOW_LOCAL_TESTS").then(|| "1".to_owned());
    assert!(!matches!(
        policy().evaluate("cargo test -p engine", &[], &allowed),
        Verdict::Deny { .. }
    ));
    // In the command itself, or in the exec tool's `env`.
    assert!(!matches!(
        verdict("RUSTCODE_ALLOW_LOCAL_TESTS=1 cargo test -p engine"),
        Verdict::Deny { .. }
    ));
    assert!(!matches!(
        verdict("env RUSTCODE_ALLOW_LOCAL_TESTS=1 cargo test -p engine"),
        Verdict::Deny { .. }
    ));
    let extra = [("RUSTCODE_ALLOW_LOCAL_TESTS".to_owned(), "1".to_owned())];
    assert!(!matches!(
        policy().evaluate("cargo test -p engine", &extra, &mac()),
        Verdict::Deny { .. }
    ));
    // A false value is not an override.
    assert_eq!(
        denied_rule("RUSTCODE_ALLOW_LOCAL_TESTS=0 cargo test -p engine").as_deref(),
        Some("no-local-tests")
    );
}

#[test]
fn heavy_cargo_runs_at_low_priority_on_every_core() {
    assert_eq!(
        rewritten("cargo build").as_deref(),
        Some("nice -n 10 cargo build")
    );
    assert_eq!(
        rewritten("cd rustcode && cargo check -p engine 2>&1 | tail").as_deref(),
        Some("cd rustcode && nice -n 10 cargo check -p engine 2>&1 | tail")
    );
    assert_eq!(
        rewritten("RUST_BACKTRACE=1 cargo clippy --workspace").as_deref(),
        Some("nice -n 10 RUST_BACKTRACE=1 cargo clippy --workspace")
    );
}

#[test]
fn each_heavy_cargo_in_a_list_is_prefixed_once() {
    assert_eq!(
        rewritten("cargo check && cargo build").as_deref(),
        Some("nice -n 10 cargo check && nice -n 10 cargo build")
    );
}

#[test]
fn a_script_run_by_a_shell_is_prefixed_as_a_whole() {
    assert_eq!(
        rewritten("bash -c 'cargo build'").as_deref(),
        Some("nice -n 10 bash -c 'cargo build'")
    );
}

#[test]
fn linux_uses_nice() {
    assert_eq!(
        policy().evaluate("cargo build", &[], &linux()),
        Verdict::Rewrite {
            command: "nice -n 10 cargo build".to_owned(),
            rules: vec!["cargo-low-priority".to_owned()],
        }
    );
}

#[test]
fn cargo_already_under_a_priority_wrapper_is_left_alone() {
    for command in [
        "taskpolicy -b env CARGO_BUILD_JOBS=2 cargo build",
        "taskpolicy -b cargo check -p engine",
        "nice -n 10 cargo build",
        "cargo fmt --all -- --check",
        "cargo --version",
        "git status",
        // Rewriting an already rewritten command changes nothing.
        "cd x && taskpolicy -b env CARGO_BUILD_JOBS=2 cargo clippy",
    ] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
}

#[test]
fn a_rewrite_is_skipped_where_its_tool_is_missing() {
    let mut bare = mac();
    bare.has_program = |_| false;
    assert_eq!(policy().evaluate("cargo build", &[], &bare), Verdict::Allow);
}

#[test]
fn a_denial_wins_over_a_rewrite_in_the_same_command() {
    assert_eq!(
        denied_rule("cargo build && git stash").as_deref(),
        Some("no-git-stash")
    );
}

#[test]
fn git_history_rewrites_are_denied() {
    for (command, rule) in [
        ("git rebase main", "no-git-rebase"),
        ("git -C /tmp/x rebase -i HEAD~3", "no-git-rebase"),
        ("git stash", "no-git-stash"),
        ("git stash pop", "no-git-stash"),
        ("cd repo && git stash push -u -m x", "no-git-stash"),
        ("git reset --hard origin/main", "no-git-reset-hard"),
        ("git push --force", "no-force-push"),
        ("git push -f origin main", "no-force-push"),
        (
            "git push origin main --force-with-lease=main",
            "no-force-push",
        ),
        ("timeout 30 git rebase origin/main", "no-git-rebase"),
        ("sudo -n git stash", "no-git-stash"),
    ] {
        assert_eq!(denied_rule(command).as_deref(), Some(rule), "{command}");
    }
}

#[test]
fn ordinary_git_passes() {
    for command in [
        "git status",
        "git reset HEAD~1",
        "git reset --soft HEAD~1",
        "git push origin feat/x",
        "git pull --ff-only",
        "git log --oneline -n 5",
        "git commit -m 'stash the rebase'",
        "git merge origin/main --no-edit",
    ] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
}

#[test]
fn offsets_survive_quotes_and_multibyte_text() {
    assert_eq!(
        rewritten("echo 'ö — \"x\"' && cargo build").as_deref(),
        Some("echo 'ö — \"x\"' && nice -n 10 cargo build")
    );
}

#[test]
fn a_policy_with_a_mistake_is_rejected() {
    for text in [
        "[[rule]]\nname = \"x\"\naction = \"deny\"\n",
        "[[rule]]\nname = \"x\"\naction = \"rewrite\"\nprogram = \"cargo\"\n",
        "[[rule]]\nname = \"x\"\naction = \"explode\"\nprogram = \"cargo\"\n",
        "[[rule]]\nname = \"x\"\naction = \"deny\"\nprogram = \"cargo\"\ntypo = 1\n",
    ] {
        assert!(
            Policy::parse(text, PathBuf::from("p.toml")).is_err(),
            "{text}"
        );
    }
}

#[test]
fn a_policy_can_be_switched_off() {
    let policy = Policy::parse(
        "enabled = false\n[[rule]]\nname = \"x\"\naction = \"deny\"\nprogram = \"git\"\n",
        PathBuf::from("p.toml"),
    )
    .unwrap();
    assert!(!policy.enabled);
}

#[test]
fn the_nearest_policy_applies_and_a_missing_one_allows() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".git")).unwrap();
    std::fs::create_dir_all(root.path().join(".rustcode")).unwrap();
    std::fs::create_dir_all(root.path().join("a/b")).unwrap();
    assert!(!has_policy(&root.path().join("a/b")));
    std::fs::write(
        root.path().join(POLICY_PATH),
        "[[rule]]\nname = \"no-stash\"\naction = \"deny\"\nprogram = \"git\"\nsubcommand = [\"stash\"]\n",
    )
    .unwrap();
    assert!(has_policy(&root.path().join("a/b")));
    assert!(matches!(
        check(&root.path().join("a/b"), "git stash", &[]),
        Verdict::Deny { .. }
    ));
    assert_eq!(
        check(&root.path().join("a/b"), "git status", &[]),
        Verdict::Allow
    );
    // A broken file is ignored rather than blocking every command.
    std::fs::write(root.path().join(POLICY_PATH), "not toml [").unwrap();
    assert_eq!(check(root.path(), "git stash", &[]), Verdict::Allow);
}

#[test]
fn a_policy_above_the_repository_root_does_not_apply() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(outer.path().join(".rustcode")).unwrap();
    std::fs::write(
        outer.path().join(POLICY_PATH),
        "[[rule]]\nname = \"x\"\naction = \"deny\"\nprogram = \"git\"\n",
    )
    .unwrap();
    let repo = outer.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    assert!(!has_policy(&repo));
}

#[test]
fn a_rewrite_is_refused_with_the_command_to_run_instead() {
    let verdict = verdict("cargo build");
    let refusal = refusal(&verdict).unwrap();
    assert!(refusal.contains("`cargo-low-priority`"), "{refusal}");
    assert!(refusal.contains("nice -n 10 cargo build"), "{refusal}");
    assert_eq!(super::refusal(&Verdict::Allow), None);
}

#[test]
fn an_approval_request_names_its_command() {
    assert_eq!(
        requested_command(&json!({"command": "git stash"})).as_deref(),
        Some("git stash")
    );
    assert_eq!(
        requested_command(&json!({"command": ["bash", "-lc", "cargo test --workspace"]}))
            .as_deref(),
        Some("bash -lc 'cargo test --workspace'")
    );
    assert_eq!(requested_command(&json!({"path": "x"})), None);
    // The argv form is checked as the shell command it stands for.
    assert_eq!(
        denied_rule(&requested_command(&json!({"command": ["bash", "-lc", "git stash"]})).unwrap())
            .as_deref(),
        Some("no-git-stash")
    );
}

#[test]
fn shell_quote_round_trips_awkward_words() {
    assert_eq!(shell_quote("cargo"), "cargo");
    assert_eq!(shell_quote("a b"), "'a b'");
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
    assert_eq!(shell_quote(""), "''");
}

#[test]
fn the_hook_denies_a_refused_bash_call_and_ignores_the_rest() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".git")).unwrap();
    std::fs::create_dir_all(root.path().join(".rustcode")).unwrap();
    std::fs::write(
        root.path().join(POLICY_PATH),
        "[[rule]]\nname = \"no-stash\"\naction = \"deny\"\nprogram = \"git\"\nsubcommand = [\"stash\"]\nmessage = \"Use a WIP commit.\"\n",
    )
    .unwrap();
    let call = |tool: &str, command: &str| {
        json!({
            "tool_name": tool,
            "cwd": root.path(),
            "tool_input": {"command": command},
        })
        .to_string()
    };
    let response: Value =
        serde_json::from_str(&hook_response(&call("Bash", "git stash")).unwrap()).unwrap();
    let output = &response["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreToolUse");
    assert_eq!(output["permissionDecision"], "deny");
    assert!(
        output["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("`no-stash`")
    );
    assert_eq!(hook_response(&call("Bash", "git status")), None);
    assert_eq!(hook_response(&call("Read", "git stash")), None);
    assert_eq!(hook_response("not json"), None);
}
