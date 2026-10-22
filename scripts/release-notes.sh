#!/usr/bin/env bash
# release-notes.sh — draft changelog notes for the next release.
#
# Usage:
#   scripts/release-notes.sh [--since v0.88.1] > notes.md
#
# Lists the PRs merged into main since the last tag (or --since), oldest
# first. Each PR contributes the line under its "## Changelog" heading, or its
# title when it has none; release commits are skipped. Edit the draft, then
# pass it to `scripts/release.sh --notes-file`.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
since=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --since) since="$2"; shift 2 ;;
        -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done
[[ -n "$since" ]] || since="$(git -C "$ROOT" describe --tags --abbrev=0 origin/main)"

git -C "$ROOT" log --reverse --format='%s' "$since..origin/main" \
    | grep -Eo '\(#[0-9]+\)$' | tr -d '(#)' \
    | while read -r number; do
        title="$(gh pr view "$number" --json title --jq .title)"
        [[ "$title" == "chore(ci): release "* ]] && continue
        line="$(gh pr view "$number" --json body --jq .body \
            | awk '/^## Changelog/ { found = 1; next } found && /^## / { exit } found && NF { print; exit }')"
        line="${line#- }"
        printf -- '- %s (#%s)\n' "${line:-$title}" "$number"
    done
