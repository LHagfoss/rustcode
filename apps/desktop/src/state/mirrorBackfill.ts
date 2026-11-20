// Complete thread copies (docs/data-store.md › Thread mirror). The mirror keeps the transcript of
// every thread this computer reads from a machine; without this, only the threads someone opened
// here had one, and the rest of a machine's threads were a title with nothing behind it once the
// machine went away.
//
// While a machine is connected and Thread copies is on, this reads, one at a time, each of its
// threads whose copy is missing or older than the thread, and writes it to the mirror. The read is
// `_rustcode/session/replay`: the transcript a `session/load` would replay, answered from the
// machine's history file without opening the thread, so nothing starts there and a thread someone
// has open there is not touched. Running threads, threads the app already holds and projects left
// out are skipped. A machine whose server predates the method is skipped. A pass ends when the
// machine goes away or the switch is turned off, and the next connection picks up where it ended.

import { type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import { parseSessionUpdate, type SessionSummary } from "../acp/types";
import type { MirroredEntry } from "./machineMirror";
import { applyUpdate, emptyThread, finishLoad, type ThreadState } from "./thread";
import { LOCAL_HOST, threadKey } from "./threadKeys";

/** The engine method that answers a thread's replay without opening it. */
export const REPLAY_METHOD = "_rustcode/session/replay";
/** How long one replay may take; a long thread's history is read and sent in one answer. */
const REPLAY_MS = 60_000;

/** How long after a machine connects the first copy is read, so the user's own reads go first. */
export const START_DELAY_MS = 15_000;
/** The pause between two copies, so a pass never crowds the connection. */
export const GAP_MS = 1_000;
/** At most this many copies per pass; the next connection continues. */
export const MAX_PER_PASS = 200;

/** True when `cwd` is `project` or inside it, by path components and case-insensitively. */
export function inProject(cwd: string, project: string): boolean {
  const parts = (path: string) =>
    path
      .toLowerCase()
      .split(/[\\/]+/)
      .filter(Boolean);
  const inner = parts(cwd);
  const outer = parts(project);
  return outer.length > 0 && outer.length <= inner.length && outer.every((part, i) => part === inner[i]);
}

/**
 * The threads of a machine whose copy needs reading, newest first: those with no transcript in the
 * mirror, or one read before the thread last changed. A running thread, one the app already holds
 * (its copy is kept as it changes) and one in a project left out are not.
 */
export function backfillCandidates(
  rows: readonly SessionSummary[],
  mirrored: readonly MirroredEntry[],
  options: { held: (id: string) => boolean; excludedProjects: readonly string[]; limit?: number },
): SessionSummary[] {
  const revisions = new Map(mirrored.map((entry) => [entry.session.id, entry.transcriptRevision]));
  return rows
    .filter((row) => {
      if (row.status === "running" || row.status === "needs_input") return false;
      if (options.held(row.id)) return false;
      if (options.excludedProjects.some((project) => inProject(row.cwd, project))) return false;
      const revision = revisions.get(row.id);
      return revision === undefined || (row.updatedAt ?? 0) > revision;
    })
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, options.limit ?? MAX_PER_PASS);
}

/**
 * The thread a replay answer draws, as opening it would: each update folded in, then the load
 * finished. `key` is the thread's key on this computer. Null when the answer is not a replay.
 */
export function threadFromReplay(key: string, answer: unknown, fallback: SessionSummary): ThreadState | null {
  const body = obj(answer as JsonObject | undefined);
  if (!body || !Array.isArray(body.updates)) return null;
  const sessionId = str(body.sessionId) || fallback.id;
  let thread: ThreadState = {
    ...emptyThread(key, str(body.cwd) || fallback.cwd, str(body.title) || fallback.title),
    load: { phase: "loading" },
  };
  for (const update of body.updates) {
    thread = applyUpdate(thread, parseSessionUpdate({ sessionId, update } as JsonObject));
  }
  return finishLoad(thread);
}

const passes = new Set<string>();
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** After this many failed reads in a row a pass stops: the machine cannot answer them now. */
const MAX_FAILURES = 3;

/** One pass over a machine's threads. Never throws; a copy that fails is tried on the next pass. */
async function backfill(host: string): Promise<void> {
  if (passes.has(host)) return;
  passes.add(host);
  try {
    const [machines, mirror, store] = await Promise.all([
      import("./machines"),
      import("./machineMirror"),
      import("./store"),
    ]);
    await wait(START_DELAY_MS);
    const status = await mirror.readMirrorStatus();
    if (!status?.enabled || !machines.machineConnection(host)) return;
    const rows = machines.useMachines.getState().sessions[host] ?? [];
    const held = (id: string) => !!store.useApp.getState().threads[threadKey(host, id)];
    const candidates = backfillCandidates(rows, await mirror.loadMirroredEntries(host), {
      held,
      excludedProjects: status.excludedProjects,
    });
    let failures = 0;
    for (const row of candidates) {
      const connection = machines.machineConnection(host);
      if (!connection) return;
      // Opened since the list was made: the app keeps that copy itself.
      if (held(row.id)) continue;
      let answer: unknown;
      try {
        answer = await connection.request(REPLAY_METHOD, { sessionId: row.id }, REPLAY_MS);
      } catch (error) {
        // A machine on an older release cannot answer (or its relay refuses the method); one
        // thread that fails is tried on the next pass.
        if ((error instanceof RpcError && error.isMethodNotFound) || ++failures >= MAX_FAILURES) return;
        continue;
      }
      failures = 0;
      const key = threadKey(host, row.id);
      const thread = threadFromReplay(key, answer, row);
      if (thread?.items.length && !held(row.id)) await mirror.mirrorTranscript(thread, key);
      const again = await mirror.readMirrorStatus();
      if (!again?.enabled) return;
      await wait(GAP_MS);
    }
  } catch {
    // Best effort: what was copied stays, and the next connection tries the rest.
  } finally {
    passes.delete(host);
  }
}

let started = false;

/** Read the missing copies of a machine's threads each time its connection opens. */
export function startMirrorBackfill(): void {
  if (started) return;
  started = true;
  void import("./machines").then((machines) => {
    machines.onMachineLinks((id, connection) => {
      if (connection && id !== LOCAL_HOST) void backfill(id);
    });
    // Machines already connected when the app got here.
    for (const link of machines.machineLinks()) if (link.connection) void backfill(link.machine.id);
  });
}
