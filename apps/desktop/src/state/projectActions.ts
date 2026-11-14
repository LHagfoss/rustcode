// A project's quick actions: named shell commands (`bun run dev`, `cargo check --workspace`) the
// thread header's Actions menu and ⌘K run in the workspace terminal, in the thread's own folder.
// They belong to the project, not the thread, so a main checkout and its worktrees share one list
// (the key is the session's `repo`, else its folder). Kept in this app's storage; nothing is sent
// to the agent. One action may run when a worktree of the project is created, and one when a thread
// in such a worktree settles: `session/new` carries those to the engine, which runs them (`worktreeHooks`,
// rustcode/engine/src/project_hooks.rs). A repository's `.rustcode/actions.json` wins over them.
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
  /** Run when a worktree of the project is created (at most one action per project). */
  onCreate?: boolean;
  /** With `onCreate`: the thread's first turn waits until it exits. */
  wait?: boolean;
  /** Run in a thread's worktree when the thread settles (at most one action per project). */
  onSettle?: boolean;
}

export type ActionRow = Pick<ProjectAction, "name" | "command" | "onCreate" | "wait" | "onSettle"> & {
  id?: string;
};

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
      actions.push(withFlags({ id: entry.id, name: entry.name, command: entry.command }, entry, actions));
    }
    if (actions.length) out[project] = actions.slice(0, MAX_ACTIONS);
  }
  return out;
}

/** `action` with the hook flags `from` asks for, each kept only while no earlier action has it. */
function withFlags(
  action: ProjectAction,
  from: Partial<ProjectAction>,
  before: readonly ProjectAction[],
): ProjectAction {
  const out = { ...action };
  if (from.onCreate === true && !before.some((a) => a.onCreate)) {
    out.onCreate = true;
    if (from.wait === true) out.wait = true;
  }
  if (from.onSettle === true && !before.some((a) => a.onSettle)) out.onSettle = true;
  return out;
}

/** What `session/new` sends for a new worktree of the project: `{ create, settle }`, or none. */
export function worktreeHooks(
  actions: readonly ProjectAction[],
):
  | { create?: { name: string; command: string; wait: boolean }; settle?: { name: string; command: string } }
  | undefined {
  const create = actions.find((a) => a.onCreate);
  const settle = actions.find((a) => a.onSettle);
  if (!create && !settle) return undefined;
  return {
    ...(create ? { create: { name: create.name, command: create.command, wait: !!create.wait } } : {}),
    ...(settle ? { settle: { name: settle.name, command: settle.command } } : {}),
  };
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
  rows: readonly ActionRow[],
): ProjectActionsMap {
  const actions: ProjectAction[] = [];
  for (const row of rows) {
    const name = row.name.trim();
    const command = row.command.trim();
    if (!name || !command || actions.length >= MAX_ACTIONS) continue;
    const id = row.id && !actions.some((a) => a.id === row.id) ? row.id : actionId(name, actions);
    actions.push(withFlags({ id, name, command }, row, actions));
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

export function saveProjectActions(project: string, rows: readonly ActionRow[]): void {
  writePref(PROJECT_ACTIONS_KEY, withProjectActions(readActions(), project, rows));
}

// ---------------------------------------------------------------- the terminal queue

const pending = new Map<string, string[]>();
const waiting = new Set<(cwd: string) => void>();

/** Leaves a command for a terminal tab (state/terminalTabs.ts); the caller opens the panel. */
export function queueTerminalCommand(cwd: string, command: string): void {
  pending.set(cwd, [...(pending.get(cwd) ?? []), command]);
  for (const listener of waiting) listener(cwd);
}

/** The commands waiting for a terminal tab, once: taking them empties the queue. */
export function takeTerminalCommands(cwd: string): string[] {
  const commands = pending.get(cwd) ?? [];
  pending.delete(cwd);
  return commands;
}

/** Tells the terminals a command arrived for some tab; each takes its own. */
export function onTerminalCommand(listener: (cwd: string) => void): () => void {
  waiting.add(listener);
  return () => {
    waiting.delete(listener);
  };
}
