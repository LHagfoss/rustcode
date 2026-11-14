// A project's quick actions: named shell commands (`bun run dev`, `cargo check --workspace`) the
// thread header's Actions menu and ⌘K run in the workspace terminal, in the thread's own folder.
// They belong to the project, not the thread, so a main checkout and its worktrees share one list
// (the key is the session's `repo`, else its folder). Kept in this app's storage; nothing is sent
// to the engine or the agent.
//
// The terminal side is a small queue: `runInTerminal` opens the panel and leaves the command for
// the terminal of that folder, which writes it once its shell is up (ui/TerminalPanel.tsx).

import { useMemo } from "react";
import { readPref, usePref, writePref } from "./sidebarPrefs";

export interface ProjectAction {
  /** Stable within its project: the menu's key and what an edit replaces. */
  id: string;
  name: string;
  command: string;
}

/** Every project's actions, by project key. */
export type ProjectActionsMap = Record<string, ProjectAction[]>;

export const PROJECT_ACTIONS_KEY = "rustcode.projectActions";

/** Enough for a menu; a list longer than this is a script's job. */
export const MAX_ACTIONS = 20;

const EMPTY: ProjectActionsMap = {};
const NONE: ProjectAction[] = [];

/** The project a thread's actions are filed under: its repository when the engine named one. */
export function projectKey(cwd: string, repo?: string): string {
  return (repo || cwd).replace(/[\\/]+$/, "") || cwd;
}

function isAction(value: unknown): value is ProjectAction {
  if (!value || typeof value !== "object") return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.id === "string" &&
    typeof v.name === "string" &&
    typeof v.command === "string" &&
    v.name.trim() !== "" &&
    v.command.trim() !== ""
  );
}

/** What a stored value means: malformed entries dropped, ids unique per project, the cap kept. */
export function sanitizeProjectActions(raw: unknown): ProjectActionsMap {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return EMPTY;
  const out: ProjectActionsMap = {};
  for (const [project, list] of Object.entries(raw as Record<string, unknown>)) {
    if (!Array.isArray(list)) continue;
    const seen = new Set<string>();
    const actions: ProjectAction[] = [];
    for (const entry of list) {
      if (!isAction(entry) || seen.has(entry.id)) continue;
      seen.add(entry.id);
      actions.push(entry);
    }
    if (actions.length) out[project] = actions.slice(0, MAX_ACTIONS);
  }
  return out;
}

/** An id for a new action from its name, unique among the project's: `dev`, `dev-2`. */
export function actionId(name: string, taken: readonly ProjectAction[]): string {
  const base =
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40) || "action";
  const ids = new Set(taken.map((a) => a.id));
  if (!ids.has(base)) return base;
  let n = 2;
  while (ids.has(`${base}-${n}`)) n += 1;
  return `${base}-${n}`;
}

/**
 * The project's list after an edit from the editor: rows with a name and a command, trimmed, in
 * the order given; a row keeps its id, a new row gets one. An empty list removes the project.
 */
export function withProjectActions(
  map: ProjectActionsMap,
  project: string,
  rows: readonly { id?: string; name: string; command: string }[],
): ProjectActionsMap {
  const actions: ProjectAction[] = [];
  for (const row of rows) {
    const name = row.name.trim();
    const command = row.command.trim();
    if (!name || !command || actions.length >= MAX_ACTIONS) continue;
    const id = row.id && !actions.some((a) => a.id === row.id) ? row.id : actionId(name, actions);
    actions.push({ id, name, command });
  }
  const next = { ...map };
  if (actions.length) next[project] = actions;
  else delete next[project];
  return next;
}

function readActions(): ProjectActionsMap {
  return sanitizeProjectActions(readPref<unknown>(PROJECT_ACTIONS_KEY, null));
}

/** The project's actions, live: every menu and ⌘K see an edit at once. */
export function useProjectActions(project: string | undefined): ProjectAction[] {
  const [raw] = usePref<unknown>(PROJECT_ACTIONS_KEY, null);
  return useMemo(() => (project ? (sanitizeProjectActions(raw)[project] ?? NONE) : NONE), [raw, project]);
}

/** The project's actions, outside React (⌘K builds its list on open). */
export function projectActions(project: string): ProjectAction[] {
  return readActions()[project] ?? NONE;
}

export function saveProjectActions(
  project: string,
  rows: readonly { id?: string; name: string; command: string }[],
): void {
  writePref(PROJECT_ACTIONS_KEY, withProjectActions(readActions(), project, rows));
}

// ---------------------------------------------------------------- the terminal queue

const pending = new Map<string, string[]>();
const waiting = new Set<(cwd: string) => void>();

/** Leaves a command for the terminal of `cwd`; the caller opens the panel. */
export function queueTerminalCommand(cwd: string, command: string): void {
  pending.set(cwd, [...(pending.get(cwd) ?? []), command]);
  for (const listener of waiting) listener(cwd);
}

/** The commands waiting for the terminal of `cwd`, once: taking them empties the queue. */
export function takeTerminalCommands(cwd: string): string[] {
  const commands = pending.get(cwd) ?? [];
  pending.delete(cwd);
  return commands;
}

/** Tells a terminal a command arrived for some folder; it takes its own. */
export function onTerminalCommand(listener: (cwd: string) => void): () => void {
  waiting.add(listener);
  return () => {
    waiting.delete(listener);
  };
}
