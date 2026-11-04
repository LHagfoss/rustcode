// Dev-only mirror of the Tauri shell's native commands (`src-tauri/src/*.rs`),
// so `bun run dev:web` can show link chips, thread actions and the terminal.
// Same rules as the shell; the shell's Rust code is the one that ships.

import { lookup } from "node:dns/promises";
import { existsSync, readFileSync, realpathSync, renameSync, statSync, writeFileSync } from "node:fs";
import { isIP } from "node:net";
import { homedir } from "node:os";
import { basename, isAbsolute, join } from "node:path";
import { resolveEngine } from "./engine-bridge";

const TIMEOUT = 3000;
const MAX_HTML = 512 * 1024;
const MAX_ICON = 64 * 1024;

export function isPublic(ip: string): boolean {
  if (isIP(ip) === 4) {
    const [a = 0, b = 0] = ip.split(".").map(Number);
    return !(
      a === 0 ||
      a === 10 ||
      a === 127 ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b < 32) ||
      (a === 192 && b === 168) ||
      (a === 100 && b >= 64 && b < 128) ||
      a >= 224
    );
  }
  const v6 = ip.toLowerCase();
  if (v6.startsWith("::ffff:")) return isPublic(v6.slice(7));
  return !(v6 === "::1" || v6 === "::" || /^f[cd]/.test(v6) || /^fe[89ab]/.test(v6) || v6.startsWith("ff"));
}

async function get(raw: string, cap: number): Promise<{ url: URL; body: Uint8Array; type: string }> {
  let url = new URL(raw);
  for (let hop = 0; hop <= 3; hop++) {
    if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error("Only web links.");
    if (url.username || url.password) throw new Error("No credentials.");
    const addrs = await lookup(url.hostname.replace(/^\[|\]$/g, ""), { all: true });
    if (!addrs.length || addrs.some((a) => !isPublic(a.address))) throw new Error("Only public hosts.");
    const res = await fetch(url, {
      redirect: "manual",
      signal: AbortSignal.timeout(TIMEOUT),
      headers: { "user-agent": "Mozilla/5.0 (Macintosh) RustCode link preview", accept: "text/html,image/*" },
    });
    if (res.status >= 300 && res.status < 400) {
      const location = res.headers.get("location");
      if (!location) throw new Error("Redirect without location.");
      url = new URL(location, url);
      continue;
    }
    if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
    const chunks: Uint8Array[] = [];
    let size = 0;
    for await (const chunk of res.body) {
      chunks.push(chunk.slice(0, cap - size));
      size += Math.min(chunk.length, cap - size);
      if (size >= cap) break;
    }
    return { url, body: Buffer.concat(chunks), type: res.headers.get("content-type") ?? "" };
  }
  throw new Error("Too many redirects.");
}

function attr(tag: string, name: string): string | undefined {
  const m = new RegExp(`\\s${name}\\s*=\\s*("([^"]*)"|'([^']*)'|([^\\s>]+))`, "i").exec(tag);
  return m ? (m[2] ?? m[3] ?? m[4]) : undefined;
}

const decode = (s: string) =>
  s
    .replace(/&#x([0-9a-f]+);/gi, (_, h) => String.fromCodePoint(parseInt(h, 16)))
    .replace(/&#(\d+);/g, (_, d) => String.fromCodePoint(Number(d)))
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&");

const linkCache = new Map<string, unknown>();

async function linkMeta(url: string) {
  if (linkCache.has(url)) return linkCache.get(url);
  let meta: Record<string, string> | null = null;
  try {
    const page = await get(url, MAX_HTML);
    if (page.type.includes("html") || !page.type) {
      const html = new TextDecoder().decode(page.body);
      const head = html.slice(0, html.toLowerCase().indexOf("</head") >>> 0 || html.length);
      meta = {};
      const title = /<title[^>]*>([\s\S]*?)<\/title/i.exec(head)?.[1];
      if (title) meta.title = decode(title).replace(/\s+/g, " ").trim();
      let icon: string | undefined;
      for (const tag of head.match(/<(meta|link)\b[^>]*>/gi) ?? []) {
        const key = (attr(tag, "property") ?? attr(tag, "name") ?? "").toLowerCase();
        const content = decode(attr(tag, "content") ?? "");
        if (key === "og:title" && content) meta.title = content.trim();
        if (key === "og:site_name" && content) meta.siteName = content.trim();
        if (key === "theme-color" && !meta.color && !/dark/.test(attr(tag, "media") ?? ""))
          meta.color = content;
        const rel = (attr(tag, "rel") ?? "").toLowerCase();
        if (/\bicon\b/.test(rel) && (!icon || /png/.test(tag))) icon = attr(tag, "href");
      }
      try {
        const ico = await get(new URL(icon ?? "/favicon.ico", page.url).toString(), MAX_ICON);
        if (ico.body.length < MAX_ICON && ico.type.startsWith("image/")) {
          meta.icon = `data:${ico.type.split(";")[0]};base64,${Buffer.from(ico.body).toString("base64")}`;
        }
      } catch {
        // No icon.
      }
    }
  } catch {
    meta = null;
  }
  linkCache.set(url, meta);
  return meta;
}

/** Thread `id`'s folder from `rustcode threads path`, only when it is that thread's folder in the store. */
function threadDir(id: unknown): string {
  if (typeof id !== "string" || !/^[\w-]{1,128}$/.test(id)) throw new Error("Not a thread id.");
  const out = Bun.spawnSync([resolveEngine(), "threads", "path", id]);
  const dir = out.stdout.toString().trim();
  if (
    out.exitCode !== 0 ||
    !dir.startsWith("/") ||
    dir.split("/").pop() !== id ||
    dir.includes("/../") ||
    !/\/rustcode\/(threads|sessions)\//.test(dir) ||
    !statSync(dir, { throwIfNoEntry: false })?.isDirectory()
  ) {
    throw new Error("Could not find this thread's folder.");
  }
  return dir;
}

interface BridgeTerminal {
  write(data: string): void;
  resize(cols: number, rows: number): void;
}
type Sender = { send(text: string): void };
const ptys = new Map<number, { proc: ReturnType<typeof Bun.spawn>; terminal: BridgeTerminal }>();
let nextPty = 1;
const clamp = (n: unknown, lo: number, hi: number) => Math.min(hi, Math.max(lo, Number(n) || lo));

/** Kill a socket's terminals when it closes. */
export function closePtys() {
  for (const { proc } of ptys.values()) proc.kill();
  ptys.clear();
}

export type Native = (command: string, args: Record<string, unknown>) => Promise<unknown>;

export const nativeCommands: Record<string, (args: Record<string, unknown>) => Promise<unknown>> = {
  link_meta: async ({ url }) => {
    if (typeof url !== "string" || !/^https?:\/\//i.test(url) || url.length > 2048)
      throw new Error("Only web links.");
    return linkMeta(url);
  },
  project_files: async ({ cwd }) => {
    if (typeof cwd !== "string" || !isAbsolute(cwd)) throw new Error("Not a folder.");
    const out = Bun.spawnSync([
      "git",
      "-C",
      cwd,
      "-c",
      "core.quotepath=off",
      "ls-files",
      "-co",
      "--exclude-standard",
    ]);
    if (!out.success) throw new Error("Not a git repository.");
    return out.stdout.toString().split("\n").filter(Boolean).slice(0, 20_000);
  },
  pty_open: async ({ cwd, cols, rows, __socket }) => {
    if (
      typeof cwd !== "string" ||
      !cwd.startsWith("/") ||
      !statSync(cwd, { throwIfNoEntry: false })?.isDirectory()
    ) {
      throw new Error("The terminal needs an existing folder.");
    }
    if (ptys.size >= 8) throw new Error("Too many terminals are open.");
    const id = nextPty++;
    const socket = __socket as Sender;
    const emit = (event: Record<string, unknown>) =>
      socket.send(JSON.stringify({ type: "pty", id, ...event }));
    const decoder = new TextDecoder();
    const proc = Bun.spawn([process.env.SHELL || "/bin/zsh", "-l"], {
      cwd,
      env: { ...process.env, TERM: "xterm-256color", COLORTERM: "truecolor", TERM_PROGRAM: "RustCode" },
      terminal: {
        cols: clamp(cols, 2, 1000),
        rows: clamp(rows, 2, 500),
        data: (_t: unknown, data: Uint8Array) => emit({ data: decoder.decode(data, { stream: true }) }),
      },
    } as never);
    const terminal = (proc as unknown as { terminal: BridgeTerminal }).terminal;
    ptys.set(id, { proc, terminal });
    void proc.exited.then((code) => {
      ptys.delete(id);
      emit({ exit: code });
    });
    return id;
  },
  pty_write: async ({ id, data }) => {
    ptys.get(Number(id))?.terminal.write(String(data));
    return null;
  },
  pty_resize: async ({ id, cols, rows }) => {
    ptys.get(Number(id))?.terminal.resize(clamp(cols, 2, 1000), clamp(rows, 2, 500));
    return null;
  },
  pty_close: async ({ id }) => {
    ptys.get(Number(id))?.proc.kill();
    ptys.delete(Number(id));
    return null;
  },
  thread_rename: async ({ id, title }) => {
    const clean = String(title ?? "")
      .split(/\s+/)
      .join(" ")
      .trim()
      .slice(0, 200);
    if (!clean) throw new Error("A title can't be empty.");
    writeFileSync(join(threadDir(id), "title.txt"), clean);
    return clean;
  },
  // The shell's `thread_image` (native.rs), with the same rules: inside the roots, pictures only, 20 MB.
  thread_image: async ({ path, roots }) => {
    const real = (p: string) => realpathSync(p);
    const dirs = (Array.isArray(roots) ? roots : [])
      .map(String)
      .filter((r) => isAbsolute(r) && existsSync(r))
      .map(real);
    if (!dirs.length) throw new Error("This thread has no folder.");
    const raw = String(path ?? "").replace(/^~\//, `${homedir()}/`);
    const joined = isAbsolute(raw) ? raw : join(dirs[0]!, raw);
    if (!existsSync(joined)) throw new Error("No such picture.");
    const file = real(joined);
    if (!dirs.some((d) => file === d || file.startsWith(`${d}/`)))
      throw new Error("Pictures outside the thread's folder are not shown.");
    const ext = file.split(".").pop()?.toLowerCase() ?? "";
    const mimeType = (
      {
        png: "image/png",
        jpg: "image/jpeg",
        jpeg: "image/jpeg",
        gif: "image/gif",
        webp: "image/webp",
        svg: "image/svg+xml",
      } as Record<string, string>
    )[ext];
    if (!mimeType) throw new Error("Not a picture.");
    const stat = statSync(file);
    if (!stat.isFile()) throw new Error("Not a file.");
    if (stat.size > 20 * 1024 * 1024) throw new Error("Pictures must be 20 MB or smaller.");
    return { name: basename(file), mimeType, data: readFileSync(file).toString("base64") };
  },
  thread_trash: async ({ id }) => {
    const dir = threadDir(id);
    const trash = join(homedir(), ".Trash");
    let target = join(trash, String(id));
    for (let n = 2; existsSync(target); n++) target = join(trash, `${id} ${n}`);
    renameSync(dir, target);
    return null;
  },
};
