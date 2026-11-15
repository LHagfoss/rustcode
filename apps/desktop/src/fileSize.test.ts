// A size budget for source files. A file past BUDGET lines is hard to own: parallel branches
// collide in it and every change re-checks whatever imports it. New code goes in a module of its
// own (see state/autoCompactRun.ts, state/remoteAccess.ts). The files already past the budget are
// listed with their size today, and may only shrink: lower a cap when a split lands, never raise it.
// A cap more than SLACK lines above its file must come down, so a small edit elsewhere in a big
// file does not touch this list but a split does.

import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const SRC = import.meta.dir;
const BUDGET = 1500;
const SLACK = 100;

/** Files over the budget, capped at their size when the budget came in, rounded up to the next hundred. */
const OVERSIZED: Record<string, number> = {
  "state/store.ts": 5600,
  "acp/mock.ts": 7400,
  "state/host.ts": 3100,
  "state/orchestration.ts": 3200,
  "ui/Sidebar.tsx": 2800,
  "ui/Composer.tsx": 2200,
  "ui/Settings.tsx": 2200,
  "state/machines.ts": 2100,
  "state/thread.ts": 1800,
  "ui/Transcript.tsx": 1600,
};

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
  });
}

function lines(path: string): number {
  return readFileSync(path, "utf8").split("\n").length - 1;
}

describe("file size budget", () => {
  const sizes = new Map(sources(SRC).map((path) => [relative(SRC, path), lines(path)]));

  test(`no new source file passes ${BUDGET} lines`, () => {
    const over = [...sizes].filter(([file, size]) => size > BUDGET && !(file in OVERSIZED));
    expect(over.map(([file, size]) => `${file}: ${size}`)).toEqual([]);
  });

  test("an oversized file does not grow past its cap", () => {
    const grown = Object.entries(OVERSIZED).filter(([file, cap]) => (sizes.get(file) ?? 0) > cap);
    expect(grown.map(([file, cap]) => `${file}: ${sizes.get(file)} > ${cap}`)).toEqual([]);
  });

  test("a cap is lowered once its file shrinks", () => {
    const stale = Object.entries(OVERSIZED).filter(([file, cap]) => (sizes.get(file) ?? 0) < cap - SLACK);
    expect(stale.map(([file, cap]) => `${file}: ${sizes.get(file) ?? "gone"}, cap ${cap}`)).toEqual([]);
  });
});
