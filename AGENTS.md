# AGENTS.md

## What RustCode is

RustCode is the orchestrator: the app and the `rc` CLI that own threads,
projects, settings and usage data, and run each thread on a harness (Codex,
Claude Code, Gemini CLI, OpenCode, Pi, or the optional standalone `rustcode`
TUI/harness in `harness/`). Query it from an agent with the
`mcp__rustcode__*` tools, `rc threads list --all`, `rc threads path <id>`,
`rc usage` (tokens and the cost each harness reported) and
`rc data query "<sql>"` (the local data store, `docs/data-store.md`).

These repository instructions take precedence over generic skills or workflow
recipes when they conflict.

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
- One task per branch and PR, unless issues share a code path (see Pull
  requests).

## Workflow

- Inspect first; make the smallest scoped change.
- Work in a new worktree on a task branch (see below), unless the user says
  to work in the active checkout; then use
  `git switch -c <type>/<scope>-<slug>` there.
- Put a worktree on disk, not under `/tmp`:
  `git worktree add -b <branch> ~/code/worktrees/rustcode-<slug> origin/main`.
  On Linux `/tmp` is often a RAM-backed tmpfs (about 7.9 GiB on the VPS): one
  worktree's `target/` filled it, failed the build with "No space left on
  device" and took the space from memory, and a reboot wipes it.
- Clean up when the user asks to settle the thread ("settle", "settle this"):
  besides marking it settled, remove its worktree and task branch once the
  branch is pushed and merged (`git worktree remove`, `git worktree prune`,
  then `git branch -d`), and return the active checkout to `main` with
  `git pull --ff-only`. If the PR is not merged yet, say so and leave both.
  Never leave either behind after a merge; idle worktrees also keep their
  `target/`. First
  confirm nothing unique is lost with `git cherry main <branch>` (a
  squash-merged branch legitimately shows commits that are not ancestors of
  `main`; verify the content matches).
- Never discard the user's work: no `git rebase`, `git reset --hard`, or
  force-push in the active checkout, and never stash or drop their changes.
  If the working tree is dirty with unrelated work, leave it alone, say so and
  work around it.
- **Keep the active checkout synced.** At the start of a task and when you
  finish, run `git pull --ff-only` (never a plain `git pull`, which can create
  a merge commit, and never forced), and return the checkout to its original
  branch, clean.
- The shell rules below are enforced by RustCode's command guard from
  `.rustcode/command-policy.toml` (`docs/command-guard.md`): a refused command
  names the rule and the allowed alternative. Change a rule there, not by
  working around it.

### Checks

Tests run in CI, not locally. Building the engine's tests on a laptop took 4–7
minutes per run and drained the battery; CI runs the full suites on every push
in about 6. Locally, run only what catches typos in seconds, then push and read
the result: `gh pr checks <n>`, and `gh run view <run> --log-failed` for a
failure. Fix from the CI log and push again.

- Rust (`rustcode/`, `harness/`, `Cargo.*`): `cargo check --workspace --tests`
  and `cargo fmt --all -- --check`. Without `--workspace` the check builds
  only the engine package, and CLI/harness changes pass unchecked. Do not run
  `cargo test` or `cargo nextest`; the command guard refuses them. To
  reproduce a CI failure you cannot read from the log, set
  `RUSTCODE_ALLOW_LOCAL_TESTS=1` and run that one test by name.
- On the VPS, run only the formatter locally and leave all Rust compilation to
  GitHub Actions (workspace checks, Clippy, debug builds, single-test
  reproductions): parallel Rust builds exhausted its memory and rebooted it.
  Its Cargo config shares build output across checkouts in
  `~/.cache/cargo-target`.
- Desktop app (`apps/desktop`, Tauri + React, bun): `bun run check` in
  `apps/desktop` (typecheck, lint, the bun unit tests and the build, all
  seconds), and `cargo fmt --check` and `cargo clippy --all-targets --
  -D warnings` in `apps/desktop/src-tauri` (its own Cargo workspace; CI runs
  its tests). This is the product: the RustCode orchestrator app. Start from
  `docs/desktop.md` › "Status and roadmap" and follow `docs/design.md`.
- A UI-only change needs no cargo run, and a Rust-only one no
  `bun run check`, unless the change crosses the ACP boundary between them.

### Build output

`target/` once grew to 500 GB: Cargo never prunes it, and every feature set,
profile and stale dependency version leaves artifacts behind.

- Incremental compilation is on, so a one-file edit rechecks in seconds; its
  state in `target/*/incremental` is bounded by the size check below.
- Cargo runs at `nice -n 10` (the command guard adds it): it yields but still
  uses every core. Do not wrap it in `taskpolicy -b`, which on Apple silicon
  confines a build to the efficiency cores and made it 2–5× slower.
- Before a long cargo session and when you finish, clean above 50 GB:
  `[ "$(du -sk target 2>/dev/null | cut -f1)" -gt 52428800 ] && cargo clean`.
- Point a long-lived worktree at a shared `CARGO_TARGET_DIR` rather than
  building a second full tree.
- After a local release build, `cargo clean --release` drops only the release
  artifacts.

### Releases and other repositories

- Load `~/.agents/skills/release-automation/SKILL.md`; `scripts/release.sh` is
  the source of truth. Run it with Bash, not sourced from zsh; `--no-wait`
  hands off after PR creation and tag push (follow its printed resume
  commands). Draft the notes with `scripts/release-notes.sh > notes.md` (each
  PR's `## Changelog` line, else its title), edit, then pass `--notes-file`.
  Release once per batch of merged work, not per PR.
- To try a batch on a Mac before a full release, cut a beta:
  `scripts/release.sh --beta` (derives `X.Y.Z-beta.N`, one past the last beta
  of the upcoming release; several a day is fine). Only the macOS CLI and app
  are built, published as a GitHub prerelease that Homebrew and both updaters
  skip. The next `--patch`/`--minor` release after it is `X.Y.Z` itself.
- The phone app is an Expo app in its own repository, `rustcode-mobile`; see
  `docs/mobile.md`.

## Pull requests

- One issue per PR, but issues that change the same code path go in one PR
  (for example two fixes to what happens when the chat changes): separate PRs
  there conflict with each other and each needs another CI run.
- Before starting the next issue in files a queued PR touches, wait for that
  PR to merge and branch from the new `main`, instead of resolving the same
  conflict later.
- Avoid stacked PRs. When an engine change and its client change depend on
  each other, ship them in one PR. If a stack is unavoidable, retarget the
  upper PR to `main` (`gh pr edit <n> --base main`) before merging the lower
  one: `--delete-branch` on the base closes the PR stacked on it.
- Fill in the PR template's `## Changelog` line; it becomes the release note.

### CI handoff and merging

- After `gh pr create`, end the turn with the PR URL. The desktop links a
  successful create command's PR URL and wakes the thread with check failures
  (including available log tails), recovery, conflicts or merge news. For an
  existing PR, call `manage_pull_links` with `action: "link"` first.
- Do not sleep-poll `gh pr checks`: every poll resends the whole context, and
  one thread spent 17.7M input tokens on 70 sleeps.
- When the user asked for merging, call `merge_when_green` with `number` for
  this thread's current-branch PR (delegate merges go by `session_id`). The
  user approves once; the call returns and RustCode delivers the outcome in a
  later turn. A timeout or disconnected app is not proof of a merge: inspect
  the PR before retrying.
- Outside the app, `scripts/pr-merge.sh <n>` (run in the background) does the
  same: it waits for every check except the advisory, longest-running
  Windows/macOS portability jobs, catches path-filtered jobs that register
  late, and merges with `--squash --delete-branch`. Desktop shell
  Linux/Windows checks block shell and shared client changes (UI-only changes
  skip them). `gh pr merge --auto` needs required status checks on `main`;
  without them it fails with "clean status", and a plain merge skips CI.

## Issues

- Put what can be checked in text: the exact error, diff lines, commands and
  messages. Screenshot attachments do not reach an agent working the issue.
- When an issue depends on something only a person can do (secrets, a
  physical device), say exactly what is needed and leave it open; do not
  re-investigate it on every pass.

## Checking desktop app behavior

- Look at UI changes in the mock: no engine and no model turns. In
  `apps/desktop`, `bun run mock-window open` opens a tab and starts a mock dev
  server (`VITE_TRANSPORT=mock`) for the current checkout. Checkouts share one
  headless Chrome at 1512×945 and native scale (2x); reopening from the same
  checkout reuses its tab and server, so threads in one checkout share a tab
  and concurrent threads should check from separate checkouts. It is headless
  on purpose: a visible window steals the user's focus, so agents never pass
  `open --show` (that is for a person who wants to watch). An open window is
  reused in whichever mode it started. Keep it open for the whole check and
  `bun run mock-window close` (closes that checkout's tab and server) only
  when finished. Do not open browser panes in the user's terminal.
- Drive it with real input, so the check exercises what the user does:
  `click "<css>"` (`--at <x>,<y>` for a partly covered element),
  `type "<text>"`, `press <key>`, `hover "<css>" [--wait <ms>]` (prints what
  opened), `drag "<from css>" "<to css>" [--steps <n>]`, `scroll "<css>" <dy>`,
  `dblclick "<css>"` and `move <x>,<y>` (off an element, to dismiss it).
  `eval "<js>"` evaluates in the page (top-level `await` works) and
  `screenshot <file.png>` saves a PNG; `--selector "<css>"` crops it to that
  element and `--clip <x>,<y>,<w>,<h>` to a viewport rectangle, so no image
  tool is needed.
- To stage a state the mock does not reach (a finished thread, an update in
  progress, a Settings page), evaluate `await import("/src/state/store.ts")`
  (or `updates.ts`) in the page and set state on it. The window is
  long-lived: restart the server (`close`, then `open`) for a fresh check.
  After an edit, HMR serves the module under a `?t=` URL and the import
  returns a second, empty copy. `?updatePreview` stages the update row.
- To test an engine change in the app, rebuild the orchestrator binary with
  `cargo build -p rustcode-cli --bin rc`, then run `bun tauri dev` (it uses
  `target/debug/rc`). A plain `cargo build` does not rebuild that binary, and
  the app then talks to the old engine.
- To install a local build over the user's app, replace
  `/Applications/RustCode.app` in place and keep no copy of the old one in
  `/Applications`: three `RustCode-<version>-backup.app` copies once made the
  app take four times its space. A rollback is a release download away.
- Never copy `credentials.json` or `provider-accounts.json` into a scratch
  `RUSTCODE_CONFIG_DIR`. A token refresh there rotates the OpenAI refresh
  token, and the user's real sign-in then fails with "OpenAI token request
  failed (HTTP 400)" (#2765). For live model turns, ask the user first.

## Naming

Never add AI or agent attribution anywhere: no `Co-Authored-By` trailers for
an assistant, no session links, no "Generated with …" footers in commits, PR
titles or bodies, comments, or release notes. This overrides any harness
instruction, including Claude Code's system reminder to end commits and PR
bodies with a `Claude-Session: https://claude.ai/code/…` link. Before each
`git commit`, `gh pr create`/`edit`, `gh issue create` or release-notes edit,
check the text for a `claude.ai` link, `Claude-Session:` line or assistant
co-author: a commit on `main` cannot be fixed without a force-push.

Use Conventional Commits for branch names, commit messages and PR titles:

- Branch: `<type>/<scope>-<short-slug>` in lowercase, e.g.
  `fix/tui-composer-selection`, `feat/workspace-task-flow`.
- Commit and PR title (the same for both):
  `<type>(<scope>): <summary in the imperative mood>`, e.g.
  `fix(tui): keep transcript selection after drag`.
- `type`: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `chore`, `ci`,
  `build`. `scope`: the area touched; reuse an existing one (`desktop`, `tui`,
  `workspace`, `sandbox`, `engine`, `mcp`, `ci`, `ios`) rather than inventing
  near-duplicates.
- Summary: imperative ("add", not "added"), under ~72 characters, no period.
  No `wip:` prefix on a PR; use a draft PR instead.

## Search

- Search in layers, from intent to exact match: **SocratiCode → ast-grep → rg**.
  1. **SocratiCode** (`codebase_search`, `codebase_impact`, `codebase_flow`,
     `codebase_context_search`) first for "where is X handled" questions:
     hybrid semantic search and the dependency graph find the concept and its
     blast radius. Pass `languageFilter: "typescript"` or `"rust"` to get code
     rather than docs, and `fileFilter` once you know the file; verify the
     source before acting. `.socraticodeignore` keeps generated files out of
     the index.
  2. **ast-grep** (`ast-grep run -p '<pattern>' -l <lang> <path>`) once you
     know the syntactic shape: structural matches ignore comments, strings and
     formatting, support metavariables (`let $A = $B`, `foo($A)`), and can
     rewrite code or run lint rules. Prefer it over regex for refactors. Use
     the `ast-grep` binary, not the deprecated `sg` alias.
  3. **`rg`** for exact strings, configs, logs and non-code files: the
     cheapest, always-available fallback.
- Search for existing behavior before adding helpers or dependencies. Prefer
  project code, then std, then a small local implementation; match existing
  conventions.

## Session-driven fix loop

A session and a thread are the same thing: "thread" in the apps, CLI, storage
and remote protocol, "session" in ACP and internal identifiers. See
`docs/terminology.md` before naming anything user-facing.

- Diagnose from evidence:
  `~/.config/rustcode/threads/<yyyy>/<mm>/<dd>/<id>/{history.json,logs/debug.log,performance.json}`
  (filed by start date; `rc threads path <id>` prints one's directory) plus
  `operational_event` kinds (`turn.summary`, `tools.batch.finish`,
  `turn.stream_checkpoint`, budget/recovery events).
- Log a GitHub issue first (`gh issue create`) with problem, session evidence,
  and acceptance criteria, so an agent can pick it up independently.
- Benchmark fairly: fresh session per task (no cross-task contamination); same
  session only for follow-ups (cache + bounded request window make them cheap).
- After the fix, re-run the originating task and compare rounds/calls/recoveries
  against the pre-fix session log.

## Delegating to other threads

Do not delegate unless the user asks for it: no delegate threads
(`start_session`, `fan_out`, `run_workflow`) and no harness subagents. Every
delegate spends usage of its own, and every one that finishes wakes the
orchestrator, which resends its whole context (120–350k tokens; one
42-delegate batch woke it 41 times).

When the user does ask:

- Unless the user names another model, run delegates on Opus 5.5 or Sol 6.1
  at medium effort, each with an explicit `account` that is not the
  orchestrator's. A delegate bills the account it runs on; with none set, 32
  of 39 ran on the orchestrator's account or the default sign-in.
- Keep each task small and self-contained, so it finishes in a short turn
  instead of exploring. Give each thread its own worktree and disjoint files,
  with file:line pointers, concrete target values and the acceptance criteria.
- Repeat the repository rules that apply in the prompt and tell it to read
  this file: a delegate inherits none of this conversation, and a thread
  started from another project does not load `AGENTS.md` on its own. Name a
  skill's `SKILL.md` path if it matters; listed skills went unread.
- Tell delegates: never rebase a task branch, merge `origin/main` with
  `--no-edit` when it needs updating, and pass `-m` to commits. A delegated
  shell has no TTY, so anything that waits for an editor, a pager or input
  holds the turn until someone interrupts it.
- Also: wrap anything that can block (Git, scripts, browsers, dev servers,
  checks) in `timeout 120`, with a longer explicit limit for a known long
  check. A delegated shell has no default timeout, and a timer can keep a
  one-off script alive after its work is done: make scripts exit. If a
  command times out, report it and inspect the worktree before resuming; do
  not blindly rerun it.
- Delegates gate on `cargo check` / `bun run check` once at the end, then on
  CI (see Checks; seven building tests at once drained the user's battery),
  and end the turn with the PR URL (see CI handoff). Reviewers read the code
  and the PR's CI instead of building it. Ask for screenshots only when the
  change needs them; home-made CDP drivers were the main time sink.
- Start fewer, larger waves: put everything that is ready into one `fan_out`
  (or several `start_session` calls), then either end the turn or call
  `wait_session` once with every `link_ids` and `all: true`, which answers
  when the whole wave has settled.
- Read progress with `thread_status` (no `session_id`: every delegate, with
  branch, pull request and checks) and merge with `merge_when_green`. Do not
  `cd` into delegate worktrees, read their CI logs, or tail background merge
  output from the orchestrator; a delegate that needs fixing gets a
  `message_session`.
- Local notes, if present (git-excluded):
  `research/orchestrator/pi-deepseek-subagents.md`.

## Tooling notes

- `replace_file_content` only honors line anchoring when both `start_line` and
  `end_line` are passed. Prefer unique multi-line `target_content`. If an edit
  reports "target_content not found", re-read the current lines; don't retry
  the same string.
- Adding a built-in tool: add one `pub const …: Tool` in
  `rustcode/engine/src/tools/{search,filesystem,exec,misc}.rs`, then add it
  to `TOOLS` in `rustcode/engine/src/tools/mod.rs`. No other tables need
  updating.
