// The project book: the folders the user works in. RustCode keeps no project
// table — a project is a folder a thread runs in, and `projectOf` derives it —
// so what a project tool needs beyond that lives here, in app storage: a
// display name, the harness and model new threads in that folder start on, and
// folders registered before any thread runs there. One store for the tools and
// the app's own "Add project folder", so the two never disagree. (Finding a
// project on another machine is `projects.ts`; this book is what the user set
// on this one.)

import { projectName } from "../acp/types";
import { cleanProjectPath } from "./orchestration";

/** What the app remembers about one project folder. */
export interface ProjectEntry {
  /** A display name the user gave; the folder's own name when unset. */
  name?: string;
  /** The harness a new thread in this folder starts on (an id from `list_harnesses`). */
  harness?: string;
  /** The model that harness starts on, when it differs from its default. */
  model?: string;
  /** When it was registered before any thread ran there, in ms. */
  registeredAt?: number;
}

/** Every project's entry, by normalized path. */
export type ProjectBook = Record<string, ProjectEntry>;

export const PROJECTS_STORAGE_KEY = "rustcode.projects";

/** The path a project is filed under: the same rule `projectOf` uses. */
export const projectKey = cleanProjectPath;

/** What the app keeps about the projects: an unreadable value reads as none. */
export function readProjectBook(): ProjectBook {
  try {
    const raw = localStorage.getItem(PROJECTS_STORAGE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const book: ProjectBook = {};
    for (const [path, entry] of Object.entries(parsed as Record<string, unknown>)) {
      if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
      const { name, harness, model, registeredAt } = entry as Record<string, unknown>;
      book[projectKey(path)] = {
        ...(typeof name === "string" && name.trim() ? { name: name.trim() } : {}),
        ...(typeof harness === "string" && harness.trim() ? { harness: harness.trim() } : {}),
        ...(typeof model === "string" && model.trim() ? { model: model.trim() } : {}),
        ...(typeof registeredAt === "number" && Number.isFinite(registeredAt) ? { registeredAt } : {}),
      };
    }
    return book;
  } catch {
    return {};
  }
}

function writeProjectBook(book: ProjectBook): void {
  try {
    localStorage.setItem(PROJECTS_STORAGE_KEY, JSON.stringify(book));
  } catch {
    // Storage can be unavailable (private window); the entry just does not persist.
  }
}

/** One project's entry, if the app has one. */
export function projectEntry(path: string): ProjectEntry | undefined {
  return readProjectBook()[projectKey(path)];
}

/** What a project is called: the user's display name, else the folder's own. */
export function projectLabel(path: string): string {
  return projectEntry(path)?.name ?? projectName(path);
}

/**
 * Register a folder, or change what is kept about it. An empty value clears
 * that field. A new harness with no model clears the model too: the pair the
 * book holds must be one a thread can start on, and the new harness's own
 * default is what it starts on. Returns the entry as stored.
 */
export function updateProjectEntry(path: string, patch: ProjectEntry): ProjectEntry {
  const key = projectKey(path);
  const book = readProjectBook();
  const entry: ProjectEntry = { ...book[key] };
  for (const field of ["name", "harness", "model"] as const) {
    const value = patch[field];
    if (value === undefined) continue;
    const trimmed = value.trim();
    if (trimmed) entry[field] = trimmed;
    else delete entry[field];
  }
  if (patch.harness !== undefined && patch.model === undefined) delete entry.model;
  if (patch.registeredAt !== undefined) entry.registeredAt = patch.registeredAt;
  // An entry with nothing left in it is no entry: the folder is unregistered.
  if (Object.keys(entry).length) book[key] = entry;
  else delete book[key];
  writeProjectBook(book);
  return entry;
}

/** Register a folder before any thread runs there (create and clone). */
export function registerProject(path: string, at = Date.now()): ProjectEntry {
  return updateProjectEntry(path, { registeredAt: at });
}

/**
 * Unregister a project: its entry goes, and so do its display name and
 * defaults. The folder and its files are never touched, and neither are its
 * threads.
 */
export function forgetProject(path: string): boolean {
  const key = projectKey(path);
  const book = readProjectBook();
  if (!(key in book)) return false;
  delete book[key];
  writeProjectBook(book);
  return true;
}

/** The folders registered before any thread ran there, newest first. */
export function registeredProjects(): { path: string; entry: ProjectEntry }[] {
  return Object.entries(readProjectBook())
    .filter(([, entry]) => entry.registeredAt !== undefined)
    .sort((a, b) => (b[1].registeredAt ?? 0) - (a[1].registeredAt ?? 0))
    .map(([path, entry]) => ({ path, entry }));
}

/** The harness and model a new thread in `path` starts on, when the project set them. */
export function projectChoice(path: string): { harness?: string; model?: string } {
  const entry = projectEntry(path);
  return { harness: entry?.harness, model: entry?.model };
}
