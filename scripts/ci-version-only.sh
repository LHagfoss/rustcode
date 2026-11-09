#!/usr/bin/env bash
# Succeed only when a PR changes release notes and version literals in the
# Cargo manifests.
#
#   ci-version-only.sh <base> [head]   read the diff and decide
#   ci-version-only.sh --prefilter     read changed paths on stdin and decide
#
# The prefilter is the same allow-list without the diff, for a CI job that has
# to choose its checkout before it can diff anything: it can only rule a PR out
# of the release-metadata path, and the diff mode rechecks it afterwards.
set -euo pipefail

is_release_metadata_path() {
    case "$1" in
        CHANGELOG.md|Cargo.toml|Cargo.lock|rustcode/*/Cargo.toml) ;;
        *) return 1 ;;
    esac
}

if [[ "${1:-}" == "--prefilter" ]]; then
    seen=false
    while IFS= read -r path; do
        [[ -n "$path" ]] || continue
        is_release_metadata_path "$path" || exit 1
        seen=true
    done
    # An empty list says nothing about the change.
    if "$seen"; then exit 0; fi
    exit 1
fi

base="${1:?base revision required}"
head="${2:-HEAD}"
paths="$(git diff --name-only "$base...$head")"
[[ -n "$paths" ]] || exit 1

while IFS= read -r path; do
    is_release_metadata_path "$path" || exit 1
done <<< "$paths"

# A manifest/lockfile edit beyond the literal version bump must run checks.
if git diff --unified=0 "$base...$head" -- Cargo.toml Cargo.lock ':(glob)rustcode/**/Cargo.toml' |
    grep -E '^[+-]' |
    grep -Ev '^(\+\+\+ |--- )|^[+-]version = "[^"]+"$'; then
    exit 1
fi
