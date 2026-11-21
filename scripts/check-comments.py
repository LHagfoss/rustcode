#!/usr/bin/env python3
"""Comment ratchet: no new code comments, and the baseline only shrinks.

Usage:
  python3 scripts/check-comments.py            check against the baseline
  python3 scripts/check-comments.py --update   rewrite the baseline
  python3 scripts/check-comments.py --list PATH...  print each counted comment
  python3 scripts/check-comments.py --self-test     check the lexer

Counted: `//` and `/* */` comments in tracked .rs, .ts, .tsx, .js and .mjs
files, one per physical line. Exempt: doc comments (`///`, `//!`, `/**`),
`// SAFETY:` runs, and tool directives (biome-ignore, @ts-expect-error, ...).
See docs/adr/0001-comment-ratchet.md.
"""

import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE = os.path.join(ROOT, "scripts", "comment-baseline.txt")
EXTENSIONS = (".rs", ".ts", ".tsx", ".js", ".mjs")
DIRECTIVE = re.compile(
    r"^\s*(biome-ignore|@ts-expect-error|@ts-ignore|@ts-nocheck|eslint-|prettier-ignore"
    r"|@vitest-environment|@jsx|#region|#endregion|rustfmt::|language=)"
)
SAFETY = re.compile(r"^\s*SAFETY:")
REGEX_PRECEDERS = set("(,=:[!&|?{};+-*%<>~^")
REGEX_KEYWORDS = ("return", "typeof", "case", "in", "of", "void", "throw", "yield", "await")


def scan(text, rust):
    """Yield (line, body, is_doc, starts_line) for every comment in `text`."""
    i, n, line = 0, len(text), 1
    last_sig = ""
    last_word = ""
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if c == "\n":
            line += 1
            i += 1
            continue
        if c == "/" and nxt == "/":
            end = text.find("\n", i)
            end = n if end == -1 else end
            body = text[i + 2 : end]
            is_doc = body.startswith("/") and not body.startswith("//") or body.startswith("!")
            starts = text[text.rfind("\n", 0, i) + 1 : i].strip() == ""
            yield line, body, is_doc, starts
            i = end
            continue
        if c == "/" and nxt == "*":
            end = text.find("*/", i + 2)
            end = n if end == -1 else end + 2
            body = text[i + 2 : end - 2]
            is_doc = body.startswith("*") and not body.startswith("**") or body.startswith("!")
            starts = text[text.rfind("\n", 0, i) + 1 : i].strip() == ""
            for k, part in enumerate(body.split("\n")):
                yield line + k, part, is_doc, starts or k > 0
            line += body.count("\n")
            i = end
            continue
        if rust and c == "r" and re.match(r'r#*"', text[i:]) and not (i and (text[i - 1].isalnum() or text[i - 1] == "_")):
            m = re.match(r'r(#*)"', text[i:])
            close = '"' + m.group(1)
            end = text.find(close, i + len(m.group(0)))
            end = n if end == -1 else end + len(close)
            line += text.count("\n", i, end)
            i = end
            last_sig = '"'
            continue
        if c == '"' or (not rust and c in "'`") or (rust and c == "'" and re.match(r"'(\\.[^']*|[^\\'])'", text[i:])):
            quote = c
            j = i + 1
            depth = 0
            while j < n:
                ch = text[j]
                if ch == "\\":
                    j += 2
                    continue
                if quote == "`" and ch == "$" and j + 1 < n and text[j + 1] == "{":
                    depth += 1
                    j += 2
                    continue
                if quote == "`" and depth and ch == "}":
                    depth -= 1
                elif ch == quote and not depth:
                    break
                elif ch == "\n" and quote == "'" and not rust:
                    break
                j += 1
            line += text.count("\n", i, j)
            i = j + 1
            last_sig = '"'
            continue
        if not rust and c == "/" and (last_sig in REGEX_PRECEDERS or last_sig == "" or last_word in REGEX_KEYWORDS):
            j = i + 1
            in_class = False
            while j < n and text[j] != "\n":
                ch = text[j]
                if ch == "\\":
                    j += 2
                    continue
                if ch == "[":
                    in_class = True
                elif ch == "]":
                    in_class = False
                elif ch == "/" and not in_class:
                    break
                j += 1
            if j < n and text[j] == "/":
                i = j + 1
                last_sig = '"'
                continue
        if not c.isspace():
            if c.isalnum() or c == "_":
                m = re.match(r"[A-Za-z0-9_$]+", text[i:])
                last_word = m.group(0)
                last_sig = "a"
                i += len(last_word)
                continue
            last_sig = c
            last_word = ""
        i += 1


def counted(path, text=None):
    """Line numbers of the comments the ratchet counts in `path`."""
    if text is None:
        try:
            with open(os.path.join(ROOT, path), encoding="utf-8") as f:
                text = f.read()
        except (UnicodeDecodeError, FileNotFoundError):
            return []
    lines = []
    safety_run = False
    prev_line = 0
    for line, body, is_doc, starts in scan(text, path.endswith(".rs")):
        if line == 1 and body.startswith("!/"):
            continue
        if not (starts and line == prev_line + 1):
            safety_run = False
        prev_line = line
        if SAFETY.match(body):
            safety_run = True
        if is_doc or safety_run or DIRECTIVE.match(body):
            continue
        if not body.strip():
            continue
        lines.append(line)
    return sorted(set(lines))


def tracked():
    out = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, check=True, capture_output=True).stdout
    return sorted(p for p in out.decode().split("\0") if p.endswith(EXTENSIONS))


def load_baseline():
    base = {}
    if os.path.exists(BASELINE):
        with open(BASELINE, encoding="utf-8") as f:
            for raw in f:
                if raw.strip() and not raw.startswith("#"):
                    count, path = raw.rstrip("\n").split(" ", 1)
                    base[path] = int(count)
    return base


SELF_TEST = [
    ("a.rs", 'let url = "https://x"; // why\n', [1]),
    ("a.rs", "/// doc\n//! inner\nfn f() {}\n", []),
    ("a.rs", "// SAFETY: the pointer is live\n// and aligned\nunsafe { f() }\n// later\n", [4]),
    ("a.rs", 'let s = r#"// not a comment"#;\nlet c = \'"\';\nfn f<\'a>() {} // yes\n', [3]),
    ("a.ts", "const re = /\\/\\//g;\nconst t = `a ${b} // c`;\n/* one\ntwo */\n", [3, 4]),
    ("a.ts", "/** doc */\n// biome-ignore lint: reason\nx = a / b; // tail\n", [3]),
]


def self_test():
    failed = 0
    for path, text, want in SELF_TEST:
        got = counted(path, text)
        if got != want:
            failed += 1
            print(f"check-comments self-test: {text!r} counted {got}, want {want}", file=sys.stderr)
    print(f"check-comments self-test: {len(SELF_TEST) - failed}/{len(SELF_TEST)} passed")
    return 1 if failed else 0


def main(argv):
    if argv[:1] == ["--self-test"]:
        return self_test()
    if argv[:1] == ["--list"]:
        wanted = [os.path.relpath(os.path.abspath(a), ROOT) for a in argv[1:]]
        for path in tracked():
            if wanted and not any(path == w or path.startswith(w.rstrip("/") + "/") for w in wanted):
                continue
            src = open(os.path.join(ROOT, path), encoding="utf-8").read().split("\n")
            for line in counted(path):
                print(f"{path}:{line}: {src[line - 1].strip()}")
        return 0
    current = {}
    for path in tracked():
        count = len(counted(path))
        if count:
            current[path] = count
    if argv[:1] == ["--update"]:
        with open(BASELINE, "w", encoding="utf-8") as f:
            f.write("# Comment ratchet baseline: <count> <path>. Regenerate with\n")
            f.write("# `python3 scripts/check-comments.py --update`; counts may only fall.\n")
            for path, count in sorted(current.items()):
                f.write(f"{count} {path}\n")
        print(f"check-comments: baseline holds {sum(current.values())} comment lines in {len(current)} files")
        return 0
    base = load_baseline()
    grew, shrank = [], []
    for path in sorted(set(base) | set(current)):
        now, was = current.get(path, 0), base.get(path, 0)
        if now > was:
            grew.append((path, was, now))
        elif now < was:
            shrank.append((path, was, now))
    for path, was, now in grew:
        print(f"check-comments: {path} has {now} comment lines, baseline allows {was}", file=sys.stderr)
    if grew:
        print(
            "check-comments: new comments are banned. Put the reason in the code (a name, a type, a test)\n"
            "or in an ADR under docs/adr/; list them with `python3 scripts/check-comments.py --list <file>`.",
            file=sys.stderr,
        )
        return 1
    if shrank:
        for path, was, now in shrank:
            print(f"check-comments: {path} fell from {was} to {now}", file=sys.stderr)
        print(
            "check-comments: comments were removed; tighten the ratchet with\n"
            "`python3 scripts/check-comments.py --update` and commit scripts/comment-baseline.txt.",
            file=sys.stderr,
        )
        return 1
    print(f"check-comments: {sum(current.values())} comment lines, none above the baseline")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
