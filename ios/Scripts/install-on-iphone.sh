#!/usr/bin/env bash
# Install a published GitHub CI build; use --local for unpublished source changes.
set -euo pipefail
script_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
exec python3 "$script_root/ios-github-release.py" "$@"
