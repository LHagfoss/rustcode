# 0005: `rc threads` status rules and `rc usage` totals

Status: accepted, 2026-11-21. Code: `rustcode/cli/src/run.rs`.

## Thread status

- Settling is the way out of review. Un-settling is not, so a thread marked
  for review that the user has not looked at yet stays in review.
- A thread with a folder but no transcript yet (settled right after it was
  started, or whose history was lost) is still that thread: an exact id may
  name it, as `rc threads path` does. A prefix may not, since that would be
  guessing at a directory no list vouches for.

## Usage totals

- `rc usage` reads the same aggregation as the engine's `thread_usage` tool.
- A column no turn reported stays blank in the total, not `0`.
- A total is partial when some turns did not report its column. It is marked
  with `*`, as the app's usage page does, and the mark is explained once
  under the table.
