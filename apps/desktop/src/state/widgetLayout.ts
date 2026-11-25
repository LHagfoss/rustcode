/**
 * Which cards the new-thread overview shows, in what order and how wide. The user arranges them
 * in the overview's edit mode; the arrangement is kept on this device. Everything here is pure
 * apart from the small store at the end, so the grid and its tests share one answer.
 */

import { useSyncExternalStore } from "react";
import { readStorage, writeStorage } from "../lib/storage";

export const WIDGET_IDS = [
  "activity",
  "threads",
  "spend",
  "needs",
  "pulls",
  "running",
  "today",
  "models",
  "harnesses",
  "rhythm",
] as const;
export type WidgetId = (typeof WIDGET_IDS)[number];

/** The overview's grid is this many columns wide; a card takes one or two columns and one or two rows. */
export const GRID_COLUMNS = 4;
export type WidgetExtent = 1 | 2;

export interface WidgetSlot {
  id: WidgetId;
  w: WidgetExtent;
  h: WidgetExtent;
  /** The other cards stacked in this place, which take turns with `id` (a stack, as on iOS). */
  stack?: WidgetId[];
}

/** How long a stack shows one card before it turns to the next, in milliseconds. */
export const STACK_INTERVAL_MS = 8000;

/** The cards a place holds, its own first. */
export function stackMembers(slot: WidgetSlot): WidgetId[] {
  return [slot.id, ...(slot.stack ?? [])];
}

export const WIDGET_NAMES: Record<WidgetId, string> = {
  activity: "Activity",
  threads: "Recent threads",
  spend: "Spend",
  needs: "Needs you",
  pulls: "Pull requests",
  running: "Running now",
  today: "Today",
  models: "Models",
  harnesses: "Harnesses",
  rhythm: "Weekly rhythm",
};

export const WIDGET_BLURBS: Record<WidgetId, string> = {
  activity: "Turns per day and your streak",
  threads: "This project's latest threads",
  spend: "This month's spend and 30 days of tokens",
  needs: "Approvals and questions waiting on you",
  pulls: "This project's pull requests",
  running: "Threads with a turn running",
  today: "Today's turns and tokens against yesterday, and the last seven days",
  models: "The models this month's turns ran on",
  harnesses: "Which harnesses this month's turns ran on",
  rhythm: "The weekdays you run the most turns on",
};

/** The size a card takes when it is added back: the cards with the most to show get the most room. */
const DEFAULT_SIZE: Record<WidgetId, Pick<WidgetSlot, "w" | "h">> = {
  activity: { w: 2, h: 2 },
  threads: { w: 1, h: 2 },
  needs: { w: 1, h: 1 },
  running: { w: 1, h: 1 },
  spend: { w: 2, h: 1 },
  pulls: { w: 2, h: 1 },
  today: { w: 2, h: 1 },
  models: { w: 2, h: 1 },
  harnesses: { w: 1, h: 1 },
  rhythm: { w: 1, h: 1 },
};

const slotOf = (id: WidgetId): WidgetSlot => ({ id, ...DEFAULT_SIZE[id] });

/**
 * Four full rows: the activity square and the threads column with the two quiet cards beside them,
 * then the models, harnesses and rhythm, then pulls and today. Spend waits in the tray, because the
 * dashboard's figures over the grid already lead with it.
 */
export const DEFAULT_LAYOUT: readonly WidgetSlot[] = (
  ["activity", "threads", "needs", "running", "models", "harnesses", "rhythm", "pulls", "today"] as const
).map(slotOf);

/** Cards an earlier version had, and the card that took their place. */
const RENAMED: Record<string, WidgetId> = { usage: "spend", month: "spend" };

const isWidgetId = (value: unknown): value is WidgetId => WIDGET_IDS.includes(value as WidgetId);
const isExtent = (value: unknown): value is WidgetExtent => value === 1 || value === 2;

/** A stored layout, or the default when it is missing or unreadable. Unknown and repeated cards drop out, and a bad size takes the card's own. */
export function parseLayout(raw: string | null): WidgetSlot[] {
  if (raw === null) return [...DEFAULT_LAYOUT];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [...DEFAULT_LAYOUT];
    const seen = new Set<WidgetId>();
    const slots: WidgetSlot[] = [];
    for (const item of parsed) {
      const stored = (item as { id?: unknown })?.id;
      const id = typeof stored === "string" ? (RENAMED[stored] ?? stored) : stored;
      if (!isWidgetId(id) || seen.has(id)) continue;
      seen.add(id);
      const { w, h, stack } = item as { w?: unknown; h?: unknown; stack?: unknown };
      const slot = isExtent(w) && isExtent(h) && id === stored ? { id, w, h } : slotOf(id);
      const members: WidgetId[] = [];
      for (const member of Array.isArray(stack) ? stack : []) {
        if (!isWidgetId(member) || seen.has(member)) continue;
        seen.add(member);
        members.push(member);
      }
      slots.push(members.length ? { ...slot, stack: members } : slot);
    }
    return slots;
  } catch {
    return [...DEFAULT_LAYOUT];
  }
}

/** The layout with `id` moved to `index`, counted in the layout without it. */
export function moveWidget(layout: readonly WidgetSlot[], id: WidgetId, index: number): WidgetSlot[] {
  const moving = layout.find((slot) => slot.id === id);
  if (!moving) return [...layout];
  const rest = layout.filter((slot) => slot.id !== id);
  const to = Math.max(0, Math.min(index, rest.length));
  rest.splice(to, 0, moving);
  return rest;
}

/** The layout without `id`: a stacked card leaves its stack, and a stack's own card hands the place to the next. */
export function removeWidget(layout: readonly WidgetSlot[], id: WidgetId): WidgetSlot[] {
  return layout.flatMap((slot) => {
    const members = stackMembers(slot);
    if (!members.includes(id)) return [slot];
    const [first, ...rest] = members.filter((member) => member !== id);
    if (!first) return [];
    return [rest.length ? { ...slot, id: first, stack: rest } : { id: first, w: slot.w, h: slot.h }];
  });
}

/** The layout with `id` stacked into the place `target` holds, taken from wherever it was. */
export function stackWidget(layout: readonly WidgetSlot[], target: WidgetId, id: WidgetId): WidgetSlot[] {
  if (target === id) return [...layout];
  const without = removeWidget(layout, id);
  if (!without.some((slot) => slot.id === target)) return [...layout];
  return without.map((slot) => (slot.id === target ? { ...slot, stack: [...(slot.stack ?? []), id] } : slot));
}

export function addWidget(layout: readonly WidgetSlot[], id: WidgetId): WidgetSlot[] {
  if (layout.some((slot) => stackMembers(slot).includes(id))) return [...layout];
  return [...layout, slotOf(id)];
}

/** The sizes a card steps through, wide before tall, so one control reaches them all. */
const SIZES: readonly Pick<WidgetSlot, "w" | "h">[] = [
  { w: 1, h: 1 },
  { w: 2, h: 1 },
  { w: 1, h: 2 },
  { w: 2, h: 2 },
];

export function nextSize(slot: WidgetSlot): WidgetSlot {
  const at = SIZES.findIndex((size) => size.w === slot.w && size.h === slot.h);
  return { ...slot, ...SIZES[(at + 1) % SIZES.length] };
}

export function resizeWidget(layout: readonly WidgetSlot[], id: WidgetId): WidgetSlot[] {
  return layout.map((slot) => (slot.id === id ? nextSize(slot) : slot));
}

/** The cards the layout leaves out, in the catalogue's order, for the add tray. */
export function hiddenWidgets(layout: readonly WidgetSlot[]): WidgetId[] {
  return WIDGET_IDS.filter((id) => !layout.some((slot) => stackMembers(slot).includes(id)));
}

export function sameLayout(one: readonly WidgetSlot[], other: readonly WidgetSlot[]): boolean {
  return (
    one.length === other.length &&
    one.every(
      (slot, i) =>
        slot.id === other[i]?.id &&
        slot.w === other[i]?.w &&
        slot.h === other[i]?.h &&
        (slot.stack ?? []).join() === (other[i]?.stack ?? []).join(),
    )
  );
}

const LAYOUT_KEY = "rc-new-thread-widgets";
const listeners = new Set<() => void>();
let current: WidgetSlot[] | undefined;

export function getWidgetLayout(): WidgetSlot[] {
  current ??= parseLayout(readStorage(LAYOUT_KEY));
  return current;
}

export function setWidgetLayout(layout: readonly WidgetSlot[]): void {
  current = [...layout];
  writeStorage(LAYOUT_KEY, sameLayout(layout, DEFAULT_LAYOUT) ? null : JSON.stringify(layout));
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useWidgetLayout(): WidgetSlot[] {
  return useSyncExternalStore(subscribe, getWidgetLayout);
}

/** A grid row's height when the overview has no room to fill, and the least and most it takes when it has. */
export const ROW_HEIGHT = 128;
export const MIN_ROW_HEIGHT = 112;
export const MAX_ROW_HEIGHT = 280;
/** The space between the grid's cards, in pixels. */
export const CARD_GAP = 8;

/**
 * Where the grid puts each card, as CSS grid's dense auto-placement does: each card in order takes
 * the first cell from the top left where it fits, so a small card fills a hole a big one left.
 */
export function packLayout(
  layout: readonly WidgetSlot[],
  columns = GRID_COLUMNS,
): { rows: number; places: { id: WidgetId; row: number; column: number }[] } {
  const taken: boolean[][] = [];
  const fits = (row: number, column: number, slot: WidgetSlot) => {
    for (let r = row; r < row + slot.h; r++)
      for (let c = column; c < column + slot.w; c++) if (taken[r]?.[c]) return false;
    return true;
  };
  const places: { id: WidgetId; row: number; column: number }[] = [];
  for (const wanted of layout) {
    const slot = { ...wanted, w: Math.min(wanted.w, columns) as WidgetExtent };
    let row = 0;
    let column = 0;
    while (!fits(row, column, slot)) {
      column += 1;
      if (column + slot.w > columns) {
        column = 0;
        row += 1;
      }
    }
    for (let r = row; r < row + slot.h; r++) {
      const cells = taken[r] ?? [];
      taken[r] = cells;
      for (let c = column; c < column + slot.w; c++) cells[c] = true;
    }
    places.push({ id: slot.id, row, column });
  }
  return { rows: taken.length, places };
}

/** The one row height that lets `rows` rows fill `room`, kept between the least and the most. */
export function fittedRowHeight(room: number, rows: number): number {
  if (rows === 0) return ROW_HEIGHT;
  const each = Math.floor((room - (rows - 1) * CARD_GAP) / rows);
  return Math.max(MIN_ROW_HEIGHT, Math.min(MAX_ROW_HEIGHT, each));
}

/** A card's height in pixels: its rows and the gaps between them. */
export function cardHeight(row: number, h: WidgetExtent): number {
  return row * h + CARD_GAP * (h - 1);
}
