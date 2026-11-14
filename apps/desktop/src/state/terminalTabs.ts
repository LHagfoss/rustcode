// The workspace terminal's tabs (issue 3726): several shells per folder, each a PTY that keeps
// running while the panel is hidden or another thread is open. A project action opens in its own
// tab, named after it, and reuses that tab when it runs again. Pure transitions plus one store, so
// the panel, the actions menu and the tests share them.

import { create } from "zustand";

export interface TerminalTab {
  id: string;
  /** The folder the shell runs in. */
  cwd: string;
  /** What the tab says: the shell's own title once it sets one, else its number or action. */
  title: string;
  /** The project action this tab was opened for, so running it again reuses the tab. */
  action?: string;
}

export interface TerminalTabs {
  tabs: TerminalTab[];
  /** The tab shown for each folder. */
  active: Record<string, string>;
}

/** At most this many shells per folder: each one is a process. */
export const MAX_TABS_PER_FOLDER = 8;

let nextId = 0;
const newId = () => `term-${Date.now().toString(36)}-${nextId++}`;

export function tabsOf(state: TerminalTabs, cwd: string): TerminalTab[] {
  return state.tabs.filter((t) => t.cwd === cwd);
}

/** The tab a folder shows: the one picked last, else its first. */
export function activeTab(state: TerminalTabs, cwd: string): TerminalTab | undefined {
  const list = tabsOf(state, cwd);
  return list.find((t) => t.id === state.active[cwd]) ?? list[0];
}

/** The first free `Shell N` title in a folder. */
function shellTitle(list: readonly TerminalTab[]): string {
  const taken = new Set(list.map((t) => t.title));
  for (let n = 1; ; n++) if (!taken.has(`Shell ${n}`)) return `Shell ${n}`;
}

/** Adds a shell to `cwd` and shows it; a folder at its limit shows its last tab instead. */
export function withNewTab(
  state: TerminalTabs,
  cwd: string,
  init: { title?: string; action?: string } = {},
  id = newId(),
): { state: TerminalTabs; tab: TerminalTab } {
  const list = tabsOf(state, cwd);
  if (list.length >= MAX_TABS_PER_FOLDER) {
    const last = list[list.length - 1] as TerminalTab;
    return { state: { ...state, active: { ...state.active, [cwd]: last.id } }, tab: last };
  }
  const tab: TerminalTab = { id, cwd, title: init.title ?? shellTitle(list), action: init.action };
  return { state: { tabs: [...state.tabs, tab], active: { ...state.active, [cwd]: id } }, tab };
}

/** A folder has a shell to show: its first tab is made when it has none. */
export function withFolder(state: TerminalTabs, cwd: string): TerminalTabs {
  return tabsOf(state, cwd).length ? state : withNewTab(state, cwd).state;
}

/** The tab a project action runs in: the one it opened before, else a new one named after it. */
export function withActionTab(
  state: TerminalTabs,
  cwd: string,
  action: { id: string; name: string },
  id = newId(),
): { state: TerminalTabs; tab: TerminalTab } {
  const known = tabsOf(state, cwd).find((t) => t.action === action.id);
  if (known) return { state: { ...state, active: { ...state.active, [cwd]: known.id } }, tab: known };
  return withNewTab(state, cwd, { title: action.name, action: action.id }, id);
}

/** Closes a tab; its folder shows the tab beside it, and a folder left empty has none. */
export function withoutTab(state: TerminalTabs, id: string): TerminalTabs {
  const tab = state.tabs.find((t) => t.id === id);
  if (!tab) return state;
  const list = tabsOf(state, tab.cwd);
  const at = list.findIndex((t) => t.id === id);
  const tabs = state.tabs.filter((t) => t.id !== id);
  const active = { ...state.active };
  if (active[tab.cwd] === id || !active[tab.cwd]) {
    const next = list[at + 1] ?? list[at - 1];
    if (next && next.id !== id) active[tab.cwd] = next.id;
    else delete active[tab.cwd];
  }
  return { tabs, active };
}

/** Shows the tab `step` places from the active one in its folder, wrapping round. */
export function withStep(state: TerminalTabs, cwd: string, step: number): TerminalTabs {
  const list = tabsOf(state, cwd);
  const current = activeTab(state, cwd);
  if (!list.length || !current) return state;
  const at = list.indexOf(current);
  const next = list[(at + step + list.length) % list.length] as TerminalTab;
  return { ...state, active: { ...state.active, [cwd]: next.id } };
}

export function withTitle(state: TerminalTabs, id: string, title: string): TerminalTabs {
  const clean = title.trim();
  if (!clean) return state;
  return { ...state, tabs: state.tabs.map((t) => (t.id === id && !t.action ? { ...t, title: clean } : t)) };
}

// ---------------------------------------------------------------- the store

export const useTerminalTabs = create<TerminalTabs>(() => ({ tabs: [], active: {} }));

const apply = (fn: (s: TerminalTabs) => TerminalTabs) =>
  useTerminalTabs.setState(fn(useTerminalTabs.getState()));

export function ensureTerminalFolder(cwd: string): void {
  apply((s) => withFolder(s, cwd));
}

export function addTerminalTab(cwd: string): TerminalTab {
  const next = withNewTab(useTerminalTabs.getState(), cwd);
  useTerminalTabs.setState(next.state);
  return next.tab;
}

export function terminalTabForAction(cwd: string, action: { id: string; name: string }): TerminalTab {
  const next = withActionTab(useTerminalTabs.getState(), cwd, action);
  useTerminalTabs.setState(next.state);
  return next.tab;
}

export function closeTerminalTab(id: string): void {
  apply((s) => withoutTab(s, id));
}

export function showTerminalTab(cwd: string, id: string): void {
  apply((s) => ({ ...s, active: { ...s.active, [cwd]: id } }));
}

export function stepTerminalTab(cwd: string, step: number): void {
  apply((s) => withStep(s, cwd, step));
}

export function titleTerminalTab(id: string, title: string): void {
  apply((s) => withTitle(s, id, title));
}

// ---------------------------------------------------------------- output as message context

/** The most lines a terminal chip carries; the rest is cut with a note. */
export const TERMINAL_CONTEXT_LINES = 400;

const HEAD = /^Terminal output \((.*)\), (\d+) lines?/;

/**
 * Selected terminal output as the text a message carries: a fenced block under a line naming
 * the tab it came from, its last `TERMINAL_CONTEXT_LINES` lines when it is longer.
 */
export function terminalContext(selection: string, title: string): string {
  const lines = selection
    .replace(/\s+$/, "")
    .split("\n")
    .map((l) => l.replace(/\s+$/, ""));
  const cut = lines.length > TERMINAL_CONTEXT_LINES;
  const kept = cut ? lines.slice(-TERMINAL_CONTEXT_LINES) : lines;
  const head = `Terminal output (${title}), ${kept.length} ${kept.length === 1 ? "line" : "lines"}${
    cut ? `, the last of ${lines.length}` : ""
  }:`;
  const fence = kept.some((l) => l.startsWith("```")) ? "````" : "```";
  return `${head}\n${fence}\n${kept.join("\n")}\n${fence}`;
}

/** `Terminal · 24 lines` for a chip made from terminal output, else undefined. */
export function terminalContextLabel(text: string): string | undefined {
  const match = HEAD.exec(text);
  if (!match) return undefined;
  const n = Number(match[2]);
  return `Terminal · ${n} ${n === 1 ? "line" : "lines"}`;
}
