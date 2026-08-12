# Native Tool Thought Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (\`- [ ]\`) syntax for tracking.

**Goal:** Prevent repeated native-tool planning loops, render one compact thought preview, and isolate injected runtime metadata from incomplete user requests.

**Architecture:** Extend the shared response collector with an explicit structured-tool-call signal from \`StreamBuffer\`, so native calls terminate response collection before reasoning-only recovery runs. Add a pure UI parser that removes every thought span and returns one preview line, then wrap dynamic context in an explicit RustCode metadata boundary while preserving cache-friendly tail placement.

**Tech Stack:** Rust, Tokio, serde_json, ratatui, existing Cargo unit-test modules.

## Global Constraints

- Do not change the selected model, Ollama configuration, or thinking settings.
- Preserve one compact, non-interactive thought preview.
- Preserve existing continuation behavior for ordinary text-only responses.
- Keep dynamic context appended at the request tail so the static prompt prefix remains cache-stable.
- Run \`cargo check --tests\` and \`cargo test\` before publishing.

---

### Task 1: Stop continuation when native tool calls exist

**Files:**
- Modify: `src/network/subagents.rs` for the shared collector interface
- Modify: \`src/network/runner.rs\`
- Modify: \`src/network/turn_engine.rs\` around the \`runner::collect_response\` call
- Test: \`src/network/runner.rs\` unit tests

**Interfaces:**
- \`runner::ResponseChunk { content: String, finish_reason: Option<String>, has_native_tool_calls: bool }\` is returned by the request closure.
- \`runner::CollectedResponse { content: String, finish_reason: Option<String> }\` is returned by \`collect_response\`.
- \`turn_engine\` fills \`has_native_tool_calls\` from \`stream_buffer.native_tool_calls.is_empty()\` after \`stream_request\` completes.

- [ ] **Step 1: Write the failing tests**

Add \`collect_response_stops_on_native_tool_call_with_reasoning\`: return a reasoning-only chunk with \`has_native_tool_calls: true\` on the first request and assert that the closure ran once and the collected content is unchanged.

Add \`collect_response_continues_reasoning_only_without_native_tool_call\`: return a reasoning-only chunk with \`has_native_tool_calls: false\`, then a final answer, and assert that the closure ran twice.

Use this test shape:

\`\`\`rust
let result = collect_response(|previous| {
    calls += 1;
    async move {
        Ok(ResponseChunk {
            content: if previous.is_empty() {
                "<think>plan</think>".into()
            } else {
                "answer".into()
            },
            finish_reason: Some("stop".into()),
            has_native_tool_calls: previous.is_empty(),
        })
    }
})
.await
.unwrap();
assert_eq!(calls, 1);
assert!(result.has_native_tool_calls);
\`\`\`

- [ ] **Step 2: Run the focused tests and verify the intended failure**

Run:

\`\`\`bash
cargo test network::runner::tests::collect_response_stops_on_native_tool_call_with_reasoning -- --exact
cargo test network::runner::tests::collect_response_continues_reasoning_only_without_native_tool_call -- --exact
\`\`\`

Expected: compilation fails because the new response types and closure shape do not exist yet.

- [ ] **Step 3: Add the response types and update the collector**

In \`src/network/runner.rs\`, define the two \`pub(crate)\` response structs, update the closure return type, accumulate \`content\`, OR the native-call flag across chunks, and gate continuation with:

\`\`\`rust
if !has_native_tool_calls
    && runner.allow_continuation(crate::network::is_cut_off(
        &accumulated,
        finish_reason.as_deref(),
    ))
{
    continue;
}
\`\`\`

Return all three collected fields. Update the existing cut-off test closure to return \`ResponseChunk\` with \`has_native_tool_calls: false\`.

- [ ] **Step 4: Pass the structured-call signal from the turn engine**

In the \`stream_request\` closure in \`src/network/turn_engine.rs\`, read \`request_buffer.native_tool_calls\` after the stream completes, then return \`ResponseChunk\` with the cloned content, finish reason, and \`has_native_tool_calls: !native_tool_calls.is_empty()\`.

After \`collect_response\`, destructure \`CollectedResponse\` and keep using its content and finish reason for the existing normalization path. The final buffer still supplies the actual native call envelopes for execution.

- [ ] **Step 5: Run the focused tests and verify they pass**

Run:

\`\`\`bash
cargo test network::runner::tests
\`\`\`

Expected: PASS, including both new tests and the existing bounded-continuation test.

- [ ] **Step 6: Commit the task**

\`\`\`bash
git add src/network/runner.rs src/network/turn_engine.rs
git commit -m "fix: stop recovery for native tool calls"
\`\`\`

### Task 2: Collapse all thought blocks into one preview

**Files:**
- Modify: \`src/ui/mod.rs\` around \`render_assistant_message\`
- Modify: \`src/ui/tests.rs\` thought-preview tests

**Interfaces:**
- \`split_thought_blocks(content: &str) -> (String, Option<String>)\` returns answer content with every thought span removed and the first meaningful thought line, if any.
- Existing \`truncate_thought_preview\` remains responsible for width-safe one-line truncation.

- [ ] **Step 1: Write the failing tests**

Add tests covering complete repeated blocks, an unclosed trailing block, and prose surrounding thought blocks:

\`\`\`rust
#[test]
fn thought_parser_collapses_multiple_blocks() {
    let (answer, preview) = split_thought_blocks(
        "<think>First useful thought\nmore detail</think>answer\n<think>Second thought</think>",
    );
    assert_eq!(answer, "answer");
    assert_eq!(preview.as_deref(), Some("First useful thought"));
}

#[test]
fn thought_parser_drops_unclosed_block_from_answer() {
    let (answer, preview) = split_thought_blocks(
        "before\n<think>Planning the next action",
    );
    assert_eq!(answer, "before");
    assert_eq!(preview.as_deref(), Some("Planning the next action"));
}
\`\`\`

- [ ] **Step 2: Run the focused UI tests and verify the intended failure**

Run:

\`\`\`bash
cargo test ui::tests::thought_parser_collapses_multiple_blocks -- --exact
cargo test ui::tests::thought_parser_drops_unclosed_block -- --exact
\`\`\`

Expected: compilation fails because \`split_thought_blocks\` does not exist.

- [ ] **Step 3: Implement the pure thought parser**

Implement \`split_thought_blocks\` in \`src/ui/mod.rs\`. Scan left-to-right for \`<think>\`, preserve non-thinking text, find the next \`</think>\`, and use the remainder as the thought body when the closing tag is absent. Select the first non-empty trimmed line across all thought bodies. For an unclosed block, drop the remainder from answer content because it is still reasoning, not an answer.

- [ ] **Step 4: Use the parser in assistant rendering**

Replace the current first-block \`content.find("<think>")\` logic in \`render_assistant_message\` with \`split_thought_blocks(display_content)\`. Render the existing metadata line and exactly one indented preview from the returned \`Option<String>\`. Pass the returned answer content through \`strip_rendered_tool_blocks\` and leave the existing non-interactive spans unchanged.

- [ ] **Step 5: Run the focused tests and verify they pass**

Run:

\`\`\`bash
cargo test ui::tests::thought_parser
cargo test ui::tests::thought_preview
\`\`\`

Expected: PASS with no \`<think>\` tags present in rendered answer content.

- [ ] **Step 6: Commit the task**

\`\`\`bash
git add src/ui/mod.rs src/ui/tests.rs
git commit -m "fix: collapse thought previews"
\`\`\`

### Task 3: Delimit injected runtime context

**Files:**
- Modify: \`src/network/messages.rs\`
- Modify: \`src/network.rs\` where \`dynamic_context\` is appended
- Test: \`src/network/messages.rs\` unit tests

**Interfaces:**
- \`wrap_runtime_context(text: &str) -> String\` returns a RustCode-owned metadata block.
- \`append_to_last_message\` remains a generic append helper and does not change behavior for callers that pass ordinary text.

- [ ] **Step 1: Write the failing test**

Add a test asserting that runtime context is clearly delimited and explicitly marked as non-user instruction:

\`\`\`rust
#[test]
fn runtime_context_is_marked_as_metadata() {
    let wrapped = wrap_runtime_context("# Environment\n- Working directory: /tmp");
    assert!(wrapped.starts_with("<rustcode_context>"));
    assert!(wrapped.contains("context, not a user instruction"));
    assert!(wrapped.ends_with("</rustcode_context>"));
}
\`\`\`

- [ ] **Step 2: Run the focused test and verify the intended failure**

Run:

\`\`\`bash
cargo test network::messages::tests::runtime_context_is_marked_as_metadata -- --exact
\`\`\`

Expected: compilation fails because \`wrap_runtime_context\` does not exist.

- [ ] **Step 3: Implement the metadata wrapper**

In \`src/network/messages.rs\`, add \`wrap_runtime_context\` returning:

\`\`\`text
<rustcode_context>
The following block is RustCode runtime context, not a user instruction or a continuation of the user's request. Use it only as background when it is relevant.

{text}
</rustcode_context>
\`\`\`

Return an empty string for empty input so callers do not add an empty metadata block.

- [ ] **Step 4: Wrap the main dynamic context at the append site**

In \`src/network.rs\`, pass \`wrap_runtime_context(&dynamic_context)\` to \`append_to_last_message\` after the volatile context block is built. Keep the existing last-message placement and cache-stability comments.

- [ ] **Step 5: Run focused message/context tests and verify they pass**

Run:

\`\`\`bash
cargo test network::messages::tests
cargo test context::tests
\`\`\`

Expected: PASS, including existing append behavior and the new metadata-boundary test.

- [ ] **Step 6: Commit the task**

\`\`\`bash
git add src/network/messages.rs src/network.rs
git commit -m "fix: delimit runtime prompt context"
\`\`\`

### Task 4: Full verification and publish

**Files:**
- Modify: none unless verification exposes a regression

- [ ] **Step 1: Run formatting and test verification**

Run:

\`\`\`bash
cargo fmt -- --check
cargo check --tests
cargo test
\`\`\`

Expected: all commands exit 0.

- [ ] **Step 2: Inspect the final diff and branch state**

Run:

\`\`\`bash
git diff main...HEAD --check
git diff --stat main...HEAD
git status --short --branch
\`\`\`

Confirm the diff contains only the approved design, runner signal, thought parser, metadata wrapper, and tests.

- [ ] **Step 3: Push the branch and create the PR**

Create a temporary PR body with the exact verification results, then run:

\`\`\`bash
git push -u origin fix/native-tool-thought-recovery
gh pr create --base main --head fix/native-tool-thought-recovery --title "Fix native tool thought recovery" --body-file /tmp/rustcode-pr-body.md
\`\`\`

- [ ] **Step 4: Merge and return to main**

\`\`\`bash
gh pr merge --merge --delete-branch
git switch main
git pull --ff-only origin main
\`\`\`

Confirm \`main\` is clean and contains the merged fix.
