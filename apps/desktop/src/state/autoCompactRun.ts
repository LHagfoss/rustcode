// Auto-compaction run by the app: the per-thread timers, the `/compact` turn it sends, and the
// notice that turn leaves behind. The decision is pure (`state/autoCompact.ts`); this file owns the
// clock and the side effects. Split out of state/store.ts; the settings it writes (`autoCompact`,
// `autoCompactOff`) still live in the app store.

import type { SessionUsage } from "../acp/types";
import { delegateParent } from "../acp/types";
import {
  type AutoCompactMode,
  type AutoCompactSettings,
  autoCompactDecision,
  autoCompactingText,
  autoCompactMark,
  autoCompactPolicy,
  CONTINUE_AFTER_COMPACTION_TEXT,
  clampAutoCompactThreshold,
  shouldContinueAfterCompaction,
} from "./autoCompact";
import { emptyExtras, hasExtras } from "./compose";
import { linksOf } from "./host";
import { isActive } from "./orchestration";
import {
  type AppState,
  cancel,
  commandsFor,
  harnessOf,
  runPrompt,
  STORAGE,
  sessionById,
  statusOf,
  store,
  type TurnOutcome,
  updateThread,
  useApp,
} from "./store";
import { isRunning, localId, settleCompaction, type ThreadState } from "./thread";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);
const get = () => useApp.getState();

/** How recently a delegate's parent must have worked for it to still count as talking to it. */
const DELEGATE_PARENT_ACTIVE_MS = 10 * 60_000;
/** One timer per thread: the next cache deadline, or an immediate size rule. */
const autoCompactTimers = new Map<string, ReturnType<typeof setTimeout>>();
/** Threads that already auto-compacted in this idle period, until the next turn starts. */
const autoCompactedIdle = new Set<string>();
/** The thread an auto-compaction runs in, so its card can say it ran on its own. */
const autoCompactInFlight = new Set<string>();
const autoCompactCardSeen = new Set<string>();
/** A compaction with no card waits this long for the usage that proves it moved. */
const AUTO_COMPACT_USAGE_WAIT_MS = 10_000;
/** Compactions whose notice waits for the post-compaction usage update. */
const autoCompactPendingNotice = new Set<string>();
const autoCompactNoticeTimers = new Set<ReturnType<typeof setTimeout>>();
/** The context before each automatic compaction, for the inspector's before → after. */
const autoCompactBefore = new Map<string, SessionUsage>();

function clearAutoCompactTimer(id: string): boolean {
  const timer = autoCompactTimers.get(id);
  if (timer === undefined) return false;
  clearTimeout(timer);
  autoCompactTimers.delete(id);
  return true;
}

export function setAutoCompactMode(mode: AutoCompactMode) {
  set((s) => ({ autoCompact: { ...s.autoCompact, mode } }));
  store(STORAGE.autoCompact, JSON.stringify(get().autoCompact));
  rescheduleAutoCompact();
}

export function setAutoCompactThreshold(tokens: number) {
  const threshold = clampAutoCompactThreshold(tokens);
  set((s) => ({ autoCompact: { ...s.autoCompact, threshold } }));
  store(STORAGE.autoCompact, JSON.stringify(get().autoCompact));
  rescheduleAutoCompact();
}

/** The thread's menu switch: true turns auto-compaction on for that thread again. */
export function setAutoCompactForThread(id: string, on: boolean) {
  set((s) => {
    const next = { ...s.autoCompactOff };
    if (on) delete next[id];
    else next[id] = true;
    return { autoCompactOff: next };
  });
  store(STORAGE.autoCompactOff, JSON.stringify(get().autoCompactOff));
  if (on) {
    autoCompactedIdle.delete(id);
    scheduleAutoCompact(id);
  } else {
    clearAutoCompactTimer(id);
  }
}

export function autoCompactOffFor(s: Pick<AppState, "autoCompactOff">, id: string): boolean {
  return s.autoCompactOff[id] === true;
}

function autoCompactSettingsFor(s: AppState, id: string): AutoCompactSettings {
  return autoCompactOffFor(s, id) ? { ...s.autoCompact, mode: "off" } : s.autoCompact;
}

function autoCompactPolicyFor(s: AppState, id: string) {
  const harness = harnessOf(s, id);
  const known = s.harnesses?.harnesses.find((h) => h.id === harness);
  return autoCompactPolicy(harness ?? "", known?.kind);
}

/** The threshold the context bar marks for this thread, if rule 1 can run on it. */
export function autoCompactThresholdFor(s: AppState, id: string): number | undefined {
  return autoCompactMark(autoCompactSettingsFor(s, id), autoCompactPolicyFor(s, id));
}

/** Whether the harness runs its own compaction. An ACP agent says so by advertising `/compact`. */
function canCompact(s: AppState, id: string): boolean {
  const harness = harnessOf(s, id);
  const known = s.harnesses?.harnesses.find((h) => h.id === harness);
  if (!known) return false;
  if (known.builtin || known.kind === "codex") return true;
  // Claude Code takes `/compact` whether or not its agent lists it as a command.
  if (known.id === "claude-code") return true;
  return commandsFor(s, id).some((command) => command.name === "/compact");
}

/** When the thread's last turn ended: where its prompt cache clock starts. */
function lastTurnEndedAt(thread: ThreadState | undefined): number | undefined {
  for (let i = (thread?.items.length ?? 0) - 1; i >= 0; i -= 1) {
    const item = thread!.items[i]!;
    if (item.kind === "turnEnd" && item.at !== undefined) return item.at;
  }
  return undefined;
}

/**
 * Whether the user stopped the thread's last turn, or it failed. The app's own quiet turns leave no
 * footer, so the last `turnEnd` is the last turn the thread ran for its work.
 */
function lastTurnInterrupted(thread: ThreadState | undefined): boolean {
  for (let i = (thread?.items.length ?? 0) - 1; i >= 0; i -= 1) {
    const item = thread!.items[i]!;
    if (item.kind === "turnEnd") return item.outcome !== "completed";
  }
  return false;
}

/**
 * Whether the thread still owes work it started: a delegate that is still working, or a command it
 * left running in the background. A thread that finished what it was doing has none, and a
 * compaction is not a reason to hand it another turn.
 */
function workInFlightOn(s: AppState, id: string): boolean {
  if (linksOf(s.links, id).driving.some((link) => isActive(link.state))) return true;
  return (s.threads[id]?.waiting ?? 0) > 0;
}

function autoCompactInput(s: AppState, id: string, now: number) {
  const thread = s.threads[id];
  const links = linksOf(s.links, id);
  const parentId = delegateParent(sessionById(s, id)?.parent)?.id;
  const parent = parentId ? s.sessions.find((x) => x.id === parentId) : undefined;
  const delegated = !!parentId || links.drivenBy !== undefined;
  // A delegate is kept only while its orchestrator is still talking to it: a
  // live link, or a parent turn/wake in the last ten minutes. A settled or
  // merely existing parent is not enough.
  const parentActive = parent?.updatedAt !== undefined && now - parent.updatedAt <= DELEGATE_PARENT_ACTIVE_MS;
  const orchestratorTalking = links.drivenBy !== undefined || parentActive;
  return {
    settings: autoCompactSettingsFor(s, id),
    policy: autoCompactPolicyFor(s, id),
    canCompact: canCompact(s, id),
    usedTokens: thread?.usage?.used,
    now,
    idleSince: lastTurnEndedAt(thread),
    status: statusOf(s, id),
    running: isRunning(thread),
    queued: thread?.queue.length ?? 0,
    approvals: thread?.permissions.length ?? 0,
    waiting: (thread?.waiting ?? 0) > 0,
    limitWaiting: s.limitWaits[id] !== undefined || s.internetWaits[id] !== undefined,
    snoozed: id in s.snoozed,
    // Done or Failed until the row is opened (`markFinished`, `clearFinished`). A delegate is
    // read by its orchestrator, not by the user, so its own flag says nothing about what they read.
    unread: !delegated && (id in s.finished || id in s.failed),
    typing: !!(s.drafts[id] ?? "").trim() || hasExtras(s.extras[id]),
    compactedThisIdle: autoCompactedIdle.has(id),
    delegated,
    orchestratorTalking,
  };
}

/** Whether a usage gauge shows the compaction the app asked for. */
function contextMoved(before: SessionUsage | undefined, after: SessionUsage | undefined): boolean {
  return (
    !!after &&
    (!before ||
      after.used < before.used ||
      (after.context?.compactions ?? 0) > (before.context?.compactions ?? 0))
  );
}

/**
 * The quiet row for a compaction whose harness sends no card (Claude Code).
 * It waits for the usage update that proves the context moved, so it does not
 * race the turn's reply.
 */
function settleAutoCompactNotice(id: string): void {
  if (!autoCompactPendingNotice.has(id)) return;
  autoCompactPendingNotice.delete(id);
  const before = autoCompactBefore.get(id);
  if (autoCompactCardSeen.has(id)) return;
  // The usage never proved it moved: "Compacting context…" must not stay up for ever.
  if (!contextMoved(before, get().threads[id]?.usage)) {
    updateThread(id, (thread) => settleCompaction(thread, null));
    return;
  }
  updateThread(id, (thread) => settleCompaction(thread, compactedNotice()));
}

/** The quiet row an automatic compaction leaves when its harness sent no card. */
function compactedNotice() {
  return {
    kind: "notice",
    id: localId("compaction"),
    text: "Compacted automatically.",
    tone: "info",
    setting: "auto-compact",
  } as const;
}

function runAutoCompact(id: string): void {
  const s = get();
  const decision = autoCompactDecision(autoCompactInput(s, id, Date.now()));
  if (decision.action !== "compact") return;
  autoCompactedIdle.add(id);
  const before = s.threads[id]?.usage;
  if (before) autoCompactBefore.set(id, before);
  autoCompactInFlight.add(id);
  autoCompactCardSeen.delete(id);
  autoCompactPendingNotice.delete(id);
  // What the compaction is about to hold up, read before the app's own turn starts: that turn clears
  // the thread's waiting state (thread.ts `beginQuietTurn`), and a compaction must not lose work.
  const wasWorking = workInFlightOn(s, id);
  const userStopped = lastTurnInterrupted(s.threads[id]);
  // Where the transcript is now: whatever a turn writes after it ran while the compaction did.
  const itemsBefore = s.threads[id]?.items.length ?? 0;
  void runPrompt(id, "/compact", emptyExtras, {
    automatic: true,
    autoCompact: true,
    autoCompactText: autoCompactingText(decision.rule, s.autoCompact.threshold),
  })
    // The compaction's own row is settled whether or not the prompt became a turn.
    .then(
      (outcome) => {
        settleAutoCompactTurn(id, before);
        return outcome;
      },
      () => {
        settleAutoCompactTurn(id, before);
        return "skipped" as TurnOutcome;
      },
    )
    .then((outcome) => {
      const after = get();
      const thread = after.threads[id];
      // A turn that ran since the compaction started: the queue's, which `runPrompt` starts when the
      // compaction's own turn ends. It is the thread's next step already.
      const carriedOn = (thread?.items.slice(itemsBefore) ?? []).some((item) => item.kind === "turnEnd");
      if (
        !shouldContinueAfterCompaction({
          outcome,
          // Read before the compaction, and again now: the work it held up must still be owed. A
          // delegate that settled while it ran is carried by its wake, not by a second turn.
          workInFlight: wasWorking && workInFlightOn(after, id),
          running: isRunning(thread),
          queued: thread?.queue.length ?? 0,
          // What the user is composing counts as having the thread back, text or attachments.
          typing: !!(after.drafts[id] ?? "").trim() || hasExtras(after.extras[id]),
          interrupted: userStopped,
          carriedOn,
          putAway: id in after.snoozed || statusOf(after, id) === "done",
        })
      ) {
        return;
      }
      // The compaction itself is the only turn that ran: the thread carries on with the work it
      // still owed, as its own divider row rather than a message the user typed.
      void runPrompt(id, CONTINUE_AFTER_COMPACTION_TEXT, emptyExtras, {
        automatic: true,
        continued: true,
      }).catch(() => undefined);
    })
    .catch(() => undefined);
}

/**
 * The app's compaction turn ended: its "Compacting context…" row becomes the card (a harness that
 * sends one), the "Compacted automatically" row, or the reason nothing came of it.
 */
function settleAutoCompactTurn(id: string, before: SessionUsage | undefined): void {
  autoCompactInFlight.delete(id);
  if (autoCompactCardSeen.has(id)) {
    autoCompactPendingNotice.delete(id);
    autoCompactCardSeen.delete(id);
    return;
  }
  if (contextMoved(before, get().threads[id]?.usage)) {
    updateThread(id, (thread) => settleCompaction(thread, compactedNotice()));
    return;
  }
  // The harness may send its usage just after the prompt returns: wait for
  // that event, bounded, instead of reading a stale gauge.
  autoCompactPendingNotice.add(id);
  const timer = setTimeout(() => {
    autoCompactNoticeTimers.delete(timer);
    settleAutoCompactNotice(id);
  }, AUTO_COMPACT_USAGE_WAIT_MS);
  autoCompactNoticeTimers.add(timer);
}

/**
 * Look at one thread and set its next auto-compaction timer. Called when a
 * turn ends, a thread opens, a draft changes or the setting does; the decision
 * itself is pure (`state/autoCompact.ts`).
 */
export function scheduleAutoCompact(id: string): void {
  clearAutoCompactTimer(id);
  const s = get();
  if (!s.threads[id]?.loaded) return;
  const decision = autoCompactDecision(autoCompactInput(s, id, Date.now()));
  if (decision.action === "compact") {
    autoCompactTimers.set(
      id,
      setTimeout(() => {
        autoCompactTimers.delete(id);
        runAutoCompact(id);
      }, 0),
    );
  } else if (decision.action === "wait") {
    // Timers cap at a day; a longer wait is rescheduled by the next check.
    const delay = Math.min(Math.max(0, decision.at - Date.now()), 24 * 60 * 60_000);
    autoCompactTimers.set(
      id,
      setTimeout(() => {
        autoCompactTimers.delete(id);
        scheduleAutoCompact(id);
      }, delay),
    );
  }
}

/** The user typing in a thread ends its idle period: give it back to them. */
export function noteTyping(id: string): void {
  if (!get().threads[id]) return;
  clearAutoCompactTimer(id);
  autoCompactedIdle.add(id);
  // Typing in a thread whose automatic compaction already runs stops it, so
  // the user gets the thread back rather than waiting for a summary.
  if (autoCompactInFlight.has(id)) void cancel(id);
}

/** Start a fresh idle period: the next turn may auto-compact once more. */
export function beginAutoCompactIdle(id: string): void {
  autoCompactedIdle.delete(id);
  autoCompactBefore.delete(id);
}

function rescheduleAutoCompact(): void {
  for (const id of Object.keys(get().threads)) scheduleAutoCompact(id);
}

/** The context an automatic compaction started from, for the inspector's comparison. */
export function autoCompactBeforeUsage(id: string): SessionUsage | undefined {
  return autoCompactBefore.get(id);
}

/** Drop every timer and per-thread record, for tests and when a thread is trashed. */
export function forgetAutoCompact(id?: string): void {
  if (id === undefined) {
    for (const timer of autoCompactTimers.values()) clearTimeout(timer);
    for (const timer of autoCompactNoticeTimers) clearTimeout(timer);
    autoCompactTimers.clear();
    autoCompactNoticeTimers.clear();
    autoCompactedIdle.clear();
    autoCompactInFlight.clear();
    autoCompactCardSeen.clear();
    autoCompactPendingNotice.clear();
    autoCompactBefore.clear();
    return;
  }
  clearAutoCompactTimer(id);
  autoCompactedIdle.delete(id);
  autoCompactInFlight.delete(id);
  autoCompactCardSeen.delete(id);
  autoCompactPendingNotice.delete(id);
  autoCompactBefore.delete(id);
}

/**
 * A compaction card arrived for the thread. When the app's own compaction runs there, the card is
 * that compaction's (so no "Compacted automatically" notice follows) and this returns true.
 */
export function noteCompactionCard(id: string): boolean {
  if (!autoCompactInFlight.has(id)) return false;
  autoCompactCardSeen.add(id);
  autoCompactPendingNotice.delete(id);
  return true;
}
