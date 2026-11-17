import { isLoaded } from "./thread";
// Lost turns: a turn on another machine whose prompt request died with that machine's connection,
// followed until the machine says how it ended or stays silent too long (issue 3440). Split out of
// state/store.ts; the state these write still lives in the app store.

import type { AcpConnection, JsonObject } from "../acp/jsonrpc";
import { Method, parseSessionList } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import { linksFinished } from "./host";
import { latestReply, MACHINE_LOST_TURN } from "./orchestration";
import { connectionFor, connectionForThread, openThread, updateThread, useApp } from "./store";
import type { ThreadState, TranscriptItem } from "./thread";
import { splitThreadKey } from "./threadKeys";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();

/**
 * How long the app waits for a machine's channel to come back before it gives a lost turn up:
 * past it the machine is unreachable, and the links it was answering are told so. Every read the
 * machine answers pushes it out, so a turn that is still running there is followed for as long as
 * it runs.
 */
export const LOST_MACHINE_MS = 2 * 60_000;

/** How often a lost turn the machine has not spoken about is looked at again (`LOST_MACHINE_MS`). */
export const LOST_RECHECK_MS = 30_000;

/** A turn on another machine whose prompt request died with that machine's connection. */
interface LostTurn {
  /** This window's key for the thread, `‹host›/‹session›`. */
  thread: string;
  host: string;
  /** The links the turn was answering: nothing else settles them once it is handed here. */
  links: string[];
  /** When the machine has been silent long enough to give up (`LOST_MACHINE_MS`). */
  deadline: number;
  /** The next look at the machine, or the give-up, whichever comes first. */
  timer?: ReturnType<typeof setTimeout>;
  /** A read of the machine is in flight: a second one would settle the same links twice. */
  busy?: boolean;
  /** The thread has been read again since the machine said the turn still runs there. */
  following?: boolean;
}

/** The lost turns this window is following, by thread key. */
const lostTurns = new Map<string, LostTurn>();

/** Tests only: `LOST_MACHINE_MS`, shortened so a test need not wait two minutes. */
let lostMachineMs = LOST_MACHINE_MS;
export function setLostMachineMsForTests(ms: number): void {
  lostMachineMs = ms;
}

/** Stops following every lost turn, for the store's reset between tests. */
export function resetLostTurns(): void {
  for (const record of lostTurns.values()) clearTimeout(record.timer);
  lostTurns.clear();
  lostMachineMs = LOST_MACHINE_MS;
}

/**
 * A turn on another machine whose prompt request died with the machine's connection: the turn goes
 * on there. Nothing is settled from the rows that had arrived — a half-written answer is not an
 * answer — so the links keep working until the machine can be asked how the thread ended
 * (`settleLostTurn`), or for `LOST_MACHINE_MS` if its channel never comes back (issue 3440).
 */
export function lostTurn(thread: string, links: readonly (string | undefined)[]): void {
  const host = splitThreadKey(thread).host;
  const ids = links.filter((link): link is string => typeof link === "string");
  if (!host || !ids.length) return;
  // A second prompt can be lost on the same thread: it is followed by the record already there, so
  // the links of both are answered by the turn the machine ends with, never one of them orphaned.
  const before = lostTurns.get(thread);
  if (before) {
    before.links = [...new Set([...before.links, ...ids])];
    before.deadline = Date.now() + lostMachineMs;
    void lookAtLostTurn(before);
    return;
  }
  const record: LostTurn = { thread, host, links: ids, deadline: Date.now() + lostMachineMs };
  lostTurns.set(thread, record);
  void lookAtLostTurn(record);
}

/**
 * One look at a lost turn's machine, then the next look before the deadline. A machine that
 * answers resets the deadline (`askMachine`), so this repeats only while the turn runs there or
 * while the machine is away; `LOST_RECHECK_MS` bounds how fast, `LOST_MACHINE_MS` how long.
 */
async function lookAtLostTurn(record: LostTurn): Promise<void> {
  if (lostTurns.get(record.thread) !== record) return;
  clearTimeout(record.timer);
  if (Date.now() >= record.deadline) return giveUpLostTurn(record);
  await askMachine(record);
  if (lostTurns.get(record.thread) !== record) return;
  const next = Math.min(record.deadline, Date.now() + LOST_RECHECK_MS);
  record.timer = setTimeout(() => void lookAtLostTurn(record), Math.max(0, next - Date.now()));
}

/** A machine's channel was opened again: ask it about the turns this window lost with it. */
export function resumeLostTurns(host: string): void {
  for (const record of lostTurns.values()) if (record.host === host) void askMachine(record);
}

/**
 * A machine reported what one of its threads does (`session/update` `session_info_update`, read by
 * `onSessionUpdate`): a lost turn whose thread stopped running there is read and settled now.
 */
export function lostTurnStatus(thread: string, status: ThreadStatus): void {
  const record = lostTurns.get(thread);
  if (record && status !== "running" && status !== "needs_input") void askMachine(record);
}

/**
 * Ask the machine how the lost turn's thread ended, if it can be asked now. A turn still running
 * there keeps the links working and waits for the machine's next word about that thread; one that
 * ended is settled from what the thread holds (`settleLostTurn`).
 */
async function askMachine(record: LostTurn): Promise<void> {
  if (record.busy || lostTurns.get(record.thread) !== record) return;
  const conn = connectionFor(record.host);
  // No channel yet: `resumeLostTurns` asks again when the machine is back, and the deadline gives up.
  if (!conn) return;
  record.busy = true;
  try {
    const status = await machineStatusOf(conn, record.thread);
    // A thread the machine no longer lists is not one it can be asked about: the deadline says so.
    if (!status || lostTurns.get(record.thread) !== record) return;
    // It answered about this thread, so it is reachable, and it has longer to report.
    record.deadline = Date.now() + lostMachineMs;
    if (status === "running") {
      // Still working there. The delegate's own chat is read again once (the machine's rows, not
      // the failure the drop wrote), and the answer arrives when the machine says it stopped.
      if (!record.following) {
        record.following = true;
        void reloadThread(record.thread);
      }
      return;
    }
    await settleLostTurn(record);
  } catch {
    // The channel went again: its return asks once more, and the deadline bounds the wait.
  } finally {
    record.busy = false;
  }
}

/** What a machine says one of its threads does, from its own `session/list` pages. */
async function machineStatusOf(conn: AcpConnection, thread: string): Promise<ThreadStatus | undefined> {
  const { id } = splitThreadKey(thread);
  let cursor: string | undefined;
  for (let page = 0; page < 10; page++) {
    const params: JsonObject = cursor ? { cursor } : {};
    const result = parseSessionList(await conn.request(Method.sessionList, params, 10_000));
    const found = result.sessions.find((x) => x.id === id);
    if (found) return found.status;
    cursor = result.nextCursor;
    if (!cursor) break;
  }
  return undefined;
}

/**
 * The lost turn's thread no longer runs: read it again from the machine (the replay a reopened
 * thread gets) and settle its links by what the turn left — its answer, or the failure it ended in.
 */
async function settleLostTurn(record: LostTurn): Promise<void> {
  if (lostTurns.get(record.thread) !== record) return;
  // The thread as the machine has it now, not as this window last saw it. A replay it stops
  // answering is no end of the turn: the recheck and the deadline still hold the links.
  if (!(await reloadThread(record.thread))) return;
  lostTurns.delete(record.thread);
  clearTimeout(record.timer);
  const rows = lastTurnRows(get().threads[record.thread]);
  const answer = latestReply(rows);
  // The turn's own answer, never the one before it: a turn that wrote none settled on nothing.
  if (answer.trim()) linksFinished(record.links, record.thread, "completed", undefined, rows);
  else linksFinished(record.links, record.thread, "failed", turnErrorIn(rows) ?? MACHINE_LOST_TURN, rows);
}

/**
 * A thread's rows for the turn that ended last, as its replay holds them: after its last prompt,
 * so a turn that answered nothing does not take the answer of the turn before it.
 */
function lastTurnRows(thread: ThreadState | undefined): TranscriptItem[] {
  const items = thread?.items ?? [];
  const from = items.findLastIndex((item) => item.kind === "user");
  return from < 0 ? items : items.slice(from);
}

/**
 * What a thread's rows say about how its last turn ended: its last notice (the engine's stop,
 * limit or failure marker, which a replay carries), an error-toned one first. A turn the machine
 * ended in one is that failure, not the connection's.
 */
function turnErrorIn(items: readonly TranscriptItem[]): string | undefined {
  let notice: string | undefined;
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]!;
    if (item.kind !== "notice") continue;
    if (item.tone === "error") return item.text;
    notice ??= item.text;
  }
  return notice;
}

/** The machine never came back inside the bound: its links are told so, with the sentence for it. */
function giveUpLostTurn(record: LostTurn): void {
  if (!lostTurns.delete(record.thread)) return;
  clearTimeout(record.timer);
  const rows = lastTurnRows(get().threads[record.thread]);
  linksFinished(record.links, record.thread, "failed", MACHINE_LOST_TURN, rows);
}

/**
 * A thread read again from its machine, as opening it after a restart does: its replay, not ours.
 * Answers whether that replay arrived; one the machine did not give leaves the thread as it was.
 */
export async function reloadThread(thread: string): Promise<boolean> {
  if (!connectionForThread(thread)) return false;
  updateThread(thread, (t) => ({ ...t, load: { phase: "idle" } }));
  await openThread(thread);
  const after = get().threads[thread];
  return isLoaded(after);
}
