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

check 'harness only' 'harness/src/ui.rs' '-p rustcode-harness '
check 'CLI only' 'rustcode/cli/src/main.rs' '-p rustcode-cli '
check 'engine only' 'rustcode/engine/src/lib.rs' '-p rustcode -p rustcode-cli -p rustcode-harness '
check 'remote protocol contract' 'docs/remote-protocol/v1/frame.schema.json' '-p rustcode -p rustcode-cli -p rustcode-harness -p rustcode-client '
check 'remote protocol prose' 'docs/remote-protocol/README.md' '-p rustcode -p rustcode-cli -p rustcode-harness -p rustcode-client '
check 'remote gateway change runs the client contract' 'rustcode/engine/src/remote_gateway/handshake.rs' '-p rustcode -p rustcode-cli -p rustcode-harness -p rustcode-client '
check 'machine client only' 'rustcode/client/src/lib.rs' '-p rustcode-cli -p rustcode-client '
check 'machine client and harness' $'rustcode/client/tests/gateway_round_trip.rs\nharness/src/ui.rs' '-p rustcode-cli -p rustcode-harness -p rustcode-client '
check 'core covers dependents' 'rustcode/core/rustcode-tools/src/lib.rs' '--workspace'
check 'desktop app only' 'apps/desktop/src/main.tsx' ''
check 'workflow only' '.github/workflows/ci.yml' ''
check 'crate README only' 'harness/README.md' ''
check 'shared changes override leaf selection' $'harness/src/ui.rs\nrustcode/core/rustcode-tools/src/lib.rs\nrustcode/core/rustcode-command/src/lib.rs' '--workspace'
check 'lockfile covers all' $'harness/src/ui.rs\nCargo.lock' '--workspace'
check 'shared manifest covers dependents' 'rustcode/core/rustcode-tools/Cargo.toml' '--workspace'
