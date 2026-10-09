#!/bin/sh
# Refresh the remote protocol contract (JSON Schemas and golden frames) from a RustCode
# checkout into the package's test resources.
#
#   Scripts/sync-protocol.sh <path-to-rustcode> [git-ref]
#
# The files are read from git, not from the working tree, so the result is exactly what
# the named commit contains. The default ref is origin/main; fetch first if you want the
# newest. The source commit is recorded in SOURCE.txt next to the copied files.
set -eu

if [ $# -lt 1 ]; then
    echo "usage: $0 <path-to-rustcode> [git-ref]" >&2
    exit 64
fi

source_repo=$1
ref=${2:-origin/main}
contract=docs/remote-protocol/v1
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
destination=$here/../RustCodeRemote/Tests/RustCodeRemoteTests/Resources/remote-protocol

commit=$(git -C "$source_repo" rev-parse --verify "$ref^{commit}")

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
git -C "$source_repo" archive "$commit" "$contract" | tar -x -C "$staging"

rm -rf "$destination/v1"
mkdir -p "$destination"
cp -R "$staging/$contract" "$destination/v1"

requests=$(find "$destination/v1/golden/requests" -name '*.json' | wc -l | tr -d ' ')
frames=$(find "$destination/v1/golden/frames" -name '*.json' | wc -l | tr -d ' ')

cat > "$destination/SOURCE.txt" <<NOTE
Copied by Scripts/sync-protocol.sh. Do not edit these files by hand.

repository: LHagfoss/rustcode
path:       $contract
ref:        $ref
commit:     $commit
golden:     $requests requests, $frames frames
NOTE

echo "remote protocol synced from $commit ($requests requests, $frames frames)"
