// The sidebar's view preferences (group by project, the Working shelf, one project only, which
// shelves are open), kept in localStorage and shared: the sidebar draws from them and ⌘K changes
// them (ui/CommandPalette.tsx), so both read one store rather than two copies of a flag.

import { useCallback, useMemo, useSyncExternalStore } from "react";

export const PREF = {
  rowLayout: "rustcode.sidebar.rowLayout",
  groupByProject: "rustcode.sidebar.groupByProject",
  workingSection: "rustcode.sidebar.workingSection",
  onlyProject: "rustcode.sidebar.onlyProject",
  folded: "rustcode.sidebar.folded",
} as const;

const listeners = new Set<() => void>();
/** Stands in for localStorage where it is unavailable (a private window). */
const memory = new Map<string, string>();

function rawPref(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return memory.get(key) ?? null;
  }
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
  try {
    localStorage.setItem(key, raw);
  } catch {
    memory.set(key, raw);
  }
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
