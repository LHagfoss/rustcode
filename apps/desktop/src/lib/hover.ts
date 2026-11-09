// One behaviour for every tip in the app, tooltip or hover card (docs/design.md › Tooltips and
// hover cards). The pointer rests `HOVER_OPEN_MS` before a tip appears, so passing over a button
// says nothing and the app never talks over the user; a tip the pointer has left keeps its place
// `HOVER_CLOSE_MS`, so a slip off it and back changes nothing; and a target the pointer enters
// while a tip is open — or a moment after one closed — takes the tip over at once, so reading
// along a row of buttons never waits. A press, Escape, a scroll, a resize, the window losing focus
// or the target going away puts the tip out, and nothing brings it back on its own: the next tip
// waits for a fresh pointer enter, and a click's own focus is not one.
//
// The tips are one store rather than one per component: only one is ever on screen, and a
// neighbour has to be able to take the open tip's place without a round trip through React. There
// are two stores, one for tooltips and one for hover cards, so a tip inside a card (the usage
// card's context tiles) never takes the card itself out from under the pointer.

import {
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { KEYBOARD_NAV } from "./focusModality";

/** How long the pointer rests on a target before its tip appears. */
export const HOVER_OPEN_MS = 300;

/** How long a tip outlives the pointer leaving its target: a slip off and back changes nothing. */
export const HOVER_CLOSE_MS = 80;

/** A target entered this soon after a tip closed takes the closed tip's place at once. */
export const HOVER_SWITCH_MS = 400;

/** A tip's identity: one per mounted tooltip or hover card. */
export type TipId = symbol;

export interface HoverTiming {
  /** Milliseconds since load, monotonic. */
  now: () => number;
  /** Runs `run` after `ms`; the handle it returns goes to `cancel`. */
  after: (ms: number, run: () => void) => unknown;
  cancel: (handle: unknown) => void;
}

/** The tips' one open/close timing. A class rather than a set of timers, so it can be tested with
 * a clock that does not exist (lib/hover.test.ts). */
export class HoverTips {
  #timing: HoverTiming;
  /** The tip on screen. */
  #open: TipId | undefined;
  /** The target the pointer rests on whose tip is still waiting. */
  #waiting: TipId | undefined;
  /** When the last tip closed, for the instant hand-over to the next one. */
  #closedAt: number | undefined;
  #openTimer: unknown;
  #closeTimer: unknown;
  #listeners = new Set<() => void>();

  constructor(timing: Partial<HoverTiming> = {}) {
    this.#timing = {
      now: timing.now ?? (() => performance.now()),
      after: timing.after ?? ((ms, run) => setTimeout(run, ms)),
      cancel: timing.cancel ?? ((handle) => clearTimeout(handle as ReturnType<typeof setTimeout>)),
    };
  }

  /** Stable, for `useSyncExternalStore`. */
  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** Whether this tip is the one on screen. */
  isOpen(id: TipId): boolean {
    return this.#open === id;
  }

  /** Whether this tip is on screen or still waiting to be: the tip listens for its dismissals
   * while it waits too, so a scroll during the rest never leaves one to appear behind it. */
  isArmed(id: TipId): boolean {
    return this.#open === id || this.#waiting === id;
  }

  /** The pointer or the keyboard arrived on a target. */
  enter(id: TipId): void {
    if (this.#open === id) {
      // Back on the tip's own target inside its close delay: it stays, no flicker.
      this.#cancelClose();
      return;
    }
    // Whatever was waiting is no longer where the pointer is.
    this.#cancelOpen();
    const handOver = this.#open !== undefined || this.#justClosed();
    if (handOver) {
      this.#show(id);
      return;
    }
    this.#waiting = id;
    this.#openTimer = this.#timing.after(HOVER_OPEN_MS, () => {
      this.#openTimer = undefined;
      // The pointer may have left while the timer ran; a tip never appears behind it.
      if (this.#waiting !== id) return;
      this.#show(id);
    });
  }

  /** The pointer or the keyboard left a target: its tip has the close delay, then goes. */
  leave(id: TipId): void {
    if (this.#waiting === id) this.#cancelOpen();
    if (this.#open !== id) return;
    this.#cancelClose();
    this.#closeTimer = this.#timing.after(HOVER_CLOSE_MS, () => {
      this.#closeTimer = undefined;
      this.#hide();
    });
  }

  /** A press, Escape or a scroll: the tip goes now and does not come back on its own. */
  dismiss(id: TipId): void {
    if (this.#waiting === id) this.#cancelOpen();
    if (this.#open === id) this.#hide();
  }

  /** The target is gone (it unmounted, or its row was dragged away): nothing waits for it. */
  forget(id: TipId): void {
    if (this.#waiting === id) this.#cancelOpen();
    if (this.#open === id) this.#hide();
  }

  #show(id: TipId): void {
    this.#cancelOpen();
    this.#cancelClose();
    this.#waiting = undefined;
    this.#open = id;
    this.#notify();
  }

  #hide(): void {
    this.#cancelOpen();
    this.#cancelClose();
    this.#waiting = undefined;
    if (this.#open === undefined) return;
    this.#open = undefined;
    this.#closedAt = this.#timing.now();
    this.#notify();
  }

  #justClosed(): boolean {
    return this.#closedAt !== undefined && this.#timing.now() - this.#closedAt <= HOVER_SWITCH_MS;
  }

  #cancelOpen(): void {
    if (this.#openTimer === undefined) return;
    this.#timing.cancel(this.#openTimer);
    this.#openTimer = undefined;
    this.#waiting = undefined;
  }

  #cancelClose(): void {
    if (this.#closeTimer === undefined) return;
    this.#timing.cancel(this.#closeTimer);
    this.#closeTimer = undefined;
  }

  #notify(): void {
    for (const listener of this.#listeners) listener();
  }
}

/** The two families of tip that share the timing. A tooltip and a hover card never close each
 * other: the usage card has tooltips on its own context tiles, and a tip there must not take the
 * card out from under the pointer. */
export type TipTrack = "tooltip" | "card";

const TRACKS: Record<TipTrack, HoverTips> = { tooltip: new HoverTips(), card: new HoverTips() };

/** The tips of one track: a tooltip waits behind a tooltip, a hover card behind a hover card. */
export function tipTrack(track: TipTrack): HoverTips {
  return TRACKS[track];
}

/** Whether the keyboard, not a press, moved focus here (lib/focusModality.ts). A tip follows the
 * keyboard, and a click's own focus is not a reason for one: it would outlive the pointer. */
function keyboardFocus(): boolean {
  return document.documentElement.hasAttribute(KEYBOARD_NAV);
}

export interface HoverTip {
  /** Whether this tip is the one on screen. */
  open: boolean;
  /** Puts this tip out now, and cancels it if it was still waiting. */
  dismiss: () => void;
  /** On the element the tip hangs off: a tooltip's trigger, a hover card's row. */
  triggerProps: {
    onPointerEnter: (event: ReactPointerEvent<HTMLElement>) => void;
    onPointerLeave: () => void;
    onPointerDown: () => void;
    onPointerUp: () => void;
    onClick: () => void;
    onFocus: () => void;
    onBlur: () => void;
  };
  /** On a card that takes the pointer (one with controls in it): the tip holds while the pointer
   * crosses the gap from the trigger to the card and stays on it. */
  contentProps: { onPointerEnter: () => void; onPointerLeave: () => void };
}

/** The one timing a tooltip and a hover card share (lib/hover.ts); see the top of this file. */
export function useHoverTip(track: TipTrack = "tooltip"): HoverTip {
  const tips = tipTrack(track);
  const [id] = useState<TipId>(() => Symbol(track));
  const open = useSyncExternalStore(
    tips.subscribe,
    () => tips.isOpen(id),
    () => false,
  );
  // A tip that is still waiting listens too: a scroll or Escape during the rest dismisses it.
  const armed = useSyncExternalStore(
    tips.subscribe,
    () => tips.isArmed(id),
    () => false,
  );
  // The press that focuses a target, so its own focus does not open the tip it just put away.
  const pressed = useRef(false);
  const dismiss = useCallback(() => tips.dismiss(id), [tips, id]);

  // Nothing may outlive its reason: a scroll moves the target out from under the tip, a resize does
  // the same, Escape is the app's way out of anything floating, and the window losing focus means
  // the pointer is somewhere else now.
  useEffect(() => {
    if (!armed) return;
    const close = () => tips.dismiss(id);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    document.addEventListener("scroll", close, true);
    document.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("scroll", close, true);
      document.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, [armed, tips, id]);

  // A tip whose target unmounts leaves with it instead of staying on screen over nothing.
  useEffect(() => () => tips.forget(id), [tips, id]);

  const triggerProps = useMemo(
    () => ({
      onPointerEnter: (event: ReactPointerEvent<HTMLElement>) => {
        // A touch has no hover: its press is the whole interaction.
        if (event.pointerType === "touch") return;
        pressed.current = false;
        tips.enter(id);
      },
      onPointerLeave: () => {
        pressed.current = false;
        tips.leave(id);
      },
      onPointerDown: () => {
        pressed.current = true;
        tips.dismiss(id);
      },
      onPointerUp: () => {
        pressed.current = false;
      },
      // Activation, by press or by Enter and Space on the keyboard: the target has done its work,
      // and a tip left hanging over the result is the one that needed a second press to dismiss.
      onClick: () => {
        tips.dismiss(id);
      },
      onFocus: () => {
        if (pressed.current || !keyboardFocus()) return;
        tips.enter(id);
      },
      onBlur: () => {
        pressed.current = false;
        tips.leave(id);
      },
    }),
    [tips, id],
  );

  const contentProps = useMemo(
    () => ({
      onPointerEnter: () => tips.enter(id),
      onPointerLeave: () => tips.leave(id),
    }),
    [tips, id],
  );

  return { open, dismiss, triggerProps, contentProps };
}
