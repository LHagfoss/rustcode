// `bun run perf [dev|prod] [--seconds N] [--rate N]`: drives the UI against the
// perf mock (`src/acp/perf.ts`) in headless Chrome over the DevTools protocol
// and prints startup, thread-open, streaming, typing, scrolling and idle numbers.
// `dev` is the Vite dev server (what `bun tauri dev` shows), `prod` a `vite build`.
// Chrome stands in for WKWebView: compare runs with each other, not with the app.

import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const mode = args.find((a) => a === "dev" || a === "prod") ?? "prod";
const flag = (name: string, fallback: number) => {
  const at = args.indexOf(`--${name}`);
  return at >= 0 ? Number(args[at + 1]) : fallback;
};
const SECONDS = flag("seconds", 30);
const RATE = flag("rate", 100);
const PORT = flag("port", mode === "dev" ? 1433 : 1434);
const CDP_PORT = flag("cdp", 9334);
const REPLAY =
  args[args.indexOf("--replay") + 1] && args.includes("--replay")
    ? args[args.indexOf("--replay") + 1]
    : undefined;
const JSON_OUT = args.includes("--json");

const root = join(import.meta.dir, "..");
const children: { kill(): void }[] = [];
const cleanup = () => {
  for (const c of children) c.kill();
};
process.on("exit", cleanup);
process.on("SIGINT", () => process.exit(130));

async function waitFor(url: string, ms = 60_000) {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {}
    await Bun.sleep(200);
  }
  throw new Error(`Nothing answered at ${url}`);
}

// ---------------------------------------------------------------- server
const vite = join(root, "node_modules/.bin/vite");
if (mode === "prod") {
  const out = join(tmpdir(), "rustcode-desktop-perf-dist");
  // `--profile` keeps names readable: production React, unminified.
  const minify = args.includes("--profile") ? ["--minify", "false"] : [];
  const build = Bun.spawnSync(
    [vite, "build", "--outDir", out, "--emptyOutDir", "--logLevel", "error", ...minify],
    {
      cwd: root,
    },
  );
  if (build.exitCode !== 0) throw new Error(`vite build failed:\n${build.stderr}`);
  children.push(
    Bun.spawn([vite, "preview", "--outDir", out, "--port", String(PORT), "--strictPort"], {
      cwd: root,
      stdout: "ignore",
      stderr: "ignore",
    }),
  );
} else {
  // Its own dependency cache: a second dev server on the shared one would make `bun tauri dev` re-optimize.
  const config = join(tmpdir(), `rustcode-desktop-perf-vite-${PORT}.config.mjs`);
  const cache = join(tmpdir(), `rustcode-desktop-perf-vite-cache-${PORT}`);
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
await waitFor(base);

// ---------------------------------------------------------------- browser
const chromePath = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
if (!existsSync(chromePath)) throw new Error("Google Chrome is needed for bun run perf.");
const profile = join(tmpdir(), `rustcode-perf-chrome-${CDP_PORT}`);
children.push(
  Bun.spawn(
    [
      chromePath,
      "--headless=new",
      `--remote-debugging-port=${CDP_PORT}`,
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--enable-precise-memory-info",
      "--window-size=1440,900",
      "about:blank",
    ],
    { stdout: "ignore", stderr: "ignore" },
  ),
);
await waitFor(`http://127.0.0.1:${CDP_PORT}/json/version`);
const tabs = (await (await fetch(`http://127.0.0.1:${CDP_PORT}/json`)).json()) as {
  type: string;
  webSocketDebuggerUrl: string;
}[];
const tab = tabs.find((t) => t.type === "page");
if (!tab) throw new Error("No Chrome page");
const ws = new WebSocket(tab.webSocketDebuggerUrl);
await new Promise((r) => {
  ws.onopen = r;
});
let nextId = 0;
const pending = new Map<number, { resolve: (v: Cdp) => void; reject: (e: Error) => void }>();
const traceEvents: { name: string; ph: string; dur?: number; tid: number; args?: { name?: string } }[] = [];
let traceDone: (() => void) | null = null;
ws.onmessage = (m) => {
  const d = JSON.parse(String(m.data));
  if (d.method === "Tracing.dataCollected") traceEvents.push(...d.params.value);
  if (d.method === "Tracing.tracingComplete") traceDone?.();
  const p = d.id ? pending.get(d.id) : undefined;
  if (!p) return;
  pending.delete(d.id);
  if (d.error) p.reject(new Error(JSON.stringify(d.error)));
  else p.resolve(d.result);
};
// biome-ignore lint/suspicious/noExplicitAny: DevTools protocol results are untyped here
type Cdp = any;
const cdp = (method: string, params: object = {}) =>
  new Promise<Cdp>((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
async function evaluate<T = Cdp>(expression: string): Promise<T> {
  const r = await cdp("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails)
    throw new Error(`${expression.slice(0, 80)}: ${JSON.stringify(r.exceptionDetails).slice(0, 400)}`);
  return r.result.value as T;
}

// The probe: React commits (a devtools hook stub works in production builds too),
// long tasks, frame intervals and key-to-next-frame latency.
const PROBE = `(() => {
  const p = window.__perf = { commits: 0, longtasks: [], frames: [], keys: [], events: [] };
  window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = { renderers: new Map(), supportsFiber: true, isDisabled: false,
    inject() { return 1; }, onCommitFiberRoot() { p.commits++; }, onCommitFiberUnmount() {}, onPostCommitFiberRoot() {},
    onScheduleFiberRoot() {}, checkDCE() {} };
  new PerformanceObserver((l) => { for (const e of l.getEntries()) p.longtasks.push(e.duration); })
    .observe({ type: "longtask", buffered: true });
  new PerformanceObserver((l) => { for (const e of l.getEntries()) p.events.push([e.name, e.duration]); })
    .observe({ type: "event", durationThreshold: 16, buffered: true });
  let last = 0;
  const loop = (t) => { if (last) p.frames.push(t - last); last = t; requestAnimationFrame(loop); };
  requestAnimationFrame(loop);
  addEventListener("keydown", (e) => {
    const at = e.timeStamp;
    requestAnimationFrame(() => p.keys.push(performance.now() - at));
  }, true);
  p.reset = () => { p.commits = 0; p.longtasks = []; p.frames = []; p.keys = []; p.events = []; };
  p.until = (test, ms = 30000) => new Promise((resolve, reject) => {
    const t0 = performance.now();
    const tick = () => { let ok = false; try { ok = test(); } catch {} if (ok) resolve(performance.now() - t0);
      else if (performance.now() - t0 > ms) reject(new Error("timed out")); else requestAnimationFrame(tick); };
    tick();
  });
})();`;

await cdp("Page.enable");
await cdp("Performance.enable");
await cdp("Emulation.setDeviceMetricsOverride", {
  width: 1440,
  height: 900,
  deviceScaleFactor: 2,
  mobile: false,
});
await cdp("Page.addScriptToEvaluateOnNewDocument", { source: PROBE });
if (REPLAY) {
  const lines = (await Bun.file(REPLAY).text()).split("\n").filter(Boolean);
  await cdp("Page.addScriptToEvaluateOnNewDocument", {
    source: `window.__perfReplay = ${JSON.stringify(lines)};`,
  });
}

const stat = (xs: number[]) => {
  const s = [...xs].sort((a, b) => a - b);
  const q = (f: number) => (s.length ? s[Math.min(s.length - 1, Math.floor(f * s.length))]! : 0);
  return { n: s.length, p50: q(0.5), p95: q(0.95), max: s.at(-1) ?? 0 };
};
type Probe = {
  commits: number;
  longtasks: number[];
  frames: number[];
  keys: number[];
  events: [string, number][];
};
const probe = () => evaluate<Probe>("({ ...window.__perf, reset: undefined, until: undefined })");
const metrics = async () =>
  Object.fromEntries(
    ((await cdp("Performance.getMetrics")).metrics as { name: string; value: number }[]).map((m) => [
      m.name,
      m.value,
    ]),
  );
const heapMb = async () => {
  await cdp("HeapProfiler.collectGarbage");
  return (await cdp("Runtime.getHeapUsage")).usedSize / 1048576;
};
const results: Record<string, string | number> = {};
if (!JSON_OUT) console.log(`perf (${mode}, ${RATE} chunks/s for ${SECONDS} s)`);
const put = (key: string, value: number | string) => {
  results[key] = typeof value === "number" ? Math.round(value * 10) / 10 : value;
  // Printed as it goes, so a run that stalls still shows what it measured.
  if (!JSON_OUT) console.log(`  ${key.padEnd(28)}  ${results[key]}`);
};
const frameStats = (frames: number[], seconds: number) => {
  const dropped = frames.reduce((n, f) => n + Math.max(0, Math.round(f / 16.67) - 1), 0);
  return { fps: frames.length / seconds, dropped };
};

// ---------------------------------------------------------------- startup
const MESSAGES = flag("messages", 2000);
const url = `${base}/?transport=perf&threads=500&messages=${MESSAGES}&rate=${RATE}&seconds=${SECONDS}`;
const t0 = Date.now();
await cdp("Page.navigate", { url });
await evaluate(
  "new Promise((r) => (document.readyState === 'complete' ? r(0) : addEventListener('load', r)))",
);
await evaluate(
  "window.__perf.until(() => window.__rustcode?.useApp.getState().sessionsLoaded && document.querySelectorAll('aside li, aside [role=button], aside button').length > 50, 60000)",
);
const startup = await evaluate<{ fcp: number; list: number }>(`(() => ({
  fcp: performance.getEntriesByName("first-contentful-paint")[0]?.startTime ?? -1,
  list: performance.now(),
}))()`);
put("startup.firstPaintMs", startup.fcp);
put("startup.threadListMs", startup.list);
put("startup.wallMs", Date.now() - t0);
await Bun.sleep(1500);
put("startup.heapMb", await heapMb());

// ---------------------------------------------------------------- open threads
const PROFILE = args.includes("--profile");
/** Top self-time functions of a CPU profile, for `--profile`. */
function topFunctions(
  profile: {
    nodes: {
      id: number;
      callFrame: { functionName: string; url: string; lineNumber: number };
      hitCount?: number;
    }[];
    samples: number[];
    timeDeltas: number[];
  },
  n = 25,
) {
  const self = new Map<number, number>();
  profile.samples.forEach((id, i) => {
    self.set(id, (self.get(id) ?? 0) + (profile.timeDeltas[i] ?? 0));
  });
  const keyOf = (f: { functionName: string; url: string; lineNumber: number }) =>
    `${f.functionName || "(anon)"} ${f.url.split("/").slice(-2).join("/")}:${f.lineNumber}`;
  const byName = new Map<string, number>();
  for (const node of profile.nodes) {
    const key = keyOf(node.callFrame);
    byName.set(key, (byName.get(key) ?? 0) + (self.get(node.id) ?? 0) / 1000);
  }
  if (!args.includes("--inclusive")) return [...byName].sort((a, b) => b[1] - a[1]).slice(0, n);
  // Inclusive: each function's subtree, counted once per stack.
  const children = profile.nodes as unknown as {
    id: number;
    children?: number[];
    callFrame: Parameters<typeof keyOf>[0];
  }[];
  const byId = new Map(children.map((c) => [c.id, c]));
  const total = new Map<string, number>();
  const walk = (id: number, seen: Set<string>): number => {
    const node = byId.get(id)!;
    let sum = (self.get(id) ?? 0) / 1000;
    const key = keyOf(node.callFrame);
    const fresh = !seen.has(key);
    if (fresh) seen.add(key);
    for (const child of node.children ?? []) sum += walk(child, seen);
    if (fresh) {
      seen.delete(key);
      total.set(key, (total.get(key) ?? 0) + sum);
    }
    return sum;
  };
  walk(children[0]!.id, new Set());
  return [...total].sort((a, b) => b[1] - a[1]).slice(0, n * 2);
}
const TRACE = args.includes("--trace");
/** `--trace`: main-thread time by trace event (script, style, layout, paint) for a step. */
async function traced<T>(label: string, run: () => Promise<T>): Promise<T> {
  traceEvents.length = 0;
  await cdp("Tracing.start", {
    categories: "devtools.timeline,disabled-by-default-devtools.timeline",
    transferMode: "ReportEvents",
  });
  const out = await run();
  const done = new Promise<void>((r) => {
    traceDone = r;
  });
  await cdp("Tracing.end");
  await done;
  const mains = new Set(
    traceEvents
      .filter((e) => e.name === "thread_name" && e.args?.name === "CrRendererMain")
      .map((e) => e.tid),
  );
  const sums = new Map<string, number>();
  for (const e of traceEvents) {
    if (e.ph !== "X" || !mains.has(e.tid) || !e.dur) continue;
    sums.set(e.name, (sums.get(e.name) ?? 0) + e.dur / 1000);
  }
  console.log(`-- trace ${label} (inclusive ms, main thread)`);
  for (const [k, ms] of [...sums].sort((a, b) => b[1] - a[1]).slice(0, 14))
    console.log(`  ${ms.toFixed(1).padStart(9)} ms  ${k}`);
  return out;
}

async function profiled<T>(label: string, run: () => Promise<T>): Promise<T> {
  if (TRACE) return traced(label, run);
  if (!PROFILE) return run();
  await cdp("Profiler.enable");
  await cdp("Profiler.setSamplingInterval", { interval: 200 });
  await cdp("Profiler.start");
  const out = await run();
  const { profile } = await cdp("Profiler.stop");
  console.log(`-- profile ${label}`);
  for (const [k, ms] of topFunctions(profile)) console.log(`  ${ms.toFixed(1).padStart(9)} ms  ${k}`);
  return out;
}

const OPEN_TIMEOUT_MS = flag("open-timeout", 120_000);
async function openThread(id: string, label: string) {
  await evaluate("window.__perf.reset()");
  const ms = await profiled(label, () =>
    evaluate<number>(`(async () => {
    const t0 = performance.now();
    window.__rustcode.selectThread(${JSON.stringify(id)});
    await window.__perf.until(() => {
      const t = window.__rustcode.useApp.getState().threads[${JSON.stringify(id)}];
      return t?.loaded && document.querySelectorAll('[role=log] [data-index]').length > 3;
    }, ${OPEN_TIMEOUT_MS}).catch(() => -1);
    await new Promise((r) => requestAnimationFrame(() => setTimeout(r, 0)));
    return performance.now() - t0;
  })()`),
  ).then((ms) => ms);
  const timedOut = await evaluate<boolean>(
    `!window.__rustcode.useApp.getState().threads[${JSON.stringify(id)}]?.loaded`,
  );
  await Bun.sleep(1000);
  const p = await probe();
  const items = await evaluate<number>(
    `window.__rustcode.useApp.getState().threads[${JSON.stringify(id)}]?.items.length ?? 0`,
  );
  put(`${label}.items`, items);
  put(`${label}.toContentMs`, timedOut ? `>${OPEN_TIMEOUT_MS} (not loaded)` : ms);
  put(`${label}.longestTaskMs`, stat(p.longtasks).max);
  put(`${label}.commits`, p.commits);
  put(`${label}.heapMb`, await heapMb());
}
await openThread("perf-long", "open2000");
if (REPLAY) await openThread("perf-real", "openReal");

// ---------------------------------------------------------------- scrolling the long thread
await openThread("perf-long", "reopen2000");
await evaluate("window.__perf.reset()");
{
  const box = await evaluate<{ x: number; y: number }>(
    `(() => { const r = document.querySelector('[role=log]').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`,
  );
  const start = Date.now();
  let n = 0;
  while (Date.now() - start < 3000) {
    await cdp("Input.dispatchMouseEvent", {
      type: "mouseWheel",
      x: box.x,
      y: box.y,
      deltaX: 0,
      deltaY: n++ < 90 ? -120 : 120,
    });
    await Bun.sleep(16);
  }
  const p = await probe();
  const f = frameStats(p.frames, 3);
  put("scroll.fps", f.fps);
  put("scroll.droppedFrames", f.dropped);
  put("scroll.longestTaskMs", stat(p.longtasks).max);
}

// ---------------------------------------------------------------- streaming + typing
await evaluate(`window.__rustcode.selectThread("perf-stream")`);
await evaluate(
  `window.__perf.until(() => window.__rustcode.useApp.getState().threads["perf-stream"]?.loaded)`,
);
await Bun.sleep(500);
await evaluate("window.__perf.reset()");
const before = await metrics();
await evaluate(`void window.__rustcode.runPrompt("perf-stream", "stream")`);
await evaluate(`document.querySelector('textarea[aria-label="Message"]')?.focus()`);
const streamStart = Date.now();
await profiled("stream", async () => {
  while (Date.now() - streamStart < SECONDS * 1000) {
    const key = "abcdefghij"[Math.floor(Math.random() * 10)]!;
    await cdp("Input.dispatchKeyEvent", { type: "keyDown", key, text: key, code: `Key${key.toUpperCase()}` });
    await cdp("Input.dispatchKeyEvent", { type: "keyUp", key, code: `Key${key.toUpperCase()}` });
    await Bun.sleep(120);
  }
});
{
  const after = await metrics();
  const p = await probe();
  const f = frameStats(p.frames, SECONDS);
  const keys = stat(p.keys);
  const lt = stat(p.longtasks);
  put("stream.chunksPerSec", RATE);
  put("stream.commitsPerSec", p.commits / SECONDS);
  put("stream.longTasks>50ms", p.longtasks.filter((d) => d > 50).length);
  put("stream.longestTaskMs", lt.max);
  put("stream.fps", f.fps);
  put("stream.droppedFrames", f.dropped);
  put("stream.keyToFrameP50Ms", keys.p50);
  put("stream.keyToFrameP95Ms", keys.p95);
  put("stream.slowInputEvents", p.events.filter(([name]) => /key|input/.test(name)).length);
  put("stream.mainThreadBusyPct", (((after.TaskDuration ?? 0) - (before.TaskDuration ?? 0)) / SECONDS) * 100);
  put("stream.heapMb", await heapMb());
}
{
  const t = performance.now();
  const ended = await evaluate<boolean>(
    `window.__perf.until(() => !window.__rustcode.useApp.getState().threads["perf-stream"].running, 120000).then(() => true, () => false)`,
  );
  // The turn ends when the UI has applied every chunk: time past the stream is backlog.
  put("stream.backlogAfterEndMs", ended ? performance.now() - t : ">120000");
}

// ---------------------------------------------------------------- ⌘K switcher over 500 threads
await evaluate(`window.__rustcode.setSwitcher(true)`);
await evaluate(
  `window.__perf.until(() => document.querySelector('input[aria-label="Search threads and actions"]'))`,
);
await evaluate("window.__perf.reset()");
for (const key of "thread 4") {
  await cdp("Input.dispatchKeyEvent", { type: "keyDown", key, text: key });
  await cdp("Input.dispatchKeyEvent", { type: "keyUp", key });
  await Bun.sleep(80);
}
await Bun.sleep(300);
{
  const p = await probe();
  put("switcher.keyToFrameP95Ms", stat(p.keys).p95);
  put("switcher.longestTaskMs", stat(p.longtasks).max);
}
await evaluate(`window.__rustcode.setSwitcher(false)`);

// ---------------------------------------------------------------- sidebar filter over 500 threads
await evaluate(`document.querySelector('input[aria-label="Search threads"]')?.focus()`);
await evaluate("window.__perf.reset()");
for (const key of "project") {
  await cdp("Input.dispatchKeyEvent", { type: "keyDown", key, text: key });
  await cdp("Input.dispatchKeyEvent", { type: "keyUp", key });
  await Bun.sleep(80);
}
await Bun.sleep(300);
{
  const p = await probe();
  put("sidebar.keyToFrameP95Ms", stat(p.keys).p95);
  put("sidebar.longestTaskMs", stat(p.longtasks).max);
}

// ---------------------------------------------------------------- idle
await evaluate(
  `(() => { const el = document.querySelector('input[aria-label="Search threads"]'); el?.blur(); })()`,
);
await Bun.sleep(1500);
{
  const a = await metrics();
  await evaluate("window.__perf.reset()");
  await Bun.sleep(5000);
  const b = await metrics();
  const p = await probe();
  put("idle.mainThreadBusyPct", (((b.TaskDuration ?? 0) - (a.TaskDuration ?? 0)) / 5) * 100);
  put("idle.commitsPer5s", p.commits);
}

if (JSON_OUT) console.log(JSON.stringify({ mode, ...results }));
ws.close();
process.exit(0);
