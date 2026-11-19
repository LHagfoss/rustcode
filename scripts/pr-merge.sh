#!/usr/bin/env bash
# Merge a PR once its checks pass: `scripts/pr-merge.sh <number>`.
#
# Waits for every check except the advisory ones, which take longest and never
# block a merge: the portability jobs (Windows and macOS compile coverage,
# `continue-on-error` in ci.yml). Late-registering, path-filtered jobs are
# caught by waiting until a pass finds nothing pending twice in a row.
#
# A check GitHub still lists as pending but that has a completion time counts
# as finished: GitHub sometimes leaves a finished job's status at
# `in_progress`, and the script then waited out its whole limit (#4049).
# While waiting, it prints the checks it waits on whenever that list changes.
set -euo pipefail

pr="${1:?usage: scripts/pr-merge.sh <pr-number>}"
# Desktop shell (Linux/Windows) checks are required; only the root workspace's
# explicitly named advisory jobs are excluded.
advisory='^(macOS|Windows) portability coverage$'
# One `bucket<TAB>name` line per check. A pending check with a real completion
# time (not GitHub's 0001-01-01 placeholder) is reported as passed.
query='.[] | select(.name | test("'"$advisory"'") | not)
  | (.completedAt // "") as $done
  | [ (if .bucket == "pending" and $done != "" and ($done | startswith("0001") | not)
       then "pass" else .bucket end), .name ] | @tsv'
quiet=0
waiting=""
for _ in $(seq 1 240); do
    checks="$(gh pr checks "$pr" --json name,bucket,completedAt --jq "$query" 2>/dev/null || true)"
    if [[ -z "$checks" ]]; then
        quiet=0
    elif grep -q '^fail'$'\t' <<<"$checks"; then
        echo "PR #$pr has a failing check:" >&2
        grep '^fail'$'\t' <<<"$checks" | cut -f2- >&2
        exit 1
    elif grep -q '^pending'$'\t' <<<"$checks"; then
        quiet=0
        now="$(grep '^pending'$'\t' <<<"$checks" | cut -f2- | paste -sd ',' - | sed 's/,/, /g')"
        if [[ "$now" != "$waiting" ]]; then
            echo "PR #$pr: waiting on $now"
            waiting="$now"
        fi
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
