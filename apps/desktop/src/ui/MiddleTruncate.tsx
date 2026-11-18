// Text cut in the middle at its real rendered width, so a branch keeps its start and its end
// ("feat/sidebar-…row-lines"). Adapted from T3 Code's `components/ui/middle-truncate.tsx`
// (https://github.com/pingdotgg/t3code, MIT License, Copyright (c) T3 Tools Inc.).

import { useLayoutEffect, useRef } from "react";
import { cx } from "./primitives";

/**
 * `value` in one line: a head that is clipped to the room it has and a tail kept whole, the
 * ellipsis drawn on the tail only when the head is cut. Short values render as one truncating span.
 */
export function MiddleTruncate({
  value,
  tail,
  title = true,
  className,
}: {
  value: string;
  /** Characters the tail keeps; by default the last path segment, else the last word or 10. */
  tail?: number;
  title?: boolean;
  className?: string;
}) {
  const raw = splitForMiddleTruncate(value, tail);
  const split = raw && collapseSegments(raw);
  const headRef = useRef<HTMLSpanElement>(null);
  const head = split?.head;
  useLayoutEffect(() => {
    const element = headRef.current;
    // A new head can keep the same box size, so refit here, not only on resize.
    if (!element || head === undefined) return;
    scheduleFit(element);
    const observer = new ResizeObserver(() => scheduleFit(element));
    observer.observe(element);
    return () => observer.disconnect();
  }, [head]);
  return (
    <span
      title={title ? value : undefined}
      className={cx("inline-flex min-w-0 max-w-full overflow-hidden whitespace-nowrap", className)}
    >
      {split ? (
        <>
          {/* The tail carries the ellipsis so it sits flush; a CSS ellipsis leaves a ragged gap. */}
          <span ref={headRef} className="peer min-w-0 overflow-hidden whitespace-pre">
            {split.head}
          </span>
          <span className="shrink-0 whitespace-pre peer-data-[overflow]:before:content-['…']">
            {split.tail}
          </span>
        </>
      ) : (
        // Keyed so React does not reuse the head node and its imperative clip.
        <span key="whole" className="min-w-0 truncate">
          {value}
        </span>
      )}
    </span>
  );
}

// Heads queued in one commit or resize fit together in a microtask, before paint: all reads,
// then all writes, instead of one forced layout per row.
const pendingFits = new Set<HTMLElement>();

function scheduleFit(head: HTMLElement) {
  if (pendingFits.size === 0) queueMicrotask(fitPendingHeads);
  pendingFits.add(head);
}

// An overflowing head is cut after its last whole character: clip the partial glyph and slide
// the tail left over the slack. Neither changes layout, so the observed size stays true.
function fitPendingHeads() {
  const heads = [...pendingFits].filter((head) => head.isConnected);
  pendingFits.clear();
  const overflowing = heads.map(
    (head) => head.scrollWidth > head.clientWidth && head.firstChild instanceof Text,
  );
  heads.forEach((head, index) => {
    head.toggleAttribute("data-overflow", overflowing[index]);
  });
  const slacks = heads.map((head, index) => (overflowing[index] ? measureSlack(head) : 0));
  heads.forEach((head, index) => {
    const slack = slacks[index] ?? 0;
    head.style.clipPath = slack > 0 ? `inset(-0.25em ${slack}px -0.25em 0)` : "";
    const next = head.nextElementSibling;
    if (next instanceof HTMLElement) next.style.translate = slack > 0 ? `-${slack}px 0` : "";
  });
}

function measureSlack(head: HTMLElement) {
  const text = head.firstChild as Text;
  const range = document.createRange();
  const left = head.getBoundingClientRect().left;
  const width = head.clientWidth;
  const widthTo = (end: number) => {
    range.setStart(text, 0);
    range.setEnd(text, end);
    return range.getBoundingClientRect().right - left;
  };
  let fits = 0;
  let over = text.length;
  while (over - fits > 1) {
    const mid = (fits + over) >> 1;
    if (widthTo(mid) <= width) fits = mid;
    else over = mid;
  }
  if (/[\uD800-\uDBFF]/.test(text.data[fits - 1] ?? "")) fits -= 1;
  return Math.max(0, width - widthTo(fits));
}

// The segments render `pre` so the space at the cut survives; collapse what CSS would have.
function collapseSegments(split: { head: string; tail: string }) {
  const collapse = (text: string) => text.replace(/[ \t\n\r\f]+/g, " ");
  const head = collapse(split.head).replace(/^ /, "");
  const tail = collapse(split.tail).replace(/ $/, "");
  return { head, tail: head.endsWith(" ") ? tail.replace(/^ /, "") : tail };
}

const DEFAULT_TAIL = 10;
const MAX_SEGMENT_TAIL = 16;
const MIN_WORD_TAIL = 4;
const WORD_BREAK = /[-_./]/;

/** Where `value` splits into a head that may be cut and a tail kept whole; null when too short. */
export function splitForMiddleTruncate(value: string, tail?: number): { head: string; tail: string } | null {
  // By code point, so a surrogate pair is never cut into broken glyphs.
  const chars = Array.from(value);
  let keep = tail ?? DEFAULT_TAIL;
  if (tail === undefined) {
    const slash = chars.lastIndexOf("/");
    if (slash > 0 && slash < chars.length - 1) {
      const segment = chars.length - slash - 1;
      keep = segment <= MAX_SEGMENT_TAIL ? segment : DEFAULT_TAIL;
    }
  }
  if (keep <= 0 || chars.length <= keep + 4) return null;
  let cut = chars.length - keep;
  // Start an automatic tail on a word ("machine", not "er-machine"): a shorter tail first, so
  // the head stays readable, a longer one as the fallback.
  if (tail === undefined && !WORD_BREAK.test(chars[cut - 1] ?? "")) {
    const startsWord = (index: number) => WORD_BREAK.test(chars[index - 1] ?? "");
    let snapped = -1;
    for (let index = cut + 1; index <= chars.length - MIN_WORD_TAIL; index++) {
      if (startsWord(index)) {
        snapped = index;
        break;
      }
    }
    for (let index = cut - 1; snapped < 0 && index >= chars.length - MAX_SEGMENT_TAIL && index > 4; index--) {
      if (startsWord(index)) snapped = index;
    }
    if (snapped > 0) cut = snapped;
  }
  return { head: chars.slice(0, cut).join(""), tail: chars.slice(cut).join("") };
}
