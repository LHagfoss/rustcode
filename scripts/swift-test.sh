#!/usr/bin/env bash
# swift-test.sh — run the macOS app's tests and print only what matters.
#
# Usage:
#   scripts/swift-test.sh                 # every test
#   scripts/swift-test.sh SessionFlowTests # a filter, as `swift test --filter`
#
# `swift test` floods the log with Contacts framework errors from the test
# host, and a target-qualified filter such as `RustCodeTests.X` silently runs
# nothing. This prints each suite's count, every failure, and exits with
# `swift test`'s status; a filter that matches no test is an error.

set -uo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$(mktemp -t rustcode-swift-test)"
trap 'rm -f "$LOG"' EXIT

args=()
[[ $# -gt 0 ]] && args=(--filter "$1")

swift test --package-path "$ROOT/macos" "${args[@]}" >"$LOG" 2>&1
status=$?

grep -v '^CoreData' "$LOG" \
    | grep -E 'error: |Test Case .* failed|Executed [0-9]+ tests?' \
    | sort -u
executed=$(grep -Eo 'Executed [0-9]+ tests?' "$LOG" | awk '{ if ($2 > max) max = $2 } END { print max + 0 }')

if [[ $status -ne 0 ]]; then
    echo "swift test failed (exit $status)"
elif [[ $# -gt 0 && $executed -eq 0 ]]; then
    echo "No test matched '$1'. Filter by class or test name, not by target." >&2
    status=1
else
    echo "swift test passed"
fi
exit $status
