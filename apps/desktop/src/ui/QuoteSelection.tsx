import { type RefObject, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { quoteMarkdown } from "../lib/quoteSelection";
import { appendToComposer } from "../state/composerInsert";
import { MessageSquareQuote } from "./icons";
import { Pill } from "./primitives";

/** Marks the part of a reply whose text can be quoted into the composer. */
export const QUOTE_SOURCE = "data-quote-source";

const CHIP_GAP = 8;
const CHIP_HEIGHT = 32;

interface Marked {
  quote: string;
  x: number;
  y: number;
}

function sourceOf(node: Node | null): Element | null {
  const el = node instanceof Element ? node : node?.parentElement;
  return el?.closest(`[${QUOTE_SOURCE}]`) ?? null;
}

/** What is marked inside one reply in `container`, with where the chip goes; null otherwise. */
function readSelection(container: HTMLElement): Marked | null {
  const selection = document.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const source = sourceOf(selection.anchorNode);
  if (!source || source !== sourceOf(selection.focusNode) || !container.contains(source)) return null;
  const quote = quoteMarkdown(selection.toString());
  if (!quote) return null;
  const rect = selection.getRangeAt(0).getBoundingClientRect();
  const bounds = container.getBoundingClientRect();
  if (rect.bottom < bounds.top || rect.top > bounds.bottom) return null;
  const above = rect.top - CHIP_GAP - CHIP_HEIGHT;
  return {
    quote,
    x: Math.min(Math.max(rect.left + rect.width / 2, bounds.left + 60), bounds.right - 60),
    y: above >= bounds.top ? above : Math.min(rect.bottom + CHIP_GAP, bounds.bottom - CHIP_HEIGHT),
  };
}

/**
 * A Quote chip over text marked in a reply: pressing it puts the text in the composer as a block
 * quote, after the draft, so the user can ask about exactly that part. It follows the selection,
 * and goes away when the selection does, on scroll and on Escape.
 */
export function QuoteSelection({ container }: { container: RefObject<HTMLElement | null> }) {
  const [marked, setMarked] = useState<Marked | null>(null);
  useEffect(() => {
    const el = container.current;
    if (!el) return;
    let frame = 0;
    const read = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setMarked(readSelection(el)));
    };
    const hide = () => setMarked(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") hide();
    };
    document.addEventListener("selectionchange", read);
    el.addEventListener("scroll", hide, { passive: true });
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", hide);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("selectionchange", read);
      el.removeEventListener("scroll", hide);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", hide);
    };
  }, [container]);
  if (!marked) return null;
  return createPortal(
    <div
      data-quote-chip
      className="elevated fixed z-50 -translate-x-1/2 rounded-full p-0.5"
      style={{ left: marked.x, top: marked.y }}
    >
      <Pill
        quiet
        size="md"
        aria-label="Quote in the composer"
        onPointerDown={(e) => e.preventDefault()}
        onClick={() => {
          appendToComposer(`${marked.quote}\n\n`);
          document.getSelection()?.removeAllRanges();
          setMarked(null);
        }}
      >
        <MessageSquareQuote size={13} />
        Quote
      </Pill>
    </div>,
    document.body,
  );
}
