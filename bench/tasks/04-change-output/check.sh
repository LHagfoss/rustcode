#!/usr/bin/env bash
# Pass only when the program prints exactly the required line.
out="$(cargo run -q 2>/dev/null)"
[ "$out" = "Goodbye, world!" ]
