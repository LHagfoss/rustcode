// Trackpad haptics (src-tauri `haptic`, NSHapticFeedbackManager): felt only
// on a Force Touch trackpad with a finger on it, and only in the macOS app.
// Settings › General turns them off.

import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";

const KEY = "rustcode.haptics";

export type HapticKind = "generic" | "alignment" | "level";

export function hapticsEnabled(): boolean {
  try {
    return localStorage.getItem(KEY) !== "0";
  } catch {
    return true;
  }
}

export function setHaptics(on: boolean) {
  try {
    localStorage.setItem(KEY, on ? "1" : "0");
  } catch {
    // Not persisted in a private window.
  }
}

let last = 0;

/** One tap; repeats within 40 ms (a burst of hovers) are dropped. */
export function haptic(kind: HapticKind = "generic") {
  if (!isTauri() || !hapticsEnabled()) return;
  const now = performance.now();
  if (now - last < 40) return;
  last = now;
  void shellCall("haptic", { kind }).catch(() => undefined);
}
