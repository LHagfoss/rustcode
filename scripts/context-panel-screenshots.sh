#!/usr/bin/env bash
# Render `/context` screenshots for every shipped theme.
#
# The panel is a ratatui widget and the repo has no PTY harness, so the images
# come from the `TestBackend` buffer: the cells carry truecolor `Color::Rgb`
# values, `context_panel_screenshots_are_written_when_requested` writes them out
# as HTML, and headless Chrome rasterises that to a PNG.
#
#   scripts/context-panel-screenshots.sh [output-dir]
#
# Defaults to `images/`. Set CHROME if Google Chrome is installed elsewhere.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/images}"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ ! -x "$CHROME" ]; then
  echo "chrome not found at $CHROME; set CHROME=/path/to/chrome" >&2
  exit 1
fi

RUSTCODE_CONTEXT_PANEL_SHOTS="$WORK" \
  cargo test --manifest-path "$ROOT/Cargo.toml" -p rustcode-tui \
  context_panel_screenshots_are_written_when_requested

mkdir -p "$OUT"
for html in "$WORK"/context-panel-*.html; do
  theme="$(basename "$html" .html)"
  "$CHROME" --headless \
    --disable-gpu \
    --hide-scrollbars \
    --force-device-scale-factor=2 \
    --window-size=1260,420 \
    --screenshot="$OUT/${theme}.png" \
    "file://$html" >/dev/null 2>&1
  echo "$OUT/${theme}.png"
done