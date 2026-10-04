# Performance and agent-thread overhaul

Tracks #1707 and every linked issue (#1699–#1706, #1605, #1607).

The implementation extends existing controller/supervisor, tool scheduler,
read replay, compaction, compiler and transcript services. It preserves native
provider call/result pairing, announcement order, permission barriers, isolated
child histories and cancellation cleanup. Codex is architectural reference,
not a code source.

Implementation sequence:
1. Structured request/turn telemetry, diagnostics and reproducible baseline.
2. Bounded concurrent inspection groups with ordered results and serial barriers.
3. Workspace generations, incremental symbols and cached environment snapshots.
4. Evidence validity and context lifecycle built on mutation invalidation.
5. Successful verification reuse with conservative identity checks.
6. Inspectable agent threads, inheritance, mailbox/lifecycle and persistence.
7. Bounded skill routing and measured selection rendering.
8. Documentation reflecting delivered behavior and measurement limitations.

Each architectural change includes focused tests. Required final gates are
cargo check --tests and cargo test, plus terminal frontend tests. Benchmarks use
fresh fixtures/sessions, distinguish synthetic workloads from provider runs,
and report wall time and context size without inferring task success.
