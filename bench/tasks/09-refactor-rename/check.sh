#!/usr/bin/env bash
# Fail if any `Widget` identifier remains, else require tests to pass.
if grep -rq 'Widget' src; then exit 1; fi
cargo test 2>/dev/null
