// What the Tauri shell adds beyond ACP. Each call has a browser fallback so
// `bun run dev:web` keeps working; the shell validates every argument.

import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";

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

/** The working tree's uncommitted changes, from git in the Tauri shell. */
export async function gitChanges(cwd: string): Promise<GitChanges | null> {
  if (!isTauri()) return null;
  return invoke<GitChanges>("git_changes", { cwd });
}

export function revealPath(path: string): void {
  if (isTauri()) void invoke("reveal_path", { path });
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
