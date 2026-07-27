# rustcode — agent conventions

Read this before editing. It captures invariants that are NOT visible from any
single file and that `cargo check` cannot catch (deadlocks, event-loop rules,
concurrency). When adding code, **mirror how the nearest sibling code already
does the same thing** — matching signatures, locking, and error handling —
instead of inventing a new pattern.

## Architecture (where things live)
- `src/main.rs` — the TUI event loop. Reads key/paste/mouse events, drains the
  pending queue, and spawns the agent orchestrator. One async loop; never block it.
- `src/app/actions.rs` — `handle_enter` and all `/slash` command handlers.
- `src/app/state.rs` — `AppState` (the single shared state) and its methods.
- `src/network.rs` — `process_queue_orchestrator` (the agent turn loop),
  `stream_request` (LLM streaming), the compile-check finish gate, token pruning.
- `src/config.rs` — `ModelProfile`, `config.toml` load/save. This is a LOCAL-LLM
  CLI: models are OpenAI-compatible endpoints (Ollama, proxies, etc). It is NOT a
  cloud/Cloudflare/Workers project — do not add provider-specific defaults.
- `src/ui/mod.rs` — rendering (chat, input). Has a render cache; keep it in mind.
- `src/tools/` — `filesystem.rs` (view/edit tools), `exec.rs` (`run_command`),
  `mod.rs` (tool parsing + the system prompt).

## Concurrency — the #1 source of hard bugs
`AppState` lives in `Arc<Mutex<AppState>>` (tokio Mutex). The rules:

- **`handle_enter` already holds the lock.** It does `let mut s = state.lock().await`
  at the top, and holds that guard across the entire slash-command match. Every
  handler therefore receives the **guard**, e.g. `check_memory_usage(&mut s)`,
  `start_new_session(&mut s)`, `trigger_quota_fetch(&s, …)`.
- **NEVER call `state.lock().await` from inside a handler** (or any code reached
  while a guard is live). Re-locking the same tokio Mutex deadlocks instantly and
  freezes the whole UI. `cargo check` will NOT catch this.
- **For async/streaming work that needs the state, drop the guard and spawn.**
  Pattern (see the `/summarize` handler): clear input, `drop(s)`, then
  `tokio::spawn(async move { … })` with an `Arc::clone(state)`. Do not run a full
  `stream_request` inline in `handle_enter` — it blocks the event loop.
- **Do not hold the guard across an `.await`** unless you are certain nothing under
  that await re-locks.
- The orchestrator is single-flight: spawns are gated on `AppState.orchestrator_running`,
  not on `status == Idle`. If you add a new spawn site, gate it the same way.

## Editing
- Before a `replace_file_content`, `view_file` (or `grep`) the exact region so your
  `target_content` matches byte-for-byte. If an edit reports "target_content not
  found", re-read the current lines — don't retry the same string.
- After edits, the harness runs a compile gate. If it reports errors, fix them
  before doing anything else. If it reports `__BUILD_UNVERIFIED__`, the checker
  couldn't run — verify manually with `run_command: cargo check`.

## Build / test
- Build: `cargo build --release`. Check: `cargo check`. Test: `cargo test`.
- The installed binary is a symlink at `/opt/homebrew/bin/rustcode`; a running
  process must be restarted to pick up a new build.
- Do not commit or push unless explicitly asked.

## Tool protocol
- `tool_protocol` in config is `json` | `native` | `apinative`. Text protocols emit
  a single ```tool fenced JSON block per call; emit up to 2 for parallel calls.
  Never use ```tool_code or ```json fences.
