import { sessionsListed } from "./engine";
import { book, setBook, wakeTick } from "./host";
import {
  advanceLink,
  capped,
  failQueuedLinks,
  isActive,
  type LinkBook,
  latestReply,
  MAX_DELEGATES_PER_THREAD,
  reopenLinks,
  restateLinks,
  type TurnAnswer,
  updateLink,
  waitEnded,
  waitStarted,
} from "./orchestration";
import {
  cancel,
  dropInternetWait,
  dropLimitWait,
  releaseLimitWaitLink,
  sessionById,
  setThreadStatus,
  statusOf,
  updateThread,
  useApp,
} from "./store";
import { isLoading, isRunning, type TranscriptItem, turnCompletion } from "./thread";
import { isRemoteThread } from "./threadKeys";

// What a link's turns do to it: waits, starts, steers, answers, finishing and stopping. Split out
// of host.ts, which re-exports it.

// ---------------------------------------------------------------- link turns

/**
 * The user must answer something in `thread`: every live link to it stops its
 * work clock, so the delegate tree never counts a wait on the user (`state/delegates.ts`).
 */
export function linkWaitStarted(thread: string, at = Date.now()) {
  const links = waitStarted(book().links, thread, at);
  // No link to that thread waits on anything: leave the book, and its save, alone.
  if (links === book().links) return;
  setBook((b) => ({ ...b, links }));
}

/** It was answered (or stopped): the wait it took is remembered and the clock resumes. */
export function linkWaitEnded(thread: string, at = Date.now()) {
  const links = waitEnded(book().links, thread, at);
  if (links === book().links) return;
  setBook((b) => ({ ...b, links }));
}

export function linkStarted(id: string) {
  // The turn starts as this link's turn: its notice reports the time from here.
  setBook((b) => updateLink(b, id, { state: "working", startedAt: Date.now() }));
}

/**
 * The links whose message went into a running turn of a thread (`steer`), by thread, with the text
 * the engine was given: that turn settles them, and the text finds the ones it never read.
 */
export const steeredIntoTurn = new Map<string, Map<string, string>>();

/** What a steer says as the thread recorded it (`appendSteer`), else the text it was asked with. */
const steerText = (thread: string, fallback: string) =>
  useApp.getState().threads[thread]?.steers.at(-1)?.text ?? fallback;

/**
 * A link's message `text` was steered into the running turn of `thread`. No turn of its own carries
 * the link (`runPrompt` `options.link`), so the running turn is the one that answers it: the link
 * works from here and is settled when that turn ends (`threadTurnEnded`), however many steers went
 * in and whether or not the thread is open.
 *
 * A turn that ended while the steer was in flight either handed the steer back (a copy waits in the
 * queue: it carries the link, and the link is `queued` behind it so a failed turn fails it) or read
 * it, in which case that turn's own reply is the answer and the link settles on it. The message is
 * never sent a second time.
 */
export function linkSteered(thread: string, link: string | undefined, text: string): void {
  if (!link) return;
  // The clock starts at the steer: a steered link has no turn of its own to start it, and without a
  // start time `reconcileLinks` would count it from when it was made.
  linkStarted(link);
  const said = steerText(thread, text);
  const current = useApp.getState().threads[thread];
  if (!isRunning(current)) {
    const queued = current?.queue.find((q) => !q.link && q.text.trim() === said.trim());
    if (queued) {
      updateThread(thread, (t) => ({
        ...t,
        queue: t.queue.map((q) => (q.id === queued.id ? { ...q, link } : q)),
      }));
      setBook((b) => updateLink(b, link, { state: "queued" }));
    } else {
      linksFinished([link], thread, "completed", undefined, []);
    }
    return;
  }
  const links = steeredIntoTurn.get(thread) ?? new Map<string, string>();
  links.set(link, said);
  steeredIntoTurn.set(thread, links);
}

/**
 * The turn of `thread` ended and the engine handed back steers it never read (`unusedSteers`); they
 * wait in the queue as turns of their own. The links they came from are still owed an answer, so
 * those turns carry them, and the ending turn does not settle them (`threadTurnEnded`).
 */
export function requeueUnreadSteers(thread: string, unused: readonly string[]): void {
  const steered = steeredIntoTurn.get(thread);
  if (!steered?.size || !unused.length) return;
  const carried = new Map<string, string>();
  for (const said of unused) {
    const found = [...steered].find(([link, text]) => !carried.has(link) && text.trim() === said.trim());
    if (!found) continue;
    carried.set(found[0], found[1]);
    steered.delete(found[0]);
  }
  if (!steered.size) steeredIntoTurn.delete(thread);
  if (!carried.size) return;
  // Queued behind the turn that will read it: a failed turn that holds the queue fails the link
  // (`failQueuedOn`) instead of leaving it Working beside a thread that runs nothing.
  setBook((b) => [...carried.keys()].reduce((acc, link) => updateLink(acc, link, { state: "queued" }), b));
  updateThread(thread, (t) => {
    const waiting = new Map(carried);
    return {
      ...t,
      queue: t.queue.map((q) => {
        if (q.link) return q;
        const found = [...waiting].find(([, text]) => text.trim() === q.text.trim());
        if (!found) return q;
        waiting.delete(found[0]);
        return { ...q, link: found[0] };
      }),
    };
  });
}

/** The links steered into `thread`'s turn, taken from it: a turn that is lost carries them on (`lostTurn`). */
export function takeSteeredLinks(thread: string): string[] {
  const links = [...(steeredIntoTurn.get(thread)?.keys() ?? [])];
  steeredIntoTurn.delete(thread);
  return links;
}

/**
 * The links a resumed turn of `thread` answers: the ones a restart severed while their turn ran
 * (`orchestration.ts` `interruptActive`). A link the restart caught still queued is not one of
 * them: its message never reached the thread, and the queue it waited in is read back and run on
 * its own (`state/queues.ts`), which starts and settles that link as a link turn does.
 */
export function interruptedLinks(thread: string): string[] {
  return book()
    .links.filter((l) => l.target === thread && l.state === "interrupted" && l.startedAt !== undefined)
    .map((l) => l.id);
}

/**
 * The links a resumed thread's turn answers (`store.ts` `resumeInterrupted`): a
 * restart severed them (`orchestration.ts` `interruptActive`), and the turn the
 * app resumes that thread with is the turn they were waiting on. They are left
 * as {@link linkStarted} leaves a link — working, with the clock set by this
 * turn — and marked `resumedAt`, so the parent that reads the notice knows the
 * answer came after a restart, and its work time is not the time it spent
 * waiting out the quit. A link that is no longer interrupted (a newer one to
 * the same thread replaced it, or the user stopped it) is left alone.
 */
export function linksResumed(ids: readonly string[]): void {
  if (!ids.length) return;
  const now = Date.now();
  setBook((b) => ({
    ...b,
    links: b.links.map((link) =>
      ids.includes(link.id) && link.state === "interrupted"
        ? advanceLink(
            link,
            {
              state: "working" as const,
              startedAt: now,
              resumedAt: now,
              waitingSince: undefined,
              waitedMs: undefined,
            },
            now,
          )
        : link,
    ),
  }));
}

/**
 * A thread carries on after the app waited out the stop that ended its turn (`store.ts`
 * `continueDueThreads`): the links that stop settled work again and settle with the new turn.
 */
export function linksReopened(thread: string, ids: readonly string[]): void {
  if (!ids.length) return;
  // A parent the user archived is not woken by a delegate that carries on without it.
  const archived = useApp.getState().archived;
  const next = reopenLinks(book(), thread, ids, Date.now(), (orchestrator) => !!archived[orchestrator]);
  if (next === book()) return;
  setBook(() => next);
}

/** The harness a settled link ran on, as the notice names it: its id and display name. */
function linkHarness(thread: string): { harness?: string; harnessName?: string } {
  const s = useApp.getState();
  const id = s.threads[thread]?.harness?.id ?? s.sessions.find((x) => x.id === thread)?.harness;
  if (!id) return {};
  return { harness: id, harnessName: s.harnesses?.harnesses.find((h) => h.id === id)?.name };
}

/**
 * What a turn answered: its last reply with something in it, bounded, and how
 * many files it changed. An assistant row an empty chunk left behind (`"\n"`
 * between two calls) is skipped, so it cannot win over the reply before it.
 */
function answerOf(items: TranscriptItem[], failed?: string): TurnAnswer {
  const { text, truncated } = capped(latestReply(items));
  return {
    text: failed ? (text ? `${text}\n\n${failed}` : failed) : text,
    truncated,
    filesChanged: turnCompletion(items, items.length)?.files.length ?? 0,
  };
}

/**
 * A turn's answer, and the thread's last reply when the turn's own rows hold
 * none. The rows of a reply reach the app a moment after the prompt returns (and
 * a thread whose rows were replayed can leave the turn's window behind the
 * reply), so a link must never settle on an empty answer while the thread knows
 * one: the parent reads that answer, and nothing would ever fill it in.
 */
function answerFor(thread: string, items: TranscriptItem[], failed?: string, fallback = true): TurnAnswer {
  const says = answerOf(items, failed);
  if (!fallback || says.text.trim()) return says;
  const { text, truncated } = capped(latestReply(useApp.getState().threads[thread]?.items ?? []));
  return text ? { ...says, text, truncated } : says;
}

/**
 * The links' turn ended: their answer is that turn's last reply, never an
 * earlier one. A turn that ran for several links at once (a thread a restart
 * interrupted on more than one link, which one resume turn answers) settles
 * them in one write to the book and one wake tick, so the parents read one
 * notice and nothing is woken twice.
 */
export function linksFinished(
  ids: readonly string[],
  thread: string,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
  /**
   * The thread waits to carry on again (a usage limit, an overload): a link the parent already
   * heard the stop of stays heard, so it is woken once when the thread settles for good.
   */
  holding = false,
) {
  if (!ids.length) return;
  const now = Date.now();
  // A stopped link is not an answer: it takes only what its own turn said.
  const says = answerFor(thread, items, failed, outcome !== "cancelled");
  const state = outcome === "cancelled" ? "cancelled" : outcome === "failed" ? "failed" : "done";
  const harness = linkHarness(thread);
  let settled = false;
  setBook((b) => ({
    ...b,
    links: b.links.map((link) => {
      if (!ids.includes(link.id)) return link;
      // A link already settled (stopped by the user, or reported by a newer turn of the thread)
      // is not settled again: this turn is not the one it was waiting on.
      if (link.state === "done" || link.state === "failed" || link.state === "cancelled") return link;
      settled = true;
      const workedMs = link.startedAt === undefined ? undefined : Math.max(0, now - link.startedAt);
      return advanceLink(
        link,
        {
          state,
          answer: says.text,
          ...(state === "failed" && failed?.trim() ? { error: failed.trim() } : {}),
          truncated: says.truncated,
          filesChanged: says.filesChanged,
          settledAt: now,
          ...(holding || state === "cancelled" ? {} : { notifiedAt: undefined }),
          ...harness,
          ...(workedMs ? { workedMs } : {}),
        },
        now,
      );
    }),
  }));
  if (settled) wakeTick();
}

/** The link's turn ended: its answer is that turn's last reply, never an earlier one. */
export function linkFinished(
  id: string,
  thread: string,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
) {
  linksFinished([id], thread, outcome, failed, items);
}

/**
 * A turn of `thread` ended, whichever way it was started. A parent's wake waits
 * for the thread's own subtree to be quiet, so a link that settled but was not
 * reported yet takes this turn's answer instead of the one that ended first
 * (host.ts `wait` reads the same link). A link already reported is re-armed once
 * when this turn was started by the wake from the thread's own child, so the
 * real answer reaches the parent too.
 *
 * A link a restart severed (`orchestration.ts` `interruptActive`) is not
 * settled here: only the turn the app resumes that thread with answers it
 * (`store.ts` `resumeInterrupted` hands the link to that turn, `host.ts`
 * {@link linksResumed} and {@link linksFinished}). A turn the user starts in
 * the thread afterwards is their own work, not the branch's result.
 */
export function threadTurnEnded(
  thread: string,
  woken: boolean,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
) {
  // The links whose messages were steered into this turn end with it: no prompt carries them.
  const steered = takeSteeredLinks(thread);
  if (steered.length) linksFinished(steered, thread, outcome, failed, items);
  // A stopped turn is not an answer: the link turns record their own, and nothing else does. Its
  // end still matters to a parent: this thread no longer runs, so what it owes may be due now.
  if (outcome !== "cancelled") {
    const now = Date.now();
    const says = answerFor(thread, items, outcome === "completed" ? undefined : failed);
    setBook((b) => restateLinks(b, thread, says, now, woken));
  }
  if (outcome === "failed") failQueuedOn(thread, failed);
  wakeTick();
}

/**
 * A failed turn holds the thread's queue (`thread.ts` `holdsQueue`), so a message another thread
 * queued there would say Queued for good. It fails instead, with the reason, and leaves the queue:
 * its parent reads why and may message the thread again, which reopens it (`messageNow`). The
 * user's own queued messages stay held for Resume.
 */
function failQueuedOn(thread: string, failure: string | undefined) {
  const { book: next, failed } = failQueuedLinks(book(), thread, failure, Date.now());
  if (!failed.length) return;
  const gone = new Set(failed);
  updateThread(thread, (t) => {
    const queue = t.queue.filter((item) => !item.link || !gone.has(item.link));
    return { ...t, queue, queueHeld: queue.length ? t.queueHeld : undefined };
  });
  setBook(() => next);
}

/** A queued link message was removed from the target's queue. */
export function queuedLinkDropped(id: string) {
  const link = book().links.find((l) => l.id === id);
  if (link?.state !== "queued") return;
  setBook((b) => updateLink(b, id, { state: "cancelled", settledAt: Date.now() }));
  // Its siblings can now be the last ones working: look again at once.
  wakeTick();
}

/** Whether `id` runs nothing and waits on nobody, as far as this window can tell. */
function threadIdle(id: string): boolean {
  const state = useApp.getState();
  const status = statusOf(state, id);
  if (status === "running" || status === "needs_input") return false;
  // A thread this window has not opened may be running in the engine: its list row says so.
  const listed = sessionById(state, id)?.status;
  return listed !== "running" && listed !== "needs_input";
}

/**
 * Stop a link: its queued message goes, or its running turn is cancelled. A link whose thread runs
 * nothing has no turn to cancel and would stay Working for good, so stopping it settles it as
 * cancelled and wakes the parent. The user's own turns there go on.
 */
export async function stopLink(id: string): Promise<boolean> {
  const link = book().links.find((l) => l.id === id);
  if (!link || !isActive(link.state)) return false;
  if (link.state === "queued") {
    updateThread(link.target, (t) => ({ ...t, queue: t.queue.filter((q) => q.link !== id) }));
    setBook((b) => updateLink(b, id, { state: "cancelled" }));
    return true;
  }
  // Another computer's thread is cancelled on that computer, whatever this window last heard of it.
  if (!isRemoteThread(link.target) && threadIdle(link.target)) {
    // A wait that would carry the thread on (a limit, the internet) is the work being stopped.
    dropLimitWait(link.target);
    dropInternetWait(link.target);
    linksFinished([id], link.target, "cancelled", undefined, []);
    return true;
  }
  await cancel(link.target);
  return true;
}

/** How long a working link may sit beside an idle thread before it is read as lost, not just starting. */
export const STALE_LINK_MS = 20_000;

/** How long a working link with no start time may sit beside an idle thread: a worktree or a cold harness can take minutes. */
export const UNSTARTED_LINK_MS = 10 * 60_000;

/**
 * Settles the working links whose thread, by its real status, runs nothing: a turn that ended
 * without settling its link (a steer, a window that missed the end) must not read Working for
 * hours. The answer is the thread's last reply, or its failure. Run when the thread list is read
 * (load and refocus) and when a thread reports a status. A link that is only starting (its turn
 * has not begun yet), one whose message waits in the thread's queue, and another computer's
 * threads (their lost turns are followed by `lostTurn`) are left alone.
 */
export function reconcileLinks(now = Date.now()): void {
  const state = useApp.getState();
  if (!sessionsListed(state.engine)) return;
  const stale = book().links.filter((link) => {
    if (link.state !== "working" && link.state !== "needs_user") return false;
    // A link that is only starting (a worktree being made, a cold harness) has an idle thread beside
    // it for a moment. One with a start time gets `STALE_LINK_MS` from it; one that never recorded a
    // start (a steered link saved before `linkSteered` set it, a turn that never began) counts from
    // when it was made and gets the longer `UNSTARTED_LINK_MS`, so a slow start is not read as lost
    // while a forgotten link still does not read Working for hours.
    if (isRemoteThread(link.target)) return false;
    const idleFor = link.startedAt === undefined ? now - link.createdAt : now - link.startedAt;
    if (idleFor < (link.startedAt === undefined ? UNSTARTED_LINK_MS : STALE_LINK_MS)) return false;
    const thread = state.threads[link.target];
    if (isLoading(thread) || thread?.queue.some((q) => q.link === link.id)) return false;
    if (!thread && !sessionById(state, link.target)) return false;
    // A thread held for a usage limit, the internet or background commands carries on by itself.
    if (state.limitWaits[link.target] || state.internetWaits[link.target] || thread?.waiting) return false;
    return threadIdle(link.target);
  });
  for (const link of stale) {
    const error = state.threads[link.target]?.error;
    linksFinished([link.id], link.target, error ? "failed" : "completed", error, []);
  }
}

/** Settling a sender abandons its outstanding links without stopping their targets. */
export function clearWaitingLinks(ids: readonly string[]) {
  const senders = new Set(ids);
  const at = Date.now();
  setBook((book) => ({
    ...book,
    links: book.links.map((link) =>
      senders.has(link.orchestrator) && isActive(link.state)
        ? advanceLink(link, { state: "cancelled", settledAt: at, notifiedAt: at }, at)
        : link,
    ),
  }));
}

/** Stops the work `orchestrator` gave one delegate (its status row's Stop); the thread's own messages stay. */
export async function stopDelegate(orchestrator: string, target: string): Promise<number> {
  let stopped = 0;
  for (const link of book().links) {
    if (link.orchestrator !== orchestrator || link.target !== target || !isActive(link.state)) continue;
    if (await stopLink(link.id)) stopped += 1;
  }
  return stopped;
}

/** The threads whose work is being stopped right now: a cascade stops each one once. */
const stopping = new Set<string>();

/**
 * Stops the work `orchestrator` handed to other threads, and what those
 * threads handed on: stopping a thread takes its whole delegation tree with
 * it, because the thread above is no longer there to read the answers. A
 * normal finish does not cascade: a child that settles leaves its own children
 * running, they are woken and answer into the thread that started them.
 *
 * Each thread is stopped once ({@link stopping}), so a stop that also arrives
 * from a child's own cancel cannot loop or count twice. Returns how many links
 * it stopped, for a tool's answer to say.
 */
export async function stopLinksFrom(orchestrator: string): Promise<number> {
  if (stopping.has(orchestrator)) return 0;
  stopping.add(orchestrator);
  try {
    let stopped = 0;
    for (const link of book().links) {
      if (link.orchestrator !== orchestrator || !isActive(link.state)) continue;
      if (await stopLink(link.id)) stopped += 1;
    }
    stopped += releaseLimitWaitsFrom(orchestrator);
    return stopped;
  } finally {
    stopping.delete(orchestrator);
  }
}

/**
 * The delegates of `orchestrator` that are waiting out a usage limit stop with it: their failed
 * links are cancelled and their wait is let go (or only the link, when another parent's link
 * shares it), so the reset never wakes a parent the user stopped or archived. Returns how many
 * links it cancelled.
 */
export function releaseLimitWaitsFrom(orchestrator: string): number {
  const waits = useApp.getState().limitWaits;
  const held = book().links.filter(
    (l) =>
      l.orchestrator === orchestrator &&
      (l.state === "failed" || l.state === "done") &&
      waits[l.target]?.links?.includes(l.id),
  );
  if (!held.length) return 0;
  const now = Date.now();
  const ids = new Set(held.map((l) => l.id));
  setBook((b) => ({
    ...b,
    links: b.links.map((l) =>
      ids.has(l.id) ? advanceLink(l, { state: "cancelled", settledAt: now }, now) : l,
    ),
  }));
  for (const link of held) releaseLimitWaitLink(link.target, link.id);
  return held.length;
}

/** Forget a finished link's chip. */
export function dismissLink(id: string) {
  setBook((b) => ({ ...b, links: b.links.filter((l) => l.id !== id || isActive(l.state)) }));
}

/** "Keep this": the fan-out's other threads are done, and the kept one goes to review. */
export async function keepFanOut(fanId: string, keep: string) {
  const group = book().fanOuts.find((f) => f.id === fanId);
  if (!group) return;
  const targets = group.links.flatMap((id) => book().links.find((l) => l.id === id)?.target ?? []);
  for (const target of targets) {
    if (target !== keep) await setThreadStatus(target, "done");
  }
  await setThreadStatus(keep, "review");
  setBook((b) => ({ ...b, fanOuts: b.fanOuts.map((f) => (f.id === fanId ? { ...f, kept: keep } : f)) }));
}

/** The links of a thread, both ways, for chips, banners and badges. */
export function linksOf(b: LinkBook, id: string) {
  return {
    driving: b.links.filter((l) => l.orchestrator === id).slice(-MAX_DELEGATES_PER_THREAD * 2),
    drivenBy: [...b.links].reverse().find((l) => l.target === id && isActive(l.state)),
  };
}
