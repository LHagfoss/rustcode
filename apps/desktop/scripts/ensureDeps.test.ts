import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { depsStaleReason, writeLockfileStamp } from "./ensure-deps";

function makeTempDir() {
  return mkdtempSync(join(tmpdir(), "rustcode-ensure-deps-"));
}

function seedBins(dir: string, bins: string[]) {
  const binDir = join(dir, "node_modules/.bin");
  mkdirSync(binDir, { recursive: true });
  for (const bin of bins) writeFileSync(join(binDir, bin), "");
}

describe("ensure-deps", () => {
  test("reports missing required binaries", () => {
    const dir = makeTempDir();
    try {
      seedBins(dir, ["tsc", "biome", "vite"]);
      expect(depsStaleReason(dir)).toContain("playwright");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("detects bun.lock drift once binaries are present", () => {
    const dir = makeTempDir();
    try {
      seedBins(dir, ["tsc", "biome", "vite", "playwright"]);
      writeFileSync(join(dir, "bun.lock"), "v1\n");
      expect(depsStaleReason(dir)).toBe("bun.lock changed since the last install");

      writeLockfileStamp(dir);
      expect(depsStaleReason(dir)).toBeNull();

      writeFileSync(join(dir, "bun.lock"), "v2\n");
      expect(depsStaleReason(dir)).toBe("bun.lock changed since the last install");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
