// Where a thread the list may draw goes: under the thread that started it (with
// no row of its own, its parent's stack of marks holds it), or a row where
// every other thread goes. A thread an agent started is never a top-level row
// while its parent exists at all — a project filter or a search cannot make it
// one — and it never loses its row because the parent is gone: the rule is one
// pure function so the sidebar and its tests agree on it (ui/Sidebar.tsx,
// docs/orchestrator.md).

import type { Link } from "./orchestration";

/** Where a thread sits in the sidebar. */
export type Placement =
  /** Under the thread that started it, with no row of its own: its parent's stack shows it. */
  | "nested"
  /** A row where it would be anyway: it has no parent, or its parent is gone, or it is the user's. */
  | "shown";

/** What the rule needs to know about one thread. */
export interface DelegatedThread {
  id: string;
  /** The user pinned it: it is their thread, in the Pinned section. */
  pinned?: boolean;
  /** The user sent it a message themselves: it is their thread, a row of its own. */
  messaged?: boolean;
  /**
   * The parent recorded in the thread's metadata when it was started
   * (`_meta.rustcode.parent`): `id` is absent for a thread an older release
   * started, where only the parent's title was recovered.
   */
  parent?: { id?: string; title?: string };
}

/**
 * The thread above `thread` in the delegation tree: the newest link that started
 * it, else the parent its metadata recorded. A later message from another thread
 * is not a start, and an older start link must not outvote a newer one. The link
 * is preferred while it exists because it is the live record; the metadata is
 * what survives `prune`.
 */
export function parentOf(thread: DelegatedThread, links: readonly Link[]): string | undefined {
  let newest: Link | undefined;
  for (const link of links) {
    if (!link.started || link.target !== thread.id) continue;
    if (!newest || link.createdAt > newest.createdAt) newest = link;
  }
  return newest?.orchestrator ?? thread.parent?.id;
}

/**
 * Where `thread` goes, given the link book and every thread the app knows
 * (`known`: all sessions, not the filtered list — a project filter or a search
 * changes what is drawn, never who a thread belongs to).
 *
 * - No parent at all: shown, as any thread.
 * - The user pinned it or messaged it: shown; it is their thread now, and it
 *   stays where they put it.
 * - A parent that exists: nested under it, with no row of its own (the parent's
 *   row shows its live delegates as a stack), whatever the filter or search
 *   shows.
 * - A parent that is gone from the app entirely: shown. Its row is the only
 *   place it can be seen, so it keeps one; hiding it would make the thread
 *   vanish.
 */
export function topLevel(
  thread: DelegatedThread,
  links: readonly Link[],
  known: ReadonlySet<string>,
): Placement {
  const parent = parentOf(thread, links);
  // A thread older releases started has only its parent's title: it is still
  // delegated, and still not a top-level thread.
  if (!parent && !thread.parent) return "shown";
  if (thread.pinned || thread.messaged) return "shown";
  if (parent && known.has(parent)) return "nested";
  return "shown";
}

/**
 * A delegated thread drawn as an indented row under the row it belongs to:
 * the open thread and every thread between it and its nearest row of its own.
 */
export interface SubRow {
  /** The delegated thread drawn as an indented row. */
  id: string;
  /** The row it hangs under: its parent, itself a row or another sub-row. */
  parent: string;
  /** 1 for the parent's own delegate, 2 for its delegate's, and so on. */
  depth: number;
}

/**
 * The delegated threads to draw as indented rows while `selected` is open: the
 * chain from the nearest row of its own down to it, deepest first, each hanging
 * under the one above (`ui/Sidebar.tsx` draws them under their parent's row and
 * folds them away when the user moves on).
 *
 * Empty when the selected thread is not delegated, or when the row at the top of
 * its chain is not drawn (`drawn`: the rows the list shows): hanging it under
 * nothing would hide it, so the caller gives it a row of its own instead.
 */
export function openChain(
  selected: string | null,
  parentOf: ReadonlyMap<string, string>,
  drawn: ReadonlySet<string>,
): SubRow[] {
  if (!selected || !parentOf.has(selected)) return [];
  const up: string[] = [];
  const seen = new Set<string>();
  let at = selected;
  while (parentOf.has(at) && !seen.has(at)) {
    seen.add(at);
    up.push(at);
    at = parentOf.get(at)!;
  }
  // A self-parenting record, a loop, or a top row the list filtered out: no chain to draw.
  if (seen.has(at) || !drawn.has(at)) return [];
  // `up` is deepest first: its last entry is the delegate directly under the row of its own.
  return up.map((id, index) => ({
    id,
    parent: index === up.length - 1 ? at : up[index + 1]!,
    depth: up.length - index,
  }));
}

/**
 * Whether a prompt is the user's own message, which makes a delegated thread
 * theirs and keeps it a row of its own (state/store.ts `runPrompt`). A resume
 * after a usage limit, a wake the app sends, a message from another thread or
 * any other automatic turn is not the user typing.
 */
export function userTypedPrompt(options: {
  link?: string;
  wake?: boolean;
  automatic?: boolean;
  limitResume?: boolean;
}): boolean {
  return !options.link && !options.wake && !options.automatic && !options.limitResume;
}
