// A parent thread's children in the sidebar: what each one is doing, how the
// group reads in one line, and which of them stay as rows. Pure, so the
// sidebar and its tests share it (ui/Sidebar.tsx ChildThreads).

import type { ThreadStatus } from "../acp/v2";
import { isActive, type LinkState } from "./orchestration";

/** What a child is doing, as the sidebar says it. */
export type ChildState = "needs" | "working" | "finished" | "failed";

export interface ChildInfo {
  id: string;
  state: ChildState;
  /**
   * How far under the group's parent the row sits: 0 for its own child, 1 for a
   * grandchild shown flat in the same group (see `flatChildren`).
   */
  level?: 0 | 1;
  /**
   * The most urgent state among the threads this one started, when they are
   * drawn flat with it: a finished child whose own child still works stays a
   * row, so that grandchild is not folded away.
   */
  below?: ChildState;
}

/**
 * The one extra inset a grandchild's row takes: a step past the `pl-8` a child's
 * row starts at. A grandchild has no group of its own to open, so it is drawn
 * under the top-level parent's children, one level deeper, and not behind a
 * second chevron.
 */
export const GRANDCHILD_INSET = "pl-10";

/**
 * The rows one group draws: its children in the order they are drawn, and
 * right under the child that started it, a grandchild's row. A grandchild's
 * own group would be a second chevron under a row that is already small, so it
 * is shown flat with one level of extra inset instead of being lost — and
 * never above the thread it works for. A grandchild whose own parent is not in
 * this group (another project, a filtered list) stays out: its group is where
 * it belongs.
 */
export function flatChildren(
  children: readonly ChildInfo[],
  childrenOf: (id: string) => readonly ChildInfo[] | undefined,
): ChildInfo[] {
  const out: ChildInfo[] = [];
  for (const child of children) {
    out.push({ ...child, level: 0 });
    for (const grandchild of childrenOf(child.id) ?? []) out.push({ ...grandchild, level: 1 });
  }
  return out;
}

/**
 * A thread's state as its parent sees it: a thread waiting on you or working
 * is live, and anything else is finished as far as the group is concerned
 * (`review` and `idle` children are not running either).
 */
export function childState(status: ThreadStatus, failed = false): ChildState {
  if (status === "needs_input") return "needs";
  if (status === "running") return "working";
  return failed ? "failed" : "finished";
}

/**
 * A child as its parent's group reads it. A live link beats the thread's own status, because a
 * child that was just started or messaged has not reported `running` yet.
 */
export function childInfo(
  id: string,
  read: { status?: ThreadStatus; link?: LinkState; failed?: boolean },
): ChildInfo {
  const live =
    read.link && isActive(read.link)
      ? read.link === "needs_user"
        ? "needs_input"
        : "running"
      : (read.status ?? "idle");
  return { id, state: childState(live, read.failed) };
}

/**
 * The group's one line, for the parent row's title: `5 threads · 2 working ·
 * 1 needs you`, with nothing said about the parts that are zero.
 */
export function childSummary(children: ChildInfo[]): string {
  const n = children.length;
  const working = children.filter((c) => c.state === "working").length;
  const needs = children.filter((c) => c.state === "needs").length;
  const parts = [`${n} thread${n === 1 ? "" : "s"}`];
  if (working) parts.push(`${working} working`);
  if (needs) parts.push(`${needs} ${needs === 1 ? "needs" : "need"} you`);
  return parts.join(" · ");
}

/** The children that always stay as rows, whatever the group is doing. */
export const childVisible = (child: ChildInfo, selectedId: string | null) =>
  child.id === selectedId ||
  child.state === "needs" ||
  child.state === "working" ||
  child.below === "needs" ||
  child.below === "working";

/** More finished children than this fold behind the group's last line. */
export const FINISHED_FOLD_AT = 3;

/**
 * A group's rows: the children it lists, and the finished ones behind its last
 * line. That line appears only when folding buys something: finished children
 * beside rows that are still live, or more than three of them. A quiet parent
 * with two finished children simply shows the two.
 *
 * `collapsed` is the reader closing the group by hand: it still lists the rows
 * that cannot be closed away (one that wants you, one that works, the one they
 * opened), so the chevron always answers with something.
 */
export function foldFinished(
  children: ChildInfo[],
  selectedId: string | null,
  collapsed = false,
): { rows: ChildInfo[]; folded: ChildInfo[]; fold: boolean } {
  const live = children.filter((c) => childVisible(c, selectedId));
  const done = children.filter((c) => !childVisible(c, selectedId));
  const fold = done.length > 0 && (live.length > 0 || done.length > FINISHED_FOLD_AT);
  const rows = collapsed
    ? live.filter((c) => c.state === "needs" || c.state === "working" || c.id === selectedId)
    : live;
  return fold ? { rows, folded: done, fold } : { rows: collapsed ? rows : children, folded: [], fold };
}

/**
 * The children in the order to show them: what wants you, then what works,
 * then the one you have open, then the rest as they came.
 */
export function orderChildren(children: ChildInfo[], selectedId: string | null): ChildInfo[] {
  const rank = (c: ChildInfo) =>
    c.state === "needs" ? 0 : c.state === "working" ? 1 : c.id === selectedId ? 2 : 3;
  return children
    .map((child, i) => ({ child, i, rank: rank(child) }))
    .sort((a, b) => a.rank - b.rank || a.i - b.i)
    .map((x) => x.child);
}

/**
 * Whether the children show as rows at all. Otherwise the group is one line:
 * a parent you are not reading, with nothing under it that wants you, has no
 * reason to spend five rows of the sidebar. `collapsed` is the reader closing
 * the group by hand: it overrides the parent (or a finished child) being open,
 * so the chevron can always say what clicking it did. A child that wants you is
 * listed whatever else is true: it is the one row that cannot be closed away.
 */
export function childrenListed(
  children: ChildInfo[],
  open: {
    expanded: boolean;
    collapsed?: boolean;
    parentSelected: boolean;
    selectedId: string | null;
  },
): boolean {
  if (open.expanded) return true;
  if (children.some((c) => c.state === "needs")) return true;
  if (open.collapsed) return false;
  if (open.parentSelected) return true;
  return children.some((c) => c.id === open.selectedId);
}

/**
 * The groups the reader opened or closed, without the ones whose parent is no
 * longer in the list: a record that keeps every id it ever saw grows forever.
 */
export function pruneOpenGroups(
  groups: Record<string, boolean>,
  listed: Iterable<string>,
): Record<string, boolean> {
  const keep = new Set(listed);
  const next: Record<string, boolean> = {};
  for (const [id, open] of Object.entries(groups)) if (keep.has(id)) next[id] = open;
  return next;
}

/**
 * The group's dot: a child that failed while you were looking elsewhere (a
 * failure you have opened reads as finished). A child finishing says nothing
 * about its parent, so it is not flagged here at all.
 */
export const unseenChild = (children: ChildInfo[]) =>
  children.some((c) => c.state === "failed") ? ("failed" as const) : undefined;
