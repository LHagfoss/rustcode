# Compact Input Status Bar Design

## Goal

Make the terminal UI more compact while keeping the most useful live state visible.

## Design

- Remove the input-box shortcut hints for Tab autocomplete, Shift+Enter newline, slash commands, and Ctrl+O model selection.
- Replace them with a single `Ctrl+P commands` hint in the input-box bottom border.
- Keep the status bar's activity indicator and `Ready`/activity text on the left.
- Put `Auto-Confirm: ON/OFF`, `Context: <tokens> (<percent>%)`, and `Tps: <rate>` in the status bar, with the command-palette hint at the right.
- Show context and TPS values consistently, including before the first message (`Context: 0 (0%)`, `Tps: 0.0`). Preserve cached-token and quota details when available.
- Remove the blank layout row between the input box and status bar so the status bar sits directly below the input.

## Scope and implementation

The change is limited to `src/ui/mod.rs` and focused UI tests. Existing keyboard behavior is unchanged; only the displayed hints and vertical spacing change.

Tests will cover the formatting helpers/visible status content and the full Rust test/check gates will verify compilation and regressions.

