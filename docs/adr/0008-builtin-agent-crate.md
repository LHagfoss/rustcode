# 0008: The built-in agent gets its own crate

Status: proposed, 2026-11-22. Code: `rustcode/engine/src/{app,network,tools,controller,provider_auth}`,
`rustcode/engine/src/acp.rs`, `rustcode/engine/src/acp/`, `harness/`.

## Why

The engine is one crate that holds two things: the server every client talks
to (ACP, external harnesses, orchestration, the session store, settings, the
data store) and RustCode's own built-in agent (the model loop, its tools, its
provider sign-ins). A change to either rebuilds both, and nothing stops server
code from reaching into the agent's state. External harnesses already sit
behind a narrow surface (`harness/session.rs`: start, prompt, steer, cancel,
config options); the built-in agent does not.

## Decision

- A new crate holds the built-in agent: `app`, `network`, `tools`,
  `controller`, `provider_auth`, and the ACP files that drive it
  (`acp/prompt.rs`, `usage.rs`, `permissions.rs`, `streaming.rs`,
  `questions.rs`, `commands.rs`, `agents.rs`, `steer.rs`, `rewind.rs`,
  `config.rs`).
- The engine keeps the server, the ACP dispatcher, external harnesses,
  orchestration, `sessions`, `config` and the data store, and never names an
  agent type.
- The dependency runs one way: the agent crate depends on the engine. The
  engine defines a `BuiltinAgent` trait (open a session, prompt, steer, cancel,
  config options, history) and holds a session as a trait object where it holds
  `Arc<Mutex<AppState>>` today. Each binary (`rc`, `rustcode`, the desktop
  sidecar) registers the agent at startup.
- Types both sides read (`ChatMessage`, `History`, `Verbosity`,
  `OutputCapSource`) live in the core crates, as they already do.

## Order

1. Untangle `config` from the agent: the session store moves to `sessions`
   (#4230), and `config` owns or imports from core every type its file names
   (#4231).
2. Introduce the trait inside the engine crate and route the prompt handler's
   built-in branch through it; the ACP server structs hold the trait object.
3. Move the agent's ACP files behind the trait, one area per pull request
   (prompt and streaming, permissions and questions, usage, commands).
4. Point `remote`, `raw_cli`, `skills` and `orchestration` at the trait or the
   core crates (about 300 references today).
5. Cut the crate and move `acp_tests.rs`'s agent tests with it.

At the start of step 2, about 800 references from engine code into the agent
remain, 500 of them in `acp`.
