// Writes src-tauri/gen-version/tauri.conf.json with the repository's release
// version (the root Cargo.toml `version`, the engine's), which `bun run
// bundle` merges into the build so RustCode.app, the Windows installer and
// the Linux packages carry the same version as the engine and the release
// tag. tauri.conf.json keeps 0.1.0 for `tauri dev`; the in-app updater treats
// that build as a development one and never replaces it.
//
//   bun scripts/bundle-version.ts            prints the version it wrote

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const repo = resolve(import.meta.dir, "../../..");
const manifest = readFileSync(resolve(repo, "Cargo.toml"), "utf8");
const version = /^\[package\][^[]*?^version\s*=\s*"([^"]+)"/ms.exec(manifest)?.[1];
if (!version || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error("Could not read the [package] version from the root Cargo.toml.");
}

const out = resolve(import.meta.dir, "../src-tauri/gen-version");
mkdirSync(out, { recursive: true });
writeFileSync(resolve(out, "tauri.conf.json"), `${JSON.stringify({ version }, null, 2)}\n`);
console.log(version);
