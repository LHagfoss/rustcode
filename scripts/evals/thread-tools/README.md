# Thread-tools eval

Checks that an agent reaches for the right orchestration tool. Each case is a
prompt a user would type; the agent gets the tools from RustCode's MCP server
(`rustcode mcp-orchestrator`) as stand-ins named `mcp__rustcode__<tool>`, with
their real descriptions and schemas, and nothing else. It has no app, no
engine socket and no file or shell tools, and every call is answered from
canned data, so a run costs one model turn per case and changes nothing.

A case passes when the agent called every tool in `must` and none in
`mustNot`. Reading the answer is not checked.

## Run

```sh
cargo build -p rustcode-tui --bin rustcode
bun scripts/evals/thread-tools/run.ts
```

Options: `--model <model>` (default `deepseek/deepseek-flash:high`),
`--case <id>[,<id>]`, `--samples <n>` (default 1), `--concurrency <n>`
(default 3). `RUSTCODE_BIN` points at another binary (default
`target/debug/rustcode`).

Each run writes `runs/<timestamp>.jsonl`, one line per case: the tools it
called and their arguments, the reply, and whether it passed. The folder is
git-ignored. The exit code is 1 when any case fails.

The runner passes `--tools` with the stand-in names: `pi --no-tools` alone
empties the allowlist, and extension tools are hidden with it.

## Add a case

Append one line to `cases.jsonl`:

```json
{"id":"rename-thread","kind":"bad","prompt":"rename this thread to X","must":["update_session"],"mustNot":[]}
```

- `kind`: `bad` (the prompt should pick a tool), `guard` (ordinary work, no
  thread tool) or `control` (nothing to do, no tool at all).
- `must`: tools that must be called, in any order.
- `oneOf`: when several tools answer the same question, at least one of these.
- `mustNot`: tools that must not be called; controls name all of them.

Canned results live in `MOCKS` in `run.ts`; add one when a follow-up call
needs data from the one before it (for example `list_sessions` before
`interrupt_session`).
