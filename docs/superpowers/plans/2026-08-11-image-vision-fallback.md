# Image Vision Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route pasted images through a configured vision profile before text-only models, with native-image preservation and hash-based caching.

**Architecture:** Extend the existing `ModelProfile`/`AppConfig` configuration. Add a focused preprocessing module that recognizes existing `file://` image markers, hashes and reads their bytes, calls the dedicated profile through an injectable provider-agnostic client boundary, and rewrites only unsupported turns into structured text. Invoke preprocessing immediately before `prepare_turn_request`; native profiles bypass it.

**Tech Stack:** Rust, Tokio, reqwest, serde/serde_json, sha2, existing `AppState`, `ChatMessage`, and OpenAI-compatible payload helpers.

## Global Constraints

- Reuse existing `![image](file://…)` markers and `parse_multimodal_content` native handling.
- Do not send image parts to a profile resolved as text-only.
- Preserve image ordering and surrounding user text.
- Do not persist large base64 data in normal text context.
- Keep provider/model selection configurable through existing profiles.

### Task 1: Configuration and capability model

**Files:**
- Modify: `src/config.rs` (`ModelProfile`, `AppConfig`, runtime serialization/defaults)
- Modify: `src/app/state.rs` (active profile lookup and analysis cache state)
- Test: existing config/state test modules

**Interfaces:** Add `supports_vision: Option<bool>` to profiles and `vision_model: Option<String>` to app config, with serde defaults. Add `ModelProfile::image_input_supported()` and `AppState::active_model_profile()`/`vision_profile()` helpers. Store cache entries as hash-to-description in runtime-only state.

- [ ] Write tests for explicit supported/unsupported capability and missing/round-tripped config fields.
- [ ] Run focused config/state tests and verify the new tests fail before implementation.
- [ ] Implement the fields, defaults, profile resolution, and runtime cache storage.
- [ ] Run focused tests and `cargo check --tests`.
- [ ] Commit configuration/capability changes.

### Task 2: Image fallback preprocessing

**Files:**
- Create: `src/network/image_fallback.rs`
- Modify: `src/network.rs` to register the module and invoke preprocessing before request preparation
- Modify: `Cargo.toml` to add the minimal hash dependency if unavailable
- Test: `src/network/image_fallback.rs` unit tests

**Interfaces:** Implement `preprocess_history_for_model(client, state, cancel_token) -> Result<(), String>`, plus pure marker scanning/rewrite helpers and an injectable `VisionRequester` boundary. The helper sends one image request per uncached hash, formats `[Attached image analysis]` blocks, and rewrites only the image-bearing user messages.

- [ ] Write failing tests for native bypass, text-only fallback, multiple ordered images, cache reuse, failure, and unchanged text-only history.
- [ ] Run the focused tests and verify failure is caused by missing fallback behavior.
- [ ] Implement image extraction using existing markers, SHA-256 hashing, concise vision prompt construction, configured-profile request routing, and safe rewrite/cache behavior.
- [ ] Run focused tests, then `cargo check --tests`.
- [ ] Commit fallback preprocessing.

### Task 3: End-to-end turn integration and regression verification

**Files:**
- Modify: `src/network.rs` turn orchestration at the boundary before `prepare_turn_request`
- Modify: relevant config defaults/docs if needed
- Test: existing network tests and new integration-style request assertions

**Interfaces:** Ensure preprocessing runs once per user turn before `ctx.last_sent_messages` is captured; native models retain the exact prior payload path; fallback errors become system notices and do not invoke the active text-only provider.

- [ ] Add an end-to-end test asserting vision request → text-only main request ordering and payload contents.
- [ ] Run it red, then wire preprocessing into the turn lifecycle.
- [ ] Run all relevant tests, `cargo check --tests`, and `cargo test`.
- [ ] Inspect the diff for metadata/base64 leakage and verify all requirements.
- [ ] Commit, push the branch, create the PR into `main`, merge it, then checkout `main` and pull.
