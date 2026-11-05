// The sidebar's rows as data: the order they are drawn in, what a thread's calls changed, and
// the rules that move and pick them. Pure, so the sidebar and its tests share one answer
// (ui/Sidebar.tsx).

import type { SessionSummary } from "../acp/types";
import { diffCounts, type TranscriptItem } from "./thread";

/** A row the list draws. A delegated thread has no row of its own: it is in its parent's stack. */
export interface OrderRow {
  id: string;
}

/** A section as the list draws it: folded shut, or grouped, or only its first rows. */
export interface OrderSection {
  id: string;
  /** Shut: none of its rows are drawn. */
  shut?: boolean;
  /** Grouped by project when there are several: each group's own fold, in draw order. */
  groups?: { key: string; shut?: boolean; rows: OrderRow[] }[];
  /** Rows when the section is not grouped. */
  rows?: OrderRow[];
  /** Only the first rows are drawn (the Settled shelf's tail). */
  limit?: number;
}

/** Every row the list draws, in the order it draws them: a shut section or group draws none. */
export function visibleRows(sections: readonly OrderSection[]): string[] {
  const out: string[] = [];
  for (const section of sections) {
    if (section.shut) continue;
    if (section.groups) {
      for (const group of section.groups) if (!group.shut) out.push(...group.rows.map((r) => r.id));
      continue;
    }
    const rows = section.rows ?? [];
    out.push(...(section.limit === undefined ? rows : rows.slice(0, section.limit)).map((r) => r.id));
  }
  return out;
}

/**
 * The row that stands for the open thread. A delegated thread has no row, so the
 * open one answers with its nearest ancestor that is drawn: the parent row stays
 * lit as the context, and the keyboard lands on something visible.
 */
export function contextRow(
  selectedId: string | null,
  rows: readonly string[],
  parentOf: ReadonlyMap<string, string>,
): string | null {
  if (!selectedId) return null;
  const drawn = new Set(rows);
  const seen = new Set<string>();
  let at: string | undefined = selectedId;
  while (at && !seen.has(at)) {
    if (drawn.has(at)) return at;
    seen.add(at);
    at = parentOf.get(at);
  }
  return null;
}

/**
 * The row ↑ or ↓ goes to: clamped, so the ends of the list stop rather than jump across it.
 * A highlight that is no longer drawn starts again from the top (↑ from the bottom).
 */
export function nextRow(rows: readonly string[], current: string | null, delta: -1 | 1): string | null {
  if (!rows.length) return null;
  const at = current === null ? -1 : rows.indexOf(current);
  if (at < 0) return delta > 0 ? rows[0]! : rows[rows.length - 1]!;
  return rows[Math.min(rows.length - 1, Math.max(0, at + delta))]!;
}

/** Every row between two, both ends included, whichever way round the reader clicked. */
export function rangeRows(rows: readonly string[], from: string | null, to: string): string[] {
  const a = from === null ? -1 : rows.indexOf(from);
  const b = rows.indexOf(to);
  if (b < 0) return [];
  if (a < 0) return [to];
  return rows.slice(Math.min(a, b), Math.max(a, b) + 1);
}

/** The picked threads still in the list: the only ones the selection bar acts on. */
export function listedPicks(
  sessions: readonly SessionSummary[],
  picked: ReadonlySet<string>,
): SessionSummary[] {
  return sessions.filter((s) => picked.has(s.id));
}

/** The selection without the threads the list no longer draws; the same set when none left. */
export function keepListed(selected: ReadonlySet<string>, rows: readonly string[]): ReadonlySet<string> {
  const listed = new Set(rows);
  const next = new Set<string>();
  for (const id of selected) if (listed.has(id)) next.add(id);
  return next.size === selected.size ? selected : next;
}

export type RowKeyAction = { kind: "move"; delta: -1 | 1 } | { kind: "open" } | { kind: "clear" };

/**
 * What a key does to the list, or null. A field inside it (the rename box) keeps its own keys,
 * and ⌘/⌃/⌥ belong to the app rather than to the row.
 */
export function rowKeyAction(e: {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  /** Typed in a field inside the list: the field keeps it. */
  editable?: boolean;
}): RowKeyAction | null {
  if (e.editable || e.metaKey || e.ctrlKey || e.altKey) return null;
  if (e.key === "ArrowDown") return { kind: "move", delta: 1 };
  if (e.key === "ArrowUp") return { kind: "move", delta: -1 };
  if (e.key === "Enter") return { kind: "open" };
  if (e.key === "Escape") return { kind: "clear" };
  return null;
}

/**
 * What a thread's calls changed, from the diffs the app already read (`_rustcode/rewind`
 * history, replayed when the thread is opened). Null when it has none to count: a thread never
 * opened in this run has no items, and shows no totals.
 */
const counts = new WeakMap<object, { added: number; removed: number }>();

export function threadDiff(
  items: readonly TranscriptItem[] | undefined,
): { added: number; removed: number } | null {
  if (!items?.length) return null;
  let added = 0;
  let removed = 0;
  let any = false;
  for (const item of items) {
    if (item.kind !== "tool" || !item.diffs?.length) continue;
    // A held item's diffs do not change: counted once, however often a reply streams past.
    let sum = counts.get(item);
    if (!sum) {
      sum = diffCounts(item.diffs);
      counts.set(item, sum);
    }
    added += sum.added;
    removed += sum.removed;
    any = true;
  }
  return any ? { added, removed } : null;
}
