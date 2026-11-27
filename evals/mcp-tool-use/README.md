# MCP tool use

Replays small fake threads on a Claude model and checks how it uses
RustCode's MCP tools and whether it follows `AGENTS.md`: naming the thread,
settling, credentials, settings, past work, new threads, merging, commit
attribution. Each case names the rule or incident it guards.

The model gets a short harness preamble, the case's harness note (as Claude
Code's reminders add), `AGENTS.md` when the case asks for it, and RustCode's
MCP instructions and tools as `mcp__rustcode__*`, read from this checkout
with `rc tools manifest`. It also gets a `Bash` tool. Nothing runs: shell
commands get the case's canned output (empty by default) and RustCode tools
answer `{"ok":true}` unless the case gives an answer.

## Run

```sh
cd evals
bun run mcp-tool-use --case settle-this --samples 1 --out runs/check
bun run mcp-tool-use --out runs/sonnet
bun run mcp-tool-use --out runs/after --baseline runs/sonnet/results.jsonl
```

- `ANTHROPIC_API_KEY` in the repo root `.env` pays for the model.
- Without `--manifest`, the eval builds `rc` from this checkout and runs
  `rc tools manifest`. Pass `--manifest <file>` to use one saved earlier
  (`rc tools manifest > manifest.json`), for example on a machine that should
  not build Rust.
- `--model` defaults to `claude-sonnet-5-5`, `--samples` to 3 and
  `--max-turns` (model requests per sample) to 6.

## Grading

`grade.ts` checks each sample's calls and final text against the case:
`first` (the first call), `calls` (each must happen), `anyOf`, `never`,
`bashSome`, `bashNever` and `textNever`. A call match names the public tool
(`update_thread`, `Bash`), and optionally `args` that must be equal, keys it
`has` and keys it `lacks`.

## Add a case

Add it to `cases.ts` with an `id`, its `source` and its checks; give canned
`shell` outputs or tool `results` when the model needs them to get to the
step being checked. Run it on the old text first to see it fail.

## Cost

About 10,000 input tokens per model request (the tool list is most of it),
and two to four requests per sample: a full run of 12 cases × 3 samples is
roughly a million input tokens. `summary.md` records what the run used.
