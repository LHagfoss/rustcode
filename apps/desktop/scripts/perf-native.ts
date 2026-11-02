// `bun run perf:native [prod|dev] [--near60] [--rate N] [--seconds N]`: the perf
// scenarios (scripts/perf-native-scenario.js) in a real WKWebView, the engine the
// desktop app ships with, instead of headless Chrome (`bun run perf`). `prod` is a
// `vite build` (what a bundled app runs), `dev` the Vite dev server (what
// `bun tauri dev` shows). `--near60` keeps WebKit's 60 fps preference on, as the
// app had it before src-tauri/src/webview.rs. It opens a floating 1440×900 window
// for about a minute; the display must stay awake (a locked screen throttles
// WebKit to a few frames a second, and `visible` in the output says so).
// docs/desktop.md › Native frame rate.

import { tmpdir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const mode = args.find((a) => a === "dev" || a === "prod") ?? "prod";
const flag = (name: string, fallback: number) => {
  const at = args.indexOf(`--${name}`);
  return at >= 0 ? Number(args[at + 1]) : fallback;
};
const PORT = flag("port", mode === "dev" ? 1443 : 1444);
const root = join(import.meta.dir, "..");
const vite = join(root, "node_modules/.bin/vite");
const children: { kill(): void }[] = [];
process.on("exit", () => {
  for (const c of children) c.kill();
});

const binary = join(tmpdir(), "rustcode-perf-native");
const swift = Bun.spawnSync(["swiftc", "-O", join(import.meta.dir, "perf-native.swift"), "-o", binary]);
if (swift.exitCode !== 0) throw new Error(`swiftc failed:\n${swift.stderr}`);

if (mode === "prod") {
  const out = join(tmpdir(), "rustcode-desktop-perf-native-dist");
  const build = Bun.spawnSync([vite, "build", "--outDir", out, "--emptyOutDir", "--logLevel", "error"], {
    cwd: root,
  });
  if (build.exitCode !== 0) throw new Error(`vite build failed:\n${build.stderr}`);
  children.push(
    Bun.spawn([vite, "preview", "--outDir", out, "--port", String(PORT), "--strictPort"], {
      cwd: root,
      stdout: "ignore",
      stderr: "ignore",
    }),
  );
} else {
  // Its own dependency cache, so `bun tauri dev`'s server does not re-optimize.
  const config = join(tmpdir(), `rustcode-desktop-perf-native-vite-${PORT}.config.mjs`);
  const cache = join(tmpdir(), `rustcode-desktop-perf-native-vite-cache-${PORT}`);
  await Bun.write(
    config,
    `import base from ${JSON.stringify(join(root, "vite.config.ts"))};\nexport default { ...base, cacheDir: ${JSON.stringify(cache)} };\n`,
  );
  children.push(
    Bun.spawn([vite, "--config", config, "--port", String(PORT), "--strictPort"], {
      cwd: root,
      stdout: "ignore",
      stderr: "ignore",
    }),
  );
}
const base = `http://localhost:${PORT}`;
for (let i = 0; ; i++) {
  try {
    if ((await fetch(base)).ok) break;
  } catch {}
  if (i > 300) throw new Error(`Nothing answered at ${base}`);
  await Bun.sleep(200);
}

const url = `${base}/?transport=perf&threads=500&messages=${flag("messages", 2000)}&rate=${flag("rate", 100)}&seconds=${flag("seconds", 5)}`;
const run = Bun.spawnSync(
  [
    binary,
    url,
    join(import.meta.dir, "perf-native-scenario.js"),
    ...(args.includes("--near60") ? ["--near60"] : []),
  ],
  { stdout: "pipe", stderr: "inherit" },
);
const text = run.stdout.toString();
const json = text.slice(text.indexOf("{"));
// biome-ignore lint/suspicious/noExplicitAny: the harness's JSON
const d = JSON.parse(json) as any;
console.log(
  `perf:native (${mode}${args.includes("--near60") ? ", near-60 fps preference on" : ""}): display ${d.screenMaxFps} Hz, Low Power Mode ${d.lowPowerMode ? "on" : "off"}, window ${d.visible ? "visible" : "NOT visible (numbers invalid)"}`,
);
for (const phase of ["idle", "scroll", "stream", "switcher", "settings", "sidebar", "panel"]) {
  const p = d[phase];
  if (!p) continue;
  const cpu = d.cpu?.[phase] ?? {};
  console.log(
    `  ${phase.padEnd(9)} ${String(p.fps).padStart(6)} fps  frame p95 ${String(p.frameP95Ms).padStart(5)} ms  late ${String(p.lateFrames).padStart(3)}  work p95 ${String(p.workP95Ms).padStart(4)} ms  long tasks ${p.longTasks} (max ${p.longestTaskMs} ms)  CPU web ${cpu.webContentCpuPct ?? "?"}% gpu ${cpu.gpuCpuPct ?? "?"}% WindowServer ${cpu.windowServerCpuPct ?? "?"}%`,
  );
}
process.exit(0);
