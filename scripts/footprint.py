#!/usr/bin/env python3
"""Measure `rc` startup and memory, and the macOS desktop app's process tree.

Run by .github/workflows/footprint.yml; docs/release-footprint.md has the
results. Prints a Markdown report on stdout.

    footprint.py cli <rc> [--samples N] [--cold N]
    footprint.py desktop <rustcode-desktop> <rc> [--seconds N]

Every process runs with an isolated, empty home, XDG directories and
`RUSTCODE_CONFIG_DIR`. Wall time is monotonic from just before spawn to exit;
peak RSS is the child's `wait4` `ru_maxrss`. A cold sample drops the page
cache first (`sudo purge` on macOS, `drop_caches` on Linux).
"""

import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time

MACOS = sys.platform == "darwin"


def isolated_env(root):
    env = {k: v for k, v in os.environ.items() if not k.startswith(("RUSTCODE_", "XDG_"))}
    for name in ("home", "config", "data", "cache", "state", "rustcode"):
        os.makedirs(os.path.join(root, name), exist_ok=True)
    env.update(
        HOME=os.path.join(root, "home"),
        XDG_CONFIG_HOME=os.path.join(root, "config"),
        XDG_DATA_HOME=os.path.join(root, "data"),
        XDG_CACHE_HOME=os.path.join(root, "cache"),
        XDG_STATE_HOME=os.path.join(root, "state"),
        RUSTCODE_CONFIG_DIR=os.path.join(root, "rustcode"),
        NO_COLOR="1",
    )
    return env


def drop_caches():
    if MACOS:
        subprocess.run(["sudo", "purge"], check=True)
    else:
        subprocess.run(["sudo", "sh", "-c", "sync; echo 3 > /proc/sys/vm/drop_caches"], check=True)


def run_once(argv, env):
    """Wall ms and peak RSS KiB of one fresh process."""
    start = time.monotonic()
    proc = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, status, usage = os.wait4(proc.pid, 0)
    wall = (time.monotonic() - start) * 1000
    proc.returncode = os.waitstatus_to_exitcode(status)
    if proc.returncode != 0:
        raise SystemExit(f"{' '.join(argv)} exited {proc.returncode}")
    rss_kib = usage.ru_maxrss / 1024 if MACOS else usage.ru_maxrss
    return wall, rss_kib


def summary(values, fmt):
    if not values:
        return "–"
    return f"{fmt(statistics.median(values))} ({fmt(min(values))}–{fmt(max(values))})"


def ms(v):
    return f"{v:.3f}"


def kib(v):
    return f"{v:,.0f}"


def ps_table():
    """pid -> (ppid, rss KiB, command) for every process."""
    out = subprocess.run(["ps", "-axo", "pid=,ppid=,rss=,comm="], capture_output=True, text=True, check=True).stdout
    table = {}
    for line in out.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4:
            table[int(parts[0])] = (int(parts[1]), int(parts[2]), parts[3])
    return table


def descendants(table, root):
    found, frontier = set(), {root}
    while frontier:
        frontier = {pid for pid, (ppid, _, _) in table.items() if ppid in frontier} - found
        found |= frontier
    return found


def header(binary):
    print(f"- Platform: `{platform.platform()}` ({platform.machine()}, {os.cpu_count()} CPUs)")
    print(f"- Binary: `{os.path.basename(binary)}`, {os.path.getsize(binary):,} bytes")
    print()


def cli(rc, samples, cold):
    header(rc)
    with tempfile.TemporaryDirectory() as root:
        env = isolated_env(root)
        commands = [["--version"], ["--help"], ["threads", "list"]]
        print(f"One warm-up, then {samples} warm samples and {cold} cold samples per command.")
        print()
        print("| Command | Warm wall ms | Warm peak RSS KiB | Cold wall ms | Cold peak RSS KiB |")
        print("| --- | ---: | ---: | ---: | ---: |")
        for args in commands:
            argv = [rc, *args]
            run_once(argv, env)
            warm = [run_once(argv, env) for _ in range(samples)]
            cold_runs = []
            for _ in range(cold):
                drop_caches()
                cold_runs.append(run_once(argv, env))
            print(
                f"| `rc {' '.join(args)}` | {summary([w for w, _ in warm], ms)} | {summary([r for _, r in warm], kib)}"
                f" | {summary([w for w, _ in cold_runs], ms)} | {summary([r for _, r in cold_runs], kib)} |"
            )
        print()

        # The long-lived process the desktop app runs: `rc --acp`, idle with its
        # stdin open, sampled with ps.
        proc = subprocess.Popen([rc, "--acp"], env=env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        print("Idle `rc --acp` (what the desktop app spawns), resident set from `ps`:")
        print()
        print("| Seconds after spawn | RSS KiB |")
        print("| ---: | ---: |")
        start = time.monotonic()
        for mark in (1, 5, 15, 30):
            time.sleep(max(0, start + mark - time.monotonic()))
            if proc.poll() is not None:
                print(f"| {mark} | exited {proc.returncode} |")
                break
            print(f"| {mark} | {kib(ps_table().get(proc.pid, (0, 0, ''))[1])} |")
        proc.kill()
        proc.wait()
        print()


def desktop(app, rc, seconds):
    header(app)
    with tempfile.TemporaryDirectory() as root:
        env = isolated_env(root)
        env["RUSTCODE_ENGINE"] = rc
        before = set(ps_table())
        start = time.monotonic()
        proc = subprocess.Popen([app], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        first = {}
        rows = []
        marks = [1, 2, 5, 10, seconds]
        while time.monotonic() - start < seconds + 0.5:
            table = ps_table()
            elapsed = time.monotonic() - start
            tree = descendants(table, proc.pid) | {proc.pid}
            # WebKit's helpers are XPC services launchd starts, not children of
            # the app; on an otherwise idle runner the new ones are the app's.
            webkit = {pid for pid in set(table) - before if "com.apple.WebKit" in table[pid][2]}
            engine = {pid for pid in tree if os.path.basename(table.get(pid, (0, 0, ""))[2]) == "rc"}
            for name, pids in (("engine", engine), ("webkit", webkit)):
                if pids and name not in first:
                    first[name] = elapsed
            if marks and elapsed >= marks[0]:
                rss = lambda pids: sum(table[p][1] for p in pids if p in table)
                app_rss = table.get(proc.pid, (0, 0, ""))[1]
                rows.append((marks.pop(0), app_rss, rss(engine), rss(webkit), app_rss + rss(engine) + rss(webkit)))
            if proc.poll() is not None:
                raise SystemExit(f"the app exited {proc.returncode} after {elapsed:.1f} s")
            time.sleep(0.05)
        proc.terminate()
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()

    print("Launch with an empty configuration; ms from spawn to the first process of each kind:")
    print()
    for name, label in (("engine", "`rc --acp` engine spawned"), ("webkit", "first WebKit helper (webview created)")):
        print(f"- {label}: {first[name] * 1000:.0f} ms" if name in first else f"- {label}: not seen")
    print()
    print("| Seconds after launch | App KiB | Engine KiB | WebKit helpers KiB | Total KiB |")
    print("| ---: | ---: | ---: | ---: | ---: |")
    for mark, *values in rows:
        print(f"| {mark} | " + " | ".join(kib(v) for v in values) + " |")
    print()


def flag(args, name, default):
    return int(args[args.index(name) + 1]) if name in args else default


def main():
    args = sys.argv[1:]
    if args[:1] == ["cli"] and len(args) >= 2:
        cli(os.path.abspath(args[1]), flag(args, "--samples", 30), flag(args, "--cold", 5))
    elif args[:1] == ["desktop"] and len(args) >= 3:
        desktop(os.path.abspath(args[1]), os.path.abspath(args[2]), flag(args, "--seconds", 20))
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
