// What the Tauri shell adds beyond ACP. Each call has a browser fallback so
// `bun run dev:web` keeps working; the shell validates every argument.

import { isTauri } from "../acp/transport";
import type { ServerInfo, SystemInfo } from "./diagnostics";
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

/** Runs a harness's own login command in Terminal. Only the shell can, and it accepts only known harness tools. `env` is a login directory, for a named account. */
export async function openInTerminal(command: string[], env?: [string, string][]): Promise<boolean> {
  if (!isTauri()) return false;
  await invoke("open_login_terminal", { command, env: env ?? null });
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

/** The project's files for `@` references; empty outside a git repository or the app. */
export async function projectFiles(cwd: string): Promise<string[]> {
  if (!isTauri() && !hasShell()) return [];
  try {
    return await invoke<string[]>("project_files", { cwd });
  } catch {
    return [];
  }
}

/** A Terminal window in a folder (macOS). */
export function openTerminalAt(path: string): Promise<void> {
  return invoke("open_terminal_at", { path });
}

export function revealPath(path: string): void {
  if (hasShell()) void invoke("reveal_path", { path }).catch((e) => console.warn("reveal_path", e));
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
 * A picture a reply shows: an absolute path, or one relative to the first of
 * `roots` (thread, worktree and project). The shell adds OS temporary roots
 * and validates the resolved path, image signature and size.
 */
export async function threadImage(path: string, roots: string[]): Promise<ImageFile> {
  return invoke<ImageFile>("thread_image", { path, roots });
}

/**
 * The picture on the system clipboard, read by the shell (PNG, at most 20 MB).
 * WebKitGTK on Linux can keep a copied screenshot to the native clipboard and
 * hand a paste event nothing, so the composer asks for it when the paste is
 * empty. Null when the clipboard holds no picture, or in the browser.
 */
export async function clipboardImage(): Promise<ImageFile | null> {
  if (!hasShell()) return null;
  try {
    return await invoke<ImageFile | null>("clipboard_image");
  } catch (error) {
    // The dev bridge and a shell older than the command answer it no such thing: nothing to paste.
    if (/Unknown command|does not run/.test(String(error))) return null;
    throw error;
  }
}

/** A drag of files over the window, where it is in CSS pixels (absent on `leave`). */
export interface FileDropEvent {
  type: "over" | "leave" | "drop";
  paths: string[];
  position?: { x: number; y: number };
}

/**
 * Files dropped on the window. The Tauri webview takes file drops itself and
 * reports paths; the browser delivers them as DOM drop events instead. The
 * position says what the drag is over, so the sidebar can take a folder
 * that the composer would otherwise attach.
 */
export async function onFileDrop(handler: (event: FileDropEvent) => void): Promise<() => void> {
  if (!isTauri()) return () => undefined;
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    // Tauri reports physical pixels; the page lays out in CSS pixels.
    const scale = window.devicePixelRatio || 1;
    const position = "position" in p ? { x: p.position.x / scale, y: p.position.y / scale } : undefined;
    if (p.type === "drop") handler({ type: "drop", paths: p.paths, position });
    else if (p.type === "leave") handler({ type: "leave", paths: [] });
    else handler({ type: "over", paths: p.type === "enter" ? p.paths : [], position });
  });
}

/** What a path on this disk is, as the shell finds it (`src-tauri/src/native.rs` `path_kinds`). */
export type PathKind = "folder" | "file" | "missing" | "denied";

/** Each path's kind, in order. Without the shell nothing can be read, so every one is missing. */
export async function pathKinds(paths: string[]): Promise<PathKind[]> {
  if (!hasShell() || !paths.length) return paths.map(() => "missing");
  return invoke<PathKind[]>("path_kinds", { paths });
}

// ---------------------------------------------------------------- Diagnostics (src-tauri/src/diagnostics.rs)

/** The server process as the shell runs it; null where no shell answers (the browser). */
export async function serverStatus(): Promise<ServerInfo | null> {
  if (!hasShell()) return null;
  return invoke<ServerInfo>("server_status").catch(() => null);
}

/** The OS, the web view and the app's version; null where no shell answers. */
export async function systemInfo(): Promise<SystemInfo | null> {
  if (!hasShell()) return null;
  return invoke<SystemInfo>("system_info").catch(() => null);
}

/** Each command's binary as the app's PATH finds it. */
export async function resolveBinaries(names: string[]): Promise<(string | null)[]> {
  if (!hasShell() || !names.length) return names.map(() => null);
  return invoke<(string | null)[]>("resolve_binaries", { names }).catch(() => names.map(() => null));
}

/** Shows the server's debug.log in the file manager. */
export function openLogsFolder(): Promise<void> {
  return invoke<void>("logs_folder_open");
}

/** Opens a thread's logs/debug.log (its folder when it has none yet); answers what opened. */
export function openThreadLog(id: string): Promise<string> {
  return invoke<string>("thread_log_open", { id });
}

/**
 * Saves text where the user picks: the save panel in the app, a download in
 * the browser. Answers where it went, or null when the user cancelled.
 */
export async function saveText(name: string, text: string): Promise<string | null> {
  if (hasShell()) return invoke<string | null>("diagnostics_save", { name, text });
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  return name;
}
