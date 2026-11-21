// `bun scripts/ensure-deps.ts`: installs `node_modules` from `bun.lock` when a
// fresh checkout or worktree has none, so `bun run check` and `mock-window open`
// work on the first run instead of failing with `tsc: command not found`.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dir, "..");
/** Binaries the checks and the mock window run; any missing one means no usable install. */
const REQUIRED = ["tsc", "biome", "vite"];

export function ensureDeps() {
  const missing = REQUIRED.filter((bin) => !existsSync(join(root, "node_modules/.bin", bin)));
  if (missing.length === 0) return;
  console.error(`node_modules is missing ${missing.join(", ")}; running bun install --frozen-lockfile`);
  const result = spawnSync(process.execPath, ["install", "--frozen-lockfile"], {
    cwd: root,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    console.error("bun install failed. Run `bun install` in apps/desktop and try again.");
    process.exit(result.status ?? 1);
  }
}

if (import.meta.main) ensureDeps();
