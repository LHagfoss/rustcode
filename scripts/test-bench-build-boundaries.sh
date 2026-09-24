#!/usr/bin/env bash
# Smoke-test the benchmark harness safety guards without compiling the project.
set -uo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd -P)"
benchmark="$script_dir/bench-build-boundaries.sh"
repo_root="$(cd "$script_dir/.." && pwd -P)"
tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/rustcode-build-boundary-smoke.XXXXXX")" ||
    exit 1
outside_target="/var/tmp/rustcode-build-boundary-smoke.$$"
failures=0

cleanup() {
    rm -rf -- "$tmp_root" "$outside_target"
}
trap cleanup EXIT INT TERM

pass() {
    echo "ok - $1"
}

fail() {
    echo "not ok - $1" >&2
    failures=$((failures + 1))
}

expect_rejection() {
    local label="$1"
    shift
    local output
    if output="$("$benchmark" "$@" 2>&1)"; then
        fail "$label (accepted unsafe target)"
    elif [[ "$output" == *"allow-cargo-clean"* ||
        "$output" == *"temporary root"* ]]; then
        pass "$label"
    else
        fail "$label (unexpected error: $output)"
    fi
}

if bash -n "$benchmark"; then
    pass "benchmark script has valid shell syntax"
else
    fail "benchmark script has valid shell syntax"
fi

if "$benchmark" --help >/dev/null; then
    pass "benchmark help is available"
else
    fail "benchmark help is available"
fi

clock_output="$("$benchmark" --clock-smoke-test 2>&1)" || {
    fail "millisecond clock reports a positive subsecond interval: $clock_output"
    clock_output=""
}
if [[ -n "$clock_output" ]]; then
    clock_elapsed="${clock_output##*elapsed_ms=}"
    if [[ "$clock_elapsed" =~ ^[1-9][0-9]*$ ]]; then
        pass "millisecond clock reports a positive subsecond interval ($clock_output)"
    else
        fail "millisecond clock reports a positive subsecond interval (unexpected: $clock_output)"
    fi
fi

expect_rejection "path traversal is rejected after canonicalization" \
    --target-dir "/tmp/../var/tmp/rustcode-build-boundary-smoke.$$" \
    --allow-cargo-clean

symlink_target="$tmp_root/outside-link"
if ln -s /var/tmp "$symlink_target"; then
    expect_rejection "symlink escape outside temporary roots is rejected" \
        --target-dir "$symlink_target" \
        --allow-cargo-clean
else
    fail "create symlink for escape test"
fi

repository_link="$tmp_root/repository-link"
if ln -s "$repo_root" "$repository_link"; then
    expect_rejection "symlink to repository is rejected" \
        --target-dir "$repository_link" \
        --allow-cargo-clean
else
    fail "create repository symlink for escape test"
fi

expect_rejection "temporary root itself is rejected" \
    --target-dir /tmp \
    --allow-cargo-clean

expect_rejection "repository shared target is rejected" \
    --target-dir "$repo_root/target" \
    --allow-cargo-clean

if ((failures > 0)); then
    echo "$failures safety smoke test(s) failed" >&2
    exit 1
fi
echo "all benchmark safety smoke tests passed"
