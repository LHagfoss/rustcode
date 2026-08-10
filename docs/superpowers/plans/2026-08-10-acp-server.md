# ACP Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a headless ACP v1 stdio runtime to rustcode using the official Rust ACP SDK while leaving the TUI path independent.

**Architecture:** Add an `--acp` entry point that connects the official SDK's ACP `Agent` to stdio. The adapter stores per-session `AppState` and calls a reusable headless turn function; ACP updates are emitted through the SDK connection and diagnostics stay on stderr.

**Tech Stack:** Rust 2024, Tokio, `agent-client-protocol = 2.0`, existing rustcode network/MCP/session code, serde_json.

## Global Constraints

- Stable ACP wire protocol is v1; do not enable draft protocol v2.
- Do not enable unstable MCP-over-ACP transport.
- Stdout in `--acp` mode is reserved for ACP JSON-RPC frames.
- The existing TUI and `--prompt` behavior must remain compatible.
- Follow repository workflow: verify with `cargo check --tests` / `cargo test`, commit, push, open and merge a PR into `main`, then checkout `main` and pull.

### Task 1: Add the ACP dependency and CLI switch

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Test: `src/cli.rs`

- [ ] Write a failing parser test asserting `Cli::try_parse_from(["rustcode", "--acp"])` sets `acp`.
- [ ] Run `cargo test cli::tests::parses_acp_flag` and verify it fails because the field is absent.
- [ ] Add `agent-client-protocol = "2.0.0"`, add `pub acp: bool` with `#[arg(long = "acp")]`, and route the flag to `acp::run_acp()` before TUI setup.
- [ ] Run the focused parser test and `cargo check --tests`.
- [ ] Commit as `feat: add acp runtime entrypoint`.

### Task 2: Build the ACP v1 session adapter

**Files:**
- Create: `src/acp.rs`
- Modify: `src/main.rs`
- Modify: `src/raw_cli.rs`

- [ ] Add failing unit tests for session creation and prompt text extraction.
- [ ] Run the focused ACP tests and verify they fail because the adapter does not exist.
- [ ] Implement an ACP `Agent` using `agent-client-protocol::schema::v1`, with `initialize`, `session/new`, and `session/prompt` callbacks registered through `Agent::builder()` and `on_receive_request!()`.
- [ ] Store session IDs and `Arc<Mutex<AppState>>` in an adapter-owned map; initialize each state using the requested working directory and configured model override.
- [ ] Refactor the shared headless turn loop to return final prose in addition to preserving `--prompt` printing behavior.
- [ ] Emit ACP `session/update` text chunks and return a normal stop reason after the shared turn completes.
- [ ] Keep all ACP diagnostics on stderr.
- [ ] Run focused ACP tests and verify they pass.
- [ ] Commit as `feat: implement headless acp agent`.

### Task 3: Add protocol smoke coverage and documentation

**Files:**
- Modify: `src/acp.rs`
- Modify: `README.md`

- [ ] Add a test or fixture that sends ACP initialize and session-new JSON-RPC requests to the binary and asserts valid ACP responses.
- [ ] Run the smoke test and verify it fails if stdout contains non-ACP diagnostic output.
- [ ] Ensure logging and model/config diagnostics use stderr in ACP mode.
- [ ] Document `rustcode --acp`, stdio launching, and a generic orchestrator command configuration.
- [ ] Run `cargo check --tests` and `cargo test`.
- [ ] Commit as `docs: document acp runtime`.

### Task 4: Publish the feature branch

- [ ] Review `git diff main...HEAD` and run the complete verification commands.
- [ ] Push `feature/acp-server`.
- [ ] Create a PR with `gh pr create --base main`.
- [ ] Merge it with `gh pr merge` after checks pass.
- [ ] Checkout `main` and run `git pull`.
