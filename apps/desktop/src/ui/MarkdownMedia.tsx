// Pictures and diagrams in a reply's Markdown.
//
// - `![alt](path)`: a web or `data:` picture shows as is; a file path
//   (absolute, `~/…`, `file://…` or relative to the thread's folder) is read
//   by the shell's `thread_image`, which refuses anything outside the
//   thread, project, worktree and OS scratch folders. A thread on a paired
//   machine names paths on that machine's disk, so its engine reads them
//   instead, in chunks over the connection (`_rustcode/attachments/read` with
//   the thread's id), within the same folders there. A click opens the lightbox.
// - ```mermaid blocks: mermaid is loaded only when the first such block
//   appears, themed from the tokens, rendered strictly (no scripts, no
//   links out), capped in size, and shown as code when it does not parse.

import { useEffect, useState } from "react";
import { RpcError } from "../acp/jsonrpc";
import { openPath, revealPath, threadImage } from "../lib/native";
import { isAbsolute, join } from "../lib/paths";
import { appPlatform } from "../lib/platform";
import { hasShell } from "../lib/shell";
import { readImage } from "../state/attachmentTransfer";
import { connectionForThread, useApp } from "../state/store";
import { isRemoteThread, splitThreadKey } from "../state/threadKeys";
import { CircleAlert, ImageOff } from "./icons";
import { openLightbox } from "./Lightbox";
import { cx, Pill } from "./primitives";

// ---------------------------------------------------------------- pictures

/** A picture read and decoded, with its own size, so it is laid out at that size from its first frame. */
type Picture = { url: string; width: number; height: number };

/** Read pictures by their path and folders, kept while the app runs (at most 64). */
const pictures = new Map<string, Promise<Picture>>();
/** The ones already decoded: a row that mounts again (a thread opened again, a scroll back) draws them at once. */
const decoded = new Map<string, Picture>();

/** The open thread's folders: its own folder, then its worktree's. */
function threadRoots(): string[] {
  const s = useApp.getState();
  const thread = s.selectedId ? s.threads[s.selectedId] : undefined;
  return [thread?.cwd ?? s.projectCwd, thread?.worktree?.path, thread?.worktree?.repo].filter(
    (r): r is string => !!r,
  );
}

/** The open thread's key when it runs on a paired machine, whose disk holds its pictures. */
function remoteThread(): string | null {
  const id = useApp.getState().selectedId;
  return id && isRemoteThread(id) ? id : null;
}

function pictureKey(path: string, roots: string[]): string {
  return `${remoteThread() ?? ""}\0${roots.join("\0")}\0${path}`;
}

function bytesUrl(mimeType: string, bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return `data:${mimeType};base64,${btoa(text)}`;
}

/** A picture on the paired machine a thread runs on, read by that machine's engine. */
async function remotePicture(thread: string, path: string): Promise<string> {
  const conn = connectionForThread(thread);
  if (!conn) throw new Error("The computer this thread runs on is not connected.");
  const { mimeType, bytes } = await readImage(
    (method, params, timeout) => conn.request(method, params, timeout),
    path,
    splitThreadKey(thread).id,
  ).catch((e: unknown) => {
    // The engine says why in the error's data ("No such picture.", "outside the thread's folders…").
    const reason =
      e instanceof RpcError && typeof e.data === "string"
        ? e.data
        : e instanceof Error
          ? e.message
          : String(e);
    // An engine from before thread pictures were served answers only for stored attachments.
    if (/not kept on this computer/.test(reason)) {
      throw new Error("Update RustCode on the computer this thread runs on to see its pictures.");
    }
    throw new Error(reason.charAt(0).toUpperCase() + reason.slice(1));
  });
  return bytesUrl(mimeType, bytes);
}

async function decode(url: string): Promise<Picture> {
  const img = new Image();
  img.src = url;
  // A picture the web view cannot decode still shows its own error below, from the <img>.
  await img.decode().catch(() => undefined);
  return { url, width: img.naturalWidth, height: img.naturalHeight };
}

function loadPicture(path: string, roots: string[]): Promise<Picture> {
  const key = pictureKey(path, roots);
  let hit = pictures.get(key);
  if (!hit) {
    const remote = remoteThread();
    const url = remote
      ? remotePicture(remote, path)
      : threadImage(path, roots).then((f) => `data:${f.mimeType};base64,${f.data}`);
    hit = url.then(decode).then((picture) => {
      decoded.set(key, picture);
      return picture;
    });
    // A failure is not kept: the file may appear later.
    hit.catch(() => pictures.delete(key));
    pictures.set(key, hit);
    if (pictures.size > 64) {
      const oldest = pictures.keys().next().value as string;
      pictures.delete(oldest);
      decoded.delete(oldest);
    }
  }
  return hit;
}

/** A path a reply names for a picture, without a `file://` prefix; null for a web or `data:` source. */
export function picturePath(src: string): string | null {
  if (/^(https?:|data:image\/)/i.test(src)) return null;
  if (/^file:\/\//i.test(src)) {
    try {
      const url = new URL(src);
      const path = decodeURIComponent(url.pathname);
      if (url.hostname) return `//${url.hostname}${path}`;
      return /^\/[a-z]:\//i.test(path) ? path.slice(1) : path;
    } catch {
      return null;
    }
  }
  // Any other scheme (javascript:, blob:, …) is not a picture path.
  if (/^[a-z]:[\\/]/i.test(src)) return src;
  if (/^[a-z][a-z0-9+.-]*:/i.test(src)) return null;
  try {
    return decodeURIComponent(src);
  } catch {
    return src;
  }
}

/** A failed picture remains actionable, including when its file has disappeared. */
export function BlockedImage({ path, reason, remote }: { path: string; reason: string; remote?: boolean }) {
  const reveal =
    appPlatform() === "windows"
      ? "Reveal in Explorer"
      : appPlatform() === "mac"
        ? "Reveal in Finder"
        : "Reveal in File Manager";
  // Its own line, as a picture would be, and only as wide as it needs: inline it wrapped the
  // sentence around it, and a full-width reason line stretched it across the reply.
  return (
    <span className="my-1 flex w-fit max-w-full flex-wrap items-center gap-x-2 gap-y-0.5 rounded-md bg-fill-subtle py-0.5 pr-1 pl-2 text-callout text-fg-muted">
      <ImageOff size={12} className="shrink-0" />
      <span className="min-w-0 break-all font-mono text-caption">{path}</span>
      <span className="min-w-0 truncate text-fg-faint" title={reason}>
        {reason.split("\n")[0]}
      </span>
      {/* A paired machine's file is not on this computer to open or reveal. */}
      {!remote && (
        <span className="flex shrink-0 items-center">
          <Pill quiet size="sm" onClick={() => openPath(path)}>
            Open
          </Pill>
          <Pill quiet size="sm" onClick={() => revealPath(path)}>
            {reveal}
          </Pill>
        </span>
      )}
    </span>
  );
}

export function MarkdownImage({ src, alt }: { src?: string; alt?: string }) {
  const path = src ? picturePath(src) : null;
  const direct = src && !path && /^(https?:|data:image\/)/i.test(src) ? src : null;
  const [state, setState] = useState<{ picture?: Picture; error?: string }>(() => ({
    picture: path ? decoded.get(pictureKey(path, threadRoots())) : undefined,
  }));
  useEffect(() => {
    if (!path) return;
    if (!hasShell() && !remoteThread()) {
      setState({ error: "Pictures from files show in the desktop app." });
      return;
    }
    const roots = threadRoots();
    const ready = decoded.get(pictureKey(path, roots));
    if (ready) {
      setState({ picture: ready });
      return;
    }
    setState({});
    let live = true;
    loadPicture(path, roots).then(
      (picture) => live && setState({ picture }),
      (e) => live && setState({ error: e instanceof Error ? e.message : String(e) }),
    );
    return () => {
      live = false;
    };
  }, [path]);
  const picture = state.picture;
  const url = direct ?? picture?.url;
  if (state.error && path) {
    const actionPath = isAbsolute(path) || path.startsWith("~/") ? path : join(threadRoots()[0] ?? "", path);
    return <BlockedImage path={actionPath} reason={state.error} remote={!!remoteThread()} />;
  }
  if (!url) {
    if (!state.error && !src) return null;
    if (state.error) {
      return (
        <span className="inline-flex max-w-full items-center gap-1.5 rounded-md bg-fill-subtle px-2 py-1 align-middle text-callout text-fg-muted">
          <ImageOff size={12} className="shrink-0" />
          <span className="truncate">{`${alt || path}: ${state.error}`}</span>
        </span>
      );
    }
    // Still reading: a picture-sized box, not a one-line label, so the reply does not jump by a
    // picture's height when it arrives (`PICTURE_PLACEHOLDER` in ui/Transcript.tsx's estimate).
    return (
      <span
        role="img"
        aria-label={alt ? `Loading picture: ${alt}` : "Loading picture"}
        className="block h-[240px] w-full max-w-[420px] rounded-lg bg-fill-subtle"
      />
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
        // Its own size, read when it was decoded: the box is right before the pixels are.
        width={picture?.width || undefined}
        height={picture?.height || undefined}
        onError={() => setState({ error: "Could not display this picture." })}
        draggable={false}
        className="h-auto max-h-[360px] max-w-full rounded-lg object-contain"
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
            <CircleAlert size={12} className="shrink-0" />
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
