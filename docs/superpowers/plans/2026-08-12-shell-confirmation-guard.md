# Shell Confirmation Guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Require confirmation for unknown or potentially mutating `run_command` invocations while preserving automatic execution for an explicit read-only shell allowlist.

**Architecture:** Keep authorization centralized in `command_requires_confirmation` and classify each shell segment conservatively. A command requires confirmation unless every segment is recognized as read-only and the command contains no unsafe redirection or substitution boundary. Existing destructive Git detection remains part of the same classifier.

**Tech Stack:** Rust, serde_json, existing `run_command` authorization helpers, Cargo unit tests.

## Global Constraints

- Preserve automatic execution for clearly read-only inspection commands already covered by the harness contract.
- Require confirmation when any command segment is unknown or potentially mutating.
- Do not change command execution, shell spawning, or tool batching outside authorization classification.
- Keep the patch limited to `src/tools/exec.rs`, `src/tools/mod.rs` tests, and focused documentation updates if needed.

---

### Task 1: Add failing classifier and authorization tests

**Files:**
- Modify: `src/tools/exec.rs` tests
- Modify: `src/tools/mod.rs` tests

**Interfaces:**
- Consumes: existing `command_requires_confirmation` and `authorize_tool_with_args` helpers.
- Produces: regression coverage for read-only `gh`, mutating `gh`, arbitrary shell mutation, and safe command chains.

- [ ] **Step 1: Add the failing command-classification tests**

Add tests asserting that `gh issue list --repo lhagfoss/rustcode`, `gh auth status`, `rg`, and `git status` do not require confirmation, while `gh issue close`, `rm -rf`, `python -c`, and `cargo test` do. Add a chained-command case where one safe segment and one unknown segment require confirmation.

- [ ] **Step 2: Add the failing authorization test**

Assert that `authorize_tool_with_args("run_command", {"command":"gh issue close 1"}, Build, false, false)` returns `RequireConfirmation`, while the equivalent `gh issue list` call returns `Allow`.

- [ ] **Step 3: Run the focused tests and verify they fail for the missing allowlist behavior**

Run:

```bash
cargo test command_confirmation
cargo test command_authorization_distinguishes
```

Expected: the new unknown/mutating-command assertions fail because the current implementation only detects selected destructive Git commands.

### Task 2: Implement the conservative read-only allowlist

**Files:**
- Modify: `src/tools/exec.rs`

**Interfaces:**
- Consumes: existing shell segment splitter and Git safety classifier.
- Produces: `command_confirmation_scope`/`command_requires_confirmation` behavior that allows only recognized read-only commands.

- [ ] **Step 1: Add read-only command recognition**

Recognize only explicit read-only command families and subcommands. Treat `git` inspection commands, `gh issue/pr list/view`, `gh auth status/help`, filesystem/search inspection commands, and harmless shell builtins as read-only. Return an unsafe/unknown scope for every other command.

- [ ] **Step 2: Make shell boundaries conservative**

Require confirmation when a command contains redirection, command substitution, grouping, or a segment that is not recognized as read-only. Preserve safe chains only when every segment is read-only.

- [ ] **Step 3: Preserve destructive Git scope reporting**

Keep existing destructive Git scopes in confirmation previews so prompts continue to explain why a command is blocked.

### Task 3: Verify the safety boundary

**Files:**
- Test: `src/tools/exec.rs` and `src/tools/mod.rs` focused tests

**Interfaces:**
- Consumes: the completed classifier and authorization behavior.
- Produces: evidence that the observed latest-session commands remain non-blocking and unsafe alternatives are gated.

- [ ] **Step 1: Run focused tests**

Run:

```bash
cargo test command_confirmation
cargo test command_authorization_distinguishes
```

Expected: all focused tests pass.

- [ ] **Step 2: Run repository gates**

Run:

```bash
cargo check --tests
cargo test
git diff --check
```

Expected: commands exit successfully; existing warnings may remain, but no new errors or test failures appear.

- [ ] **Step 3: Review the final diff**

Confirm the diff changes only shell authorization classification and its regression tests, and that no planner files or unrelated user changes are included.
