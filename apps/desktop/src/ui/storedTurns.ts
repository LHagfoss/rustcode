import { useEffect, useState } from "react";
import { Method } from "../acp/types";
import { splitThreadKey } from "../state/threadKeys";
import { connectionFor } from "../state/threadMachines";
import type { TurnEntry } from "./overview";

/** One turn the engine recorded in its data store: when it began, how long it ran, what it cost. */
export interface StoredTurn {
  startedAt: number;
  durationMs?: number;
  cost?: number;
}

/**
 * How far before its message a stored turn may begin and still be that message's: the engine
 * stamps the turn and the message a moment apart.
 */
const START_SLACK_MS = 5_000;

const STORED_TURN_ROWS = 1_000;

/** The thread's recorded turns, oldest first. */
export function storedTurnsSql(threadId: string): string {
  const id = threadId.replaceAll("'", "''");
  return `SELECT started_at, duration_ms, cost_usd FROM turns WHERE thread_id = '${id}' ORDER BY started_at`;
}

const finite = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;

/** The rows of a `_rustcode/data/query` answer for {@link storedTurnsSql}. */
export function parseStoredTurns(answer: unknown): StoredTurn[] {
  const rows = (answer as { rows?: unknown } | null)?.rows;
  if (!Array.isArray(rows)) return [];
  const turns: StoredTurn[] = [];
  for (const row of rows) {
    const record = row as Record<string, unknown> | null;
    const startedAt = typeof record?.started_at === "string" ? Date.parse(record.started_at) : Number.NaN;
    if (!Number.isFinite(startedAt)) continue;
    turns.push({ startedAt, durationMs: finite(record?.duration_ms), cost: finite(record?.cost_usd) });
  }
  return turns.sort((a, b) => a.startedAt - b.startedAt);
}

/**
 * Fills in the time and cost of turns the app did not watch run (a replay after a restart, or an
 * old thread opened again) from the turns the engine stored. A stored turn belongs to the last
 * turn whose message was sent before it began; several (a steer, a retry) add up. What the app
 * measured itself is kept.
 */
export function withStoredTurns(entries: readonly TurnEntry[], stored: readonly StoredTurn[]): TurnEntry[] {
  if (stored.length === 0) return [...entries];
  const found = new Map<number, { durationMs?: number; cost?: number }>();
  for (const turn of stored) {
    let owner = -1;
    for (let i = 0; i < entries.length; i++) {
      const at = entries[i]!.promptAt;
      if (at !== undefined && at <= turn.startedAt + START_SLACK_MS) owner = i;
    }
    if (owner < 0) continue;
    const sum = found.get(owner) ?? {};
    if (turn.durationMs !== undefined) sum.durationMs = (sum.durationMs ?? 0) + turn.durationMs;
    if (turn.cost !== undefined) sum.cost = (sum.cost ?? 0) + turn.cost;
    found.set(owner, sum);
  }
  return entries.map((entry, i) => {
    const sum = found.get(i);
    if (!sum) return entry;
    return {
      ...entry,
      elapsedMs: entry.elapsedMs ?? sum.durationMs,
      cost: entry.cost ?? sum.cost,
    };
  });
}

/**
 * The turns a thread's engine stored, read again whenever another turn settles. An engine that
 * cannot answer leaves the list empty: the rows then show what the transcript knows.
 */
export function useStoredTurns(threadKey: string | undefined, settled: number): readonly StoredTurn[] {
  const [stored, setStored] = useState<readonly StoredTurn[]>([]);
  useEffect(() => {
    if (!threadKey || settled === 0) return;
    const { host, id } = splitThreadKey(threadKey);
    const conn = connectionFor(host);
    if (!conn) return;
    let live = true;
    conn
      .request(Method.dataQuery, { sql: storedTurnsSql(id), limit: STORED_TURN_ROWS }, 10_000)
      .then((answer) => {
        if (live) setStored(parseStoredTurns(answer));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [threadKey, settled]);
  return stored;
}
