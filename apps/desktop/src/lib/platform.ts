// The desktop OS the app runs on, as `data-platform` on <html>, so index.css can fit text, glass
// and motion to each web view (docs/design.md › Platforms): WebKit on macOS, WebView2 on Windows,
// WebKitGTK on Linux. The app reads the web view's user agent: there is no OS plugin, and each
// web view names its OS there. A browser (the mock, `dev:web`) is designed against the macOS
// look, so it gets `mac` whatever it runs on.

export type Platform = "mac" | "windows" | "linux";

/** The platform for a user agent; `tauri` is whether the page runs in the app's own web view. */
export function platformFor(userAgent: string, tauri: boolean): Platform {
  if (!tauri) return "mac";
  if (/Windows/i.test(userAgent)) return "windows";
  if (/Linux|X11|FreeBSD/i.test(userAgent) && !/Android/i.test(userAgent)) return "linux";
  return "mac";
}

/** This page's platform. */
export function currentPlatform(): Platform {
  if (typeof window === "undefined") return "mac";
  return platformFor(navigator.userAgent, "__TAURI_INTERNALS__" in window);
}

/** Sets `data-platform` on <html>; call before the first render, so nothing paints in the wrong look. */
export function applyPlatform(): Platform {
  const platform = currentPlatform();
  document.documentElement.dataset.platform = platform;
  return platform;
}

/** Whether the platform paints blur at all: Windows and Linux are solid, decorative blur included. */
export function paintsBlur(platform: Platform = currentPlatform()): boolean {
  return platform === "mac";
}

/** The system file browser name in actions. */
export function fileManagerName(platform: Platform = currentPlatform()): string {
  return platform === "mac" ? "Finder" : platform === "windows" ? "Explorer" : "file manager";
}

/** The platform primary shortcut modifier. */
export function primaryModifier(
  event: { metaKey: boolean; ctrlKey: boolean },
  platform: Platform = currentPlatform(),
): boolean {
  return platform === "mac" ? event.metaKey : event.ctrlKey;
}

/** A wheel gesture that zooms a picture: Ctrl-wheel pinches on macOS. */
export function zoomGestureModifier(
  event: { metaKey: boolean; ctrlKey: boolean },
  platform: Platform = currentPlatform(),
): boolean {
  return primaryModifier(event, platform) || (platform === "mac" && event.ctrlKey);
}
