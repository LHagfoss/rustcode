#!/usr/bin/env bash
# Read changed paths on stdin and print Cargo packages including consumers.
# Output is a fixed list of Cargo arguments, safe to pass as shell words.
set -euo pipefail

all=false
engine=false
cli=false
harness=false
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
        rustcode/cli/Cargo.toml|rustcode/cli/build.rs|rustcode/cli/src/*|rustcode/cli/tests/*|rustcode/cli/benches/*|rustcode/cli/examples/*)
            cli=true ;;
        harness/Cargo.toml|harness/build.rs|harness/src/*|harness/tests/*|harness/benches/*|harness/examples/*)
            harness=true ;;
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

# Engine feeds both binaries; the machine client feeds the orchestrator CLI (`rc`).
if "$engine"; then
    cli=true
    harness=true
fi
if "$client"; then
    cli=true
fi

args=()
if "$engine"; then args+=(-p rustcode); fi
if "$cli"; then args+=(-p rustcode-cli); fi
if "$harness"; then args+=(-p rustcode-harness); fi
if "$client"; then args+=(-p rustcode-client); fi
if ((${#args[@]})); then printf '%s ' "${args[@]}"; fi
printf '\n'
