// Text zoom, issue 2865: ⌘+ / ⌘= larger, ⌘- smaller, ⌘0 actual size. In the app
// it is the webview's own page zoom, so popovers and menus still line up; in a
// browser (dev, mock) the root's CSS zoom stands in. The level is kept.

import { isTauri } from "../acp/transport";
import { readStorage, writeStorage } from "./storage";

const KEY = "rustcode.zoom";
export const ZOOM_MIN = 0.8;
export const ZOOM_MAX = 1.5;
const STEP = 0.1;

export function clampZoom(level: number): number {
  const rounded = Math.round(level * 10) / 10;
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Number.isFinite(rounded) ? rounded : 1));
}

export function storedZoom(): number {
  return clampZoom(Number(readStorage(KEY) ?? 1));
}

async function apply(level: number): Promise<void> {
  if (isTauri()) {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    await getCurrentWebview().setZoom(level);
  } else {
    document.documentElement.style.setProperty("zoom", level === 1 ? "" : String(level));
  }
}

/** Sets the zoom level (clamped) and keeps it; answers the level now in force. */
export async function setZoom(level: number): Promise<number> {
  const next = clampZoom(level);
  // Not kept in a private window.
  writeStorage(KEY, String(next));
  await apply(next);
  return next;
}

/** ⌘+ (1), ⌘- (-1) or ⌘0 (0). */
export function stepZoom(direction: 1 | -1 | 0): Promise<number> {
  return setZoom(direction === 0 ? 1 : storedZoom() + direction * STEP);
}

/** At start: the kept level, if it is not 100%. */
export function restoreZoom(): void {
  const level = storedZoom();
  if (level !== 1) void apply(level).catch(() => undefined);
}
