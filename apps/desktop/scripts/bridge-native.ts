// Dev-only mirror of the Tauri shell's native commands (`src-tauri/src/*.rs`),
// so `bun run dev:web` can show link chips, thread actions and the terminal.
// Same rules as the shell; the shell's Rust code is the one that ships.

import { lookup } from "node:dns/promises";
import { isIP } from "node:net";

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

export type Native = (command: string, args: Record<string, unknown>) => Promise<unknown>;

export const nativeCommands: Record<string, (args: Record<string, unknown>) => Promise<unknown>> = {
  link_meta: async ({ url }) => {
    if (typeof url !== "string" || !/^https?:\/\//i.test(url) || url.length > 2048)
      throw new Error("Only web links.");
    return linkMeta(url);
  },
};
