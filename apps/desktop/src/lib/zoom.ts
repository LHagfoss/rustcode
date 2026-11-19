// One saved interface scale for Appearance and the keyboard shortcuts. The native
// webview zoom scales the whole viewport, including floating menus and dialogs.

import { isTauri } from "../acp/transport";
import { readStorage, writeStorage } from "./storage";

const KEY = "rustcode.zoom";
export const ZOOM_DEFAULT = 1;
export const ZOOM_MIN = 0.8;
export const ZOOM_MAX = 1.5;
const STEP = 0.1;

export function clampZoom(level: number): number {
  const rounded = Math.round(level * 10) / 10;
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Number.isFinite(rounded) ? rounded : ZOOM_DEFAULT));
}

export function zoomLabel(level: number): string {
  return `${Math.round(level * 100)}%`;
}

let currentZoom = ZOOM_DEFAULT;
let pending = Promise.resolve();
const listeners = new Set<() => void>();

export function getZoom(): number {
  return currentZoom;
}

export function subscribeZoom(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

async function apply(level: number): Promise<void> {
  if (isTauri()) {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    await getCurrentWebview().setZoom(level);
  } else {
    document.documentElement.style.zoom = level === ZOOM_DEFAULT ? "" : String(level);
  }
}

function updateZoom(nextLevel: (level: number) => number): Promise<number> {
  // A held shortcut can issue another step before the previous native call returns.
  const update = pending.then(async () => {
    const level = clampZoom(nextLevel(currentZoom));
    if (level === currentZoom) return level;
    await apply(level);
    writeStorage(KEY, String(level));
    currentZoom = level;
    for (const listener of listeners) listener();
    return level;
  });
  pending = update.then(
    () => undefined,
    () => undefined,
  );
  return update;
}

export function stepZoom(direction: 1 | -1 | 0): Promise<number> {
  return updateZoom((level) => (direction === 0 ? ZOOM_DEFAULT : level + direction * STEP));
}

export function restoreZoom(): void {
  const saved = Number(readStorage(KEY) ?? ZOOM_DEFAULT);
  void updateZoom(() => saved).catch(() => undefined);
}
