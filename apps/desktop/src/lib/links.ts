// Link metadata for chips: title, favicon (a data URL) and the page's colour,
// read by the shell (`src-tauri/src/links.rs`), never by the webview. Cached
// here for the session, up to MAX_CACHED pages (least recently used go first); the shell keeps its
// own memory and disk cache.

import { clip } from "./clip";
import { hasShell, shellCall } from "./shell";
import { readFlag, writeFlag } from "./storage";

export interface LinkMeta {
  title?: string;
  siteName?: string;
  /** A `data:` URL; remote images never load in the app. */
  icon?: string;
  /** A CSS colour from `theme-color`, or none. */
  color?: string;
}

const KEY = "rustcode.linkPreviews";
/** Pages kept in this window: each can carry a favicon data URL, so the map is bounded. */
export const MAX_CACHED = 500;
const cache = new Map<string, LinkMeta | null>();

/** A cached answer, made the most recently used. */
function cached(url: string): LinkMeta | null | undefined {
  if (!cache.has(url)) return undefined;
  const meta = cache.get(url) ?? null;
  cache.delete(url);
  cache.set(url, meta);
  return meta;
}

function remember(url: string, meta: LinkMeta | null) {
  cache.delete(url);
  cache.set(url, meta);
  // A Map iterates in insertion order: the first key is the least recently used.
  while (cache.size > MAX_CACHED) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}
const pending = new Map<string, Promise<LinkMeta | null>>();
const waiting: (() => void)[] = [];
let active = 0;
const MAX_ACTIVE = 4;

/** Chips on, until the user turns them off (Settings › Chat). */
export const LINK_PREVIEWS_DEFAULT = true;

export function linkPreviewsEnabled(): boolean {
  return readFlag(KEY, LINK_PREVIEWS_DEFAULT);
}

export function setLinkPreviews(on: boolean) {
  // Not persisted in a private window.
  writeFlag(KEY, on);
}

export function linkMetaCached(url: string): LinkMeta | null {
  return cached(url) ?? null;
}

/** The site a URL points at, without a `www.` nobody reads. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** A GitHub issue or pull request a URL points at, with anything after its number ignored. */
export interface GitHubRef {
  repo: string;
  kind: "pull" | "issue";
  number: number;
}

export function githubRefOf(url: string): GitHubRef | undefined {
  const match = /^https:\/\/(?:www\.)?github\.com\/([\w.-]+\/[\w.-]+)\/(pull|issues)\/(\d+)(?:[/?#]|$)/i.exec(
    url,
  );
  const number = Number(match?.[3]);
  if (!match || !Number.isSafeInteger(number) || number <= 0) return undefined;
  return { repo: match[1] ?? "", kind: match[2]?.toLowerCase() === "pull" ? "pull" : "issue", number };
}

/**
 * The issue's or pull request's own title out of GitHub's page title, "Fix it by someone · Pull
 * Request #12 · owner/repo · GitHub". A page that does not name the number (a private repository's
 * "Page not found", a sign-in page) has no title worth showing.
 */
export function githubTitle(title: string | undefined, ref: GitHubRef): string | undefined {
  const marker = ref.kind === "pull" ? ` · Pull Request #${ref.number}` : ` · Issue #${ref.number}`;
  const at = title?.indexOf(marker) ?? -1;
  if (!title || at <= 0) return undefined;
  let own = title.slice(0, at).trim();
  // A pull request's page title names its author: "Fix it by someone".
  if (ref.kind === "pull") own = own.replace(/\s+by\s+[\w-]+(?:\[bot\])?$/, "");
  return own || undefined;
}

/**
 * What a chip says: the page's title over its host, or the host alone when the shell
 * had no title (or a title that is only the host). A GitHub issue or pull request leads with
 * its number (`tag`) and its own title, and says `owner/repo#12` until the title is known.
 * These are the chip's words, so a text selection dragged across a chip copies them and
 * never the favicon or the arrow.
 */
export function linkChipLabel(
  url: string,
  meta: LinkMeta | null,
): { title: string; host: string; showsHost: boolean; tag?: string; github?: GitHubRef } {
  const host = hostOf(url);
  const github = githubRefOf(url);
  if (github) {
    const own = githubTitle(meta?.title, github);
    const repoName = github.repo.split("/")[1] ?? github.repo;
    return own
      ? { tag: `#${github.number}`, title: own, host: repoName, showsHost: true, github }
      : { title: `${github.repo}#${github.number}`, host, showsHost: false, github };
  }
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
  const known = cached(url);
  if (known !== undefined) return Promise.resolve(known);
  if (!linkPreviewsEnabled() || !hasShell() || !/^https?:\/\//i.test(url)) return Promise.resolve(null);
  const existing = pending.get(url);
  if (existing) return existing;
  const job = slot(() => shellCall<unknown>("link_meta", { url }))
    .then(sanitizeMeta, () => null)
    .then((meta) => {
      remember(url, meta);
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
