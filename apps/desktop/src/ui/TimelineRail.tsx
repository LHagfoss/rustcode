// The thread's minimap: a strip per message you sent, in the gutter left of
// the chat's column, evenly spaced and squeezed to fit however long the thread
// gets. Strips of the turns on screen read brighter. Hovering widens the strips
// around the pointer and shows that turn's message and how it was answered;
// clicking goes there, and the chevrons step a turn at a time. It renders only
// when the turns or the ones on screen change, never per scroll frame.

import { ChevronDown, ChevronUp } from "lucide-react";
import { type MouseEvent, memo, type ReactNode, useRef, useState } from "react";
import { clip } from "../lib/clip";
import { cx, Tooltip } from "./primitives";

export interface Turn {
  /** The user message's row in the transcript. */
  row: number;
  title: string;
}

/** Between strips, while they fit. */
const STEP = 8;
/** Kept free above and below the strip for the chevrons. */
const MARGIN = 40;
/** Where the strip starts in the gutter. */
const LEFT = 12;
/** From this much gutter the minimap stays visible; with less it shows on hover. */
const PERSISTENT_GUTTER = 48;
/** The chat's column; the gutter is what is left beside it. */
const COLUMN = 768;

/** How wide the gutter leaves the rail's hit area, in px: 0 when there is no room to hover it. */
export function railHitWidth(width: number): number {
  const gutter = (width - COLUMN) / 2;
  return Math.max(0, Math.min(40, Math.floor(gutter) - LEFT));
}

/** The strip's height for this many turns: their natural spacing, or what the view has room for. */
export function railHeight(count: number, visible: number): number {
  return Math.max(1, Math.min((count - 1) * STEP, visible - 2 * MARGIN));
}

/** Which turn a pointer `y` px down the strip is nearest to. */
export function railIndexAt(y: number, height: number, count: number): number {
  if (count <= 1 || height <= 0) return 0;
  const at = Math.round((y / height) * (count - 1));
  return Math.max(0, Math.min(count - 1, at));
}

/** How wide a strip draws, by how far it is from the hovered one. */
export function railScale(distance: number | null): number {
  if (distance === 0) return 1;
  if (distance === 1) return 0.667;
  if (distance === 2) return 0.417;
  return 0.333;
}

/** The first words of `text`, on one line, cut at a word. */
export function turnTitle(text: string, limit = 120): string {
  let out = "";
  for (const word of text.split(/\s+/).filter(Boolean)) {
    const next = out ? `${out} ${word}` : word;
    if (next.length > limit) return out ? `${out}…` : `${clip(word, limit)}…`;
    out = next;
  }
  return out || "Message";
}

/** A reply as a preview reads it: its words, without the Markdown around them. */
export function plainPreview(text: string): string {
  return text
    .replace(/```[\s\S]*?(```|$)/g, " ")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(#{1,6}\s+|>\s?|[-*+]\s+|\d+\.\s+)/gm, "")
    .replace(/[`*_~]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

function RailStep({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Tooltip label={label} side="right">
      <button
        type="button"
        aria-label={label}
        disabled={disabled}
        onClick={onClick}
        className="flex size-5 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg disabled:opacity-40"
      >
        {children}
      </button>
    </Tooltip>
  );
}

export const TimelineRail = memo(function TimelineRail({
  turns,
  current,
  last,
  width,
  visible,
  offset = 0,
  onJump,
  replyOf,
}: {
  turns: Turn[];
  /** Index into `turns`: the turn being read. */
  current: number;
  /** The last turn on screen; `current` to this read brighter. */
  last: number;
  /** The transcript's width and the height of its visible part (above the composer). */
  width: number;
  visible: number;
  /** Where the visible part starts (under a floating header). */
  offset?: number;
  onJump: (row: number) => void;
  /** How a turn was answered, read only for the one hovered. */
  replyOf: (row: number) => string | undefined;
}) {
  const [active, setActive] = useState<number | null>(null);
  const strip = useRef<HTMLButtonElement>(null);
  const hit = railHitWidth(width);
  if (turns.length < 2 || visible <= 0 || hit <= 0) return null;
  const count = turns.length;
  const height = railHeight(count, visible);
  const shown = active !== null && active < count ? active : null;
  const hovered = shown === null ? null : turns[shown]!;
  const reply = hovered && replyOf(hovered.row);
  const percent = (index: number) => (index / (count - 1)) * 100;
  const indexAt = (e: MouseEvent) => {
    const rect = strip.current!.getBoundingClientRect();
    return railIndexAt(e.clientY - rect.top, rect.height, count);
  };
  const step = (delta: number) => {
    const next = turns[current + delta];
    if (next) onJump(next.row);
  };
  return (
    <nav
      aria-label="Thread timeline"
      className={cx(
        "group/rail pointer-events-none absolute left-0 z-10 w-18",
        (width - COLUMN) / 2 < PERSISTENT_GUTTER &&
          "opacity-0 transition-opacity duration-150 focus-within:opacity-100 hover:opacity-100",
      )}
      style={{ top: offset, height: visible }}
    >
      <div
        className="pointer-events-auto absolute -translate-y-1/2"
        style={{ top: "50%", left: LEFT, width: shown === null ? hit : hit + 320, height }}
      >
        <span className="-translate-x-1/2 absolute bottom-full left-1 mb-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover/rail:opacity-100">
          <RailStep label="Previous turn" disabled={current <= 0} onClick={() => step(-1)}>
            <ChevronUp size={14} />
          </RailStep>
        </span>
        <button
          ref={strip}
          type="button"
          aria-label={`Jump to message: ${hovered?.title ?? turns[current]?.title ?? "Message"}`}
          className="absolute inset-0 cursor-pointer rounded-xs outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
          onMouseMove={(e) => setActive(indexAt(e))}
          onMouseLeave={() => setActive(null)}
          onBlur={() => setActive(null)}
          onFocus={() => setActive((a) => a ?? current)}
          onClick={(e) => {
            const turn = turns[indexAt(e)];
            if (turn) onJump(turn.row);
            e.currentTarget.blur();
          }}
          onKeyDown={(e) => {
            const move = { ArrowDown: 1, ArrowUp: -1 }[e.key];
            if (move) {
              e.preventDefault();
              setActive((a) => Math.max(0, Math.min(count - 1, (a ?? current) + move)));
            } else if (e.key === "Home" || e.key === "End") {
              e.preventDefault();
              setActive(e.key === "Home" ? 0 : count - 1);
            } else if ((e.key === "Enter" || e.key === " ") && shown !== null) {
              e.preventDefault();
              onJump(turns[shown]!.row);
            }
          }}
        >
          {turns.map((turn, index) => {
            const inView = index >= current && index <= last;
            return (
              <span
                key={turn.row}
                aria-hidden
                data-in-view={inView}
                className={cx(
                  "-translate-y-1/2 pointer-events-none absolute left-0 h-0.5 w-6 origin-left rounded-full transition-[transform,background-color] duration-150",
                  inView ? "bg-fg/60" : index === shown ? "bg-fg/50" : "bg-fg/25",
                )}
                style={{
                  top: `${percent(index)}%`,
                  transform: `translateY(-50%) scaleX(${railScale(shown === null ? null : Math.abs(index - shown))})`,
                }}
              />
            );
          })}
          {hovered && (
            <span
              className="elevated pointer-events-none absolute left-8 z-20 block w-80 p-3 text-left"
              style={{
                top: `${percent(shown!)}%`,
                transform: `translateY(${shown === 0 ? "0%" : shown === count - 1 ? "-100%" : "-50%"})`,
              }}
            >
              <span className="block truncate font-medium text-callout leading-5">{hovered.title}</span>
              {reply && (
                <span className="mt-1 line-clamp-3 text-callout text-fg-muted leading-5">{reply}</span>
              )}
            </span>
          )}
        </button>
        <span className="-translate-x-1/2 absolute top-full left-1 mt-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover/rail:opacity-100">
          <RailStep label="Next turn" disabled={current >= count - 1} onClick={() => step(1)}>
            <ChevronDown size={14} />
          </RailStep>
        </span>
      </div>
    </nav>
  );
});
