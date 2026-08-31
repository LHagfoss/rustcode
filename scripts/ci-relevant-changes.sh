#!/usr/bin/env bash
# Return success when stdin contains a path that should run the code CI jobs.
set -euo pipefail

# Keep this list broad enough to cover both the root package and extracted
# workspace crates. Documentation-only changes should not start the suite.
grep -Eq '^(Cargo\.toml|Cargo\.lock|build\.rs$|crates/|src/|tests/|benches/|scripts/|install\.sh$|install\.ps1$|rust-toolchain[^/]*$|\.cargo/|\.github/workflows/)'
