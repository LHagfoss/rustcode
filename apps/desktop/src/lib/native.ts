// What the Tauri shell adds beyond ACP. Each call has a browser fallback so
// `bun run dev:web` keeps working; the shell validates every argument.

import { isTauri } from "../acp/transport";
import { hasShell, shellCall } from "./shell";

function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return shellCall<T>(command, args);
}

export function openExternal(url: string): void {
  if (!/^(https?:|mailto:)/i.test(url)) return;
  if (isTauri()) void invoke("open_url", { url });
  else window.open(url, "_blank", "noopener");
}

/** A folder picker; in the browser, a prompt for a path. */
export async function pickFolder(start?: string): Promise<string | null> {
  if (isTauri()) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false, defaultPath: start });
    return typeof picked === "string" ? picked : null;
  }
  const typed = window.prompt("Project folder (absolute path)", start ?? "");
  return typed?.trim() || null;
}

/** Runs a harness's own login command in Terminal. Only the shell can, and it accepts only known harness tools. */
export async function openInTerminal(command: string[]): Promise<boolean> {
  if (!isTauri()) return false;
  await invoke("open_login_terminal", { command });
  return true;
}

export function canOpenTerminal(): boolean {
  return isTauri();
}

export interface GitFileChange {
  path: string;
  status: string;
  oldPath?: string;
  added: number;
  removed: number;
  binary: boolean;
}

export interface GitChanges {
  branch?: string;
  files: GitFileChange[];
  diff: string;
}

/**
 * The working tree's uncommitted changes, from git in the Tauri shell (the
 * mock answers too). With `base`, everything since the branch left it.
 */
export async function gitChanges(cwd: string, base?: string): Promise<GitChanges | null> {
  if (!isTauri() && !hasShell()) return null;
  try {
    return await invoke<GitChanges>("git_changes", base ? { cwd, base } : { cwd });
  } catch (error) {
    if (!isTauri() && /does not run|Unknown command/.test(String(error))) return null;
    throw error;
  }
}

/** A Terminal window in a folder (macOS). */
export function openTerminalAt(path: string): Promise<void> {
  return invoke("open_terminal_at", { path });
}

export function revealPath(path: string): void {
  if (isTauri()) void invoke("reveal_path", { path });
}

/**
 * Opens a text file (Markdown, TOML, JSON) in its editor, or a folder in
 * Finder. The shell accepts nothing else, so nothing here can run a program.
 */
export function openPath(path: string): void {
  if (hasShell()) void invoke("open_path", { path }).catch((e) => console.warn("open_path", e));
}

/** Opens a file a reply names in the user's editor, at its line when the editor takes one. */
export function openInEditor(path: string, line?: number, column?: number): Promise<void> {
  return invoke<void>("open_in_editor", { path, line: line ?? null, column: column ?? null });
}

/** Writes a new skill's SKILL.md from the template and answers its path. */
export function createSkill(name: string, scope: "user" | "project", cwd: string): Promise<string> {
  return invoke<string>("skill_create", { name, scope, cwd });
}

/** Files and folders to attach, from the open panel. Null in the browser, where the composer uses a file input. */
export async function pickFiles(start?: string): Promise<string[] | null> {
  if (!isTauri()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: true, directory: false, defaultPath: start });
  if (!picked) return [];
  return Array.isArray(picked) ? picked : [picked];
}

export interface ImageFile {
  name: string;
  mimeType: string;
  data: string;
}

/** An image file read by the shell (PNG, JPEG, GIF, WebP, at most 20 MB). */
export async function readImage(path: string): Promise<ImageFile> {
  return invoke<ImageFile>("read_image", { path });
}

/**
 * Files dropped on the window. The Tauri webview takes file drops itself and
 * reports paths; the browser delivers them as DOM drop events instead.
 */
export async function onFileDrop(
  handler: (event: { type: "over" | "leave" | "drop"; paths: string[] }) => void,
): Promise<() => void> {
  if (!isTauri()) return () => undefined;
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "drop") handler({ type: "drop", paths: p.paths });
    else if (p.type === "leave") handler({ type: "leave", paths: [] });
    else handler({ type: "over", paths: [] });
  });
}
