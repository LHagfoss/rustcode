/**
 * Which cards the new-thread overview shows, in what order and how wide. The user arranges them
 * in the overview's edit mode; the arrangement is kept on this device. Everything here is pure
 * apart from the small store at the end, so the grid and its tests share one answer.
 */

import { useSyncExternalStore } from "react";
import { readStorage, writeStorage } from "../lib/storage";

export const WIDGET_IDS = ["activity", "threads", "spend", "needs", "pulls", "running"] as const;
export type WidgetId = (typeof WIDGET_IDS)[number];

/** Columns of the overview's six-column grid a card may take. */
export const WIDGET_SPANS = [2, 3, 4, 6] as const;
export type WidgetSpan = (typeof WIDGET_SPANS)[number];

export interface WidgetSlot {
  id: WidgetId;
  span: WidgetSpan;
}

export const WIDGET_NAMES: Record<WidgetId, string> = {
  activity: "Activity",
  threads: "Recent threads",
  spend: "Spend",
  needs: "Needs you",
  pulls: "Pull requests",
  running: "Running now",
};

export const WIDGET_BLURBS: Record<WidgetId, string> = {
  activity: "Turns per day and your streak",
  threads: "This project's latest threads",
  spend: "This month's spend and 30 days of tokens",
  needs: "Approvals and questions waiting on you",
  pulls: "This project's pull requests",
  running: "Threads with a turn running",
};

/** The span a card takes when it is added back. */
const DEFAULT_SPAN: Record<WidgetId, WidgetSpan> = {
  activity: 3,
  threads: 3,
  spend: 2,
  needs: 2,
  pulls: 2,
  running: 2,
};

export const DEFAULT_LAYOUT: readonly WidgetSlot[] = [
  { id: "activity", span: 3 },
  { id: "threads", span: 3 },
  { id: "spend", span: 2 },
  { id: "needs", span: 2 },
  { id: "pulls", span: 2 },
];

/** Cards an earlier version had, and the card that took their place. */
const RENAMED: Record<string, WidgetId> = { usage: "spend", month: "spend" };

const isWidgetId = (value: unknown): value is WidgetId => WIDGET_IDS.includes(value as WidgetId);
const isSpan = (value: unknown): value is WidgetSpan => WIDGET_SPANS.includes(value as WidgetSpan);

/** A stored layout, or the default when it is missing or unreadable. Unknown and repeated cards drop out. */
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
      const span = (item as { span?: unknown }).span;
      slots.push({ id, span: isSpan(span) && id === stored ? span : DEFAULT_SPAN[id] });
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

export function removeWidget(layout: readonly WidgetSlot[], id: WidgetId): WidgetSlot[] {
  return layout.filter((slot) => slot.id !== id);
}

export function addWidget(layout: readonly WidgetSlot[], id: WidgetId): WidgetSlot[] {
  if (layout.some((slot) => slot.id === id)) return [...layout];
  return [...layout, { id, span: DEFAULT_SPAN[id] }];
}

/** The next width up, wrapping from full width back to the narrowest. */
export function nextSpan(span: WidgetSpan): WidgetSpan {
  return WIDGET_SPANS[(WIDGET_SPANS.indexOf(span) + 1) % WIDGET_SPANS.length] ?? span;
}

export function resizeWidget(layout: readonly WidgetSlot[], id: WidgetId): WidgetSlot[] {
  return layout.map((slot) => (slot.id === id ? { ...slot, span: nextSpan(slot.span) } : slot));
}

/** The cards the layout leaves out, in the catalogue's order, for the add tray. */
export function hiddenWidgets(layout: readonly WidgetSlot[]): WidgetId[] {
  return WIDGET_IDS.filter((id) => !layout.some((slot) => slot.id === id));
}

export function sameLayout(one: readonly WidgetSlot[], other: readonly WidgetSlot[]): boolean {
  return (
    one.length === other.length &&
    one.every((slot, i) => slot.id === other[i]?.id && slot.span === other[i]?.span)
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
