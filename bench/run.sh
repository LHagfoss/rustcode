#!/usr/bin/env bash
#
# Benchmark the rustcode harness across models on a suite of self-checking tasks.
#
# Usage:
#   bash bench/run.sh                         # default models
#   bash bench/run.sh gemini-3.6-flash gemini-3.5-flash-lite
#   REPEATS=5 TIMEOUT=300 bash bench/run.sh <model>...
#
# Each task lives in bench/tasks/<name>/ with:
#   setup/       initial (broken) crate — copied fresh per attempt
#   prompt.txt   the instruction handed to the agent
#   check.sh     exit 0 == task solved (run with cwd = the crate)
#
# Every (model, task) is run REPEATS times (default 3) because a single agent
# run can flake (a transient stream hiccup, one stray prose reply). Pass rate is
# reported as k/N so one flake doesn't read as a hard failure.
#
# For each attempt the runner copies setup/ to a temp dir, runs
#   rustcode -p "<prompt>" -m <model>
# there (auto-approving tool calls by piping `yes`), then runs check.sh and
# records pass/fail, wall-clock seconds, and how many agent rounds it took.
#
# NOTE: `-p` drives the headless raw_cli loop. It shares the tools, system
# prompt, and tool protocol with the interactive TUI, but not the TUI-only
# machinery (compaction, finish gate, parallel tool exec). It measures the core
# loop, not the full harness.

set -u

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/release/rustcode"
TIMEOUT="${TIMEOUT:-180}"
REPEATS="${REPEATS:-3}"

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
echo "model,task,attempt,pass,seconds,rounds" > "$RESULTS"

echo "Models: ${MODELS[*]}   |   repeats: $REPEATS   |   timeout: ${TIMEOUT}s"
printf '\n%-24s %-20s %-8s %-8s %s\n' "MODEL" "TASK" "PASS" "AVG-T" "AVG-ROUNDS"
printf '%.0s-' {1..72}; echo

for model in "${MODELS[@]}"; do
    for taskdir in "$BENCH_DIR"/tasks/*/; do
        task="$(basename "$taskdir")"
        prompt="$(cat "$taskdir/prompt.txt")"
        passes=0; tot_s=0; tot_r=0

        for attempt in $(seq 1 "$REPEATS"); do
            sandbox="$WORK/${model}__${task}__${attempt}"
            cp -R "$taskdir/setup" "$sandbox"

            start=$(date +%s)
            out="$(cd "$sandbox" && yes | timeout "$TIMEOUT" "$BIN" -p "$prompt" -m "$model" 2>&1)"
            end=$(date +%s)

            rounds=$(printf '%s' "$out" | grep -c '=== Round')
            if (cd "$sandbox" && bash "$taskdir/check.sh") >/dev/null 2>&1; then
                pass=1
            else
                pass=0
            fi

            passes=$((passes + pass))
            tot_s=$((tot_s + end - start))
            tot_r=$((tot_r + rounds))
            echo "$model,$task,$attempt,$pass,$((end - start)),$rounds" >> "$RESULTS"
        done

        printf '%-24s %-20s %-8s %-8s %s\n' \
            "$model" "$task" "$passes/$REPEATS" "$((tot_s / REPEATS))s" "$((tot_r / REPEATS))"
    done
done

echo
echo "=== Scorecard ==="
awk -F, 'NR>1 {
    p[$1] += $4; n[$1]++; t[$1] += $5; r[$1] += $6
}
END {
    for (m in p)
        printf "%-24s  pass %d/%d (%.0f%%)   avg %.0fs   avg %.1f rounds\n", \
            m, p[m], n[m], 100 * p[m] / n[m], t[m] / n[m], r[m] / n[m]
}' "$RESULTS"

cp "$RESULTS" "$BENCH_DIR/last-results.csv"
echo
echo "Raw results: $BENCH_DIR/last-results.csv"
