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
- Run the checks for what the change touches; CI runs the same split.
  - Rust (`rustcode/`, `Cargo.*`): `cargo check --workspace --tests` and
    `cargo test --workspace`. Without `--workspace` only the engine package is
    built, and a TUI change passes unchecked. CI also runs
    `cargo fmt --all -- --check`.
  - macOS app (`macos/`): `scripts/swift-test.sh [TestClass]`. It hides the
    test host's Contacts noise and fails a filter that matches nothing
    (filter by class or test name; `RustCodeTests.X` matches nothing).
  - A Swift-only change needs no cargo run, and a Rust-only one no Swift run,
    unless the change crosses the ACP boundary between them.
- The iOS app lives in `ios/` with its own checks; see `ios/AGENTS.md`.
- For releases, load `~/.agents/skills/release-automation/SKILL.md`;
  use `scripts/release.sh` as the source of truth. Draft the notes with
  `scripts/release-notes.sh > notes.md` (each PR's `## Changelog` line, else
  its title), edit, then pass `--notes-file`. Release once per batch of merged
  work, not per PR.

## Pull requests

- One issue per PR, but issues that change the same code path go in one PR
  (for example two fixes to what happens when the chat changes): separate PRs
  there conflict with each other and each needs another CI run.
- When the user asked for merging, merge once CI is green without polling in
  a loop: `gh pr checks <n> --watch --fail-fast && gh pr merge <n> --squash
  --delete-branch`. (`gh pr merge --auto` needs required status checks on
  `main`; without them it fails with "clean status", and a plain merge would
  skip CI.)
- Before starting the next issue in files a queued PR touches, wait for that
  PR to merge and branch from the new `main`, instead of resolving the same
  conflict later.
- Avoid stacked PRs. When an engine change and its client change depend on
  each other, ship them in one PR. If a stack is unavoidable, retarget the
  upper PR to `main` (`gh pr edit <n> --base main`) before merging the lower
  one: `--delete-branch` on the base closes the PR stacked on it.
- Fill in the PR template's `## Changelog` line; it becomes the release note.

## Issues

- Put what can be checked in text: the exact error, diff lines, commands and
  messages. Screenshot attachments do not reach an agent working the issue.
- When an issue depends on something only a person can do (secrets, a
  physical device), say exactly what is needed and leave it open; do not
  re-investigate it on every pass.

## Checking macOS app behavior

- Build with `swift build --package-path macos` and drive the debug app with
  `DebugDriver` (`macos/Sources/RustCode/DebugDriver.swift`): start
  `.build/debug/RustCode` with `RUSTCODE_DRIVE=<dir>`, write commands to
  `<dir>/cmd`, read timings from `<dir>/time`. `snap <file.png>` captures the
  window; `wheelstream`, `timescroll` and `stream` measure scrolling and
  streaming. Run a measurement under `sample <pid> 3` to see where the main
  thread goes.
- Synthesized wheel, drag and pinch events do not reach SwiftUI views from the
  driver. Say in the PR when real pointer input was not exercised.

## Naming

Never add AI or agent attribution anywhere: no `Co-Authored-By` trailers for
an assistant, no session links, no "Generated with …" footers in commits, PR
titles or bodies, comments, or release notes.

Use Conventional Commits consistently for branch names, commit messages, and
PR titles:

- Branch: `<type>/<scope>-<short-slug>` in lowercase, e.g.
  `fix/tui-composer-selection`, `feat/workspace-task-flow`.
- Commit and PR title: `<type>(<scope>): <summary in the imperative mood>`, e.g.
  `fix(tui): keep transcript selection after drag`.
- Reuse the exact same title for the commit and its PR.

Accepted `type` values: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`,
`chore`, `ci`, `build`. Keep `scope` to the area touched; reuse an existing
scope (`tui`, `workspace`, `sandbox`, `engine`, `mcp`, `ci`, `macos`, `ios`) rather than
inventing near-duplicates. No period at the end. Write the summary in the
imperative ("add", not "added"), under ~72 characters. Do not prefix with
`wip:` on a PR; use a draft PR instead.

## Search

- Search in layers, from intent to exact match: **SocratiCode → ast-grep → rg**.
  1. **SocratiCode** (`codebase_search`, `codebase_impact`, `codebase_flow`,
     `codebase_context_search`) when you don't yet know where something lives:
     hybrid semantic search and the dependency graph find the concept and its
     blast radius. Verify the source before acting.
  2. **ast-grep** (`ast-grep run -p '<pattern>' -l <lang> <path>`) once you know
     the syntactic shape: structural matches ignore comments/strings and
     formatting, support metavariables (`let $A = $B`, `foo($A)`), and can
     rewrite code (codemods) or run lint rules. Prefer it over regex for
     refactors. Use the `ast-grep` binary, not the deprecated `sg` alias.
  3. **`rg`** for exact strings, configs, logs, and non-code files — the
     cheapest, always-available fallback.
- Search for existing behavior before adding helpers or dependencies.
- Prefer project code, then std, then a small local implementation; match
  existing conventions.

## Session-driven fix loop

- Diagnose from evidence:
  `~/.config/rustcode/sessions/<yyyy>/<mm>/<dd>/<id>/{history.json,logs/debug.log,performance.json}`
  (sessions are filed by start date; `rustcode sessions path <id>` prints one's
  directory from its id) plus
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
- Sessions/history: `~/.config/rustcode/sessions/<yyyy>/<mm>/<dd>/<id>/history.json`.
- Adding a built-in tool: add one `pub const …: Tool` in
  `rustcode/engine/src/tools/{search,filesystem,exec,misc}.rs`, then add it
  to `TOOLS` in `rustcode/engine/src/tools/mod.rs`. No other tables need
  updating.
