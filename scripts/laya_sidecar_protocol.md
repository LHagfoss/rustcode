# RustCode Laya sidecar protocol

The Rust client starts one long-lived local process on the first eligible
advisory request. The process is not started when `laya.mode = "off"`. Standard
output is protocol-only; diagnostics belong on standard error. Rust sends one
request at a time and restarts a failed sidecar at most once on a later request.

## Pinned runtime

- Python `>=3.11`.
- Apple Silicon macOS (`arm64`); the Rust status command reports unsupported
  architectures without attempting inference.
- `laya-mlx==0.2.0`.
- MLX `>=0.32.2,<0.33`.
- The checkpoint is staged locally before RustCode starts. The supported
  release checkpoint is `aac6fef/laya-mlx`, pinned to upstream revision
  `052592a15d198d9ad47da779604259b10b47b7aa`.

The adapter receives a filesystem path, not a Hub identifier. It calls
`laya_mlx.load()` once during startup and then uses the loaded agent's local
`predict(state, questions)` API. It never downloads packages or model assets at
runtime. Install the pinned environment and stage the checkpoint separately.

Example configuration:

```toml
[laya]
mode = "shadow"
python = "/Users/me/.venvs/rustcode-laya/bin/python"
adapter = "/Users/me/rustcode/scripts/laya_sidecar.py"
model = "/Users/me/models/aac6fef-laya-mlx"
timeout_ms = 150
startup_timeout_ms = 5000
min_confidence = 0.98
max_extra_read_only_recoveries = 1
```

## Framing and readiness

Each message is one UTF-8 JSON object terminated by `\n`. Lines are limited to
16 KiB, and Rust allows at most one request in flight. The first stdout line is
readiness:

```json
{"protocol":1,"backend":"laya-mlx","model":"aac6fef-laya-mlx","kinds":["shell_policy","repetition"]}
```

Rust requires protocol `1`, a backend and model identity, and both supported
decision kinds before sending requests. Any other readiness message makes the
sidecar unavailable for the current turn. Readiness uses the separate
`startup_timeout_ms` bound (default 5 seconds); inference uses `timeout_ms`
(default 150 ms).

## Request

The caller owns a unique request ID. Rust validates the serialized request and
the sidecar validates it again:

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

`kind` is `shell_policy` or `repetition`. Input is bounded and must already be
redacted by the Rust policy layer. The adapter does not accept arbitrary prompt
text, environment data, or command output.

## Successful response

```json
{
  "protocol": 1,
  "id": "turn-call-17",
  "ok": true,
  "decision": {
    "label": "read_only",
    "confidence": 0.997,
    "effects": ["read_only"],
    "rationale_code": "laya_shell_policy"
  },
  "latency_ms": 9
}
```

Allowed labels are `read_only`, `novel_evidence`, `confirmatory_evidence`,
`no_new_information`, and `unknown`. Confidence must be finite and in the
inclusive range `0.0..=1.0`, and must meet the configured `min_confidence`.
`unknown` and below-threshold decisions become model errors, never successful
advisory results. Rust rejects missing fields, unknown labels, unknown protocol
versions, mismatched or duplicate IDs, non-finite confidence, and oversized
lines. A transport or validation error is never an allow result.

## Error response

The adapter may return a correlated error object:

```json
{
  "protocol": 1,
  "id": "turn-call-17",
  "ok": false,
  "error": {"category": "model_error"}
}
```

Stable categories are `unavailable`, `invalid_request`, `timeout`,
`malformed_response`, `model_error`, and `process_exit`. Human diagnostics are
written to stderr and are not copied into the JSON protocol. Rust treats all
categories as “no advisory result” for policy purposes, preserving the local
policy decision.
