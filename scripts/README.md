# Scripts Index

Quick reference for scripts in `scripts/` (repository-wide) and `apps/desktop/scripts/` (desktop app).

## Pre-push & Repo Checks

| Script | Purpose |
| --- | --- |
| `scripts/preflight.sh` | Unified, VPS-aware pre-push runner (comments, Rust size caps, frontend seam, version check, `cargo fmt`, `apps/desktop` checks, and script tests based on changed files). Supports `--all`, `--base <ref>`, `--dry-run`, `--skip-cargo-check`, `--with-cargo-check`. |
| `scripts/check-comments.py` | Enforces the comment ratchet (`docs/adr/0001-comment-ratchet.md`) against `scripts/comment-baseline.txt`. Pass `--update` after removing comments; `--self-test` tests the lexer. |
| `scripts/check-rust-size.py` | Enforces the 2,000-line cap on Rust files against `scripts/rust-size-baseline.txt`. |
| `scripts/check-frontend-seam.sh` | Verifies the TUI render layer only reaches core through `rustcode::controller` (allow-list: `scripts/frontend-seam-allowlist.txt`). |
| `scripts/ci-version-consistency.sh` | Verifies Cargo, Tauri, and package versions agree across the repo. |

## Desktop Development & Verification (`apps/desktop/scripts/`)

| Script | Purpose |
| --- | --- |
| `ensure-deps.ts` | Checks required binaries (`tsc`, `biome`, `vite`, `playwright`) and `bun.lock` hash stamp; runs `bun install --frozen-lockfile` when missing or stale. |
| `mock-window.ts` | Headless Chrome driver against `VITE_TRANSPORT=mock` (`bun run mock-window open\|click\|type\|hover\|eval\|screenshot\|close`). |
| `ui-shots.ts` / `uiShots.ts` | Captures and diffs light/dark UI screenshots for PR visual review (used by `.github/workflows/ui-screenshots.yml`). |
| `dev-web.ts` | Runs the desktop frontend in a browser paired with `engine-bridge.ts` (`bun run dev:web`). |
| `engine-bridge.ts` / `bridge-native.ts` | Local WebSocket ACP bridge and Bun mirror of Tauri shell commands for browser dev mode (`bun run bridge`). |
| `perf.ts` / `perfBudget.ts` / `perf-budget.json` | Automated desktop UI performance benchmark and budget gate (`bun run perf`). |
| `perf-native.ts` / `perf-native.swift` / `perf-native-scenario.js` | Native macOS WKWebView performance harness (`bun run perf:native`). |
| `settings-perf.ts` | Micro-benchmark for `settingsReadCache` probe deduplication. |

## Release, Packaging & Install

| Script | Purpose |
| --- | --- |
| `scripts/release.sh` | End-to-end release automation (`--patch`, `--minor`, `--beta`, `--no-wait`, `--notes-file`). |
| `scripts/release-notes.sh` | Collects `## Changelog` lines (or PR titles) since the previous release tag. |
| `scripts/macos-release-signing.sh` | macOS Developer ID signing and notarization helper used by release workflows. |
| `scripts/macos-stable-signature.sh` | Stable local macOS code-signature helper. |
| `scripts/install.sh` / `scripts/install.ps1` | End-user installers for macOS/Linux and Windows. |
| `apps/desktop/scripts/bundle-engine.ts` | Builds and stages the `rc` sidecar binary into `src-tauri/binaries/`. |
| `apps/desktop/scripts/bundle-version.ts` | Generates `src-tauri/gen-version/tauri.conf.json` for app bundling. |
| `apps/desktop/scripts/app-icon.ts` | Generates macOS layered app icon assets for bundling. |
| `apps/desktop/scripts/release-assets.ts` | Packages desktop build artifacts and checksums for GitHub Releases. |

## PR, CI & Benchmarks

| Script | Purpose |
| --- | --- |
| `scripts/pr-merge.sh` | Waits for non-advisory CI checks on a PR and squash-merges with `--delete-branch`. |
| `scripts/ci-affected-packages.sh` | Maps changed paths to affected Cargo packages for CI filtering. |
| `scripts/ci-relevant-changes.sh` | Detects whether a diff requires full Rust CI checks. |
| `scripts/ci-version-only.sh` | Detects release metadata-only commits that can skip redundant test suites. |
| `scripts/footprint.py` | Measures release artifact / binary footprint (used by `.github/workflows/footprint.yml`). |
| `scripts/bench-build-boundaries.sh` | Benchmarks incremental Rust build times across workspace boundaries (see `scripts/bench-build-boundaries.md`). |
| `scripts/context-panel-screenshots.sh` | Renders TUI `/context` panel screenshots for all shipped themes. |
| `scripts/test-*.sh` / `scripts/test-install-linux.py` | Unit/integration test suites for CI matchers, frontend seam, build bench, and Linux installer. |
