// F6 / Shift+F6: move focus between the window's areas (the Threads sidebar, the conversation,
// the message input, the right panel, the terminal), or in Settings between the section list
// and the open section. Each area is a labelled landmark marked with `data-focus-area`; the
// order is fixed here rather than read from the DOM, which puts the docked right panel inside
// the chat row. An area gets back the control that last had focus in it.
//
// `stepArea` and `restoreChoice` are pure and tested; the rest reads the document.

/** The attribute naming an area on its root element. */
export const FOCUS_AREA_ATTR = "data-focus-area";
/** Inside an area, where focus lands when nothing there had it yet (the message field). */
export const FOCUS_DEFAULT_ATTR = "data-focus-default";
/** A dialog whose own areas F6 moves between (Settings) instead of standing still. */
export const FOCUS_SCOPE_ATTR = "data-focus-scope";

/** The window's areas, in F6 order. "main" is the conversation, or the open view's content. */
export const WINDOW_AREAS = ["threads", "main", "composer", "panel", "terminal"] as const;
/** Settings' areas, in F6 order. */
export const SETTINGS_AREAS = ["sections", "section"] as const;

/**
 * The area F6 (`step` 1) or Shift+F6 (`step` -1) moves to: the next one in `order` that is
 * shown, wrapping. From no area (or one that is not in `order`) it starts at the first shown one,
 * or the last going back. Null when nothing is shown, or only the current one is.
 */
export function stepArea(
  order: readonly string[],
  shown: ReadonlySet<string>,
  current: string | null,
  step: 1 | -1,
): string | null {
  const at = current === null ? -1 : order.indexOf(current);
  const n = order.length;
  if (n === 0) return null;
  let i = at === -1 ? (step === 1 ? -1 : n) : at;
  for (let tries = 0; tries < n; tries += 1) {
    i = (((i + step) % n) + n) % n;
    const area = order[i] as string;
    if (area === current) return null;
    if (shown.has(area)) return area;
  }
  return null;
}

/**
 * Where focus goes in an area: the control that last had it while it can still take focus,
 * else the first usable fallback (the area's default field, its first focusable control, the
 * area itself), else null.
 */
export function restoreChoice<T>(
  remembered: T | null | undefined,
  usable: (target: T) => boolean,
  fallbacks: readonly (T | null | undefined)[],
): T | null {
  if (remembered != null && usable(remembered)) return remembered;
  for (const fallback of fallbacks) if (fallback != null && usable(fallback)) return fallback;
  return null;
}

const TABBABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type=hidden])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[contenteditable=true]",
  "[tabindex]",
]
  .map((s) => `${s}:not([tabindex="-1"])`)
  .join(",");

/** What holds the keyboard on its own: F6 leaves focus alone while one of these has it. */
const HOLDS_KEYBOARD = '[role="dialog"], [role="alertdialog"], [aria-modal="true"], [role="menu"]';

/** The last focused control in each area, by the area's name. */
const lastFocused = new Map<string, HTMLElement>();

function isShown(el: HTMLElement): boolean {
  if (!el.isConnected || el.getClientRects().length === 0) return false;
  return !el.closest('[aria-hidden="true"], [inert]');
}

function canFocus(el: HTMLElement, area: HTMLElement): boolean {
  return area.contains(el) && isShown(el) && !(el as HTMLButtonElement).disabled;
}

function areaOf(el: Element | null): HTMLElement | null {
  return el?.closest<HTMLElement>(`[${FOCUS_AREA_ATTR}]`) ?? null;
}

/** Keeps each area's last focused control; the app listens with it on `focusin`. */
export function rememberFocus(event: FocusEvent): void {
  const target = event.target;
  if (!(target instanceof HTMLElement)) return;
  const area = areaOf(target);
  const name = area?.getAttribute(FOCUS_AREA_ATTR);
  if (name) lastFocused.set(name, target);
}

/**
 * Moves focus to the next (1) or previous (-1) area. False when it did nothing: a dialog, a
 * menu or the palette has the keyboard, or there is no other area to go to.
 */
export function cycleFocusArea(step: 1 | -1): boolean {
  const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const scope = document.querySelector<HTMLElement>(`[${FOCUS_SCOPE_ATTR}]`);
  const holder = active?.closest<HTMLElement>(HOLDS_KEYBOARD) ?? null;
  if (holder && holder !== scope) return false;
  const root: ParentNode = scope ?? document;
  const order: readonly string[] = scope ? SETTINGS_AREAS : WINDOW_AREAS;
  const areas = new Map<string, HTMLElement>();
  for (const el of root.querySelectorAll<HTMLElement>(`[${FOCUS_AREA_ATTR}]`)) {
    const name = el.getAttribute(FOCUS_AREA_ATTR);
    if (!name || areas.has(name) || !isShown(el)) continue;
    if (!scope && el.closest(HOLDS_KEYBOARD)) continue;
    areas.set(name, el);
  }
  const current = active && root.contains(active) ? areaOf(active)?.getAttribute(FOCUS_AREA_ATTR) : null;
  const next = stepArea(order, new Set(areas.keys()), current ?? null, step);
  const area = next ? areas.get(next) : undefined;
  if (!next || !area) return false;
  const preferred = area.querySelector<HTMLElement>(`[${FOCUS_DEFAULT_ATTR}]`);
  const target = restoreChoice<HTMLElement>(lastFocused.get(next), (el) => canFocus(el, area), [
    preferred?.matches(TABBABLE) ? preferred : preferred?.querySelector<HTMLElement>(TABBABLE),
    ...area.querySelectorAll<HTMLElement>(TABBABLE),
    area,
  ]);
  if (!target) return false;
  // The area's root takes focus only as the last resort; make it able to.
  if (target === area && !area.hasAttribute("tabindex")) area.tabIndex = -1;
  target.focus();
  return document.activeElement === target;
}
