// Who a thread came from, and who it works with: the parent that started it
// (from the link book, or the thread's own metadata when the link was pruned),
// a distinct thread that messaged it, and the threads it started or messaged
// itself. Pure, so the details card and its tests share one answer
// (ui/ThreadDetails.tsx).

import { isActive, type Link, MAX_ACTIVE_LINKS } from "./orchestration";

/** A thread above this one, live or settled. */
export interface ParentRelation {
  /** The link, when one survived the book's pruning; absent for metadata only. */
  link?: Link;
  /** The thread above, when its id is known. */
  id?: string;
  title: string;
  /** It works for that thread right now. */
  live: boolean;
  /** It started this thread, rather than only messaging it. */
  started: boolean;
}

export interface ThreadLineage {
  /**
   * The thread that started this one, live or settled. It survives the link
   * book: a pruned link still leaves the parent recorded in the thread.
   */
  parent?: ParentRelation;
  /**
   * A different thread that messaged this one, shown beside the parent rather
   * than in place of it. Same thread as the parent: folded into it.
   */
  messagedBy?: ParentRelation;
  /** The threads this one started. */
  started: Link[];
  /** The threads this one messaged without starting. */
  messaged: Link[];
}

const title = (value: string | undefined) => value?.trim() ?? "";

/**
 * Two records are the same thread only when both name the same id. A title is
 * a label, not an identity: two threads can share one (or be renamed), and a
 * title-only parent record cannot prove that any link belongs to it.
 */
function sameThread(one?: string, other?: string): boolean {
  return !!one && one === other;
}

/** The newest link matching `pick`, or none. Ties keep the earlier one. */
function newest(links: readonly Link[], pick: (link: Link) => boolean): Link | undefined {
  return links.reduce<Link | undefined>(
    (best, link) => (pick(link) && (!best || link.createdAt > best.createdAt) ? link : best),
    undefined,
  );
}

/** A live link first (it is what the relation is doing now), else the newest one. */
function current(links: readonly Link[], pick: (link: Link) => boolean): Link | undefined {
  return newest(links, (link) => pick(link) && isActive(link.state)) ?? newest(links, pick);
}

/**
 * The relations of `id`, from every link (pruned or not) and the parent the
 * thread recorded in its own metadata.
 *
 * The parent is the thread a `started` link names, else the metadata's parent,
 * so a later message from another thread never hides who started this one. A
 * link marks the parent live (a follow-up) only when its id matches the
 * parent's id; with a title-only parent no link can be attributed to it, and
 * every inbound link is a separate `messagedBy`.
 */
export function threadLineage(
  id: string,
  links: readonly Link[],
  parentMeta?: { id?: string; title?: string },
): ThreadLineage {
  // A self-link (a broken record) is neither a parent nor work this thread did.
  const inbound = links.filter((link) => link.target === id && link.orchestrator !== id);
  const outbound = links
    .filter((link) => link.orchestrator === id && link.target !== id)
    .slice(-MAX_ACTIVE_LINKS * 2);
  // A thread that names itself as its parent is a broken record too.
  const meta = parentMeta?.id === id ? undefined : parentMeta;
  const startedLink = newest(inbound, (link) => !!link.started);
  const metaTitle = title(meta?.title);
  // A started link is the live record; the metadata is what survives pruning.
  const anchor: { id?: string; title: string } | undefined = startedLink
    ? { id: startedLink.orchestrator, title: startedLink.orchestratorTitle }
    : metaTitle || meta?.id
      ? { id: meta?.id, title: metaTitle || "Another thread" }
      : undefined;
  // The newest link from that same parent: a follow-up message keeps it live.
  const parentLink = anchor
    ? current(inbound, (link) => sameThread(anchor.id, link.orchestrator))
    : undefined;
  const parent: ParentRelation | undefined = anchor
    ? {
        link: parentLink,
        id: anchor.id,
        title: anchor.title,
        live: !!parentLink && isActive(parentLink.state),
        started: true,
      }
    : undefined;
  // A message from another thread is its own relation, not a replacement parent.
  const other = current(
    inbound,
    (link) => !link.started && !(parent && sameThread(parent.id, link.orchestrator)),
  );
  const messagedBy: ParentRelation | undefined = other
    ? {
        link: other,
        id: other.orchestrator,
        title: other.orchestratorTitle,
        live: isActive(other.state),
        started: false,
      }
    : undefined;
  return {
    parent,
    messagedBy,
    started: outbound.filter((link) => !!link.started),
    messaged: outbound.filter((link) => !link.started),
  };
}
