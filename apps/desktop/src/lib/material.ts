// The window's own material behind the sidebar (index.css `data-material`). Only macOS has one:
// the `sidebar` vibrancy `tauri.conf.json` gives the transparent window, which the system draws
// whether or not the page shows it. A browser (the mock) has no window behind the page, and
// Windows and Linux windows are opaque, so none of them gets the attribute and the sidebar stays
// solid there.

import { isTauri } from "../acp/transport";
import type { Platform } from "./platform";
import { shellCall } from "./shell";

export type Material = "vibrancy" | "none";

/** The material for a platform: macOS's vibrancy in the app's own window, else none. */
export function materialFor(platform: Platform, tauri: boolean): Material {
  return tauri && platform === "mac" ? "vibrancy" : "none";
}

/** Sets `data-material` on <html> before the first render; nothing is set without a material. */
export function applyMaterial(platform: Platform): Material {
  const material = materialFor(platform, isTauri());
  if (material !== "none") document.documentElement.dataset.material = material;
  return material;
}

/**
 * Gives the window the app's light or dark mode (`null` follows the system), so the material is
 * drawn in the tone the sidebar is tinted with. Only where there is a material.
 */
export function matchWindowTheme(mode: "light" | "dark" | null) {
  if (document.documentElement.dataset.material === undefined) return;
  void shellCall("window_theme", { mode }).catch(() => undefined);
}
