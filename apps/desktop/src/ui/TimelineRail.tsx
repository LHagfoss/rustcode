// The thread's minimap: a strip per message you sent, in the gutter left of
// the chat's column, evenly spaced and squeezed to fit however long the thread
// gets. Strips of the turns on screen read brighter. Hovering widens the strips
// around the pointer and shows that turn's message and how it was answered;
// clicking goes there, and the chevrons step a turn at a time. It renders only
// when the turns or the ones on screen change, never per scroll frame.
//
// A turn a rewind can go back to is named in its hover card instead of marked on
// the strip: the card says which checkpoint it is (the head, the newest, the turn
// the thread is at, is current) and offers the rewind the turn's Changes chip
// does, so the strips stay plain marks with no dot on them. The card sits clear
// of the strip rather than widening it, waits a moment before it goes and then
// takes the pointer itself, holding the turn it shows while the pointer is
// inside: moving into it never re-picks a turn, and a click on its body still
// goes to the turn.

import { ChevronDown, ChevronUp, History } from "lucide-react";
import { type MouseEvent, memo, type ReactNode, useEffect, useRef, useState } from "react";
import type { Checkpoint } from "../acp/v2";
import { clip } from "../lib/clip";
import { cx, Pill, Tooltip } from "./primitives";
import { rewindNote } from "./TurnChanges";

export interface Turn {
  /** The user message's row in the transcript. */
  row: number;
  /** That message's id, which its checkpoint is paired with. */
  id: string;
  title: string;
}

/** A turn with a checkpoint, by its index in `turns`: the card names it and offers the rewind. */
export interface RailNode {
  at: number;
  /** The checkpoint a rewind from that turn goes back to. */
  checkpoint: Checkpoint;
}

/** Between strips, while they fit. */
const STEP = 8;
/** Kept free above and below the strip for the chevrons. */
const MARGIN = 40;
/** Where the strip starts in the gutter. */
const LEFT = 12;
/** The card's gap from the strip, and how wide it is: it sits clear of the strip's hit area. */
const CARD_GAP = 8;
const CARD_WIDTH = 320;
/** How wide a strip draws (`w-6`), so the card never covers one where the gutter is narrow. */
const STRIP_WIDTH = 24;
/** How long a card that can be pointed at stays while the pointer crosses to it. */
const HOVER_INTENT = 160;
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

/**
 * The card's box, right of the strip's hit area and of the strips it draws (`left` never lands inside
 * either): pointing at the card is pointing at the card, not at a turn, so the turn it shows cannot
 * change under the pointer.
 */
export function cardBox(hit: number): { left: number; width: number } {
  return { left: Math.max(hit, STRIP_WIDTH) + CARD_GAP, width: CARD_WIDTH };
}

/**
 * The card's rewind, when that strip has a checkpoint and the thread can go back: disabled while the
 * thread runs, and saying what the click does, since a checkpointed engine goes back without a confirm.
 */
export function rewindOffer(
  checkpoint: Checkpoint | undefined,
  offered: boolean,
  { busy, keep }: { busy: boolean; keep: boolean },
): { turn: number; disabled: boolean; note: string } | undefined {
  if (!checkpoint || !offered) return undefined;
  return {
    turn: checkpoint.turn,
    disabled: busy,
    note: busy ? "Stop the turn first" : rewindNote(true, keep, checkpoint.turn),
  };
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

/** How the rail's button is named: the turn, plus the checkpoint it carries when it has one. */
export function railLabel(title: string, checkpoint: number | undefined, head: boolean): string {
  const at = checkpoint === undefined ? "" : `, checkpoint turn ${checkpoint}${head ? ", current" : ""}`;
  return `Jump to message: ${title}${at}`;
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
  nodes = [],
  onRewind,
  keepsAttempts = false,
  busy = false,
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
  /** The turns that have a checkpoint: the last one is the head, which the card marks current. */
  nodes?: RailNode[];
  /** Rewind to a checkpoint: the card's button, when the thread can go back. */
  onRewind?: (checkpoint: Checkpoint) => void;
  /** The engine keeps the state a rewind abandons, so a rewind here needs no confirm. */
  keepsAttempts?: boolean;
  /** The thread is running, so a rewind waits for it to stop. */
  busy?: boolean;
}) {
  const [active, setActive] = useState<number | null>(null);
  const strip = useRef<HTMLButtonElement>(null);
  const closing = useRef<ReturnType<typeof setTimeout> | null>(null);
  // The pointer is on the card: the turn it shows holds still and the strip behind it ignores the pointer.
  const inCard = useRef(false);
  // A focus the rail asked for itself (Escape, a rewind) must not put the card back.
  const quietFocus = useRef(false);
  const hit = railHitWidth(width);
  const keepOpen = () => {
    if (closing.current !== null) clearTimeout(closing.current);
    closing.current = null;
  };
  // The card sits a gap right of the strip: a short wait lets the pointer cross it.
  const closeSoon = () => {
    keepOpen();
    closing.current = setTimeout(() => {
      closing.current = null;
      setActive(null);
    }, HOVER_INTENT);
  };
  // Putting focus on the strip without opening the card on the way.
  const focusStrip = () => {
    const element = strip.current;
    if (!element) return;
    quietFocus.current = document.activeElement !== element;
    element.focus();
  };
  // A card that is closing when the rail goes does not come back.
  useEffect(
    () => () => {
      if (closing.current !== null) clearTimeout(closing.current);
    },
    [],
  );
  if (turns.length < 2 || visible <= 0 || hit <= 0) return null;
  const count = turns.length;
  const height = railHeight(count, visible);
  const shown = active !== null && active < count ? active : null;
  const hovered = shown === null ? null : turns[shown]!;
  const reply = hovered && replyOf(hovered.row);
  const nodeAt = (index: number) => nodes.find((node) => node.at === index);
  const head = nodes.at(-1);
  const hoveredNode = shown === null ? undefined : nodeAt(shown);
  const box = cardBox(hit);
  const rewind = rewindOffer(hoveredNode?.checkpoint, onRewind !== undefined, {
    busy,
    keep: keepsAttempts,
  });
  const goRewind = () => {
    if (!hoveredNode || !onRewind) return;
    inCard.current = false;
    setActive(null);
    // The button goes with the card, so the focus lands back on the strip, where the keyboard was.
    focusStrip();
    onRewind(hoveredNode.checkpoint);
  };
  // The turn Enter goes to, so the label says what pressing it does, checkpoint and all.
  const target = shown ?? current;
  const targetTurn = turns[target];
  const targetNode = nodeAt(target);
  const ariaLabel = targetTurn
    ? railLabel(targetTurn.title, targetNode?.checkpoint.turn, head?.at === target)
    : "Jump to message";
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
        style={{ top: "50%", left: LEFT, width: hit, height }}
        onKeyDown={(e) => {
          if (e.key !== "Escape" || shown === null) return;
          e.preventDefault();
          e.stopPropagation();
          inCard.current = false;
          setActive(null);
          focusStrip();
        }}
        onBlur={(e) => {
          // Focus moving between the strip, the card and the chevrons keeps the card; leaving ends it.
          if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
          keepOpen();
          setActive(null);
        }}
      >
        <span className="-translate-x-1/2 absolute bottom-full left-1 mb-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover/rail:opacity-100">
          <RailStep label="Previous turn" disabled={current <= 0} onClick={() => step(-1)}>
            <ChevronUp size={14} />
          </RailStep>
        </span>
        <button
          ref={strip}
          type="button"
          aria-label={ariaLabel}
          className="absolute inset-0 cursor-pointer rounded-xs outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
          onMouseMove={(e) => {
            if (inCard.current) return;
            keepOpen();
            setActive(indexAt(e));
          }}
          onMouseLeave={closeSoon}
          onFocus={() => {
            if (quietFocus.current) {
              quietFocus.current = false;
              return;
            }
            setActive((a) => a ?? current);
          }}
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
        </button>
        {hovered && (
          <div
            className="elevated pointer-events-auto absolute z-20 block p-3 text-left"
            style={{
              left: box.left,
              width: box.width,
              top: `${percent(shown!)}%`,
              transform: `translateY(${shown === 0 ? "0%" : shown === count - 1 ? "-100%" : "-50%"})`,
            }}
            onMouseEnter={() => {
              // Pointing here freezes the turn: the card cannot chase the pointer it is under.
              inCard.current = true;
              keepOpen();
            }}
            onMouseLeave={() => {
              inCard.current = false;
              closeSoon();
            }}
            onClick={() => onJump(hovered.row)}
          >
            <span aria-hidden className="block truncate font-medium text-callout leading-5">
              {hovered.title}
            </span>
            {hoveredNode && (
              <span aria-hidden className="mt-1 block text-caption text-fg-faint tabular">
                Checkpoint · turn {hoveredNode.checkpoint.turn}
                {head?.at === shown ? " · current" : ""}
              </span>
            )}
            {reply && (
              <span aria-hidden className="mt-1 line-clamp-3 text-callout text-fg-muted leading-5">
                {reply}
              </span>
            )}
            {rewind && (
              <Pill
                size="sm"
                disabled={rewind.disabled}
                onClick={(e) => {
                  e.stopPropagation();
                  goRewind();
                }}
                aria-label={`Rewind to before turn ${rewind.turn}`}
                title={rewind.note}
                className="mt-2"
              >
                <History size={12} /> Rewind here
              </Pill>
            )}
          </div>
        )}
        <span className="-translate-x-1/2 absolute top-full left-1 mt-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover/rail:opacity-100">
          <RailStep label="Next turn" disabled={current >= count - 1} onClick={() => step(1)}>
            <ChevronDown size={14} />
          </RailStep>
        </span>
      </div>
    </nav>
  );
});
