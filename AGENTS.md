# AGENTS.md

## Scope and instruction priority

This repository is the native SwiftUI iOS client for RustCode. The Rust host
lives separately in `~/code/rustcode`. These instructions adapt that project's
workflow to this app; host-specific Cargo commands and engine rules do not apply.
Follow explicit user instructions first, then repository guidance over generic
skills and workflow recipes, subject to higher-priority system instructions.

## Workflow

- Inspect the branch, working tree and existing behavior before changing code.
  Make the smallest scoped change and preserve unrelated work.
- Work in the active checkout on a task branch. Create it with
  `git switch -c <type>/<scope>-<slug>`. Use a separate worktree only for
  concurrent work or unrelated dirty work in the active checkout.
- Never discard the user's work: do not rebase, reset hard, force-push, or stash
  and drop their changes to make a task easier. Work around unrelated edits.
- Keep clean checkouts synced with `git pull --ff-only` at task start and finish.
  On a task branch, pull its upstream; update `main` before branching from it.
  If local changes prevent a safe pull, preserve them and explain the limitation.
- Commit explicit paths, push the task branch and open a PR. Inspect any checks.
  When the user asks for a PR, leave it open; merge when they authorize merging.
- After an authorized merge, return to `main` and run
  `git pull --ff-only origin main`. Confirm local HEAD matches `origin/main`
  and report the working-tree state.
- Remove task branches and temporary worktrees after merging, only after
  checking that no unique work is lost. For squash merges, verify the content;
  ancestry alone is insufficient. Keep an open PR's branch available.

## Naming

Use Conventional Commits for branch names, commits and PR titles:

- Branch: `<type>/<scope>-<short-slug>`, e.g. `fix/ios-composer-focus`.
- Commit and PR: `<type>(<scope>): <imperative summary>`, e.g.
  `fix(ios): preserve the draft when dismissing the keyboard`.
- Reuse the main change's commit title for the PR. Use a draft PR for unfinished
  work rather than a `wip:` prefix.
- Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `chore`, `ci`, `build`.
  Prefer existing scopes such as `app`, `ios`, `remote`, `docs` and `ci`.
  Keep titles concise, with no trailing period.

## Search and architecture

- Start with `rg` or `rg --files` for exact strings and files. Use available
  semantic or structural search when the location or syntax is unclear, and
  verify the source before acting.
- Search for existing behavior before adding helpers or dependencies. Prefer
  project code, then platform APIs, then a small local implementation.
- App views use `RemoteStore`; the store uses `RemoteClient`. Keep transport and
  protocol DTOs behind the client/codec boundary. Put presentation parsing in
  `RustCodeRemote/Presentation` and native rendering in `RustCode/Views`.
- Preserve raw transcript content. Keep streaming presentation safe for partial
  Markdown and thinking delimiters without changing literal code examples.
- Check the actual host contract before adding controls or metadata. Do not
  invent model choices, timing, completion timestamps or operations the host
  does not expose. Keep unknown values and absent optional fields compatible.
- Refresh protocol schemas and golden frames with
  `Scripts/sync-protocol.sh /path/to/rustcode [ref]`; record the source revision
  through that script. Host changes belong in the host repository.

## Verification

- For package behavior changes, run `swift test --package-path RustCodeRemote`.
  Add meaningful regressions for bugs in streaming, mapping or state handling.
- For app changes, build the shared scheme:
  `xcodebuild -project RustCode.xcodeproj -scheme RustCode -destination 'generic/platform=iOS Simulator' build CODE_SIGNING_ALLOWED=NO`.
- For touch, focus, keyboard or transcript interaction changes, run the relevant
  `RustCodeUITests` on an installed iPhone Simulator. Discover destinations with
  `xcodebuild -showdestinations -project RustCode.xcodeproj -scheme RustCode`.
  Example:
  `xcodebuild test -project RustCode.xcodeproj -scheme RustCode -destination 'platform=iOS Simulator,name=iPhone 17 Pro'`.
- Visually check layout changes, including expanded thinking and the keyboard.
  A build passing does not establish correct touch handling or real-device UX.
- Real-gateway tests are opt-in using `RUSTCODE_GATEWAY_BIN=/path/to/rustcode`.
  Report when they were skipped; distinguish demo, Simulator, real host and
  physical phone evidence. Do not rerun unrelated suites for documentation edits.
- Keep signing settings in ignored `Config/Local.xcconfig`. Preserve it and
  never commit personal team IDs, credentials or pairing tokens.
- When the user asks to run on their phone, use the available paired device and
  existing signing configuration. Verify build, installation and launch
  separately; report any blocked step precisely. Preserve paired app data.
- Before publishing, run `git diff --check`, inspect the final diff and report
  actual checks and any remaining limits. Do not claim checks that did not run.
