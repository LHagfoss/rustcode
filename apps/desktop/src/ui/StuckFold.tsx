// An opened turn's `Worked for …` header stays under the glass header while its steps are in view,
// so it can be closed from anywhere in them. The transcript's rows are virtual (each its own box),
// so CSS `sticky` has no shared parent to stick in: a copy floats over the scroller instead.
import type { Virtualizer } from "@tanstack/react-virtual";
import { type RefObject, useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { TurnFoldRow } from "./Rows";
import type { Row } from "./Transcript";

type Stuck = { index: number; shift: number; left: number; width: number };

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
  // Each opened fold with steps, and the last row of those steps.
  const ranges = useMemo(() => {
    const out: { index: number; last: number }[] = [];
    for (let i = 0; i < rows.length; i++) {
      const fold = rows[i]!.fold;
      if (!fold?.open || !fold.steps) continue;
      let j = i + 1;
      while (j < rows.length && rows[j]!.inFold) j++;
      if (j - 1 > i) out.push({ index: i, last: j - 1 });
    }
    return out;
  }, [rows]);
  const boxRef = useRef<HTMLDivElement>(null);
  const [stuck, setStuck] = useState<Stuck | null>(null);

  const updateStuck = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const top = el.scrollTop + topInset;
    const starts = virtualizer.measurementsCache;
    const height = boxRef.current?.offsetHeight || 32;
    let next: Stuck | null = null;
    for (const { index, last } of ranges) {
      const header = starts[index];
      if (!header || header.end > top) break;
      const end = starts[last]?.end;
      if (end === undefined || end <= top) continue;
      // The lane's own box, so the copy lines up with the rows whatever the scroller pads.
      const lane = innerRef.current?.getBoundingClientRect();
      const box = el.parentElement?.getBoundingClientRect();
      next = {
        index,
        // Pushed up by the row after the steps as they leave, as a sticky header would be.
        shift: Math.min(0, end - top - height),
        left: lane && box ? lane.left - box.left : 0,
        width: lane?.width ?? el.clientWidth,
      };
    }
    setStuck((prev) =>
      prev?.index === next?.index &&
      prev?.shift === next?.shift &&
      prev?.left === next?.left &&
      prev?.width === next?.width
        ? prev
        : next,
    );
  }, [ranges, topInset, virtualizer, scrollRef, innerRef]);

  const total = virtualizer.getTotalSize();
  // biome-ignore lint/correctness/useExhaustiveDependencies: a resize or a fold opened or closed moves the rows under the view top
  useLayoutEffect(updateStuck, [updateStuck, size.width, size.height, total]);

  const fold = stuck ? rows[stuck.index]?.fold : undefined;
  const close = useCallback(() => {
    if (!stuck || !fold) return;
    const el = scrollRef.current;
    const start = virtualizer.measurementsCache[stuck.index]?.start;
    toggleFold(fold.key);
    // Closing from the copy brings the real header back where the copy was, rather than leaving
    // the view somewhere in the turns below.
    if (el && start !== undefined) {
      pinned.current = false;
      requestAnimationFrame(() => {
        el.scrollTop = Math.max(0, start - topInset);
      });
    }
  }, [stuck, fold, toggleFold, topInset, virtualizer, scrollRef, pinned]);

  const stuckHeader =
    stuck && fold ? (
      <div
        ref={boxRef}
        className="pointer-events-none absolute z-10 overflow-hidden"
        style={{ top: topInset, left: stuck.left, width: stuck.width }}
      >
        <div className="pointer-events-auto bg-bg pt-2" style={{ transform: `translateY(${stuck.shift}px)` }}>
          <div className="mx-auto w-full max-w-[var(--rc-chat-content-max-width)]">
            <TurnFoldRow fold={fold} onToggle={close} />
          </div>
        </div>
      </div>
    ) : null;

  return { updateStuck, stuckHeader };
}
