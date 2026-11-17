import { isLoaded } from "./thread";
// Run state: the poll that reads which threads the engine is running, and folding that list into
// the threads this window follows. Split out of state/store.ts; the state these write still lives in
// the app store.

import type { SessionSummary } from "../acp/types";
import { reloadThread } from "./lostTurns";
import { type DriftReads, engineBusy, noDrift, pollDelay, reconcileRuns } from "./runState";
import { type AppState, connection, refreshSessions, updateThread, useApp } from "./store";
import { finishTurn, isRunning } from "./thread";
import { isRemoteThread } from "./threadKeys";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

/** What the last list reads said about threads that disagree with this window, by thread. */
let driftReads: DriftReads = noDrift;

/**
 * Compares one read of this computer's thread list with the window's own `running` flags
 * (state/runState.ts). A turn the window still shows running after the engine called its thread
 * idle on two reads has ended unseen (a sleep, a dropped connection): it is closed here and read
 * again from the engine, so the row and the composer stop saying Working. A thread the engine runs
 * and the window does not know of reads as running (`engineRunning`) until the engine says it stopped.
 */
export function reconcileRunState(listed: readonly SessionSummary[]): void {
  const state = get();
  const local = Object.values(state.threads)
    .filter((t) => !isRemoteThread(t.id))
    .map((t) => ({ id: t.id, running: isRunning(t), startedAt: t.turn?.startedAt }));
  const result = reconcileRuns(
    local,
    listed.filter((x) => !x.host).map((x) => ({ id: x.id, status: x.status })),
    driftReads,
    Date.now(),
  );
  driftReads = result.reads;
  const adopted = new Set(result.adopt);
  const before = Object.keys(state.engineRunning);
  if (before.length !== adopted.size || before.some((id) => !adopted.has(id))) {
    set({ engineRunning: Object.fromEntries(result.adopt.map((id) => [id, true as const])) });
  }
  for (const id of result.settle) {
    updateThread(id, (t) => (isRunning(t) ? finishTurn(t, "completed", Date.now()) : t));
    // What the turn left is the engine's to say: read the thread again.
    if (isLoaded(get().threads[id])) void reloadThread(id);
  }
}

/** Whether something runs, or is in doubt, so the list is worth reading often. */
function runStateActive(): boolean {
  const s = get();
  if (Object.keys(s.engineRunning).length) return true;
  if (Object.keys(driftReads.stale).length || Object.keys(driftReads.missed).length) return true;
  if (Object.values(s.threads).some((t) => isRunning(t))) return true;
  return s.sessions.some((x) => !x.host && engineBusy(x.status));
}

let runPollTimer: ReturnType<typeof setTimeout> | undefined;
let runPollReading = false;

/** Forgets which threads looked out of step, after an engine restart reset them all. */
export function forgetRunDrift(): void {
  driftReads = noDrift;
}

/** Stops the poll and forgets what it read, for the store's reset between tests. */
export function resetRunStatePoll(): void {
  driftReads = noDrift;
  clearTimeout(runPollTimer);
  runPollTimer = undefined;
  runPollReading = false;
}

/**
 * Reads the engine's thread list on a timer while the window is visible, so the sidebar follows
 * the engine within a few seconds without a focus or a turn to prompt it: every 5 s while anything
 * runs, every 30 s otherwise (a thread another client starts). Started once; each handshake (a
 * restart or a reconnect) also reads the list itself.
 */
export function startRunStatePoll(): void {
  if (runPollTimer !== undefined) return;
  const tick = async () => {
    if (
      !runPollReading &&
      connection &&
      get().engine.phase === "ready" &&
      (typeof document === "undefined" || !document.hidden)
    ) {
      runPollReading = true;
      try {
        await refreshSessions({ quiet: true });
      } finally {
        runPollReading = false;
      }
    }
    runPollTimer = setTimeout(() => void tick(), pollDelay(runStateActive()));
  };
  runPollTimer = setTimeout(() => void tick(), pollDelay(false));
}
