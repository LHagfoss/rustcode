#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

bash -n scripts/preflight.sh

help_out="$(bash scripts/preflight.sh --help)"
grep -q "Usage:" <<<"$help_out"

dry_out="$(bash scripts/preflight.sh --all --dry-run --skip-cargo-check)"
for expected in \
  "Comment ratchet" \
  "Rust file size caps" \
  "Frontend seam" \
  "Version consistency" \
  "Rust formatting" \
  "Rust workspace check skipped" \
  "Desktop check" \
  "CI script tests"; do
  if ! grep -Fq "$expected" <<<"$dry_out"; then
    echo "FAIL: missing '$expected' in --dry-run output" >&2
    exit 1
  fi
done

echo "PASS: preflight script checks"
