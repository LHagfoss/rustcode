# AGENTS.md

## Instruction priority

These repository instructions are specific to RustCode and take precedence over
generic skills or workflow recipes when they conflict. Follow the repository
instructions for work in this repository.

## Workflow

- Work in the active checkout on a task branch, and leave it synced and clean
  when you finish. This is the default, not the exception.
- Create the task branch in the active checkout (`git switch -c <type>/<scope>-<slug>`).
  Do not use a separate worktree unless it is genuinely called for: subagents or
  other concurrent work, or an active checkout holding unrelated dirty work.
- When a worktree is used, build it under `/tmp` with `git worktree add`, and
  clean it up as soon as its branch is pushed and merged: `git worktree remove`,
  `git worktree prune`, then `git branch -d`. Never leave a worktree or task
  branch behind. Before deleting, confirm nothing unique is lost with
  `git cherry main <branch>` (a squash-merged branch legitimately shows
  commits that are not ancestors of `main`; verify the content matches).
- Never discard the user's work: no `git rebase`, `git reset --hard`, or
  force-push in the active checkout, and never stash or drop their changes. If
  the working tree is dirty with unrelated work, say so and work around it.
- **Keep the active checkout synced.** At the start of a task and again when you
  finish, run `git pull --ff-only` so it never drifts behind `origin/main`, and
  return it to its original branch. Fast-forward only, never a plain `git pull`
  (it can create a merge commit) and never forced. If the working tree is dirty,
  leave it alone and say so instead.
- Inspect first; make the smallest scoped change and preserve unrelated work.
- Run `cargo check --tests` and `cargo test`.
- For releases, load `~/.config/rustcode/skills/release-automation/SKILL.md`;
  use `scripts/release.sh` as the source of truth.

## Naming

Use Conventional Commits consistently for branch names, commit messages, and
PR titles:

- Branch: `<type>/<scope>-<short-slug>` in lowercase, e.g.
  `fix/tui-composer-selection`, `feat/workspace-task-flow`.
- Commit and PR title: `<type>(<scope>): <summary in the imperative mood>`, e.g.
  `fix(tui): keep transcript selection after drag`.
- Reuse the exact same title for the commit and its PR.

Accepted `type` values: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`,
`chore`, `ci`, `build`. Keep `scope` to the area touched; reuse an existing
scope (`tui`, `workspace`, `sandbox`, `engine`, `mcp`, `ci`) rather than
inventing near-duplicates. No period at the end. Write the summary in the
imperative ("add", not "added"), under ~72 characters. Do not prefix with
`wip:` on a PR; use a draft PR instead.

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
  `rustcode/engine/src/tools/{search,filesystem,exec,misc}.rs`, then add it
  to `TOOLS` in `rustcode/engine/src/tools/mod.rs`. No other tables need
  updating.
