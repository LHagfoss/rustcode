# Wakeup Turn Lock Release Design

## Goal

Ensure a queued background wakeup either reaches provider request startup or
remains observable and recoverable by the existing watchdog, without allowing
context/schema preparation to monopolize the `AppState` mutex.

## Evidence and Root-Cause Hypothesis

Issue #1279's recorded session stopped after `turn.context_diagnostics` and
before `mcp.native_schema_selection`, `provider.request_start`, or
`turn.summary`. On current `main`, both request preparation and provider
startup call native schema selection while holding the `AppState` mutex.
Native schema selection can synchronously traverse the workspace to determine
the schema phase and can inspect the MCP registry and construct provider
schemas. The second request projection occurs after context diagnostics and
before the provider-start event, matching the observed boundary.

The exact session directory is not present in the local configuration, so the
fix must be validated with a deterministic seam that pauses this projection
and observes whether another task can still acquire `AppState`.

## Scope

In scope:

- native schema snapshot/compute/commit ownership in request preparation and
  provider startup;
- stale-session and MCP-generation checks when committing sticky schema state;
- a test-only deterministic schema gate;
- a wakeup-turn regression test using a local streaming provider;
- lifecycle evidence needed by the regression test.

Out of scope:

- changing watchdog timeout or recovery policy;
- changing queue ordering, cancellation semantics, or orchestrator lease rules;
- changing the provider payload, schema ranking, or MCP selection policy;
- changing production filesystem traversal behavior beyond its lock boundary.

## Design

### Snapshot, compute, commit

`PromptCache` remains the owner of sticky MCP selection metadata, but schema
selection is split into three operations:

1. Under a short `AppState` lock, capture the active session id, MCP
   generation, user-message count, selection policy, and sticky selected names.
2. After releasing the lock, compute the phase, built-in schemas, MCP schemas,
   relevance ranking, and provider-compatible schema values from those inputs.
3. Reacquire `AppState` and commit the selected names only if the active
   session, MCP generation, policy, and user-message count still match the
   snapshot. Otherwise discard the cache mutation and retain the computed
   schemas for this request; the next request will rebuild from current state.

The compute phase must not borrow `AppState` or hold its mutex. This makes
workspace traversal, registry inspection, and JSON schema construction
independent of UI rendering, watchdog checks, cancellation handling, and
queue submission.

The existing schema selection algorithm and returned statistics remain
unchanged. The commit guard prevents a stale wakeup or a concurrent session
switch from overwriting newer sticky selection state.

### Request paths

`prepare_turn_request_with_checkpoint_and_prefix_cache` uses the same
snapshot/compute/commit helper for both of its native-schema projections.
`stream_request` uses it for the final provider payload. Each path keeps its
existing session snapshot and provider request arguments; only the lock
duration and cache update boundary change.

### Lifecycle and watchdog behavior

The existing `turn.context_diagnostics`, `mcp.native_schema_selection`, and
`provider.request_start` events remain in their current semantic positions.
No watchdog deadline or queue recovery behavior changes. Releasing the mutex
allows the runtime loop to observe stale work and emit the existing
`turn.stall_recovered` event if a future failure occurs.

## Deterministic Regression Test

Add a test-only schema preparation gate that can pause a selected schema
projection and signal when it is entered. The regression test will:

1. configure an `AppState` with an API-native local model and a queued
   `__task_wakeup__:` prompt;
2. run the real queue orchestrator against a local streaming HTTP provider;
3. pause the schema projection corresponding to the post-diagnostics,
   pre-provider handoff;
4. assert that a separate task can acquire `AppState` while the projection is
   paused;
5. release the gate and assert that the local provider receives the request;
6. assert that the wakeup turn drains normally and does not require queue or
   watchdog changes.

The test must fail on the pre-fix code because the schema gate holds the
`AppState` guard across the synchronous projection, preventing the lock
acquisition. It must pass after the fix while still exercising the real
orchestrator, schema selection, request assembly, and provider startup path.

Additional cache-commit coverage will exercise a session or MCP-generation
change while computation is paused and assert that stale sticky names are not
committed.

## Acceptance Criteria

- A paused schema computation does not prevent another task from acquiring
  `AppState`.
- The deterministic wakeup regression observes the local provider request
  after the gate is released.
- Existing normal-turn, cancellation, queue, provider, and watchdog tests
  remain green.
- No production await or synchronous schema/filesystem work is performed
  while holding the `AppState` mutex in the changed paths.
- The patch remains limited to schema ownership/request assembly and the
  regression instrumentation/tests.
