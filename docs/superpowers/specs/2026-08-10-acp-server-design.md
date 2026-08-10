# Headless ACP Server Design

## Goal

Add a `rustcode --acp` mode that runs the agent as a headless ACP v1 server over stdio, so ACP clients and agent orchestrators can launch rustcode as a runtime while the existing TUI remains unchanged.

## Design

The existing binary gains an `--acp` flag. When present, `main` skips terminal initialization and runs an ACP `Agent` built with the official `agent-client-protocol` Rust SDK, connected through the SDK's `Stdio` transport. The ACP adapter owns protocol/session concerns and delegates each prompt to the existing headless turn loop and model/MCP stack.

ACP sessions are represented by an in-memory map of ACP session IDs to `AppState` instances. `session/new` creates a fresh state rooted at the requested working directory. `session/prompt` appends the incoming text to that state, invokes the shared agent turn loop, streams assistant text through `session/update`, and returns a v1 stop reason. The existing `--prompt` path and TUI path are not changed.

The first implementation supports ACP v1 initialization, new sessions, prompts, cancellation plumbing, and text streaming. It advertises no unstable protocol features. Tool execution uses the existing non-interactive headless policy, including plan-mode safety checks; tool-specific ACP permission dialogs are intentionally not added until the shared turn engine can expose structured permission events without coupling the TUI to ACP.

The existing MCP integration remains available to rustcode's agent internally. Native MCP-over-ACP attachment is excluded because the official SDK marks it unstable and it is not required for an ACP client to launch rustcode.

## Components

- `src/cli.rs`: add the `--acp` command-line switch.
- `src/main.rs`: route `--acp` before raw/TUI startup.
- `src/acp.rs`: ACP v1 agent adapter, session map, protocol callbacks, and stdio entry point.
- `src/raw_cli.rs`: expose a reusable headless turn function that returns the final assistant content while preserving the existing CLI behavior.
- `Cargo.toml`: add the official `agent-client-protocol` SDK dependency.

## Data flow

```text
ACP client/orchestrator
        │ JSON-RPC over stdin/stdout
        ▼
agent-client-protocol::Agent + Stdio
        │ ACP callbacks
        ▼
ACP session map → AppState → network::run_agent_turn
                              │
                              ├─ model provider
                              └─ configured MCP tools
```

All diagnostic output from ACP mode goes to stderr. Stdout is reserved for ACP frames.

## Errors and cancellation

Malformed or unsupported ACP requests are handled by the SDK. Application failures become ACP request errors with useful messages. A session prompt is cancellable through the existing cancellation token; cancellation returns ACP's cancelled stop reason when the underlying turn exits.

## Verification

- Unit-test CLI parsing for `--acp`.
- Unit-test ACP initialization capabilities and session creation behavior.
- Unit-test conversion of ACP text prompts into the existing state/history representation.
- Run `cargo check --tests` and `cargo test`.
- Smoke-test the built binary with an ACP initialize/session-new exchange and verify stdout contains only JSON-RPC messages.

