// The threads a thread delegated to, as one tree: which thread it started,
// which of those started threads of their own, how each one goes, how long it
// worked and what it cost — with a parent's figures rolled up over its whole
// branch. The right panel's details draws it (ui/DelegateTree.tsx), and a Board
// card sums it in one line.
//
// Two numbers read differently on purpose:
// - time is work time: the clock runs while a thread works and stops while it
//   waits on an approval, so a question left open for ten minutes costs the
//   row nothing. A parent adds up every thread in its branch, which can exceed
//   the wall clock when a branch ran in parallel;
// - cost is the best-known cost of each thread's own turns
//   (`state/pricing.ts`), added up the same way. A cost that is unknown stays
//   unknown: it is never filled in with a zero.
//
// Pure (no React, no store), so the tree, the Board card and their tests share
// one answer.

import type { ThreadStatus } from "../acp/v2";
import { isActive, type Link, type LinkState, MAX_DEPTH } from "./orchestration";
import { type CostTotals, emptyCost, formatCost } from "./pricing";

/** How a node in the tree is doing, the same words the agent rows use. */
export type AgentState = "working" | "needs" | "done" | "failed" | "stopped";

export const LINK_AGENT_STATE: Record<LinkState, AgentState> = {
  queued: "working",
  working: "working",
  needs_user: "needs",
  done: "done",
  failed: "failed",
  cancelled: "stopped",
  interrupted: "stopped",
};

/** What a state is called in a row: `Working`, `Needs you`, `Done`. */
export const AGENT_WORD: Record<AgentState, string> = {
  working: "Working",
  needs: "Needs you",
  done: "Done",
  failed: "Failed",
  stopped: "Stopped",
};

/** The engine's own word for what a thread does now, when it reported one. */
export type ThreadActivity = ThreadStatus | undefined;

/** A link whose thread has settled: its clock is no longer running. */
const settled = (state: AgentState) => state === "done" || state === "failed" || state === "stopped";

/**
 * What a node shows. The thread's own live status wins over the link's record:
 * a link says what it was doing when it settled, while its thread may already
 * be working again (a follow-up, a wake) or waiting for the user. A link the
 * user stopped, or one that failed, keeps its word even if that thread is busy
 * with something else: this branch's work did not resume. An interrupted link
 * is a book that a restart could not resume (`state/host.ts`), so its thread's
 * own status speaks for it.
 */
export function agentStateOf(link: Link, activity?: ThreadActivity): AgentState {
  const recorded = LINK_AGENT_STATE[link.state];
  if (recorded === "failed" || link.state === "cancelled") return recorded;
  if (activity === "needs_input") return "needs";
  if (activity === "running") return "working";
  return recorded;
}

/** What the clock of one node counts. */
export interface DelegateTiming {
  /** When the work this node stands for started. */
  startedAt: number;
  /** When it settled, when it has. */
  endedAt?: number;
  /** It waits on the user since this moment, so its clock stays where it stopped. */
  waitingSince?: number;
  /** Time it spent waiting on the user before this wait. */
  waitedMs?: number;
}

/** A clock's reading: how much work time it holds, and whether it still runs. */
export interface Elapsed {
  ms: number;
  /** It counts up now, so the row must tick. */
  live: boolean;
  /** It waits on the user: the clock holds still until the answer comes. */
  frozen: boolean;
}

/**
 * How long a node worked, with every wait on the user left out. A wait that is
 * running now stops the clock at the moment it began; a wait a settled node was
 * in when it settled is taken out too, so answering late never adds time.
 */
export function elapsedOf(timing: DelegateTiming, now = Date.now()): Elapsed {
  const waiting = timing.waitingSince;
  const end = timing.endedAt ?? waiting ?? now;
  const waited = (timing.waitedMs ?? 0) + (waiting === undefined ? 0 : Math.max(0, end - waiting));
  return {
    ms: Math.max(0, end - timing.startedAt - waited),
    live: timing.endedAt === undefined && waiting === undefined,
    frozen: timing.endedAt === undefined && waiting !== undefined,
  };
}

/** One thread in the tree: the link that speaks for it, and the threads it started. */
export interface DelegateNode {
  /** The thread it opens. */
  id: string;
  /** The link that speaks for the relation now: live first, else the newest. */
  link: Link;
  /** Every link to it: a follow-up message is the same delegate, not another node. */
  messages: number;
  children: DelegateNode[];
}

/** The link of a group that speaks for the relation: live first, else the newest. */
function currentLink(links: readonly Link[]): Link {
  return links.reduce((best, link) => {
    const better = isActive(link.state) && !isActive(best.state);
    if (better) return link;
    if (isActive(best.state) && !isActive(link.state)) return best;
    return link.createdAt >= best.createdAt ? link : best;
  });
}

/**
 * The threads `root` started, and the threads those started, depth-limited the
 * way delegation itself is (`MAX_DEPTH`). One node per thread, however many
 * times it was messaged; a thread that appears twice in a broken book is kept
 * once. Ordered by when each delegation began, so the tree reads in the order
 * the parent handed the work out.
 */
export function delegateTree(root: string, links: readonly Link[], maxDepth = MAX_DEPTH): DelegateNode[] {
  const seen = new Set([root]);
  const build = (parent: string, depth: number): DelegateNode[] => {
    if (depth > maxDepth) return [];
    const groups = new Map<string, Link[]>();
    for (const link of links) {
      if (link.orchestrator !== parent || link.target === parent) continue;
      const list = groups.get(link.target);
      if (list) list.push(link);
      else groups.set(link.target, [link]);
    }
    const nodes: DelegateNode[] = [];
    for (const [id, group] of groups) {
      // A thread it only messaged is not a delegate: that relation reads on its own row.
      if (seen.has(id) || !group.some((link) => !!link.started)) continue;
      seen.add(id);
      nodes.push({ id, link: currentLink(group), messages: group.length, children: build(id, depth + 1) });
    }
    return nodes.sort((one, other) => one.link.createdAt - other.link.createdAt);
  };
  return build(root, 1);
}

/** Every thread in a tree, however deep. */
export function delegateCount(nodes: readonly DelegateNode[]): number {
  return nodes.reduce((sum, node) => sum + 1 + delegateCount(node.children), 0);
}

/** What the tree reads of the app for one thread. */
export interface DelegateRead {
  now: number;
  /** The thread's name: its live title, else the link's. */
  title: (id: string, link: Link) => string | undefined;
  /** The harness it runs on. */
  harness: (id: string, link: Link) => string | undefined;
  /** What its own thread does now, when the engine reported it. */
  activity: (id: string) => ThreadActivity;
  /** When the approval it waits on was asked, when the app knows it. */
  waitingSince: (id: string, link: Link) => number | undefined;
  /** Its own best-known cost, without the threads under it. */
  cost: (id: string) => CostTotals | undefined;
}

/** What one thread and everything under it is and cost. */
export interface BranchTotal {
  /** The branch's cost; absent when no thread in it has a known cost. */
  usd?: number;
  /** Some part of `usd` is a list-price estimate rather than a reported number. */
  estimated: boolean;
  /** Work time across the branch: each thread's own, waits on the user left out. */
  ms: number;
  /** A thread in the branch is working, so `ms` still grows. */
  live: boolean;
  /** Nothing in the branch works or waits: it is finished. */
  finished: boolean;
  counts: Record<AgentState, number>;
  threads: number;
}

const NO_COUNTS = (): Record<AgentState, number> => ({
  working: 0,
  needs: 0,
  done: 0,
  failed: 0,
  stopped: 0,
});

/** One thread in the tree, ready to draw, with its branch added up. */
export interface DelegateView {
  id: string;
  link: Link;
  title: string;
  /** The first line of what it was asked, for the hover tip. */
  task?: string;
  harness?: string;
  state: AgentState;
  /** Its own clock and cost, without the threads under it. */
  elapsed: Elapsed;
  cost: CostTotals;
  /** Its own figures and every thread under it. */
  total: BranchTotal;
  children: DelegateView[];
}

/** The first line of a task, without the harness's own framing. */
export function taskLine(message: string | undefined): string | undefined {
  const line = message
    ?.split("\n")
    .map((text) => text.trim())
    .find((text) => text.length > 0);
  if (!line) return undefined;
  const plain = line
    .replace(/^[>\-*+\s]+/, "")
    .replace(/[*`]/g, "")
    .trim();
  return plain || undefined;
}

/** Nothing known yet: a branch no thread has been added to. */
export function emptyBranch(): BranchTotal {
  return {
    usd: undefined,
    estimated: false,
    ms: 0,
    live: false,
    finished: true,
    counts: NO_COUNTS(),
    threads: 0,
  };
}

/** Two branches added: their costs only when one side knows one, their work time always. */
export function addBranch(one: BranchTotal, other: BranchTotal): BranchTotal {
  const usd =
    one.usd === undefined && other.usd === undefined ? undefined : (one.usd ?? 0) + (other.usd ?? 0);
  const counts = NO_COUNTS();
  for (const key of Object.keys(counts) as AgentState[]) counts[key] = one.counts[key] + other.counts[key];
  return {
    usd,
    estimated: usd !== undefined && (one.estimated || other.estimated),
    ms: one.ms + other.ms,
    live: one.live || other.live,
    finished: counts.working === 0 && counts.needs === 0,
    counts,
    threads: one.threads + other.threads,
  };
}

/** One thread's own figures, before the threads under it: a branch of one. */
export function ownBranch(state: AgentState, elapsed: Elapsed, cost: CostTotals): BranchTotal {
  const counts = NO_COUNTS();
  counts[state] = 1;
  return {
    usd: cost.usd,
    estimated: cost.usd !== undefined && cost.estimatedUsd !== undefined,
    ms: elapsed.ms,
    live: elapsed.live,
    finished: state !== "working" && state !== "needs",
    counts,
    threads: 1,
  };
}

/** One node's branch: its own figures, plus every thread under it. */
export function branchTotal(own: BranchTotal, children: readonly DelegateView[]): BranchTotal {
  return children.reduce((sum, child) => addBranch(sum, child.total), own);
}

/**
 * The tree of `root` with everything the row and the Board card need. A link's
 * times are the delegation's own: when it started working (`startedAt`, set per
 * turn), when it settled, and the waits on the user it recorded. A node whose
 * thread runs a later turn has no settled end, so its clock keeps running from
 * that turn's start.
 */
export function delegateViews(nodes: readonly DelegateNode[], read: DelegateRead): DelegateView[] {
  const view = (node: DelegateNode): DelegateView => {
    const state = agentStateOf(node.link, read.activity(node.id));
    const startedAt = node.link.startedAt ?? node.link.createdAt;
    const elapsed = elapsedOf(
      {
        startedAt,
        endedAt: settled(state) ? (node.link.settledAt ?? startedAt) : undefined,
        waitingSince: state === "needs" ? read.waitingSince(node.id, node.link) : undefined,
        waitedMs: node.link.waitedMs,
      },
      read.now,
    );
    const cost = read.cost(node.id) ?? emptyCost();
    const task = taskLine(node.link.message);
    // What needs the user, then what works, then what ended: a branch's live
    // threads must not sink under its finished ones.
    const children = node.children.map(view).sort(byUrgency);
    const total = branchTotal(ownBranch(state, elapsed, cost), children);
    return {
      id: node.id,
      link: node.link,
      title: read.title(node.id, node.link) ?? task ?? "Thread",
      task,
      harness: read.harness(node.id, node.link),
      state,
      elapsed,
      cost,
      total,
      children,
    };
  };
  return nodes.map(view).sort(byUrgency);
}

/** How soon a node's row should be read: what needs the user, then what works. */
const URGENCY: Record<AgentState, number> = { needs: 0, working: 1, failed: 2, done: 3, stopped: 4 };

/** The order rows are drawn in under one parent: the most urgent first, then the newest. */
function byUrgency(one: DelegateView, other: DelegateView): number {
  return URGENCY[one.state] - URGENCY[other.state] || other.link.createdAt - one.link.createdAt;
}

/** Every view in a tree, however deep, parents before their children. */
export function flattenViews(views: readonly DelegateView[]): DelegateView[] {
  return views.flatMap((view) => [view, ...flattenViews(view.children)]);
}

/** The branch of a whole tree: what a Board card sums under its row. */
export function forestTotal(views: readonly DelegateView[]): BranchTotal {
  return views.reduce((sum, view) => addBranch(sum, view.total), emptyBranch());
}

/** `2 working · 1 needs you`, else `2 failed`, else `done`: what a branch is doing. */
export function branchWord(total: BranchTotal): string {
  const parts: string[] = [];
  if (total.counts.working) parts.push(`${total.counts.working} working`);
  if (total.counts.needs)
    parts.push(`${total.counts.needs} ${total.counts.needs === 1 ? "needs" : "need"} you`);
  if (parts.length) return parts.join(" · ");
  if (total.counts.failed) return `${total.counts.failed} failed`;
  if (total.counts.stopped) return `${total.counts.stopped} stopped`;
  return "done";
}

/** What the strip above the composer says of a branch, one header wide. */
export interface DelegateBar {
  /** Threads working now; a queued one reads as working, as its own row does. */
  working: number;
  /** Threads waiting on the user. */
  needs: number;
  failed: number;
  /** Every thread in the branch, working and finished. */
  threads: number;
  /**
   * How long the longest of the working threads has run, by the wall clock, so
   * it ticks up once a second. Absent when nothing works: a branch's summed
   * work time is not a duration anyone can read.
   */
  longestMs?: number;
  /** What the header's mark is tinted by. */
  state: AgentState;
  /** The count itself: `34 threads`, or `34 delegates done` once nothing works. */
  label: string;
  /** What follows the count: `5 working · 2 need you`, `2 need you`, `1 failed`; empty when done. */
  status: string;
}

/**
 * A branch as the strip above the composer reads it: how many threads work now
 * and how long the longest has been at it (wall clock, never the branch's summed
 * work time), and once nothing works, one quiet count of what is done. The
 * branch's history is not a status: 34 threads that finished over a day are 34
 * done delegates, not `34 threads · 6h 34m`.
 */
export function branchBar(
  total: BranchTotal,
  /** When each thread working now started its turn (`link.startedAt`, else when it was made). */
  runningSince: readonly number[],
  now = Date.now(),
): DelegateBar {
  const { working, needs, failed } = total.counts;
  const longestMs = working && runningSince.length ? now - Math.min(...runningSince) : undefined;
  const state: AgentState = needs
    ? "needs"
    : working
      ? "working"
      : failed
        ? "failed"
        : total.counts.stopped
          ? "stopped"
          : "done";
  const count = `${total.threads} ${total.threads === 1 ? "thread" : "threads"}`;
  const parts: string[] = [];
  if (working) parts.push(`${working} working`);
  if (needs) parts.push(`${needs} ${needs === 1 ? "needs" : "need"} you`);
  if (parts.length) {
    return {
      working,
      needs,
      failed,
      threads: total.threads,
      longestMs,
      state,
      label: count,
      status: parts.join(" · "),
    };
  }
  // Nothing works and nothing waits: a failure is still news, but a finished branch only a count.
  if (failed) {
    return {
      working,
      needs,
      failed,
      threads: total.threads,
      state,
      label: count,
      status: `${failed} failed`,
    };
  }
  return {
    working,
    needs,
    failed,
    threads: total.threads,
    state,
    label: `${total.threads} ${total.threads === 1 ? "delegate" : "delegates"} done`,
    // The label already says it: a finished branch has no second word in its header.
    status: "",
  };
}

/**
 * A time as a row shows it: `2m 04s`, or `—` while it is under a second. The
 * parts are padded, so a live clock does not jump a pixel a second under it.
 */
export function timeLabel(ms: number): string {
  if (ms < 1000) return "—";
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/** A cost as a row shows it: `$0.31`, `≈$0.31` when part is estimated, or `—` when unknown. */
export function costLabel(usd: number | undefined, estimated = false): string {
  if (usd === undefined) return "—";
  return `${estimated ? "≈" : ""}${formatCost(usd)}`;
}
