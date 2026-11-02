// Pictures and diagrams in a reply's Markdown.
//
// - `![alt](path)`: a web or `data:` picture shows as is; a file path
//   (absolute, `~/…`, `file://…` or relative to the thread's folder) is read
//   by the shell's `thread_image`, which refuses anything outside the
//   thread's folder and its worktree. A click opens the lightbox.
// - ```mermaid blocks: mermaid is loaded only when the first such block
//   appears, themed from the tokens, rendered strictly (no scripts, no
//   links out), capped in size, and shown as code when it does not parse.

import { CircleAlert, ImageOff } from "lucide-react";
import { useEffect, useState } from "react";
import { threadImage } from "../lib/native";
import { hasShell } from "../lib/shell";
import { useApp } from "../state/store";
import { openLightbox } from "./Lightbox";
import { cx } from "./primitives";

// ---------------------------------------------------------------- pictures

/** Read pictures by their path and folders, kept while the app runs (at most 64). */
const pictures = new Map<string, Promise<string>>();

/** The open thread's folders: its own folder, then its worktree's. */
function threadRoots(): string[] {
  const s = useApp.getState();
  const thread = s.selectedId ? s.threads[s.selectedId] : undefined;
  return [thread?.cwd ?? s.projectCwd, thread?.worktree?.path].filter((r): r is string => !!r);
}

function loadPicture(path: string, roots: string[]): Promise<string> {
  const key = `${roots.join("\0")}\0${path}`;
  let hit = pictures.get(key);
  if (!hit) {
    hit = threadImage(path, roots).then((f) => `data:${f.mimeType};base64,${f.data}`);
    // A failure is not kept: the file may appear later.
    hit.catch(() => pictures.delete(key));
    pictures.set(key, hit);
    if (pictures.size > 64) pictures.delete(pictures.keys().next().value as string);
  }
  return hit;
}

/** A path a reply names for a picture, without a `file://` prefix; null for a web or `data:` source. */
export function picturePath(src: string): string | null {
  if (/^(https?:|data:image\/)/i.test(src)) return null;
  if (/^file:\/\//i.test(src)) {
    try {
      return decodeURIComponent(new URL(src).pathname);
    } catch {
      return null;
    }
  }
  // Any other scheme (javascript:, blob:, …) is not a picture path.
  if (/^[a-z][a-z0-9+.-]*:/i.test(src)) return null;
  return decodeURIComponent(src);
}

export function MarkdownImage({ src, alt }: { src?: string; alt?: string }) {
  const path = src ? picturePath(src) : null;
  const direct = src && !path && /^(https?:|data:image\/)/i.test(src) ? src : null;
  const [state, setState] = useState<{ url?: string; error?: string }>({});
  useEffect(() => {
    if (!path) return;
    if (!hasShell()) {
      setState({ error: "Pictures from files show in the desktop app." });
      return;
    }
    let live = true;
    loadPicture(path, threadRoots()).then(
      (url) => live && setState({ url }),
      (e) => live && setState({ error: e instanceof Error ? e.message : String(e) }),
    );
    return () => {
      live = false;
    };
  }, [path]);
  const url = direct ?? state.url;
  if (!url) {
    if (!state.error && !src) return null;
    return (
      <span className="inline-flex max-w-full items-center gap-1.5 rounded-md bg-fill-subtle px-2 py-1 align-middle text-callout text-fg-muted">
        {state.error ? <ImageOff size={12} className="shrink-0" /> : null}
        <span className="truncate">{state.error ? `${alt || path}: ${state.error}` : alt || "Picture"}</span>
      </span>
    );
  }
  return (
    <button
      type="button"
      onClick={() => openLightbox([{ src: url, alt: alt ?? "" }])}
      className="block max-w-full cursor-zoom-in rounded-lg"
      aria-label={alt ? `Open picture: ${alt}` : "Open picture"}
    >
      <img
        src={url}
        alt={alt ?? ""}
        loading="lazy"
        draggable={false}
        className="max-h-[360px] max-w-full rounded-lg object-contain"
      />
    </button>
  );
}

// ---------------------------------------------------------------- mermaid

/** Larger diagrams stay code: rendering them blocks the main thread for too long. */
const MERMAID_MAX_CHARS = 12_000;
const MERMAID_MAX_LINES = 300;

type Mermaid = typeof import("mermaid").default;
let mermaidLoad: Promise<Mermaid> | null = null;
let themed = "";
let renders = 0;
/** Rendered diagrams by theme and source. */
const diagrams = new Map<string, Promise<string>>();

function token(css: CSSStyleDeclaration, name: string): string {
  return css.getPropertyValue(`--rc-${name}`).trim();
}

/** Mermaid's colours from the theme on :root, so a diagram matches the app in either mode. */
function themeMermaid(mermaid: Mermaid, key: string) {
  if (themed === key) return;
  themed = key;
  const css = getComputedStyle(document.documentElement);
  const t = (n: string) => token(css, n);
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: "base",
    fontFamily: getComputedStyle(document.body).fontFamily,
    themeVariables: {
      darkMode: document.documentElement.dataset.theme === "dark",
      fontSize: "13px",
      background: t("code-bg"),
      mainBkg: t("surface-2"),
      primaryColor: t("surface-2"),
      primaryTextColor: t("text"),
      primaryBorderColor: t("text-faint"),
      secondaryColor: t("surface-3"),
      tertiaryColor: t("surface"),
      textColor: t("text"),
      lineColor: t("text-muted"),
      nodeBorder: t("text-faint"),
      clusterBkg: t("surface"),
      clusterBorder: t("text-faint"),
      edgeLabelBackground: t("code-bg"),
      noteBkgColor: t("surface-3"),
      noteTextColor: t("text"),
      noteBorderColor: t("text-faint"),
    },
  });
}

function renderDiagram(code: string, key: string): Promise<string> {
  const cacheKey = `${key}\0${code}`;
  let hit = diagrams.get(cacheKey);
  if (!hit) {
    mermaidLoad ??= import("mermaid").then((m) => m.default);
    hit = mermaidLoad.then(async (mermaid) => {
      themeMermaid(mermaid, key);
      const { svg } = await mermaid.render(`rc-mermaid-${++renders}`, code);
      return svg;
    });
    hit.catch(() => diagrams.delete(cacheKey));
    diagrams.set(cacheKey, hit);
    if (diagrams.size > 48) diagrams.delete(diagrams.keys().next().value as string);
  }
  return hit;
}

/** A ```mermaid block: the diagram, or its code (with why) when it is too big or does not parse. */
export function MermaidBlock({ code, fallback }: { code: string; fallback: React.ReactNode }) {
  const themeKey = useApp((s) => `${s.theme}/${s.themeActive}/${s.themes ? 1 : 0}`);
  const [state, setState] = useState<{ svg?: string; error?: string; code?: string }>({});
  const tooBig = code.length > MERMAID_MAX_CHARS || code.split("\n").length > MERMAID_MAX_LINES;
  useEffect(() => {
    if (tooBig) return;
    let live = true;
    // A streaming block changes per chunk: wait for it to settle before parsing.
    const timer = setTimeout(() => {
      const mode = document.documentElement.dataset.theme ?? "";
      renderDiagram(code, `${themeKey}/${mode}`).then(
        (svg) => live && setState({ svg, code }),
        (e) => live && setState({ error: e instanceof Error ? e.message.split("\n")[0] : String(e), code }),
      );
    }, 200);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [code, themeKey, tooBig]);

  const error = tooBig ? "Too large to draw; shown as code." : state.code === code ? state.error : undefined;
  if (error || !state.svg) {
    return (
      <div className="flex flex-col gap-1">
        {fallback}
        {error && (
          <span className="flex items-center gap-1.5 px-1 text-caption text-fg-faint">
            <CircleAlert size={11} className="shrink-0" />
            {tooBig ? error : `Mermaid: ${error}`}
          </span>
        )}
      </div>
    );
  }
  const svg = state.svg;
  return (
    <button
      type="button"
      onClick={() =>
        openLightbox([{ src: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`, alt: "Diagram" }])
      }
      aria-label="Open diagram"
      className={cx(
        "rc-mermaid block max-h-[520px] w-full cursor-zoom-in overflow-auto rounded-lg bg-code-bg p-3 text-left",
      )}
      // Mermaid's own output, rendered with securityLevel "strict" (no scripts, no click handlers).
      // biome-ignore lint/security/noDangerouslySetInnerHtml: sanitized by mermaid in strict mode
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
