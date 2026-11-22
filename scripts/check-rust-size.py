#!/usr/bin/env python3
"""A size budget for Rust source files, as apps/desktop/src/fileSize.test.ts is for TypeScript.

A file past BUDGET lines is hard to own: parallel branches collide in it and
every edit recompiles more than it needs. New code goes in a module of its own.
The files already past the budget are listed in scripts/rust-size-baseline.txt
with a cap, their size when the budget came in rounded up to the next hundred.
A cap may only come down: lower it when a split lands, never raise it. A cap
more than SLACK lines above its file must come down.

Tests (`tests.rs`, `*_tests.rs`, `tests/`) are not counted.

`--write` rewrites the baseline from today's sizes (only for lowering caps).
"""

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
BASELINE = ROOT / "scripts" / "rust-size-baseline.txt"
BUDGET = 2000
SLACK = 100
ROOTS = ["rustcode", "harness", "src", "apps/desktop/src-tauri/src"]
SKIP_DIRS = {"target", "node_modules", "tests", ".git"}


def is_test(path: pathlib.Path) -> bool:
    return path.name == "tests.rs" or path.name.endswith("_tests.rs")


def sizes() -> dict[str, int]:
    found = {}
    for root in ROOTS:
        base = ROOT / root
        if not base.exists():
            continue
        for path in base.rglob("*.rs"):
            rel = path.relative_to(ROOT)
            if SKIP_DIRS & set(rel.parts) or is_test(path):
                continue
            with path.open("rb") as handle:
                found[rel.as_posix()] = sum(1 for _ in handle)
    return found


def read_baseline() -> dict[str, int]:
    caps = {}
    for line in BASELINE.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        cap, file = line.split(None, 1)
        caps[file] = int(cap)
    return caps


def write_baseline(found: dict[str, int]) -> None:
    rows = sorted(
        ((-(-size // 100) * 100, file) for file, size in found.items() if size > BUDGET),
        key=lambda row: row[1],
    )
    header = (
        f"# Rust files over {BUDGET} lines and the cap each may not pass.\n"
        "# Lower a cap when its file is split; never raise one. See scripts/check-rust-size.py.\n"
    )
    BASELINE.write_text(header + "".join(f"{cap} {file}\n" for cap, file in rows))


def main() -> int:
    found = sizes()
    if "--write" in sys.argv:
        write_baseline(found)
        return 0
    caps = read_baseline()
    problems = []
    for file, size in sorted(found.items()):
        cap = caps.get(file)
        if cap is None and size > BUDGET:
            problems.append(f"{file}: {size} lines, past the {BUDGET}-line budget; split it")
        elif cap is not None and size > cap:
            problems.append(f"{file}: {size} lines, past its cap of {cap}; split it instead")
    for file, cap in sorted(caps.items()):
        size = found.get(file, 0)
        if size < cap - SLACK:
            problems.append(
                f"{file}: {size or 'gone'} lines, cap {cap}; lower the cap in {BASELINE.relative_to(ROOT)}"
            )
    for problem in problems:
        print(problem)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
