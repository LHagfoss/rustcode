#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd -P)"
matcher="$script_dir/ci-affected-packages.sh"

check() {
    local actual
    actual="$(printf '%s\n' "$2" | bash "$matcher")"
    if [[ "$actual" != "$3" ]]; then
        printf 'not ok - %s: got <%s>, expected <%s>\n' "$1" "$actual" "$3" >&2
        exit 1
    fi
    printf 'ok - %s\n' "$1"
}

check 'TUI only' 'rustcode/tui/src/ui.rs' '-p rustcode-tui '
check 'engine only' 'rustcode/engine/src/lib.rs' '-p rustcode '
check 'remote protocol contract' 'docs/remote-protocol/v1/frame.schema.json' '-p rustcode '
check 'remote protocol prose' 'docs/remote-protocol/README.md' ''
check 'core only' 'rustcode/core/rustcode-tools/src/lib.rs' '-p rustcode-tools '
check 'desktop only' 'rustcode/desktop/src/main.rs' ''
check 'workflow only' '.github/workflows/ci.yml' ''
check 'crate README only' 'rustcode/tui/README.md' ''
check 'multiple packages are ordered' $'rustcode/tui/src/ui.rs\nrustcode/core/rustcode-tools/src/lib.rs\nrustcode/core/rustcode-command/src/lib.rs' '-p rustcode-tui -p rustcode-command -p rustcode-tools '
check 'lockfile covers all' $'rustcode/tui/src/ui.rs\nCargo.lock' '--workspace --exclude rustcode-app'
