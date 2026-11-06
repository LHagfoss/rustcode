// Link metadata for chips: title, favicon (a data URL) and the page's colour,
// read by the shell (`src-tauri/src/links.rs`), never by the webview. Cached
// here for the session; the shell keeps its own memory and disk cache.

import { clip } from "./clip";
import { hasShell, shellCall } from "./shell";

export interface LinkMeta {
  title?: string;
  siteName?: string;
  /** A `data:` URL; remote images never load in the app. */
  icon?: string;
  /** A CSS colour from `theme-color`, or none. */
  color?: string;
}

const KEY = "rustcode.linkPreviews";
const cache = new Map<string, LinkMeta | null>();
const pending = new Map<string, Promise<LinkMeta | null>>();
const waiting: (() => void)[] = [];
let active = 0;
const MAX_ACTIVE = 4;

/** Chips on, until the user turns them off (Settings › Chat). */
export const LINK_PREVIEWS_DEFAULT = true;

export function linkPreviewsEnabled(): boolean {
  try {
    const raw = localStorage.getItem(KEY);
    return raw === null ? LINK_PREVIEWS_DEFAULT : raw === "1";
  } catch {
    return LINK_PREVIEWS_DEFAULT;
  }
}

export function setLinkPreviews(on: boolean) {
  try {
    localStorage.setItem(KEY, on ? "1" : "0");
  } catch {
    // Not persisted in a private window.
  }
}

export function linkMetaCached(url: string): LinkMeta | null {
  return cache.get(url) ?? null;
}

/** The site a URL points at, without a `www.` nobody reads. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/**
 * What a chip says: the page's title over its host, or the host alone when the shell
 * had no title (or a title that is only the host). These are the chip's words, so a
 * text selection dragged across a chip copies them and never the favicon or the arrow.
 */
export function linkChipLabel(
  url: string,
  meta: LinkMeta | null,
): { title: string; host: string; showsHost: boolean } {
  const host = hostOf(url);
  const title = meta?.title?.trim() || host;
  return { title, host, showsHost: title !== host };
}

const COLOR = /^(#[0-9a-f]{3,8}|rgba?\([\d\s.,%/]+\)|hsla?\([\d\s.,%/deg]+\))$/i;

/** Only what the chip can use safely: plain strings, a data: image, a colour literal. */
export function sanitizeMeta(raw: unknown): LinkMeta | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const text = (v: unknown, max: number) =>
    typeof v === "string" && v.trim() ? clip(v.trim().replace(/\s+/g, " "), max) : undefined;
  const icon =
    typeof r.icon === "string" &&
    /^data:image\/(png|jpeg|gif|webp|x-icon|vnd\.microsoft\.icon|svg\+xml);base64,/i.test(r.icon)
      ? r.icon
      : undefined;
  const color = typeof r.color === "string" && COLOR.test(r.color.trim()) ? r.color.trim() : undefined;
  const meta = { title: text(r.title, 160), siteName: text(r.siteName, 80), icon, color };
  return meta.title || meta.icon || meta.color ? meta : null;
}

async function slot<T>(run: () => Promise<T>): Promise<T> {
  if (active >= MAX_ACTIVE) await new Promise<void>((r) => waiting.push(r));
  active++;
  try {
    return await run();
  } finally {
    active--;
    waiting.shift()?.();
  }
}

export function loadLinkMeta(url: string): Promise<LinkMeta | null> {
  if (cache.has(url)) return Promise.resolve(cache.get(url) ?? null);
  if (!linkPreviewsEnabled() || !hasShell() || !/^https?:\/\//i.test(url)) return Promise.resolve(null);
  const existing = pending.get(url);
  if (existing) return existing;
  const job = slot(() => shellCall<unknown>("link_meta", { url }))
    .then(sanitizeMeta, () => null)
    .then((meta) => {
      cache.set(url, meta);
      pending.delete(url);
      return meta;
    });
  pending.set(url, job);
  return job;
}

export function clearLinkCacheForTests() {
  cache.clear();
  pending.clear();
}
