import { describe, expect, test } from "bun:test";
import type { TranscriptItem } from "../state/thread";
import { turnEntries } from "./overview";
import { parseStoredTurns, storedTurnsSql, withStoredTurns } from "./storedTurns";

const at = (time: string) => Date.parse(`2026-11-25T10:${time}Z`);

const replayed: TranscriptItem[] = [
  { kind: "user", id: "u1", text: "first", at: at("00:00") },
  { kind: "assistant", id: "a1", text: "one" },
  { kind: "user", id: "u2", text: "second", at: at("05:00") },
  { kind: "assistant", id: "a2", text: "two" },
  { kind: "user", id: "u3", text: "third", at: at("09:00") },
  { kind: "assistant", id: "a3", text: "three" },
];

describe("stored turns", () => {
  test("a replayed thread's turns take their time and cost from the store", () => {
    const stored = parseStoredTurns({
      rows: [
        { started_at: "2026-11-25T10:05:00.400Z", duration_ms: 15_000, cost_usd: 0.13 },
        { started_at: "2026-11-25T10:00:00.200Z", duration_ms: 9_000, cost_usd: 0.12 },
      ],
    });
    const turns = withStoredTurns(turnEntries(replayed), stored);
    expect(turns.map((turn) => [turn.prompt, turn.elapsedMs, turn.cost])).toEqual([
      ["first", 9_000, 0.12],
      ["second", 15_000, 0.13],
      ["third", undefined, undefined],
    ]);
  });

  test("what the app measured itself is kept, and a steered turn's rows add up", () => {
    const items: TranscriptItem[] = [
      ...replayed.slice(0, 2),
      { kind: "turnEnd", id: "t1", outcome: "completed", elapsedMs: 4_000, at: at("00:04") },
    ];
    const stored = parseStoredTurns({
      rows: [
        { started_at: "2026-11-25T10:00:00.100Z", duration_ms: 3_000, cost_usd: 0.05 },
        { started_at: "2026-11-25T10:00:02.000Z", duration_ms: 2_000, cost_usd: 0.02 },
      ],
    });
    const [turn] = withStoredTurns(turnEntries(items), stored);
    expect(turn?.elapsedMs).toBe(4_000);
    expect(turn?.cost).toBeCloseTo(0.07);
  });

  test("rows without a start or with no turn to own them are left out", () => {
    expect(parseStoredTurns({ rows: [{ started_at: null, cost_usd: 1 }] })).toEqual([]);
    expect(parseStoredTurns(null)).toEqual([]);
    const early = parseStoredTurns({ rows: [{ started_at: "2026-11-25T09:00:00Z", cost_usd: 1 }] });
    expect(withStoredTurns(turnEntries(replayed), early).every((turn) => turn.cost === undefined)).toBe(true);
  });

  test("the query quotes the thread id", () => {
    expect(storedTurnsSql("a'b")).toContain("thread_id = 'a''b'");
  });
});
