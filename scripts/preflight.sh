#!/usr/bin/env bash
# Unified, VPS-aware pre-push check for agents and developers.
#
# Runs the fast repository lints every time, then runs only the checks matching
# what changed against `--base` (default: origin/main), or everything with `--all`.
# On a shared-target VPS (Linux with ~/.cache/cargo-target or RUSTCODE_SKIP_CARGO_CHECK=1),
# Rust checks stop at `cargo fmt --check` and leave compilation to GitHub Actions.
#
# Usage:
#   scripts/preflight.sh [--all] [--base <ref>] [--dry-run] [--skip-cargo-check] [--with-cargo-check]

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BASE="origin/main"
RUN_ALL=0
DRY_RUN=0
CARGO_CHECK_MODE="auto"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --all) RUN_ALL=1 ;;
    --base)
      shift
      BASE="${1:?--base requires a git ref}"
      ;;
    --dry-run) DRY_RUN=1 ;;
    --skip-cargo-check) CARGO_CHECK_MODE="skip" ;;
    --with-cargo-check) CARGO_CHECK_MODE="run" ;;
    -h|--help)
      sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      exit 2
      ;;
  esac
  shift
done

run_step() {
  local label="$1"
  shift
  echo "==> $label"
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '    [dry-run]'
    printf ' %q' "$@"
    printf '\n'
    return 0
  fi
  "$@"
}

is_vps() {
  if [ "${RUSTCODE_SKIP_CARGO_CHECK:-0}" = "1" ]; then
    return 0
  fi
  if [ "$(uname -s)" = "Linux" ] && [ -f "$HOME/.cargo/config.toml" ] && grep -q 'cargo-target' "$HOME/.cargo/config.toml"; then
    return 0
  fi
  return 1
}

changed_files() {
  if [ "$RUN_ALL" -eq 1 ]; then
    return 0
  fi
  local base_ref="$BASE"
  if ! git rev-parse --verify --quiet "$base_ref" >/dev/null 2>&1; then
    base_ref="HEAD"
  fi
  {
    git diff --name-only "$base_ref"...HEAD 2>/dev/null || true
    git diff --name-only 2>/dev/null || true
    git diff --cached --name-only 2>/dev/null || true
    git ls-files --others --exclude-standard 2>/dev/null || true
  } | sort -u
}

CHANGED="$(changed_files)"
has_match() {
  local pattern="$1"
  [ "$RUN_ALL" -eq 1 ] && return 0
  printf '%s\n' "$CHANGED" | grep -Eq "$pattern"
}

run_step "Comment ratchet" python3 scripts/check-comments.py
run_step "Rust file size caps" python3 scripts/check-rust-size.py
run_step "Frontend seam" bash scripts/check-frontend-seam.sh
run_step "Version consistency" bash scripts/ci-version-consistency.sh

if has_match '^(Cargo\.(toml|lock)|rustcode/|harness/)'; then
  run_step "Rust formatting" cargo fmt --all -- --check
  if [ "$CARGO_CHECK_MODE" = "run" ] || { [ "$CARGO_CHECK_MODE" = "auto" ] && ! is_vps; }; then
    run_step "Rust workspace check" cargo check --workspace --tests
  else
    echo "==> Rust workspace check skipped (VPS mode; GitHub Actions runs compilation)"
  fi
fi

if has_match '^apps/desktop/src-tauri/'; then
  run_step "Desktop Tauri formatting" cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check
  if [ "$CARGO_CHECK_MODE" = "run" ] || { [ "$CARGO_CHECK_MODE" = "auto" ] && ! is_vps; }; then
    run_step "Desktop Tauri clippy" cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
  else
    echo "==> Desktop Tauri clippy skipped (VPS mode; GitHub Actions runs compilation)"
  fi
fi

if has_match '^apps/desktop/'; then
  run_step "Desktop check (deps, typecheck, lint, test, build)" bash -c 'cd apps/desktop && bun run check'
fi

if has_match '^evals/'; then
  run_step "Eval grader tests" bash -c 'cd evals && bun test'
fi

if has_match '^scripts/'; then
  sh_files=()
  if [ "$RUN_ALL" -eq 1 ]; then
    while IFS= read -r f; do sh_files+=("$f"); done < <(find scripts -maxdepth 1 -name '*.sh' | sort)
  else
    while IFS= read -r f; do
      [ -f "$f" ] && sh_files+=("$f")
    done < <(printf '%s\n' "$CHANGED" | grep -E '^scripts/.*\.sh$' || true)
  fi
  for f in "${sh_files[@]}"; do
    run_step "Shell syntax: $f" bash -n "$f"
  done
  run_step "CI script tests" bash -c '
    bash scripts/test-ci-relevant-changes.sh &&
    bash scripts/test-ci-affected-packages.sh &&
    bash scripts/test-ci-version-only.sh &&
    bash scripts/test-check-frontend-seam.sh &&
    bash scripts/test-preflight.sh
  '
fi

echo "Preflight passed."
