// `bun run mock-window <open [path] [--show] | eval "<js>" | click "<css>" [--at <x>,<y>] |
//   dblclick "<css>" [--at <x>,<y>] | hover "<css>" [--at <x>,<y>] [--wait <ms>] |
//   drag "<from css>" "<to css>" [--from <x>,<y>] [--to <x>,<y>] [--steps <n>] |
//   scroll "<css>" <dy> | move <x>,<y> |
//   type "<text>" | press <key> [--shift] [--ctrl] [--alt] [--meta] |
//   screenshot <file.png> [--selector "<css>" | --clip <x>,<y>,<w>,<h>] | close>`:
// One shared Chrome at native scale, with a tab and mock server per checkout.
// `open --show` starts visible Chrome if the browser is not running yet.
// `close` releases this checkout's tab and server. Chrome stays available for reuse.
// `eval` runs in REPL mode, so top-level `await` and `await import(...)` work.
// MOCK_WINDOW_CDP_PORT and MOCK_WINDOW_PROFILE isolate tests of the helper.

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, existsSync, openSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { ensureDeps } from "./ensure-deps";

const root = join(import.meta.dir, "..");
// Chrome where it installs on macOS and Linux; `MOCK_WINDOW_CHROME` names another binary.
const CHROME_PATHS = [
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome-stable",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
];
const chromePath =
  process.env.MOCK_WINDOW_CHROME || CHROME_PATHS.find((path) => existsSync(path)) || CHROME_PATHS[0]!;
const envPort = Number(process.env.MOCK_WINDOW_CDP_PORT);
const CDP_PORT = Number.isInteger(envPort) && envPort > 0 ? envPort : 9223;
const PORT_FROM = 1421; // 1420 belongs to `bun tauri dev` and its strict port
const profile = process.env.MOCK_WINDOW_PROFILE || join(tmpdir(), "rustcode-mock-window");
const cdp = `http://127.0.0.1:${CDP_PORT}`;
const NOT_UP = "No mock window is open. Run `bun run mock-window open` first.";
const CDP_TIMEOUT = 30_000; // a wedged renderer must not hang the caller

/** Keys `press` can send: the ones a check needs to move focus and submit. */
const KEYS: Record<string, { code: string; vk: number; text?: string }> = {
  Enter: { code: "Enter", vk: 13, text: "\r" },
  Escape: { code: "Escape", vk: 27 },
  Tab: { code: "Tab", vk: 9 },
  Backspace: { code: "Backspace", vk: 8 },
  Delete: { code: "Delete", vk: 46 },
  Space: { code: "Space", vk: 32, text: " " },
  ArrowUp: { code: "ArrowUp", vk: 38 },
  ArrowDown: { code: "ArrowDown", vk: 40 },
  ArrowLeft: { code: "ArrowLeft", vk: 37 },
  ArrowRight: { code: "ArrowRight", vk: 39 },
  Home: { code: "Home", vk: 36 },
  End: { code: "End", vk: 35 },
};

/** A hover card needs the pointer to rest on its trigger: 500 ms ones have opened by then. */
const HOVER_WAIT = 700;
/** Movements a drag makes between press and release, so the app sees a drag, not a teleport. */
const DRAG_STEPS = 12;

const USAGE =
  'Usage: bun run mock-window <open [path] [--show] | eval "<js>" | click "<css>" [--at <x>,<y>] | ' +
  'dblclick "<css>" [--at <x>,<y>] | hover "<css>" [--at <x>,<y>] [--wait <ms>] | ' +
  'drag "<from css>" "<to css>" [--from <x>,<y>] [--to <x>,<y>] [--steps <n>] | ' +
  'scroll "<css>" <dy> | move <x>,<y> | type "<text>" | ' +
  "press <key> [--shift] [--ctrl] [--alt] [--meta] | " +
  'screenshot <file> [--selector "<css>" | --clip <x>,<y>,<w>,<h>] | close>';

type Point = { x: number; y: number };
type Flags = Map<string, string | true>;
type Server = { pid: number; port: number; tabId?: string };
type Target = { id: string; type: string; webSocketDebuggerUrl?: string };
// biome-ignore lint/suspicious/noExplicitAny: DevTools protocol results are untyped here
type Cdp = any;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function waitFor(test: () => Promise<boolean>, ms: number, every = 200) {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    if (await test()) return true;
    await sleep(every);
  }
  return false;
}

async function answers(url: string, ms = 1000) {
  try {
    return (await fetch(url, { signal: AbortSignal.timeout(ms) })).ok;
  } catch {
    return false;
  }
}

const alive = (pid: number) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

const sh = (cmd: string[]) => Bun.spawnSync(cmd).stdout.toString();
const commandOf = (pid: number) => sh(["ps", "-o", "command=", "-p", String(pid)]).trim();
const real = (path: string) => {
  try {
    return realpathSync(path);
  } catch {
    return path;
  }
};
/** Where a process runs: a reused pid may belong to another checkout, or program. */
const cwdOf = (pid: number) =>
  real(
    sh(["lsof", "-a", "-p", String(pid), "-d", "cwd", "-Fn"])
      .split("\n")
      .find((line) => line.startsWith("n"))
      ?.slice(1) ?? "",
  );
const here = real(root);
const checkout = createHash("sha256").update(here).digest("hex").slice(0, 16);
const record = `${profile}-${CDP_PORT}-${checkout}.json`;
const serverLog = `${record}.log`;
const listener = () => Number(sh(["lsof", "-nP", `-iTCP:${CDP_PORT}`, "-sTCP:LISTEN", "-t"]).trim()) || 0;
/** 9223 is a shared port, so a Chrome there is this helper's only by its profile. */
const ourChrome = (pid: number) => pid > 0 && commandOf(pid).includes(`--user-data-dir=${profile}`);

/** Leaves the process running after this one exits, in its own process group. */
function detach(cmd: string[], env: Record<string, string> = {}, output: number | "ignore" = "ignore") {
  const child = spawn(cmd[0]!, cmd.slice(1), {
    cwd: root,
    detached: true,
    stdio: ["ignore", output, output],
    env: { ...process.env, ...env },
  });
  child.unref();
  return child.pid ?? 0;
}

async function stop(pid: number) {
  for (const signal of ["SIGTERM", "SIGKILL"] as const) {
    if (!alive(pid)) return;
    for (const target of [-pid, pid]) {
      try {
        process.kill(target, signal); // detached: the pid leads its own group
        break;
      } catch {}
    }
    await waitFor(async () => !alive(pid), 2500, 100);
  }
}

const readRecord = async () => {
  try {
    return (await Bun.file(record).json()) as Server;
  } catch {
    return null;
  }
};

/** Only ever stops the Vite process started from this checkout. */
async function stopMockServer(saved: Server | null) {
  if (saved && ownsServer(saved.pid)) await stop(saved.pid);
  rmSync(record, { force: true });
  rmSync(serverLog, { force: true });
}

const vite = join(root, "node_modules/.bin/vite");
const ownsServer = (pid: number) => alive(pid) && cwdOf(pid) === here && commandOf(pid).includes(vite);

async function closeTab(saved: Server | null) {
  if (!saved?.tabId || !ourChrome(listener())) return;
  const response = await fetch(`${cdp}/json/close/${saved.tabId}`, {
    signal: AbortSignal.timeout(CDP_TIMEOUT),
  });
  if (!response.ok && response.status !== 404)
    throw new Error(`Chrome could not close the mock tab (HTTP ${response.status})`);
}

async function saveRecord(server: Server) {
  await Bun.write(record, JSON.stringify(server));
}

async function ensureServer() {
  const saved = await readRecord();
  if (saved && ownsServer(saved.pid) && (await answers(serverUrl(saved)))) return saved;

  await closeTab(saved);
  await stopMockServer(saved);
  // Vite selects an available port and reports the one it actually bound.
  const log = openSync(serverLog, "w", 0o600);
  let pid: number;
  try {
    pid = detach(
      [vite, "--host", "127.0.0.1", "--port", String(PORT_FROM), "--no-strictPort"],
      {
        VITE_TRANSPORT: "mock",
        NO_COLOR: "1",
      },
      log,
    );
  } finally {
    closeSync(log);
  }
  let port = 0;
  try {
    const up = await waitFor(async () => {
      const output = await Bun.file(serverLog).text();
      if (!alive(pid)) throw new Error(output.trim() || "The mock dev server exited.");
      port = Number(output.match(/Local:\s+http:\/\/127\.0\.0\.1:(\d+)\//)?.[1]);
      return port > 0 && (await answers(serverUrl({ port })));
    }, 60_000);
    if (!up) throw new Error("The mock dev server did not start within 60 seconds.");
    const server: Server = { pid, port };
    await saveRecord(server);
    return server;
  } catch (e) {
    await stop(pid);
    rmSync(serverLog, { force: true });
    throw e;
  }
}

const serverUrl = ({ port }: Pick<Server, "port">) => `http://127.0.0.1:${port}/`;

/** Reuses the window on 9223 if it is up, so there is only ever one. */
async function ensureChrome(show: boolean) {
  if (await answers(`${cdp}/json/version`)) {
    if (ourChrome(listener())) return;
    throw new Error(
      `Another Chrome is listening on ${CDP_PORT}, not the mock window's profile (${profile}). Stop it, then run open again.`,
    );
  }
  if (!existsSync(chromePath))
    throw new Error(`Google Chrome is needed for bun run mock-window: ${chromePath}`);
  const flags = [
    "--no-first-run",
    "--no-default-browser-check",
    `--remote-debugging-port=${CDP_PORT}`,
    `--user-data-dir=${profile}`,
    "--window-size=1512,945",
    // Headless has no display to take a scale from; pin the retina one a visible window gets.
    ...(show ? [] : ["--headless=new", "--force-device-scale-factor=2", "--hide-scrollbars"]),
    // Headless on Linux reports no hover and no fine pointer, so `group-hover` and every
    // `@media (hover: hover)` rule never apply: report the mouse a desktop has.
    "--blink-settings=primaryHoverType=2,availableHoverTypes=2,primaryPointerType=4,availablePointerTypes=4",
    "about:blank",
  ];
  detach([chromePath, ...flags]);
  if (!(await waitFor(() => answers(`${cdp}/json/version`, 500), 30_000)) || !ourChrome(listener()))
    throw new Error("Chrome opened but its DevTools port never answered");
}

async function tabs() {
  if (!(await answers(`${cdp}/json/version`))) throw new Error(NOT_UP);
  return ((await (await fetch(`${cdp}/json`)).json()) as Target[]).filter((t) => t.type === "page");
}

async function connect(url: string) {
  const ws = new WebSocket(url);
  const opened = await Promise.race([
    new Promise<boolean>((ok) => {
      ws.onopen = () => ok(true);
      ws.onerror = () => ok(false);
    }),
    sleep(10_000).then(() => false),
  ]);
  if (!opened) {
    ws.close();
    throw new Error(`No DevTools connection to ${url} in 10 s. Run \`bun run mock-window open\` again.`);
  }
  const waiting = new Map<number, { ok: (v: Cdp) => void; fail: (e: Error) => void }>();
  let id = 0;
  ws.onmessage = (m) => {
    const d = JSON.parse(String(m.data));
    const p = d.id ? waiting.get(d.id) : undefined;
    if (!p) return;
    waiting.delete(d.id);
    if (d.error) p.fail(new Error(`${d.method ?? "CDP"} failed: ${JSON.stringify(d.error)}`));
    else p.ok(d.result);
  };
  const send = (method: string, params: object = {}) => {
    const n = ++id;
    const reply = new Promise<Cdp>((ok, fail) => {
      waiting.set(n, { ok, fail });
      ws.send(JSON.stringify({ id: n, method, params }));
    });
    return Promise.race([
      reply,
      sleep(CDP_TIMEOUT).then(() => {
        waiting.delete(n);
        throw new Error(`${method} did not answer in ${CDP_TIMEOUT / 1000} s.`);
      }),
    ]);
  };
  return { send, close: () => ws.close() };
}

/** Connects exclusively to the recorded target, even if it navigates away. */
async function withPage<T>(
  fn: (send: (method: string, params?: object) => Promise<Cdp>) => Promise<T>,
  saved = readRecord(),
) {
  const tabId = (await saved)?.tabId;
  if (!tabId || !ourChrome(listener())) throw new Error(NOT_UP);
  const page = (await tabs()).find((target) => target.id === tabId);
  if (!page?.webSocketDebuggerUrl) throw new Error(NOT_UP);
  const client = await connect(page.webSocketDebuggerUrl);
  try {
    return await fn(client.send);
  } finally {
    client.close();
  }
}

/** Real input needs focus: Chrome queues mouse and key events for a window that
 * is behind others, so a page that is not frontmost never receives them. */
async function withInput<T>(fn: (send: (method: string, params?: object) => Promise<Cdp>) => Promise<T>) {
  return withPage(async (send) => {
    await send("Emulation.setFocusEmulationEnabled", { enabled: true });
    return fn(send);
  });
}

const evaluate = (send: (method: string, params?: object) => Promise<Cdp>, expression: string) =>
  send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });

/** Unwraps a page result, failing with the page's own first error line. */
function pageValue(r: Cdp) {
  if (!r.exceptionDetails) return r.result?.value;
  const text = r.exceptionDetails.exception?.description ?? JSON.stringify(r.exceptionDetails);
  throw new Error(
    String(text)
      .replace(/^Error: /, "")
      .split("\n")[0],
  );
}

/** CDP's `clip` is in document coordinates, so a viewport rectangle adds the page scroll. */
async function viewportClip(send: (method: string, params?: object) => Promise<Cdp>, text: string) {
  const [x, y, width, height] = text.split(",").map(Number);
  if (![x, y, width, height].every(Number.isFinite) || !(width! > 0) || !(height! > 0))
    throw new Error(`--clip needs <x>,<y>,<w>,<h> in viewport pixels, got "${text}"`);
  const scroll = pageValue(await evaluate(send, "({ x: window.scrollX, y: window.scrollY })")) as Point;
  return { x: x! + scroll.x, y: y! + scroll.y, width: width!, height: height! };
}

/** The first element matching `selector`, scrolled into view, as a document-coordinate clip. */
async function elementClip(send: (method: string, params?: object) => Promise<Cdp>, selector: string) {
  const expression = `(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) throw new Error("Nothing matches the selector " + ${JSON.stringify(selector)});
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
    const b = el.getBoundingClientRect();
    if (b.width <= 0 || b.height <= 0) throw new Error("The element " + ${JSON.stringify(selector)} + " has no size");
    return { x: b.x + window.scrollX, y: b.y + window.scrollY, width: b.width, height: b.height };
  })()`;
  return pageValue(await evaluate(send, expression)) as Point & { width: number; height: number };
}

/** `document.activeElement` as `tag [aria-label]`, printed after every input. */
async function focused(send: (method: string, params?: object) => Promise<Cdp>) {
  const expression = `(() => {
    const a = document.activeElement;
    if (!a) return "none";
    const label = a.getAttribute("aria-label");
    return a.tagName.toLowerCase() + (label ? " [" + label + "]" : "");
  })()`;
  return pageValue(await evaluate(send, expression)) ?? "none";
}

/** Flags that take the word after them; every other `--flag` is a switch. */
const VALUED = new Set(["--at", "--from", "--to", "--wait", "--steps", "--selector", "--clip"]);

/** `<x>,<y>`, taken from the element's top-left as `--at`, `--from` and `--to` do. */
function parsePoint(text: string, flag: string): Point {
  const [x, y] = text.split(",").map(Number);
  if (!Number.isFinite(x) || !Number.isFinite(y))
    throw new Error(`${flag} needs <x>,<y> from the element's top-left, got "${text}"`);
  return { x: x!, y: y! };
}

/** `--at`, `--from` or `--to` when given, else the element's centre. */
function offsetOf(flags: Flags, flag: string) {
  const value = flags.get(flag);
  return value === undefined ? null : parsePoint(String(value), flag);
}

/** A counted option such as `--wait <ms>` or `--steps <n>`, else its default. */
function countOf(flags: Flags, flag: string, fallback: number) {
  const value = flags.get(flag);
  if (value === undefined) return fallback;
  const n = Number(value);
  if (!Number.isInteger(n) || n < 0) throw new Error(`${flag} needs a whole number, got "${String(value)}"`);
  return n;
}

/** Splits `--flag` options from the words before them, so a selector may hold spaces. */
function parseArgs(args: string[]) {
  const list: string[] = [];
  const flags: Flags = new Map();
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (!arg.startsWith("--")) list.push(arg);
    else if (VALUED.has(arg)) flags.set(arg, args[++i] ?? "");
    else flags.set(arg, true);
  }
  const modifiers =
    (flags.has("--alt") ? 1 : 0) |
    (flags.has("--ctrl") ? 2 : 0) |
    (flags.has("--meta") ? 4 : 0) |
    (flags.has("--shift") ? 8 : 0);
  return { words: list.join(" "), list, flags, modifiers };
}

const modifierNames = (m: number) =>
  [m & 1 ? "Alt+" : "", m & 2 ? "Ctrl+" : "", m & 4 ? "Meta+" : "", m & 8 ? "Shift+" : ""].join("");

/** Page-side helper naming an element for output: `<div role=tooltip aria-label="x">`. */
const NAME = `(e) =>
  e
    ? "<" + e.tagName.toLowerCase() +
      (e.getAttribute("role") ? " role=" + e.getAttribute("role") : "") +
      (e.getAttribute("aria-label") ? ' aria-label="' + e.getAttribute("aria-label") + '"' : "") + ">"
    : "nothing"`;

type Located = Point & { self: string; under: string; covers: boolean };

/** Where a command acts: the element's centre or `offset` from its top-left, and
 * what sits there. `scroll` centres the element first, which moves the page. */
async function locate(
  send: (method: string, params?: object) => Promise<Cdp>,
  selector: string,
  offset: Point | null,
  scroll = true,
): Promise<Located> {
  const page = `(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) throw new Error("Nothing matches the selector " + ${JSON.stringify(selector)});
    ${scroll ? `el.scrollIntoView({ block: "center", inline: "center" });` : ""}
    const b = el.getBoundingClientRect();
    const x = b.x + ${offset ? offset.x : "b.width / 2"};
    const y = b.y + ${offset ? offset.y : "b.height / 2"};
    const name = ${NAME};
    const hit = document.elementFromPoint(x, y);
    return {
      x, y,
      self: name(el),
      under: name(hit),
      covers: hit === el || (hit ? el.contains(hit) : false),
    };
  })()`;
  return pageValue(await evaluate(send, page)) as Located;
}

/** A press has to land on its element: a covered one is pressed instead, or nothing happens. */
async function pressPoint(
  send: (method: string, params?: object) => Promise<Cdp>,
  selector: string,
  offset: Point | null,
  flag: string,
) {
  const at = await locate(send, selector, offset);
  if (!at.covers)
    throw new Error(
      `A press at ${Math.round(at.x)},${Math.round(at.y)} would land on ${at.under}, not ${at.self}` +
        (offset ? "" : ` — pass ${flag} <x>,<y> to press a visible part of it`),
    );
  return at;
}

/** Clicks counted from one: a double click is the second press of a pair. */
async function clickCountAt(
  send: (method: string, params?: object) => Promise<Cdp>,
  at: Point,
  counts: number[],
) {
  for (const clickCount of counts) {
    await send("Input.dispatchMouseEvent", {
      type: "mousePressed",
      x: at.x,
      y: at.y,
      button: "left",
      buttons: 1,
      clickCount,
    });
    await send("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      x: at.x,
      y: at.y,
      button: "left",
      buttons: 0,
      clickCount,
    });
  }
}

/** Scrolls a selector into view, checks the press would hit it, then presses it. */
async function click(selector: string, offset: Point | null) {
  await withInput(async (send) => {
    const { x, y } = await pressPoint(send, selector, offset, "--at");
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await sleep(250); // long enough for someone watching to see the pointer land
    await clickCountAt(send, { x, y }, [1]);
    await sleep(300);
    console.log(`Clicked ${selector} at ${Math.round(x)},${Math.round(y)}; focus is ${await focused(send)}.`);
  });
}

/** Two presses at the same point: the second one makes the `dblclick`. */
async function dblclick(selector: string, offset: Point | null) {
  await withInput(async (send) => {
    const { x, y } = await pressPoint(send, selector, offset, "--at");
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await sleep(250);
    await clickCountAt(send, { x, y }, [1, 2]);
    await sleep(300);
    console.log(
      `Double-clicked ${selector} at ${Math.round(x)},${Math.round(y)}; focus is ${await focused(send)}.`,
    );
  });
}

/** Anything a hover opened: a tooltip, a Radix popper or an open hover card, named
 * with its first words. The innermost wins, so a popper wrapper does not repeat
 * the tooltip it holds. */
const OPEN_POPOVER = `(() => {
  const name = ${NAME};
  const shown = (el) => {
    const b = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return b.width > 0 && b.height > 0 && style.display !== "none" && style.visibility !== "hidden";
  };
  const describe = (el) => {
    const text = (el.textContent || "").trim().replace(/\\s+/g, " ");
    return name(el) + (text ? ' "' + text.slice(0, 60) + '"' : "");
  };
  const found = [
    ...document.querySelectorAll(
      '[role=tooltip], [data-radix-popper-content-wrapper], [data-state=open]',
    ),
  ].filter(shown);
  return found
    .filter((el) => !found.some((other) => other !== el && el.contains(other)))
    .slice(0, 3)
    .map(describe)
    .join(", ");
})()`;

/** Rests the pointer on an element and reports what it opened. */
async function hover(selector: string, offset: Point | null, wait: number) {
  await withInput(async (send) => {
    const { x, y, under } = await locate(send, selector, offset);
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await sleep(wait);
    const popover = pageValue(await evaluate(send, OPEN_POPOVER)) as string;
    console.log(
      `Hovered ${selector} at ${Math.round(x)},${Math.round(y)} for ${wait} ms; under the pointer is ` +
        `${under}; ${popover ? `opened ${popover}.` : "no tooltip or popover opened."}`,
    );
  });
}

/** Moves the pointer without pressing, e.g. off an element to dismiss its hover card. */
async function move(point: Point) {
  await withInput(async (send) => {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: point.x, y: point.y });
    await sleep(300);
    const under = pageValue(
      await evaluate(send, `(${NAME})(document.elementFromPoint(${point.x}, ${point.y}))`),
    );
    console.log(`Moved the pointer to ${Math.round(point.x)},${Math.round(point.y)}; under it is ${under}.`);
  });
}

/** Presses on one element, moves in `steps` to another and releases. */
async function drag(
  from: string,
  to: string,
  fromOffset: Point | null,
  toOffset: Point | null,
  steps: number,
) {
  await withInput(async (send) => {
    const start = await pressPoint(send, from, fromOffset, "--from");
    // Without scrolling again: centring the target would move the source out from under.
    const end = await locate(send, to, toOffset, false);
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: start.x, y: start.y });
    await sleep(250);
    await send("Input.dispatchMouseEvent", {
      type: "mousePressed",
      x: start.x,
      y: start.y,
      button: "left",
      buttons: 1,
      clickCount: 1,
    });
    for (let step = 1; step <= steps; step++) {
      const t = step / steps;
      await send("Input.dispatchMouseEvent", {
        type: "mouseMoved",
        x: start.x + (end.x - start.x) * t,
        y: start.y + (end.y - start.y) * t,
        button: "left",
        buttons: 1,
      });
      await sleep(20); // one frame apart, so a drag over a list keeps up
    }
    await send("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      x: end.x,
      y: end.y,
      button: "left",
      buttons: 0,
      clickCount: 1,
    });
    await sleep(400);
    console.log(
      `Dragged ${from} at ${Math.round(start.x)},${Math.round(start.y)} to ${to} at ` +
        `${Math.round(end.x)},${Math.round(end.y)} in ${steps} steps, over ${end.under}.`,
    );
  });
}

/** `scrollTop` of an element and each ancestor, before a wheel turns. */
const scrollTops = (selector: string) => `(() => {
  const el = document.querySelector(${JSON.stringify(selector)});
  if (!el) throw new Error("Nothing matches the selector " + ${JSON.stringify(selector)});
  const tops = [];
  for (let s = el; s; s = s.parentElement) tops.push(Math.round(s.scrollTop));
  return tops;
})()`;

/** Which of those moved, then how far it can go: the wheel goes to the nearest
 * scroller under the pointer, or up to the page when none of them can. */
const scrolledTo = (selector: string, before: number[]) => `(() => {
  const el = document.querySelector(${JSON.stringify(selector)});
  const before = ${JSON.stringify(before)};
  let depth = 0;
  for (let s = el; s; s = s.parentElement, depth++) {
    const now = Math.round(s.scrollTop);
    if (now !== before[depth])
      return "the scroller moved to " + now + " of " + Math.round(s.scrollHeight - s.clientHeight);
  }
  return "nothing under it scrolled";
})()`;

/** Turns the wheel over an element: downward for a positive `<dy>`. */
async function scroll(selector: string, dy: number) {
  await withInput(async (send) => {
    const { x, y, under } = await locate(send, selector, null);
    const before = pageValue(await evaluate(send, scrollTops(selector))) as number[];
    await send("Input.dispatchMouseEvent", {
      type: "mouseWheel",
      x,
      y,
      deltaX: 0,
      deltaY: dy,
    });
    await sleep(400); // smooth scrolling settles within this
    const state = pageValue(await evaluate(send, scrolledTo(selector, before))) as string;
    console.log(
      `Scrolled ${selector} by ${dy} at ${Math.round(x)},${Math.round(y)} over ${under}; ${state}.`,
    );
  });
}

async function type(text: string) {
  await withInput(async (send) => {
    await send("Input.insertText", { text });
    await sleep(300);
    console.log(`Typed ${JSON.stringify(text)}; focus is ${await focused(send)}.`);
  });
}

/** A letter as a real key press, so a page's keydown sees it before its text lands. */
function letterKey(name: string) {
  if (!/^[a-z]$/i.test(name)) return undefined;
  const upper = name.toUpperCase();
  return { code: `Key${upper}`, vk: upper.charCodeAt(0), text: name };
}

async function press(name: string, modifiers: number) {
  const key = KEYS[name] ?? letterKey(name);
  if (!key)
    throw new Error(`press does not know ${name}. Use a letter or one of ${Object.keys(KEYS).join(", ")}.`);
  await withInput(async (send) => {
    const base = {
      key: name,
      code: key.code,
      windowsVirtualKeyCode: key.vk,
      nativeVirtualKeyCode: key.vk,
      modifiers,
    };
    // Enter inserts a newline only with its text, as in a real key press.
    await send("Input.dispatchKeyEvent", {
      type: "keyDown",
      ...base,
      text: key.text,
      unmodifiedText: key.text,
    });
    await send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
    await sleep(300);
    console.log(`Pressed ${modifierNames(modifiers)}${name}; focus is ${await focused(send)}.`);
  });
}

async function main() {
  const [command, ...rest] = process.argv.slice(2);
  if (command === "open") {
    ensureDeps();
    const { words, flags } = parseArgs(rest);
    await ensureChrome(flags.has("--show"));
    const server = await ensureServer();
    const base = serverUrl(server);
    const url = new URL(words, base).href;
    if (!(await tabs()).some((target) => target.id === server.tabId)) {
      const response = await fetch(`${cdp}/json/new?about:blank`, {
        method: "PUT",
        signal: AbortSignal.timeout(CDP_TIMEOUT),
      });
      if (!response.ok) throw new Error(`Chrome could not open a mock tab (HTTP ${response.status})`);
      const { id } = (await response.json()) as Target;
      server.tabId = id;
      await saveRecord(server);
    }
    await withPage(async (send) => {
      await send("Page.enable");
      await send("Page.navigate", { url });
      if (flags.has("--show")) await send("Page.bringToFront");
      await waitFor(async () => {
        const state = await evaluate(send, "document.readyState").catch(() => null);
        return state?.result?.value === "complete";
      }, 15_000);
    }, Promise.resolve(server));
    console.log(`Open ${url} (server ${base})`);
  } else if (command === "close") {
    const saved = await readRecord();
    await closeTab(saved);
    await stopMockServer(saved);
    console.log("Closed this checkout's mock tab and dev server.");
  } else if (command === "eval") {
    const expression = rest.join(" ");
    if (!expression) throw new Error('Usage: bun run mock-window eval "location.href"');
    // REPL mode allows top-level `await`, so `await import("/src/state/store.ts")` works as written.
    const r = await withPage((send) =>
      send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, replMode: true }),
    );
    if (r.exceptionDetails) {
      console.error(r.exceptionDetails.exception?.description ?? JSON.stringify(r.exceptionDetails));
      process.exit(1);
    }
    console.log(JSON.stringify(r.result?.value, null, 2));
  } else if (command === "click") {
    const { words, flags } = parseArgs(rest);
    if (!words) throw new Error('Usage: bun run mock-window click "<css selector>" [--at <x>,<y>]');
    await click(words, offsetOf(flags, "--at"));
  } else if (command === "dblclick") {
    const { words, flags } = parseArgs(rest);
    if (!words) throw new Error('Usage: bun run mock-window dblclick "<css selector>" [--at <x>,<y>]');
    await dblclick(words, offsetOf(flags, "--at"));
  } else if (command === "hover") {
    const { words, flags } = parseArgs(rest);
    if (!words)
      throw new Error('Usage: bun run mock-window hover "<css selector>" [--at <x>,<y>] [--wait <ms>]');
    await hover(words, offsetOf(flags, "--at"), countOf(flags, "--wait", HOVER_WAIT));
  } else if (command === "drag") {
    const { list, flags } = parseArgs(rest);
    if (list.length !== 2)
      throw new Error(
        'Usage: bun run mock-window drag "<from css>" "<to css>" ' +
          "[--from <x>,<y>] [--to <x>,<y>] [--steps <n>]",
      );
    await drag(
      list[0]!,
      list[1]!,
      offsetOf(flags, "--from"),
      offsetOf(flags, "--to"),
      countOf(flags, "--steps", DRAG_STEPS),
    );
  } else if (command === "scroll") {
    const { list } = parseArgs(rest);
    const dy = Number(list.at(-1));
    // The selector may hold spaces, so only the last word is the distance.
    const selector = list.slice(0, -1).join(" ");
    if (list.length < 2 || !selector || !Number.isFinite(dy))
      throw new Error('Usage: bun run mock-window scroll "<css selector>" <dy>');
    await scroll(selector, dy);
  } else if (command === "move") {
    const text = rest.join(" ");
    if (!text) throw new Error("Usage: bun run mock-window move <x>,<y>");
    await move(parsePoint(text, "move"));
  } else if (command === "type") {
    const text = rest.join(" ");
    if (!text) throw new Error('Usage: bun run mock-window type "<text>"');
    await type(text);
  } else if (command === "press") {
    const { words, modifiers } = parseArgs(rest);
    if (!words) throw new Error("Usage: bun run mock-window press <key> [--shift] [--ctrl] [--alt] [--meta]");
    await press(words, modifiers);
  } else if (command === "screenshot") {
    const { list, flags } = parseArgs(rest);
    const file = list[0];
    const selector = flags.get("--selector");
    const clipText = flags.get("--clip");
    if (!file || list.length > 1 || selector === true || clipText === true || (selector && clipText))
      throw new Error(
        'Usage: bun run mock-window screenshot <file.png> [--selector "<css>" | --clip <x>,<y>,<w>,<h>]',
      );
    const data = await withPage(async (send) => {
      await send("Page.enable");
      const clip = selector
        ? await elementClip(send, selector)
        : clipText
          ? await viewportClip(send, clipText)
          : null;
      const params = clip ? { format: "png", clip: { ...clip, scale: 1 } } : { format: "png" };
      return (await send("Page.captureScreenshot", params)).data as string;
    });
    await Bun.write(file, Buffer.from(data, "base64"));
    console.log(`Saved ${resolve(file)}`);
  } else {
    throw new Error(USAGE);
  }
}

try {
  await main();
  process.exit(0);
} catch (e) {
  console.error(e instanceof Error ? e.message : String(e));
  process.exit(1);
}
