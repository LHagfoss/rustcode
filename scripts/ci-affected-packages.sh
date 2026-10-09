#!/usr/bin/env bash
# Read changed paths on stdin and print the affected non-desktop Cargo packages.
# Output is a fixed list of Cargo arguments, safe to pass as shell words.
set -euo pipefail

all=false
engine=false
tui=false
core=()

while IFS= read -r path; do
    case "$path" in
        Cargo.toml|Cargo.lock|rust-toolchain*|.cargo/*)
            all=true ;;
        rustcode/engine/src/*|rustcode/engine/tests/*|rustcode/engine/build.rs|src/*|tests/*|build.rs)
            engine=true ;;
        # Generated from the engine's wire types; its tests detect drift.
        docs/remote-protocol/v1/*)
            engine=true ;;
        rustcode/tui/Cargo.toml|rustcode/tui/build.rs|rustcode/tui/src/*|rustcode/tui/tests/*|rustcode/tui/benches/*|rustcode/tui/examples/*)
            tui=true ;;
        rustcode/core/*/Cargo.toml|rustcode/core/*/build.rs|rustcode/core/*/src/*|rustcode/core/*/tests/*|rustcode/core/*/benches/*|rustcode/core/*/examples/*)
            crate="${path#rustcode/core/}"
            crate="${crate%%/*}"
            if [[ "$crate" == rustcode-* ]]; then
                core+=("$crate")
            fi ;;
    esac
done

if "$all"; then
    echo '--workspace --exclude rustcode-app'
    exit 0
fi

args=()
if "$engine"; then args+=(-p rustcode); fi
if "$tui"; then args+=(-p rustcode-tui); fi
if ((${#core[@]})); then
    while IFS= read -r crate; do args+=(-p "$crate"); done < <(printf '%s\n' "${core[@]}" | LC_ALL=C sort -u)
fi
if ((${#args[@]})); then printf '%s ' "${args[@]}"; fi
printf '\n'
