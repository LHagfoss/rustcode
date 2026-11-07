// The thread's minimap: a strip per message you sent, in the gutter left of
// the chat's column, evenly spaced and squeezed to fit however long the thread
// gets. Strips of the turns on screen read brighter. Hovering widens the strips
// around the pointer and shows that turn's message and how it was answered;
// clicking goes there, and the chevrons step a turn at a time. It renders only
// when the turns or the ones on screen change, never per scroll frame.
//
// A turn a rewind can go back to carries a bead at its strip's left end, the
// head (the newest, the turn the thread is at) in the accent. The strip stays
// the one target: the bead is drawn in it, not beside it, so pointing at its
// height picks that turn as pointing anywhere on the strip does, and the card
// says which checkpoint it is. A card with a rewind to offer waits a moment
// before it goes and takes the pointer only then, so moving into it works and
// a click on the strip still goes to the turn.

import { ChevronDown, ChevronUp, History } from "lucide-react";
import { type MouseEvent, memo, type ReactNode, useEffect, useRef, useState } from "react";
import type { Checkpoint } from "../acp/v2";
import { clip } from "../lib/clip";
import { cx, Pill, Tooltip } from "./primitives";

export interface Turn {
  /** The user message's row in the transcript. */
  row: number;
  /** That message's id, which its checkpoint is paired with. */
  id: string;
  title: string;
}

/** A turn with a checkpoint, by its index in `turns`: the bead on that turn's strip. */
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
  /** The turns that have a checkpoint: each draws a bead, the last one the head, marked current. */
  nodes?: RailNode[];
  /** Rewind to a checkpoint: the card's button, when the thread can go back. */
  onRewind?: (checkpoint: Checkpoint) => void;
  /** The thread is running, so a rewind waits for it to stop. */
  busy?: boolean;
}) {
  const [active, setActive] = useState<number | null>(null);
  const strip = useRef<HTMLButtonElement>(null);
  const closing = useRef<ReturnType<typeof setTimeout> | null>(null);
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
  const checkpoint = hoveredNode?.checkpoint;
  const rewindHere =
    checkpoint && onRewind
      ? {
          turn: checkpoint.turn,
          go: () => {
            setActive(null);
            onRewind(checkpoint);
          },
        }
      : undefined;
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
        style={{ top: "50%", left: LEFT, width: shown === null ? hit : hit + 320, height }}
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
            keepOpen();
            setActive(indexAt(e));
          }}
          onMouseLeave={closeSoon}
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
          {nodes.map((node, index) => {
            const atHead = index === nodes.length - 1;
            return (
              <span
                key={`node-${node.checkpoint.turn}`}
                aria-hidden
                data-current={atHead}
                className={cx(
                  "-translate-x-1/2 -translate-y-1/2 pointer-events-none absolute left-0 rounded-full",
                  atHead ? "size-[7px] bg-accent" : "size-[5px] bg-fg/35",
                )}
                style={{ top: `${percent(node.at)}%` }}
              />
            );
          })}
        </button>
        {hovered && (
          <div
            className="elevated pointer-events-none absolute left-8 z-20 block w-80 p-3 text-left"
            style={{
              top: `${percent(shown!)}%`,
              transform: `translateY(${shown === 0 ? "0%" : shown === count - 1 ? "-100%" : "-50%"})`,
            }}
          >
            <span className="block truncate font-medium text-callout leading-5">{hovered.title}</span>
            {hoveredNode && (
              <span className="mt-1 block text-caption text-fg-faint tabular">
                Checkpoint · turn {hoveredNode.checkpoint.turn}
                {head?.at === shown ? " · current" : ""}
              </span>
            )}
            {reply && <span className="mt-1 line-clamp-3 text-callout text-fg-muted leading-5">{reply}</span>}
            {rewindHere && (
              <Pill
                size="sm"
                disabled={busy}
                onClick={rewindHere.go}
                onMouseEnter={keepOpen}
                onMouseLeave={closeSoon}
                aria-label={`Rewind to before turn ${rewindHere.turn}`}
                title={busy ? "Stop the turn first" : undefined}
                className="pointer-events-auto mt-2"
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
