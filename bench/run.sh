#!/usr/bin/env bash
#
# Benchmark the rustcode harness across models on a suite of self-checking tasks.
#
# Usage:
#   bash bench/run.sh                         # default models
#   bash bench/run.sh gemini-3.6-flash gemini-3.5-flash-lite
#   TIMEOUT=300 bash bench/run.sh <model>...
#
# Each task lives in bench/tasks/<name>/ with:
#   setup/       initial (broken) crate — copied fresh per run
#   prompt.txt   the instruction handed to the agent
#   check.sh     exit 0 == task solved (run with cwd = the crate)
#
# For every (model, task) pair the runner copies setup/ to a temp dir, runs
#   rustcode -p "<prompt>" -m <model>
# there (auto-approving tool calls by piping `yes`), then runs check.sh and
# records pass/fail, wall-clock seconds, and how many agent rounds it took.
#
# NOTE: `-p` drives the headless raw_cli loop (one tool per round). It shares
# the tools, system prompt, and tool protocol with the interactive TUI, but not
# the TUI-only machinery (compaction, loop detection, finish gate, parallel
# tool exec). It measures the core loop, not the full harness.

set -u

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/release/rustcode"
TIMEOUT="${TIMEOUT:-180}"

if [ "$#" -gt 0 ]; then
    MODELS=("$@")
else
    MODELS=(gemini-3.6-flash gemini-3.5-flash-lite)
fi

echo "Building rustcode (release)…"
(cd "$REPO_ROOT" && cargo build --release -q) || {
    echo "cargo build failed — aborting."
    exit 1
}
[ -x "$BIN" ] || { echo "binary not found at $BIN"; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
RESULTS="$WORK/results.csv"
echo "model,task,pass,seconds,rounds" > "$RESULTS"

printf '\n%-24s %-20s %-6s %-7s %s\n' "MODEL" "TASK" "RESULT" "TIME" "ROUNDS"
printf '%.0s-' {1..70}; echo

for model in "${MODELS[@]}"; do
    for taskdir in "$BENCH_DIR"/tasks/*/; do
        task="$(basename "$taskdir")"
        sandbox="$WORK/${model}__${task}"
        cp -R "$taskdir/setup" "$sandbox"
        prompt="$(cat "$taskdir/prompt.txt")"

        start=$(date +%s)
        out="$(cd "$sandbox" && yes | timeout "$TIMEOUT" "$BIN" -p "$prompt" -m "$model" 2>&1)"
        end=$(date +%s)

        rounds=$(printf '%s' "$out" | grep -c '=== Round')
        if (cd "$sandbox" && bash "$taskdir/check.sh") >/dev/null 2>&1; then
            pass=1; verdict="PASS"
        else
            pass=0; verdict="FAIL"
        fi

        echo "$model,$task,$pass,$((end - start)),$rounds" >> "$RESULTS"
        printf '%-24s %-20s %-6s %-7s %s\n' "$model" "$task" "$verdict" "$((end - start))s" "$rounds"
    done
done

echo
echo "=== Scorecard ==="
awk -F, 'NR>1 {
    p[$1] += $3; n[$1]++; t[$1] += $4; r[$1] += $5
}
END {
    for (m in p)
        printf "%-24s  pass %d/%d   avg %.0fs   avg %.1f rounds\n", m, p[m], n[m], t[m]/n[m], r[m]/n[m]
}' "$RESULTS"

cp "$RESULTS" "$BENCH_DIR/last-results.csv"
echo
echo "Raw results: $BENCH_DIR/last-results.csv"
