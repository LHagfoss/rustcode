// Copies what `bun run bundle` built into release assets with stable names,
// the ones the installers, the in-app updater and SHA256SUMS use:
//
//   macOS    RustCode-<ver>-macos-<arch>.app.zip    (ditto zip of RustCode.app)
//   Windows  RustCode-<ver>-windows-x64-setup.exe   (NSIS, per-user)
//   Linux    RustCode-<ver>-linux-<arch>.AppImage, RustCode-<ver>-linux-<arch>.deb
//
// Only the bundles this platform built are copied; one that is missing fails.
// With --sums it also writes <out>/SHA256SUMS over every file in <out>, the way
// the release workflow does, for a local installer test
// (RUSTCODE_INSTALL_ARTIFACTS, docs/install.md).
//
//   bun scripts/release-assets.ts <out dir> [--sums]

import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { resolve } from "node:path";

const args = process.argv.slice(2);
const outArg = args.find((a) => !a.startsWith("--"));
if (!outArg) throw new Error("usage: bun scripts/release-assets.ts <out dir> [--sums]");
const out = resolve(outArg);
const version = JSON.parse(
  readFileSync(resolve(import.meta.dir, "../src-tauri/gen-version/tauri.conf.json"), "utf8"),
).version as string;
const bundle = resolve(import.meta.dir, "../src-tauri/target/release/bundle");
mkdirSync(out, { recursive: true });

const arch = process.arch === "arm64" ? "aarch64" : "x86_64";

function only(dir: string, suffix: string): string {
  const found = existsSync(dir)
    ? readdirSync(dir).filter((f) => f.endsWith(suffix) && f.includes(version))
    : [];
  if (found.length !== 1)
    throw new Error(`Expected one ${version} *${suffix} in ${dir}, found ${found.length}.`);
  return resolve(dir, found[0]!);
}

function copy(from: string, name: string) {
  copyFileSync(from, resolve(out, name));
  console.log(`${name}  <-  ${from}`);
}

if (process.platform === "darwin") {
  const app = resolve(bundle, "macos/RustCode.app");
  if (!existsSync(app)) throw new Error(`No ${app}; run \`bun run bundle\` first.`);
  const name = `RustCode-${version}-macos-${arch}.app.zip`;
  rmSync(resolve(out, name), { force: true });
  const zip = Bun.spawnSync(
    ["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", app, resolve(out, name)],
    {
      stdout: "inherit",
      stderr: "inherit",
    },
  );
  if (zip.exitCode !== 0) process.exit(zip.exitCode ?? 1);
  console.log(`${name}  <-  ${app}`);
} else if (process.platform === "win32") {
  copy(only(resolve(bundle, "nsis"), "-setup.exe"), `RustCode-${version}-windows-x64-setup.exe`);
} else {
  copy(only(resolve(bundle, "appimage"), ".AppImage"), `RustCode-${version}-linux-${arch}.AppImage`);
  copy(only(resolve(bundle, "deb"), ".deb"), `RustCode-${version}-linux-${arch}.deb`);
}

if (args.includes("--sums")) {
  const lines = readdirSync(out)
    .filter((f) => f !== "SHA256SUMS")
    .sort()
    .map(
      (f) =>
        `${createHash("sha256")
          .update(readFileSync(resolve(out, f)))
          .digest("hex")}  ${f}`,
    );
  writeFileSync(resolve(out, "SHA256SUMS"), `${lines.join("\n")}\n`);
  console.log(`SHA256SUMS: ${lines.length} files`);
}
