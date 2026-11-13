// The diff layout the Changes panel and Review share: unified or side by side.
// One setting in app storage, so both views open the same way, and one width
// rule, measured on the view's own column, so side by side is never drawn in a
// column too narrow to read it.

import { type RefObject, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { observeSize } from "./resize";
import { readStorage, writeStorage } from "./storage";

export type DiffMode = "unified" | "split";

/** Side by side needs two readable columns: narrower than this, the panes stay unified. */
export const SPLIT_MIN_WIDTH = 900;

const KEY = "rustcode.diffMode";
const listeners = new Set<() => void>();

function read(): DiffMode {
  return readStorage(KEY) === "split" ? "split" : "unified";
}

let mode: DiffMode | null = null;

function current(): DiffMode {
  mode ??= read();
  return mode;
}

/** The layout the user picked, whether or not the view can draw it. */
export function diffMode(): DiffMode {
  return current();
}

export function setDiffMode(next: DiffMode) {
  if (next === current()) return;
  mode = next;
  // Not kept in a private window; the session still remembers it.
  writeStorage(KEY, next);
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The picked layout, re-rendering when the other view changes it. */
export function useDiffMode(): DiffMode {
  return useSyncExternalStore(subscribe, current, current);
}

/**
 * The width of the column a view draws its diffs in, kept up to date as the
 * panel is dragged, the window resized or the file tree comes and goes.
 */
export function useDiffWidth<T extends HTMLElement>(): [RefObject<T | null>, number] {
  const ref = useRef<T>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setWidth(el.clientWidth);
    measure();
    return observeSize(el, measure);
  }, []);
  return [ref, width];
}

/** What a view draws: the picked layout, or unified until its column is wide enough. */
export function useDiffLayout(width: number): { mode: DiffMode; picked: DiffMode; wide: boolean } {
  const picked = useDiffMode();
  const wide = width >= SPLIT_MIN_WIDTH;
  return { mode: picked === "split" && wide ? "split" : "unified", picked, wide };
}
