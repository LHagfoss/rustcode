// Compiles the Icon Composer source (src-tauri/icons/AppIcon.icon) into
// Assets.car, the layered Liquid Glass icon macOS 26 draws. It also writes src-tauri/gen-icon/
// tauri.conf.json, which `bun run bundle` merges after tauri.bundle.conf.json:
// it ships Assets.car at Contents/Resources/Assets.car, and Info.plist names it
// with CFBundleIconName. Without macOS or Xcode 26's actool the merged config is
// empty and the bundle keeps the flat icons/icon.icns (CFBundleIconFile).
//
//   bun scripts/app-icon.ts

import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const source = resolve(import.meta.dir, "../src-tauri/icons/AppIcon.icon");
const out = resolve(import.meta.dir, "../src-tauri/gen-icon");
const compiled = resolve(out, "compiled");

rmSync(out, { recursive: true, force: true });
mkdirSync(compiled, { recursive: true });

function writeConfig(config: object) {
  writeFileSync(resolve(out, "tauri.conf.json"), `${JSON.stringify(config, null, 2)}\n`);
}

function skip(reason: string) {
  console.warn(`warning: ${reason}; the bundle uses the flat icons/icon.icns.`);
  writeConfig({});
  process.exit(0);
}

if (process.platform !== "darwin") skip("the layered app icon needs macOS to compile");
if (Bun.spawnSync(["xcrun", "-f", "actool"]).exitCode !== 0)
  skip("xcrun actool was not found (install Xcode 26 or later)");

const args = [
  "xcrun",
  "actool",
  source,
  "--compile",
  compiled,
  "--app-icon",
  "AppIcon",
  "--platform",
  "macosx",
  "--minimum-deployment-target",
  "14.0",
  "--output-partial-info-plist",
  resolve(out, "partial-info.plist"),
];
console.log(`$ ${args.join(" ")}`);
const run = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe" });
const car = resolve(compiled, "Assets.car");
if (run.exitCode !== 0 || !existsSync(car)) {
  const detail = `${run.stderr.toString()}${run.stdout.toString()}`.trim();
  if (detail) console.warn(detail);
  skip("actool could not compile AppIcon.icon (it needs Xcode 26 or later)");
}

writeConfig({ bundle: { macOS: { files: { "Resources/Assets.car": car } } } });
console.log(`Compiled the layered app icon into ${car}`);
