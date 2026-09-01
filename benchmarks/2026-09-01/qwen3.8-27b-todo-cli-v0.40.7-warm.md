# Qwen3.8 27B todo CLI — Rustcode v0.40.7, warm cache

## Outcome

**Harness result: incomplete.** Rustcode stopped at the maximum 40 tool rounds before observing the last test result or producing a final answer. The generated project was nevertheless complete: an independent post-run check passed 18/18 tests (39 assertions) and `bunx tsc --noEmit`.

This run is retained as a failure rather than replaced by a favorable rerun.

## Setup

| Field | Value |
|---|---|
| Date | 2026-09-01 |
| Rustcode | v0.40.7, release commit `f7385717` |
| Relevant fix | PR #911 / issue #910 |
| Profile | `qwen-3.8` |
| Model | `Qwen3.8-27B-MTPLX-4bit` |
| Server | remote oMLX at `tokmax.paral.no` |
| SpecPrefill | enabled (profile/server default) |
| Cache policy | warm production-style; retained across runs and turns |
| Workspace | `/tmp/rustcode-qwen38-v0407-final2` |
| Session | `1788265553321` |

The first v0.40.7 launch (`1788265484497`) was stopped before any tool call because the model incorrectly assumed the working directory was the home directory. It is excluded from metrics.

## Prompt

> Build and fully verify a small todo-list CLI in this empty directory. Use bun init, TypeScript, and Bun SQLite. Implement add, list, complete, and delete commands with persistent SQLite storage. Add automated tests for all operations, run them, fix any failures, run TypeScript checking, and run a CLI smoke test. Do not stop until verification passes; summarize exact results.

## Metrics

| Metric | Result |
|---|---:|
| Provider responses | 40 |
| Tool calls / results | 42 / 42 |
| Prompt tokens | 991,093 |
| Completion tokens | 15,099 |
| Cached tokens reported by provider | 40,960 |
| Rustcode summed response time | 4,233.002 s |
| oMLX summed server completion time | 2,100.85 s |
| Observed wall interval, first to last oMLX completion | about 35m 10s |
| Rustcode thought time | 293.396 s |
| Failed tool results | 6 |
| Terminal condition | maximum tool rounds reached (40) |

The oMLX totals match Rustcode exactly for 40 responses, 991,093 prompt tokens, and 15,099 completion tokens. The Rustcode response-time sum is not wall time in this run and should not be used as elapsed duration; the oMLX timestamp interval is the comparable elapsed measure.

## Behavior observed

The model initially wrote a working project, but repeatedly interpreted bounded or truncated model-facing tool output as proof that complete files on disk were corrupt. It repeatedly read the same files and re-described the same output mismatches. Later it made progress, reduced the test suite to one failure, fixed that final behavior, and requested another test run, but the 40-round backstop stopped the turn before execution.

Rustcode did correctly emit bounded protections for repeated smoke commands and equivalent failed edits. The v0.40.7 verification-to-summary false-positive fix was not exercised because the run never reached a completed verification-to-summary transition.

oMLX decode performance fell from roughly 47–59 tokens/s in the short successful runs to mostly 20–25 tokens/s after the context grew. Several MTP generations parked and re-entered, including one 4,164-token response that took 247.53 seconds. The excessive trajectory, not transport overhead, dominated runtime.

## Independent verification

After Rustcode stopped:

```text
bun test: 18 pass, 0 fail, 39 assertions
bunx tsc --noEmit: exit 0, no diagnostics
```

The generated project therefore succeeded as code, but the agent harness failed the requested completion contract because it exhausted its round budget without reporting completion.

## Comparison

Earlier v0.40.6 warm runs completed in approximately 293–294 seconds with 11–12 provider responses and 12 tool calls. SpecPrefill on and off differed by about 0.5%, which is run noise. This v0.40.7 run is not a speed regression measurement: its divergent 40-round trajectory makes it a reliability failure and consumes more than seven times the wall time.

## Follow-up

Investigate model-facing truncation semantics and loop detection for repeated claims that a complete file is corrupt. A complete tool result whose rendered content is intentionally bounded must make the truncation explicit and should direct the model to request a specific range, while repeated unchanged reads caused by that misunderstanding should trigger grounded recovery well before 40 rounds.
