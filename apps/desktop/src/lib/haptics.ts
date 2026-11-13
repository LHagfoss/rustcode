// Trackpad haptics (src-tauri `haptic`, NSHapticFeedbackManager): felt only
// on a Force Touch trackpad with a finger on it, and only in the macOS app.
// Settings › General turns them off.

import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";
import { readFlag, writeFlag } from "./storage";

const KEY = "rustcode.haptics";

export type HapticKind = "generic" | "alignment" | "level";

/** Felt on a Force Touch trackpad, until the user turns them off (Settings › General). */
export const HAPTICS_DEFAULT = true;

export function hapticsEnabled(): boolean {
  return readFlag(KEY, HAPTICS_DEFAULT);
}

export function setHaptics(on: boolean) {
  // Not persisted in a private window.
  writeFlag(KEY, on);
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
