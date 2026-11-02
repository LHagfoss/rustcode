// The thread's timeline, after the SwiftUI app's TimelineRail
// (TimelineRail.swift, TimelineMarkers.swift): a short tick per message the
// user sent, stacked 8px apart and centred in the gutter left of the chat's
// column. The turn being read (the last one starting above 30% of the view,
// or the last turn while the view follows the end) has the widest tick, in
// the accent; the others narrow and fade with distance. Click a tick to go
// to its turn; hover shows its first words. It renders only when the turns
// or the current one change, never per scroll frame.

import { memo } from "react";
import { cx, Tooltip } from "./primitives";

export interface Turn {
  /** The user message's row in the transcript. */
  row: number;
  title: string;
}

/** Between ticks, and between the rail's ends and its outermost ticks. */
const STEP = 8;
const MARGIN = 8;

/** Where the rail sits for the chat's width (ChatColumnLayout.swift); null when too narrow. */
export function railPlacement(width: number): { x: number; width: number } | null {
  if (width >= 1072) return { x: Math.min(10, ((width - 980) / 2 - 28) / 2), width: 28 };
  if (width >= 600) return { x: 5, width: 18 };
  if (width >= 360) return { x: 3, width: 14 };
  return null;
}

/** The first words of `text`, on one line, cut at a word. */
export function turnTitle(text: string, limit = 60): string {
  let out = "";
  for (const word of text.split(/\s+/).filter(Boolean)) {
    const next = out ? `${out} ${word}` : word;
    if (next.length > limit) return out ? `${out}…` : `${word.slice(0, limit)}…`;
    out = next;
  }
  return out || "Message";
}

export const TimelineRail = memo(function TimelineRail({
  turns,
  current,
  width,
  visible,
  offset = 0,
  onJump,
}: {
  turns: Turn[];
  /** Index into `turns`. */
  current: number;
  /** The transcript's width and the height of its visible part (above the composer). */
  width: number;
  visible: number;
  /** Where the visible part starts (under a floating header). */
  offset?: number;
  onJump: (row: number) => void;
}) {
  const place = railPlacement(width);
  if (!place || turns.length < 2 || visible <= 0) return null;
  const longest = place.width - 6;
  const fits = Math.max(1, Math.floor((visible - 2 * MARGIN) / STEP) + 1);
  const shown = Math.min(turns.length, fits);
  const first = Math.min(Math.max(current - Math.floor(shown / 2), 0), turns.length - shown);
  const top = (visible - (shown - 1) * STEP) / 2;
  return (
    <nav
      aria-label="Thread timeline"
      className="rc-fade-in pointer-events-none absolute z-10"
      style={{ top: offset, left: place.x, width: place.width, height: visible }}
    >
      {turns.slice(first, first + shown).map((turn, i) => {
        const index = first + i;
        const distance = Math.abs(index - current);
        const isCurrent = index === current;
        const scale = isCurrent ? 1 : Math.max(0.45, 0.65 - 0.05 * distance);
        const opacity = isCurrent ? 1 : Math.max(0.55, 0.9 - 0.08 * distance) * 0.55;
        return (
          <Tooltip key={turn.row} label={turn.title} side="right">
            <button
              type="button"
              aria-label={`Turn ${index + 1}: ${turn.title}`}
              aria-current={isCurrent ? "step" : undefined}
              onClick={() => onJump(turn.row)}
              className="group pointer-events-auto absolute left-0 flex w-full items-center justify-center"
              style={{ top: top + i * STEP - STEP / 2, height: STEP }}
            >
              <span
                className={cx(
                  "block h-0.5 rounded-full transition-[width,opacity,background-color] duration-150 ease-out",
                  isCurrent ? "bg-accent" : "bg-fg group-hover:!opacity-80",
                )}
                style={{ width: longest * scale, opacity }}
              />
            </button>
          </Tooltip>
        );
      })}
    </nav>
  );
});
