// The app's appearance: tokens resolved from `index.css`, and a way to hear
// when they change (the Appearance setting or the system's light/dark).

import { useApp } from "../state/store";

export function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

export function isDark(): boolean {
  const theme = useApp.getState().theme;
  if (theme !== "system") return theme === "dark";
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
}

/** Calls `listener` after the resolved appearance may have changed (the new `data-theme` is applied first). */
export function onAppThemeChange(listener: () => void): () => void {
  const fire = () => requestAnimationFrame(() => listener());
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  media.addEventListener("change", fire);
  const unsubscribe = useApp.subscribe((s, prev) => {
    if (s.theme !== prev.theme) fire();
  });
  return () => {
    media.removeEventListener("change", fire);
    unsubscribe();
  };
}
