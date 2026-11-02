// Shiki, off the main thread (lib/highlight.ts). In WebKit its JavaScript
// regex engine took about 1 s to compile the TypeScript grammar and 60 ms for a
// 200-line block, in one main-thread task: a stall when a code block scrolled
// into view. Here it runs on the Oniguruma (WebAssembly) engine, 73 ms and
// 13 ms, and none of it on the main thread (docs/desktop.md › Native frame rate).

import type { HighlighterGeneric } from "shiki";
import { THEMES } from "./highlightThemes";

export interface HighlightRequest {
  id: number;
  code: string;
  language: string;
}

export interface HighlightResponse {
  id: number;
  /** Null for a language Shiki does not know. */
  html: string | null;
}

let highlighter: Promise<HighlighterGeneric<string, string>> | null = null;
const loaded = new Set<string>();
const failed = new Set<string>();

function getHighlighter() {
  highlighter ??= (async () => {
    const [{ createHighlighter }, { createOnigurumaEngine }] = await Promise.all([
      import("shiki"),
      import("shiki/engine/oniguruma"),
    ]);
    return createHighlighter({
      themes: THEMES,
      langs: [],
      engine: createOnigurumaEngine(import("shiki/wasm")),
    }) as Promise<HighlighterGeneric<string, string>>;
  })();
  return highlighter;
}

async function render(code: string, language: string): Promise<string | null> {
  if (failed.has(language)) return null;
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
  return h.codeToHtml(code, {
    lang: language,
    themes: { light: "rc-light", dark: "rc-dark" },
    defaultColor: false,
  });
}

// The worker's global scope (the project's TypeScript lib is the DOM's).
const scope = self as unknown as {
  onmessage: ((event: MessageEvent<HighlightRequest>) => void) | null;
  postMessage(message: HighlightResponse): void;
};

// One block at a time, in order, so a burst of blocks does not interleave.
let queue = Promise.resolve();
scope.onmessage = (event) => {
  const { id, code, language } = event.data;
  queue = queue.then(async () => {
    let html: string | null = null;
    try {
      html = await render(code, language);
    } catch {
      html = null;
    }
    scope.postMessage({ id, html });
  });
};
