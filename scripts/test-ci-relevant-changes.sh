#!/usr/bin/env bash
# Deterministic smoke tests for the path filter used by .github/workflows/ci.yml.
set -uo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd -P)"
matcher="$script_dir/ci-relevant-changes.sh"
failures=0

pass() {
    echo "ok - $1"
}

fail() {
    echo "not ok - $1" >&2
    failures=$((failures + 1))
}

expect_relevant() {
    local label="$1"
    local paths="$2"
    if printf '%s\n' "$paths" | bash "$matcher"; then
        pass "$label"
    else
        fail "$label"
    fi
}

expect_irrelevant() {
    local label="$1"
    local paths="$2"
    if printf '%s\n' "$paths" | bash "$matcher"; then
        fail "$label"
    else
        pass "$label"
    fi
}

if bash -n "$matcher"; then
    pass "CI path matcher has valid shell syntax"
else
    fail "CI path matcher has valid shell syntax"
fi

expect_relevant "root Rust source runs code CI" 'src/network.rs'
expect_relevant "workspace library changes run code CI" 'rustcode/core/rustcode-core/src/lib.rs'
expect_relevant "workspace library manifest changes run code CI" 'rustcode/core/rustcode-tools/Cargo.toml'
expect_relevant "engine package source runs code CI" 'rustcode/engine/src/network.rs'
expect_relevant "tui package source runs code CI" 'rustcode/tui/src/ui/render.rs'
expect_relevant "desktop package manifest runs code CI" 'rustcode/desktop/Cargo.toml'
expect_relevant "android app manifest runs code CI" 'apps/android/package.json'
expect_relevant "android app source runs code CI" 'apps/android/app/session.tsx'
expect_relevant "shared protocol package runs code CI" 'packages/protocol/src/index.ts'
expect_relevant "CI helper changes run code CI" 'scripts/ci-relevant-changes.sh'
expect_relevant "build configuration runs code CI" 'Cargo.lock'
expect_relevant "workflow changes run code CI" '.github/workflows/ci.yml'
expect_irrelevant "documentation-only changes skip code CI" 'README.md'
expect_irrelevant "library README changes skip code CI" 'rustcode/core/rustcode-core/README.md'
expect_irrelevant "library documentation changes skip code CI" 'rustcode/core/rustcode-core/docs/usage.md'
expect_irrelevant "image-only changes skip code CI" 'images/header.png'
expect_irrelevant "android app README changes skip code CI" 'apps/android/README.md'

if ((failures > 0)); then
    echo "$failures CI path-filter smoke test(s) failed" >&2
    exit 1
fi
echo "all CI path-filter smoke tests passed"
