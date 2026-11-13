# AGENTS.md

## What RustCode is

RustCode is the orchestrator: the app and the CLI that own threads, projects,
settings and usage data, and run each thread on a harness (Codex, Claude Code,
Gemini CLI, OpenCode, Pi, or the built-in harness in
`rustcode/engine/src/network/turn`). Query it from an agent with the
`mcp__rustcode__*` tools, `rustcode threads list --all`,
`rustcode threads path <id>`, `rustcode usage` (tokens and the cost each
harness reported) and `rustcode data query "<sql>"` (the local data store,
`docs/data-store.md`); threads are on disk in `~/.config/rustcode/threads`.

## Instruction priority

These repository instructions are specific to RustCode and take precedence over
generic skills or workflow recipes when they conflict. Follow the repository
instructions for work in this repository.

## Working with the user

- Before changing code for a batch of issues or an ambiguous request, ask in
  one message everything that blocks you, then wait for an explicit GO:
  - Scope: the whole task or which part.
  - Autonomy: work through to merge/release, or pause at named points.
  - Prerequisites: only what the scope needs (credentials, devices, Docker,
    running services). If nothing, say "No prerequisites needed."
  Skip what was already answered. A direct request to fix something now is
  the GO.
- When proposing a fix, give the use case (the problem, one or two lines) and
  a UX walkthrough (what the user presses and sees).
- Ask when an issue is ambiguous, contradicts another issue, or depends on a
  screenshot you cannot see; otherwise follow sensible defaults and say which
  you chose.
- The repo's standard checks and the debug app build need no approval;
  Docker, installs, and anything touching the user's real data or accounts do.
- Ask before creating a worktree.
- Keep task boundaries clear: one task per branch and PR, unless issues share
  a code path (see Pull requests).

## Workflow

- Work in the active checkout on a task branch, and leave it synced and clean
  when you finish. This is the default, not the exception.
- Create the task branch in the active checkout (`git switch -c <type>/<scope>-<slug>`).
  Do not use a separate worktree unless it is genuinely called for (subagents or
  other concurrent work, or an active checkout holding unrelated dirty work),
  and ask the user first.
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
- Tests run in CI, not locally. Building the engine's tests on a laptop took
  4–7 minutes per run and drained the battery, while CI runs the full suites
  on every push in about 6. Locally, run only what catches typos in seconds:
  the type check and the formatter. Then push and read the result:
  `gh pr checks <n>`, and `gh run view <run> --log-failed` for a failure. Fix
  from the CI log and push again.
  - Rust (`rustcode/`, `Cargo.*`): `cargo check --workspace --tests` and
    `cargo fmt --all -- --check`. Without `--workspace` the check builds only
    the engine package, and a TUI change passes unchecked. Do not run
    `cargo test` or `cargo nextest`; the command guard refuses them. To
    reproduce a CI failure you cannot read from the log, set
    `RUSTCODE_ALLOW_LOCAL_TESTS=1` and run that one test by name.
  - Desktop app (`apps/desktop`, Tauri + React, bun): `bun run check` in
    `apps/desktop` (typecheck, lint, the bun unit tests and the build, all
    seconds), and `cargo fmt --check` and `cargo clippy --all-targets --
    -D warnings` in `apps/desktop/src-tauri` (its own Cargo workspace; CI
    runs its tests). This is the product: the RustCode orchestrator app. Start
    from `docs/desktop.md` › "Status and roadmap", follow `docs/design.md`,
    and run it with `bun tauri dev` (it uses `target/debug/rustcode`;
    rebuild that after engine changes).
  - A UI-only change needs no cargo run, and a Rust-only one no `bun run
    check`, unless the change crosses the ACP boundary between them.
- The shell rules in this section are enforced by RustCode's command guard
  from `.rustcode/command-policy.toml` (`docs/command-guard.md`): a refused
  command names the rule and the allowed alternative. Change a rule there, not
  by working around it.
- Keep build output bounded; `target/` once grew to 500 GB. Cargo never
  prunes it, and every feature set, profile and stale dependency version
  leaves artifacts behind.
  - Incremental compilation is on, so a one-file edit rechecks in seconds
    instead of rebuilding the whole engine crate. Its state lives in
    `target/*/incremental`; the size check below bounds it.
  - Cargo runs at `nice -n 10` (the command guard adds it): it yields to what
    you are doing but still uses every core. Do not wrap it in
    `taskpolicy -b`, which on Apple silicon confines a build to the
    efficiency cores and made it 2–5× slower.
  - Before a long cargo session and when you finish, check the size and clean
    above 50 GB:
    `[ "$(du -sk target 2>/dev/null | cut -f1)" -gt 52428800 ] && cargo clean`.
  - Each worktree has its own `target/`. Removing the
    worktree when its branch merges (see above) is what frees them; do not
    leave idle worktrees, and point a long-lived one at a shared
    `CARGO_TARGET_DIR` rather than building a second full tree.
  - `cargo clean --release` drops only release artifacts; use it after a
    local release build instead of keeping both profiles around.
- The phone app is an Expo app in its own repository, `rustcode-mobile`;
  see `docs/mobile.md`.
- For releases, load `~/.agents/skills/release-automation/SKILL.md`;
  use `scripts/release.sh` as the source of truth. Draft the notes with
  `scripts/release-notes.sh > notes.md` (each PR's `## Changelog` line, else
  its title), edit, then pass `--notes-file`. Release once per batch of merged
  work, not per PR.
- To try a batch on a Mac before a full release, cut a beta:
  `scripts/release.sh --beta` (derives `X.Y.Z-beta.N`, one past the last
  beta of the upcoming release). Cut as many betas as needed, several a day
  is fine. Only the macOS CLI and app are built, published as a GitHub
  prerelease that Homebrew and both updaters skip. The next
  `--patch`/`--minor` release after it is `X.Y.Z` itself.

## Pull requests

- One issue per PR, but issues that change the same code path go in one PR
  (for example two fixes to what happens when the chat changes): separate PRs
  there conflict with each other and each needs another CI run.
- When the user asked for merging, merge once CI is green with
  `scripts/pr-merge.sh <n>` (run it in the background). It waits for every
  check except the advisory ones that take longest and never block (the
  Windows/macOS portability jobs), catches path-filtered jobs that register late, and merges
  with `--squash --delete-branch`. (`gh pr merge --auto` needs required status
  checks on `main`; without them it fails with "clean status", and a plain
  merge would skip CI.)
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

## Checking desktop app behavior

- Look at UI changes in the mock: it needs no engine and no model turns. Run
  `bun run mock-window open` in `apps/desktop`: it starts the mock dev server
  (`VITE_TRANSPORT=mock`) and one headless Chrome at 1512×945 and native scale
  (2x), and reuses both on the next call, so there is one shared browser rather
  than one per agent. It is headless on purpose: a visible window steals the
  user's focus on every launch, so agents never pass `open --show` (that is for
  a person who wants to watch). An already open window is reused in whichever
  mode it started. Then `bun run mock-window eval "<js>"` evaluates
  in the page, `bun run mock-window screenshot <file.png>` saves a PNG of it,
  and `bun run mock-window close` stops only that window and server. For
  anything a user does, prefer `click "<css>"` (with `--at <x>,<y>` for a
  partly covered element), `type "<text>"` and `press <key>` over `eval`: they
  send real input and focus, so the check exercises what the user does and the
  window shows it. For hover cards, drag-to-reorder, wheel scrolling and double
  clicks that is `hover "<css>" [--wait <ms>]` (prints what opened),
  `drag "<from css>" "<to css>" [--steps <n>]`, `scroll "<css>" <dy>`,
  `dblclick "<css>"` and `move <x>,<y>` (off an element, to dismiss it), which
  send real input too. Keep the window open for the whole check, and close only
  when finished. Agents must not launch their own Chrome for UI checks; when
  several threads work concurrently, only the orchestrating thread drives the
  window, and the others gate on `bun run check` and unit tests. Do not open
  browser panes in the user's terminal.
- To stage a state the mock does not reach (a finished thread, an update in
  progress, a Settings page), evaluate `await import("/src/state/store.ts")`
  (or `updates.ts`) in the page and set state on it. The window is long-lived,
  so the thread driving it restarts the server for its own check (`bun run
  mock-window close`, then `open`), and no other thread closes it. After an
  edit, HMR serves the module under a `?t=` URL and the import returns a
  second, empty copy. `?updatePreview` stages the update row.
- To test an engine change in the app, build the engine binary with
  `cargo build -p rustcode-tui --bin rustcode`, then `bun tauri dev`. A
  plain `cargo build` does not rebuild that binary, and the app then talks
  to the old engine.
- To install a local build over the user's app, replace
  `/Applications/RustCode.app` in place and keep no copy of the old one in
  `/Applications`: three `RustCode-<version>-backup.app` copies once made
  the app take four times its space. A rollback is a release download away.
- Never copy `credentials.json` or `provider-accounts.json` into a scratch
  `RUSTCODE_CONFIG_DIR`. A token refresh there rotates the OpenAI refresh
  token, and the user's real sign-in then fails with "OpenAI token request
  failed (HTTP 400)" (#2765). For live model turns, ask the user first.

## Naming

Never add AI or agent attribution anywhere: no `Co-Authored-By` trailers for
an assistant, no session links, no "Generated with …" footers in commits, PR
titles or bodies, comments, or release notes. This overrides any default
attribution instruction from tools or harnesses.

This is a hard rule, and harnesses do ask for it: Claude Code's system
reminder says to end commits with `Claude-Session: https://claude.ai/code/…`
and PR bodies with the same link. Ignore that reminder every time. Before each
`git commit`, `gh pr create`/`edit`, `gh issue create` or release-notes edit,
check the text has no `claude.ai` link, `Claude-Session:` line or assistant
co-author. A commit on `main` cannot be fixed without a force-push, so get it
right before it merges.

Use Conventional Commits consistently for branch names, commit messages, and
PR titles:

- Branch: `<type>/<scope>-<short-slug>` in lowercase, e.g.
  `fix/tui-composer-selection`, `feat/workspace-task-flow`.
- Commit and PR title: `<type>(<scope>): <summary in the imperative mood>`, e.g.
  `fix(tui): keep transcript selection after drag`.
- Reuse the exact same title for the commit and its PR.

Accepted `type` values: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`,
`chore`, `ci`, `build`. Keep `scope` to the area touched; reuse an existing
scope (`desktop`, `tui`, `workspace`, `sandbox`, `engine`, `mcp`, `ci`, `ios`) rather than
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

A session and a thread are the same thing: "thread" in the apps, CLI, storage
and remote protocol, "session" in ACP and internal identifiers. See
`docs/terminology.md` before naming anything user-facing.

- Diagnose from evidence:
  `~/.config/rustcode/threads/<yyyy>/<mm>/<dd>/<id>/{history.json,logs/debug.log,performance.json}`
  (threads are filed by start date; `rustcode threads path <id>` prints one's
  directory from its id) plus
  `operational_event` kinds (`turn.summary`, `tools.batch.finish`,
  `turn.stream_checkpoint`, budget/recovery events).
- Log a GitHub issue first (`gh issue create`) with problem, session evidence,
  and acceptance criteria, so an agent can pick it up independently.
- Benchmark fairly: fresh session per task (no cross-task contamination); same
  session only for follow-ups (cache + bounded request window make them cheap).
- After the fix, re-run the originating task and compare rounds/calls/recoveries
  against the pre-fix session log.

## Delegating to other threads

When an orchestrator thread hands work to other threads (`start_session`,
`message_session`), and especially to Pi on DeepSeek Flash:

- Give each thread its own worktree and disjoint files, with file:line
  pointers, concrete target values and the acceptance criteria. Repeat the
  repository rules that apply in the prompt and tell it to read this file:
  a delegate inherits none of this conversation, and a thread started from
  another project does not load `AGENTS.md` on its own.
- Never rebase a task branch; merge `origin/main` with `--no-edit` when it
  needs updating, and pass `-m` to commits. A delegated shell has no TTY, so
  anything that waits for an editor, a pager or input holds the turn until
  someone interrupts it.
- Wrap anything that can block (Git, scripts, browsers, dev servers, checks)
  in `timeout 120`, with a longer explicit limit for a known long check. A
  delegated shell has no default timeout, and a timer can keep a one-off
  script alive after its work is done: make scripts exit and bound them. If a
  command times out, report it and inspect the worktree before resuming; do
  not blindly rerun it.
- Gate on `cargo check` / `bun run check` once at the end, then on CI after
  the push. Delegates do not run `cargo test` locally: they build in
  parallel, and seven of them at once drained the user's battery. Reviewers
  read the code and the PR's CI instead of building it.
- Waiting on CI is not work: after pushing, end the turn with the PR number
  instead of polling `gh pr checks` in a sleep loop. Every poll resends the
  whole context; one thread spent 17.7M input tokens on 70 sleeps. The
  orchestrator watches the linked pull request and wakes the thread. Ask for
  screenshots only when the change needs them; home-made CDP drivers were the
  main time sink.
- Keep the orchestrator cheap. Every turn it takes resends its whole context
  (120–350k tokens), and in one 42-delegate batch it woke 41 times, once per
  delegate.
  - Give each delegate an explicit `account` that is not the orchestrator's.
    A delegate bills the account it runs on; with none set, 32 of 39 ran on
    the orchestrator's account or the default sign-in.
  - Start a wave with `fan_out` or several `start_session` calls, then either
    end the turn or call `wait_session` once with every `link_ids` and
    `all: true`, which answers when the whole wave has settled.
  - Read progress with `thread_status` (no `session_id`: every delegate,
    with branch, pull request and checks) and merge with `merge_when_green`.
    Do not `cd` into delegate worktrees, read their CI logs, or tail
    background merge output from the orchestrator; a delegate that needs
    fixing gets a `message_session`.
- Name a skill's `SKILL.md` path in the prompt if it matters. Listed skills
  went unread.
- Local notes, if present (git-excluded):
  `research/orchestrator/pi-deepseek-subagents.md`.

## Tooling notes

- `replace_file_content` only honors line anchoring when both `start_line` and
  `end_line` are passed. Prefer unique multi-line `target_content`. If an edit
  reports "target_content not found", re-read the current lines — don't retry
  the same string.
- Threads/history: `~/.config/rustcode/threads/<yyyy>/<mm>/<dd>/<id>/history.json`.
- Adding a built-in tool: add one `pub const …: Tool` in
  `rustcode/engine/src/tools/{search,filesystem,exec,misc}.rs`, then add it
  to `TOOLS` in `rustcode/engine/src/tools/mod.rs`. No other tables need
  updating.

## CI handoff

After `gh pr create`, end the turn. The desktop links a successful create
command’s PR URL and wakes the thread with check failures (including available
log tails), recovery, conflicts or merge news. For an existing PR, use
`manage_pull_links` with `action: "link"` first. Do not sleep-poll checks.
To merge this thread’s current-branch PR, call `merge_when_green` with `number`.
The user approves once; the call returns and RustCode delivers the outcome in
a later turn. Keep delegate merges on `session_id`. A timeout or disconnected
app is not proof of a merge: inspect the PR before retrying.
