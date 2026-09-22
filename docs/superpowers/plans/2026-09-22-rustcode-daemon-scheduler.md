# RustCode Daemon Scheduler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a local `rustcode daemon` that owns durable harness-managed scheduled jobs and exposes matching CLI controls.

**Architecture:** Add a daemon module with SQLite-backed jobs/runs, a local request/response control API, and an event-driven scheduler loop. The harness calls one built-in `manage_scheduled_jobs` tool through the same API used by `rustcode cron`; the executor dispatches MCP calls, headless prompts, shell commands, and bounded polling with leases, retries, and run history.

**Tech Stack:** Rust 2024, Tokio, SQLite through the existing bundled `rusqlite` dependency, `chrono`/`chrono-tz` for civil time, the existing MCP client, headless turn runner, command policy, Clap CLI, and serde JSON.

**Spec:** `docs/superpowers/specs/2026-09-22-rustcode-daemon-scheduler-design.md`

## Global Constraints

- `rustcode daemon` is the sole scheduler owner; CLI and harness are clients of the same local control API.
- Scheduler state is durable SQLite data under the RustCode config directory.
- Persist UTC instants while retaining the original IANA timezone and civil-time schedule.
- Default recurring-job misfire behavior is `skip_missed`; one-shot jobs remain due until claimed or deleted.
- Never automatically replay an ambiguous external MCP or shell side effect after a crash.
- Scheduled jobs must not interleave with the active TUI session unless explicitly targeted.
- All scheduled mutations created by the harness use normal user-visible tool confirmation.
- First implementation includes `mcp_call`, `prompt`, `shell_command`, and bounded `poll` actions.
- Local daemon transport is a `0600` Unix domain socket first; keep transport behind an abstraction for later Windows support.
- Run `cargo check --tests` and `cargo test` before completion.

## Review Focus

- A daemon restart between lease claim and external side effect must produce an inspectable ambiguous run and no automatic duplicate action; Task 3 tests this with a crash-state fixture.
- A DST spring-forward nonexistent local time and fall-back ambiguous local time must calculate deterministic next occurrences; Task 1 tests both.
- A stale daemon completion must not overwrite a newer fenced claim; Task 1 tests the transaction rejection.
- A job created or changed while the scheduler sleeps must wake it before the old deadline; Task 3 tests the notification path.
- A scheduled prompt must use its recorded workspace/config rather than the daemon process current directory; Task 4 tests workspace propagation.

## File Map

- Create `src/daemon/mod.rs`: public daemon module wiring, request dispatch, and shared daemon error types.
- Create `src/daemon/model.rs`: serializable job/action/run types and validation.
- Create `src/daemon/schedule.rs`: cron/structured schedule parsing and timezone-aware next-occurrence calculation.
- Create `src/daemon/store.rs`: SQLite schema, migrations, CRUD, leases, and run settlement.
- Create `src/daemon/protocol.rs`: local wire request/response enums and bounded framing.
- Create `src/daemon/client.rs`: CLI/harness client for the local daemon API.
- Create `src/daemon/lifecycle.rs`: registration, PID/start-time checks, detached start/stop/status, and foreground ownership.
- Create `src/daemon/server.rs`: Unix socket listener and request handling.
- Create `src/daemon/scheduler.rs`: wakeup loop, due-job claiming, and dispatch coordination.
- Create `src/daemon/executor.rs`: action execution, timeout/cancellation, retry classification, and concurrency gates.
- Modify `src/cli.rs`: `daemon` and `cron` subcommands with typed arguments.
- Modify `src/lib.rs`: route daemon/cron commands before TUI/headless startup.
- Modify `src/raw_cli.rs`: construct a headless state from explicit workspace/session/model settings.
- Modify `src/mcp.rs`: start/resolve MCP servers against a recorded workspace/config context.
- Modify `src/tools/misc.rs`: add the `manage_scheduled_jobs` built-in tool and schema.
- Modify `src/tools/mod.rs`: register the new built-in tool if the existing inventory requires a direct entry.
- Modify `Cargo.toml` and `Cargo.lock`: add only the timezone/cron parsing dependency required by the implementation.

### Task 1: Schedule domain and SQLite job store

**Files:**
- Create: `src/daemon/model.rs`
- Create: `src/daemon/schedule.rs`
- Create: `src/daemon/store.rs`
- Create: `src/daemon/mod.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`

**Interfaces:**
- Produces `JobAction`, `ScheduleSpec`, `MisfirePolicy`, `RetryPolicy`, `JobRecord`, `JobRunRecord`, `JobStore`, and `ScheduleSpec::next_after(DateTime<Utc>)` for later tasks.
- `JobStore::create`, `list`, `get`, `set_paused`, `delete`, `claim_due`, `settle_run`, and `history` are the only scheduler-state entry points later tasks use.

- [ ] **Step 1: Write failing schedule tests.** Add tests for a daily `08:00` Europe/Oslo schedule, a monthly schedule, invalid cron expressions, spring-forward adjustment, fall-back single firing, and `skip_missed` versus `run_once` policy validation.
- [ ] **Step 2: Run the focused tests and verify the expected missing-type/compiler failures.** Run `cargo test daemon::schedule -- --nocapture`; expected failure is unresolved schedule types/functions, not an unrelated dependency failure.
- [ ] **Step 3: Implement the typed schedule model.** Add serde-tagged action types and schedule validation. Use `cron::Schedule` plus `chrono_tz::Tz`; normalize persisted times to UTC and explicitly choose the documented DST behavior.
- [ ] **Step 4: Write failing SQLite store tests.** Cover schema initialization, create/list, pause/resume, delete, due-job claim, duplicate claim rejection, lease expiry, fenced settlement rejection, and bounded run-history output.
- [ ] **Step 5: Run store tests and verify they fail for absent persistence behavior.** Run `cargo test daemon::store -- --nocapture`; expected failure is missing `JobStore` behavior.
- [ ] **Step 6: Implement the SQLite store.** Create `jobs` and `job_runs` tables transactionally, use a schedule revision and lease fence, store JSON action payloads, and return domain errors rather than leaking raw SQLite errors.
- [ ] **Step 7: Run all Task 1 tests and the package check.** Run `cargo test daemon:: -- --nocapture` and `cargo check --tests`; expected result is all new domain/store tests passing with no warnings introduced.
- [ ] **Step 8: Commit Task 1.** Run `git add Cargo.toml Cargo.lock src/daemon` and commit `feat: add durable scheduler job store`.

### Task 2: Local daemon protocol and lifecycle

**Files:**
- Create: `src/daemon/protocol.rs`
- Create: `src/daemon/client.rs`
- Create: `src/daemon/lifecycle.rs`
- Create: `src/daemon/server.rs`
- Modify: `src/daemon/mod.rs`

**Interfaces:**
- Consumes Task 1 `JobStore` and domain types.
- Produces `DaemonRequest`, `DaemonResponse`, `DaemonClient`, `DaemonRegistration`, `DaemonLifecycle::{start, run, stop, status}`, and a request handler that the scheduler and CLI share.

- [ ] **Step 1: Write failing protocol tests.** Test JSON request/response round trips, bounded frame size, malformed request rejection, and response error serialization.
- [ ] **Step 2: Run protocol tests and confirm the missing protocol implementation failure.** Run `cargo test daemon::protocol -- --nocapture`.
- [ ] **Step 3: Implement protocol framing and domain requests.** Define requests for daemon status, job CRUD/control, history, and run-now; use newline-delimited JSON with a maximum frame size and no shell command interpolation.
- [ ] **Step 4: Write failing lifecycle tests.** Test atomic registration publication, stale registration cleanup, status on a live process, and client failure when the socket is absent.
- [ ] **Step 5: Run lifecycle tests and verify the expected missing behavior.** Run `cargo test daemon::lifecycle -- --nocapture`.
- [ ] **Step 6: Implement the Unix local transport.** Add a `0600` Unix socket, foreground server loop, bounded request handling, registration file with PID/start time/instance ID, and detached `start` that waits for health. Reject stale PID reuse before stop.
- [ ] **Step 7: Run protocol/lifecycle tests and `cargo check --tests`.** Confirm socket cleanup and no leaked listener tasks in test teardown.
- [ ] **Step 8: Commit Task 2.** Commit `feat: add rustcode daemon control protocol` with only daemon protocol/lifecycle files.

### Task 3: Event-driven scheduler loop and run lifecycle

**Files:**
- Create: `src/daemon/scheduler.rs`
- Create: `src/daemon/executor.rs`
- Modify: `src/daemon/store.rs`
- Modify: `src/daemon/server.rs`
- Modify: `src/daemon/mod.rs`

**Interfaces:**
- Consumes `JobStore`, `ScheduleSpec`, daemon requests, and a new executor trait `JobExecutor::execute(JobRunContext) -> Future<RunOutcome>`.
- Produces `SchedulerHandle` with `notify_changed`, `shutdown`, and `run`; server mutations call `notify_changed` rather than touching scheduler internals.

- [ ] **Step 1: Write failing scheduler tests with an injected clock/executor.** Cover immediate due dispatch, earliest-deadline sleep, wake-on-create, pause before claim, run-now, no overlapping same-session runs, and shutdown cancellation.
- [ ] **Step 2: Run scheduler tests and verify failure before implementation.** Run `cargo test daemon::scheduler -- --nocapture`.
- [ ] **Step 3: Implement the scheduler loop.** Re-read durable state after each wake, use `tokio::time::sleep_until` raced against `Notify` and cancellation, claim due jobs transactionally, and dispatch bounded concurrent runs.
- [ ] **Step 4: Write failing retry/crash tests.** Cover transient backoff, permanent failure advancing the recurrence, expired lease recovery, and ambiguous run settlement that cannot be auto-replayed.
- [ ] **Step 5: Implement settlement and retry policy.** Persist attempt number, next retry, error class, run output, and schedule revision. Ensure a stale fence receives a domain conflict and cannot mutate the newer run.
- [ ] **Step 6: Wire server mutations to scheduler notifications.** Job create/update/delete/run-now must wake the loop immediately; status must report next deadline and active runs.
- [ ] **Step 7: Run all daemon tests and `cargo test`.** Use fresh output and inspect every failure before continuing.
- [ ] **Step 8: Commit Task 3.** Commit `feat: add durable event-driven scheduler loop`.

### Task 4: MCP, prompt, shell, and bounded-poll execution

**Files:**
- Modify: `src/mcp.rs`
- Modify: `src/raw_cli.rs`
- Modify: `src/daemon/executor.rs`
- Create: `src/daemon/action_tests.rs`

**Interfaces:**
- Consumes `JobRunContext` and `JobAction` from Tasks 1 and 3.
- Produces an executor implementation that returns bounded `RunOutcome` values and records whether an external side effect may be ambiguous.

- [ ] **Step 1: Write failing executor tests.** Test a Teams-style direct MCP call with a mock client, explicit workspace propagation for prompts, shell timeout/policy rejection, and poll interval/max-run/stop-on-change behavior.
- [ ] **Step 2: Run executor tests and verify expected missing dispatch behavior.** Run `cargo test daemon::action -- --nocapture`.
- [ ] **Step 3: Refactor MCP startup to accept job workspace/config context.** Preserve existing startup timeout and per-server failure isolation; expose a narrow direct `tools/call` helper without exposing the registry mutably.
- [ ] **Step 4: Extend headless state construction.** Add a constructor that loads a recorded session/workspace/model and starts MCP servers using that workspace instead of `current_dir()`.
- [ ] **Step 5: Implement action dispatch.** Route direct MCP calls through `McpClient`, prompts through the existing headless turn runner, shell commands through existing command policy, and polls through bounded repeated direct actions with canonical-result hashing.
- [ ] **Step 6: Implement cancellation and ambiguity classification.** Stop at action boundaries, preserve timeout/error output, and mark external actions ambiguous when process death can leave their result unknown.
- [ ] **Step 7: Run action tests, `cargo check --tests`, and `cargo test`.** Confirm no existing headless/MCP tests regress.
- [ ] **Step 8: Commit Task 4.** Commit `feat: execute daemon MCP and headless jobs`.

### Task 5: Harness management tool

**Files:**
- Modify: `src/tools/misc.rs`
- Modify: `src/tools/mod.rs`
- Modify: `src/daemon/client.rs`
- Create: `src/tools/scheduled_jobs_tests.rs`

**Interfaces:**
- Consumes `DaemonClient` and the Task 1 action/schedule types.
- Produces built-in tool `manage_scheduled_jobs` with actions `create`, `list`, `pause`, `resume`, `run`, `history`, and `delete`.

- [ ] **Step 1: Write failing schema/handler tests.** Cover typed create arguments, required fields, compact list/history output, unknown job errors, and confirmation metadata for mutations.
- [ ] **Step 2: Run focused tool tests and verify they fail before registration.** Run `cargo test tools::scheduled_jobs -- --nocapture`.
- [ ] **Step 3: Implement the tool schema and handler.** Translate structured daily/monthly forms and cron expressions into daemon requests; never create shell sleep loops or write scheduler files directly.
- [ ] **Step 4: Register the tool in the built-in inventory and add bounded output.** Ensure the model can discover the tool without an MCP server and that history cannot flood context.
- [ ] **Step 5: Run tool tests and the full suite.** Confirm existing tool inventory/schema tests remain green.
- [ ] **Step 6: Commit Task 5.** Commit `feat: add harness scheduled jobs tool`.

### Task 6: CLI commands and application routing

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/lib.rs`
- Modify: `src/daemon/client.rs`
- Modify: `src/daemon/lifecycle.rs`
- Modify: `src/cli.rs` tests

**Interfaces:**
- Consumes the daemon client/lifecycle API and uses the same request variants as the harness tool.
- Produces `daemon start|run|stop|status|logs` and `cron add|list|pause|resume|run|history|delete` parsing and dispatch.

- [ ] **Step 1: Write failing Clap parsing tests.** Cover daemon subcommands, cron action arguments, explicit timezone, action JSON, output format, and invalid combinations.
- [ ] **Step 2: Run CLI tests and verify missing variants.** Run `cargo test cli:: -- --nocapture`.
- [ ] **Step 3: Implement typed CLI commands.** Keep `cron add` as a daemon client; support JSON output for scripting and human-readable bounded output by default.
- [ ] **Step 4: Route commands before interactive/headless startup.** `daemon run` must not initialize the TUI; cron operations must not start MCP servers or a model turn in the CLI process.
- [ ] **Step 5: Add status/log output and exit codes.** Distinguish invalid input, daemon unavailable, job not found, and execution failure.
- [ ] **Step 6: Run CLI tests, `cargo check --tests`, and `cargo test`.**
- [ ] **Step 7: Commit Task 6.** Commit `feat: add daemon and cron CLI controls`.

### Task 7: Integration verification and documentation

**Files:**
- Create: `docs/daemon.md`
- Modify: `docs/background-tasks.md`
- Create: `src/daemon/integration_tests.rs`

**Interfaces:**
- Consumes the complete daemon/client/tool/CLI surface from Tasks 1–6.
- Produces documented setup examples and regression coverage for the original Teams polling scenario.

- [ ] **Step 1: Write an integration test for the original use case.** Create a daily Teams-style direct MCP job, list it through the client, run it immediately through the daemon, and verify one recorded run with no shell sleeps or model-poll calls.
- [ ] **Step 2: Write an integration test for restart recovery.** Persist a due job, stop the scheduler before dispatch, restart it, and verify one claim/run with no duplicate run ID.
- [ ] **Step 3: Implement only the test harness/documentation needed to make those tests executable.** Keep external Teams credentials out of tests; use a mock MCP JSON-RPC process.
- [ ] **Step 4: Document daemon lifecycle, CLI commands, harness tool usage, Teams direct-MCP example, misfire policy, and ambiguous-run recovery.**
- [ ] **Step 5: Run the mandatory full verification.** Run `cargo check --tests` and `cargo test`; inspect exit codes and failure counts.
- [ ] **Step 6: Commit Task 7.** Commit `docs: document rustcode daemon scheduler`.

## Plan self-review

- Spec coverage: daemon lifecycle (Tasks 2/6), durable store and leases (Task 1), scheduling/wakeup (Task 3), action execution (Task 4), harness control (Task 5), CLI control (Task 6), observability and use-case regression (Task 7), and required verification (Task 7).
- Placeholder scan: no unresolved implementation placeholders; every task names files, interfaces, tests, commands, and commit messages.
- Interface consistency: Task 1 defines the domain/store types consumed by Tasks 2–6; Task 2 defines protocol/client types consumed by Tasks 5–6; Task 3 defines executor/scheduler boundaries consumed by Task 4; later tasks only extend those named interfaces.
- Review focus coverage: restart ambiguity (Tasks 1/3/7), DST (Task 1), stale fencing (Task 1), wake notification (Task 3), and workspace propagation (Task 4) each have an explicit test step.
