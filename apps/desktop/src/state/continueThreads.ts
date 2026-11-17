import { isLoaded } from "./thread";
// Continuing threads by themselves: one that waits for a usage limit to reset or for the internet
// to come back, and one a restart of the app cut off mid-turn. Split out of state/store.ts; the
// state these write still lives in the app store.

import { interruptedLinks } from "./host";
import {
  INTERNET_POLL_MS,
  INTERNET_RESUME_TEXT,
  type InternetWait,
  internetDue,
  internetGaveUp,
  internetHolds,
  networkStop,
  nextInternetWait,
} from "./internetWait";
import {
  type LimitWait,
  limitReset,
  nextWait,
  overloadDelay,
  overloadStop,
  resumeText,
  waitDue,
  waitHolds,
} from "./limitResume";
import { notifyThread } from "./notify";
import {
  markStopped,
  pruneResumed,
  RESUME_MAX,
  RESUME_NOTICE,
  type ResumeBook,
  resumeVerdict,
} from "./restartResume";
import {
  type AppState,
  accountOn,
  harnessOf,
  limitMoveFor,
  planKeyFor,
  runPrompt,
  STORAGE,
  store,
  storedJson,
  threadIsGone,
  toast,
  useApp,
} from "./store";
import { isRunning, RESUME_MARK } from "./thread";
import { threadName } from "./threadNotices";
import { openThread } from "./threadOpen";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

// ---------------------------------------------------------------- waiting to continue

// When the connection was first seen up again, by thread: a wait's stable period counts from it.
// Memory only, so a wait saved across a restart simply needs the period again.
const upSince = new Map<string, number>();
// The threads whose resume turn this tick already sent, so one wait sends one turn.
const sending = new Set<string>();

function storeLimitWaits(next: Record<string, LimitWait>) {
  set({ limitWaits: next });
  store(STORAGE.limitWaits, JSON.stringify(next));
  syncWaitTimers();
}

function storeInternetWaits(next: Record<string, InternetWait>) {
  set({ internetWaits: next });
  store(STORAGE.internetWaits, JSON.stringify(next));
  syncWaitTimers();
}

/** The user's call, or something they did that makes the wait moot: no turn goes out for it. */
/**
 * A parent let go of a link a thread's wait holds (the user stopped or archived it): the wait keeps
 * the links of other parents, and ends with the last one.
 */
export function releaseLimitWaitLink(thread: string, link: string) {
  const wait = get().limitWaits[thread];
  if (!wait) return;
  const rest = (wait.links ?? []).filter((x) => x !== link);
  if (rest.length) storeLimitWaits({ ...get().limitWaits, [thread]: { ...wait, links: rest } });
  else dropLimitWait(thread);
}

export function dropLimitWait(id: string) {
  const { [id]: gone, ...rest } = get().limitWaits;
  if (gone) storeLimitWaits(rest);
}

/** The same for a thread waiting for the internet, and the moment its connection came back. */
export function dropInternetWait(id: string) {
  upSince.delete(id);
  const { [id]: gone, ...rest } = get().internetWaits;
  if (gone) storeInternetWaits(rest);
}

/** Both waits of a thread: its cancel, or a new turn of the user's that makes them moot. */
export function dropWaits(id: string) {
  dropLimitWait(id);
  dropInternetWait(id);
}

/** The time a wait ends, as the row and the toast say it. */
export function limitTime(until: number, now = Date.now()): string {
  const at = new Date(until);
  const time = at.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  if (new Date(now).toDateString() === at.toDateString()) return time;
  return `${at.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}, ${time}`;
}

/** A turn that failed because the machine had no network waits for it to come back. */
function waitForInternet(id: string, now: number) {
  const s = get();
  const wait = nextInternetWait(s.internetWaits[id], now, {
    harness: harnessOf(s, id),
    account: accountOn(s, id),
  });
  storeInternetWaits({ ...s.internetWaits, [id]: wait });
}

/**
 * A turn ended. On a plan limit it waits for the reset; on a provider overload it
 * waits out a doubling backoff; on a network failure it waits for the internet; on
 * anything else every wait is over. The reason is read from the failure or, as
 * Claude Code says a limit, from the reply. A stop by the user always ends them.
 * Returns the limit-or-overload wait the turn started.
 */
export function settleStopWait(
  id: string,
  outcome: "completed" | "cancelled" | "failed",
  failure: string | undefined,
  startIndex: number,
  turnLinks: readonly (string | undefined)[] = [],
): LimitWait | null {
  const s = get();
  if (!s.continueAfterLimit || outcome === "cancelled") {
    dropWaits(id);
    return null;
  }
  const reply = s.threads[id]?.items.slice(startIndex).findLast((i) => i.kind === "assistant");
  const replied = reply?.kind === "assistant" ? reply.text : undefined;
  const key = planKeyFor(s, id);
  // The thread's own windows first: a thread on a paired machine has no entry
  // in this computer's cache, and its own are its login's anyway.
  const windows = s.planUsage[id]?.windows ?? (key ? s.planCache[key]?.windows : undefined) ?? [];
  const now = Date.now();
  // Either can say the limit: Claude Code writes it as the reply, and again as the turn's error
  // (`Internal error: You've hit your 5-hour limit · resets 2:40am (Europe/Oslo): {"errorKind":…}`).
  const resetAt = limitReset(failure, now, windows) ?? limitReset(replied, now, windows);
  const overload = resetAt === null && overloadStop(failure ?? replied);
  if (resetAt === null && !overload) {
    // Not a stop that waits for a time: the network may be what the turn lost.
    dropLimitWait(id);
    if (networkStop(failure, replied)) waitForInternet(id, now);
    else dropInternetWait(id);
    return null;
  }
  // A turn stopped on a limit or an overload is not also waiting for the internet.
  dropInternetWait(id);
  const reason = overload ? ("overload" as const) : ("limit" as const);
  const prior = s.limitWaits[id];
  // The attempt this one is: a wait that changed kind starts its own count (as `nextWait` does).
  const attempt = prior && (prior.reason ?? "limit") === reason ? prior.attempts + 1 : 1;
  const until = overload ? now + overloadDelay(attempt) : resetAt!;
  const wait = nextWait(prior, {
    until,
    harness: harnessOf(s, id),
    account: accountOn(s, id),
    reason,
    // The links this stop settled, kept across waits of the same stop so the turn that gets
    // through reopens and answers them.
    links: [...new Set([...(prior?.links ?? []), ...turnLinks.filter((x): x is string => !!x)])],
  });
  if (!wait) {
    dropLimitWait(id);
    return null;
  }
  storeLimitWaits({ ...get().limitWaits, [id]: wait });
  // A delegate that goes on elsewhere at once is told about by the move, not by a wait (`switchOnLimit`).
  if (!overload && !limitMoveFor(id, wait))
    notifyThread({ threadId: id, kind: "limit", key: String(wait.until), title: threadName(id) }, get());
  return wait;
}

let waitTimer: ReturnType<typeof setInterval> | null = null;

/** Stops the wait timer and forgets what this tick saw and sent, for the store's reset between tests. */
export function resetContinueWaits(): void {
  if (waitTimer) clearInterval(waitTimer);
  waitTimer = null;
  upSince.clear();
  sending.clear();
}

/**
 * One timer while anything waits, on the internet's own beat: a Mac that slept
 * wakes past a reset or past the connection's return and still goes on.
 */
export function syncWaitTimers() {
  const waiting = Object.keys(get().limitWaits).length > 0 || Object.keys(get().internetWaits).length > 0;
  if (waiting && !waitTimer) waitTimer = setInterval(tickWaits, INTERNET_POLL_MS);
  else if (!waiting && waitTimer) {
    clearInterval(waitTimer);
    waitTimer = null;
  }
}

/** One tick for both kinds of wait: a reset or backoff falling due, and the internet coming back. */
function tickWaits() {
  void continueDueThreads();
  void continueInternetThreads();
}

// The machine's own word on whether it is online is asked at once when it changes, rather than
// waiting for the next tick: a network that comes back just after one is not waited out.
if (typeof window !== "undefined") {
  const nowOnline = () => {
    // A Mac that slept past a reset wakes with focus and visibility events: the wait goes on at once.
    if (Object.keys(get().internetWaits).length > 0 || Object.keys(get().limitWaits).length > 0) tickWaits();
  };
  window.addEventListener("online", nowOnline);
  window.addEventListener("focus", nowOnline);
  if (typeof document !== "undefined")
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible") nowOnline();
    });
}

/** Whether the machine has a network at all: the webview's own word, with no probe of ours. */
function networkUp(): boolean {
  try {
    return typeof navigator === "undefined" || navigator.onLine !== false;
  } catch {
    return true;
  }
}

let continuing = false;
/** The threads a restart is about to resume (`resumeInterrupted`): their limit wait is that turn's to settle. */
const resuming = new Set<string>();

/** Each thread whose limit or overload has passed carries on, unless it now runs on something else. */
export async function continueDueThreads(now = Date.now()): Promise<void> {
  if (continuing || get().status !== "ready") return;
  continuing = true;
  try {
    for (const [id, wait] of Object.entries(get().limitWaits)) {
      // A thread missing from a list that is not whole keeps its wait: it may be past the cap.
      if (threadIsGone(id) || get().archived[id]) {
        dropLimitWait(id);
        continue;
      }
      // Past the cap and not open: its harness and login cannot be read yet, so it waits as it is.
      if (!get().sessions.some((x) => x.id === id) && !get().threads[id]) continue;
      // A turn the restart cut short is sent its own resume, which ends this wait: never both.
      if (resuming.has(id)) continue;
      const holds = () => {
        const s = get();
        return waitHolds(wait, { harness: harnessOf(s, id), account: accountOn(s, id) });
      };
      // Switched already: another harness or login is not this stop, so nothing starts on its own.
      if (!holds()) {
        dropLimitWait(id);
        continue;
      }
      if (!waitDue(wait, now)) continue;
      await openThread(id);
      for (let i = 0; i < 100 && !isLoaded(get().threads[id]); i++) {
        await new Promise((r) => setTimeout(r, 100));
      }
      const thread = get().threads[id];
      if (!isLoaded(thread) || isRunning(thread)) continue;
      if (!holds()) {
        dropLimitWait(id);
        continue;
      }
      void runPrompt(id, resumeText(wait), undefined, {
        automatic: true,
        autoResume: true,
        resumeLinks: wait.links,
      });
    }
  } finally {
    continuing = false;
  }
}

let continuingNet = false;

/**
 * Each thread waiting for the internet goes on once the connection has stayed up
 * for the period its attempt count asks for (state/internetWait.ts); a wait that
 * outlives its patience says so and stops. Nothing runs while no thread waits.
 */
export async function continueInternetThreads(now = Date.now()): Promise<void> {
  if (continuingNet) return;
  const ids = Object.keys(get().internetWaits);
  if (!ids.length) return;
  if (!networkUp()) {
    for (const id of ids) upSince.delete(id);
    return;
  }
  continuingNet = true;
  try {
    for (const [id, wait] of Object.entries(get().internetWaits)) {
      if (threadIsGone(id) || get().archived[id]) {
        dropInternetWait(id);
        continue;
      }
      // Past the cap and not open: its harness and login cannot be read yet, so it waits as it is.
      if (!get().sessions.some((x) => x.id === id) && !get().threads[id]) continue;
      const holds = () => {
        const s = get();
        return internetHolds(wait, { harness: harnessOf(s, id), account: accountOn(s, id) });
      };
      if (!holds()) {
        dropInternetWait(id);
        continue;
      }
      if (internetGaveUp(wait, now)) {
        dropInternetWait(id);
        toast(`“${threadName(id)}” stopped waiting for the internet. Send it a message when you are back.`);
        continue;
      }
      const since = upSince.get(id) ?? now;
      upSince.set(id, since);
      if (sending.has(id) || !internetDue(wait, since, now) || get().status !== "ready") continue;
      await openThread(id);
      for (let i = 0; i < 100 && !isLoaded(get().threads[id]); i++) {
        await new Promise((r) => setTimeout(r, 100));
      }
      const thread = get().threads[id];
      if (!isLoaded(thread) || isRunning(thread)) continue;
      if (!holds()) {
        dropInternetWait(id);
        continue;
      }
      // Counted before it goes out: a flaky connection that drops this turn again asks for a longer pause.
      storeInternetWaits({ ...get().internetWaits, [id]: { ...wait, attempts: wait.attempts + 1 } });
      upSince.delete(id);
      sending.add(id);
      void runPrompt(id, INTERNET_RESUME_TEXT, undefined, {
        automatic: true,
        autoResume: true,
      }).finally(() => sending.delete(id));
    }
  } finally {
    continuingNet = false;
  }
}

/** The user's "Send now": the thread goes on at once instead of waiting out its stop. */
export function resumeWaitNow(id: string) {
  const wait = get().limitWaits[id];
  const offline = get().internetWaits[id];
  if (!wait && !offline) return;
  dropWaits(id);
  if (wait) void runPrompt(id, resumeText(wait), undefined, { automatic: true, resumeLinks: wait.links });
  else void runPrompt(id, INTERNET_RESUME_TEXT, undefined, { automatic: true });
}

/** The row's Cancel: this thread is not continuing on its own. */
export function cancelWait(id: string) {
  dropWaits(id);
}

export function setContinueAfterLimit(on: boolean) {
  set({ continueAfterLimit: on });
  store(STORAGE.continueAfterLimit, on ? "1" : "0");
  if (!on) storeWaitsCleared();
}

/** Turning the waits off drops both kinds, without two writes of a half-cleared state. */
function storeWaitsCleared() {
  storeLimitWaits({});
  storeInternetWaits({});
}

// ---------------------------------------------------------------- resuming after a restart

/** The user stopped this thread's turn: a restart must not bring it back (`state/restartResume.ts`). */
export function noteStopped(id: string) {
  const stopped = markStopped(storedJson<string[]>(STORAGE.stoppedThreads, []), id);
  store(STORAGE.stoppedThreads, JSON.stringify(stopped));
}

/** A turn starts on the thread: whatever the user stopped before it no longer holds. */
export function clearStopped(id: string) {
  const stopped = storedJson<string[]>(STORAGE.stoppedThreads, []);
  if (stopped.includes(id)) {
    store(STORAGE.stoppedThreads, JSON.stringify(stopped.filter((x) => x !== id)));
  }
}

/** What an interrupted thread is told when it continues. */
export const RESUME_TEXT = `${RESUME_NOTICE}.\nThe app restarted while you were working, so your last turn stopped. Continue where you left off.\n\n${RESUME_MARK}`;

const runningIds = (s: AppState) =>
  Object.values(s.threads)
    .filter((t) => isRunning(t))
    .map((t) => t.id)
    .sort();

let lastRunning = "";

/**
 * Writes the running threads while the engine is up, so a quit, a crash, an update or an engine
 * restart leaves the list of turns it cut short. A turn that ends takes itself off it. The store
 * calls this once it is created.
 */
export function watchRunningThreads(): void {
  useApp.subscribe((s) => {
    if (s.status !== "ready") return;
    const ids = runningIds(s).join("\n");
    if (ids === lastRunning) return;
    lastRunning = ids;
    store(STORAGE.runningThreads, JSON.stringify(ids ? ids.split("\n") : []));
  });
}

/** The threads a restart cut off: the saved list, plus any this app still showed running. */
export function interruptedThreads(): string[] {
  const saved = storedJson<unknown>(STORAGE.runningThreads, []);
  const ids = new Set([
    ...(Array.isArray(saved) ? saved.filter((x): x is string => typeof x === "string") : []),
    ...runningIds(get()),
  ]);
  return [...ids];
}

/**
 * Each thread a restart cut off is opened and sent {@link RESUME_TEXT}, so it carries on
 * without the user pressing anything. Settings › General turns it off. Up to {@link RESUME_MAX}
 * are resumed, and only the ones that really get a turn count: a thread the app cannot open, or
 * one whose resume the app could not send, does not use up someone else's slot.
 */
export async function resumeInterrupted(ids: string[]): Promise<void> {
  store(STORAGE.runningThreads, "[]");
  lastRunning = "";
  if (!get().resumeAfterRestart || !ids.length) return;
  const known = new Set(get().sessions.map((x) => x.id));
  // Marked before the first await: the limit waits are checked at the same moment, and a thread
  // whose continue was cut short by the restart would otherwise be sent two turns.
  const now = Date.now();
  const book: ResumeBook = {
    stopped: storedJson<string[]>(STORAGE.stoppedThreads, []),
    resumedAt: pruneResumed(storedJson<Record<string, number>>(STORAGE.resumedThreads, {}), now),
  };
  // The user's stops only guard the restart that just happened.
  store(STORAGE.stoppedThreads, "[]");
  for (const id of ids) if (resumeVerdict(id, known, book, now) === "resume") resuming.add(id);
  try {
    await resumeAll(ids, known, book, now);
  } finally {
    resuming.clear();
  }
}

async function resumeAll(
  ids: string[],
  known: ReadonlySet<string>,
  book: ResumeBook,
  now: number,
): Promise<void> {
  let started = 0;
  const resumedAt = { ...book.resumedAt };
  for (const id of ids) {
    if (started >= RESUME_MAX) break;
    if (resumeVerdict(id, known, book, now) !== "resume") continue;
    await openThread(id);
    // The selected thread may already be loading from the handshake: wait for it.
    for (let i = 0; i < 100 && !isLoaded(get().threads[id]); i++)
      await new Promise((r) => setTimeout(r, 100));
    const thread = get().threads[id];
    if (!isLoaded(thread)) continue;
    started++;
    // Noted once the thread really gets the turn: a resume cut off again soon is the user's to restart.
    resumedAt[id] = now;
    store(STORAGE.resumedThreads, JSON.stringify(resumedAt));
    // The links the restart severed are this turn's to answer, not any later turn's
    // (state/host.ts `linksResumed`, `linksFinished`): the resume carries them, so a parent that
    // was waiting reads the answer in a wake instead of waiting for good. A thread whose turn still
    // runs (the engine outlived the window) takes the resume behind it as a queued row, which then
    // starts one that carries the links — dropping it would settle nothing again.
    void runPrompt(id, RESUME_TEXT, undefined, {
      automatic: true,
      resumeLinks: interruptedLinks(id),
    });
  }
  if (started) {
    toast(
      started === 1
        ? "A thread the restart interrupted is continuing."
        : `${started} threads the restart interrupted are continuing.`,
      "success",
    );
  }
}
