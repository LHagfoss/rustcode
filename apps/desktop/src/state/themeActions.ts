// Theme actions share the app store; read it only when called to keep module loading safe.
import { type AcpConnection, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import { BUILTIN_THEMES, parseThemes, ThemeMethod } from "../lib/themes";
import { type AppState, connectionFor, errorText, STORAGE, store, toast, useApp } from "./store";

const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((state: AppState) => Partial<AppState>)) => useApp.setState(patch);

// ---------------------------------------------------------------- themes

/** The project whose `.rustcode/themes` the engine lists too. */
function themeParams(): JsonObject {
  const s = get();
  const cwd = (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd;
  return cwd ? { cwd } : {};
}

/** Lists the engine's themes; an engine without them keeps the built-in ones. */
export async function refreshThemes(conn: AcpConnection): Promise<void> {
  try {
    const themes = parseThemes(await conn.request(ThemeMethod.list, themeParams(), 10_000));
    if (connectionFor("") !== conn) return;
    set((s) => ({ themes, features: { ...s.features, themes: true } }));
  } catch (error) {
    if (connectionFor("") !== conn) return;
    const missing = error instanceof RpcError && error.isMethodNotFound;
    set((s) => ({ themes: null, features: { ...s.features, themes: missing ? false : s.features.themes } }));
  }
}

/** `_rustcode/themes/changed`: a theme file changed, or the CLI picked another theme. */
export function onThemesChanged(params: JsonObject) {
  if (!Array.isArray(params.themes)) {
    const connection = connectionFor("");
    if (connection) void refreshThemes(connection);
    return;
  }
  const themes = parseThemes(params);
  const changed = themes.active !== get().themes?.active;
  set({ themes, features: { ...get().features, themes: true } });
  // Picked elsewhere (the CLI, another window): follow it, mode and all.
  if (changed) adoptTheme(themes.active);
}

function adoptTheme(name: string) {
  const theme = (get().themes?.themes ?? BUILTIN_THEMES).find((t) => t.name === name);
  if (!theme) return;
  set({ themeActive: name, theme: theme.mode });
  store(STORAGE.themeActive, name);
  store(STORAGE.theme, theme.mode);
}

/** Picks a theme: it applies at once, and the engine saves it when it has themes. */
export async function setActiveTheme(name: string): Promise<void> {
  adoptTheme(name);
  const connection = connectionFor("");
  // The theme changes under the pointer, and the picker shows the name: no toast to say so.
  if (!get().features.themes || !connection) return;
  try {
    const themes = parseThemes(await connection.request(ThemeMethod.set, { name, ...themeParams() }, 10_000));
    set({ themes });
  } catch (error) {
    toast(`Could not save the theme: ${errorText(error)}`, "error");
  }
}

/**
 * Makes a new theme file from `from` through the engine. Null when the
 * engine cannot (the UI then shows the CLI command instead).
 */
export async function createTheme(name: string, from: string): Promise<string | null> {
  const connection = connectionFor("");
  if (!get().features.themes || !connection) return null;
  try {
    const result = obj(await connection.request(ThemeMethod.create, { name, from }, 10_000));
    const current = connectionFor("");
    if (current) await refreshThemes(current);
    return str(result?.path) ?? "";
  } catch (error) {
    if (error instanceof RpcError && error.isMethodNotFound) return null;
    throw error;
  }
}
