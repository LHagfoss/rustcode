# Task: Split `src/network.rs` into submodules + harden the agent loop

## Context — current implementation

`rustcode` is a terminal AI coding agent (Rust). The networking + agent
orchestration lives in `src/network.rs`, which has grown to **3083 lines** —
too large for one file.

A sibling `src/network/` folder already exists and is wired up as submodules of
the `network` module (declared in `network.rs`):

- `src/network/compaction.rs` (203 loc) — token estimation + history summarization
- `src/network/retry.rs` (99 loc) — transient-failure retry policy for LLM requests
- `src/network/loop_detect.rs` (294 loc) — 4-signal repetition detector
  (exact / category / output-stagnation / frequency)

So `network.rs` + `network/*.rs` = idiomatic Rust module layout. The job is to
move most of `network.rs` into new sibling submodules, leaving `network.rs` as a
thin parent that only declares `mod`s and holds small glue.

### What's inside `network.rs` today (function map)

- **History/context management**: `count_tokens`, `classify_tool_msg`,
  `is_fully_stubbed`, `truncate_tool_output`, `save_full_tool_output`,
  `view_file_path_from_tool_msg`, `dedupe_view_file_reads`, `reduce_tool_msg`,
  `prune_class`, `compact_history_to_budget`, `estimate_msg_chars`,
  `trim_msgs_to_budget`, `inject_system_reminder`
- **Model / context window**: `estimate_token_usage`,
  `context_length_from_model_info`, `fetch_context_window`
- **Streaming / SSE**: `parse_sse_line`, `StreamBuffer`,
  `align_alternating_messages`, `stream_request` (large),
  `has_intended_tool_call`, `is_cut_off`, `strip_think_blocks`,
  `is_reasoning_only`
- **Tool execution helpers**: `is_read_only_tool`,
  `view_file_unchanged_since_last_read`, `path_mtime`, `tool_signature`,
  `get_diff_preview`, `get_tool_project_root`, `strip_ansi_escapes`,
  `run_compiler_check`, `confirm_and_execute`, `push_status_line`,
  `strip_leading_think`
- **Subagents / agent tools**: `run_subagent`, `handle_agent_tool`,
  `evaluate_and_expand_prompt`, `generate_title`
- **Orchestrator**: `process_queue_orchestrator` — a single ~840-line function
  (roughly lines 1858–2695) that drives the whole tool-call loop
- **Misc**: `parse_multimodal_content`

## Goal 1 — split into submodules

Move code into these new files under `src/network/`, building after each move:

| New file | Contents |
|---|---|
| `network/history.rs` | all History/context-management fns above |
| `network/model_info.rs` | model / context-window fns |
| `network/stream.rs` | streaming/SSE fns + `StreamBuffer` |
| `network/exec.rs` | tool-execution helper fns + `confirm_and_execute` |
| `network/agent.rs` | subagent / agent-tool fns + `generate_title` |
| `network/orchestrator.rs` | `process_queue_orchestrator` |

`network.rs` keeps only: `mod`/`pub(crate) mod` declarations, any re-exports
callers rely on, `parse_multimodal_content`, and minimal glue. Target ≤ ~250 loc.

### Rules
- Pure move-and-rewire. **No behavior change** in Goal 1.
- Fix visibility as needed (`pub(crate)`) and update `use` paths. Prefer keeping
  the public surface identical so callers outside `network` don't change.
- Move each item's doc comments with it.
- Run `cargo build` after **every** submodule extraction; do not proceed while
  broken. Run `cargo test` and `cargo clippy` at the end — must be clean.
- Keep the `#[cfg(test)] mod tests` blocks next to the code they cover (move
  tests into the submodule that now owns the function).

## Goal 2 — harden the agent loop (separate commit, after the split)

A real session got stuck: a capable model kept re-issuing the same read-only
tool calls (`view_file`, `glob`) and **never produced a prose answer**, even
though it had all the info it needed. The dedup layer kept returning "already ran
this recently" rejections (no new info → guaranteed spin), and `loop_detect`'s
Abort just stopped the run without letting the model answer. This lives in
`process_queue_orchestrator` (now `orchestrator.rs`).

Implement:

1. **Force a tool-less final turn on Abort.** Instead of bare `break` on
   `LoopStatus::Abort`, make one final model call with tools stripped from the
   request and a directive like: *"Tools are disabled. Using what you have
   already gathered, answer the user directly in prose now."* Push that answer to
   history, then stop. Converts a dead loop into a real answer.

2. **Escalate on all-repeat rounds.** If an entire tool batch is 100%
   dedup-rejected (no productive tool actually ran), count it; after 2 such
   rounds, trigger the same tool-less answer turn instead of waiting for the
   abort=6 threshold.

3. **Fix the dedup rejection copy.** Current text references "a file you just
   edited" but the model only *read*. Replace with something like: *"Already read
   this; its contents are in the context above. Stop searching and answer the
   user, or take a different action."*

Add/extend unit tests for the loop-detect + dedup behavior where practical.

## Deliverables
- Commit 1: the module split (green build/test/clippy, no behavior change).
- Commit 2: the loop-hardening changes with tests.
- Do not add AI-attribution footers to commits.
