// Where a thread the list may draw goes: under the thread that started it (with
// no row of its own, its parent's stack of marks holds it), in Settled by
// itself when its parent is gone, or where every other thread goes. A thread an
// agent started is never a top-level row while its parent is in the list; the
// rule is one pure function so the sidebar and its tests agree on it
// (ui/Sidebar.tsx, docs/orchestrator.md).

import type { Link } from "./orchestration";

/** Where a thread sits in the sidebar. */
export type Placement =
  /** Under the thread that started it, with no row of its own: its parent's stack shows it. */
  | "nested"
  /** Its parent is not in the list: finished, it joins Settled by itself. */
  | "settled"
  /** A row where it would be anyway: it has no parent, or it is the user's, or it is live. */
  | "shown";

/** What the rule needs to know about one thread. */
export interface DelegatedThread {
  id: string;
  /**
   * Working or waiting on the user now (`statusOf`, or the sidebar's own flags):
   * a thread that needs the user is never hidden, whatever became of its parent.
   */
  live?: boolean;
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
 * The thread above `thread` in the delegation tree: the link that started it,
 * else the parent its metadata recorded. The link is preferred while it exists
 * because it is the live record; the metadata is what survives `prune`.
 */
export function parentOf(thread: DelegatedThread, links: readonly Link[]): string | undefined {
  return links.find((link) => link.started && link.target === thread.id)?.orchestrator ?? thread.parent?.id;
}

/**
 * Where `thread` goes, given the link book and the threads the list draws
 * (`listed`: the filtered ids, whatever section they end up in — a settled
 * parent is still listed, in Settled).
 *
 * - No parent at all: shown, as any thread.
 * - The user pinned it or messaged it: shown; it is their thread now, and it
 *   stays where they put it.
 * - A parent in the list: nested under it, with no row of its own (the parent's
 *   row shows its live delegates as a stack).
 * - A parent that is gone, filtered out or in another project: a finished
 *   thread joins Settled by itself; one that is working or waits on the user
 *   is shown, so nothing that needs them is hidden.
 */
export function topLevel(
  thread: DelegatedThread,
  links: readonly Link[],
  listed: ReadonlySet<string>,
): Placement {
  const parent = parentOf(thread, links);
  // A thread older releases started has only its parent's title: it is still
  // delegated, and still not a top-level thread.
  if (!parent && !thread.parent) return "shown";
  if (thread.pinned || thread.messaged) return "shown";
  if (parent && listed.has(parent)) return "nested";
  return thread.live ? "shown" : "settled";
}
