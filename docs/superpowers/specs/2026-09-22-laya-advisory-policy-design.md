# Design: Opt-in self-hosted Laya policy assistance

Date: 2026-09-22
Status: conversational design approved; awaiting written-spec review
Branch: `feature/laya-policy`

## Summary

Add an optional, local Laya decision service to RustCode as an advisory layer for
shell safety and repetitive read-only work. RustCode's deterministic guards remain
the default and remain authoritative for explicit destructive operations, plan-mode
restrictions, and all failures. Users can opt in to shadow evaluation or to a
narrowly relaxed mode. The first implementation uses a persistent Python
`laya-mlx` sidecar on Apple Silicon rather than binding MLX directly from Rust.

The feature is off by default. With the feature disabled, RustCode must behave as
it does today, including its current guard and repetition behavior.

## Goals

1. Provide an entirely local, self-hosted option based on Laya-MLX; no hosted Jev
   or TypeSafe API is required at runtime.
2. Preserve RustCode's existing safety behavior when Laya is disabled,
   unavailable, uncertain, malformed, timed out, or below the configured
   confidence threshold.
3. Reduce avoidable false-positive confirmations for otherwise unclassified,
   non-destructive shell commands in an explicitly enabled relaxed mode.
4. Make repetition handling less rigid only for bounded, read-only recovery
   attempts when Laya identifies genuine new evidence or a confirmatory result.
5. Make the effect measurable through shadow mode, replayable decisions, and
   compact diagnostics that do not record secrets.
6. Keep the Rust-side interface replaceable so another local decision backend can
   be added later without changing policy call sites.

## Non-goals and safety invariants

- Do not add a cloud dependency, API key requirement, telemetry service, or
  runtime model download.
- Do not port MLX to Rust in the first implementation. A native `mlx-rs` or
  `safemlx` integration may be evaluated separately after the sidecar path has
  demonstrated value.
- Do not let Laya authorize a command that RustCode's local policy classified as
  `Deny` or as an explicit destructive operation.
- Do not let Laya bypass Plan-mode denial, confirmation for explicit destructive
  commands, process-control hazards, shell redirection/backgrounding hazards,
  or known mutating primitives.
- Do not use a model result as proof that a command is safe. Laya can only add
  bounded advisory evidence to a local policy decision.
- Do not reset loop detectors, progress ledgers, compiler/completion gates, or
  general turn/tool budgets based on Laya output.
- Do not extend repetition recovery for mutating, mixed, network/external, or
  otherwise uncertain calls.
- Never log raw environment values, tokens, file contents, command output, or
  model prompts that can contain secrets. Diagnostics use redacted metadata and
  hashes where correlation is useful.

## Research constraints and rationale

Jev is a hosted typed-decision product from TypeSafe, with a different operating
model from the local Laya family. Laya-MLX is the relevant backend here because it
loads a Laya checkpoint locally through MLX on Apple Silicon, exposes a Python API,
and is self-hostable. Its command-line interface is one-shot; a persistent Python
process is therefore the practical low-latency boundary for RustCode.

The first release should pin both the sidecar package/revision and the local model
revision. The upstream package metadata and published package can move at
different speeds, and the model's confidence is not a substitute for a formal
safety proof. The status command should diagnose missing or incompatible local
dependencies, but installation and model downloads are outside the turn path.

Important platform constraints are explicit: the supported path is macOS on
Apple Silicon with Python 3.11 or newer and a compatible MLX release. Other
platforms remain fully functional with Laya off, and may use shadow/relaxed only
if a compatible adapter is later provided.

## User-facing configuration

Add a persisted `[laya]` configuration section with safe defaults:

```toml
[laya]
mode = "off"                 # off | shadow | relaxed
python = "python3"           # optional executable override
adapter = ""                 # optional local sidecar path
model = ""                   # local checkpoint path or identifier
timeout_ms = 150
min_confidence = 0.98
max_extra_read_only_recoveries = 1
```

The exact config location and load/save helpers must follow the existing RustCode
configuration conventions. Unknown or invalid values must fail closed to `off`
with an actionable diagnostic rather than changing safety behavior implicitly.

The first release does not infer a model path, install Python packages, or fetch a
checkpoint. An empty adapter/model makes `status` report unavailable and makes
turn-time evaluation fall back to local policy.

Add a CLI surface with explicit, scriptable operations:

```text
rustcode laya status
rustcode laya enable --mode shadow
rustcode laya enable --mode relaxed
rustcode laya disable
```

`enable` changes only RustCode's persisted mode and validates the requested mode;
it does not install software. `status` reports configured mode, platform
compatibility, Python/adapter/model availability, protocol version, and the last
failure category without printing secrets. A future `doctor` command can own
installation guidance if needed.

The mode semantics are:

- `off`: do not spawn or call Laya; preserve current behavior exactly.
- `shadow`: call Laya for eligible decisions, but never alter approval,
  scheduling, execution, or repetition behavior. Record compact decision metrics.
- `relaxed`: apply only the bounded changes specified below. Every ineligible or
  failed decision follows the current RustCode path.

## Architecture

Keep the policy authority in Rust and put model-specific code behind a small
trait, conceptually:

```rust
trait AdvisoryDecisionEngine: Send + Sync {
    fn evaluate(&self, request: DecisionRequest) -> Result<Decision, AdvisoryError>;
}
```

The implementation should have these boundaries:

- Rust config/CLI owns mode, executable, adapter, model, timeout, and status.
- A Rust Laya client owns process lifecycle, JSONL framing, request IDs,
  deadlines, bounded input/output, redaction, and restart/failure handling.
- A Python sidecar owns Laya-MLX import, one-time model loading, tokenization,
  inference, and conversion to the versioned response schema.
- Rust local policy owns command classification, eligibility, effective policy,
  repetition accounting, and all safety invariants.

Do not start a fresh model process per tool call. Start the sidecar lazily on the
first eligible request, keep one process per RustCode process, and restart it only
after a protocol/process failure. A sidecar that cannot become ready is treated as
unavailable for the remainder of the turn and should be rate-limited from repeated
restart attempts.

### Sidecar protocol

Use a versioned newline-delimited JSON protocol over the sidecar's stdin/stdout.
Stdout is protocol-only; human diagnostics go to stderr. The first line is a
readiness message containing the protocol version, backend name, model identity
and supported decision kinds. Every request and response carries a caller-owned
`id`.

Illustrative request shape:

```json
{
  "protocol": 1,
  "id": "turn-call-17",
  "kind": "shell_policy",
  "input": {
    "command": "git status --short",
    "cwd_class": "workspace",
    "local_class": "unclassified",
    "candidate_effects": ["unknown"]
  },
  "deadline_ms": 150
}
```

Illustrative response shape:

```json
{
  "protocol": 1,
  "id": "turn-call-17",
  "ok": true,
  "decision": {
    "label": "read_only",
    "confidence": 0.997,
    "effects": ["read_only"],
    "rationale_code": "inspect_workspace"
  },
  "latency_ms": 9
}
```

Errors use a stable category (`unavailable`, `invalid_request`, `timeout`,
`malformed_response`, `model_error`, or `process_exit`) and a redacted message.
Reject oversized lines, unknown protocol versions, duplicate IDs, missing
confidence, non-finite numbers, and unsupported labels. The client should allow
at most one in-flight request initially; concurrency can be added after the
policy/cache behavior is proven.

The sidecar script should load a pinned local checkpoint once, accept only the
small request schema, emit deterministic JSON fields, and never download assets
during a RustCode turn. Its dependency and model versions should be documented
next to the adapter. The protocol must remain testable with a fake sidecar so
Rust tests do not require Python, MLX, or a model.

## Shell policy integration

RustCode's deterministic shell parser and current command policy run first. Add a
stable classification/effects representation that distinguishes at least:

- `read_only` — observation or analysis with no intended workspace/process/
  external mutation;
- `workspace_mutation` — file, repository, build-artifact, or permission change;
- `process_control` — start/stop/kill, background jobs, service changes, or
  shell-control effects;
- `network_or_external` — network calls, uploads, package publication, or other
  external side effects;
- `unknown` — parser cannot establish the effect.

The model input is a bounded, redacted command representation plus local parser
facts. It must not include environment values or unrelated command output. The
local parser remains the source of truth for shell metacharacters, redirections,
backgrounding, privilege escalation, known destructive commands, and explicit
mutating operations.

In `relaxed` mode, Laya may change an otherwise-confirmable command to
non-confirming only when all of these hold:

1. Local policy classified it as `unclassified`, not `Deny`, explicit destructive,
   Plan-forbidden, or an already-known mutation.
2. The parsed command has no redirection, command substitution, backgrounding,
   privilege escalation, pipeline hazard, or mixed command list that the local
   policy cannot prove benign.
3. The Laya label is exactly `read_only`, confidence is at least
   `min_confidence`, and the effects do not include mutation, process control,
   or network/external effects.
4. The call is not part of a batch whose other calls require a stricter policy.
5. The assessment is cached and reused for scheduling, confirmation, and
   execution; execution must not silently re-query or lose the assessment.

Laya can promote a local `Allow` to `Confirm` if the local policy chooses to use a
high-confidence mutation warning in a future mode, but that is not required for
the first release. It can never downgrade local `Deny`, explicit destructive
confirmation, or Plan-mode denial. Any disagreement or uncertainty follows the
stricter local result.

The existing execution path must enforce the effective assessment even when a
batch-level `bypass_confirm` flag is used. A decision made only during scheduling
is insufficient because batch execution can otherwise bypass it.

## Repetition integration

The existing loop detector, progress ledger, grounded recovery, and hard recovery
cap remain in force. Before and after tool execution, RustCode should derive a
stable advisory request containing only the tool kind, normalized arguments,
current recovery reason, and redacted progress facts.

Laya may return one of:

- `novel_evidence`;
- `confirmatory_evidence`;
- `no_new_information`;
- `unknown`.

In `relaxed` mode, only a baseline `read_only` call with no mutation,
process-control, network/external, or mixed effect may receive one additional
read-only recovery credit, and only for `novel_evidence` or
`confirmatory_evidence` above the configured threshold. The credit is consumed
by one recovery round, expires immediately after that round, and is never
transferred to a later turn. A second repetition still stops normally.

Laya must not reset detector history, clear ledger state, claim progress, extend
general budgets, or permit a mutating retry. Shadow mode reports what would have
happened but consumes no credit.

As part of this work, normalize the post-result read-only check to RustCode's
canonical `crate::tools::is_read_only_call` helper. The current loop-detector
helper only knows native tool names, which can misclassify aliases or structured
calls and would undermine the advisory eligibility boundary.

## Decision flow and fallback

The intended flow is:

```text
local parse/classify
        |
        v
eligibility + immutable request/cache key
        |
        +-- off or ineligible --------> existing local policy
        |
        +-- shadow --------------------> record advisory result; existing policy
        |
        +-- relaxed -------------------> bounded effective policy
        |
        v
same cached assessment -> scheduling -> confirmation -> execution
```

All Laya errors, timeouts, process exits, invalid responses, model uncertainty,
low confidence, unsupported platforms, missing dependencies, and exceeded input
limits must be indistinguishable from “no advisory result” to the safety policy.
They may be visible through `status`, debug logs, and shadow metrics, but never
cause an operation to be allowed.

## Observability and evaluation

Shadow mode should emit compact structured events with:

- mode and decision kind;
- local classification and resulting Laya label;
- confidence bucket, not raw prompt/output;
- latency bucket and failure category;
- whether relaxed mode would have changed the result;
- a non-reversible request hash for correlation.

Do not include raw shell text in ordinary logs. If a user explicitly enables a
local debug trace, apply the same redaction and bounded-size rules.

Use recorded RustCode session evidence and a curated synthetic corpus to compare
shadow decisions against existing policy. Track false-positive confirmation
reductions, explicit destructive-operation preservation, fallback rates, latency,
sidecar restarts, and repetition recovery outcomes. The success bar is not model
accuracy alone: no safety invariant may regress, and relaxed behavior must be
easy to disable globally.

## Testing plan

Implement tests before or alongside each boundary:

1. Config parsing/defaults and CLI mode transitions, including invalid-value
   fail-closed behavior.
2. Sidecar JSONL readiness, request/response correlation, malformed data,
   timeout, process exit, oversize input, and restart handling using a fake
   executable or in-process transport.
3. Shell eligibility cases for known read-only commands, explicit destructive
   commands, redirection/backgrounding, privilege escalation, pipelines,
   network calls, mixed commands, Plan mode, and unknown commands.
4. Monotonic policy tests proving relaxed mode cannot downgrade `Deny`, explicit
   destructive confirmation, Plan denial, or any mutation.
5. Assessment-cache tests proving scheduling, confirmation, and execution use the
   same decision and do not re-query.
6. Repetition tests for one eligible credit, expiry, mutation rejection, shadow
   non-consumption, hard-cap preservation, and canonical read-only classification.
7. Status tests for missing Python, unsupported architecture, missing adapter, and
   missing model; these tests must not require installing MLX.

Before handoff, run the repository-required `cargo check --tests` and `cargo test`
from the feature worktree. Add a small integration test for the off mode proving
that the old path remains unchanged.

## Rollout

Ship the feature disabled. The recommended adoption sequence is:

1. `shadow` on a local development machine to collect disagreement and latency.
2. Review the replay corpus and verify explicit destructive commands never become
   allowed.
3. Enable `relaxed` only for users who accept the narrowly defined behavior.
4. Disable immediately with `rustcode laya disable` if the sidecar is unstable;
   RustCode continues using its local guards.

## References

- TypeSafe Jev/System One overview: https://typesafe.ai/blog/introducing-system-one-models-and-jev
- TypeSafe API documentation: https://api.typesafe.ai/docs
- Laya upstream README and limitations: https://github.com/NandhaKishorM/laya/blob/main/README.md
- Laya-MLX README and Python usage: https://github.com/mizorewww/laya-mlx
- Laya-MLX package metadata: https://github.com/mizorewww/laya-mlx/blob/main/pyproject.toml
- Laya-MLX published package: https://pypi.org/project/laya-mlx/
- Local English checkpoint: https://huggingface.co/aac6fef/laya-mlx/tree/main
- Rust MLX binding candidates considered: https://docs.rs/crate/mlx-rs/latest and https://docs.rs/crate/safemlx/latest
