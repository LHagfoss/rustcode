// Reduce transparency: glass turns solid (index.css `:root.reduce-transparency`). Four things ask
// for it, and the class on <html> is on when any does: the user's own switch (Settings ›
// Appearance), the system's request (macOS, through the shell; the web view's
// `prefers-reduced-transparency` elsewhere), and a web view that cannot blur behind a surface,
// where a translucent pane without its blur is text over text (docs/design.md › Platforms).

import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";

const QUERY = "(prefers-reduced-transparency: reduce)";

/** Whether the glass can blur: the web view has the filter, and the shell says it composites. */
export function canBlur(filter: boolean, composites: boolean): boolean {
  return filter && composites;
}

/** Whether the class is on: any one reason is enough. */
export function reducesTransparency(reasons: {
  user: boolean;
  system: boolean;
  media: boolean;
  blur: boolean;
}): boolean {
  return reasons.user || reasons.system || reasons.media || !reasons.blur;
}

let user = false;
let system = false;
let composites = true;
let installed = false;

function supportsBlur(): boolean {
  if (typeof CSS === "undefined" || typeof CSS.supports !== "function") return true;
  return CSS.supports("backdrop-filter", "blur(1px)") || CSS.supports("-webkit-backdrop-filter", "blur(1px)");
}

function media(): MediaQueryList | null {
  return typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia(QUERY)
    : null;
}

function paint() {
  if (typeof document === "undefined") return;
  document.documentElement.classList.toggle(
    "reduce-transparency",
    reducesTransparency({
      user,
      system,
      media: media()?.matches ?? false,
      blur: canBlur(supportsBlur(), composites),
    }),
  );
}

/** The user's switch (Settings › Appearance). */
export function setUserReduceTransparency(on: boolean) {
  user = on;
  paint();
}

/** What the system asks for, where the shell can read it (macOS). */
export function setSystemReduceTransparency(on: boolean) {
  system = on;
  paint();
}

/** What the shell answered for the web view's compositing (`webview_blur`, src-tauri/src/webview.rs). */
export function setShellCompositing(on: boolean) {
  // Only a plain `false` goes solid: an answer the shell could not give keeps the glass as the
  // filter itself says, rather than turning it solid for a web view that never refused.
  composites = on !== false;
  paint();
}

/** Starts following the web view's own media query; call once at start. */
export function installReduceTransparency() {
  if (installed) return;
  installed = true;
  media()?.addEventListener("change", paint);
  paint();
  // A filter the web view cannot paint is the other reason the glass goes solid. `CSS.supports`
  // answers for the property, not for the layer it is drawn on: on Linux, WebKitGTK needs
  // accelerated compositing, and the switches that take it away are only visible to the shell.
  if (isTauri())
    void shellCall<boolean>("webview_blur")
      .then(setShellCompositing)
      .catch(() => undefined);
}
