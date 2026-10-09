#!/usr/bin/env bash
# Return success when stdin contains a path that should run the code CI jobs.
set -euo pipefail

# Keep build inputs and Rust code covered across every package in the workspace:
# the root package, the libraries and apps under rustcode/, and the JS shells
# under apps/ and packages/. Documentation-only files, including crate READMEs,
# should skip CI. The committed remote protocol contract is generated from the
# engine's wire types, so editing it must run the engine's drift check.
grep -Eq '^(docs/remote-protocol/v1/|Cargo\.toml|Cargo\.lock|build\.rs$|src/|tests/|benches/|examples/|scripts/|install\.sh$|install\.ps1$|rust-toolchain[^/]*$|\.cargo/|\.github/workflows/|(crates|rustcode)/.*/(Cargo\.toml|build\.rs|src/|tests/|benches/|examples/)|(apps|packages)/.*/(package\.json|src/|app/|tsconfig\.json))'
