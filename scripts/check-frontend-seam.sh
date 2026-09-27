#!/usr/bin/env bash
# Fail if the TUI render layer references core paths outside the frontend seam.
#
# Usage: scripts/check-frontend-seam.sh [--allowlist FILE] [PATH...]
# Defaults to scripts/frontend-seam-allowlist.txt and the TUI render layer
# (rustcode/engine/src/ui + rustcode/engine/src/inline_terminal.rs).
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
allowlist="$script_dir/frontend-seam-allowlist.txt"
paths=()
while (($# > 0)); do
    case "$1" in
        --allowlist)
            (($# >= 2)) || { echo "check-frontend-seam: --allowlist requires a file" >&2; exit 2; }
            allowlist="$2"
            shift 2
            ;;
        --)
            shift
            while (($# > 0)); do paths+=("$1"); shift; done
            ;;
        -*)
            echo "check-frontend-seam: unknown flag: $1" >&2
            exit 2
            ;;
        *)
            paths+=("$1")
            shift
            ;;
    esac
done
if ((${#paths[@]} == 0)); then
    paths=("$repo_root/rustcode/engine/src/ui" "$repo_root/rustcode/engine/src/inline_terminal.rs")
fi
[[ -f "$allowlist" ]] || { echo "check-frontend-seam: allow-list not found: $allowlist" >&2; exit 2; }

observed="$(mktemp)"
allowed="$(mktemp)"
trap 'rm -f -- "$observed" "$allowed"' EXIT INT TERM

set +e
rg -o --no-filename 'crate::[A-Za-z_:]+' "${paths[@]}" 2>/dev/null \
    | sed -E -e 's/^crate::ui(::[A-Za-z_0-9]+)+$/crate::ui/' -e 's/^((crate::[a-z_]+)(::[a-z_]+)?).*/\1/' \
    | sort -u >"$observed"
rg_status="${PIPESTATUS[0]}"
set -e
if ((rg_status > 1)); then
    echo "check-frontend-seam: search failed" >&2
    exit 2
fi

grep -vE '^\s*(#|$)' "$allowlist" | sed -E 's/[[:space:]]+//g' | sort -u >"$allowed"

violations="$(comm -23 "$observed" "$allowed" || true)"
if [[ -n "$violations" ]]; then
    echo "check-frontend-seam: TUI reaches past the frontend seam:" >&2
    while IFS= read -r line; do
        [[ -n "$line" ]] && printf '  %s\n' "$line" >&2
    done <<<"$violations"
    echo "Route the use through rustcode::controller or extend $allowlist." >&2
    exit 1
fi

count="$(wc -l <"$observed" | tr -d ' ')"
echo "check-frontend-seam: clean ($count referenced paths, all allow-listed)"
