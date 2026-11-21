#!/usr/bin/env bash
# Succeed when every workspace crate, both lockfiles' entries for them and
# the desktop shell's lockfile carry the root package's version. CI runs it
# on a release PR in place of the compiled checks it skips there
# (scripts/ci-version-only.sh).
#
#   ci-version-consistency.sh [repo-root]
set -euo pipefail

root="${1:-.}"
cd "$root"

manifest_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$1" | head -n 1
}

# The version of every unsourced (path) package called NAME in LOCKFILE.
lock_versions() {
    awk -v want="$2" '
        /^\[\[package\]\]/ { if (name == want && !source) print version; name = ""; version = ""; source = 0; next }
        /^name = /    { name = $3; gsub(/"/, "", name) }
        /^version = / { version = $3; gsub(/"/, "", version) }
        /^source = /  { source = 1 }
        END { if (name == want && !source) print version }
    ' "$1"
}

expected="$(manifest_version Cargo.toml)"
[[ -n "$expected" ]] || { echo "No version in Cargo.toml" >&2; exit 1; }

status=0
check() {
    local what="$1" found="$2"
    if [[ "$found" != "$expected" ]]; then
        echo "::error::$what is at version '${found:-<missing>}', expected '$expected'"
        status=1
    fi
}

for manifest in Cargo.toml harness/Cargo.toml rustcode/*/Cargo.toml rustcode/core/*/Cargo.toml; do
    [[ -f "$manifest" ]] || continue
    name="$(sed -n 's/^name = "\(.*\)"$/\1/p' "$manifest" | head -n 1)"
    check "$manifest" "$(manifest_version "$manifest")"
    check "Cargo.lock entry for $name" "$(lock_versions Cargo.lock "$name")"
done

# The desktop shell is its own workspace and links rustcode-client by path.
desktop_lock=apps/desktop/src-tauri/Cargo.lock
if [[ -f "$desktop_lock" ]]; then
    check "$desktop_lock entry for rustcode-client" "$(lock_versions "$desktop_lock" rustcode-client)"
fi

if ((status == 0)); then echo "Every workspace crate and lockfile entry is at $expected"; fi
exit "$status"
