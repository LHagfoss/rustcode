# Harness benchmark

A small task-based eval for the rustcode agent loop. Each task is a broken Rust
crate with a known-good fix and a `check.sh` that exits 0 only when solved. The
runner hands each task to `rustcode -p` under a given model and scores it.

## Run

```sh
# default: gemini-3.6-flash vs gemini-3.5-flash-lite
bash bench/run.sh

# specific models (must exist in ~/.config/rustcode/config.toml)
bash bench/run.sh gemini-3.6-flash gemini-3.5-flash-lite qwen3.6-dense

# longer per-task timeout (seconds)
TIMEOUT=300 bash bench/run.sh gemini-3.6-flash
```

Output: a per-run table plus a scorecard — **pass rate · avg time · avg rounds**
per model. Raw rows are written to `bench/last-results.csv`.

## What it measures

- **pass rate** — does the loop actually finish the task
- **rounds** — how many agent turns it took (harness/prompt efficiency)
- **time** — wall-clock per task

## Tasks

| task | fix | check |
|------|-----|-------|
| `01-fix-compile` | make a type error compile | `cargo build` |
| `02-make-test-pass` | fix a wrong function | `cargo test` |
| `03-add-function` | implement `factorial` | `cargo test` |
| `04-change-output` | change printed text | exact stdout match |

Add a task: drop a folder in `tasks/` with `setup/` (the crate), `prompt.txt`,
and an executable `check.sh` (cwd is the crate, exit 0 = solved).

## Caveats

- `-p` drives the **headless `raw_cli` loop** (one tool per round). It shares
  tools, system prompt, and tool protocol with the interactive TUI but not the
  TUI-only machinery (compaction, loop detection, finish gate, parallel tool
  exec). It benchmarks the core loop, not the full harness.
- Tool calls are auto-approved by piping `yes` into the binary.
- Each run creates a session under `~/.config/rustcode/sessions/`.
- Tasks use **no external crates** so `cargo` stays offline and fast.
