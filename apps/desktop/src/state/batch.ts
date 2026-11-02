// Coalescing for the store. The engine sends one line per streamed chunk (and
// thousands while a thread replays); folding each one into the store and
// notifying every subscriber per line made React render per line. Lines are
// queued and applied once per frame inside `batch`, where state still updates
// at once (so `getState()` stays exact) but subscribers hear about it once.

import type { StoreApi } from "zustand";

type Listener<T> = (state: T, previous: T) => void;

export interface Batcher<T> {
  /** Called from the store's creator with its api, so subscribers can be deferred. Returns `{}` to spread. */
  attach: (store: StoreApi<T>) => Record<string, never>;
  /** Run `fn`; subscribers hear about every change it made once, at the end. */
  batch: (fn: () => void) => void;
}

export function createBatcher<T>(): Batcher<T> {
  let depth = 0;
  let api: StoreApi<T> | null = null;
  // The store's own subscribers, called by one subscription on the store, so a
  // change inside a batch costs one check however many components listen.
  const listeners = new Set<Listener<T>>();
  /** The state before the batch's first change; null when the batch changed nothing yet. */
  let before: { state: T } | null = null;

  const flush = () => {
    if (!api || !before) return;
    const previous = before.state;
    before = null;
    const state = api.getState();
    if (state === previous) return;
    for (const listener of listeners) listener(state, previous);
  };

  return {
    attach: (store) => {
      api = store;
      store.subscribe((state, previous) => {
        if (depth > 0) {
          before ??= { state: previous };
          return;
        }
        for (const listener of listeners) listener(state, previous);
      });
      store.subscribe = (listener) => {
        listeners.add(listener);
        return () => {
          listeners.delete(listener);
        };
      };
      return {};
    },
    batch: (fn) => {
      depth++;
      try {
        fn();
      } finally {
        depth--;
        if (depth === 0) flush();
      }
    },
  };
}

/**
 * Queues items and hands them to `apply` together: on the next animation frame
 * while the page is visible (so a frame renders once however many arrive), else
 * after `hiddenMs` (frames do not run in a hidden window).
 */
export function frameQueue<T>(
  apply: (items: T[]) => void,
  hiddenMs = 16,
): { push: (item: T) => void; flush: () => void } {
  let queue: T[] = [];
  let scheduled = false;
  const flush = () => {
    scheduled = false;
    if (!queue.length) return;
    const items = queue;
    queue = [];
    apply(items);
  };
  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    if (typeof document === "undefined") {
      queueMicrotask(flush); // Tests and workers: no frames to wait for.
      return;
    }
    if (document.visibilityState === "visible" && typeof requestAnimationFrame === "function") {
      // rAF alone stalls if the page hides before it fires; the timer is the backstop.
      const timer = setTimeout(flush, 100);
      requestAnimationFrame(() => {
        clearTimeout(timer);
        flush();
      });
    } else setTimeout(flush, hiddenMs);
  };
  return {
    push: (item) => {
      queue.push(item);
      schedule();
    },
    flush,
  };
}
