// The sidebar's view preferences (group by project, the Working shelf, one project only, which
// shelves are open, how dense the rows are), kept in localStorage and shared: the sidebar draws from them and ⌘K changes
// them (ui/CommandPalette.tsx), so both read one store rather than two copies of a flag.

import { useCallback, useMemo, useSyncExternalStore } from "react";
import { readStorage, writeStorage } from "../lib/storage";

export const PREF = {
  groupByProject: "rustcode.sidebar.groupByProject",
  workingSection: "rustcode.sidebar.workingSection",
  onlyProject: "rustcode.sidebar.onlyProject",
  folded: "rustcode.sidebar.folded",
  density: "rustcode.sidebar.density",
  toolbar: "rustcode.sidebar.toolbar",
  settleAfterDays: "rustcode.sidebar.settleAfterDays",
} as const;

export type SidebarSort = "manual" | "activity" | "recent" | "title";

export const SIDEBAR_SORTS: readonly { id: SidebarSort; label: string }[] = [
  { id: "manual", label: "Manual" },
  { id: "title", label: "Title" },
  { id: "activity", label: "Activity" },
  { id: "recent", label: "Last updated" },
];

export const DEFAULT_SIDEBAR_SORT: SidebarSort = "manual";
let sort: SidebarSort = DEFAULT_SIDEBAR_SORT;

export function sidebarSort(): SidebarSort {
  return sort;
}

export function applySidebarSort(next: SidebarSort) {
  sort = next;
  for (const listener of listeners) listener();
}

export function useSidebarSort() {
  return [useSyncExternalStore(subscribe, sidebarSort, () => DEFAULT_SIDEBAR_SORT)] as const;
}

/** How many idle days settle a thread on its own; 0 never does. */
export const SETTLE_AFTER_CHOICES: readonly { days: number; label: string }[] = [
  { days: 0, label: "Never" },
  { days: 1, label: "1 day" },
  { days: 3, label: "3 days" },
  { days: 7, label: "7 days" },
  { days: 14, label: "14 days" },
];

/** The default: three idle days, what the sidebar always did. */
export const SETTLE_AFTER_DEFAULT_DAYS = 3;

/** A stored day count, read leniently: anything but one of the choices is the default. */
export function settleAfterDays(raw: unknown): number {
  return SETTLE_AFTER_CHOICES.some((c) => c.days === raw) ? (raw as number) : SETTLE_AFTER_DEFAULT_DAYS;
}

/** The idle time that settles a thread, in ms; 0 never settles one. */
export function settleAfterMs(days: number): number {
  return days * 86_400_000;
}

/**
 * How much a thread row says: `detailed` is the three-line row (title, where it runs, the branch),
 * `compact` one line (status, harness, title, age) with the rest on its hover card.
 */
export type SidebarDensity = "detailed" | "compact";

export const SIDEBAR_DENSITIES: readonly { id: SidebarDensity; label: string }[] = [
  { id: "detailed", label: "Detailed" },
  { id: "compact", label: "Compact" },
];

/** A stored density, read leniently: anything but `compact` is the default, `detailed`. */
export function sidebarDensity(raw: unknown): SidebarDensity {
  return raw === "compact" ? "compact" : "detailed";
}

const listeners = new Set<() => void>();
/** Stands in for localStorage where it is unavailable (a private window). */
const memory = new Map<string, string>();

function rawPref(key: string): string | null {
  return readStorage(key) ?? memory.get(key) ?? null;
}

function parse<T>(raw: string | null, initial: T): T {
  if (raw === null) return initial;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return initial;
  }
}

/** A preference's current value, outside React. */
export function readPref<T>(key: string, initial: T): T {
  return parse(rawPref(key), initial);
}

export function writePref<T>(key: string, value: T): void {
  const raw = JSON.stringify(value);
  if (!writeStorage(key, raw)) memory.set(key, raw);
  notifySidebarPrefs();
}

/** Notifies subscribers that a sidebar preference changed in another window. */
export function notifySidebarPrefs(): void {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** `useState` over a stored preference: every user of the key sees a change at once. */
export function usePref<T>(key: string, initial: T) {
  const raw = useSyncExternalStore(
    subscribe,
    () => rawPref(key),
    () => null,
  );
  const value = useMemo(() => parse(raw, initial), [raw, initial]);
  const set = useCallback((next: T) => writePref(key, next), [key]);
  return [value, set] as const;
}

/** The sidebar's row density and its setter, shared by the sidebar, ⌘K and Settings. */
export function useSidebarDensity() {
  const [raw, set] = usePref<string>(PREF.density, "detailed");
  return [sidebarDensity(raw), set as (next: SidebarDensity) => void] as const;
}
