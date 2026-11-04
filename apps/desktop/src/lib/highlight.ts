// Syntax highlighting with Shiki, in a worker (lib/highlight.worker.ts): the
// highlighter and each language load there on first use, so neither the
// grammar compile nor a long block ever runs on the main thread. Highlighted
// HTML is cached here, so a remounted block draws its colours in its first frame.

import type { HighlightRequest, HighlightResponse } from "./highlight.worker";

const ALIASES: Record<string, string> = {
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  console: "bash",
  js: "javascript",
  ts: "typescript",
  rs: "rust",
  py: "python",
  yml: "yaml",
  md: "markdown",
  objc: "objective-c",
  "c++": "cpp",
  txt: "text",
  plaintext: "text",
};

const failed = new Set<string>();

let worker: Worker | null | undefined;
let nextId = 0;
const pending = new Map<number, (html: string | null) => void>();

/** The highlighter worker; null where there is none (then code stays plain). */
function getWorker(): Worker | null {
  if (worker !== undefined) return worker;
  try {
    worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (event: MessageEvent<HighlightResponse>) => {
      pending.get(event.data.id)?.(event.data.html);
      pending.delete(event.data.id);
    };
    worker.onerror = () => {
      // A worker that failed to start or crashed: everything waiting, and later, renders plain.
      worker?.terminate();
      worker = null;
      for (const done of pending.values()) done(null);
      pending.clear();
    };
  } catch {
    worker = null;
  }
  return worker;
}

function render(code: string, language: string): Promise<string | null> {
  const w = getWorker();
  if (!w) return Promise.resolve(null);
  const id = ++nextId;
  return new Promise((resolve) => {
    pending.set(id, resolve);
    w.postMessage({ id, code, language } satisfies HighlightRequest);
  });
}

export function normalizeLang(lang: string | undefined): string {
  const l = (lang ?? "").toLowerCase().trim();
  return ALIASES[l] ?? (l || "text");
}

// Highlighted HTML by language and code. A code block that scrolls back into
// view (rows remount as the transcript virtualizes) shows its colours at once,
// with no plain first frame to measure and no second Shiki pass.
const cache = new Map<string, string>();
const CACHE_MAX = 400;

function remember(key: string, html: string) {
  cache.set(key, html);
  if (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value as string);
}

/** Cached HTML for a block, without highlighting it: null when unknown (plain), undefined when not cached. */
export function peekHighlight(code: string, lang: string | undefined): string | null | undefined {
  const language = normalizeLang(lang);
  if (language === "text" || failed.has(language)) return null;
  const key = `${language}\u0000${code}`;
  const hit = cache.get(key);
  if (hit === undefined) return undefined;
  cache.delete(key);
  cache.set(key, hit);
  return hit;
}

/** HTML for a highlighted block, or null when the language is unknown (render plain). */
export async function highlight(code: string, lang: string | undefined): Promise<string | null> {
  const cached = peekHighlight(code, lang);
  if (cached !== undefined) return cached;
  const language = normalizeLang(lang);
  const html = await render(code, language);
  if (html === null) {
    // Unknown to Shiki, or no worker: plain from now on.
    failed.add(language);
    return null;
  }
  remember(`${language}\u0000${code}`, html);
  return html;
}

/** The language a path's extension implies, for diffs. */
const LANG_BY_EXTENSION: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  rs: "rust",
  py: "python",
  swift: "swift",
  go: "go",
  md: "markdown",
  mdx: "mdx",
  json: "json",
  jsonc: "jsonc",
  toml: "toml",
  yml: "yaml",
  yaml: "yaml",
  css: "css",
  scss: "scss",
  less: "less",
  html: "html",
  xml: "xml",
  svg: "xml",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  fish: "fish",
  sql: "sql",
  c: "c",
  h: "c",
  cc: "cpp",
  cpp: "cpp",
  hpp: "cpp",
  cs: "csharp",
  java: "java",
  kt: "kotlin",
  rb: "ruby",
  php: "php",
  lua: "lua",
  vue: "vue",
  svelte: "svelte",
  graphql: "graphql",
  proto: "proto",
  nix: "nix",
  zig: "zig",
  tf: "terraform",
  diff: "diff",
  patch: "diff",
};

const LANG_BY_NAME: Record<string, string> = {
  dockerfile: "dockerfile",
  makefile: "makefile",
  gemfile: "ruby",
  rakefile: "ruby",
};

export function langForPath(path: string): string | undefined {
  const name = path.split("/").pop()?.toLowerCase() ?? "";
  const dot = name.lastIndexOf(".");
  if (dot < 0) return LANG_BY_NAME[name];
  return LANG_BY_EXTENSION[name.slice(dot + 1)] ?? LANG_BY_NAME[name.slice(0, dot)];
}
