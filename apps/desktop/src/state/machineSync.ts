// Machine sync: folding paired machines' thread lists into this one, and flushing the changes made
// to a machine's threads while it was away (its outbox) once it is back. Split out of state/store.ts;
// the state these write still lives in the app store.

import type { AcpConnection } from "../acp/jsonrpc";
import { byActivity, type SessionSummary } from "../acp/types";
import { type ThreadStatus, V2Method } from "../acp/v2";
import { changesFor, type OutboxChange, pendingStatus, removeChange, resolveChange } from "./machineOutbox";
import { useMachines } from "./machines";
import {
  type AppState,
  connectionForThread,
  errorText,
  machineOfThread,
  openThread,
  STORAGE,
  store,
  threadIsRemote,
  toast,
  useApp,
} from "./store";
import { splitThreadKey, threadKey } from "./threadKeys";
import { isTombstoned, keptUnlisted } from "./threadListGuards";
import { isPrimaryWindow } from "./windowSync";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

// Pending changes cover stale machine lists until the owner answers.
export const remoteStatusPending = new Map<
  string,
  { status: "review" | "done" | "idle"; confirmed: ThreadStatus; tail: Promise<void>; version: number }
>();

/** Replaces the sessions of every machine with the list it last gave (this computer's are kept). */
export function mergeMachineLists(lists: Record<string, readonly SessionSummary[]>): void {
  set((s) => {
    // A thread the app just started there has the title it was sent with until the machine titles
    // it from its own history: `session/new` carries the title, and the list follows.
    const known = new Map(s.sessions.map((session) => [session.id, session]));
    const remote = Object.entries(lists).flatMap(([host, items]) => {
      const listed = new Set(items.map((session) => session.id));
      const rows = items
        .filter((session) => !isTombstoned(threadKey(host, session.id)))
        .map((session) => {
          const id = threadKey(host, session.id);
          const title = session.title || known.get(id)?.title;
          return {
            ...session,
            id,
            host,
            parent: session.parent
              ? { ...session.parent, id: session.parent.id ? threadKey(host, session.parent.id) : undefined }
              : undefined,
            title,
            // A change made while the machine was away shows until the machine has it.
            status: remoteStatusPending.get(id)?.status ?? pendingStatus(id) ?? session.status,
          };
        });
      // A thread the app was told about, here or by another client, that this list has not got to
      // yet stays: a read that started before it is not news that it is gone.
      const kept = keptUnlisted(host, listed).map((summary) => ({
        ...summary,
        title: summary.title || known.get(summary.id)?.title,
      }));
      return [...rows, ...kept];
    });
    return {
      sessions: byActivity([...s.sessions.filter((x) => !x.host), ...remote]),
      snoozed: {
        ...Object.fromEntries(
          Object.entries(s.snoozed).filter(
            ([id]) => !remote.some((x) => x.id === id && x.snoozedUntil !== undefined),
          ),
        ),
        ...Object.fromEntries(
          remote
            .filter(
              (x) => x.snoozedUntil !== undefined && x.snoozedUntil !== null && x.snoozedUntil > Date.now(),
            )
            .map((x) => [x.id, x.snoozedUntil as number]),
        ),
      },
    };
  });
  for (const row of get().sessions) {
    if (row.host && (row.status === "running" || row.status === "needs_input")) void openThread(row.id);
  }
}

/**
 * What a toast adds when some of `ids` run on a machine that is away: that the change waits for it.
 * `lead` ends the sentence before it, when the toast's own text has no full stop.
 */
export function syncNote(ids: readonly string[], lead = ""): string {
  const names = [
    ...new Set(
      ids
        .filter((id) => threadIsRemote(id) && !connectionForThread(id))
        .map((id) => machineOfThread(id) ?? ""),
    ),
  ].filter(Boolean);
  if (!names.length) return "";
  return `${lead} Syncs when ${names.length === 1 ? `${names[0]} is` : "those computers are"} back.`;
}

/** Machines whose waiting changes are being sent now: a second list read does not send them twice. */
export const flushingOutbox = new Set<string>();

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/**
 * Send what the user changed about `host`'s threads while it was away (`state/machineOutbox.ts`),
 * in the order it was made, now that `rows` (the machine's own list, with its own ids) says what
 * the machine has. A change the machine already has, or that its newer state overrides, is
 * dropped before anything is sent, so the list drawn from `rows` next shows the machine's state;
 * one the machine refuses is dropped and said; one cut off by the machine going away again waits
 * for the next time. `state/machines.ts` `readThreads` calls it with every list it reads.
 */
export async function flushMachineOutbox(
  host: string,
  conn: AcpConnection,
  rows: readonly SessionSummary[],
): Promise<void> {
  if (!isPrimaryWindow() || flushingOutbox.has(host) || !changesFor(host).length) return;
  const byId = new Map(rows.map((row) => [row.id, row]));
  let overridden = 0;
  const toSend: OutboxChange[] = [];
  for (const change of changesFor(host)) {
    const row = byId.get(splitThreadKey(change.key).id);
    const verdict = resolveChange(change, row);
    if (verdict === "send") {
      toSend.push(change);
      continue;
    }
    removeChange(change);
    if (verdict !== "machine") continue;
    overridden++;
    restoreRow(change, row);
  }
  const name = () => get().machineNames[host] ?? "The machine";
  if (overridden) {
    toast(
      `${name()} changed ${plural(overridden, "thread")} after you did, so its newer state stands.`,
      "info",
    );
  }
  if (!toSend.length) return;
  flushingOutbox.add(host);
  let synced = 0;
  const failures: unknown[] = [];
  try {
    for (const change of toSend) {
      if (conn.isClosed) break;
      const { id: sessionId } = splitThreadKey(change.key);
      try {
        await (change.kind === "status"
          ? conn.request(V2Method.sessionStatus, { session_id: sessionId, status: change.value }, 10_000)
          : conn.request(V2Method.sessionPin, { session_id: sessionId, pinned: change.value }, 10_000));
        synced++;
        await keepSent(host, sessionId, change);
        removeChange(change);
      } catch (error) {
        // The machine went away again: the change waits for the next time it is back.
        if (conn.isClosed) break;
        // Refused (an older machine keeps no status or pin of its own): said, not retried, and
        // the row goes back to what the machine has.
        removeChange(change);
        restoreRow(change, byId.get(sessionId));
        failures.push(error);
      }
    }
  } finally {
    flushingOutbox.delete(host);
  }
  if (synced) toast(`Synced ${plural(synced, "change")} to ${name()}.`, "info");
  if (failures.length) {
    toast(
      `Could not sync ${plural(failures.length, "change")} to ${name()}: ${errorText(failures[0])}`,
      "error",
    );
  }
}

/**
 * Write a sent change into the machine's list as this computer holds it, so the row keeps it once
 * the change stops covering it, until the machine's next list says the same.
 */
async function keepSent(host: string, sessionId: string, change: OutboxChange): Promise<void> {
  useMachines.setState((state) => {
    const rows = state.sessions[host];
    if (!rows?.some((row) => row.id === sessionId)) return {};
    const patch = change.kind === "status" ? { status: change.value } : { pinned: change.value };
    return {
      sessions: {
        ...state.sessions,
        [host]: rows.map((row) => (row.id === sessionId ? { ...row, ...patch } : row)),
      },
    };
  });
}

/** Put a thread's row back the way its machine has it, once a change to it is not sent. */
function restoreRow(change: OutboxChange, row: SessionSummary | undefined): void {
  const key = change.key;
  if (change.kind === "status") {
    const status = row?.status;
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === key ? { ...x, status } : x)) }));
    return;
  }
  const on = !!row?.pinned;
  const pinned = get().pinned;
  if (pinned.includes(key) === on) return;
  const next = on ? [key, ...pinned] : pinned.filter((id) => id !== key);
  set({ pinned: next });
  store(STORAGE.pinned, JSON.stringify(next));
}
