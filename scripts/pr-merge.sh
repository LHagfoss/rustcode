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
            if merge_out="$(gh pr merge "$pr" --squash --delete-branch 2>&1)"; then
                [[ -z "$merge_out" ]] || printf '%s\n' "$merge_out"
                exit 0
            fi
            # gh exits non-zero when the merge landed but it could not delete the
            # local branch, which a delegate's worktree holding it causes. The
            # pull request is the truth: a merged one is success, so chaining
            # `pr-merge.sh N && next-step` does not skip the next step.
            if grep -q 'used by worktree\|failed to delete local branch' <<<"$merge_out"; then
                state="$(gh pr view "$pr" --json state --jq .state 2>/dev/null || true)"
                if [[ "$state" == "MERGED" ]]; then
                    printf 'PR #%s merged; the local branch was kept because a worktree holds it:\n%s\n' "$pr" "$merge_out"
                    exit 0
                fi
            fi
            printf '%s\n' "$merge_out" >&2
            exit 1
        fi
    fi
    sleep 20
done
echo "PR #$pr: checks still pending after 80 minutes" >&2
exit 1
