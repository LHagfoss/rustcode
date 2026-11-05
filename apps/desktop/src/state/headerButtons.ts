// Which buttons the thread header shows, in what order (Settings › Chat ›
// Thread header buttons). Pure, so the store, the header, the settings list
// and the tests share one order and one sanitiser.
//
// The stored value is `{ shown, known }`: the ids shown in order, and the ids
// the build that saved it knew about. Anything not in `shown` is hidden, and a
// button added in a later release (an id missing from `known`) shows by
// default, after the buttons the user arranged.

/** The header's buttons, in the order a fresh install shows them. */
export const HEADER_BUTTON_IDS = ["status", "plan", "pulls", "terminal", "details", "changes"] as const;

export type HeaderButtonId = (typeof HEADER_BUTTON_IDS)[number];

/** Every button, shown, in registry order: what a fresh install and a reset start from. */
export const DEFAULT_HEADER_BUTTONS: readonly HeaderButtonId[] = HEADER_BUTTON_IDS;

/** What `rustcode.headerButtons` holds. */
export interface StoredHeaderButtons {
  shown: HeaderButtonId[];
  known: HeaderButtonId[];
}

export function isHeaderButtonId(value: unknown): value is HeaderButtonId {
  return typeof value === "string" && (HEADER_BUTTON_IDS as readonly string[]).includes(value);
}

/** Two orders are the same when they list the same ids in the same order. */
export function sameHeaderButtons(a: readonly HeaderButtonId[], b: readonly HeaderButtonId[]): boolean {
  return a.length === b.length && a.every((id, i) => id === b[i]);
}

/**
 * The shown ids a stored value means, in order: unknown ids and duplicates
 * dropped, an empty list kept (every button hidden is a valid choice), and a
 * button this build knows but the saved value did not appended at the end. No
 * usable value at all (nothing stored, or not JSON of that shape) shows every
 * button.
 */
export function sanitizeHeaderButtons(raw: unknown): HeaderButtonId[] {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return [...DEFAULT_HEADER_BUTTONS];
  const { shown: storedShown, known: storedKnown } = raw as Partial<
    Record<keyof StoredHeaderButtons, unknown>
  >;
  if (!Array.isArray(storedShown)) return [...DEFAULT_HEADER_BUTTONS];

  const shown: HeaderButtonId[] = [];
  for (const id of storedShown) if (isHeaderButtonId(id) && !shown.includes(id)) shown.push(id);

  // A value saved before this field existed cannot say which buttons were new,
  // so nothing is re-added: a button the user hid stays hidden.
  const known = Array.isArray(storedKnown) ? storedKnown.filter(isHeaderButtonId) : [...HEADER_BUTTON_IDS];
  for (const id of HEADER_BUTTON_IDS) if (!known.includes(id) && !shown.includes(id)) shown.push(id);
  return shown;
}

/** A row of a list being dragged over: its box in the window. */
export interface DropRow {
  top: number;
  height: number;
}

/**
 * Where a dragged row lands: the count of rows whose middle is above the
 * pointer, which is also the slot the insertion line goes before (the end of
 * the list when the pointer is below every middle). Pass the rows without the
 * dragged one, so the answer is already its index in the list it will land in.
 */
export function insertIndex(rows: readonly DropRow[], y: number): number {
  let index = 0;
  for (const row of rows) if (y > row.top + row.height / 2) index += 1;
  return index;
}
