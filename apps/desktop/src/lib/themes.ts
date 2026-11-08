// design-lint: allow-file (the built-in themes' token values, mirrored from index.css)
// Themes (docs/themes.md, docs/design.md): the engine lists them
// (`_rustcode/themes`), and the app writes the active one's tokens onto
// :root as `--rc-*` variables, so a change applies live without
// re-rendering React. Without the engine methods the built-in themes below
// (the values in index.css) are all there is.

import { arr, type Json, obj, str } from "../acp/jsonrpc";

export const ThemeMethod = {
  list: "_rustcode/themes",
  set: "_rustcode/themes/set",
  create: "_rustcode/themes/new",
  changed: "_rustcode/themes/changed",
} as const;

/** The semantic tokens, as theme files and the engine spell them. */
export const TOKENS = [
  "bg",
  "sidebar",
  "surface",
  "surface_2",
  "surface_3",
  "border",
  "border_strong",
  "text",
  "text_muted",
  "text_faint",
  "accent",
  "accent_text",
  "danger",
  "success",
  "warning",
  "focus",
  "selection",
  "code_bg",
  "scrollbar",
] as const;

export type Token = (typeof TOKENS)[number];
export type ThemeMode = "dark" | "light";
export type Radius = "sharp" | "default" | "round";

export interface Theme {
  name: string;
  description: string;
  source: "builtin" | "user" | "project";
  path?: string;
  mode: ThemeMode;
  tokens: Record<Token, string>;
  radius: Radius;
}

export interface ThemesSnapshot {
  active: string;
  themes: Theme[];
}

/** `surface_2` → `--rc-surface-2`. */
export function cssName(token: Token): string {
  return `--rc-${token.replace(/_/g, "-")}`;
}

const DARK: Record<Token, string> = {
  bg: "#0b0b0c",
  sidebar: "#151517",
  surface: "#1d1d20",
  surface_2: "#242428",
  surface_3: "#2a2a2e",
  border: "#ffffff0a",
  border_strong: "#ffffff1c",
  text: "#ededed",
  text_muted: "#a1a1a1",
  text_faint: "#6b6b6b",
  accent: "#93a3e8",
  accent_text: "#0b0b0c",
  danger: "#f26d6d",
  success: "#4ec27a",
  warning: "#e5a54b",
  focus: "#93a3e8",
  selection: "#93a3e84d",
  code_bg: "#111113",
  scrollbar: "#ffffff26",
};

const LIGHT: Record<Token, string> = {
  bg: "#ffffff",
  sidebar: "#f7f7f8",
  surface: "#eaeaea",
  surface_2: "#ffffff",
  surface_3: "#e4e4e6",
  border: "#0000000a",
  border_strong: "#0000001f",
  text: "#171717",
  text_muted: "#5c5c5c",
  text_faint: "#9a9a9a",
  accent: "#2f6fe4",
  accent_text: "#ffffff",
  danger: "#d93d3d",
  success: "#1f9d55",
  warning: "#b7791f",
  focus: "#2f6fe4",
  selection: "#2f6fe433",
  code_bg: "#f6f6f7",
  scrollbar: "#00000026",
};

/** The app's own themes: the values index.css paints before any theme is applied. */
export const BUILTIN_THEMES: Theme[] = [
  {
    name: "rustcode",
    description: "RustCode dark",
    source: "builtin",
    mode: "dark",
    tokens: DARK,
    radius: "default",
  },
  {
    name: "light",
    description: "RustCode light",
    source: "builtin",
    mode: "light",
    tokens: LIGHT,
    radius: "default",
  },
];

const COLOR = /^#[0-9a-fA-F]{3,8}$/;

function parseTheme(value: Json | undefined): Theme | undefined {
  const o = obj(value);
  const name = str(o?.name);
  if (!o || !name) return undefined;
  const mode: ThemeMode = str(o.mode) === "light" ? "light" : "dark";
  const raw = obj(o.tokens) ?? {};
  const fallback = mode === "light" ? LIGHT : DARK;
  const tokens = {} as Record<Token, string>;
  for (const t of TOKENS) {
    const v = str(raw[t]);
    tokens[t] = v && COLOR.test(v) ? v : fallback[t];
  }
  const radius = str(o.radius);
  const source = str(o.source);
  return {
    name,
    description: str(o.description) ?? name,
    source: source === "user" || source === "project" ? source : "builtin",
    path: str(o.path),
    mode,
    tokens,
    radius: radius === "sharp" || radius === "round" ? radius : "default",
  };
}

export function parseThemes(value: Json): ThemesSnapshot {
  const root = obj(value);
  const themes = arr(root?.themes)
    .map(parseTheme)
    .filter((t): t is Theme => !!t);
  return { active: str(root?.active) ?? themes[0]?.name ?? "rustcode", themes };
}

/** The theme to paint in `mode`: the active one when it is that mode, else a built-in of it. */
export function resolveTheme(snapshot: ThemesSnapshot | null, active: string, mode: ThemeMode): Theme {
  const themes = snapshot?.themes.length ? snapshot.themes : BUILTIN_THEMES;
  const chosen = themes.find((t) => t.name === active);
  if (chosen?.mode === mode) return chosen;
  return (
    themes.find(
      (t) => t.source === "builtin" && t.mode === mode && BUILTIN_THEMES.some((b) => b.name === t.name),
    ) ?? BUILTIN_THEMES.find((t) => t.mode === mode)!
  );
}

const CACHE = "rustcode.themeTokens";

/**
 * Paints `theme` on :root: tokens as inline variables (they win over
 * index.css), `data-theme` for the mode and `data-radius` for the scale.
 * React does not re-render; everything reads the variables.
 */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): void {
  const builtin = BUILTIN_THEMES.find((b) => b.name === theme.name && b.source === theme.source);
  for (const t of TOKENS) {
    // A built-in theme is index.css itself: no inline values to go stale.
    if (builtin && builtin.tokens[t] === theme.tokens[t]) root.style.removeProperty(cssName(t));
    else root.style.setProperty(cssName(t), theme.tokens[t]);
  }
  root.dataset.radius = theme.radius;
  try {
    localStorage.setItem(CACHE, JSON.stringify(theme));
  } catch {
    // Not kept in a private window: the next start paints the default first.
  }
}

/** Paints the last applied theme before the first render, so a custom theme does not flash. */
export function applyCachedTheme(): void {
  try {
    const raw = localStorage.getItem(CACHE);
    const theme = raw ? parseTheme(JSON.parse(raw) as Json) : undefined;
    if (theme) applyTheme(theme);
  } catch {
    // Nothing cached.
  }
}
