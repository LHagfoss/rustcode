// An opened row's header stays under the glass header while what it opened is in view, so it can
// be closed from anywhere in it. Two kinds:
// - A turn's `Worked for …` fold and a run of tool calls open into rows of their own. The rows are
//   virtual (each its own box), so CSS `sticky` has no shared parent to stick in: a copy of each
//   header floats over the scroller, stacked (a group's under its turn's).
// - A tool call or a thought opens inside its own row: its header (`.rc-sticky-head`) is moved down
//   that row to just below the floating copies, and marked `data-stuck` while it is pinned so it
//   lifts off what scrolls under it.
import type { Virtualizer } from "@tanstack/react-virtual";
import { type RefObject, useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { cx } from "./primitives";
import { ToolGroupRow, TurnFoldRow } from "./Rows";
import type { Row } from "./Transcript";

type Stuck = { index: number; top: number };
type Stack = { items: Stuck[]; height: number; left: number; width: number };
const EMPTY: Stack = { items: [], height: 0, left: 0, width: 0 };

export function useStuckFold({
  rows,
  scrollRef,
  innerRef,
  virtualizer,
  topInset,
  toggleFold,
  pinned,
  size,
}: {
  rows: Row[];
  scrollRef: RefObject<HTMLDivElement | null>;
  innerRef: RefObject<HTMLDivElement | null>;
  virtualizer: Virtualizer<HTMLDivElement, Element>;
  topInset: number;
  toggleFold: (key: string) => void;
  pinned: RefObject<boolean>;
  size: { width: number; height: number };
}) {
  // Each opened fold with steps and each opened group, with the last row it opened.
  const ranges = useMemo(() => {
    const out: { index: number; last: number }[] = [];
    for (let i = 0; i < rows.length; i++) {
      const { fold, group } = rows[i]!;
      let j = i + 1;
      if (fold?.open && fold.steps) while (j < rows.length && rows[j]!.inFold) j++;
      else if (group?.open) while (j < rows.length && rows[j]!.inFold === group.key) j++;
      if (j - 1 > i) out.push({ index: i, last: j - 1 });
    }
    return out;
  }, [rows]);
  const boxes = useRef<(HTMLDivElement | null)[]>([]);
  // The offset each in-row header was moved by, so its own place can be read back from where it is.
  const moved = useRef(new Map<HTMLElement, number>());
  const [stack, setStack] = useState<Stack>(EMPTY);

  const updateStuck = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const top = el.scrollTop + topInset;
    const starts = virtualizer.measurementsCache;
    const items: Stuck[] = [];
    let offset = 0;
    for (const { index, last } of ranges) {
      const header = starts[index];
      if (!header) continue;
      const line = top + offset;
      const height = boxes.current[items.length]?.offsetHeight || 32;
      // Pinned once the header itself reaches the line (its row's foot less the bar's height).
      if (header.end - height > line) break;
      const end = starts[last]?.end;
      if (end === undefined || end <= line) continue;
      // Pushed up by the row after what it opened as that leaves, as a sticky header would be.
      const shift = Math.min(0, end - line - height);
      items.push({ index, top: offset + shift });
      offset += Math.max(0, height + shift);
    }
    // The lane's own box, so the copies line up with the rows whatever the scroller pads.
    const lane = innerRef.current?.getBoundingClientRect();
    const box = el.parentElement?.getBoundingClientRect();
    const next: Stack = items.length
      ? {
          items,
          height: offset,
          left: lane && box ? lane.left - box.left : 0,
          width: lane?.width ?? el.clientWidth,
        }
      : EMPTY;
    setStack((prev) => (JSON.stringify(prev) === JSON.stringify(next) ? prev : next));

    // In-row headers ride down their own row to the line, and stop at its foot. CSS `sticky` would
    // do this, but the rows sit under one translated box (Transcript) and Chromium places a sticky
    // box from its untranslated layout, so it pinned at the wrong place.
    // 8px down: the 4px between bars, and the 4px its bar reaches above it.
    const line = el.getBoundingClientRect().top + topInset + offset + 8;
    const heads = new Set(el.querySelectorAll<HTMLElement>(".rc-sticky-head"));
    for (const head of moved.current.keys()) {
      if (heads.has(head)) continue;
      head.style.transform = "";
      head.removeAttribute("data-stuck");
      moved.current.delete(head);
    }
    for (const head of heads) {
      const row = head.parentElement?.getBoundingClientRect();
      if (!row) continue;
      const natural = head.getBoundingClientRect().top - (moved.current.get(head) ?? 0);
      const by = Math.max(0, Math.min(line - natural, row.bottom - head.offsetHeight - natural));
      head.style.transform = by ? `translateY(${by}px)` : "";
      head.toggleAttribute("data-stuck", by > 0);
      if (by) moved.current.set(head, by);
      else moved.current.delete(head);
    }
  }, [ranges, topInset, virtualizer, scrollRef, innerRef]);

  const total = virtualizer.getTotalSize();
  // biome-ignore lint/correctness/useExhaustiveDependencies: a resize or a row opened or closed moves the rows under the view top
  useLayoutEffect(updateStuck, [updateStuck, size.width, size.height, total]);

  const close = useCallback(
    (index: number) => {
      const row = rows[index];
      const key = row?.fold?.key ?? row?.group?.key;
      if (!key) return;
      const el = scrollRef.current;
      const start = virtualizer.measurementsCache[index]?.start;
      const above = stack.items.findIndex((s) => s.index === index);
      const over = above > 0 ? stack.items[above]!.top : 0;
      toggleFold(key);
      // Closing from the copy brings the real header back where the copy was, rather than leaving
      // the view somewhere in the rows below.
      if (el && start !== undefined) {
        pinned.current = false;
        requestAnimationFrame(() => {
          el.scrollTop = Math.max(0, start - topInset - over);
        });
      }
    },
    [rows, stack, toggleFold, topInset, virtualizer, scrollRef, pinned],
  );

  const stuckHeader = stack.items.length ? (
    <div
      className="pointer-events-none absolute z-10"
      style={{ top: topInset, left: stack.left, width: stack.width }}
    >
      {stack.items.map((s, i) => {
        const row = rows[s.index];
        return (
          <div
            key={row?.key ?? s.index}
            ref={(el) => {
              boxes.current[i] = el;
            }}
            className="absolute inset-x-0"
            style={{ top: s.top, zIndex: stack.items.length - i }}
          >
            <div className="mx-auto w-full max-w-[var(--rc-chat-content-max-width)]">
              <div className={cx("rc-stuck pointer-events-auto mt-1", row?.fold ? "pt-2" : "py-1")}>
                {row?.fold ? (
                  <TurnFoldRow fold={row.fold} onToggle={() => close(s.index)} rule={false} />
                ) : row?.group ? (
                  <ToolGroupRow group={row.group} onToggle={() => close(s.index)} />
                ) : null}
              </div>
            </div>
          </div>
        );
      })}
    </div>
  ) : null;

  return { updateStuck, stuckHeader };
}
