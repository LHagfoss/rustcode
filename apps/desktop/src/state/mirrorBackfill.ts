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

import { type AcpConnection, type JsonObject, num, obj, RpcError, str } from "../acp/jsonrpc";
import { parseSessionUpdate, type SessionSummary } from "../acp/types";
import { errorDetail, recordAutoSync } from "./autoSyncStatus";
import type { MirroredEntry } from "./machineMirror";
import { applyUpdate, emptyThread, finishLoad, type ThreadState } from "./thread";
import { LOCAL_HOST, threadKey } from "./threadKeys";

/** The engine method that answers a thread's replay without opening it. */
export const REPLAY_METHOD = "_rustcode/session/replay";
/** How long one replay may take; a long thread's history is read and sent in one answer. */
const REPLAY_MS = 60_000;
/** Hard upper bound on replay pages per thread so a misbehaving peer cannot loop forever. */
export const MAX_REPLAY_PAGES = 200;
/** Drops while replaying the same thread revision before skipping it for this app run. */
export const MAX_DROP_RETRIES = 2;

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
  options: {
    held: (id: string) => boolean;
    excludedProjects: readonly string[];
    skipped?: (row: SessionSummary) => boolean;
    limit?: number;
  },
): SessionSummary[] {
  const revisions = new Map(mirrored.map((entry) => [entry.session.id, entry.transcriptRevision]));
  return rows
    .filter((row) => {
      if (row.status === "running" || row.status === "needs_input") return false;
      if (options.held(row.id)) return false;
      if (options.skipped?.(row)) return false;
      if (options.excludedProjects.some((project) => inProject(row.cwd, project))) return false;
      const revision = revisions.get(row.id);
      return revision === undefined || (row.updatedAt ?? 0) > revision;
    })
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, options.limit ?? MAX_PER_PASS);
}

/**
 * Fetch a full thread replay across `nextCursor` pages so large transcripts never exceed the
 * remote gateway's frame size cap.
 */
export async function fetchReplay(
  connection: Pick<AcpConnection, "request">,
  sessionId: string,
  maxPages = MAX_REPLAY_PAGES,
): Promise<JsonObject | null> {
  const updates: unknown[] = [];
  let meta: { sessionId?: string; cwd?: string; title?: string } | null = null;
  let cursor: number | undefined;
  for (let page = 0; page < maxPages; page++) {
    const params: JsonObject = cursor === undefined ? { sessionId } : { sessionId, cursor };
    const body = obj((await connection.request(REPLAY_METHOD, params, REPLAY_MS)) as JsonObject | undefined);
    if (!body || !Array.isArray(body.updates)) return null;
    if (!meta) {
      meta = {
        sessionId: str(body.sessionId),
        cwd: str(body.cwd),
        title: str(body.title),
      };
    }
    updates.push(...body.updates);
    const next = num(body.nextCursor);
    if (next === undefined || !Number.isInteger(next) || next <= (cursor ?? 0)) break;
    cursor = next;
  }
  if (!meta) return null;
  return {
    ...(meta.sessionId !== undefined ? { sessionId: meta.sessionId } : {}),
    ...(meta.cwd !== undefined ? { cwd: meta.cwd } : {}),
    ...(meta.title !== undefined ? { title: meta.title } : {}),
    updates: updates as JsonObject[],
  };
}

/**
 * Tracks threads whose replay dropped the connection (for example, an older server sending a
 * single oversized frame) so a machine does not enter an endless reconnect-and-drop loop.
 */
export class ReplayDropGuard {
  private readonly drops = new Map<string, { revision: number; count: number }>();

  recordDrop(host: string, row: Pick<SessionSummary, "id" | "updatedAt">): void {
    const key = threadKey(host, row.id);
    const revision = row.updatedAt ?? 0;
    const prev = this.drops.get(key);
    const count = prev && prev.revision === revision ? prev.count + 1 : 1;
    this.drops.set(key, { revision, count });
  }

  clear(host: string, sessionId: string): void {
    this.drops.delete(threadKey(host, sessionId));
  }

  isSkipped(host: string, row: Pick<SessionSummary, "id" | "updatedAt">): boolean {
    const entry = this.drops.get(threadKey(host, row.id));
    return !!entry && entry.revision === (row.updatedAt ?? 0) && entry.count >= MAX_DROP_RETRIES;
  }
}

const dropGuard = new ReplayDropGuard();

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
    if (!status?.enabled) return;
    if (!machines.machineConnection(host)) {
      recordAutoSync(host, "backfill", "skipped", "It went away before the copy started.");
      return;
    }
    const rows = machines.useMachines.getState().sessions[host] ?? [];
    const held = (id: string) => !!store.useApp.getState().threads[threadKey(host, id)];
    const candidates = backfillCandidates(rows, await mirror.loadMirroredEntries(host), {
      held,
      excludedProjects: status.excludedProjects,
      skipped: (row) => dropGuard.isSkipped(host, row),
    });
    let failures = 0;
    let copied = 0;
    for (const row of candidates) {
      const connection = machines.machineConnection(host);
      if (!connection) {
        recordAutoSync(
          host,
          "backfill",
          "skipped",
          `It went away after ${copied} of ${candidates.length} copies.`,
        );
        return;
      }
      // Opened since the list was made: the app keeps that copy itself.
      if (held(row.id)) continue;
      let answer: unknown;
      try {
        answer = await fetchReplay(connection, row.id);
      } catch (error) {
        if (connection.isClosed) {
          dropGuard.recordDrop(host, row);
          recordAutoSync(host, "backfill", "failed", "The connection dropped while a thread was copied.");
          return;
        }
        // A machine on an older release cannot answer (or its relay refuses the method); one
        // thread that fails is tried on the next pass.
        if (error instanceof RpcError && error.isMethodNotFound) {
          recordAutoSync(host, "backfill", "skipped", "Its server is too old to copy threads.");
          return;
        }
        if (++failures >= MAX_FAILURES) {
          recordAutoSync(
            host,
            "backfill",
            "failed",
            `${failures} copies in a row failed (${errorDetail(error)}).`,
          );
          return;
        }
        continue;
      }
      dropGuard.clear(host, row.id);
      failures = 0;
      const key = threadKey(host, row.id);
      const thread = threadFromReplay(key, answer, row);
      if (thread?.items.length && !held(row.id)) {
        await mirror.mirrorTranscript(thread, key);
        copied++;
      }
      const again = await mirror.readMirrorStatus();
      if (!again?.enabled) return;
      await wait(GAP_MS);
    }
    recordAutoSync(
      host,
      "backfill",
      "synced",
      copied ? `${copied} ${copied === 1 ? "thread" : "threads"} copied.` : "Every copy is up to date.",
    );
  } catch (error) {
    // Best effort: what was copied stays, and the next connection tries the rest.
    recordAutoSync(host, "backfill", "failed", errorDetail(error));
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
