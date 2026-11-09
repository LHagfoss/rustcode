// The sidebar's view preferences (group by project, the Working shelf, one project only, which
// shelves are open), kept in localStorage and shared: the sidebar draws from them and ⌘K changes
// them (ui/CommandPalette.tsx), so both read one store rather than two copies of a flag.

import { useCallback, useEffect, useMemo, useSyncExternalStore } from "react";

/**
 * How a thread row is drawn: `compact` is one 32px line, with a quiet second line only when the
 * thread has a branch, a diff, a pull request or a stack to show; `comfortable` is the two-line
 * row the sidebar had before (title first, then where it runs and what its branch carries).
 */
export type RowLayout = "compact" | "comfortable";

/** One line per thread unless the user asked for the two-line rows. */
export const DEFAULT_ROW_LAYOUT: RowLayout = "compact";

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

/**
 * The row layout a stored value means, including the two this release retires: `current` (the
 * meta line above the title) and `title-first` (the title above it) were both two-line rows, and
 * only a user who picked one in Settings ever wrote them, so both keep that row as `comfortable`.
 * A store with no value at all was never chosen, and takes the new one-line default.
 */
export function rowLayoutOf(stored: unknown): RowLayout {
  if (stored === "compact" || stored === "comfortable") return stored;
  if (stored === "current" || stored === "title-first") return "comfortable";
  return DEFAULT_ROW_LAYOUT;
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

/** The migration has run: every row reads this hook, and the store is written back once. */
let migratedRowLayout = false;

/**
 * The sidebar's row layout, read through the migration above, and written back once so a store
 * still holding `current` or `title-first` is left in a value this release understands.
 */
export function useRowLayout(): readonly [RowLayout, (next: RowLayout) => void] {
  const [stored, setStored] = usePref<unknown>(PREF.rowLayout, DEFAULT_ROW_LAYOUT);
  const layout = rowLayoutOf(stored);
  useEffect(() => {
    if (migratedRowLayout) return;
    migratedRowLayout = true;
    if (stored !== layout) writePref(PREF.rowLayout, layout);
  }, [stored, layout]);
  const set = useCallback((next: RowLayout) => setStored(next), [setStored]);
  return [layout, set] as const;
}
