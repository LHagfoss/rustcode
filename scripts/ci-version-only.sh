#!/usr/bin/env bash
# Succeed only when a PR changes release notes and version literals: Cargo
# manifests and the iOS app's MARKETING_VERSION and CURRENT_PROJECT_VERSION.
set -euo pipefail

base="${1:?base revision required}"
head="${2:-HEAD}"
paths="$(git diff --name-only "$base...$head")"
[[ -n "$paths" ]] || exit 1

while IFS= read -r path; do
    case "$path" in
        CHANGELOG.md|Cargo.toml|Cargo.lock|rustcode/*/Cargo.toml|rustcode/core/*/Cargo.toml) ;;
        ios/RustCode.xcodeproj/project.pbxproj) ;;
        *) exit 1 ;;
    esac
done <<< "$paths"

# A manifest/lockfile edit beyond the literal version bump must run checks.
if git diff --unified=0 "$base...$head" -- Cargo.toml Cargo.lock ':(glob)rustcode/**/Cargo.toml' |
    grep -E '^[+-]' |
    grep -Ev '^(\+\+\+ |--- )|^[+-]version = "[^"]+"$'; then
    exit 1
fi
if git diff --unified=0 "$base...$head" -- ios/RustCode.xcodeproj/project.pbxproj |
    grep -E '^[+-]' |
    grep -Ev '^(\+\+\+ |--- )|^[+-][[:space:]]*(MARKETING_VERSION|CURRENT_PROJECT_VERSION) = [0-9.]+;$'; then
    exit 1
fi
