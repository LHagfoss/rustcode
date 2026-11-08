// The window's backdrop material on Windows: Mica (Windows 11), applied by the shell when the
// window is built (src-tauri/src/webview.rs). The window is transparent there, so the page only
// lets it show (`data-material="mica"` on <html>, read by index.css) once the shell says it is on;
// on Windows 10 there is none and the sidebar stays solid, as on Linux.

import { isTauri } from "../acp/transport";
import { currentPlatform } from "./platform";
import { shellCall } from "./shell";

export type Material = "mica" | "none";

/** What the shell answered as a material; anything else is none. */
export function materialFrom(answer: unknown): Material {
  return answer === "mica" ? "mica" : "none";
}

/** Sets `data-material` on <html> from the shell. Windows only; nothing is set elsewhere. */
export async function applyMaterial(): Promise<Material> {
  if (!isTauri() || currentPlatform() !== "windows") return "none";
  const material = await shellCall<string>("window_material")
    .then(materialFrom)
    .catch((): Material => "none");
  if (material !== "none") document.documentElement.dataset.material = material;
  return material;
}
