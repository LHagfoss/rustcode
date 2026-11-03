#!/usr/bin/env bash
# Merge a PR once its checks pass: `scripts/pr-merge.sh <number>`.
#
# Waits for every check except the advisory portability jobs (Windows and
# macOS compile coverage, `continue-on-error` in ci.yml), which take longest
# and never block a merge. Late-registering, path-filtered jobs are caught by
# waiting until a pass finds nothing pending twice in a row.
set -euo pipefail

pr="${1:?usage: scripts/pr-merge.sh <pr-number>}"
advisory='portability coverage'
quiet=0
for _ in $(seq 1 240); do
    checks="$(gh pr checks "$pr" 2>/dev/null | grep -v "$advisory" || true)"
    if [[ -z "$checks" ]]; then
        quiet=0
    elif grep -q $'\tfail\t' <<<"$checks"; then
        echo "PR #$pr has a failing check:" >&2
        grep $'\tfail\t' <<<"$checks" >&2
        exit 1
    elif grep -q $'\tpending\t' <<<"$checks"; then
        quiet=0
    else
        quiet=$((quiet + 1))
        if ((quiet >= 2)); then
            gh pr merge "$pr" --squash --delete-branch
            exit 0
        fi
    fi
    sleep 20
done
echo "PR #$pr: checks still pending after 80 minutes" >&2
exit 1
