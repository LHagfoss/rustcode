# Measured performance and validity

`/perf` and session `performance.json` report measured microseconds, requests,
rounds, tool calls, context bytes, replays and supplied token usage. Provider
wall time includes network waiting. Tool work is summed execution time and may
overlap; harness time subtracts provider and tool wall time, never tool work.
TTFT observes generated deltas. Persistence time measures enqueue work, not
flush latency. Missing provider facts remain unavailable. The report is not a
correctness score. Use `rustcode bench --report file --baseline baseline`.

The scheduler overlaps contiguous distinct safe built-in inspections, at most
four. Unknown MCP tools, shell commands, mutations and control calls are serial
barriers. Results retain call order. Cancellation waits for blocking read cleanup.

Workspace snapshots stat their known manifest and directory topology, avoiding
repeat recursive discovery when unchanged. Symbols update changed files.
Instruction/environment caches include workspace and Git state. Explicit writes
invalidate changed paths; unbounded commands invalidate conservatively. External
edits and new/deleted paths refresh generations. Persisted evidence carries a
process UUID epoch as well as generation and body hash.

Under context pressure, old inspection evidence becomes a reacquisition stub;
requirements, failures, mutations and native call/result structure remain.
Durable history is preserved. Full reads can be reacquired after eviction.
Verification receipts require a complete successful result with stable root,
command, generation, environment, checker configuration and sandbox identity.
Failures, cancellation, partial output and unsafe external inputs never seed
receipts; resume starts without verification receipts.

## Recovering compacted context

Compaction saves the exact removed messages in content-addressed JSONL archives.
The read-only `zoom_context` tool navigates archives linked to the active session:

1. Call `zoom_context` with `{}` to list message previews and obtain `root`.
2. Open a message with `{"root":"<returned root>","message":2}`. Continue with
   the returned `next_offset` until it is null; offsets count UTF-8 bytes.
3. Follow a summary's `child_path`, for example
   `{"root":"<returned root>","path":[1]}`, to list an earlier compaction.
4. Continue an inventory using its `next_start`. Message numbers start at 1
   within the selected archive, not within the whole session.

Each inventory contains at most eight previews. Each message page contains at
most 2 KiB of text, and the complete tool response is capped at 16 KiB. Native
tool-call arguments and tool-result metadata are included when present. Listings
and message slices are partial evidence; a preview is never a complete read.
Older summaries without an archive link cannot be expanded. A changed `root`
requires a fresh listing, so callers should pass it on every follow-up.

Navigation follows typed compaction links, never caller-supplied filesystem
paths or session IDs. Archives must be regular files in the archive directory
with matching SHA-256 addresses. A call traverses at most 32 earlier compactions,
reads at most 16 MiB per archive and 64 MiB in total, and fails explicitly on
missing, corrupt, malformed or oversized data.

Recalled messages describe historical conversation data. They do not prove the
current state of a file or command. Recall appends ordinary tool results without
rewriting history, rebuilding summaries or making background model calls. Tools
remain fixed during a turn and changing runtime context remains at the request
tail. These properties preserve prefix reuse; actual cache hits, cost and latency
still depend on the provider and must be measured from its usage reports.

This adopts on-demand recovery from the
[UniiChat design](https://gist.github.com/VictorTaelin/91837951a5ce5b38f341ec1ba1df6449),
using RustCode's existing compaction links rather than its full summary tree.

## Local fixture measurements

Debug tests use fresh temporary roots; these are not provider benchmarks.

| Workload | Baseline | Final |
| --- | ---: | ---: |
| 16 inspections, serial versus bounded batch | 36.5 ms | 20.3 ms |
| 20 refresh/query passes, 1,000 Rust files | 351 ms | 89 ms |
| 20 successful checks, minimal Cargo package | 524 ms | 36 ms |

Cold symbol refresh: 667 ms baseline, 44 ms final. Cold verification: 176 ms
baseline, 175 ms final. First Cargo.lock creation changes generation and cannot
seed a receipt. Commands:

```sh
cargo test --lib inspection_batch_baseline -- --ignored --nocapture
cargo test --lib benchmark_repeated -- --ignored --nocapture
```

See [transcript selection measurements](benchmarks/tui-selection.md).

The recovered Spotify task originally tried MCP discovery, grep and shell
commands before its explicit-skill followup. A fresh natural-language run now
called list_skills, use_skill and run_command, but Spotify authentication was
expired without a refresh token. The original deepseek-flash profile was absent
and the run used the configured default; same-provider success/round comparison
is therefore unverified. No task-success improvement is claimed from that run.
