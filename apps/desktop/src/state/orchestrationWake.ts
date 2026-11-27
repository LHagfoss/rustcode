// Waking a parent thread when its delegates settle, stall or ask: which wakes are due, what they say, and the links they mark delivered. Pure: no store, no engine.
import type { Json } from "../acp/jsonrpc";
import {
  advanceLink,
  capped,
  eventKey,
  isActive,
  type Link,
  type LinkBook,
  type LinkEvent,
  type LinkState,
  MAX_LINK_EVENTS,
  STALL_NOTICE_MS,
  WAKE_ANSWER_CHARS,
  WAKE_COALESCE_MS,
} from "./orchestration";
import type { PermissionMode } from "./permissions";
import { type ChildActivity, formatDuration, type QueuedPrompt, withWakeMark } from "./thread";

// ---------------------------------------------------------------- waking a parent

/** One item of a child's transcript, as the idle summary reads it. */
export interface ActivityItem {
  kind: string;
  at?: number;
  startedAt?: number;
  endedAt?: number;
  title?: string;
  toolName?: string;
  /** The tool call's input, for the command a stall notice names. */
  input?: Json;
}

/** What a child was last doing, and how many turns it finished (`wait_session` timeout). */
export interface LinkActivity {
  lastAt?: number;
  lastTool?: string;
  /** The last tool's command, when it reported one. */
  lastCommand?: string;
  /** The processes below the child's harness, when the engine sampled any (`ThreadState.children`). */
  children?: { processes: number; cpuMs: number; busiest?: string };
  turns: number;
}

/** A tool call's command, when its input names one. */
function commandOf(input: Json | undefined): string | undefined {
  if (!input || typeof input !== "object" || Array.isArray(input)) return undefined;
  const command = input.command ?? input.cmd;
  return typeof command === "string" && command.trim() ? command.trim() : undefined;
}

/**
 * The last activity in a thread, from the items the app already holds: no polling. The processes
 * its harness started count too: a build that prints nothing for ten minutes while a compiler uses
 * CPU is working, not stalled (`children.activeAt` is when they last did).
 */
export function activityOf(items: readonly ActivityItem[], children?: ChildActivity): LinkActivity {
  let lastAt: number | undefined;
  let lastTool: string | undefined;
  let lastCommand: string | undefined;
  let turns = 0;
  for (const item of items) {
    const at = item.endedAt ?? item.startedAt ?? item.at;
    if (at !== undefined && (lastAt === undefined || at > lastAt)) lastAt = at;
    if (item.kind === "tool") {
      lastTool = item.toolName ?? item.title ?? lastTool;
      lastCommand = commandOf(item.input) ?? lastCommand;
    }
    if (item.kind === "turnEnd") turns += 1;
  }
  if (children && children.processes > 0) {
    if (children.activeAt !== undefined && (lastAt === undefined || children.activeAt > lastAt))
      lastAt = children.activeAt;
  }
  return {
    lastAt,
    lastTool,
    ...(lastCommand ? { lastCommand } : {}),
    ...(children && children.processes > 0
      ? {
          children: {
            processes: children.processes,
            cpuMs: children.cpuMs,
            ...(children.busiest ? { busiest: children.busiest } : {}),
          },
        }
      : {}),
    turns,
  };
}

/** One child that has shown no progress while its turn still runs, and what it last ran. */
export interface StallNotice {
  link: Link;
  /** The activity timestamp this notice is about; the mark that stops a repeat. */
  at: number;
  idleMs: number;
  lastTool?: string;
  lastCommand?: string;
  /** The child's processes when it stalled: what runs and the CPU it has used in all. */
  children?: LinkActivity["children"];
}

/** A parent that should hear about its children, and what about them. */
export interface Wake {
  parent: string;
  parentTitle: string;
  /** Children that settled and were not reported yet. */
  settled: Link[];
  /** Children waiting on the user, whose request was not reported yet. */
  asking: Link[];
  /** Children still running that have shown no progress for {@link STALL_NOTICE_MS}. */
  stalled?: StallNotice[];
}

/** What the decision needs of the app: states, pending requests, waits, threads. */
export interface WakeContext {
  /** Each link's state as shown (`needs_user` while its child waits on the user). */
  states: Readonly<Record<string, LinkState>>;
  /** Each child's pending request id, when it waits on one. */
  pending: Readonly<Record<string, string>>;
  /** The links each parent is blocked on in `wait_session` right now. */
  waiting: Readonly<Record<string, readonly string[]>>;
  /**
   * The links each parent's queued wake rows already carry, by parent: they are reported when that
   * row's turn starts (`markDelivered`), so no second wake may report them first. A queue is the
   * app's memory only, so a row that is lost to a restart leaves its links unmarked and due again.
   */
  queued?: Readonly<Record<string, readonly string[]>>;
  /**
   * The threads these rules need, by id: every link's orchestrator and every
   * link's target, with the title a wake message names and whether a turn runs.
   * A thread that is missing is gone.
   */
  threads: Readonly<Record<string, { title: string; running: boolean }>>;
  /** Each link target's latest reply, for a finished link that holds no answer. */
  answers?: Readonly<Record<string, string>>;
  /** Each link target's last activity, for the stall notice and `wait_session`'s idle time. */
  activity?: Readonly<Record<string, LinkActivity>>;
  /**
   * The batch window in ms (Settings › Delegation, `wake-batch-seconds`). With one, a parent
   * hears about its delegates when the oldest unreported finish or stall is that old, all in one
   * wake; a question for the user goes at once and takes whatever else is waiting along. Without
   * one (0 or unset) a settled child waits {@link WAKE_COALESCE_MS} for its siblings only while
   * one of them still works.
   */
  batchMs?: number;
  now: number;
}

const stateOfLink = (link: Link, ctx: WakeContext) => ctx.states[link.id] ?? link.state;

/**
 * A child that still runs but has shown nothing for {@link STALL_NOTICE_MS}.
 * Only a working link whose target turn runs counts: a queued link has not
 * started, a settled one has answered, and one waiting on the user is not stuck.
 */
function stallNotice(link: Link, ctx: WakeContext): StallNotice | undefined {
  if (stateOfLink(link, ctx) !== "working" || !ctx.threads[link.target]?.running) return undefined;
  const activity = ctx.activity?.[link.target];
  const at = activity?.lastAt ?? link.startedAt;
  if (at === undefined) return undefined;
  const idleMs = Math.max(0, ctx.now - at);
  if (idleMs < STALL_NOTICE_MS) return undefined;
  return {
    link,
    at,
    idleMs,
    ...(activity?.lastTool ? { lastTool: activity.lastTool } : {}),
    ...(activity?.lastCommand ? { lastCommand: activity.lastCommand } : {}),
    ...(activity?.children ? { children: activity.children } : {}),
  };
}

/**
 * Whether `id` still owes its parent an answer: a child of its own is active,
 * its own child result was not reported to it yet, or one of its turns runs. A
 * middle thread that ended its turn after starting a leaf has *not* finished
 * for its parent while the leaf works, or while the wake from the leaf is still
 * to come — the answer it gave was "I started it", not the real one.
 */
export function quietTree(book: LinkBook, ctx: WakeContext, id: string): boolean {
  if (ctx.threads[id]?.running) return false;
  return quietBelow(book, ctx, id);
}

/**
 * Nothing `id` handed out still works or owes it an answer, whatever its own turn does: what a
 * link that already answered still waits on. A turn the target runs after it answered is not that
 * link's (the user writing to it, say), only the work the target handed out is (state/waiting.ts).
 */
export function quietBelow(book: LinkBook, ctx: WakeContext, id: string): boolean {
  if (book.links.some((l) => l.orchestrator === id && isActive(stateOfLink(l, ctx)))) return false;
  return !pendingWakeUnder(book, ctx, id);
}

/**
 * A child result `id` has not been told about yet: a wake for it is due or
 * about to be. Only `done` and `failed` wake a parent, so an interrupted or
 * stopped child is not a reason to wait: nothing will ever report it. A result
 * a newer link to the same thread replaced is not owed either — the newer answer
 * is — which is what lets the tree settle without marking the old one for good:
 * should that successor be stopped or interrupted, this one is owed again.
 */
export function pendingWakeUnder(book: LinkBook, ctx: WakeContext, id: string): boolean {
  return book.links.some((link) => {
    if (link.orchestrator !== id) return false;
    const state = stateOfLink(link, ctx);
    if (state === "done" || state === "failed") {
      if (link.notifiedAt !== undefined) return false;
      return !replaced(link, book, ctx);
    }
    if (state !== "needs_user") return false;
    const request = ctx.pending[link.id];
    return request !== undefined && link.askedRequest !== request;
  });
}

/** What one turn of a thread answered. */
export interface TurnAnswer {
  text: string;
  truncated: boolean;
  filesChanged: number;
}

/**
 * What the app says about a turn whose machine went away while it ran (`state/store.ts`
 * `watchMachineConnection` and the prompt that died with it). One sentence, so the thread's row and
 * the link's preview tell the same story.
 */
export const MACHINE_LOST_TURN = "The machine disconnected during this turn.";

/** The last reply in a transcript, whatever turn it belongs to. */
export function latestReply(items: readonly { kind: string; text?: string }[]): string {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]!;
    if (item.kind === "assistant" && item.text?.trim()) return item.text;
  }
  return "";
}

/**
 * A finished link's answer, read from the thread as it is now. A turn's rows
 * reach the app a moment after its prompt returns, and a link that settled on
 * that turn can hold nothing; the target's last reply is still the answer, and
 * the wake message and `wait_session` must carry it rather than an empty line.
 */
export function fillAnswers(book: LinkBook, answers: Readonly<Record<string, string>>): LinkBook {
  let changed = false;
  const links = book.links.map((link) => {
    if (link.state !== "done" && link.state !== "failed") return link;
    if (link.answer?.trim()) return link;
    const reply = answers[link.target];
    if (!reply) return link;
    changed = true;
    const { text, truncated } = capped(reply);
    return { ...link, answer: text, truncated };
  });
  return changed ? { ...book, links } : book;
}

/**
 * What a parent reads about a message it queued on `title` that never started: the turn ahead of
 * it failed. The message is not sent into a thread that just failed (it would meet the same full
 * context or harness error); the parent hears why, and `message_session` reopens the thread when it
 * sends again.
 */
export function undeliveredReason(title: string, failure: string | undefined): string {
  const why = failure?.trim() ? ` (${failure.trim()})` : "";
  return `Not delivered: “${title}” failed its turn before this message started${why}. It was taken out of the queue; message the thread again to retry.`;
}

/**
 * Two copies of one link's events as one list: the same moment written in both
 * copies is one event, and a copy that knows newer ones keeps them.
 */
export function mergeEvents(
  one: readonly LinkEvent[] | undefined,
  other: readonly LinkEvent[] | undefined,
): LinkEvent[] | undefined {
  const all = [...(one ?? []), ...(other ?? [])];
  if (!all.length) return undefined;
  // One row per moment across both copies, including a moment one copy repeats itself: the first
  // copy of it wins, and its duplicate does not survive the merge.
  const seen = new Set<string>();
  const merged: LinkEvent[] = [];
  for (const event of all) {
    const key = eventKey(event.kind, event.at);
    if (seen.has(key)) continue;
    seen.add(key);
    merged.push(event);
  }
  return merged.sort((a, b) => a.at - b.at).slice(-MAX_LINK_EVENTS);
}

/**
 * The book after a turn of `target` failed: every message still queued on it is failed with
 * {@link undeliveredReason}, so no row says Queued for a message that will never start on its own
 * (a failed turn holds the queue, `thread.ts` `holdsQueue`). Answers the links it failed, whose
 * messages the caller takes out of the target's queue.
 */
export function failQueuedLinks(
  book: LinkBook,
  target: string,
  failure: string | undefined,
  now: number,
): { book: LinkBook; failed: string[] } {
  const failed: string[] = [];
  const links = book.links.map((link) => {
    if (link.target !== target || link.state !== "queued") return link;
    failed.push(link.id);
    return advanceLink(
      link,
      {
        state: "failed" as const,
        answer: undeliveredReason(link.targetTitle || "the thread", failure),
        truncated: false,
        settledAt: now,
      },
      now,
    );
  });
  return failed.length ? { book: { ...book, links }, failed } : { book, failed };
}

/**
 * The book after a turn of `target` ended. A link from a parent that settled
 * but was not reported yet takes this turn's answer: the parent's wake waits for
 * the target's whole subtree to be quiet, so the answer it carries must be the
 * last one, not the "I started a leaf" of a turn that ended first. A turn that
 * answered nothing leaves the answer it had. A link already reported is re-armed
 * once — and only when this turn was started by the wake from the target's own
 * child — so the real answer reaches the parent too; `answerChangedAt` keeps
 * that from repeating, so no wake loops.
 */
export function restateLinks(
  book: LinkBook,
  target: string,
  says: TurnAnswer,
  now: number,
  woken: boolean,
): LinkBook {
  return {
    ...book,
    links: book.links.map((link) => {
      if (link.target !== target) return link;
      if (link.state !== "done" && link.state !== "failed") return link;
      // A turn that answered nothing (its rows were applied later, or the harness said nothing)
      // leaves the link as it is: it must not erase the answer the parent is about to read.
      if (!says.text.trim()) return link;
      const replaced = {
        ...link,
        answer: says.text,
        truncated: says.truncated,
        filesChanged: says.filesChanged,
      };
      if (link.notifiedAt === undefined) return { ...replaced, settledAt: now };
      if (!woken || link.answerChangedAt !== undefined) return link;
      // The answer is a later turn's: the time of the turn already reported is not its own.
      return {
        ...replaced,
        settledAt: now,
        startedAt: undefined,
        workedMs: undefined,
        notifiedAt: undefined,
        answerChangedAt: now,
      };
    }),
  };
}

/**
 * The book after a thread carried on from a stop the app waited out (a usage limit, an
 * overload): the links that stop settled as `failed` or `done` work again, with the clock
 * set to now, and settle with the answer the new turn gives. The parent that was told the
 * stop is told again only when the thread settles for good, not at each retry. A link the user
 * stopped, a newer link to the same thread, one that is not the thread's own, and one whose
 * parent is gone (`parentGone`) are left alone.
 */
export function reopenLinks(
  book: LinkBook,
  thread: string,
  ids: readonly string[],
  now: number,
  /** Whether the thread above a link is gone (archived, trashed): its link stays as it settled. */
  parentGone: (orchestrator: string) => boolean = () => false,
): LinkBook {
  if (!ids.length) return book;
  let changed = false;
  const links = book.links.map((link) => {
    if (!ids.includes(link.id) || link.target !== thread) return link;
    if (link.state !== "failed" && link.state !== "done") return link;
    if (parentGone(link.orchestrator)) return link;
    if (book.links.some((other) => other.target === thread && other.createdAt > link.createdAt)) return link;
    changed = true;
    // `notifiedAt` stays: the parent already heard the stop, and it is not told again if the thread
    // stops on the limit once more. The turn that settles for good clears it (`linksFinished`).
    return advanceLink(
      {
        ...link,
        answer: undefined,
        error: undefined,
        truncated: undefined,
        settledAt: undefined,
        answerChangedAt: undefined,
        workedMs: undefined,
        waitingSince: undefined,
        waitedMs: undefined,
      },
      { state: "working", startedAt: now },
      now,
    );
  });
  return changed ? { ...book, links } : book;
}

/**
 * A mark a wait set when it handed a link over: the field it wrote, so the one
 * mark can be taken back when the call that handed it over turns out to be gone.
 */
export interface HeardMark {
  id: string;
  notifiedAt?: number;
  askedRequest?: string;
}

/**
 * The book without the marks those waits set: the call that handed their links
 * over was dropped, so nobody read the answer it carried, and the ordinary wake
 * has to deliver it instead. Only the mark the wait itself wrote is cleared — a
 * newer one (another wait, or a wake) stands, so nothing is delivered twice.
 */
export function unheard(book: LinkBook, marks: readonly HeardMark[]): LinkBook {
  if (!marks.length) return book;
  const byId = new Map(marks.map((mark) => [mark.id, mark]));
  let changed = false;
  const links = book.links.map((link) => {
    const mark = byId.get(link.id);
    if (!mark) return link;
    if (mark.notifiedAt !== undefined) {
      if (link.notifiedAt !== mark.notifiedAt) return link;
      changed = true;
      const { notifiedAt: _cleared, ...rest } = link;
      return rest;
    }
    if (mark.askedRequest === undefined || link.askedRequest !== mark.askedRequest) return link;
    changed = true;
    const { askedRequest: _cleared, ...rest } = link;
    return rest;
  });
  return changed ? { ...book, links } : book;
}

/**
 * Settled links a newer link to the same target replaced: the parent messaged that thread again
 * since, so the older answer is out of date and its wake must not land after the newer one. Only
 * unreported links count: one the parent already heard about leaves nothing to fix.
 */
export function supersededLinks(book: LinkBook, ctx: WakeContext): Link[] {
  const replaced = replacedIds(book, ctx);
  return book.links.filter((link) => {
    const state = stateOfLink(link, ctx);
    if (state !== "done" && state !== "failed") return false;
    if (link.notifiedAt !== undefined) return false;
    return replaced.has(link.id);
  });
}

/** A link a restart or the user's Stop ended: it never answered anything. */
const stoppedLink = (link: Link, ctx: WakeContext) => {
  const state = stateOfLink(link, ctx);
  return state === "cancelled" || state === "interrupted";
};

/**
 * Whether a newer link to the same thread replaced `link`: the parent messaged that thread since, so
 * this answer is out of date and the newer turn's is the one it needs.
 *
 * A successor that was stopped never answered, so it replaces nothing: the older answer is still the
 * last real one. Nothing is written down for a replaced link either (`wakeTick` leaves it unmarked) —
 * the successor can itself be stopped or interrupted by a restart, and then the older answer is owed
 * again, which only a rule read afresh can tell.
 */
function replaced(link: Link, book: LinkBook, ctx: WakeContext): boolean {
  if (stoppedLink(link, ctx)) return false;
  return book.links.some(
    (other) =>
      other.orchestrator === link.orchestrator &&
      other.target === link.target &&
      other.createdAt > link.createdAt &&
      !stoppedLink(other, ctx),
  );
}

/** The ids of the links a newer one to the same thread replaced, heard about or not ({@link replaced}). */
function replacedIds(book: LinkBook, ctx: WakeContext): Set<string> {
  return new Set(book.links.filter((link) => replaced(link, book, ctx)).map((link) => link.id));
}

/**
 * What to tell each parent, one message per parent: children that settled and
 * children waiting on the user. A link is reported once — the mark lives in the
 * book (`notifiedAt`, `askedRequest`), so a restart does not tell it again — and
 * a parent blocked on it in `wait_session` already has the answer. A settled link
 * a newer one replaced is not reported either ({@link supersededLinks}): the
 * answer the parent needs is the newer turn's. A thread that is itself a child is
 * woken the same way its top-level parent is: its children's results are its own
 * to read, and its parent hears from it when its own turn ends. A settled child
 * is reported only once its own subtree is quiet ({@link quietTree}): a middle
 * thread is not finished for its parent while the thread it started still works.
 * Parallel children are coalesced: while any of a parent's links still runs, a
 * settled one waits up to `WAKE_COALESCE_MS` for its siblings; with none active
 * the message goes at once.
 *
 * A link a queued wake row already carries is left to that row (`ctx.queued`): it
 * is reported when the row's turn starts, so a second wake would be the same
 * result twice.
 */
export function wakesDue(book: LinkBook, ctx: WakeContext): Wake[] {
  const batch = ctx.batchMs ?? 0;
  const stale = new Set(supersededLinks(book, ctx).map((l) => l.id));
  const out: Wake[] = [];
  for (const [parent, links] of linksByParent(book)) {
    const here = ctx.threads[parent];
    if (!here) continue;
    const pending = pendingForParent(book, ctx, links, stale);
    const active = links.some((l) => isActive(stateOfLink(l, ctx)));
    let { settled } = pending;
    const { asking, stalled } = pending;
    if (batch > 0) {
      // One turn for everything that came in a window: nothing goes until the oldest finish or
      // stall is `batch` old, unless a delegate needs the user, which takes the rest with it.
      const oldest = oldestEvent(settled, stalled);
      if (!asking.length && (oldest === null || ctx.now - oldest < batch)) continue;
    } else {
      settled = settled.filter((link) => !active || ctx.now - (link.settledAt ?? 0) >= WAKE_COALESCE_MS);
    }
    if (settled.length || asking.length || stalled.length) {
      out.push({ parent, parentTitle: here.title, settled, asking, stalled });
    }
  }
  return out;
}

function linksByParent(book: LinkBook): Map<string, Link[]> {
  const byParent = new Map<string, Link[]>();
  for (const link of book.links) {
    const list = byParent.get(link.orchestrator);
    if (list) list.push(link);
    else byParent.set(link.orchestrator, [link]);
  }
  return byParent;
}

/** What one parent has not heard yet, before any timing rule: finished, asking and stalled. */
function pendingForParent(
  book: LinkBook,
  ctx: WakeContext,
  links: readonly Link[],
  stale: ReadonlySet<string>,
): { settled: Link[]; asking: Link[]; stalled: StallNotice[] } {
  const parent = links[0]?.orchestrator ?? "";
  const waiting = new Set(ctx.waiting[parent] ?? []);
  const carried = new Set(ctx.queued?.[parent] ?? []);
  const settled = links.filter((link) => {
    const state = stateOfLink(link, ctx);
    if (state !== "done" && state !== "failed") return false;
    if (link.notifiedAt !== undefined || waiting.has(link.id)) return false;
    if (carried.has(link.id)) return false;
    if (stale.has(link.id)) return false;
    return quietTree(book, ctx, link.target);
  });
  const asking = links.filter((link) => {
    if (stateOfLink(link, ctx) !== "needs_user" || waiting.has(link.id)) return false;
    if (carried.has(link.id)) return false;
    const request = ctx.pending[link.id];
    return request !== undefined && link.askedRequest !== request;
  });
  // A child that has gone quiet with its turn still running: told once per activity timestamp, so
  // a resumed child can go quiet again and be told again.
  const stalled = links.flatMap((link) => {
    const notice = stallNotice(link, ctx);
    if (!notice || link.stallNotifiedFor === notice.at) return [];
    return [notice];
  });
  return { settled, asking, stalled };
}

/** When the oldest of these finishes and stalls happened, or null when there are none. */
function oldestEvent(settled: readonly Link[], stalled: readonly StallNotice[]): number | null {
  let oldest: number | null = null;
  for (const at of [
    ...settled.map((link) => link.settledAt ?? 0),
    ...stalled.map((notice) => notice.at + STALL_NOTICE_MS),
  ]) {
    if (oldest === null || at < oldest) oldest = at;
  }
  return oldest;
}

/**
 * What a wake row promised, by the two things a wake can say: a child finished, or a child waits on
 * the user. The row's own words, so what it may mark is what it reported — not what its links happen
 * to be when it finally runs.
 */
export interface WakeReport {
  /** Links the row named as finished (or failed). */
  settled: Iterable<string>;
  /** Links the row named as waiting on the user. */
  asking: Iterable<string>;
}

/**
 * The book after a queued wake row's turn starts: the links it reported are heard now, and nothing
 * else is. This is where a queued wake marks, not where the row is written: the queue is the app's
 * memory only, so a mark written there would outlive the row and the notice with it (`host.ts`
 * `sendWake`, `store.ts` `runPrompt`).
 *
 * Each link is marked by what the row said about it. A child the row named as *asking* that finished
 * meanwhile is left unmarked: its finish is still owed, and the ordinary wake reports it once the row
 * has run (a question is its own notice, and a finish is another). A child the row named as finished
 * that is still running, or that now asks something, is left unmarked for the same reason the other
 * way round. The call is idempotent, so a row delivered twice cannot double-mark.
 */
export function markDelivered(book: LinkBook, reported: WakeReport, ctx: WakeContext, now: number): LinkBook {
  const settled = new Set(reported.settled);
  const asking = new Set(reported.asking);
  let changed = false;
  const links = book.links.map((link) => {
    const state = stateOfLink(link, ctx);
    if (settled.has(link.id)) {
      // What it was told: this child finished. Only a finish can be marked as one.
      if (state !== "done" && state !== "failed") return link;
      if (link.notifiedAt !== undefined) return link;
      changed = true;
      return { ...link, notifiedAt: now };
    }
    if (!asking.has(link.id)) return link;
    // What it was told: this child waits on the user. A finish it reached since is a separate notice.
    if (state !== "needs_user") return link;
    const request = ctx.pending[link.id];
    if (request === undefined || link.askedRequest === request) return link;
    changed = true;
    return { ...link, askedRequest: request };
  });
  return changed ? { ...book, links } : book;
}

/**
 * Whether a queued wake row still has something to tell its parent: its turn starting now would mark
 * at least one of the links it names. A row whose children the parent already heard about — the app
 * wrote the mark and then died before the queue was written back — is nothing to run, and it is
 * dropped when the queue is read back (`state/host.ts`), so a restart cannot report the same finish
 * twice. It judges a row exactly as its turn would ({@link markDelivered}), so the two cannot
 * disagree about what the row promised.
 */
export function wakeStillOwed(
  book: LinkBook,
  row: { links: readonly string[]; asking: readonly string[] },
  ctx: WakeContext,
): boolean {
  if (!row.links.length) return false;
  const asking = new Set(row.asking);
  const reported: WakeReport = { settled: row.links.filter((id) => !asking.has(id)), asking };
  return markDelivered(book, reported, ctx, ctx.now) !== book;
}

/** Settled links whose parent thread is gone: nothing will ever deliver them. */
export function orphanedLinks(book: LinkBook, ctx: WakeContext): Link[] {
  return book.links.filter(
    (link) =>
      !ctx.threads[link.orchestrator] &&
      link.notifiedAt === undefined &&
      (stateOfLink(link, ctx) === "done" || stateOfLink(link, ctx) === "failed"),
  );
}

/**
 * When a wake could next be due, for a timer to look again: the end of the oldest
 * batch window (or, with none, the oldest settled child's coalescing window), and a
 * stall's silence reaching its notice. Null when nothing waits.
 */
export function nextWakeAt(book: LinkBook, ctx: WakeContext): number | null {
  let next: number | null = null;
  const consider = (at: number) => {
    if (at > ctx.now && (next === null || at < next)) next = at;
  };
  const stale = new Set(supersededLinks(book, ctx).map((l) => l.id));
  // A stall not yet due: look again when the silence reaches the notice.
  for (const link of book.links) {
    if (stateOfLink(link, ctx) !== "working" || !ctx.threads[link.target]?.running) continue;
    const at = ctx.activity?.[link.target]?.lastAt ?? link.startedAt;
    if (at === undefined || link.stallNotifiedFor === at) continue;
    consider(at + STALL_NOTICE_MS);
  }
  const batch = ctx.batchMs ?? 0;
  if (batch > 0) {
    // Each parent's window ends `batch` after its oldest unreported finish or stall.
    for (const [parent, links] of linksByParent(book)) {
      if (!ctx.threads[parent]) continue;
      const { settled, stalled } = pendingForParent(book, ctx, links, stale);
      const oldest = oldestEvent(settled, stalled);
      if (oldest !== null) consider(oldest + batch);
    }
    return next;
  }
  const carried = new Set(Object.values(ctx.queued ?? {}).flat());
  for (const link of book.links) {
    const state = stateOfLink(link, ctx);
    if (state !== "done" && state !== "failed") continue;
    if (link.notifiedAt !== undefined || stale.has(link.id) || carried.has(link.id)) continue;
    if (!quietTree(book, ctx, link.target)) continue;
    consider((link.settledAt ?? 0) + WAKE_COALESCE_MS);
  }
  return next;
}

/** The link ids a wake reports: what a queued wake keeps, so a newer one can be folded into it. */
export function wakeIds(wake: Wake): string[] {
  return [...wake.settled, ...wake.asking, ...(wake.stalled ?? []).map((notice) => notice.link)].map(
    (link) => link.id,
  );
}

/** The ids a wake names as waiting on the user, of {@link wakeIds}: what the row said it was. */
export function askingIds(wake: Wake): string[] {
  return wake.asking.map((link) => link.id);
}

/**
 * The wake those links make now, with each link's state as shown. It is how a wake waiting for a busy
 * parent is rebuilt around a newer one: one turn reports everything, not a queue of turns each saying
 * "Delegate finished".
 *
 * It follows `wakesDue`'s rules, so a rebuild never restates what that rule withdrew: a settled link a
 * newer one to the same thread replaced is out of date, and one the parent is blocked on in
 * `wait_session` is already in that call's answer. The two rules it leaves out are the ones about
 * timing and the mark: the coalescing window (this row already waited for it), and `notifiedAt` — a
 * queued wake's links are marked when its turn starts, not when the row is written. A link still
 * marked new is judged like `wakesDue` does, though: a settled child whose own subtree still works is
 * not finished for its parent yet. A link the row being rebuilt already carries (`promised`), or one
 * delivered before (`notifiedAt`), keeps its line whatever its thread does meanwhile, or the notice
 * the parent is waiting for would vanish with nothing else to deliver it.
 *
 * Null when nothing is left to report.
 */
export function wakeFor(
  parent: string,
  parentTitle: string,
  ids: Iterable<string>,
  book: LinkBook,
  ctx: WakeContext,
  promised?: ReadonlySet<string>,
): Wake | null {
  const want = new Set(ids);
  const replaced = replacedIds(book, ctx);
  const waiting = new Set(ctx.waiting[parent] ?? []);
  const settled: Link[] = [];
  const asking: Link[] = [];
  const stalled: StallNotice[] = [];
  for (const link of book.links) {
    if (link.orchestrator !== parent || !want.has(link.id)) continue;
    if (waiting.has(link.id)) continue;
    const state = stateOfLink(link, ctx);
    if (state === "done" || state === "failed") {
      if (replaced.has(link.id)) continue;
      // Reported already, or promised by the row being rebuilt: its line stands whatever its thread
      // does meanwhile, because nothing else would deliver that answer.
      const told = link.notifiedAt !== undefined || !!promised?.has(link.id);
      if (!told && !quietTree(book, ctx, link.target)) continue;
      settled.push(link);
    } else if (state === "needs_user") {
      asking.push(link);
    } else {
      const notice = stallNotice(link, ctx);
      if (notice) stalled.push(notice);
    }
  }
  return settled.length || asking.length || stalled.length
    ? { parent, parentTitle, settled, asking, stalled }
    : null;
}

/**
 * A queued wake with a newer one folded into it: one wake turn for everything that finished, instead
 * of a queue of turns each saying "Delegate finished". Its text is rebuilt from the links the waiting
 * row already carried plus the new wake's ({@link wakeFor}), so a child that asked and then finished
 * reads once, as finished. The row keeps its place among the user's own messages; when nothing is
 * left to report it is returned as it is.
 *
 * Null when the row cannot be rebuilt faithfully: one of the links it already reports is gone from
 * the book (pruned after many finished children), so rewriting its text would drop a report the parent
 * is still waiting for, or the row names no links at all (written by a build before the row kept them,
 * or by a preview). `queueWake` then queues the new wake beside it instead, and the old row is left as
 * it was.
 */
export function foldWake(
  waiting: QueuedPrompt,
  wake: Wake,
  book: LinkBook,
  ctx: WakeContext,
  now: number,
): QueuedPrompt | null {
  // No `wakeLinks`: what it reports cannot be read back, so its text must not be rewritten.
  if (!waiting.wakeLinks?.length) return null;
  const carried = waiting.wakeLinks;
  const known = new Set(
    book.links.filter((link) => link.orchestrator === wake.parent).map((link) => link.id),
  );
  if (carried.some((id) => !known.has(id))) return null;
  const merged = wakeFor(
    wake.parent,
    wake.parentTitle,
    [...carried, ...wakeIds(wake)],
    book,
    ctx,
    new Set(carried),
  );
  if (!merged) return waiting;
  return {
    ...waiting,
    text: wakeMessage(merged, now),
    wakeLinks: wakeIds(merged),
    wakeAsking: askingIds(merged),
  };
}

/**
 * A busy parent's queue with this wake in it: folded into the newest wake row that can take it, so one
 * turn reports everything that finished, else a row of its own. The user's own messages keep their
 * places, and the row keeps the id it was queued with, so nothing a reader is looking at moves.
 *
 * Nothing is marked here: the row carries its links and the marks are written when its turn starts
 * (`markDelivered`). The queue lives in memory, so a mark written now would survive a restart that the
 * row does not, and the wake would be lost.
 */
export function queueWake(
  queue: readonly QueuedPrompt[],
  wake: Wake,
  book: LinkBook,
  ctx: WakeContext,
  now: number,
  id: string,
): QueuedPrompt[] {
  const rows = [...queue];
  for (let at = rows.length - 1; at >= 0; at--) {
    const row = rows[at]!;
    if (!row.wake) continue;
    const folded = foldWake(row, wake, book, ctx, now);
    // A row whose own reports the book no longer holds: leave it whole and try the one before it.
    if (!folded) continue;
    rows[at] = folded;
    return rows;
  }
  return [
    ...rows,
    {
      id,
      text: wakeMessage(wake, now),
      wake: true,
      wakeLinks: wakeIds(wake),
      wakeAsking: askingIds(wake),
    },
  ];
}

/** A wake's compact first line, and the transcript's: "2 delegates finished · 1 delegate asks". */
export function wakeSummary(wake: Wake): string {
  const parts: string[] = [];
  if (wake.settled.length) {
    const failed = wake.settled.filter((link) => link.state === "failed").length;
    if (wake.settled.length === 1) {
      const link = wake.settled[0]!;
      const how = link.state === "failed" ? "failed" : "finished";
      parts.push(
        `Delegate ${how}${link.resumedAt !== undefined ? " after a restart" : ""}: ${link.targetTitle}`,
      );
    } else if (failed === wake.settled.length) {
      parts.push(`${wake.settled.length} delegates failed`);
    } else {
      parts.push(`${wake.settled.length} delegates finished`);
      if (failed) parts.push(`${failed} failed`);
    }
  }
  if (wake.asking.length === 1) {
    parts.push(`Delegate asks: ${wake.asking[0]!.targetTitle}`);
  } else if (wake.asking.length > 1) {
    parts.push(`${wake.asking.length} delegates need you`);
  }
  if (wake.stalled?.length === 1) {
    parts.push(`Delegate stalled: ${wake.stalled[0]!.link.targetTitle}`);
  } else if (wake.stalled && wake.stalled.length > 1) {
    parts.push(`${wake.stalled.length} delegates stalled`);
  }
  return parts.join(" · ");
}

/** One line of text cut at `max` characters, with an ellipsis when it was longer. */
const preview = (text: string, max: number): string =>
  text.length > max ? `${text.slice(0, max).trimEnd()}…` : text;

/** One delegate, as the wake message names it: link id, status, title, harness, worked time, thread, changes. */
const linkLine = (link: Link, state: LinkState, now: number) => {
  const bits = [`link_id ${link.id}`, state, `“${link.targetTitle}”`];
  // The turn this settled on began with the restart resume, not with the work before the quit.
  if (link.resumedAt !== undefined) bits.push("resumed after a restart");
  const harness = link.harnessName ?? link.harness;
  if (harness) bits.push(harness);
  if (link.workedMs) bits.push(`worked ${formatDuration(link.workedMs)}`);
  bits.push(`session ${link.target}`);
  if (link.filesChanged) bits.push(`${link.filesChanged} file${link.filesChanged === 1 ? "" : "s"} changed`);
  if (link.settledAt) bits.push(`${Math.max(1, Math.round((now - link.settledAt) / 1000))}s ago`);
  return `- ${bits.join(" · ")}`;
};

/**
 * The message a woken parent gets: what finished, with each child's status,
 * title, link id, files and a bounded answer, and which children wait on the
 * user. The first line is the compact summary the transcript shows. It ends by
 * naming `read_session` / `wait_session` for the rest and says not to poll.
 */
export function wakeMessage(wake: Wake, now: number): string {
  const lines: string[] = [wakeSummary(wake), ""];
  if (wake.settled.length) {
    lines.push("Finished:", "");
    for (const link of wake.settled) {
      lines.push(linkLine(link, link.state, now));
      const { text, truncated } = capped((link.answer ?? "").trim(), WAKE_ANSWER_CHARS);
      if (!text) lines.push("  answer: (nothing it said came back)");
      else lines.push(`  answer: ${text.split("\n").join("\n  ")}${truncated ? " …" : ""}`);
    }
    lines.push("");
  }
  if (wake.asking.length) {
    lines.push("Waiting on you (the user answers these; you cannot):", "");
    for (const link of wake.asking) lines.push(linkLine(link, "needs_user", now));
    lines.push("", "list_pending_requests shows what it asks; ask the user if it is unclear.", "");
  }
  if (wake.stalled?.length) {
    lines.push("No progress while the turn still runs:", "");
    for (const { link, idleMs, lastTool, lastCommand, children } of wake.stalled) {
      const tool = lastTool
        ? `${lastTool}${lastCommand ? `: ${preview(lastCommand, 120)}` : ""}`
        : "none reported";
      // What still runs below it and how much CPU it has used: a process that used none for the
      // whole window is stuck (a lock, a prompt), not building.
      const below = children
        ? ` ${children.processes === 1 ? "1 process runs" : `${children.processes} processes run`} under it${
            children.busiest ? ` (busiest: ${children.busiest})` : ""
          }, ${formatDuration(children.cpuMs)} of CPU used in all and none for ${formatDuration(idleMs)}.`
        : "";
      lines.push(
        `- Delegate “${link.targetTitle}” has made no progress for ${formatDuration(idleMs)}; last tool: ${tool}.${below} It may be stuck on a command that does not exit; interrupt_session it or message it.`,
      );
    }
    lines.push("");
  }
  lines.push(
    "The rest is in read_session on a child's session id, or wait_session on a link_id (a settled link answers at once). This message is the notification: do not poll with sleeps.",
  );
  // The engine keeps the text, not the app's flag: the mark makes a wake turn a wake turn when its
  // history is replayed (state/thread.ts `isWakeMessage`), so it never reads as the user's message.
  return withWakeMark(lines.join("\n"));
}

/** What the user is asked, in words. */
export interface GrantAsk {
  kind:
    | "message"
    | "read"
    | "start"
    | "fan_out"
    | "organize"
    | "answer"
    | "interrupt"
    | "transfer"
    | "configure"
    | "merge"
    | "cleanup"
    | "handoff"
    | "project"
    | "schedule"
    | "exec"
    | "desktop";
  caller: string;
  callerTitle: string;
  /** A request that needs individual approval, even in a Full access thread. */
  onceOnly?: boolean;
  /** The thread messaged or read, or the new thread's title. */
  target: string;
  detail: string;
  project?: string;
  harnesses?: string[];
  /** The access a new thread starts at (`start`, `fan_out`): never more than the caller's own. */
  access?: PermissionMode;
  worktree?: boolean;
  /**
   * A thread of the user's own at the top level (`standalone`), not a delegate:
   * the card says so and nothing comes back to the caller.
   */
  standalone?: boolean;
  /** The paired machine a start or a message acts on, named on the card; absent for this computer. */
  machine?: string;
  /** The remote machine asking to control this computer's desktop. */
  fromMachine?: string;
  /** Whether the target desktop app always asks on every action (password managers, System Settings). */
  sensitive?: boolean;
}

export type GrantChoice = "always" | "once" | "deny";

/** Capture the last branch and PR before deleting a delegate's checkout. */
export function retainDelegateCleanup(
  book: LinkBook,
  target: string,
  cleanup: NonNullable<Link["cleanup"]>,
): LinkBook {
  return { ...book, links: book.links.map((link) => (link.target === target ? { ...link, cleanup } : link)) };
}
