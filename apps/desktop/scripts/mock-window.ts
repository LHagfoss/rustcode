// `bun run mock-window <open [path] | eval "<js>" | click "<css>" [--at <x>,<y>] |
//   type "<text>" | press <key> [--shift] [--ctrl] [--alt] [--meta] |
//   screenshot <file.png> | close>`:
// one shared, visible Chrome window on the mock dev server at native scale, so UI
// checks look like the app and agents do not each launch their own browser.
// `open` leaves the server and window running detached; `close` stops only those
// two. `scripts/perf.ts` stays headless: it measures performance, not appearance.
// `MOCK_WINDOW_CDP_PORT` and `MOCK_WINDOW_PROFILE` move this helper to another
// port and profile, so a second thread can drive a window of its own.

import { spawn } from "node:child_process";
import { existsSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = join(import.meta.dir, "..");
const chromePath = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const envPort = Number(process.env.MOCK_WINDOW_CDP_PORT);
const CDP_PORT = Number.isInteger(envPort) && envPort > 0 ? envPort : 9223; // fixed, so every caller finds the same window
const PORT_FROM = 1421; // 1420 belongs to `bun tauri dev` and its strict port
const profile = process.env.MOCK_WINDOW_PROFILE || join(tmpdir(), "rustcode-mock-window");
const record = `${profile}-server.json`; // per profile, so two windows keep their own server
const cdp = `http://127.0.0.1:${CDP_PORT}`;
const NOT_UP = "No mock window is open. Run `bun run mock-window open` first.";
const NO_TAB = "The mock window has no tab. Run `bun run mock-window close`, then `open`.";
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

type Server = { pid: number; cwd: string; port: number };
type Target = { id: string; type: string; url: string; webSocketDebuggerUrl?: string };
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
const listener = () => Number(sh(["lsof", "-nP", `-iTCP:${CDP_PORT}`, "-sTCP:LISTEN", "-t"]).trim()) || 0;
/** 9223 is a shared port, so a Chrome there is this helper's only by its profile. */
const ourChrome = (pid: number) => pid > 0 && commandOf(pid).includes(`--user-data-dir=${profile}`);

/** Leaves the process running after this one exits, in its own process group. */
function detach(cmd: string[], env: Record<string, string> = {}) {
  const child = spawn(cmd[0]!, cmd.slice(1), {
    cwd: root,
    detached: true,
    stdio: "ignore",
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

/** Only ever stops the vite this helper spawned: a reused pid is left alone. */
async function stopMockServer() {
  const saved = await readRecord();
  if (saved && alive(saved.pid)) {
    const spawned = join(saved.cwd, "node_modules/.bin/vite");
    const command = commandOf(saved.pid);
    if (command.includes(spawned) && command.includes(`--port ${saved.port}`)) await stop(saved.pid);
  }
  rmSync(record, { force: true });
}

/** Both loopback families: vite binds `localhost` on ::1, so a server of
 * another checkout on the same port went unnoticed by an IPv4-only probe. */
function freePort() {
  for (let port = PORT_FROM; port < PORT_FROM + 200; port++) {
    try {
      for (const hostname of ["127.0.0.1", "::1"])
        Bun.listen({ hostname, port, socket: { data() {} } }).stop(true);
      return port;
    } catch {}
  }
  throw new Error(`No free port from ${PORT_FROM} up`);
}

/** Reuses this checkout's server; one from another checkout would show other code. */
async function ensureServer() {
  const saved = await readRecord();
  if (saved && alive(saved.pid) && cwdOf(saved.pid) === here) {
    const base = `http://localhost:${saved.port}/`;
    if (await answers(base)) return base;
  }
  await stopMockServer();
  const port = freePort();
  const base = `http://localhost:${port}/`;
  const pid = detach([join(root, "node_modules/.bin/vite"), "--port", String(port), "--strictPort"], {
    VITE_TRANSPORT: "mock",
  });
  const up = await waitFor(async () => {
    if (!alive(pid))
      throw new Error("The mock dev server exited. Run `bun run dev:mock` by hand to see why.");
    return answers(base);
  }, 60_000).catch(async (e) => {
    await stop(pid); // do not leave a half-started server behind
    throw e;
  });
  if (!up) {
    await stop(pid);
    throw new Error(`The mock dev server did not answer at ${base}`);
  }
  await Bun.write(record, `${JSON.stringify({ pid, cwd: root, port } satisfies Server, null, 2)}\n`);
  return base;
}

/** Reuses the window on 9223 if it is up, so there is only ever one. */
async function ensureChrome() {
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
    "about:blank",
  ];
  detach([chromePath, ...flags]);
  if (!(await waitFor(() => answers(`${cdp}/json/version`, 500), 30_000)))
    throw new Error("Chrome opened but its DevTools port never answered");
}

async function tabs() {
  if (!(await answers(`${cdp}/json/version`))) throw new Error(NOT_UP);
  return ((await (await fetch(`${cdp}/json`)).json()) as Target[]).filter((t) => t.type === "page");
}

/** The tab on the server's origin, else the first. */
const pick = (pages: Target[], base: string) => pages.find((p) => p.url.startsWith(base)) ?? pages[0];

/** The recorded server's origin, so its tab is preferred over one on anything else. */
async function origin() {
  const saved = await readRecord();
  return saved ? `http://localhost:${saved.port}/` : "";
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

/** Runs fn against one tab and disconnects; only `open` tidies tabs. */
async function withPage<T>(
  base: string,
  fn: (send: (method: string, params?: object) => Promise<Cdp>) => Promise<T>,
  tidy = false,
) {
  const pages = await tabs();
  const page = pick(pages, base);
  if (!page?.webSocketDebuggerUrl) throw new Error(NO_TAB);
  if (tidy)
    for (const extra of pages)
      if (extra.id !== page.id) await fetch(`${cdp}/json/close/${extra.id}`).catch(() => {});
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
  return withPage(await origin(), async (send) => {
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

/** Splits `--flag` options from the words before them, so a selector may hold spaces. */
function parseArgs(args: string[]) {
  const words: string[] = [];
  const flags = new Set<string>();
  let at: string | undefined;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (!arg.startsWith("--")) words.push(arg);
    else {
      flags.add(arg);
      if (arg === "--at") at = args[++i] ?? "";
    }
  }
  let offset: { x: number; y: number } | null = null;
  if (at !== undefined) {
    const [x, y] = at.split(",").map(Number);
    if (!Number.isFinite(x) || !Number.isFinite(y))
      throw new Error(`--at needs <x>,<y> from the element's top-left, got "${at}"`);
    offset = { x: x!, y: y! };
  }
  const modifiers =
    (flags.has("--alt") ? 1 : 0) |
    (flags.has("--ctrl") ? 2 : 0) |
    (flags.has("--meta") ? 4 : 0) |
    (flags.has("--shift") ? 8 : 0);
  return { words: words.join(" "), offset, modifiers };
}

const modifierNames = (m: number) =>
  [m & 1 ? "Alt+" : "", m & 2 ? "Ctrl+" : "", m & 4 ? "Meta+" : "", m & 8 ? "Shift+" : ""].join("");

/** Scrolls a selector into view, checks the press would hit it, then presses it. */
async function click(selector: string, offset: { x: number; y: number } | null) {
  const hint = offset ? "" : " — pass --at <x>,<y> to press a visible part of it";
  const page = `(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) throw new Error("Nothing matches the selector " + ${JSON.stringify(selector)});
    el.scrollIntoView({ block: "center", inline: "center" });
    const b = el.getBoundingClientRect();
    const x = b.x + ${offset ? offset.x : "b.width / 2"};
    const y = b.y + ${offset ? offset.y : "b.height / 2"};
    const name = (e) => e
      ? "<" + e.tagName.toLowerCase() +
        (e.getAttribute("aria-label") ? ' aria-label="' + e.getAttribute("aria-label") + '"' : "") + ">"
      : "nothing";
    const hit = document.elementFromPoint(x, y);
    if (hit !== el && !(hit && el.contains(hit)))
      throw new Error("A press at " + Math.round(x) + "," + Math.round(y) + " would land on " +
        name(hit) + ", not " + name(el) + ${JSON.stringify(hint)});
    return { x, y };
  })()`;
  await withInput(async (send) => {
    const { x, y } = pageValue(await evaluate(send, page)) as { x: number; y: number };
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await sleep(250); // long enough for someone watching to see the pointer land
    await send("Input.dispatchMouseEvent", {
      type: "mousePressed",
      x,
      y,
      button: "left",
      clickCount: 1,
    });
    await send("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      x,
      y,
      button: "left",
      clickCount: 1,
    });
    await sleep(300);
    console.log(`Clicked ${selector} at ${Math.round(x)},${Math.round(y)}; focus is ${await focused(send)}.`);
  });
}

async function type(text: string) {
  await withInput(async (send) => {
    await send("Input.insertText", { text });
    await sleep(300);
    console.log(`Typed ${JSON.stringify(text)}; focus is ${await focused(send)}.`);
  });
}

async function press(name: string, modifiers: number) {
  const key = KEYS[name];
  if (!key) throw new Error(`press does not know ${name}. Use one of ${Object.keys(KEYS).join(", ")}.`);
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
    const base = await ensureServer();
    await ensureChrome();
    const url = new URL(rest[0] ?? "", base).href;
    await withPage(
      base,
      async (send) => {
        await send("Page.enable");
        await send("Page.navigate", { url });
        await send("Page.bringToFront");
        // Let the app paint, but never block on it.
        await waitFor(async () => {
          const state = await send("Runtime.evaluate", { expression: "document.readyState" }).catch(
            () => null,
          );
          return state?.result?.value === "complete";
        }, 15_000);
      },
      true,
    );
    console.log(`Open ${url} (server ${base})`);
  } else if (command === "close") {
    const chromePid = listener();
    if (chromePid && !ourChrome(chromePid))
      throw new Error(
        `Port ${CDP_PORT} belongs to another Chrome, not the mock window (profile ${profile}). Left it alone.`,
      );
    if (await answers(`${cdp}/json/version`)) {
      const { webSocketDebuggerUrl } = (await (await fetch(`${cdp}/json/version`)).json()) as {
        webSocketDebuggerUrl: string;
      };
      const client = await connect(webSocketDebuggerUrl);
      // The socket dies with the browser, so the reply may never arrive.
      await Promise.race([client.send("Browser.close").catch(() => {}), sleep(3000)]);
      await waitFor(async () => !(await answers(`${cdp}/json/version`, 300)), 10_000, 100);
    }
    await stopMockServer();
    console.log("Closed the mock window and its dev server.");
  } else if (command === "eval") {
    const expression = rest.join(" ");
    if (!expression) throw new Error('Usage: bun run mock-window eval "location.href"');
    const r = await withPage(await origin(), (send) =>
      send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }),
    );
    if (r.exceptionDetails) {
      console.error(r.exceptionDetails.exception?.description ?? JSON.stringify(r.exceptionDetails));
      process.exit(1);
    }
    console.log(JSON.stringify(r.result?.value, null, 2));
  } else if (command === "click") {
    const { words, offset } = parseArgs(rest);
    if (!words) throw new Error('Usage: bun run mock-window click "<css selector>" [--at <x>,<y>]');
    await click(words, offset);
  } else if (command === "type") {
    const text = rest.join(" ");
    if (!text) throw new Error('Usage: bun run mock-window type "<text>"');
    await type(text);
  } else if (command === "press") {
    const { words, modifiers } = parseArgs(rest);
    if (!words) throw new Error("Usage: bun run mock-window press <key> [--shift] [--ctrl] [--alt] [--meta]");
    await press(words, modifiers);
  } else if (command === "screenshot") {
    const file = rest[0];
    if (!file) throw new Error("Usage: bun run mock-window screenshot <file.png>");
    const data = await withPage(await origin(), async (send) => {
      await send("Page.enable");
      return (await send("Page.captureScreenshot", { format: "png" })).data as string;
    });
    await Bun.write(file, Buffer.from(data, "base64"));
    console.log(`Saved ${resolve(file)}`);
  } else {
    throw new Error(
      'Usage: bun run mock-window <open [path] | eval "<js>" | click "<css>" [--at <x>,<y>] | type "<text>" | press <key> [--shift] [--ctrl] [--alt] [--meta] | screenshot <file> | close>',
    );
  }
}

try {
  await main();
  process.exit(0);
} catch (e) {
  console.error(e instanceof Error ? e.message : String(e));
  process.exit(1);
}
