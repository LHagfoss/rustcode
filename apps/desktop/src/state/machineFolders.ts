// A paired machine's folders, as the project picker's Browse Folders… dialog reads them
// (ui/MachineFolderBrowser.tsx). A fresh server has no project and no thread to name one, and its
// disk is not this app's to open in a file dialog, so its engine lists one folder at a time
// (`_rustcode/project/browse`) and makes a new project folder (`_rustcode/project/create`).

import { arr, bool, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { clonedOn, requestOn } from "./host";

export interface MachineFolder {
  name: string;
  path: string;
  /** A repository of its own (`.git` in it). */
  isGit: boolean;
}

export interface FolderListing {
  path: string;
  /** `null` at the root. */
  parent: string | null;
  home: string | null;
  isGit: boolean;
  folders: MachineFolder[];
  /** More folders than the engine sends in one answer. */
  truncated: boolean;
}

/** How long one folder's listing may take; a network mount can be slow, not minutes. */
const BROWSE_MS = 15_000;
/** Making a folder and `git init` in it. */
const CREATE_MS = 60_000;
/** The longest folder name the engine's clone accepts, kept for a new one too. */
const MAX_NAME_CHARS = 120;

/** What `_rustcode/project/browse` answered. */
export function parseListing(value: unknown): FolderListing {
  const root = obj(value) ?? {};
  return {
    path: str(root.path) ?? "",
    parent: str(root.parent) ?? null,
    home: str(root.home) ?? null,
    isGit: bool(root.is_git) ?? false,
    folders: arr(root.folders).flatMap((entry) => {
      const folder = obj(entry);
      const name = str(folder?.name);
      const path = str(folder?.path);
      return name && path ? [{ name, path, isGit: bool(folder?.is_git) ?? false }] : [];
    }),
    truncated: bool(root.truncated) ?? false,
  };
}

/** A failure as the dialog says it; an engine from before these calls is named for an update. */
function folderError(error: unknown, machine: string): Error {
  if (error instanceof RpcError && error.isMethodNotFound) {
    return new Error(`Update RustCode on ${machine} to browse its folders from here.`);
  }
  if (error instanceof RpcError) {
    const data = typeof error.data === "string" ? error.data : "";
    return new Error(data || error.message);
  }
  return error instanceof Error ? error : new Error(String(error));
}

/** The folders directly in `path` on `host`; its home folder when `path` is omitted. */
export async function browseFolders(host: string, machine: string, path?: string): Promise<FolderListing> {
  try {
    const answer = await requestOn<JsonObject>(host, Method.projectBrowse, path ? { path } : {}, BROWSE_MS);
    return parseListing(answer);
  } catch (error) {
    throw folderError(error, machine);
  }
}

/** Why `name` cannot be a new folder's name, or `null` when it can. */
export function folderNameProblem(name: string): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "Type a name";
  if (trimmed === "." || trimmed === "..") return "Pick another name";
  if (/[/\\]/.test(trimmed)) return "One folder name, without slashes";
  if (trimmed.length > MAX_NAME_CHARS) return `At most ${MAX_NAME_CHARS} characters`;
  return null;
}

/** `name` inside `parent`, on whichever platform wrote `parent`. */
export function childPath(parent: string, name: string): string {
  const separator = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  return `${parent.replace(/[\\/]+$/, "")}${separator}${name.trim()}`;
}

/**
 * Make `parent/name` on `host` and `git init` it, and remember it as one of that machine's projects
 * so `clone_project` and `start_session` find it before a thread has run there. Answers the folder
 * as the engine resolved it.
 */
export async function createProjectFolder(
  host: string,
  machine: string,
  parent: string,
  name: string,
): Promise<string> {
  const problem = folderNameProblem(name);
  if (problem) throw new Error(problem);
  let answer: JsonObject;
  try {
    answer = await requestOn<JsonObject>(
      host,
      Method.projectCreate,
      { path: childPath(parent, name), init: true },
      CREATE_MS,
    );
  } catch (error) {
    throw folderError(error, machine);
  }
  const at = str(answer.path) ?? childPath(parent, name);
  const known = clonedOn.get(host) ?? [];
  if (!known.includes(at)) clonedOn.set(host, [...known, at]);
  return at;
}
