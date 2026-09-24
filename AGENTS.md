# AGENTS.md

## Instruction priority

These repository instructions are specific to RustCode and take precedence over
generic skills or workflow recipes when they conflict. Follow the repository
instructions for work in this repository.

## Workflow

- Start from current `main` in an isolated task worktree on a `feature/...` or
  `fix/...` branch.
- Never create or switch a task branch in the active user checkout. This
  includes `git branch`, `git switch -c`, `git checkout -b`, `git checkout -B`,
  and `git switch -C`. Keep the active checkout on its original branch
  throughout the task. Create and switch task branches only in an isolated
  worktree (`git worktree add /tmp/...`). If a generic skill or workflow says
  to create a branch with `git switch -c`, use `git worktree add` instead.
- Never move the user's checkout: no `rebase` or `reset --hard` in the active
  working tree. Branch and merge work belongs in an isolated worktree.
- Inspect first; make the smallest scoped change and preserve unrelated work.
- Run `cargo check --tests` and `cargo test`.
- Commit, push, PR to `main`, and merge from the isolated task worktree. After
  merge, never checkout or pull `main` by moving the original active checkout;
  the original active checkout remains on its original branch. If a local
  checkout needs to be updated to `main`, do so only in a separate clone or
  isolated checkout.
- For releases, load `~/.config/rustcode/skills/release-automation/SKILL.md`;
  use `scripts/release.sh` as the source of truth.

## Search

- Use `rg` for exact searches; use SocratiCode for unclear architecture, then
  verify source.
- Search for existing behavior before adding helpers or dependencies.
- Prefer project code, then std, then a small local implementation; match
  existing conventions.

## Session-driven fix loop

- Diagnose from evidence:
  `~/.config/rustcode/sessions/<id>/{history.json,logs/debug.log}` plus
  `operational_event` kinds (`turn.summary`, `tools.batch.finish`,
  `turn.stream_checkpoint`, budget/recovery events).
- Log a GitHub issue first (`gh issue create`) with problem, session evidence,
  and acceptance criteria, so an agent can pick it up independently.
- Benchmark fairly: fresh session per task (no cross-task contamination); same
  session only for follow-ups (cache + bounded request window make them cheap).
- After the fix, re-run the originating task and compare rounds/calls/recoveries
  against the pre-fix session log.

## Tooling notes

- `replace_file_content` only honors line anchoring when both `start_line` and
  `end_line` are passed. Prefer unique multi-line `target_content`. If an edit
  reports "target_content not found", re-read the current lines — don't retry
  the same string.
- Sessions/history: `~/.config/rustcode/sessions/<id>/history.json`.
- Adding a built-in tool: add one `pub const …: Tool` in
  `src/tools/{search,filesystem,exec,misc}.rs`, then add it to `TOOLS` in
  `src/tools/mod.rs`. No other tables need updating.
