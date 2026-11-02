// design-lint: allow-file (Shiki's syntax palette (Xcode's colours))
// Syntax highlighting with Shiki, loaded lazily: the highlighter and each
// language arrive on first use. Two small themes carry the Xcode colours the
// SwiftUI app uses (CodeStyle.swift); CSS picks light or dark per token.

import type { HighlighterGeneric, ThemeRegistration } from "shiki";

const scopes = (light: string, dark: string, scope: string[]) => ({ light, dark, scope });

const RULES = [
  scopes("#9B2393", "#FC5FA3", [
    "keyword",
    "storage",
    "storage.type",
    "keyword.control",
    "constant.language",
    "variable.language",
  ]),
  scopes("#C41A16", "#FC6A5D", [
    "string",
    "string.quoted",
    "string.template",
    "punctuation.definition.string",
  ]),
  scopes("#5D6C79", "#7F8C98", ["comment", "punctuation.definition.comment"]),
  scopes("#1C00CF", "#D0BF69", ["constant.numeric", "constant.character"]),
  scopes("#0B4F79", "#5DD8FF", [
    "entity.name.type",
    "entity.name.class",
    "support.type",
    "support.class",
    "entity.other.inherited-class",
  ]),
];

function theme(name: string, mode: "light" | "dark"): ThemeRegistration {
  return {
    name,
    type: mode,
    colors: {
      "editor.foreground": mode === "light" ? "#1f1f1f" : "#e6e6e6",
      "editor.background": "#00000000",
    },
    tokenColors: RULES.map((r) => ({ scope: r.scope, settings: { foreground: r[mode] } })),
  };
}

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

let highlighter: Promise<HighlighterGeneric<string, string>> | null = null;
const loaded = new Set<string>(["text"]);
const failed = new Set<string>();

function getHighlighter() {
  highlighter ??= (async () => {
    const [{ createHighlighter }, { createJavaScriptRegexEngine }] = await Promise.all([
      import("shiki"),
      import("shiki/engine/javascript"),
    ]);
    return createHighlighter({
      themes: [theme("rc-light", "light"), theme("rc-dark", "dark")],
      langs: [],
      engine: createJavaScriptRegexEngine(),
    }) as Promise<HighlighterGeneric<string, string>>;
  })();
  return highlighter;
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
  const h = await getHighlighter();
  if (!loaded.has(language)) {
    try {
      await h.loadLanguage(language as never);
      loaded.add(language);
    } catch {
      failed.add(language);
      return null;
    }
  }
  const html = h.codeToHtml(code, {
    lang: language,
    themes: { light: "rc-light", dark: "rc-dark" },
    defaultColor: false,
  });
  remember(`${language}\u0000${code}`, html);
  return html;
}

/** The language a path's extension implies, for diffs. */
export function langForPath(path: string): string | undefined {
  const ext = path.split(".").pop()?.toLowerCase();
  if (!ext || ext === path.toLowerCase()) return undefined;
  const map: Record<string, string> = {
    ts: "typescript",
    tsx: "tsx",
    js: "javascript",
    jsx: "jsx",
    rs: "rust",
    py: "python",
    swift: "swift",
    go: "go",
    md: "markdown",
    json: "json",
    toml: "toml",
    yml: "yaml",
    yaml: "yaml",
    css: "css",
    html: "html",
    sh: "bash",
  };
  return map[ext];
}
