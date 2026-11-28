// `bun scripts/ensure-deps.ts`: installs `node_modules` from `bun.lock` when a
// fresh checkout or worktree has none, so `bun run check` and `mock-window open`
// work on the first run instead of failing with `tsc: command not found`.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dir, "..");
/** Binaries the checks and the mock window run; any missing one means no usable install. */
const REQUIRED = ["tsc", "biome", "vite", "playwright"];
const STAMP_PATH = join("node_modules", ".rustcode-bun-lock.sha256");

export function lockfileHash(dir = root): string | null {
  const lock = join(dir, "bun.lock");
  if (!existsSync(lock)) return null;
  return createHash("sha256").update(readFileSync(lock)).digest("hex");
}

export function depsStaleReason(dir = root): string | null {
  const missing = REQUIRED.filter((bin) => !existsSync(join(dir, "node_modules/.bin", bin)));
  if (missing.length > 0) return `node_modules is missing ${missing.join(", ")}`;
  const want = lockfileHash(dir);
  if (!want) return null;
  const stamp = join(dir, STAMP_PATH);
  const have = existsSync(stamp) ? readFileSync(stamp, "utf8").trim() : null;
  if (have !== want) return "bun.lock changed since the last install";
  return null;
}

export function writeLockfileStamp(dir = root, hash = lockfileHash(dir)) {
  if (!hash) return;
  const modulesDir = join(dir, "node_modules");
  if (!existsSync(modulesDir)) mkdirSync(modulesDir, { recursive: true });
  writeFileSync(join(dir, STAMP_PATH), `${hash}\n`);
}

export function ensureDeps(dir = root) {
  const reason = depsStaleReason(dir);
  if (!reason) return;
  console.error(`${reason}; running bun install --frozen-lockfile`);
  const result = spawnSync(process.execPath, ["install", "--frozen-lockfile"], {
    cwd: dir,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    console.error("bun install failed. Run `bun install` in apps/desktop and try again.");
    process.exit(result.status ?? 1);
  }
  writeLockfileStamp(dir);
}

if (import.meta.main) ensureDeps();
