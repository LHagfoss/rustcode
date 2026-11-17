// The live background's beats, after Sable's art/events.ts (MIT, Copyright (c) 2026 Leonxlnx):
// moments in the app the light answers. Anyone may send one; every animated field that reacts
// plays it from the given point (viewport coordinates). Still fields, Reduce motion and a field
// at its lowest quality ignore them.
//
// - `send`: a message left the composer (from the send key).
// - `done` / `fail`: a turn ended, from the composer the reply came back to (`done` from the
//   middle of its top edge: the light settles down onto it).
// - `pulse`: a quieter beat for smaller moments.

import { motionAllowed } from "../sendFlight";
import type { FieldBeat } from "./fieldBase";

export const FIELD_EVENT = "rustcode:field";

export interface FieldEventDetail {
  kind: FieldBeat;
  x: number;
  y: number;
  strength: number;
}

/** Sends a beat from `el`: from its centre, or with `at: "top"` from the middle of its top edge. */
export function fieldBeat(
  kind: FieldBeat,
  el: Element | null | undefined,
  strength = 1,
  at: "centre" | "top" = "centre",
) {
  if (!el || !motionAllowed()) return;
  const r = el.getBoundingClientRect();
  if (!r.width) return;
  const detail: FieldEventDetail = {
    kind,
    x: r.left + r.width / 2,
    y: at === "top" ? r.top : r.top + r.height / 2,
    strength,
  };
  window.dispatchEvent(new CustomEvent<FieldEventDetail>(FIELD_EVENT, { detail }));
}
