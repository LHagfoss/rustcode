#!/usr/bin/env bash
# Return success when stdin contains a path that should run the code CI jobs.
set -euo pipefail

# Keep build inputs and Rust code covered across the root package and workspace
# crates. Documentation-only files, including crate READMEs, should skip CI.
grep -Eq '^(Cargo\.toml|Cargo\.lock|build\.rs$|src/|tests/|benches/|examples/|scripts/|install\.sh$|install\.ps1$|rust-toolchain[^/]*$|\.cargo/|\.github/workflows/|crates/.*/(Cargo\.toml|build\.rs|src/|tests/|benches/|examples/))'
