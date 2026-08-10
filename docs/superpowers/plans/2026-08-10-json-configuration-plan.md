# JSON Configuration Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Replace Rustcode's TOML persistence with independent `models.json` and `config.json` files, preserving in-code defaults and making ACP model/MCP initialization match other runtimes.

**Architecture:** Keep `AppConfig` as the in-memory aggregate used throughout the application, but introduce file-specific JSON DTOs or serialization helpers in `src/config.rs`. Load each file independently over its own `AppConfig::default()` slice, never write defaults during startup, and route intentional saves to the owning JSON file. Extract enabled-MCP startup into a shared helper used by normal and ACP startup.

**Tech Stack:** Rust 2024, Serde/serde_json, Tokio, existing ACP and MCP clients, Cargo tests.

## Global Constraints

- `config.toml` is deprecated and must not be read, written, or automatically migrated.
- Missing or malformed JSON must preserve the existing file and fall back to in-code defaults.
- Startup loading must not create or overwrite configuration files.
- `models.json` owns `default` and `models`; `config.json` owns runtime and integration settings.
- ACP and interactive/headless execution must share model loading and enabled-MCP startup.
- Existing callers continue consuming `AppConfig` without a broad application-wide rewrite.

---

### Task 1: Establish JSON file ownership and test fixtures

**Files:**
- Modify: `src/config.rs`
- Test: `src/config.rs` unit tests
- Modify: `Cargo.toml` only if the existing `serde_json` dependency is insufficient

**Interfaces:**
- Produces constants/helpers for `models.json` and `config.json` paths.
- Keeps `load_config_from(&Path) -> (String, String, AppConfig)` as the application-facing loader.

- [ ] **Step 1: Write failing tests for missing-file behavior**

Add tests that call `load_config_from` with an empty temporary directory and assert that defaults are returned while neither `models.json` nor `config.json` is created.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test config::tests::test_load_missing_config_returns_default -- --exact`

Expected: the current test fails because it expects `config.toml` to be created and the new JSON behavior is not implemented.

- [ ] **Step 3: Write failing tests for independent JSON ownership**

Create temporary `models.json` containing a custom profile/default and `config.json` containing a custom theme/tool protocol. Assert both values load into the aggregate `AppConfig`.

- [ ] **Step 4: Run the focused tests and verify they fail for the intended reason**

Run: `cargo test config::tests::test_load_json_configuration_files -- --exact`

Expected: FAIL because `load_config_from` currently only parses TOML.

- [ ] **Step 5: Commit the test-only changes**

Run: `git add src/config.rs && git commit -m "test: define JSON config loading behavior"`

### Task 2: Implement split JSON loading and safe fallbacks

**Files:**
- Modify: `src/config.rs`
- Test: `src/config.rs` unit tests

**Interfaces:**
- `load_config_from` reads `models.json` and `config.json` independently.
- A malformed file logs a warning and uses only that file's built-in defaults.
- `save_entire_config` and internal persistence helpers write the appropriate JSON files.

- [ ] **Step 1: Add malformed-file tests**

Write tests for malformed `models.json` and malformed `config.json`. Assert the corresponding defaults are used and the malformed bytes remain unchanged after loading.

- [ ] **Step 2: Run the new malformed-file tests and verify they fail**

Run: `cargo test config::tests::test_malformed_json_is_preserved -- --exact`

Expected: FAIL because the current loader does not recognize either JSON file.

- [ ] **Step 3: Add file-specific serializable types or serializers**

Implement JSON parsing for the model-owned fields (`default`, `models`) and runtime-owned fields. Reuse existing field types and defaults; do not duplicate model profile semantics.

- [ ] **Step 4: Replace TOML startup loading**

Remove the `config.toml` existence/read/parse path from `load_config_from`. Load each JSON file only if it exists. Use `AppConfig::default()` as the independent fallback and keep endpoint resolution valid when a file has an empty model list or invalid selected default.

- [ ] **Step 5: Replace TOML persistence**

Update save helpers to serialize the model-owned and runtime-owned portions to `models.json` and `config.json`. Preserve the `is_valid` guard. Do not create configuration files merely because `load_config_from` was called.

- [ ] **Step 6: Run focused config tests and verify they pass**

Run: `cargo test config::tests -- --nocapture`

Expected: PASS, including split loading, missing files, malformed preservation, endpoint resolution, and round-trip persistence.

- [ ] **Step 7: Commit the JSON loader implementation**

Run: `git add src/config.rs && git commit -m "feat: store config in split JSON files"`

### Task 3: Share enabled-MCP startup with ACP

**Files:**
- Modify: `src/main.rs`
- Modify: `src/acp.rs`
- Modify: `src/mcp.rs` if the helper belongs there
- Test: `src/acp.rs` and/or `src/mcp.rs` unit tests

**Interfaces:**
- A shared async helper starts every enabled configured MCP server once.
- Normal interactive startup and `run_acp` both call the helper before turns begin.

- [ ] **Step 1: Write a regression test for ACP initialization ownership**

Add a testable helper boundary or configuration-driven assertion showing that enabled MCP entries are passed to the startup routine for ACP as well as normal startup. Keep the test independent of spawning `npx` by testing the selected enabled-server list or injecting a test launcher.

- [ ] **Step 2: Run the focused ACP/MCP test and verify it fails**

Run: `cargo test acp::tests mcp::tests -- --nocapture`

Expected: FAIL because ACP currently enters `run_acp()` without the normal startup MCP initialization.

- [ ] **Step 3: Extract the shared startup helper**

Move the enabled-server iteration currently in `main.rs` into a reusable async helper. Preserve the current best-effort behavior for individual server startup errors.

- [ ] **Step 4: Call the helper from both startup paths**

Call it from normal startup and before ACP accepts prompts. Ensure configuration is loaded from the new JSON files through `AppState::new()`.

- [ ] **Step 5: Run focused tests and verify they pass**

Run: `cargo test acp::tests mcp::tests -- --nocapture`

Expected: PASS without launching external MCP processes in tests.

- [ ] **Step 6: Commit the ACP/MCP startup fix**

Run: `git add src/main.rs src/acp.rs src/mcp.rs && git commit -m "fix: initialize MCP servers in ACP mode"`

### Task 4: Update documentation and verify the complete runtime

**Files:**
- Modify: `README.md`
- Modify: `src/raw_cli.rs` if it contains stale TOML wording
- Test: repository test suite

- [ ] **Step 1: Update user-facing configuration documentation**

Document `models.json`, `config.json`, in-code fallback behavior, and the fact that `config.toml` is deprecated and ignored.

- [ ] **Step 2: Remove stale TOML references**

Update warnings and comments that still tell users to edit `config.toml`; leave historical migration notes only where they accurately describe legacy behavior.

- [ ] **Step 3: Run formatting and static checks**

Run: `cargo fmt --check` and `cargo check --tests`

Expected: both commands succeed.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test`

Expected: all tests pass.

- [ ] **Step 5: Inspect the diff and commit documentation**

Run: `git diff main...HEAD --check && git status --short`

Then: `git add README.md src/raw_cli.rs && git commit -m "docs: document JSON configuration"`

### Task 5: Publish and integrate

- [ ] **Step 1: Push the feature branch**

Run: `git push -u origin feature/json-config-split`

- [ ] **Step 2: Create the pull request**

Run: `gh pr create --base main --head feature/json-config-split --title "feat: split configuration into JSON files" --body "Replaces deprecated config.toml persistence with models.json and config.json, preserves in-code fallbacks without startup overwrites, and initializes MCP servers in ACP mode."`

- [ ] **Step 3: Merge the pull request**

Run: `gh pr merge --merge --delete-branch`

- [ ] **Step 4: Return to main and update it**

Run: `git checkout main && git pull --ff-only`

