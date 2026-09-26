# Native First-Run Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the GPUI app immediately typeable, truthful during session changes, and semantically usable through native accessibility APIs.

**Architecture:** Keep `AppView` as the owner of ephemeral UI readiness and focus intent, while treating controller snapshots as authoritative data. Add native semantics at the existing custom clickable surfaces without changing controller commands or visual composition.

**Tech Stack:** Rust 2024, GPUI/gpui-kit 0.6.6, AccessKit roles, Rust unit and GPUI tests.

**Spec:** `docs/superpowers/specs/2026-09-26-native-first-run-readiness-design.md`

## Global Constraints

- Preserve GPUI and the existing graphite visual system.
- Do not change controller protocol or approval policy.
- Add a failing focused test before each behavior change.
- Keep custom button semantics and pointer behavior on the same element.
- Run the repository-required check and test commands before completion.

## Review Focus

- A delayed initial snapshot must not discard a user's draft or focus intent.
- An empty authoritative session list must clear previously rendered sessions.
- Repeated New Chat activation during startup must emit only one start command.
- Search/settings/dialog focus must not be stolen by the one-time launch focus.
- Semantic button labels must describe actions and keep existing hit targets.

---

### Task 1: Make initial and session-start state explicit

**Files:**
- Modify: `crates/rustcode-app/src/view.rs`
- Test: `crates/rustcode-app/src/view.rs`

**Interfaces:**
- Consumes: `ControllerUpdate::{Snapshot, Error}` and existing `starting_new_session` state.
- Produces: one-time composer focus intent, readiness-aware start-screen copy, and an idempotent `start_new_chat` guard.

- [x] Add failing tests for the loading, idle, and starting copy states plus duplicate-start eligibility.
- [x] Run `cargo test -p rustcode-app view::tests::start_screen` and confirm the new tests fail because readiness policy is absent.
- [x] Add the minimal readiness state/helper, initialize composer focus intent, and guard repeated new-session activation.
- [x] Run the focused tests and confirm they pass.
- [ ] Commit the focused state/focus change.

### Task 2: Make session snapshots authoritative

**Files:**
- Modify: `crates/rustcode-app/src/view.rs`
- Test: `crates/rustcode-app/src/view.rs`

**Interfaces:**
- Consumes: `ControllerSnapshot.sessions`.
- Produces: `recent_sessions` exactly matching the latest accepted snapshot.

- [x] Add a failing regression that starts with one cached session, applies an empty snapshot, and expects no recent sessions.
- [x] Run the focused test and confirm it fails on the non-empty-only replacement condition.
- [x] Replace the cache condition with unconditional authoritative assignment.
- [x] Run the focused test and package tests; commit the fix.

### Task 3: Expose custom controls to native accessibility

**Files:**
- Modify: `crates/rustcode-app/src/view.rs`
- Test: `crates/rustcode-app/src/view.rs`

**Interfaces:**
- Consumes: GPUI `Role::Button` and existing click callbacks.
- Produces: AccessKit button nodes for custom New Chat and permission controls.

- [x] Add a render/test assertion or smallest pure policy test identifying the custom controls that require button semantics.
- [x] Run the focused test and confirm it fails before semantic metadata exists.
- [x] Put role, label, and click ownership on each custom interactive element without changing layout.
- [x] Run focused and package tests.
- [x] Build the packaged app and confirm the controls appear as native buttons in macOS accessibility state.
- [ ] Commit the accessibility change.

### Task 4: Integrate and verify

**Files:**
- Modify only for verified defects: `crates/rustcode-app/src/view.rs`

**Interfaces:**
- Consumes: the completed focus, readiness, snapshot, and semantics changes.
- Produces: a verified native first-run path.

- [x] Build and launch the disposable-config packaged app; type before clicking and confirm the draft appears.
- [x] Inspect the initial and settings states; cover loading and empty-session behavior with focused tests.
- [x] Run the Impeccable detector once against `crates/rustcode-app/src/view.rs` and fix only confirmed findings.
- [ ] Run `cargo fmt --check`, `cargo check --tests`, `cargo test`, and `cargo test -p rustcode-app`.
- [ ] Review the branch diff against issue #1417 and this spec, then push, open a PR to `main`, and merge it.
