// Reduce transparency: glass turns solid (index.css `:root.reduce-transparency`). Four things ask
// for it, and the class on <html> is on when any does: the user's own switch (Settings ›
// Appearance), the system's request (macOS, through the shell; the web view's
// `prefers-reduced-transparency` elsewhere), and a web view that cannot blur behind a surface,
// where a translucent pane without its blur is text over text (docs/design.md › Platforms).

const QUERY = "(prefers-reduced-transparency: reduce)";

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
    reducesTransparency({ user, system, media: media()?.matches ?? false, blur: supportsBlur() }),
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

/** Starts following the web view's own media query; call once at start. */
export function installReduceTransparency() {
  if (installed) return;
  installed = true;
  media()?.addEventListener("change", paint);
  paint();
}
