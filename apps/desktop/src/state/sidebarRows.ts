// The sidebar's rows as data: the order they are drawn in, what a thread's calls changed, and
// the rules that move and pick them. Pure, so the sidebar and its tests share one answer
// (ui/Sidebar.tsx).

import type { SessionSummary } from "../acp/types";
import { diffCounts, type TranscriptItem } from "./thread";

/** A row the list draws: a top-level thread, or the open delegate's indented row. */
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
 * The row that stands for the open thread. A delegate is a row of its own while it is open; one
 * whose indented row is not drawn (a filter took it away) answers with its nearest drawn ancestor,
 * so the keyboard lands on something visible and the context stays lit.
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
 * Whether a key opens a row's menu: Shift+F10 and the ContextMenu key, the two every desktop list
 * answers, so the whole action list is reachable without a pointer. F10 alone belongs to the
 * window's own menu.
 */
export function opensRowMenu(e: { key: string; shiftKey?: boolean }): boolean {
  return (e.key === "F10" && !!e.shiftKey) || e.key === "ContextMenu";
}

/** What a row's one status chip says: the states worth words rather than an age. */
export type RowStatusKind = "needs" | "working" | "waiting" | "failed" | "review";

/** The chip a row's first line ends with (`ui/Sidebar.tsx` draws it). */
export interface RowStatus {
  kind: RowStatusKind;
  /** The chip's words: `Needs you`, `Working`, `Waiting`, `Failed`, `Review`. */
  label: string;
  /** What the chip says on hover, when its words leave something out. */
  title?: string;
  /** The permission is an approval of a call, not a question the agent asked. */
  approval?: boolean;
}

/** What {@link rowStatus} reads of a row: the caller resolves each flag. */
export interface RowStatusRead {
  /** The permission it waits on: the oldest of its requests. */
  request?: { title: string; question?: { text: string } };
  /** A turn of its own runs. */
  running: boolean;
  /** Background work it handed out and still waits on (`state/waiting.ts`). */
  waiting?: { label: string; title: string };
  /** Its last turn failed while the row was not open. */
  failed?: boolean;
  /** The thread is listed as waiting for review. */
  review?: boolean;
}

/**
 * The one chip a row shows. One state at a time, in the order the reader needs it: what it asks of
 * you, then that it works, then that it waits on work it handed out, then a failure, then a review.
 * Nothing when it is idle or settled: the row then shows how long ago it moved, as it always has.
 * The clock beside `Working` is the running turn's own, so a row ticks in exactly one place.
 */
export function rowStatus(read: RowStatusRead): RowStatus | undefined {
  const request = read.request;
  if (request) {
    return {
      kind: "needs",
      label: "Needs you",
      title: request.question ? request.question.text : `Approval: ${request.title}`,
      approval: !request.question,
    };
  }
  if (read.running) return { kind: "working", label: "Working" };
  if (read.waiting) {
    const waiting = read.waiting;
    return {
      kind: "waiting",
      label: "Waiting",
      title: waiting.title ? `${waiting.label}: ${waiting.title}` : waiting.label,
    };
  }
  if (read.failed) return { kind: "failed", label: "Failed", title: "Its last turn failed" };
  if (read.review) return { kind: "review", label: "Review", title: "It waits for your review" };
  return undefined;
}

/**
 * What a thread's calls changed, from the diffs the app already read (`_rustcode/rewind`
 * history, replayed when the thread is opened). Null when it has none to count: a thread never
 * opened in this run has no items, and shows no totals.
 *
 * A file counts once, by the widest edit it was given: a thread that goes back to the same file
 * forty times has not added forty times the file's lines, and summing the calls is what turned a
 * row's `+12` into `+12404`. Only a path's own line counts bound the number now, so it reads as
 * the size of what the thread touched rather than as churn.
 */
const counts = new WeakMap<object, Map<string, { added: number; removed: number }>>();

export function threadDiff(
  items: readonly TranscriptItem[] | undefined,
): { added: number; removed: number } | null {
  if (!items?.length) return null;
  const perFile = new Map<string, { added: number; removed: number }>();
  for (const item of items) {
    if (item.kind !== "tool" || !item.diffs?.length) continue;
    // A held item's diffs do not change: read once, however often a reply streams past.
    let files = counts.get(item);
    if (!files) {
      files = new Map();
      for (const diff of item.diffs) {
        const n = diffCounts([diff]);
        files.set(diff.path, { added: n.added, removed: n.removed });
      }
      counts.set(item, files);
    }
    for (const [path, n] of files) {
      const seen = perFile.get(path);
      perFile.set(
        path,
        seen
          ? { added: Math.max(seen.added, n.added), removed: Math.max(seen.removed, n.removed) }
          : { added: n.added, removed: n.removed },
      );
    }
  }
  if (!perFile.size) return null;
  let added = 0;
  let removed = 0;
  for (const n of perFile.values()) {
    added += n.added;
    removed += n.removed;
  }
  return { added, removed };
}
