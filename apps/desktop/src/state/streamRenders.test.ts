// What a streamed reply redraws in the transcript. Without a DOM, the bench
// counts what decides it: a memoised row draws again when it is handed a new
// prop, so each pass compares what every row would be handed with what it was
// handed the frame before. A 20-turn thread (each turn folded, with a group of
// calls and a footer) takes 500 chunks of a reply, one store update each, as
// the transcript sees them after `state/batch.ts` (one per frame at most).
//
// `before` is the pipeline the transcript ran until now: rows rebuilt per chunk
// and each footer's Trace handed a fresh slice. `after` keeps unchanged rows and
// slices (`keepUnchangedRows`, `turnItemsByEnd`).

import { describe, expect, test } from "bun:test";
import type { SessionUpdate } from "../acp/types";
import { checkpointsByMessage } from "./attempts";
import { type FoldRow, foldTurns, groupTools, keepUnchangedRows, sameShallow, turnItemsByEnd } from "./fold";
import {
  applyUpdate,
  beginTurn,
  emptyThread,
  finishTurn,
  type ThreadState,
  type TranscriptItem,
} from "./thread";

const NOTHING_OPEN: ReadonlySet<string> = new Set();
const TURNS = 20;
const CHUNKS = 500;
/** About what the virtualizer mounts pinned to the end: a screen of rows and `overscan: 8`. */
const MOUNTED = 24;

function tool(id: string, title: string, status: "in_progress" | "completed" = "completed"): SessionUpdate {
  return { kind: "toolCall", id, title, toolKind: "read", status, diffs: [] };
}

/** A thread of finished turns, each a thought, six reads, a reply and its footer, then a running turn. */
function thread(): ThreadState {
  let t = emptyThread("bench", "/project");
  let now = 1_000;
  for (let turn = 0; turn < TURNS; turn++) {
    t = beginTurn(t, `Question ${turn}`, now);
    t = applyUpdate(t, { kind: "thought", text: "Looking at the files", summary: false }, now + 1);
    for (let call = 0; call < 6; call++)
      t = applyUpdate(t, tool(`t${turn}c${call}`, `Read file ${call}`), now + 2);
    t = applyUpdate(t, { kind: "agent", text: `Answer ${turn}` }, now + 3);
    t = finishTurn(t, "completed", now + 4);
    now += 10;
  }
  t = beginTurn(t, "The question streaming now", now);
  for (let call = 0; call < 4; call++) t = applyUpdate(t, tool(`live${call}`, `Read live ${call}`), now + 1);
  return t;
}

type Counts = { folds: number; groups: number; footers: number; rail: number; reply: number };

/**
 * One pass per chunk: what the fold, group and footer rows are handed. A row
 * redraws when its prop changed identity; the footer row (`TurnEndRow` with
 * its `TurnTrace`) was not memoised, so before it redrew on every pass.
 */
function stream(keep: boolean, mountedOnly: boolean): Counts {
  let t = thread();
  let rows: FoldRow[] = [];
  let traces: Record<string, TranscriptItem[]> = {};
  let nodes: { at: number }[] = [];
  const counts: Counts = { folds: 0, groups: 0, footers: 0, rail: 0, reply: 0 };
  for (let chunk = 0; chunk <= CHUNKS; chunk++) {
    if (chunk > 0) t = applyUpdate(t, { kind: "agent", text: `word ${chunk} ` }, 5_000 + chunk);
    const built = groupTools(foldTurns(t.items, t.running, NOTHING_OPEN), NOTHING_OPEN);
    const next = keep ? keepUnchangedRows(rows, built) : built;
    const nextTraces = keep ? turnItemsByEnd(t.items, traces) : sliceTurns(t.items);
    // The timeline rail (memoised) is handed the checkpoints of the turns' messages: none here.
    const byMessage = checkpointsByMessage(t.items, {});
    const builtNodes = next.flatMap((r, at) => ("item" in r && byMessage[r.item.id] ? [{ at }] : []));
    const nextNodes = keep && sameShallow(nodes, builtNodes, 2) ? nodes : builtNodes;
    if (chunk > 0) {
      if (nextNodes !== nodes) counts.rail++;
      const before = new Map(rows.map((r) => [r.key, r]));
      const shown = mountedOnly ? next.slice(-MOUNTED) : next;
      for (const row of shown) {
        const old = before.get(row.key);
        if ("fold" in row && (!old || !("fold" in old) || old.fold !== row.fold)) counts.folds++;
        if ("group" in row && (!old || !("group" in old) || old.group !== row.group)) counts.groups++;
        if ("item" in row && row.item.kind === "turnEnd") {
          const same =
            old && "item" in old && old.item === row.item && traces[row.item.id] === nextTraces[row.item.id];
          // Memoised with stable props, it skips; unmemoised (before), it always drew.
          if (!keep || !same) counts.footers++;
        }
        if (
          "item" in row &&
          row.item.kind === "assistant" &&
          (!old || !("item" in old) || old.item !== row.item)
        )
          counts.reply++;
      }
    }
    rows = next;
    traces = nextTraces;
    nodes = nextNodes;
  }
  return counts;
}

/** The slices the transcript made before: one new array per finished turn per pass. */
function sliceTurns(items: readonly TranscriptItem[]): Record<string, TranscriptItem[]> {
  const map: Record<string, TranscriptItem[]> = {};
  let from = 0;
  items.forEach((item, i) => {
    if (item.kind !== "turnEnd") return;
    map[item.id] = items.slice(from, i + 1);
    from = i + 1;
  });
  return map;
}

describe("a streamed reply redraws only itself", () => {
  test(`${CHUNKS} chunks into a ${TURNS}-turn thread`, () => {
    const before = stream(false, false);
    const after = stream(true, false);
    const mountedBefore = stream(false, true);
    const mountedAfter = stream(true, true);
    console.log(
      `stream bench, ${CHUNKS} chunks, ${TURNS} turns (all rows / mounted rows):\n` +
        `  fold rows redrawn    ${before.folds} / ${mountedBefore.folds} -> ${after.folds} / ${mountedAfter.folds}\n` +
        `  group rows redrawn   ${before.groups} / ${mountedBefore.groups} -> ${after.groups} / ${mountedAfter.groups}\n` +
        `  footer rows redrawn  ${before.footers} / ${mountedBefore.footers} -> ${after.footers} / ${mountedAfter.footers}\n` +
        `  timeline rail        ${before.rail} -> ${after.rail}\n` +
        `  streaming reply      ${before.reply} -> ${after.reply}`,
    );
    // Every fold, group and footer drew once per chunk before.
    expect(before.folds).toBe(TURNS * CHUNKS);
    expect(before.footers).toBe(TURNS * CHUNKS);
    expect(before.groups).toBe(CHUNKS);
    expect(before.rail).toBe(CHUNKS);
    // Now nothing but the reply does, and the reply still draws every chunk.
    expect(after).toEqual({ folds: 0, groups: 0, footers: 0, rail: 0, reply: CHUNKS });
    expect(mountedAfter.reply).toBe(CHUNKS);
  });
});

describe("keeping unchanged rows", () => {
  test("a row whose item changed is the new one; the rest are kept", () => {
    const a: TranscriptItem = { kind: "assistant", id: "a", text: "one" };
    const previous = [
      { key: "f", fold: { key: "f", open: false, steps: 2, tools: 2, failed: 0, files: 0, answered: true } },
      { key: "a", item: a },
    ];
    const next = [
      { key: "f", fold: { ...previous[0]!.fold! } },
      { key: "a", item: { ...a, text: "one two" } },
    ];
    const kept = keepUnchangedRows<{ key: string; fold?: unknown; item?: TranscriptItem }>(previous, next);
    expect(kept[0]).toBe(previous[0]);
    expect(kept[1]).toBe(next[1]);
  });

  test("nothing changed: the previous array itself", () => {
    const previous = [
      { key: "x", group: { key: "x", open: false, count: 3, failed: 0, label: "Read 3 files" } },
    ];
    const next = [{ key: "x", group: { ...previous[0]!.group } }];
    expect(keepUnchangedRows(previous, next)).toBe(previous);
  });

  test("a fold that opened is a new row", () => {
    const fold = { key: "f", open: false, steps: 1, tools: 1, failed: 0, files: 0, answered: true };
    const previous = [{ key: "f", fold }];
    const kept = keepUnchangedRows(previous, [{ key: "f", fold: { ...fold, open: true } }]);
    expect(kept[0]!.fold.open).toBe(true);
  });

  test("a turn's items keep their array until one of them changes", () => {
    const t = thread();
    const first = turnItemsByEnd(t.items);
    const again = turnItemsByEnd(t.items, first);
    for (const id of Object.keys(first)) expect(again[id]).toBe(first[id]!);
    const end = t.items.findIndex((i) => i.kind === "turnEnd");
    const changed = t.items.map((item, i) => (i === end - 1 ? { ...item } : item));
    const after = turnItemsByEnd(changed, first);
    const firstEnd = t.items[end]!.id;
    expect(after[firstEnd]).not.toBe(first[firstEnd]!);
    expect(after[firstEnd]).toEqual(first[firstEnd]!);
  });
});
