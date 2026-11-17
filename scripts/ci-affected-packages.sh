#!/usr/bin/env bash
# Read changed paths on stdin and print Cargo packages including consumers.
# Output is a fixed list of Cargo arguments, safe to pass as shell words.
set -euo pipefail

all=false
engine=false
tui=false
client=false

while IFS= read -r path; do
    case "$path" in
        Cargo.toml|Cargo.lock|rust-toolchain*|.cargo/*)
            all=true ;;
        # The committed contract and the engine's gateway handshake are what the
        # client's frames are mirrored against: a wire change on either side
        # must run the client's golden-contract tests too (#3240).
        docs/remote-protocol/*|rustcode/engine/src/remote_gateway/*)
            engine=true
            client=true ;;
        rustcode/engine/src/*|rustcode/engine/tests/*|rustcode/engine/build.rs|src/*|tests/*|build.rs)
            engine=true ;;
        rustcode/tui/Cargo.toml|rustcode/tui/build.rs|rustcode/tui/src/*|rustcode/tui/tests/*|rustcode/tui/benches/*|rustcode/tui/examples/*)
            tui=true ;;
        rustcode/client/Cargo.toml|rustcode/client/build.rs|rustcode/client/src/*|rustcode/client/tests/*|rustcode/client/benches/*|rustcode/client/examples/*)
            client=true ;;
        rustcode/core/*/Cargo.toml|rustcode/core/*/build.rs|rustcode/core/*/src/*|rustcode/core/*/tests/*|rustcode/core/*/benches/*|rustcode/core/*/examples/*)
            # Core crates are shared across the workspace. Selecting only the
            # changed crate misses behavioral tests in its consumers.
            all=true ;;
    esac
done

if "$all"; then
    echo '--workspace'
    exit 0
fi

# Both libraries feed the TUI; compile and test their consumer as well.
if "$engine" || "$client"; then tui=true; fi

args=()
if "$engine"; then args+=(-p rustcode); fi
if "$tui"; then args+=(-p rustcode-tui); fi
if "$client"; then args+=(-p rustcode-client); fi
if ((${#args[@]})); then printf '%s ' "${args[@]}"; fi
printf '\n'
