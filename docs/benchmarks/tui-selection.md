# Transcript selection frame benchmark

The interactive frame target is p95 <= 16.7 ms (60 Hz). Timings are diagnostic,
not CI assertions: scheduler contention and terminal/backend differences make
absolute timing assertions unreliable.

Run from the repository root, with other builds and tests stopped:

```sh
cargo test -p rustcode-tui --release bench_deep_selection_scroll_many_history_entries -- --ignored --nocapture --test-threads=1
cargo test -p rustcode-tui --release bench_selection_frame_matrix -- --ignored --nocapture --test-threads=1
```

The existing deep benchmark starts 5,000 rows back in 50,000 entries, advances
30 rows per sample, extends a mouse selection, captures a render snapshot,
renders the whole TUI through `InlineTerminal`, and reports 100 warm samples
after one cold selection frame. It creates a `TestBackend` terminal per frame
and clones the resulting buffer. Retain it for comparison with earlier runs;
its allocation count includes those harness costs.

The frame matrix retains the terminal across frames, including buffer diff
and backend draw, without cloning the output. Each independent fixture uses
5,000 or 50,000 entries and a 132x48 or 240x48 viewport. Every twentieth entry
is a completed command result in high verbosity. The primary fixture pairs
results with native call IDs; the `_legacy` fixture deliberately omits call
envelopes/IDs to exercise orphaned legacy results. Text includes Japanese,
combining accents, and an emoji grapheme. Selection starts 2,000 rows back;
each sample queues a three-row wheel movement, applies pointer extension,
captures the snapshot, and paints. The canonical streamed response changes
before each sample while selection retains its displayed revision. Report
one cold sample and 200 warm samples with nearest-rank p50/p95/p99 latency,
allocation calls, and requested bytes. Requested bytes measure allocation
traffic, not retained or peak memory. Fixture creation and final clipboard
extraction are outside the timed interval.

Both harnesses exercise the production selection, snapshot, wrap/cache,
highlight, rendering, and terminal buffer paths. They do not measure OS input
delivery, runtime event coalescing, crossterm writes, terminal compositor paint,
or human interaction. Passing their frame target is not evidence that the
original live marking/scrolling scenario has passed. Live confirmation must
record terminal, viewport, history fixture, streaming state, and input-to-paint
latency separately.

The inspected local Codex transcript view shares snapshot cell ownership,
uses content anchors and pins displayed layouts. RustCode already shares its
render snapshot and incrementally extends a selected-history projection;
visible-row slicing uses cumulative row indices. These existing mechanisms
should be measured before changing ownership or copying another architecture.

The legacy fixture exposed repeated scans and tolerant parsing of ordinary
assistant prose while correlating each orphaned result. An immutable render
snapshot now lazily indexes possible assistant call positions once. Selection
retains that snapshot, so newly exposed results reuse its index rather than
rescanning ordinary prose. Only indices are stored, without another message
copy or history owner. The index uses O(candidate count) memory and one initial
O(history content size) scan. It retains the previous fallback and ordering
for all plausible call encodings, including tolerant envelopes. Candidate-rich
legacy histories still require backward matching and result counting;
universal constant-time correlation is not claimed. Native paired results find
their nearby call directly.

## Recorded baseline (2026-10-04)

Machine: Apple M5 Pro, arm64, macOS 27.0.1 (26A434), Homebrew rustc 1.98.0.
The repository release profile uses optimization and fat LTO. A concurrent
build was present during this first recording; rerun in an idle environment
before comparing small changes.

| Fixture | Cold ms | Warm p50 ms | Warm p95 ms | Warm p99 ms |
| --- | ---: | ---: | ---: | ---: |
| Existing deep 50,000-entry wheel/drag harness, 132x48 | 7.128 | 1.306 | 1.642 | 1.720 |

Warm allocation-call p50/p95/p99 was 7,254/7,255/7,256; requested bytes was
2,745,193/2,745,833/2,746,473. This baseline is already below the diagnostic
16.7 ms p95 target. It does not support a further speculative rendering
optimization or a claim of before/after production improvement.

Before the parser fast rejection, the orphan-result matrix measured:

| Entries | Viewport | Cold ms | Warm p50 ms | Warm p95 ms | Warm p99 ms |
| ---: | --- | ---: | ---: | ---: | ---: |
| 5,000 | 132x48 | 9.822 | 0.618 | 0.681 | 0.831 |
| 5,000 | 240x48 | 10.593 | 0.836 | 0.944 | 1.042 |
| 50,000 | 132x48 | 75.635 | 0.660 | 0.785 | 3.860 |
| 50,000 | 240x48 | 87.024 | 0.882 | 1.151 | 4.453 |

These cold frames include building the pinned projection through the initial
2,000-row offset; they reveal a first-interaction stall hidden by warm p95.

## Final snapshot-index measurement

Same release fixture, 200 warm frames (2026-10-04):

| Fixture | Cold ms | Warm p95 ms | Warm p99 ms |
| --- | ---: | ---: | ---: |
| Native paired, 50,000, 132x48 | 3.688 | 0.731 | 0.763 |
| Native paired, 50,000, 240x48 | 4.121 | 0.999 | 1.031 |
| Legacy orphan, 50,000, 132x48 | 6.608 | 0.938 | 1.159 |
| Legacy orphan, 50,000, 240x48 | 7.060 | 1.075 | 1.265 |

The legacy cold-frame improvement targets correlation work; warm frames were
already within budget. Live OS input-to-paint confirmation remains outstanding.
