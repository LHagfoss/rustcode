// Builds the engine and puts it where Tauri bundles a sidecar:
// src-tauri/binaries/rustcode-<target triple>. `bun run bundle` then runs
// `tauri build` with src-tauri/tauri.bundle.conf.json, which names it in
// `externalBin`; the app finds it next to its own binary.
//
//   bun scripts/bundle-engine.ts            debug engine (fast, for a smoke test)
//   bun scripts/bundle-engine.ts --release  release engine (for a real bundle)

import { chmodSync, copyFileSync, mkdirSync } from "node:fs";
import { resolve } from "node:path";

const release = process.argv.includes("--release");
const repo = resolve(import.meta.dir, "../../..");
const triple =
  process.env.TAURI_ENV_TARGET_TRIPLE ??
  /host: (\S+)/.exec(Bun.spawnSync(["rustc", "-vV"]).stdout.toString())?.[1];
if (!triple) throw new Error("Could not read the target triple from `rustc -vV`.");

const args = ["cargo", "build", "-p", "rustcode-tui", "--bin", "rustcode", ...(release ? ["--release"] : [])];
console.log(`$ ${args.join(" ")}`);
const build = Bun.spawnSync(args, { cwd: repo, stdout: "inherit", stderr: "inherit" });
if (build.exitCode !== 0) process.exit(build.exitCode ?? 1);

const exe = triple.includes("windows") ? ".exe" : "";
const from = resolve(repo, "target", release ? "release" : "debug", `rustcode${exe}`);
const dir = resolve(import.meta.dir, "../src-tauri/binaries");
const to = resolve(dir, `rustcode-${triple}${exe}`);
mkdirSync(dir, { recursive: true });
copyFileSync(from, to);
chmodSync(to, 0o755);
console.log(`sidecar: ${to}`);
