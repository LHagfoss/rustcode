// A long paste kept out of the field: the chip above the composer, and the one a sent message keeps
// in the transcript. The chip says how much it holds; opening it shows the text itself, so the
// message reads as the question and not as the log that came with it.

import { ClipboardPaste, X } from "lucide-react";
import { pasteLabel } from "../lib/paste";
import { CardPopover } from "./cardPopover";
import { cx } from "./primitives";

export function PasteChip({
  text,
  onRemove,
  className,
}: {
  text: string;
  /** Removing it from the message; a sent message's chip has none. */
  onRemove?: () => void;
  className?: string;
}) {
  const label = pasteLabel(text);
  return (
    <span
      className={cx(
        "flex h-[26px] shrink-0 items-center gap-1.5 rounded-full bg-fill-subtle pr-1.5 pl-2 text-callout rc-in",
        className,
      )}
    >
      <ClipboardPaste size={12} className="shrink-0 text-fg-muted" />
      <CardPopover
        role="dialog"
        label={label}
        tooltip="Show the pasted text"
        // A wide-enough panel for a log's lines, and never wider than the window.
        className="w-[min(560px,calc(100vw-32px))] p-0"
        trigger={
          <button type="button" className="max-w-[200px] truncate">
            {label}
          </button>
        }
      >
        <pre className="selectable max-h-[min(50vh,340px)] overflow-auto px-3 py-2 font-mono text-caption whitespace-pre-wrap break-words">
          {text}
        </pre>
      </CardPopover>
      {onRemove && (
        <button
          type="button"
          aria-label={`Remove ${label}`}
          onClick={onRemove}
          className="shrink-0 text-fg-faint hover:text-fg"
        >
          <X size={12} />
        </button>
      )}
    </span>
  );
}
