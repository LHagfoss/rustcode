# Evals

An eval runs a real model against RustCode's real text and scores what the
model does. Use one where a unit test cannot help: model output changes from
run to run, and a small copy change can move it.

Write or run an eval when a change affects what a model sees or does: the MCP
server's instructions, tool descriptions and schemas, tool results, or the
agent instructions in `AGENTS.md`.

| Eval                          | Measures                                                                        |
| ----------------------------- | ------------------------------------------------------------------------------- |
| [mcp-tool-use](mcp-tool-use/) | Whether agents use RustCode's MCP tools and follow `AGENTS.md` in fake threads. |

## Run an eval

1. Put the API keys the eval needs in the repo root `.env` (gitignored). Never
   print or commit a key.
2. From `evals/`, run `bun run <eval> --out runs/<name>`. `runs/` is gitignored.

Each eval's README lists its flags and cost. Start with one case and one
sample to check the setup.

Evals call paid models, so CI does not run them. CI's lint job runs the
graders' unit tests (`bun test` here).

## The shape of an eval

- `evals/<eval>/run.ts` is the entry point; `package.json` has a script with
  the eval's name.
- `evals/<eval>/README.md` says what it measures, how to run it, how to add a
  case and what a run costs.
- Cases are data in `cases.ts`. Each has an `id`, names its source (the rule,
  issue or incident it guards) and its checks.
- The eval reads the text under test from this checkout, never a copy: the
  MCP instructions and tools come from `rc tools manifest`, and `AGENTS.md`
  from the repo root.
- Flags: `--out <dir>` (required), `--model <id>`, `--samples <n>`,
  `--case <ids>`, and `--baseline <earlier results.jsonl>` to compare.
- A run writes `results.jsonl` and a `summary.md` for a person.
- `run.test.ts` tests the grader, which is plain logic.

## Rules for cases

- A case reproduces a real bad experience or guards a stated rule. Name its
  source.
- Add the case before the fix. It must fail on the old text and pass on the
  new. Prefer fixing RustCode over loosening a case.
- Add controls: cases where the right move is ordinary work. They catch a fix
  that overcorrects.
- Run each case several times. A case fails when half or more of its samples
  fail. Compare against a baseline on the same model, not an absolute score.
- No user data in cases: use `/home/user/app` and `acme/app`.

## Cost

Run only the cases your change touches until they pass, then one full run per
model before the PR merges. Each summary records the tokens the run used.
