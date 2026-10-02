# Per-request thinking routing

Configure a Mapika `decider-4b` raw completions endpoint to decide whether each
upcoming model request should use the profile's normal thinking settings or
skip thinking. Decisions take effect immediately; there is no shadow phase.
The router controls generation only, never command authorization or recovery.

Add an inline `thinking_router` table to a selected `[[models]]` entry in your
**global** `~/.config/rustcode/config.toml`:

```toml
[[models]]
name = "omlx/qwen"
url = "https://your-omlx.example/v1/chat/completions"
model = "your-qwen-model"
env_key = "OMLX_API_KEY"
enable_thinking = true
thinking_router = { url = "https://your-omlx.example/v1/completions", model = "decider-4b", env_key = "OMLX_API_KEY", timeout_ms = 1000 }
```

The URL is the complete `/v1/completions` endpoint, not a chat endpoint or a
System One endpoint. The model must support Mapika's plain state-first prompt
format with an `Answer: (` slot. This is not a generic classification adapter
for every decision-model family. Router credentials are resolved independently
from its `env_key` using RustCode's existing shell-environment resolver. The main
model's API key is never implicitly forwarded to the router.

Only configured Chat Completions profiles with `enable_thinking = true` route.
Explicit thinking-off profiles, profiles without thinking controls, Responses
profiles, and existing recovery requests retain their ordinary behavior.
Remove `thinking_router` to disable routing. Project configuration cannot add
an auxiliary endpoint receiving conversation context.

Each request sends a bounded snapshot of the initial and latest user messages,
recent assistant text and tool results, and recent native tool names. System
instructions, reasoning traces, image data, tool arguments and schemas are
excluded. Text is limited to 6,000 UTF-8 bytes; long results retain their
beginning and end. This is partial context, not a full history assessment.

The router asks whether more reasoning would help the next response and reads
one answer token:

- `A`: keep configured thinking for diagnosis, conflicting evidence or planning.
- `B`: disable thinking for a direct answer or a settled routine next step.
- `C`: uncertain; keep normal settings.

This oMLX path does not return calibrated option probabilities. Routing uses
exact labels, not an invented confidence score. Wrong classifications remain
possible; evaluate end-to-end task quality on your workload.

Unavailable credentials, HTTP/transport errors, unexpected model identities,
malformed or oversized responses, cancellation and deadlines all fall back to
the original request mode. There is one attempt and no router retries. The
default deadline is 1,000 ms, capped at 2,000 ms; zero skips the attempt. With
thinking off, the normal request builder omits reasoning effort/budget and
sends `enable_thinking = false` through its existing controls. The main model
and its server template must honor those controls.

`turn.thinking_route` operational events record the session, router model,
outcome, whether thinking-off was applied, and elapsed milliseconds. They do
not include prompts, endpoint URLs, response bodies or credentials. Measure
complete task time, correctness, calls and recoveries, including router latency;
a short decision does not guarantee a faster complete task.

## Verification

```sh
cargo test --lib thinking_router
# Optional live test; the credential value stays in your existing shell variable.
RUSTCODE_ROUTER_TEST_URL=https://your-omlx.example/v1/completions \
RUSTCODE_ROUTER_TEST_MODEL=decider-4b \
RUSTCODE_ROUTER_TEST_ENV_KEY=OMLX_API_KEY \
cargo test --lib live_decider_disables_thinking_for_a_direct_answer -- --ignored
```
