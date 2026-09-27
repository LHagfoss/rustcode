#!/usr/bin/env bash
# Smoke-test the frontend-seam guard without touching the real tree.
set -uo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd -P)"
guard="$script_dir/check-frontend-seam.sh"
tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/rustcode-frontend-seam-smoke.XXXXXX")"
failures=0

cleanup() {
    rm -rf -- "$tmp_root"
}
trap cleanup EXIT INT TERM

pass() {
    echo "ok - $1"
}

fail() {
    echo "not ok - $1" >&2
    failures=$((failures + 1))
}

if bash -n "$guard"; then
    pass "seam guard has valid shell syntax"
else
    fail "seam guard has valid shell syntax"
fi

# A tree that only touches the seam and its own modules passes.
mkdir -p "$tmp_root/clean/ui"
cat >"$tmp_root/clean/allow.txt" <<'EOF'
crate::ui
crate::controller
EOF
cat >"$tmp_root/clean/ui/render.rs" <<'EOF'
use crate::ui::theme;
use rustcode_real::controller::ControllerHandle;
fn draw(handle: &crate::controller::ControllerHandle) {
    let _ = crate::ui::theme::current();
    let _ = handle;
}
EOF
if "$guard" --allowlist "$tmp_root/clean/allow.txt" "$tmp_root/clean/ui" >/dev/null; then
    pass "seam-only tree passes"
else
    fail "seam-only tree passes"
fi

# A direct reach into core fails and names the offending path.
mkdir -p "$tmp_root/dirty/ui"
cp "$tmp_root/clean/allow.txt" "$tmp_root/dirty/allow.txt"
cat >"$tmp_root/dirty/ui/render.rs" <<'EOF'
fn draw(state: &crate::app::state::AppState, turns: crate::network::ui_adapter::AgentUiEvent) {
    let _ = state;
    let _ = turns;
}
EOF
output="$("$guard" --allowlist "$tmp_root/dirty/allow.txt" "$tmp_root/dirty/ui" 2>&1)" && {
    fail "core reach fails the guard (guard passed)"
    output=""
}
if [[ "$output" == *"crate::app::state"* && "$output" == *"crate::network::ui_adapter"* ]]; then
    pass "core reach fails naming both paths"
else
    fail "core reach fails naming both paths (got: $output)"
fi

# Brace imports collapse to their one-segment key.
mkdir -p "$tmp_root/brace/ui"
cat >"$tmp_root/brace/allow.txt" <<'EOF'
crate::tools
EOF
cat >"$tmp_root/brace/ui/tools.rs" <<'EOF'
use crate::tools::{parse_tool_call, resolve_tool_calls};
fn f() {
    let _ = parse_tool_call("");
    let _ = resolve_tool_calls(&[]);
}
EOF
if "$guard" --allowlist "$tmp_root/brace/allow.txt" "$tmp_root/brace/ui" >/dev/null; then
    pass "brace imports collapse to one-segment key"
else
    fail "brace imports collapse to one-segment key"
fi

# The guard passes on the real tree with the shipped allow-list.
if "$guard" >/dev/null; then
    pass "real tree passes with shipped allow-list"
else
    fail "real tree passes with shipped allow-list"
fi

if ((failures > 0)); then
    echo "$failures seam-guard smoke test(s) failed" >&2
    exit 1
fi
echo "all frontend-seam smoke tests passed"
