// Rows put away from "Needs you" without an answer (ui/Inbox.tsx). A failed turn
// nobody will retry, or a request the user would rather answer in the thread
// itself, leaves the list and its count; nothing about the thread changes, so the
// failure still shows in its transcript and the request still waits in its chat.
//
// A row is remembered by its key and by when it was put away, so the list is
// filtered without the thread. A request's and a link's key are the ask's own, so
// the next ask is a new row. A failure's key is only its thread's, so its moment
// decides: put away after it arrived, and the next failure — which arrives later —
// is a row again. Pure, and kept in app storage by state/store.ts.

import type { InboxItem } from "./inbox";

/** The rows put away, by key, to when; kept in app storage. */
export type InboxDismissals = Readonly<Record<string, number>>;

/** How many are kept: a scrollback, not a history, so app storage cannot grow forever. */
export const DISMISSALS_MAX = 200;

/** What a dismissal is decided from: the row's identity and moment, never its drawn content. */
type Dismissable = Pick<InboxItem, "key" | "kind" | "at">;

/**
 * Whether the row was put away as it stands. A failure is only the same failure it
 * was: one that arrives after it was put away is a new row, and one re-derived from
 * the transcript rather than the store arrives at the same moment or earlier.
 */
export function isDismissed(dismissals: InboxDismissals, item: Dismissable): boolean {
  const at = dismissals[item.key];
  if (at === undefined) return false;
  return item.kind !== "failed" || at >= item.at;
}

/** The rows put away at `now`, keeping the newest {@link DISMISSALS_MAX}. */
export function dismissRows(
  dismissals: InboxDismissals,
  items: readonly Dismissable[],
  now = Date.now(),
): InboxDismissals {
  if (!items.length) return dismissals;
  const next: Record<string, number> = { ...dismissals };
  for (const item of items) next[item.key] = now;
  const keys = Object.keys(next);
  if (keys.length <= DISMISSALS_MAX) return next;
  const kept = keys.sort((a, b) => (next[b] ?? 0) - (next[a] ?? 0)).slice(0, DISMISSALS_MAX);
  return Object.fromEntries(kept.map((key) => [key, next[key] ?? now]));
}

/** The rows put back, by key; a key that was not put away changes nothing. */
export function restoreRows(dismissals: InboxDismissals, keys: readonly string[]): InboxDismissals {
  const next = { ...dismissals };
  let changed = false;
  for (const key of keys) {
    if (!(key in next)) continue;
    delete next[key];
    changed = true;
  }
  return changed ? next : dismissals;
}
