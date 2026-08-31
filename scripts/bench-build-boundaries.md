# Build-boundary benchmark

The script scripts/bench-build-boundaries.sh measures whether workspace
decomposition is reducing the Cargo rebuild surface. It emits TSV with one row
per check:

    package  scenario  status  elapsed_ms  command  log

The default run measures a clean and warm workspace check, a clean and warm
check for every workspace package, and an incremental focused-edit check for
each package after temporarily touching its src/lib.rs (or src/main.rs).
Focused edits reuse the warm workspace target, while each package's clean and
warm rows use a separate package target so their timings are independently
interpretable.

The focused-edit source timestamp is restored on success, failure, interrupt,
or termination. No source contents are changed.

Timing prefers Perl's Time::HiRes millisecond clock, which works with the Bash
3.2 shipped by macOS and with Linux. If Perl is unavailable, the script uses a
millisecond-capable date implementation, then Python 3 or Ruby. Its final
POSIX fallback multiplies date +%s by 1,000; that fallback is valid but only
whole-second-precise, so install Perl or use another high-resolution provider
for meaningful warm-build comparisons.

In the safe default mode, temporary logs are deleted at exit and the TSV marks
their log column accordingly. Use --keep-target or --target-dir when logs must
remain available.

## Safe default

By default the script creates isolated temporary CARGO_TARGET_DIR directories
and never invokes cargo clean. A clean measurement starts with an empty
temporary target, so the developer's normal target/ directory is unaffected.
Temporary build output and logs are deleted when the script exits.

    scripts/bench-build-boundaries.sh \
      --output /tmp/rustcode-build-boundaries.tsv

Select one or more packages with repeated --package flags:

    scripts/bench-build-boundaries.sh \
      --package rustcode-core \
      --output /tmp/rustcode-core-boundaries.tsv

Use --keep-target to inspect compiler logs after a run. --all-targets includes
test and example targets and is substantially slower.

## Explicit clean opt-in

An actual cargo clean is not needed for the normal clean measurement. If a
disposable target under /tmp must be cleaned explicitly, pass both options:

    scripts/bench-build-boundaries.sh \
      --target-dir /tmp/rustcode-build-boundaries-target \
      --allow-cargo-clean

Before allowing a clean, the script creates and resolves the target with
`pwd -P`. It rejects the temporary root itself, paths that escape /tmp or
/private/tmp through .., symlink escapes, and the repository or its shared
`target/` directory. Never point it at a shared or user-owned build
directory.

The safety guards can be smoke-tested without a build:

    scripts/test-bench-build-boundaries.sh

The smoke test also exercises the selected millisecond clock and requires a
positive subsecond interval.

## Comparing runs

Focused-edit rows are the primary decomposition signal. Compare their elapsed
time and Cargo log output against the workspace row on the same OS and
toolchain. Record the commit, host, Rust version, and whether sccache was active
alongside the TSV because each materially affects timings.

This is intentionally a local measurement harness. Change-aware CI selection
should be added only after package boundaries and ownership are stable.
